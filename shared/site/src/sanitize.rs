//! The final allow-list sanitiser (ADR-119 D18, D19; P-151): every fragment
//! derived from content passes it. The core's own chrome is added after it.
//!
//! The allow-list is mae-org's output vocabulary: its `Tag` and `Attr` are
//! closed enums, so this admits exactly what the renderer can write and
//! nothing a page could add (raw HTML, export blocks, event handlers). A
//! test holds the two in step (every fixture page passes through unchanged).
//!
//! `id` is kept only in mae-org's anchor forms (`id-`, `cid-`, `h-`/`h`,
//! `t-N`, `r-N`, `fn-N`, `fnr-N…`), so content can never spell an id the
//! chrome uses: the chrome's ids start with [`CHROME_ID_PREFIX`], which is
//! none of them. The bare `h` (a heading whose title has no letter or
//! digit) is the one anchor without a prefix: chrome code never reads a
//! global by bare name (`window.h`), so it cannot be clobbered through it.
//!
//! Titles and other text the core hands the chrome are raw text: the chrome
//! escapes them where it writes them.

use std::collections::{HashMap, HashSet};

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

/// mae-org's attributes (`mae_org::render::Attr`), each on the tags mae-org
/// writes it on. `class` and `id` go on any tag, each filtered by value.
pub(crate) const TAG_ATTRS: &[(&str, &[&str])] = &[
    ("a", &["href", "data-org-rel", "data-org-w", "data-org-c"]),
    ("img", &["src", "alt"]),
    ("li", &["value"]),
    ("th", &["scope"]),
    ("div", &["data-org-block", "data-org-params"]),
];

/// Whether one class token is mae-org's: `org-…` or `language-…`. Any other
/// is dropped, so content cannot wear a class the chrome styles or scripts
/// key on.
pub fn is_content_class(c: &str) -> bool {
    ["org-", "language-"].iter().any(|p| {
        c.strip_prefix(p).is_some_and(|r| {
            !r.is_empty()
                && r.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'+' | b'#' | b'.')
                })
        })
    })
}

/// A URL whose target host the browser would take from the URL itself
/// without a scheme: `//host` and, since browsers read `\` as `//` in
/// http(s) URLs, any backslash. ammonia checks schemes, but passes relative
/// URLs through.
fn is_protocol_relative(url: &str) -> bool {
    // As a browser reads it: tab, LF and CR are deleted from ANYWHERE in a
    // URL (`/<TAB>/host` is `//host`), and leading C0 controls and spaces
    // are trimmed.
    let read: String = url
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let u = read.trim_start_matches(|c: char| c == ' ' || c.is_ascii_control());
    u.starts_with("//") || u.contains('\\')
}

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
    let tag_attrs: HashMap<&'static str, HashSet<&'static str>> = TAG_ATTRS
        .iter()
        .map(|(t, a)| (*t, a.iter().copied().collect()))
        .collect();
    b.tags(TAGS.iter().copied().collect::<HashSet<_>>())
        .generic_attributes(["class", "id"].into_iter().collect())
        .tag_attributes(tag_attrs)
        .url_schemes(["http", "https", "mailto"].into_iter().collect())
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(|_, attr, value| match attr {
            "id" if !is_content_anchor(value) => None,
            "class" => {
                let kept: Vec<&str> = value
                    .split_ascii_whitespace()
                    .filter(|c| is_content_class(c))
                    .collect();
                (!kept.is_empty()).then(|| kept.join(" ").into())
            }
            "href" | "src" if is_protocol_relative(value) => None,
            _ => Some(value.into()),
        });
    b
}

/// The allow-listed form of one content-derived HTML fragment.
pub fn sanitize_fragment(html: &str) -> String {
    builder().clean(html).to_string()
}
