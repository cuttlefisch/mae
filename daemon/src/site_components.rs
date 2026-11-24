//! The live site's slot components (ADR-119 D18, Amendment 4 H1/H2): each
//! KB's operator directory, read and checked once at startup, rendered into
//! every page's slots, and its behaviours and stylesheets served under
//! content-addressed names.
//!
//! The engine (`mae_site::components`) does no I/O; this module is its
//! loader. `daemon.toml` names a directory per KB (`[oauth.site_components]`);
//! [`SiteComponents::load`] reads `components.toml` and exactly the files it
//! names, and hands them to `Components::load`. Every problem, in every KB,
//! is returned at once, each naming the KB, the component and the reason, and
//! any problem refuses to start the daemon (H1, the live P-157). A missing
//! directory is a problem too: a typo in a path never silently serves a site
//! without its components.
//!
//! A component that fails while rendering leaves its slot empty for that page
//! (D18), and the failure is logged at most once per
//! [`RENDER_ERROR_LOG_INTERVAL`] per component, never once per request.
//!
//! @ai-caution: [security] Component output reaches a page only through
//! `Components::render_slot`/`render_page`, which sanitise it. Never write a
//! template's raw output, or a failed render's partial output, into a page.

use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mae_site::components::{
    AssetKind, Components, PageContext, PageData, SiteContext, Slot, MAX_DATA_BYTES, MAX_FILE_BYTES,
};
use mae_site::Addressing;

/// The manifest's file name in a component directory (H1).
pub const MANIFEST: &str = "components.toml";

/// How often one component's render failure is logged.
///
/// A failing component fails on every page that renders its slot, which is
/// every page request: logged each time, one reader paging through a site
/// would write a line per click, and a crawler thousands. Once a minute per
/// component bounds that to 1,440 lines a day for the worst component, while
/// the first failure after a restart (or a data change) is still logged at
/// once, and every later line says how many failures it stands for.
/// `rate_limit_logs_once_per_interval_with_the_count_it_stands_for` pins it.
pub const RENDER_ERROR_LOG_INTERVAL: Duration = Duration::from_secs(60);

/// Every KB's loaded components, and the assets they serve.
#[derive(Debug, Default)]
pub struct SiteComponents {
    by_kb: BTreeMap<String, Arc<KbComponents>>,
    /// By served file name (`c-<hash>.<ext>`).
    assets: BTreeMap<String, ComponentAsset>,
}

/// A behaviour script or stylesheet, served at `/site-assets/<name>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentAsset {
    pub content_type: &'static str,
    pub body: Arc<str>,
}

/// One KB's components.
pub struct KbComponents {
    kb_id: String,
    components: Components,
    /// Every asset the KB's components name, deduplicated by file, in
    /// manifest order: what every page of the KB links.
    links: Vec<(AssetKind, String)>,
    errors: ErrorLog,
}

impl std::fmt::Debug for KbComponents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KbComponents")
            .field("kb_id", &self.kb_id)
            .field("components", &self.components)
            .finish()
    }
}

/// One page's component output, ready for the shell (`site::Chrome`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slots {
    /// Each slot's sanitised HTML, in [`Slot::ALL`] order; `""` is empty.
    html: [String; 7],
    /// Stylesheet hrefs, linked after the base stylesheet.
    pub stylesheets: Vec<String>,
    /// Behaviour hrefs, loaded as module scripts after the router.
    pub scripts: Vec<String>,
    /// What the shell writes on `<main>` for a behaviour's `ctx`.
    pub page: PageAttrs,
}

/// The page and site a behaviour's `ctx` reads (D18), from `<main>`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageAttrs {
    pub kb: String,
    /// The site's base path (`/kb/<kb-id>`).
    pub base: String,
    /// This page's address.
    pub url: String,
    /// The page's file-level ID; `None` on a generated page.
    pub id: Option<String>,
}

impl Slots {
    /// The sanitised HTML of `slot` on this page; `""` when it is empty.
    pub fn get(&self, slot: Slot) -> &str {
        Slot::ALL
            .iter()
            .position(|s| *s == slot)
            .map_or("", |i| self.html[i].as_str())
    }
}

impl SiteComponents {
    /// Read and load every configured directory. Every problem in every KB is
    /// returned, each a line naming the KB, the component and the reason.
    pub fn load(dirs: &BTreeMap<String, PathBuf>) -> Result<SiteComponents, Vec<String>> {
        let mut out = SiteComponents::default();
        let mut problems = Vec::new();
        for (kb_id, dir) in dirs {
            match load_dir(kb_id, dir) {
                Ok((kb, assets)) => {
                    out.assets.extend(assets);
                    out.by_kb.insert(kb_id.clone(), Arc::new(kb));
                }
                Err(p) => problems.extend(p),
            }
        }
        if problems.is_empty() {
            Ok(out)
        } else {
            Err(problems)
        }
    }

