//! Slot components (ADR-119 D18, Amendment 4 H1/H2; parity P-153 to P-163).
//!
//! An operator adds components to a site at named slots. Each is declared in
//! a manifest (`components.toml`), written as a minijinja template, and
//! sanitised like content ([`clean`]). This module is the engine: it takes
//! the manifest and the files it names as strings, so it does no I/O and
//! builds for WASM. Reading the operator's directory is the daemon's (and
//! later the export's) job.
//!
//! - [`Components::load`] checks everything once (every template compiled,
//!   every data file validated against its shape, every declared attribute
//!   checked) and returns every problem at once. A load error stops the
//!   daemon (H1) and fails the export (P-157).
//! - [`Components::render_slot`] renders one slot for one page: each
//!   component in manifest order, each sanitised, a failing one contributing
//!   nothing and reported by name (D18: live, the slot stays empty). An empty
//!   slot is the empty string (P-158).
//! - [`Components::render_page`] renders a page a component emits from its
//!   data (P-160), into the same sanitiser.
//!
//! # What a template sees (P-154, H1)
//!
//! `page` (the page's own `id`, `title`, `url`, `properties`, `dates`,
//! `tags`), `site` (`locale`, `base_path`, `kb_id`, and `pages`: the address
//! of each page a component emits, by name), `data` (its own validated data
//! file, if it declares one) and one function about other nodes,
//! `published(id)`: that node's address in this view, or none. Never another
//! page's content. A page template sees `site`, `data` and `published`.
//!
//! The core adds `iso_date(s)` (a date with `iso`, `year`, `month`, `day`,
//! or none) and `fail(msg)`, removes `debug()`, and caps `range()`.
//!
//! # Templates are confined
//!
//! - **No loader, no includes.** minijinja is built without its
//!   `multi_template` feature, so `include`, `extends`, `import` and `block`
//!   do not parse: a template is exactly the file the manifest names. No
//!   loader is set either, so no name can reach the file system.
//! - **Strict undefined:** printing, iterating or testing an undefined value
//!   fails the render; `is defined` and `in` are how a template asks.
//! - **Bounded:** a fuel limit ([`FUEL_LIMIT`]), a recursion limit
//!   ([`RECURSION_LIMIT`]), capped ranges ([`MAX_RANGE`]) and a cap on what
//!   one render may write ([`MAX_OUTPUT_BYTES`]). Fuel counts instructions,
//!   not the size of one: a template can still build one large string
//!   (minijinja caps `*` repetition at 100 MB) without writing it. That is
//!   accepted because a template is the operator's configuration (D3), not
//!   content; the bounds stop a mistake from looping, not a hostile operator.
//!
//! @ai-caution: [security] Everything a component writes passes [`clean`]
//! before it reaches a page. A template's own HTML escaping is not the
//! guard: the sanitiser is, and its tests are the contract (P-155).

mod clean;
mod manifest;
mod shape;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::sync::Arc;

use minijinja::value::{Object, ObjectRepr};
use minijinja::{AutoEscape, Environment, Error, ErrorKind, State, UndefinedBehavior, Value};
use serde::Serialize;

pub use clean::{attribute_problem, is_component_class, CLASS_PREFIX, HEAD_META_NAMES};
pub use manifest::{MAX_COMPONENTS, MAX_FILE_BYTES};
pub use shape::{iso_date, FieldType, IsoDate, Shape, MAX_DATA_BYTES, MAX_DATA_ITEMS};

use crate::address::{segment, Addressing};

/// The fuel one render may spend: about one unit per template instruction.
///
/// **Unmeasured** (D18 leaves the measurement open): nobody has timed a
/// render at this limit on the daemon's hardware. It is safe to choose
/// without that measurement because it is far from both ends: the largest
/// render the fixture examples can make (the site-changes page over
/// [`MAX_DATA_ITEMS`] versions) spends well under half of it, which a test
/// checks, and minijinja executes instructions in the order of millions a
/// second, so the limit caps one render at milliseconds, not seconds. When
/// D21's measurements land, this becomes their number.
pub const FUEL_LIMIT: u64 = 100_000;

