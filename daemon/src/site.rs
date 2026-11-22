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
//! @ai-caution: [architecture-debt] With `webview_enabled` too, the older
//! web view (ADR-073) owns `/kb/<kb-id>/view` and answers it before
//! authentication, so a node whose ID is `view` is not reachable at its own
//! address while both are on: the shadowing P-166 forbids. The live app is
//! ADR-119's successor to that view; the collision is to be resolved there
//! (retire the view's path, or reserve the name and withhold such a page).
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
use mae_site::{derive, Addressing, SourcePage};
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
    },
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
) -> SitePage {
    let (sources, addressing) = match readable_sources(doc_store, kb_id, principal, limits).await {
        Ok(v) => v,
        Err(answer) => return answer,
    };
    // Rendering is CPU work: off the async workers.
    let view = view.clone();
    let answered = tokio::task::spawn_blocking(move || {
        answer(&derive(&sources, &addressing), &addressing, view)
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
        if let Some(text) = page_source(&node_id, &title, &body) {
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

/// A node as the Org text of its page: its ID and title, then its body. A
/// node ID that could not be one property value (whitespace, nothing) has no
/// page; a title's line breaks are folded so it stays one keyword line.
fn page_source(node_id: &str, title: &str, body: &str) -> Option<String> {
    if node_id.is_empty() || node_id.chars().any(char::is_whitespace) {
        return None;
    }
    let title: String = title
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    Some(format!(
        ":PROPERTIES:\n:ID: {node_id}\n:END:\n#+title: {title}\n\n{body}\n"
    ))
}

/// The HTML document for a found page: the core's fixed chrome around the
/// sanitised article (added after sanitising, D19). `title` is escaped here.
pub fn document(
    lang: &str,
    title: &str,
    article: &str,
    backlinks: &[mae_site::Backlink],
    tags: &[(String, String)],
) -> String {
    let words = mae_site::strings::for_locale(lang);
    shell(
        lang,
        title,
        &format!(
            "<article id=\"mae-article\"><h1>{}</h1>{}{article}</article>{}",
            escape(title),
            tags_list(words.tags, tags),
            backlinks_nav(words.backlinks, backlinks),
        ),
    )
}

/// The HTML document for a generated listing: the tag index or one tag's
/// pages (P-70, P-71), headed in the locale's words.
pub fn listing_document(lang: &str, kind: &Listing, items: &[(String, String)]) -> String {
    let words = mae_site::strings::for_locale(lang);
    let heading = match kind {
        Listing::Tags => words.tags.to_string(),
        Listing::Tag(t) => format!("{}{t}{}", words.tag_page_open, words.tag_page_close),
    };
    let list: String = items
        .iter()
        .map(|(text, href)| format!("<li><a href=\"{}\">{}</a></li>", escape(href), escape(text)))
        .collect();
    shell(
        lang,
        &heading,
        &format!(
            "<section id=\"mae-listing\"><h1>{}</h1><ul>{list}</ul></section>",
            escape(&heading)
        ),
    )
}

/// The document around a page's main content: lang, the palette, the title,
/// one main landmark (P-165).
fn shell(lang: &str, title: &str, main: &str) -> String {
    format!(
        "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <link rel=\"stylesheet\" href=\"{}\">\
         <title>{}</title></head><body><main id=\"mae-main\">{main}</main></body></html>\n",
        escape(lang),
        mae_site::assets::href(mae_site::assets::PALETTE),
        escape(title),
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
                       connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

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
        );
        assert!(!html.contains("<script>"), "{html}");
        assert!(html.contains("&lt;/title&gt;"), "{html}");
        let src = page_source("n", "T\n#+title: evil\n:END:", "b").unwrap();
        assert_eq!(
            src.lines().filter(|l| l.starts_with("#+title:")).count(),
            1,
            "{src}"
        );
        assert_eq!(page_source("a b", "t", "b"), None);
        assert_eq!(page_source("", "t", "b"), None);
    }
}
