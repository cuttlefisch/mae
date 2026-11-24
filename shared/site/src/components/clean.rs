//! The sanitiser built for slot components (ADR-119 D18, A2; P-155).
//!
//! It is content's allow-list ([`crate::sanitize`]: mae-org's elements and
//! their attributes, `http`/`https`/`mailto` URLs, no comments) with these
//! differences, each because a component is operator markup beside the
//! chrome rather than content inside the article:
//!
//! - **Added elements:** `button` (`type="button"` only) and `nav`, and
//!   `datetime` on `time`. Nothing else: no `script`, `style`, `form`,
//!   `input`, `iframe` or SVG. A component that needs more gets it through an
//!   amendment, not here.
//! - **Classes** keep only `site-…` tokens, so a component can wear neither
//!   the chrome's classes (`mae-…`) nor content's (`org-…`, `language-…`).
//! - **Ids are dropped**, all of them. The chrome's ids start with
//!   [`crate::sanitize::CHROME_ID_PREFIX`] and content's anchors are
//!   mae-org's; a component has no id of its own to collide with either.
//! - **`data-` and ARIA attributes and `role`** survive only when the
//!   component's manifest declares that exact name, and [`attribute_problem`]
//!   refused anything it should not declare. Content's `data-org-…`
//!   attributes are not admitted at all.
//! - **URLs** in `href`/`src` are refused outright (not repaired) when they
//!   hold whitespace, a control character or a backslash, or start with `//`:
//!   each is a way for a browser to read a different host than the text
//!   shows. ammonia's own scheme check runs first.
//!
//! The document head has its own, narrower list ([`Cleaner::head`]): its
//! output goes inside `<head>`, where `meta` can change how the browser
//! treats the whole page. So only `meta[name, content]`, and only for the
//! names in [`HEAD_META_NAMES`], which describe the page and change nothing
//! about it. `http-equiv` (refresh, a CSP, cookies), `charset`, `referrer`
//! (the daemon sets Referrer-Policy) and `viewport`/`color-scheme`/
//! `theme-color` (the chrome's) are therefore never admitted.

use std::borrow::Cow;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::sanitize::{TAGS, TAG_ATTRS};

/// The prefix every component class must carry.
pub const CLASS_PREFIX: &str = "site-";

/// Elements a component may write beyond content's.
const EXTRA_TAGS: &[&str] = &["button", "nav"];

/// `meta` names a document-head component may set.
pub const HEAD_META_NAMES: &[&str] = &[
    "application-name",
    "author",
    "description",
    "generator",
    "keywords",
    "robots",
];

/// WAI-ARIA 1.2's states and properties that take a value, not an id. The
/// deprecated `aria-dropeffect` and `aria-grabbed` are left out.
const ARIA: &[&str] = &[
    "aria-atomic",
    "aria-autocomplete",
    "aria-busy",
    "aria-checked",
    "aria-colcount",
    "aria-colindex",
    "aria-colspan",
    "aria-current",
    "aria-disabled",
    "aria-expanded",
    "aria-haspopup",
    "aria-hidden",
    "aria-invalid",
    "aria-keyshortcuts",
    "aria-label",
    "aria-level",
    "aria-live",
    "aria-modal",
    "aria-multiline",
    "aria-multiselectable",
    "aria-orientation",
    "aria-placeholder",
    "aria-posinset",
    "aria-pressed",
    "aria-readonly",
    "aria-relevant",
    "aria-required",
    "aria-roledescription",
    "aria-rowcount",
    "aria-rowindex",
    "aria-rowspan",
    "aria-selected",
    "aria-setsize",
    "aria-sort",
    "aria-valuemax",
    "aria-valuemin",
    "aria-valuenow",
    "aria-valuetext",
];

/// WAI-ARIA 1.2's attributes whose value is a list of ids. A component has
/// no ids, so one of these could only point at the chrome's or content's
/// elements; `aria-owns` would even move them in the accessibility tree.
const ARIA_ID_REFS: &[&str] = &[
    "aria-activedescendant",
    "aria-controls",
    "aria-describedby",
    "aria-details",
    "aria-errormessage",
    "aria-flowto",
    "aria-labelledby",
    "aria-owns",
];

