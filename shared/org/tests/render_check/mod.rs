//! The renderer's safety oracle, written independently of the renderer: a
//! generic DOM (from the element tree, or from an HTML5 parser), and the
//! checks every output must pass. The allowlist here is typed out by hand
//! from the documented contract, NOT read from the crate's constants, so a
//! change to those constants fails here until the contract is changed too.
#![allow(dead_code)]

use mae_org::{Node, Resolver};
use std::collections::{HashMap, HashSet};

/// A parsed element or text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dom {
    El(String, Vec<(String, String)>, Vec<Dom>),
    Text(String),
}

/// The element tree as a generic DOM.
pub fn from_tree(nodes: &[Node]) -> Vec<Dom> {
    nodes
        .iter()
        .map(|n| match n {
            Node::Text(t) => Dom::Text(t.clone()),
            Node::Element(e) => Dom::El(
                e.tag.as_str().to_string(),
                e.attrs
                    .iter()
                    .map(|(a, v)| (a.as_str().to_string(), v.clone()))
                    .collect(),
                from_tree(&e.children),
            ),
            _ => Dom::Text(String::new()),
        })
        .collect()
}

/// The documented tags.
pub const TAGS: &[&str] = &[
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

/// The documented attributes: (tag, attribute), `*` for every tag.
pub const ATTRS: &[(&str, &str)] = &[
    ("*", "class"),
    ("*", "id"),
    ("a", "href"),
    ("a", "data-org-rel"),
    ("a", "data-org-w"),
    ("a", "data-org-c"),
    ("img", "src"),
    ("img", "alt"),
    ("li", "value"),
    ("th", "scope"),
    ("div", "data-org-block"),
];

const VOID: &[&str] = &["br", "hr", "img"];

/// Characters an emitted URL may hold (after percent-encoding).
fn url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&'()*+,;=%".contains(c)
}

/// An external URL as the contract allows it: one of three schemes,
/// lower-case, then only URL characters.
pub fn is_safe_external(u: &str) -> bool {
    ["http:", "https:", "mailto:"]
        .iter()
        .any(|s| u.starts_with(s) && u.len() > s.len())
        && u.chars().all(url_char)
}

/// An anchor id: an ASCII letter, then ASCII letters, digits and `_.-`,
/// or non-ASCII chars that are one token (a heading slug keeps letters of
/// any script): no whitespace, no control, no format char.
fn is_anchor_id(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|c| c.is_ascii_alphabetic())
        && c.all(|c| {
            c.is_ascii_alphanumeric()
                || "_.-".contains(c)
                || (!c.is_ascii() && c.is_alphanumeric() && !c.is_whitespace())
        })
}

fn token(s: &str, max: usize, extra: &str) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

fn is_class(c: &str) -> bool {
    const FIXED: &[&str] = &[
        "org-center",
        "org-checkbox",
        "org-citation",
        "org-cookie",
        "org-done",
        "org-dynamic-block",
        "org-example",
        "org-figure",
        "org-fixed-width",
        "org-footnote",
        "org-footnote-back",
        "org-footnote-ref",
        "org-footnotes",
        "org-inline-src",
        "org-latex",
        "org-latex-environment",
        "org-link",
        "org-macro",
        "org-priority",
        "org-properties",
        "org-radio",
        "org-radio-target",
        "org-section",
        "org-special-block",
        "org-src",
        "org-table-el",
        "org-tag",
        "org-tags",
        "org-target",
        "org-timestamp",
        "org-todo",
        "org-transclusion",
        "org-underline",
        "org-unresolved",
        "org-verbatim",
        "org-verse",
    ];
    FIXED.contains(&c)
        || c.strip_prefix("org-level-")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        || c.strip_prefix("language-")
            .is_some_and(|l| token(l, 32, "_+#.-"))
}

fn is_unit(v: &str) -> bool {
    v.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && v.parse::<f64>().is_ok_and(|f| (0.0..=1.0).contains(&f))
}

/// What a checked output may point at.
pub struct Contract {
    /// Hrefs the resolver supplied that the contract accepts.
    pub resolver_hrefs: HashSet<String>,
    pub images: bool,
}

impl Contract {
    /// The contract for `resolver`: its hrefs that are plainly safe (no
    /// scheme other than the three, not protocol-relative, nothing to
    /// escape). A test resolver's unsafe hrefs must never appear.
    pub fn new(resolver: &Resolver, images: bool) -> Contract {
        let resolver_hrefs = resolver
            .targets
            .values()
            .map(|r| r.href.clone())
            .filter(|h| {
                let first = h.split(['/', '?', '#']).next().unwrap_or("");
                is_safe_external(h)
                    || (!h.is_empty()
                        && !h.starts_with("//")
                        && !first.contains(':')
                        && h.chars().all(url_char))
            })
            .collect();
        Contract {
            resolver_hrefs,
            images,
        }
    }
}

fn ids(nodes: &[Dom], out: &mut Vec<String>) {
    for n in nodes {
        if let Dom::El(_, attrs, children) = n {
            out.extend(
                attrs
                    .iter()
                    .filter(|(a, _)| a == "id")
                    .map(|(_, v)| v.clone()),
            );
            ids(children, out);
        }
    }
}

/// Every check of the contract over `nodes`; `Err` names the first breach.
pub fn check(nodes: &[Dom], c: &Contract) -> Result<(), String> {
    let mut all = Vec::new();
    ids(nodes, &mut all);
    let mut seen = HashSet::new();
    for id in &all {
        if !seen.insert(id.clone()) {
            return Err(format!("duplicate id {id:?}"));
        }
    }
    walk(nodes, c, &seen)
}