/// How deep a render may nest (loops, conditions, recursive loops).
/// minijinja's own ceiling without stack growth is 500; a component nests a
/// handful of levels, so a far lower limit fails a runaway recursive loop
/// early.
pub const RECURSION_LIMIT: usize = 64;

/// The most items `range()` may produce (minijinja's own cap is 100,000). A
/// component iterates over its data, not over counts; a range this long is
/// already a mistake.
pub const MAX_RANGE: usize = 1_000;

/// The most bytes one render may write before it fails.
pub const MAX_OUTPUT_BYTES: usize = 64 * 1024;

/// P-153's seven slots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Slot {
    /// Inside `<head>`: `meta[name, content]` only ([`HEAD_META_NAMES`]).
    DocumentHead,
    /// The header row, after Back/Forward.
    HeaderRow,
    /// After the title and its metadata, above the article.
    BeforeBody,
    /// After the article.
    AfterBody,
    /// The bottom of the left panel.
    LeftPanelFooter,
    /// The right panel.
    RightPanel,
    /// The page footer.
    PageFooter,
}

impl Slot {
    /// Every slot, in page order.
    pub const ALL: [Slot; 7] = [
        Slot::DocumentHead,
        Slot::HeaderRow,
        Slot::BeforeBody,
        Slot::AfterBody,
        Slot::LeftPanelFooter,
        Slot::RightPanel,
        Slot::PageFooter,
    ];

    /// The slot's name in the manifest.
    pub fn name(self) -> &'static str {
        match self {
            Slot::DocumentHead => "document-head",
            Slot::HeaderRow => "header-row",
            Slot::BeforeBody => "before-body",
            Slot::AfterBody => "after-body",
            Slot::LeftPanelFooter => "left-panel-footer",
            Slot::RightPanel => "right-panel",
            Slot::PageFooter => "page-footer",
        }
    }

    /// The slot named `s` in a manifest.
    pub fn parse(s: &str) -> Option<Slot> {
        Slot::ALL.into_iter().find(|slot| slot.name() == s)
    }

    fn names() -> String {
        Slot::ALL.map(Slot::name).join(", ")
    }
}

/// A problem found while loading. `component` is `None` for the manifest as
/// a whole (a TOML syntax error, an unknown top-level key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub component: Option<String>,
    pub field: String,
    pub reason: String,
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.component {
            Some(c) => write!(f, "component `{c}`, {}: {}", self.field, self.reason),
            None if self.field.is_empty() => write!(f, "components.toml: {}", self.reason),
            None => write!(f, "components.toml, {}: {}", self.field, self.reason),
        }
    }
}

/// A component that failed while rendering; it contributed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderError {
    pub component: String,
    pub reason: String,
}

impl fmt::Display for RenderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "component `{}`: {}", self.component, self.reason)
    }
}

/// The page a slot is rendered for: its own data only (P-154).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PageData {
    /// The file-level ID.
    pub id: String,
    pub title: String,
    /// The page's address, as the derivation built it.
    pub url: String,
    /// File-level properties, keys as mae-org reports them.
    pub properties: BTreeMap<String, String>,
    /// The page's dates by name (for example `created`, `updated`), each as
    /// its source wrote it; a template reads one with `iso_date`.
    pub dates: BTreeMap<String, String>,
    pub tags: Vec<String>,
}

/// The view's settings, shared by every page rendered in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SiteContext {
    pub locale: String,
    pub base_path: String,
    pub kb_id: String,
    /// The address of each page a component emits, by page name
    /// ([`Components::page_hrefs`]).
    pub pages: BTreeMap<String, String>,
    /// Every node published in this view: ID to address. Templates reach it
    /// only through `published(id)`, one ID at a time.
    #[serde(skip)]
    pub published: Arc<BTreeMap<String, String>>,
}

/// What [`Components::render_slot`] renders against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageContext {
    pub page: PageData,
    pub site: Arc<SiteContext>,
}

/// A page a component emits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentPage {
    pub component: String,
    /// One URL segment, under `site/` (H2).
    pub name: String,
    /// The operator's title for it, for the chrome's heading and `<title>`.
    pub title: String,
}

