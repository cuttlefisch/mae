//! The live KB site (ADR-119 L1a): one page of a KB, read-only, at
//! `/kb/<kb-id>/<id>` on the OAuth listener.
//!
//! Every answer that is not a page is the same [`SitePage::NotFound`]: a KB
//! that does not exist, one the caller may not read, an end-to-end-encrypted
//! KB (L1 serves plaintext KBs only), a node that does not exist, and a page
//! the derivation withheld. So the route is no oracle for which KBs or nodes
//! exist (D7).
//!
//! The read gate is `kb_query::load_gated`, the one `kb/query.*` uses, so the
//! site and the query surface can never disagree about who may read a KB
//! (principle #8). The page itself comes from `mae_site::derive`, the core the
//! static export shares.
//!
//! One timing difference remains by design: a member asking for a missing
//! node waits for a derivation, while a non-member is refused at the gate. It
//! tells a caller only whether THEY may read the KB, which they know anyway.
//!
//! This first slice derives the KB per request, bounded (D21) by
//! [`SiteLimits`] and run off the async workers (`spawn_blocking`). The derivation cache and incremental re-derivation (D6) come with the
//! change index (ADR-074 Amendment 1).

use crate::doc_store::DocStore;
use crate::kb_query;
use mae_site::{derive_with, Addressing, SourcePage};
use mae_sync::kb::{Encryption, KbNodeDoc};

/// What the route answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SitePage {
    /// A page: its title (raw text), its sanitised article HTML, its
    /// backlinks, and its tags as `(tag, href)`, with no href for a reader
    /// refused the tag pages ([`may_list`](crate::kb_query::may_list_whole_kb)).
    Found {
        title: String,
        html: String,
        backlinks: Vec<mae_site::Backlink>,
        tags: Vec<(String, Option<String>)>,
        toc: Vec<mae_site::TocEntry>,
        explorer: Option<ExplorerRef>,
    },
    /// The content index (P-77): one entry per published page.
    Index(Vec<mae_site::IndexEntry>),
    /// A listing the engine generates: the tag index or one tag's pages, as
    /// `(text, href)` entries.
    Listing {
        kind: Listing,
        items: Vec<(String, String)>,
        explorer: Option<ExplorerRef>,
    },
    /// The explorer's data (P-39, Amendment 3), as the JSON the browser
    /// runtime reads.
    Explorer(String),
    /// Every refusal and every absence, alike.
    NotFound,
    /// The KB is larger than this listener derives per request.
    TooLarge,
}

/// Where a page's explorer loads its data from, and which version of it this
/// page was derived with: the browser runtime fetches it once and again only
/// when a page names another version (D6: chrome is not embedded in pages).
/// Present only for a reader who may list the whole KB (D7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerRef {
    pub src: String,
    /// The first 16 hex digits of the SHA-256 of [`explorer_json`]'s output.
    pub version: String,
}

/// The explorer as the browser runtime reads it:
/// `{"groups":[{"tag","pages":[{"title","href"}]}],"untagged":[{"title","href"}]}`,
/// in display order. Titles are raw text; the runtime writes them with
/// `textContent`, never as markup.
pub fn explorer_json(ex: &mae_site::explorer::Explorer) -> String {
    let pages = |ps: &[mae_site::PageRef]| -> Vec<serde_json::Value> {
        ps.iter()
            .map(|p| serde_json::json!({ "title": p.title, "href": p.href }))
            .collect()
    };
    let groups: Vec<serde_json::Value> = ex
        .groups
        .iter()
        .map(|g| serde_json::json!({ "tag": g.tag, "pages": pages(&g.pages) }))
        .collect();
    serde_json::json!({ "groups": groups, "untagged": pages(&ex.untagged) }).to_string()
}

fn explorer_ref(json: &str, addressing: &Addressing) -> ExplorerRef {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(json.as_bytes());
    ExplorerRef {
        src: addressing.explorer_href(),
        version: hex::encode(&digest[..8]),
    }
}

/// Which generated listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// Every tag (P-71).
    Tags,
    /// The pages of one tag (P-70).
    Tag(String),
}