fn walk(nodes: &[Dom], c: &Contract, ids: &HashSet<String>) -> Result<(), String> {
    for n in nodes {
        let (tag, attrs, children) = match n {
            Dom::El(tag, attrs, children) => (tag, attrs, children),
            Dom::Text(t) => {
                // The tree's text holds no control character but tab and
                // newline, and no DEL: none is printable, and an HTML
                // parser drops or replaces some of them.
                if let Some(c) = t.chars().find(|c| is_unprintable(*c)) {
                    return Err(format!("text holds control character {c:?}"));
                }
                continue;
            }
        };
        if !TAGS.contains(&tag.as_str()) {
            return Err(format!("tag <{tag}> is not allowed"));
        }
        if VOID.contains(&tag.as_str()) && !children.is_empty() {
            return Err(format!("void <{tag}> has children"));
        }
        for (a, v) in attrs {
            let allowed = ATTRS.iter().any(|(t, x)| x == a && (*t == "*" || t == tag));
            if !allowed {
                return Err(format!("attribute {a} on <{tag}> is not allowed"));
            }
            value(tag, a, v, c, ids)?;
        }
        walk(children, c, ids)?;
    }
    Ok(())
}

fn href_ok(v: &str, c: &Contract, ids: &HashSet<String>) -> bool {
    if let Some(id) = v.strip_prefix('#') {
        return ids.contains(id);
    }
    if is_safe_external(v) {
        return true;
    }
    let (base, frag) = match v.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (v, None),
    };
    c.resolver_hrefs.contains(v)
        || (c.resolver_hrefs.contains(base) && frag.is_some_and(is_anchor_like))
}

/// A fragment this crate appends to a resolver href: an anchor id with
/// every non-ASCII byte percent-encoded.
fn is_anchor_like(f: &str) -> bool {
    let b = f.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |k: usize| b.get(k).is_some_and(u8::is_ascii_hexdigit);
            if !(hex(i + 1) && hex(i + 2)) {
                return false;
            }
            i += 3;
        } else if b[i].is_ascii_alphanumeric() || b"_.-".contains(&b[i]) {
            i += 1;
        } else {
            return false;
        }
    }
    !f.is_empty()
}

fn value(tag: &str, a: &str, v: &str, c: &Contract, ids: &HashSet<String>) -> Result<(), String> {
    let ok = match a {
        "class" => v.split(' ').all(is_class) && !v.is_empty(),
        "id" => is_anchor_id(v),
        "href" => href_ok(v, c, ids),
        "src" => {
            c.images && (v.starts_with("http:") || v.starts_with("https:")) && is_safe_external(v)
        }
        "alt" => true,
        "value" => !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()),
        "scope" => v == "col",
        "data-org-rel" => token(v, 64, "_.:-"),
        "data-org-w" | "data-org-c" => is_unit(v),
        "data-org-block" => token(v, 64, "_-"),
        _ => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!("<{tag} {a}={v:?}> breaks the contract"))
    }
}

/// The concatenated text of `nodes`.
pub fn all_text(nodes: &[Dom]) -> String {
    let mut out = String::new();
    for n in nodes {
        match n {
            Dom::Text(t) => out.push_str(t),
            Dom::El(_, attrs, ch) => {
                attrs.iter().for_each(|(_, v)| out.push_str(v));
                out.push_str(&all_text(ch));
            }
        }
    }
    out
}

/// A resolver that resolves every second KB target of `targets` (sorted,
/// so the choice is deterministic) to `/n/<i>.html`, and a few to hostile
/// hrefs it must refuse.
pub fn adversarial_resolver(targets: &[String]) -> Resolver {
    let mut sorted: Vec<&String> = targets.iter().collect();
    sorted.sort();
    sorted.dedup();
    let hostile = [
        "javascript:alert(1)",
        "//evil.example/x",
        "/ok\" onmouseover=\"x",
        " JaVaScRiPt:alert(1)",
        "data:text/html,x",
    ];
    let mut r = Resolver::default();
    for (i, t) in sorted.iter().enumerate() {
        let href = match i % 4 {
            0 => format!("/n/{i}.html"),
            1 => hostile[i % hostile.len()].to_string(),
            2 => format!("https://kb.example/{i}"),
            _ => continue,
        };
        let title = (i % 3 == 0).then(|| format!("T{i}"));
        r.targets.insert((*t).clone(), resolved(href, title));
    }
    r
}

/// Every KB link target of `sem`, plus transclusion members.
pub fn kb_targets(sem: &mae_org::Semantics) -> Vec<String> {
    let mut v: Vec<String> = sem.links.iter().map(|l| l.target.clone()).collect();
    v.extend(sem.transclusions.iter().map(|t| t.member.clone()));
    v
}

/// Counts of each tag, for sanity checks.
pub fn tag_counts(nodes: &[Dom], out: &mut HashMap<String, usize>) {
    for n in nodes {
        if let Dom::El(t, _, ch) = n {
            *out.entry(t.clone()).or_default() += 1;
            tag_counts(ch, out);
        }
    }
}

/// A resolver target, built as an external crate must build it.
pub fn resolved(href: String, title: Option<String>) -> mae_org::Resolved {
    let mut r = mae_org::Resolved::new(href);
    r.title = title;
    r
}

/// A C0 control character other than tab and newline, or DEL.
pub fn is_unprintable(c: char) -> bool {
    (c < ' ' && c != '\t' && c != '\n') || c == '\u{7f}'
}
