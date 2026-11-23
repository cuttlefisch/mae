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
    /// backlinks, and its tags as `(tag, href)`.
    Found {
        title: String,
        html: String,
        backlinks: Vec<mae_site::Backlink>,
        tags: Vec<(String, String)>,
        toc: Vec<mae_site::TocEntry>,
    },
    /// The content index (P-77): one entry per published page.
    Index(Vec<mae_site::IndexEntry>),
    /// A listing the engine generates: the tag index or one tag's pages, as
    /// `(text, href)` entries.
    Listing {
        kind: Listing,
        items: Vec<(String, String)>,
    },
    /// Every refusal and every absence, alike.
    NotFound,
    /// The KB is larger than this listener derives per request.
    TooLarge,
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
    let (sources, addressing) = match readable_sources(doc_store, kb_id, principal, limits).await {
        Ok(v) => v,
        Err(answer) => return answer,
    };
    // Rendering is CPU work: off the async workers.
    let view = view.clone();
    let opts = opts.clone();
    let answered = tokio::task::spawn_blocking(move || {
        answer(
            &derive_with(&sources, &addressing, &opts),
            &addressing,
            view,
        )
    })
    .await;
    answered.unwrap_or(SitePage::NotFound)
}

/// The KB's pages as sources, once the read gate admits `principal` and the
/// KB is within `limits`; otherwise the answer to give.
async fn readable_sources(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    limits: SiteLimits,
) -> Result<(Vec<SourcePage>, Addressing), SitePage> {
    let Ok(addressing) = Addressing::live(kb_id) else {
        return Err(SitePage::NotFound);
    };
    let Ok((coll, encryption)) = kb_query::load_gated(doc_store, kb_id, principal).await else {
        return Err(SitePage::NotFound);
    };
    if !matches!(encryption, Encryption::None) {
        return Err(SitePage::NotFound);
    }
    let nodes = coll.list_nodes();
    if nodes.len() > limits.max_scan_nodes {
        return Err(SitePage::TooLarge);
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
            return Err(SitePage::TooLarge);
        }
        if let Some(text) = page_source(&node_id, &doc) {
            sources.push(SourcePage {
                origin: node_id,
                text,
            });
        }
    }
    Ok((sources, addressing))
}