/// What a request asks for under `/kb/<kb-id>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// `/kb/<kb-id>/<id>`.
    Page(String),
    /// `/kb/<kb-id>/tags`.
    Tags,
    /// `/kb/<kb-id>/tags/<tag>`.
    Tag(String),
    /// `/kb/<kb-id>/index.json`.
    Index,
    /// `/kb/<kb-id>/explorer.json`.
    Explorer,
    /// `/kb/<kb-id>/`: the KB's home page, the node the operator declared
    /// for it (ADR-119 Amendment 2 B3).
    Home(Option<String>),
}

/// What one page request may read (D21).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SiteLimits {
    /// Nodes in the KB: more, and the KB is [`SitePage::TooLarge`].
    pub max_scan_nodes: usize,
    /// Bytes of one node's title or body: more, and the KB is too large.
    pub max_body_bytes: usize,
}

/// The page `id` of `kb_id`, as `principal` may read it.
pub async fn page(
    doc_store: &DocStore,
    kb_id: &str,
    id: &str,
    principal: Option<&str>,
    limits: SiteLimits,
) -> SitePage {
    view(
        doc_store,
        kb_id,
        &View::Page(id.to_string()),
        principal,
        limits,
        &mae_site::SiteOptions::default(),
    )
    .await
}

/// One view of `kb_id`, as `principal` may read it.
pub async fn view(
    doc_store: &DocStore,
    kb_id: &str,
    view: &View,
    principal: Option<&str>,
    limits: SiteLimits,
    opts: &mae_site::SiteOptions,
) -> SitePage {
    let (sources, addressing, may_list) =
        match readable_sources(doc_store, kb_id, principal, limits).await {
            Ok(v) => v,
            Err(refused) => return refused.into(),
        };
    // Rendering is CPU work: off the async workers.
    let view = view.clone();
    let opts = opts.clone();
    let answered = tokio::task::spawn_blocking(move || {
        answer(
            &derive_with(&sources, &addressing, &opts),
            &addressing,
            view,
            may_list,
        )
    })
    .await;
    answered.unwrap_or(SitePage::NotFound)
}

/// Why [`readable_sources`] gave no sources: the answer it stands for.
enum Refused {
    NotFound,
    TooLarge,
}

impl From<Refused> for SitePage {
    fn from(r: Refused) -> Self {
        match r {
            Refused::NotFound => SitePage::NotFound,
            Refused::TooLarge => SitePage::TooLarge,
        }
    }
}

/// The KB's pages as sources, once the read gate admits `principal` and the
/// KB is within `limits`, and whether `principal` may have the outputs that
/// list the whole KB (D7); otherwise the answer to give.
async fn readable_sources(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    limits: SiteLimits,
) -> Result<(Vec<SourcePage>, Addressing, bool), Refused> {
    let Ok(addressing) = Addressing::live(kb_id) else {
        return Err(Refused::NotFound);
    };
    let Ok((coll, encryption)) = kb_query::load_gated(doc_store, kb_id, principal).await else {
        return Err(Refused::NotFound);
    };
    if !matches!(encryption, Encryption::None) {
        return Err(Refused::NotFound);
    }
    let may_list = kb_query::may_list_whole_kb(doc_store, kb_id, &coll, principal).await;
    let nodes = coll.list_nodes();
    if nodes.len() > limits.max_scan_nodes {
        return Err(Refused::TooLarge);
    }
    let mut sources = Vec::with_capacity(nodes.len());
    for (node_id, _) in nodes {
        let name = mae_sync::kb_node_doc_name(kb_id, &node_id);
        let Ok((state, _sv)) = doc_store.encode_state_and_sv(&name).await else {
            continue;
        };
        let Ok(doc) = KbNodeDoc::from_bytes(&state) else {
            continue;
        };
        let (title, body) = (doc.title(), doc.body());
        if title.len() > limits.max_body_bytes || body.len() > limits.max_body_bytes {
            return Err(Refused::TooLarge);
        }
        if let Some(text) = page_source(&node_id, &doc) {
            sources.push(SourcePage {
                origin: node_id,
                text,
            });
        }
    }
    Ok((sources, addressing, may_list))
}