/// A file a component asks the browser to load.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// A behaviour: an ES module registered with the router.
    Script,
    Stylesheet,
}

/// The loaded components of one site.
pub struct Components {
    env: Environment<'static>,
    items: Vec<Item>,
}

struct Item {
    spec: manifest::Spec,
    data: Option<Value>,
    /// The slot's sanitiser: the head's for the document head.
    cleaner: clean::Cleaner,
    /// An emitted page's: always the body's, whatever the slot.
    page_cleaner: clean::Cleaner,
}

impl fmt::Debug for Components {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.items.iter().map(|i| &i.spec.name))
            .finish()
    }
}

/// `published(id)` as a template sees it.
#[derive(Debug)]
struct Published(Arc<BTreeMap<String, String>>);

impl Object for Published {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn call(self: &Arc<Self>, _: &State<'_, '_>, args: &[Value]) -> Result<Value, Error> {
        let [id] = args else {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "published() takes one node ID",
            ));
        };
        Ok(id
            .as_str()
            .and_then(|id| self.0.get(id))
            .map_or(Value::from(()), |href| Value::from(href.as_str())))
    }
}

fn iso_date_fn(v: Value) -> Value {
    match v.as_str().and_then(iso_date) {
        Some(d) => Value::from_iter([
            ("iso", Value::from(d.to_string())),
            ("year", Value::from(d.year)),
            ("month", Value::from(d.month)),
            ("day", Value::from(d.day)),
        ]),
        None => Value::from(()),
    }
}

fn fail_fn(msg: String) -> Result<Value, Error> {
    Err(Error::new(ErrorKind::InvalidOperation, msg))
}

/// minijinja's `range`, refusing more than [`MAX_RANGE`] items.
fn range_fn(lower: i64, upper: Option<i64>, step: Option<i64>) -> Result<Value, Error> {
    let (lo, hi) = match upper {
        Some(hi) => (lower, hi),
        None => (0, lower),
    };
    let step = step.unwrap_or(1);
    let span = i128::from(hi) - i128::from(lo);
    let len = match step {
        0 => 0,
        s if (span > 0) == (s > 0) => (span.abs() + i128::from(s).abs() - 1) / i128::from(s).abs(),
        _ => 0,
    };
    if len > MAX_RANGE as i128 {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("range of {len} items; a component may make at most {MAX_RANGE}"),
        ));
    }
    let as_isize = |v: i64| {
        isize::try_from(v)
            .map_err(|_| Error::new(ErrorKind::InvalidOperation, "range bound too large"))
    };
    minijinja::functions::range(
        as_isize(lower)?,
        upper.map(as_isize).transpose()?,
        Some(as_isize(step)?),
    )
}

/// The environment every component renders in (see the module doc).
fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.set_auto_escape_callback(|_| AutoEscape::Html);
    env.set_fuel(Some(FUEL_LIMIT));
    env.set_recursion_limit(RECURSION_LIMIT);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);
    env.remove_global("debug");
    env.add_function("range", range_fn);
    env.add_function("iso_date", iso_date_fn);
    env.add_function("fail", fail_fn);
    env
}

/// Collects output, failing once more than [`MAX_OUTPUT_BYTES`] arrive.
struct Capped(Vec<u8>);