    /// The components of `kb_id`, if it has any.
    pub fn for_kb(&self, kb_id: &str) -> Option<Arc<KbComponents>> {
        self.by_kb.get(kb_id).cloned()
    }

    /// The component asset served at `path` (`/site-assets/<name>`, exactly).
    pub fn asset(&self, path: &str) -> Option<&ComponentAsset> {
        let name = path.strip_prefix(mae_site::assets::PREFIX)?;
        self.assets.get(name)
    }

    pub fn is_empty(&self) -> bool {
        self.by_kb.is_empty()
    }
}

/// One KB's directory: its components and the assets they serve.
fn load_dir(
    kb_id: &str,
    dir: &Path,
) -> Result<(KbComponents, BTreeMap<String, ComponentAsset>), Vec<String>> {
    let at = |reason: String| vec![format!("KB `{kb_id}` ({}): {reason}", dir.display())];
    if Addressing::live(kb_id).is_err() {
        return Err(at(
            "is not a KB ID ([a-z0-9][a-z0-9-], at most 63)".to_string()
        ));
    }
    if !dir.is_dir() {
        return Err(at(
            "the component directory does not exist or is not a directory".to_string(),
        ));
    }
    let manifest = match read_capped(&dir.join(MANIFEST), MAX_FILE_BYTES) {
        Ok(Some(text)) => text,
        Ok(None) => return Err(at(format!("{MANIFEST} is not in the directory"))),
        Err(why) => return Err(at(format!("{MANIFEST} {why}"))),
    };
    let mut problems = Vec::new();
    let mut files = BTreeMap::new();
    for name in Components::named_files(&manifest) {
        match read_capped(&dir.join(&name), MAX_FILE_BYTES.max(MAX_DATA_BYTES)) {
            Ok(Some(text)) => {
                files.insert(name, text);
            }
            // Absent: the load names the component that needs it.
            Ok(None) => {}
            Err(why) => problems.extend(at(format!("`{name}` {why}"))),
        }
    }
    let components = match Components::load(&manifest, &files) {
        Ok(c) => c,
        Err(errors) => {
            problems.extend(errors.iter().flat_map(|e| at(e.to_string())));
            return Err(problems);
        }
    };
    if !problems.is_empty() {
        return Err(problems);
    }
    let (links, assets) = asset_table(&components, &files);
    Ok((
        KbComponents {
            kb_id: kb_id.to_string(),
            components,
            links,
            errors: ErrorLog::default(),
        },
        assets,
    ))
}

/// The text of `path`, `None` when it does not exist, or why it cannot be
/// used. Reads at most `cap + 1` bytes, so the engine can report a file
/// over its limit without the daemon reading all of it.
fn read_capped(path: &Path, cap: usize) -> Result<Option<String>, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot be read: {e}")),
    };
    match file.metadata() {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err("is not a regular file".into()),
        Err(e) => return Err(format!("cannot be read: {e}")),
    }
    let mut bytes = Vec::new();
    file.take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("cannot be read: {e}"))?;
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| "is not UTF-8 text".into())
}

/// Each named asset's served name and body, and the hrefs every page links.
fn asset_table(
    components: &Components,
    files: &BTreeMap<String, String>,
) -> (Vec<(AssetKind, String)>, BTreeMap<String, ComponentAsset>) {
    let mut links = Vec::new();
    let mut assets = BTreeMap::new();
    let mut seen = std::collections::BTreeSet::new();
    for (_, kind, file) in components.assets() {
        if !seen.insert(file.to_string()) {
            continue;
        }
        let body = files.get(file).map_or("", String::as_str);
        let name = asset_name(kind, body);
        links.push((kind, format!("{}{name}", mae_site::assets::PREFIX)));
        let content_type = match kind {
            AssetKind::Script => "text/javascript; charset=utf-8",
            AssetKind::Stylesheet => "text/css; charset=utf-8",
        };
        assets.insert(
            name,
            ComponentAsset {
                content_type,
                body: Arc::from(body),
            },
        );
    }
    (links, assets)
}

/// A component asset's served name: `c-<first 32 hex digits of its
/// SHA-256>.js` or `.css`. Derived from the bytes alone, so it names neither
/// the component nor its file, and a change always reaches a browser under a
/// new name (the assets are cached as immutable).
pub fn asset_name(kind: AssetKind, body: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(body.as_bytes());
    let ext = match kind {
        AssetKind::Script => "js",
        AssetKind::Stylesheet => "css",
    };
    format!("c-{}.{ext}", hex::encode(&digest[..16]))
}