/// The answer to `view` from a derived KB. A reader who may not list the
/// whole KB (`may_list`, D7) gets pages only: every output that lists the KB
/// (the explorer, the content index, the tag index and tag pages) is the
/// same not-found as anything else, and their pages neither point to the
/// explorer nor link their tags.
fn answer(site: &mae_site::Site, addressing: &Addressing, view: View, may_list: bool) -> SitePage {
    let explorer = || {
        may_list.then(|| {
            let json = explorer_json(&mae_site::explorer::explorer(site));
            explorer_ref(&json, addressing)
        })
    };
    match view {
        View::Index | View::Tags | View::Tag(_) | View::Explorer if !may_list => SitePage::NotFound,
        // The declared node when this reader has it as a page; otherwise the
        // tag index. Unpublished, withheld and undeclared all answer alike, so
        // the home address says nothing about a node the reader cannot see.
        View::Home(declared) => match declared.filter(|id| site.pages.contains_key(id)) {
            Some(id) => answer(site, addressing, View::Page(id), may_list),
            None => answer(site, addressing, View::Tags, may_list),
        },
        View::Page(id) => match site.pages.get(&id) {
            Some(p) => SitePage::Found {
                title: p.title.clone(),
                html: p.html.clone(),
                backlinks: p.backlinks.clone(),
                tags: p
                    .tags
                    .iter()
                    .map(|t| (t.clone(), may_list.then(|| addressing.tag_href(t))))
                    .collect(),
                toc: p.toc.clone(),
                explorer: explorer(),
            },
            None => SitePage::NotFound,
        },
        View::Tags => SitePage::Listing {
            kind: Listing::Tags,
            items: site
                .tags_in_order()
                .into_iter()
                .map(|(t, _)| (t.to_string(), addressing.tag_href(t)))
                .collect(),
            explorer: explorer(),
        },
        View::Index => SitePage::Index(site.index.clone()),
        View::Explorer => SitePage::Explorer(explorer_json(&mae_site::explorer::explorer(site))),
        View::Tag(tag) => match site.tags.get(&tag) {
            Some(pages) => SitePage::Listing {
                kind: Listing::Tag(tag),
                items: pages
                    .iter()
                    .map(|p| (p.title.clone(), p.href.clone()))
                    .collect(),
                explorer: explorer(),
            },
            None => SitePage::NotFound,
        },
    }
}

/// A node as the Org text of its page: its ID, title and file tags, then its
/// body. Any node has a page, however it was written (seed, editor, tool), so
/// this is not the seed export's file, which refuses a node that would not
/// seed back exactly.
///
/// The tags come from the node's own field, which is where `kb seed` and the
/// editor keep `#+filetags`; a page built from title and body alone had none,
/// so no tag page ever listed a seeded page. Only valid Org tags are written
/// (P-73), so a tag cannot add a line, and a title's line breaks are folded so
/// it stays one keyword line. No page for a node whose ID could not be an
/// address (the core's own rule: controls, White_Space, bidi controls, U+FEFF).
fn page_source(node_id: &str, doc: &KbNodeDoc) -> Option<String> {
    if node_id.is_empty() || node_id.chars().any(mae_site::address::refused_in_id) {
        return None;
    }
    let title: String = doc
        .title()
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let tags: Vec<String> = doc
        .tags()
        .into_iter()
        .filter(|t| mae_site::address::valid_tag(t))
        .collect();
    let filetags = if tags.is_empty() {
        String::new()
    } else {
        format!("#+filetags: :{}:\n", tags.join(":"))
    };
    Some(format!(
        ":PROPERTIES:\n:ID: {node_id}\n:END:\n#+title: {title}\n{filetags}\n{}\n",
        doc.body()
    ))
}

/// What every page of one KB's site shares: its locale and, when the operator
/// named one, the site's title (P-16) with the address of its home page. A
/// plain locale string is a site without a title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chrome<'a> {
    pub lang: &'a str,
    /// `(site title, home href)`: operator configuration, never content (D3).
    pub site: Option<(&'a str, &'a str)>,
}

impl<'a> From<&'a str> for Chrome<'a> {
    fn from(lang: &'a str) -> Self {
        Chrome { lang, site: None }
    }
}