impl io::Write for Capped {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.0.len() + buf.len() > MAX_OUTPUT_BYTES {
            return Err(io::Error::other(format!(
                "wrote more than {MAX_OUTPUT_BYTES} bytes"
            )));
        }
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The key a component's slot template is compiled under.
fn slot_key(name: &str) -> String {
    format!("{name}/template")
}

/// The key a component's page template is compiled under.
fn page_key(name: &str) -> String {
    format!("{name}/page")
}

impl Components {
    /// Load the components `manifest` declares, from `files` (each file the
    /// manifest names, by name, as text). Every problem is returned at once.
    pub fn load(
        manifest: &str,
        files: &BTreeMap<String, String>,
    ) -> Result<Components, Vec<LoadError>> {
        let (specs, mut errors) = manifest::parse(manifest, files);
        let mut env = environment();
        let mut items = Vec::new();
        for spec in specs {
            let compiled = [
                (spec.template.as_ref(), slot_key(&spec.name), "template"),
                (
                    spec.page.as_ref().map(|p| &p.template),
                    page_key(&spec.name),
                    "page.template",
                ),
            ];
            for (file, key, field) in compiled {
                let Some(file) = file else { continue };
                if let Err(err) = env.add_template_owned(key, files[file].clone()) {
                    errors.push(LoadError {
                        component: Some(spec.name.clone()),
                        field: field.into(),
                        reason: format!("`{file}` does not compile: {err}"),
                    });
                }
            }
            let data = spec.data.as_ref().map(|(_, _, v)| Value::from_serialize(v));
            let cleaner = match spec.slot {
                Some(Slot::DocumentHead) => clean::Cleaner::head(),
                _ => clean::Cleaner::body(&spec.attributes),
            };
            let page_cleaner = clean::Cleaner::body(&spec.attributes);
            items.push(Item {
                spec,
                data,
                cleaner,
                page_cleaner,
            });
        }
        if errors.is_empty() {
            Ok(Components { env, items })
        } else {
            Err(errors)
        }
    }

    /// The file names `manifest` names, for a loader to read from the
    /// component directory and pass to [`Components::load`] (H1: the
    /// manifest, not a naming convention, says what the directory holds).
    /// Names that are not one plain file name are left out, and the load
    /// reports them.
    pub fn named_files(manifest: &str) -> BTreeSet<String> {
        manifest::named_files(manifest)
    }

    /// Render `slot` for one page: every component in it, in manifest order,
    /// each sanitised. A component that fails contributes nothing and its
    /// error is returned; an empty slot is `""`.
    pub fn render_slot(&self, slot: Slot, ctx: &PageContext) -> (String, Vec<RenderError>) {
        let mut html = String::new();
        let mut errors = Vec::new();
        let page = Value::from_serialize(&ctx.page);
        for item in self.items.iter().filter(|i| i.spec.slot == Some(slot)) {
            let root = self.root(item, Some(page.clone()), &ctx.site);
            let (out, err) = self.run(item, &slot_key(&item.spec.name), root);
            if let Some(e) = err {
                errors.push(e);
                continue;
            }
            html.push_str(&item.cleaner.clean(&out));
        }
        (html, errors)
    }

    /// Render the page named `name` (P-160), sanitised. `None` when no
    /// component emits it.
    pub fn render_page(
        &self,
        name: &str,
        site: &Arc<SiteContext>,
    ) -> Option<Result<String, RenderError>> {
        let item = self
            .items
            .iter()
            .find(|i| i.spec.page.as_ref().is_some_and(|p| p.name == name))?;
        let root = self.root(item, None, site);
        let (out, err) = self.run(item, &page_key(&item.spec.name), root);
        Some(match err {
            Some(e) => Err(e),
            None => Ok(item.page_cleaner.clean(&out)),
        })
    }

    /// Every page the components emit, in manifest order.
    pub fn pages(&self) -> Vec<ComponentPage> {
        self.items
            .iter()
            .filter_map(|i| {
                i.spec.page.as_ref().map(|p| ComponentPage {
                    component: i.spec.name.clone(),
                    name: p.name.clone(),
                    title: p.title.clone(),
                })
            })
            .collect()
    }

    /// The address of each emitted page under `addressing`, for
    /// [`SiteContext::pages`]: `/kb/<kb-id>/site/<name>` live and
    /// `<base>/site/<name>/` in the export (H2).
    pub fn page_hrefs(&self, addressing: &Addressing) -> BTreeMap<String, String> {
        self.pages()
            .into_iter()
            .map(|p| (p.name.clone(), site_page_href(addressing, &p.name)))
            .collect()
    }

    /// The scripts and stylesheets each component names, in manifest order:
    /// (component, kind, file name).
    pub fn assets(&self) -> Vec<(&str, AssetKind, &str)> {
        let mut out = Vec::new();
        for i in &self.items {
            let name = i.spec.name.as_str();
            if let Some(s) = &i.spec.script {
                out.push((name, AssetKind::Script, s.as_str()));
            }
            if let Some(s) = &i.spec.stylesheet {
                out.push((name, AssetKind::Stylesheet, s.as_str()));
            }
        }
        out
    }