impl KbComponents {
    /// The view's settings for this KB's components (P-154): the locale, the
    /// base path, the KB, its emitted pages' addresses and its published pages.
    pub fn site_context(
        &self,
        locale: &str,
        addressing: &Addressing,
        published: BTreeMap<String, String>,
    ) -> Arc<SiteContext> {
        Arc::new(SiteContext {
            locale: locale.to_string(),
            base_path: base_path(&self.kb_id),
            kb_id: self.kb_id.clone(),
            pages: self.components.page_hrefs(addressing),
            published: Arc::new(published),
        })
    }

    /// Every slot of one page. A failing component leaves its slot without
    /// its output, and is logged (rate-limited).
    pub fn render(&self, page: PageData, site: &Arc<SiteContext>) -> Slots {
        let attrs = PageAttrs {
            kb: self.kb_id.clone(),
            base: site.base_path.clone(),
            url: page.url.clone(),
            id: (!page.id.is_empty()).then(|| page.id.clone()),
        };
        let ctx = PageContext {
            page,
            site: Arc::clone(site),
        };
        let mut html: [String; 7] = Default::default();
        for (i, slot) in Slot::ALL.into_iter().enumerate() {
            let (out, errors) = self.components.render_slot(slot, &ctx);
            for e in errors {
                self.log_failure(&e.component, &e.reason, Instant::now());
            }
            html[i] = out;
        }
        Slots {
            html,
            stylesheets: self.hrefs(AssetKind::Stylesheet),
            scripts: self.hrefs(AssetKind::Script),
            page: attrs,
        }
    }

    /// The page `name` a component emits, as `(title, sanitised HTML)`;
    /// `None` when no component emits it or its render failed (logged).
    pub fn render_page(&self, name: &str, site: &Arc<SiteContext>) -> Option<(String, String)> {
        let page = self
            .components
            .pages()
            .into_iter()
            .find(|p| p.name == name)?;
        match self.components.render_page(name, site)? {
            Ok(html) => Some((page.title, html)),
            Err(e) => {
                self.log_failure(&e.component, &e.reason, Instant::now());
                None
            }
        }
    }

    fn hrefs(&self, kind: AssetKind) -> Vec<String> {
        self.links
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, h)| h.clone())
            .collect()
    }

    fn log_failure(&self, component: &str, reason: &str, now: Instant) {
        if let Some(suppressed) = self.errors.should_log(component, now) {
            tracing::warn!(
                kb = %self.kb_id,
                component,
                suppressed,
                "site component failed to render; its slot is empty: {reason}"
            );
        }
    }
}

/// The live site's base path for `kb_id`: the root its pages live under.
pub fn base_path(kb_id: &str) -> String {
    format!("/kb/{kb_id}")
}

/// When each component's failure was last logged, and how many failures
/// since went unlogged.
#[derive(Default)]
struct ErrorLog(Mutex<HashMap<String, (Instant, u64)>>);

impl ErrorLog {
    /// Whether to log `component`'s failure at `now`: `Some(n)`, the failures
    /// left unlogged since the last line, or `None` within the interval.
    fn should_log(&self, component: &str, now: Instant) -> Option<u64> {
        let mut map = self.0.lock().unwrap_or_else(|p| p.into_inner());
        match map.get_mut(component) {
            Some((last, skipped))
                if now.saturating_duration_since(*last) < RENDER_ERROR_LOG_INTERVAL =>
            {
                *skipped += 1;
                None
            }
            Some(entry) => {
                let n = entry.1;
                *entry = (now, 0);
                Some(n)
            }
            None => {
                map.insert(component.to_string(), (now, 0));
                Some(0)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_logs_once_per_interval_with_the_count_it_stands_for() {
        let log = ErrorLog::default();
        let t0 = Instant::now();
        assert_eq!(log.should_log("a", t0), Some(0));
        for s in 1..60 {
            assert_eq!(log.should_log("a", t0 + Duration::from_secs(s)), None);
        }
        // Another component is its own budget.
        assert_eq!(log.should_log("b", t0 + Duration::from_secs(1)), Some(0));
        assert_eq!(
            log.should_log("a", t0 + RENDER_ERROR_LOG_INTERVAL),
            Some(59)
        );
        assert_eq!(
            log.should_log("a", t0 + RENDER_ERROR_LOG_INTERVAL + Duration::from_secs(1)),
            None
        );
    }

    #[test]
    fn an_asset_name_is_its_hash_and_its_kind_only() {
        let js = asset_name(AssetKind::Script, "export {}");
        assert!(js.starts_with("c-") && js.ends_with(".js"), "{js}");
        assert_eq!(js.len(), 2 + 32 + 3);
        assert_ne!(js, asset_name(AssetKind::Script, "export {};"));
        assert_eq!(
            asset_name(AssetKind::Stylesheet, "export {}").trim_end_matches(".css"),
            js.trim_end_matches(".js")
        );
    }
}