/// The configured locale's catalogue, and how the chrome marks what it writes
/// from it (ADR-119 A4). A locale with no catalogue gets en-US's words, and
/// every element holding them carries `lang="en-US"` so a screen reader voices
/// them as written, while `<html lang>` stays the configured locale; content
/// inside such an element (a tag) carries the configured locale back. A found
/// catalogue is in the page's own language: no marks.
///
/// Not marked: a listing's `<title>`, which repeats its heading. It is plain
/// text holding the operator's site title too, so it cannot mark one part.
struct Words {
    s: &'static mae_site::strings::Strings,
    /// ` lang="en-US"` on an element holding fallback words; empty otherwise.
    mark: String,
    /// ` lang="<configured>"` on content inside a marked element; empty otherwise.
    unmark: String,
}

impl Words {
    fn of(lang: &str) -> Self {
        match mae_site::strings::resolve(lang) {
            (s, false) => Words {
                s,
                mark: String::new(),
                unmark: String::new(),
            },
            (s, true) => Words {
                s,
                mark: format!(" lang=\"{}\"", escape(s.locale)),
                unmark: format!(" lang=\"{}\"", escape(lang)),
            },
        }
    }

    /// Escaped content inside marked chrome, with the page's language back.
    fn content(&self, escaped: &str) -> String {
        if self.unmark.is_empty() {
            escaped.to_string()
        } else {
            format!("<span{}>{escaped}</span>", self.unmark)
        }
    }
}

/// The HTML document for a found page: the core's fixed chrome around the
/// sanitised article (added after sanitising, D19). `title` is escaped here.
pub fn document<'a>(
    chrome: impl Into<Chrome<'a>>,
    title: &str,
    article: &str,
    backlinks: &[mae_site::Backlink],
    tags: &[(String, Option<String>)],
    toc: &[mae_site::TocEntry],
    explorer: Option<&ExplorerRef>,
) -> String {
    let chrome = chrome.into();
    let words = Words::of(chrome.lang);
    let article = format!(
        "<article id=\"mae-article\"><h1>{}</h1>{}{article}</article>",
        escape(title),
        tags_list(&words, tags),
    );
    // The side panels sit outside the main landmark (P-165).
    let panels = format!(
        "{}{}",
        toc_nav(&words, toc),
        backlinks_nav(&words, backlinks)
    );
    shell(chrome, title, &article, &panels, explorer)
}

/// The page's table of contents (P-79): one link per heading, to the id the
/// heading carries on this page; nothing for a page with fewer than two.
fn toc_nav(words: &Words, toc: &[mae_site::TocEntry]) -> String {
    if toc.len() < 2 {
        return String::new();
    }
    let items: String = toc
        .iter()
        .map(|e| {
            format!(
                "<li class=\"mae-toc-l{}\"><a href=\"#{}\">{}</a></li>",
                e.level,
                escape(&e.anchor),
                escape(&e.text)
            )
        })
        .collect();
    format!(
        "<nav id=\"mae-toc\" aria-labelledby=\"mae-toc-h\"><h2 id=\"mae-toc-h\"{}>{}</h2><ol>{items}</ol></nav>",
        words.mark,
        escape(words.s.contents)
    )
}

/// The HTML document for a generated listing: the tag index or one tag's
/// pages (P-70, P-71), headed in the locale's words.
pub fn listing_document<'a>(
    chrome: impl Into<Chrome<'a>>,
    kind: &Listing,
    items: &[(String, String)],
    explorer: Option<&ExplorerRef>,
) -> String {
    let chrome = chrome.into();
    let words = Words::of(chrome.lang);
    let heading = match kind {
        Listing::Tags => words.s.tags.to_string(),
        Listing::Tag(t) => format!("{}{t}{}", words.s.tag_page_open, words.s.tag_page_close),
    };
    // The visible heading marks the catalogue's words, and gives the tag, which
    // is content, the page's language back.
    let shown = match kind {
        Listing::Tags => escape(&heading),
        Listing::Tag(t) => format!(
            "{}{}{}",
            escape(words.s.tag_page_open),
            words.content(&escape(t)),
            escape(words.s.tag_page_close)
        ),
    };
    let list: String = items
        .iter()
        .map(|(text, href)| format!("<li><a href=\"{}\">{}</a></li>", escape(href), escape(text)))
        .collect();
    shell(
        chrome,
        &heading,
        &format!(
            "<section id=\"mae-listing\"><h1{}>{shown}</h1><ul>{list}</ul></section>",
            words.mark
        ),
        "",
        explorer,
    )
}

