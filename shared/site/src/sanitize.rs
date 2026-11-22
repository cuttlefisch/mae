//! The final allow-list sanitiser (ADR-119 D18, D19; P-151): every fragment
//! derived from content passes it. The core's own chrome is added after it.
//!
//! The allow-list is mae-org's output vocabulary: its `Tag` and `Attr` are
//! closed enums, so this admits exactly what the renderer can write and
//! nothing a page could add (raw HTML, export blocks, event handlers). A
//! test holds the two in step (`the_allow_list_is_mae_orgs_vocabulary`).
//!
//! `id` is kept only in mae-org's anchor forms (`id-`, `cid-`, `h-`/`h`,
//! `t-N`, `r-N`, `fn-N`, `fnr-N…`), so content can never spell an id the
//! chrome uses: the chrome's ids start with [`CHROME_ID_PREFIX`], which is
//! none of them.

use std::collections::HashSet;

/// Every id the core's chrome writes starts with this. No mae-org anchor
/// does, so content cannot collide with or clobber one.
pub const CHROME_ID_PREFIX: &str = "mae-";

/// mae-org's tags (`mae_org::render::Tag`).
pub(crate) const TAGS: &[&str] = &[
    "a",
    "blockquote",
    "br",
    "caption",
    "code",
    "dd",
    "del",
    "div",
    "dl",
    "dt",
    "em",
    "figcaption",
    "figure",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "hr",
    "img",
    "li",
    "ol",
    "p",
    "pre",
    "section",
    "span",
    "strong",
    "sub",
    "sup",
    "table",
    "tbody",
    "td",
    "th",
    "thead",
    "time",
    "tr",
    "ul",
];

/// mae-org's attributes (`mae_org::render::Attr`), as HTML names.
pub(crate) const ATTRS: &[&str] = &[
    "class",
    "id",
    "href",
    "src",
    "alt",
    "value",
    "scope",
    "data-org-rel",
    "data-org-w",
    "data-org-c",
    "data-org-block",
    "data-org-params",
];

/// Whether `id` is one of mae-org's anchor forms.
pub fn is_content_anchor(id: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    id == "h"
        || ["id-", "cid-", "h-"]
            .iter()
            .any(|p| id.strip_prefix(p).is_some_and(|r| !r.is_empty()))
        || id.strip_prefix("t-").is_some_and(digits)
        || id.strip_prefix("r-").is_some_and(digits)
        || id.strip_prefix("fn-").is_some_and(digits)
        || id
            .strip_prefix("fnr-")
            .is_some_and(|r| r.split('-').all(digits) && r.split('-').count() <= 2)
}

fn builder() -> ammonia::Builder<'static> {
    let mut b = ammonia::Builder::empty();
    b.tags(TAGS.iter().copied().collect::<HashSet<_>>())
        .generic_attributes(ATTRS.iter().copied().collect::<HashSet<_>>())
        .url_schemes(["http", "https", "mailto"].into_iter().collect())
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(|_, attr, value| match attr {
            "id" if !is_content_anchor(value) => None,
            _ => Some(value.into()),
        });
    b
}

/// The allow-listed form of one content-derived HTML fragment.
pub fn sanitize_fragment(html: &str) -> String {
    builder().clean(html).to_string()
}