/// Why a component may not declare the attribute `name`, or `None` if it
/// may. The declarable names are `role`, WAI-ARIA 1.2's value attributes and
/// `data-` keys.
///
/// A `data-` attribute is inert in HTML: the browser never reads it as a URL,
/// a script or a style, so `data-site-href="javascript:…"` is only text. The
/// one reader is the component's own behaviour, which runs under the router's
/// Trusted Types policy (D16) and so cannot turn such a value into markup or
/// script; copying it to the clipboard, as the copy-link example does, is
/// harmless. That is why a URL is fine as data and needs no scheme check.
/// What `data-` keys may not do is wear the chrome's (`data-mae-`) or
/// content's (`data-org-`) namespace, which the router and the chrome read.
///
/// Everything else (`on…` handlers, `style`, `srcdoc`, `href`, `id`, …) is
/// refused because it is not one of those kinds, so no list of dangerous
/// names has to be complete.
pub fn attribute_problem(name: &str) -> Option<&'static str> {
    if name == "role" || ARIA.contains(&name) {
        return None;
    }
    if ARIA_ID_REFS.contains(&name) {
        return Some("names other elements by id, and a component has no ids");
    }
    if name.starts_with("aria-") {
        return Some("is not a WAI-ARIA 1.2 state or property");
    }
    let Some(key) = name.strip_prefix("data-") else {
        return Some("is not a data- key, an ARIA attribute or role");
    };
    if key.starts_with("mae-") || key.starts_with("org-") || key == "mae" || key == "org" {
        return Some("is in the chrome's (data-mae-) or content's (data-org-) namespace");
    }
    if !plain_key(key) {
        return Some(
            "is not a plain data- key: lowercase letters and digits in words joined by single hyphens",
        );
    }
    None
}

/// `[a-z][a-z0-9]*(-[a-z0-9]+)*`: a `data-` key the dataset API maps one to
/// one, with no uppercase (which HTML lowercases) and nothing to escape.
fn plain_key(key: &str) -> bool {
    key.starts_with(|c: char| c.is_ascii_lowercase())
        && key.split('-').all(|w| {
            !w.is_empty()
                && w.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// Whether one class token is a component's: `site-` and at least one more
/// letter, digit, `-` or `_`.
pub fn is_component_class(c: &str) -> bool {
    c.strip_prefix(CLASS_PREFIX).is_some_and(|r| {
        !r.is_empty()
            && r.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    })
}

/// Whether a component's `href`/`src` is admitted. A browser deletes tabs and
/// newlines anywhere in a URL, trims leading spaces and controls, reads `\`
/// as `/`, and takes the host from a `//` start; refusing every such URL is
/// simpler than reading it as a browser would, and a component writes
/// addresses the engine built, which hold none of them.
fn is_component_url(u: &str) -> bool {
    !u.starts_with("//")
        && !u
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '\\')
}

/// One attribute that survived the allow-list, filtered by value.
fn component_attr<'u>(
    declared: &BTreeSet<String>,
    element: &str,
    attr: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    match attr {
        "class" => {
            let kept: Vec<&str> = value
                .split_ascii_whitespace()
                .filter(|c| is_component_class(c))
                .collect();
            (!kept.is_empty()).then(|| kept.join(" ").into())
        }
        "href" | "src" => is_component_url(value).then_some(value.into()),
        "type" if element == "button" => value
            .eq_ignore_ascii_case("button")
            .then_some("button".into()),
        a if a == "role" || a.starts_with("data-") || a.starts_with("aria-") => {
            declared.contains(a).then_some(value.into())
        }
        _ => Some(value.into()),
    }
}

/// The sanitiser for one component's output, admitting exactly `declared`
/// beyond the base list (see the module doc).
fn builder(declared: &BTreeSet<String>) -> ammonia::Builder<'static> {
    let declared = declared.clone();
    let mut tag_attrs: HashMap<&'static str, HashSet<&'static str>> = TAG_ATTRS
        .iter()
        .map(|(t, attrs)| {
            let kept = attrs.iter().copied().filter(|a| !a.starts_with("data-"));
            (*t, kept.collect())
        })
        .collect();
    tag_attrs.entry("time").or_default().insert("datetime");
    tag_attrs.entry("button").or_default().insert("type");
    let mut b = ammonia::Builder::empty();
    b.tags(TAGS.iter().chain(EXTRA_TAGS).copied().collect())
        .clean_content_tags(["script", "style"].into_iter().collect())
        .generic_attributes(["class", "role"].into_iter().collect())
        .generic_attribute_prefixes(["data-", "aria-"].into_iter().collect())
        .tag_attributes(tag_attrs)
        .url_schemes(["http", "https", "mailto"].into_iter().collect())
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(move |element, attr, value| {
            component_attr(&declared, element, attr, value)
        });
    b
}