/// The document around a page's main content: lang, the pre-paint script,
/// the palette, the title, the site's header, one main landmark holding the
/// content, and the side panels after it, in an `aside`. Header and panels
/// are never inside `main` (P-165). The header always holds the mode and
/// palette controls, and the site's title first when there is one (a bare
/// page title otherwise); no panels, no `aside`.
///
/// The pre-paint script is a classic script before the stylesheets (D17): it
/// blocks rendering until the reader's choice is on `<html>`, so the first
/// frame already has it (P-91, P-93).
///
/// The explorer's box sits between the header and `main`, empty and `hidden`:
/// the router's behaviour fills it from `data-src` and shows it, so without
/// JavaScript it is absent, never broken. It carries the words the behaviour
/// needs (its name, the untagged group's). No box for a reader who may not
/// list the whole KB (D7).
fn shell(
    chrome: Chrome<'_>,
    title: &str,
    main: &str,
    panels: &str,
    explorer: Option<&ExplorerRef>,
) -> String {
    let words = Words::of(chrome.lang);
    let (doc_title, site_link) = match chrome.site {
        Some((site, home)) => (
            format!("{} \u{2014} {}", escape(title), escape(site)),
            format!("<a href=\"{}\">{}</a>", escape(home), escape(site)),
        ),
        None => (escape(title), String::new()),
    };
    let header = format!(
        "<header id=\"mae-header\">{site_link}{}</header>",
        theme_controls(&words)
    );
    let explorer = explorer.map_or_else(String::new, |e| {
        format!(
            "<nav id=\"mae-explorer\" class=\"mae-explorer\"{} aria-label=\"{}\" \
             data-src=\"{}\" data-version=\"{}\" data-untagged=\"{}\" hidden></nav>",
            words.mark,
            escape(words.s.explorer),
            escape(&e.src),
            escape(&e.version),
            escape(words.s.untagged),
        )
    });
    let aside = if panels.is_empty() {
        String::new()
    } else {
        format!("<aside id=\"mae-panels\">{panels}</aside>")
    };
    format!(
        "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <script src=\"{}\"></script>\
         <link rel=\"stylesheet\" href=\"{}\">\
         <link rel=\"stylesheet\" href=\"{}\">\
         <script type=\"module\" src=\"{}\"></script>\
         <title>{doc_title}</title></head><body>{header}{explorer}<main id=\"mae-main\">{main}</main>{aside}</body></html>\n",
        escape(chrome.lang),
        mae_site::assets::href(mae_site::assets::PREPAINT),
        mae_site::assets::href(mae_site::assets::PALETTE),
        mae_site::assets::href(mae_site::assets::BASE),
        mae_site::assets::href(mae_site::assets::ROUTER),
    )
}

/// The mode toggle and the palette button (P-91, P-93), in the locale's
/// words, inside a box that ships `hidden`: the router's behaviour builds the
/// menu from the palette data the pre-paint script published, and shows the
/// box once it works, so without JavaScript the controls are absent. The box
/// carries the words the menu needs (the default accent's label, each
/// accent's name) for the behaviour to read; core chrome, never content. A
/// fallback marks the box, so the menu the behaviour builds inside it is
/// marked too.
fn theme_controls(words: &Words) -> String {
    let accents: String = words
        .s
        .accents
        .iter()
        .map(|(id, name)| format!(" data-accent-{}=\"{}\"", escape(id), escape(name)))
        .collect();
    format!(
        "<div id=\"mae-theme\" class=\"mae-theme\" hidden{} data-default-label=\"{}\"{accents}>\
         <button type=\"button\" id=\"mae-mode-toggle\" aria-pressed=\"false\">\
         <span class=\"mae-mode-icon\" aria-hidden=\"true\"></span>{}</button>\
         <button type=\"button\" id=\"mae-palette-button\" aria-haspopup=\"menu\" \
         aria-expanded=\"false\" aria-controls=\"mae-palette-menu\">{}</button></div>",
        words.mark,
        escape(words.s.accent_default),
        escape(words.s.mode_toggle),
        escape(words.s.palette_menu),
    )
}