/// The answer to `view` from a derived KB.
fn answer(site: &mae_site::Site, addressing: &Addressing, view: View) -> SitePage {
    match view {
        // The declared node when this reader has it as a page; otherwise the
        // tag index. Unpublished, withheld and undeclared all answer alike, so
        // the home address says nothing about a node the reader cannot see.
        View::Home(declared) => match declared.filter(|id| site.pages.contains_key(id)) {
            Some(id) => answer(site, addressing, View::Page(id)),
            None => answer(site, addressing, View::Tags),
        },
        View::Page(id) => match site.pages.get(&id) {
            Some(p) => SitePage::Found {
                title: p.title.clone(),
                html: p.html.clone(),
                backlinks: p.backlinks.clone(),
                tags: p
                    .tags
                    .iter()
                    .map(|t| (t.clone(), addressing.tag_href(t)))
                    .collect(),
                toc: p.toc.clone(),
            },
            None => SitePage::NotFound,
        },
        View::Tags => SitePage::Listing {
            kind: Listing::Tags,
            items: site
                .tags
                .keys()
                .map(|t| (t.clone(), addressing.tag_href(t)))
                .collect(),
        },
        View::Index => SitePage::Index(site.index.clone()),
        View::Tag(tag) => match site.tags.get(&tag) {
            Some(pages) => SitePage::Listing {
                kind: Listing::Tag(tag),
                items: pages
                    .iter()
                    .map(|p| (p.title.clone(), p.href.clone()))
                    .collect(),
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

/// The HTML document for a found page: the core's fixed chrome around the
/// sanitised article (added after sanitising, D19). `title` is escaped here.
pub fn document<'a>(
    chrome: impl Into<Chrome<'a>>,
    title: &str,
    article: &str,
    backlinks: &[mae_site::Backlink],
    tags: &[(String, String)],
    toc: &[mae_site::TocEntry],
) -> String {
    let chrome = chrome.into();
    let words = mae_site::strings::for_locale(chrome.lang);
    let article = format!(
        "<article id=\"mae-article\"><h1>{}</h1>{}{article}</article>",
        escape(title),
        tags_list(words.tags, tags),
    );
    // The side panels sit outside the main landmark (P-165).
    let panels = format!(
        "{}{}",
        toc_nav(words.contents, toc),
        backlinks_nav(words.backlinks, backlinks)
    );
    shell(chrome, title, &article, &panels)
}

/// The page's table of contents (P-79): one link per heading, to the id the
/// heading carries on this page; nothing for a page with fewer than two.
fn toc_nav(label: &str, toc: &[mae_site::TocEntry]) -> String {
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
        "<nav id=\"mae-toc\" aria-labelledby=\"mae-toc-h\"><h2 id=\"mae-toc-h\">{}</h2><ol>{items}</ol></nav>",
        escape(label)
    )
}

/// The HTML document for a generated listing: the tag index or one tag's
/// pages (P-70, P-71), headed in the locale's words.
pub fn listing_document<'a>(
    chrome: impl Into<Chrome<'a>>,
    kind: &Listing,
    items: &[(String, String)],
) -> String {
    let chrome = chrome.into();
    let words = mae_site::strings::for_locale(chrome.lang);
    let heading = match kind {
        Listing::Tags => words.tags.to_string(),
        Listing::Tag(t) => format!("{}{t}{}", words.tag_page_open, words.tag_page_close),
    };
    let list: String = items
        .iter()
        .map(|(text, href)| format!("<li><a href=\"{}\">{}</a></li>", escape(href), escape(text)))
        .collect();
    shell(
        chrome,
        &heading,
        &format!(
            "<section id=\"mae-listing\"><h1>{}</h1><ul>{list}</ul></section>",
            escape(&heading)
        ),
        "",
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
fn shell(chrome: Chrome<'_>, title: &str, main: &str, panels: &str) -> String {
    let words = mae_site::strings::for_locale(chrome.lang);
    let (doc_title, site_link) = match chrome.site {
        Some((site, home)) => (
            format!("{} \u{2014} {}", escape(title), escape(site)),
            format!("<a href=\"{}\">{}</a>", escape(home), escape(site)),
        ),
        None => (escape(title), String::new()),
    };
    let header = format!(
        "<header id=\"mae-header\">{site_link}{}</header>",
        theme_controls(words)
    );
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
         <title>{doc_title}</title></head><body>{header}<main id=\"mae-main\">{main}</main>{aside}</body></html>\n",
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
/// accent's name) for the behaviour to read; core chrome, never content.
fn theme_controls(words: &mae_site::strings::Strings) -> String {
    let accents: String = words
        .accents
        .iter()
        .map(|(id, name)| format!(" data-accent-{}=\"{}\"", escape(id), escape(name)))
        .collect();
    format!(
        "<div id=\"mae-theme\" class=\"mae-theme\" hidden data-default-label=\"{}\"{accents}>\
         <button type=\"button\" id=\"mae-mode-toggle\" aria-pressed=\"false\">\
         <span class=\"mae-mode-icon\" aria-hidden=\"true\"></span>{}</button>\
         <button type=\"button\" id=\"mae-palette-button\" aria-haspopup=\"menu\" \
         aria-expanded=\"false\" aria-controls=\"mae-palette-menu\">{}</button></div>",
        escape(words.accent_default),
        escape(words.mode_toggle),
        escape(words.palette_menu),
    )
}

/// A page's tags, each linking to its tag page (P-70); nothing when untagged.
fn tags_list(label: &str, tags: &[(String, String)]) -> String {
    if tags.is_empty() {
        return String::new();
    }
    let items: String = tags
        .iter()
        .map(|(t, href)| format!("<li><a href=\"{}\">{}</a></li>", escape(href), escape(t)))
        .collect();
    format!(
        "<ul id=\"mae-tags\" aria-label=\"{}\">{items}</ul>",
        escape(label)
    )
}

/// The page's backlinks (P-36), named by the locale's words; nothing when
/// no published page links here.
fn backlinks_nav(label: &str, backlinks: &[mae_site::Backlink]) -> String {
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
         <h2 id=\"mae-backlinks-h\">{}</h2><ul>{items}</ul></nav>",
        escape(label)
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
}