/// The sanitiser for the document-head slot: `meta[name, content]` only,
/// with `name` one of [`HEAD_META_NAMES`]. [`named_meta_only`] then drops
/// the rest.
fn head_builder() -> ammonia::Builder<'static> {
    let mut b = ammonia::Builder::empty();
    b.tags(["meta"].into_iter().collect())
        .clean_content_tags(["script", "style", "title"].into_iter().collect())
        .tag_attributes(HashMap::from([(
            "meta",
            ["name", "content"].into_iter().collect(),
        )]))
        .url_schemes(HashSet::new())
        .strip_comments(true)
        .attribute_filter(|_, attr, value| match attr {
            "name" => HEAD_META_NAMES.contains(&value).then_some(value.into()),
            _ => Some(value.into()),
        });
    b
}

/// The `meta` tags of the head builder's output that kept a `name`, and
/// nothing else. Text matters here as it does nowhere else: text inside
/// `<head>` makes the parser close the head and open the body, moving
/// whatever follows. ammonia keeps text, so this drops it.
///
/// It reads the output as html5ever serialises it, which is all it needs to
/// read: text escapes `<`, so every `<` opens a tag; attribute values are
/// always double-quoted with `"` escaped, so a tag ends at the first `>`
/// outside quotes, and ` name="` can only be an attribute.
fn named_meta_only(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let tag = &rest[start..];
        let mut quoted = false;
        let end = tag
            .char_indices()
            .find(|&(_, c)| {
                quoted ^= c == '"';
                c == '>' && !quoted
            })
            .map_or(tag.len(), |(i, _)| i + 1);
        if tag.starts_with("<meta ") && tag[..end].contains(" name=\"") {
            out.push_str(&tag[..end]);
        }
        rest = &tag[end..];
    }
    out
}

/// A component's sanitiser: the body's, or the document head's.
pub(super) enum Cleaner {
    Body(ammonia::Builder<'static>),
    Head(ammonia::Builder<'static>),
}

impl Cleaner {
    /// The sanitiser for a component declaring `declared`, in the body.
    pub(super) fn body(declared: &BTreeSet<String>) -> Cleaner {
        Cleaner::Body(builder(declared))
    }

    /// The sanitiser for the document head.
    pub(super) fn head() -> Cleaner {
        Cleaner::Head(head_builder())
    }

    /// The allow-listed form of one render's output.
    pub(super) fn clean(&self, html: &str) -> String {
        match self {
            Cleaner::Body(b) => b.clean(html).to_string(),
            Cleaner::Head(b) => named_meta_only(&b.clean(html).to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_data_keys_aria_values_and_role_can_be_declared() {
        for ok in [
            "role",
            "aria-label",
            "aria-live",
            "data-site-href",
            "data-x",
            "data-a1-b2",
        ] {
            assert_eq!(attribute_problem(ok), None, "{ok}");
        }
        for bad in [
            "onclick",
            "style",
            "srcdoc",
            "href",
            "id",
            "class",
            "data-",
            "data-X",
            "data-a--b",
            "data-a-",
            "data-1a",
            "data-a_b",
            "data-a b",
            "data-a:b",
            "data-mae-x",
            "data-mae",
            "data-org-rel",
            "aria-owns",
            "aria-labelledby",
            "aria-bogus",
            "aria-grabbed",
        ] {
            assert!(attribute_problem(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn component_classes_carry_the_site_prefix() {
        for ok in ["site-a", "site-state-draft", "site-a_b"] {
            assert!(is_component_class(ok), "{ok}");
        }
        for bad in [
            "site-",
            "site",
            "mae-x",
            "org-x",
            "language-rust",
            "site-a.b",
            "x-site-a",
        ] {
            assert!(!is_component_class(bad), "{bad}");
        }
    }

    #[test]
    fn a_url_a_browser_could_read_as_another_host_is_refused() {
        for ok in ["/kb/k/x", "#h-a", "https://example.org/a", "x"] {
            assert!(is_component_url(ok), "{ok}");
        }
        for bad in [
            "//evil", "/\\evil", "/\t/evil", " //evil", "/a\nb", "\u{0}//e",
        ] {
            assert!(!is_component_url(bad), "{bad:?}");
        }
    }
}