/// A page's tags, each linking to its tag page (P-70); nothing when untagged.
/// The list's name is the catalogue's; on a fallback the list is marked and
/// each tag, which is content, gets the page's language back.
fn tags_list(words: &Words, tags: &[(String, Option<String>)]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let items: String = tags
        .iter()
        .map(|(t, href)| match href {
            Some(href) => format!(
                "<li{}><a href=\"{}\">{}</a></li>",
                words.unmark,
                escape(href),
                escape(t)
            ),
            None => format!("<li{}>{}</li>", words.unmark, escape(t)),
        })
        .collect();
    format!(
        "<ul id=\"mae-tags\"{} aria-label=\"{}\">{items}</ul>",
        words.mark,
        escape(words.s.tags)
    )
}

/// The page's backlinks (P-36), named by the locale's words; nothing when
/// no published page links here.
fn backlinks_nav(words: &Words, backlinks: &[mae_site::Backlink]) -> String {
    if backlinks.is_empty() {
        return String::new();
    }
    let items: String = backlinks
        .iter()
        .map(|b| {
            format!(
                "<li><a href=\"{}\">{}</a></li>",
                escape(&b.href),
                escape(&b.title)
            )
        })
        .collect();
    format!(
        "<nav id=\"mae-backlinks\" aria-labelledby=\"mae-backlinks-h\">\
         <h2 id=\"mae-backlinks-h\"{}>{}</h2><ul>{items}</ul></nav>",
        words.mark,
        escape(words.s.backlinks)
    )
}