    /// The names of the slots that have at least one component.
    pub fn slots(&self) -> BTreeSet<Slot> {
        self.items.iter().filter_map(|i| i.spec.slot).collect()
    }

    fn root(&self, item: &Item, page: Option<Value>, site: &Arc<SiteContext>) -> Value {
        let mut root = vec![
            ("site", Value::from_serialize(&**site)),
            (
                "published",
                Value::from_object(Published(site.published.clone())),
            ),
        ];
        if let Some(page) = page {
            root.push(("page", page));
        }
        if let Some(data) = &item.data {
            root.push(("data", data.clone()));
        }
        Value::from_iter(root)
    }

    /// One render: its raw output, and its error if it failed.
    fn run(&self, item: &Item, key: &str, root: Value) -> (String, Option<RenderError>) {
        let mut out = Capped(Vec::new());
        let result = self
            .env
            .get_template(key)
            .and_then(|t| t.render_captured_to(root, &mut out).map(|_| ()));
        let text = String::from_utf8_lossy(&out.0).into_owned();
        let err = result.err().map(|e| RenderError {
            component: item.spec.name.clone(),
            reason: describe(&e),
        });
        (text, err)
    }

    /// The fuel one render spends: `component`'s slot template for `page`,
    /// or its page template when `page` is `None`; or the render's error.
    /// The measurement D18 leaves open starts here, and the tests that pin
    /// [`FUEL_LIMIT`] use it.
    pub fn fuel_used(
        &self,
        component: &str,
        page: Option<&PageContext>,
        site: &Arc<SiteContext>,
    ) -> Result<u64, String> {
        let item = self
            .items
            .iter()
            .find(|i| i.spec.name == component)
            .ok_or("no such component")?;
        let (key, page) = match page {
            Some(p) => (slot_key(component), Some(Value::from_serialize(&p.page))),
            None => (page_key(component), None),
        };
        let root = self.root(item, page, site);
        let t = self.env.get_template(&key).map_err(|e| e.to_string())?;
        let captured = t
            .render_captured_to(root, io::sink())
            .map_err(|e| e.to_string())?;
        Ok(captured.state().fuel_levels().map_or(0, |(used, _)| used))
    }
}

/// An error and its causes (a write failure's cause is the output cap).
fn describe(e: &Error) -> String {
    let mut out = e.to_string();
    let mut cause = std::error::Error::source(e);
    while let Some(c) = cause {
        out.push_str(&format!(": {c}"));
        cause = c.source();
    }
    out
}

/// A component page's address: `/kb/<kb-id>/site/<name>` live and
/// `<base>/site/<name>/` in the export (H2).
pub fn site_page_href(addressing: &Addressing, name: &str) -> String {
    // "site" is always a valid ID, so this is always Some.
    let site = addressing.page_href("site").unwrap_or_default();
    if addressing.is_export() {
        format!("{site}{}/", segment(name))
    } else {
        format!("{site}/{}", segment(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn components_can_be_shared_between_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Components>();
    }

    #[test]
    fn a_component_page_lives_under_site() {
        let live = Addressing::live("kb-1").unwrap();
        assert_eq!(site_page_href(&live, "changes"), "/kb/kb-1/site/changes");
        let export = Addressing::export("/docs").unwrap();
        assert_eq!(site_page_href(&export, "changes"), "/docs/site/changes/");
    }

    #[test]
    fn range_refuses_more_than_the_cap_in_either_direction() {
        assert!(range_fn(MAX_RANGE as i64, None, None).is_ok());
        assert!(range_fn(MAX_RANGE as i64 + 1, None, None).is_err());
        assert!(range_fn(0, Some(i64::MAX), Some(1)).is_err());
        assert!(range_fn(i64::MAX, Some(0), Some(-1)).is_err());
        assert!(range_fn(0, Some(10_000), Some(10)).is_ok());
        assert!(range_fn(5, Some(0), None).is_ok()); // empty, not an error
    }
}