/// The Content-Security-Policy every site response carries (D19): scripts and
/// styles from this origin only, no inline script, nothing framed or framing,
/// no base or form target.
pub const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; \
                       connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'; \
                       require-trusted-types-for 'script'; trusted-types mae-router";

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_cannot_break_out_of_its_element_or_its_keyword_line() {
        let html = document(
            "en-US",
            "</title><script>alert(1)</script>",
            "<p>x</p>",
            &[],
            &[],
            &[],
            None,
        );
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;/title&gt;"), "{html}");
        // Neither a title nor a tag can add a keyword line.
        let evil = KbNodeDoc::new(
            "n",
            "T\n#+title: evil\n:END:",
            "b",
            &["ok".into(), "x:\n#+title: evil".into(), "a b".into()],
        );
        let src = page_source("n", &evil).unwrap();
        assert_eq!(
            src.lines().filter(|l| l.starts_with("#+title:")).count(),
            1,
            "{src}"
        );
        assert!(src.contains("#+filetags: :ok:\n"), "{src}");
        let plain = KbNodeDoc::new("n", "t", "b", &[]);
        assert!(page_source("n", &plain).is_some());
        assert_eq!(page_source("a b", &plain), None);
        assert_eq!(page_source("", &plain), None);
    }

    /// A small KB: `a` tagged `t`, `b` untagged and linking to `a`.
    fn kb() -> (mae_site::Site, Addressing) {
        let live = Addressing::live("kb-1").unwrap();
        let src = |id: &str, extra: &str| SourcePage {
            origin: format!("{id}.org"),
            text: format!(":PROPERTIES:\n:ID: {id}\n:END:\n#+title: {id}\n{extra}\n"),
        };
        let site = mae_site::derive(
            &[
                src("a", "#+filetags: :t:\n"),
                src("b", "\n[[id:a][to a]]\n"),
            ],
            &live,
        );
        (site, live)
    }

    /// Every view that lists the KB: what D7 withholds from a query-only member.
    fn listing_views() -> [View; 5] {
        [
            View::Index,
            View::Tags,
            View::Tag("t".into()),
            View::Explorer,
            View::Home(None),
        ]
    }

    /// D7, Amendment 3: a reader who may not list the KB gets each page and
    /// nothing that lists the KB, as the one not-found; its pages neither
    /// point to the explorer nor link their tags.
    #[test]
    fn a_reader_who_may_not_list_the_kb_gets_pages_and_nothing_that_lists_it() {
        let (site, live) = kb();
        for v in listing_views() {
            assert_eq!(
                answer(&site, &live, v.clone(), false),
                SitePage::NotFound,
                "{v:?}"
            );
            assert_ne!(
                answer(&site, &live, v.clone(), true),
                SitePage::NotFound,
                "{v:?}"
            );
        }
        for v in [View::Page("a".into()), View::Home(Some("a".into()))] {
            let SitePage::Found { tags, explorer, .. } = answer(&site, &live, v, false) else {
                panic!("a page is still a page");
            };
            assert_eq!(explorer, None);
            assert_eq!(tags, [("t".to_string(), None)]);
        }
        // And the chrome writes neither the box nor a tag link.
        let SitePage::Found { tags, explorer, .. } =
            answer(&site, &live, View::Page("a".into()), false)
        else {
            panic!()
        };
        let html = document("en-US", "a", "", &[], &tags, &[], explorer.as_ref());
        assert!(!html.contains("mae-explorer"), "{html}");
        assert!(!html.contains("/tags/"), "{html}");
    }

    /// A reader who may list it: every page and listing names the explorer's
    /// data and its version, which is the hash of exactly what
    /// `explorer.json` answers, so a changed KB is a changed version.
    #[test]
    fn a_reader_who_may_list_the_kb_gets_the_explorer_at_the_version_it_answers() {
        let (site, live) = kb();
        let SitePage::Explorer(json) = answer(&site, &live, View::Explorer, true) else {
            panic!("the explorer");
        };
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "groups": [{"tag": "t", "pages": [{"title": "a", "href": "/kb/kb-1/a"}]}],
                "untagged": [{"title": "b", "href": "/kb/kb-1/b"}],
            })
        );
        let want = explorer_ref(&json, &live);
        assert_eq!(want.src, "/kb/kb-1/explorer.json");
        assert_eq!(want.version.len(), 16);
        for view in [View::Page("a".into()), View::Tags, View::Tag("t".into())] {
            let got = match answer(&site, &live, view.clone(), true) {
                SitePage::Found { explorer, tags, .. } => {
                    assert_eq!(
                        tags,
                        [("t".to_string(), Some("/kb/kb-1/tags/t".to_string()))]
                    );
                    explorer
                }
                SitePage::Listing { explorer, .. } => explorer,
                other => panic!("{view:?}: {other:?}"),
            };
            assert_eq!(got.as_ref(), Some(&want), "{view:?}");
        }
        // Another KB, another version.
        let other = mae_site::derive(
            &[SourcePage {
                origin: "c.org".into(),
                text: ":PROPERTIES:\n:ID: c\n:END:\n#+title: c\n".into(),
            }],
            &live,
        );
        let SitePage::Explorer(json2) = answer(&other, &live, View::Explorer, true) else {
            panic!()
        };
        assert_ne!(explorer_ref(&json2, &live).version, want.version);
    }

    /// The box: after the header, before `main`, hidden, its words and
    /// attributes escaped, and marked when the words fell back to en-US.
    #[test]
    fn the_explorer_box_sits_before_main_hidden_with_escaped_attributes() {
        let e = ExplorerRef {
            src: "/kb/k/explorer.json".into(),
            version: "\"><script>".into(),
        };
        let html = document("es-ES", "t", "", &[], &[], &[], Some(&e));
        let nav = html.find("<nav id=\"mae-explorer\"").expect("the box");
        assert!(html.find("</header>").unwrap() < nav);
        assert!(nav < html.find("<main").unwrap());
        assert!(!html.contains("<script>\""), "{html}");
        assert!(html.contains("aria-label=\"P\u{e1}ginas\""), "{html}");
        assert!(html.contains("data-untagged=\"Sin etiqueta\""), "{html}");
        let box_ = &html[nav..html[nav..].find("</nav>").unwrap() + nav];
        assert!(box_.ends_with("hidden>"), "{box_}");
        assert!(!box_.contains("lang="), "{box_}");
        let fallback = document("de", "t", "", &[], &[], &[], Some(&e));
        assert!(
            fallback.contains("class=\"mae-explorer\" lang=\"en-US\" aria-label=\"Pages\""),
            "{fallback}"
        );
        let listing = listing_document("en-US", &Listing::Tags, &[], Some(&e));
        assert!(listing.contains("<nav id=\"mae-explorer\""), "{listing}");
    }
}
