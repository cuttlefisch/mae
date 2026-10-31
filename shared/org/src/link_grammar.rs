//! The one link grammar (ADR-112 D2, ADR-030): what a link's path means.
//!
//! The tree decides where a link is and splits a bracket link's PATH from its
//! DESCRIPTION exactly as `org-element` does (the path stops at the first
//! unescaped `]`, so `][` is found by the tree, never searched for in text).
//! [`classify_link`] reads that path, in one fixed order:
//!
//! 1. **Normalise like Org** (`org-element-link-parser`): a line break inside
//!    the path, with the blanks around it, becomes one space, and
//!    `org-link-unescape` halves every backslash run before `[`, `]` or the
//!    end. Then trim.
//! 2. **Scheme** (ASCII case-insensitive): `id:` is a KB link with the prefix
//!    stripped; `http://`/`https://` is [`LinkKind::Web`]; `mailto:` is
//!    [`LinkKind::Mailto`]; a scheme in [`OTHER_SCHEMES`] is
//!    [`LinkKind::Other`]. A web, mail or other target is kept whole: no store
//!    form, fragment or query is read from it (`?` and `#` belong to the URL).
//!    Anything else is a KB link. This is an EXCLUDE list on purpose: an
//!    org-roam id is often a bare UUID with no namespace, so an include list
//!    would silently drop real edges, while a missed scheme only costs a
//!    broken-link report (the reasoning of the `@ai-caution` on mae-kb's
//!    `EXTERNAL_LINK_SCHEMES`, which this list supersedes).
//! 3. **Store form** `T|D` (MAE's own storage of `[[T][D]]`): the first `|`
//!    ends the target; `D` is [`LinkClass::description`]. Before `?` and `#`,
//!    so a description may contain either (`[[x|What? C# notes]]`).
//! 4. **Query**: the first `?` starts the ADR-030 query, to the end.
//! 5. **Fragment**: the first `#` before the query starts the fragment
//!    (`NODE_ID [#FRAGMENT] [?QUERY]`, ADR-030's order; a `#` after the `?`
//!    is part of the query).
//!
//! MAE had three split orders before this (pinned in
//! `tests/link_grammar.rs`): mae-kb's projector split `][` then `|`, then `?`
//! then `#`; the exporter searched `][` past the link's own `]]`; the help
//! renderer split `|` before `][` and kept the `id:` prefix.

/// What a link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LinkKind {
    /// A KB node (`id:UUID`, `concept:x`, a bare id, a fuzzy `[[name]]`).
    Kb,
    /// `http://` or `https://`.
    Web,
    /// `mailto:`.
    Mailto,
    /// A known non-KB scheme ([`OTHER_SCHEMES`]): never a node, never
    /// rendered as a hyperlink (ADR-112 D3's protocol allowlist is http,
    /// https and mailto).
    Other,
}

impl LinkKind {
    /// The name used in JSON: `kb`, `web`, `mailto`, `other`.
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Kb => "kb",
            LinkKind::Web => "web",
            LinkKind::Mailto => "mailto",
            LinkKind::Other => "other",
        }
    }
}

/// A classified link path.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkClass {
    pub kind: LinkKind,
    /// For a KB link, the node id (no `id:`, store-form description, query
    /// or fragment). Otherwise the whole normalised path, scheme included.
    pub target: String,
    /// `#FRAGMENT` of a KB link (a block index or heading slug); `None` when
    /// absent or empty.
    pub fragment: Option<String>,
    /// `rel=`: the relationship type, [`DEFAULT_REL`] when absent or empty.
    pub rel: String,
    /// `w=`/`weight=`, clamped to 0–1; 1.0 when absent, malformed or not finite.
    pub weight: f64,
    /// `c=`/`conf=`/`confidence=`, as `weight`.
    pub confidence: f64,
    /// Every other `KEY=VALUE` of the query, in source order (a key without
    /// `=` has an empty value). ADR-030: unknown keys are carried, not dropped.
    pub attrs: Vec<(String, String)>,
    /// The store form's description: `D` of `[[T|D]]`.
    pub description: Option<String>,
}

/// The relationship of a link whose query names none.
pub const DEFAULT_REL: &str = "references";

/// Schemes that are never a KB node id. Matched as ASCII case-insensitive
/// prefixes. Only add one that cannot be the start of a node id.
pub const OTHER_SCHEMES: &[&str] = &[
    "ftp://",
    "ftps://",
    "file:",
    "file+sys:",
    "file+emacs:",
    "attachment:",
    "docview:",
    "news:",
    "irc:",
    "info:",
    "help:",
    "shell:",
    "elisp:",
    "man:",
    "doi:",
];

/// Classifies a link path (the text between `[[` and `]]`/`][`, or a plain
/// or angle link's path). Total: any input gives a link; nothing is dropped.
pub fn classify_link(raw: &str) -> LinkClass {
    let path = normalize_path(raw);
    let path = path.trim();
    let starts = |p: &str| {
        path.len() >= p.len() && path.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
    };
    let whole = |kind| LinkClass::whole(kind, path);
    if starts("id:") {
        return kb_link(&path[3..]);
    }
    if starts("http://") || starts("https://") {
        return whole(LinkKind::Web);
    }
    if starts("mailto:") {
        return whole(LinkKind::Mailto);
    }
    if OTHER_SCHEMES.iter().any(|s| starts(s)) {
        return whole(LinkKind::Other);
    }
    kb_link(path)
}

impl LinkClass {
    fn whole(kind: LinkKind, path: &str) -> LinkClass {
        LinkClass {
            kind,
            target: path.to_string(),
            fragment: None,
            rel: DEFAULT_REL.to_string(),
            weight: 1.0,
            confidence: 1.0,
            attrs: Vec::new(),
            description: None,
        }
    }
}

/// Steps 3–5 of the grammar on a KB path (after `id:`).
fn kb_link(path: &str) -> LinkClass {
    let (target, description) = match path.split_once('|') {
        Some((t, d)) => (t, Some(d.trim().to_string())),
        None => (path, None),
    };
    let (target, query) = match target.split_once('?') {
        Some((t, q)) => (t, Some(q)),
        None => (target, None),
    };
    let (target, fragment) = match target.split_once('#') {
        Some((t, f)) => (t, Some(f.trim()).filter(|f| !f.is_empty())),
        None => (target, None),
    };
    let q = query.map(parse_query).unwrap_or_default();
    LinkClass {
        kind: LinkKind::Kb,
        target: target.trim().to_string(),
        fragment: fragment.map(str::to_string),
        rel: q.rel.unwrap_or_else(|| DEFAULT_REL.to_string()),
        weight: q.weight.unwrap_or(1.0),
        confidence: q.confidence.unwrap_or(1.0),
        attrs: q.attrs,
        description,
    }
}

#[derive(Default)]
struct Query {
    rel: Option<String>,
    weight: Option<f64>,
    confidence: Option<f64>,
    attrs: Vec<(String, String)>,
}

/// ADR-030's query: `&`-separated `KEY=VALUE` pairs, keys and values
/// trimmed. Keys are case-sensitive. A repeated recognised key: the last
/// valid value wins. Empty pairs and empty keys are skipped.
fn parse_query(query: &str) -> Query {
    let mut q = Query::default();
    for pair in query.split('&') {
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (k.trim(), v.trim()),
            None => (pair.trim(), ""),
        };
        match key {
            "" => {}
            "rel" => q.rel = Some(value.to_string()).filter(|v| !v.is_empty()).or(q.rel),
            "w" | "weight" => q.weight = unit(value).or(q.weight),
            "c" | "conf" | "confidence" => q.confidence = unit(value).or(q.confidence),
            _ => q.attrs.push((key.to_string(), value.to_string())),
        }
    }
    q
}

/// A finite number, clamped to 0–1; `None` if malformed or not finite.
fn unit(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .map(|f| f.clamp(0.0, 1.0))
}

/// A path as Org displays a link without a description: normalised and
/// trimmed, scheme kept.
pub(crate) fn display_path(raw: &str) -> String {
    normalize_path(raw).trim().to_string()
}

/// `org-element-link-parser`'s normalisation of a path: `[ \t]*\n[ \t]*` →
/// one space (a CR before the LF goes with it), then `org-link-unescape`.
fn normalize_path(raw: &str) -> String {
    let blank = [' ', '\t'];
    let lines: Vec<&str> = raw.split('\n').collect();
    let last = lines.len() - 1;
    let mut out = String::with_capacity(raw.len());
    for (i, line) in lines.into_iter().enumerate() {
        // Each `\n` is one match: blanks before it (and a CR) and after it
        // go, and every match leaves exactly one space.
        let mut l = line;
        if i < last {
            l = l.strip_suffix('\r').unwrap_or(l).trim_end_matches(blank);
        }
        if i > 0 {
            out.push(' ');
            l = l.trim_start_matches(blank);
        }
        out.push_str(l);
    }
    unescape(&out)
}

/// `org-link-unescape`: a run of backslashes followed by `[`, `]` or the end
/// of the string loses half of its backslashes (rounded down).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        let run = b[i..].iter().take_while(|&&c| c == b'\\').count();
        if run == 0 {
            let c = s[i..].chars().next().unwrap_or('\0');
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        let next = b.get(i + run);
        let halve = matches!(next, None | Some(b'[' | b']'));
        let keep = if halve { run / 2 } else { run };
        out.extend(std::iter::repeat_n('\\', keep));
        i += run;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescape_halves_runs_before_brackets_and_the_end_only() {
        assert_eq!(unescape(r"a\]b"), "a]b");
        assert_eq!(unescape(r"a\\\[b"), r"a\[b");
        assert_eq!(unescape(r"a\\"), r"a\");
        assert_eq!(unescape(r"a\\b"), r"a\\b");
        assert_eq!(unescape("é\\"), "é");
    }

    #[test]
    fn line_breaks_in_a_path_become_one_space() {
        assert_eq!(normalize_path("a  \n\t b"), "a b");
        assert_eq!(normalize_path("a\r\nb"), "a b");
        assert_eq!(normalize_path("a\n\nb"), "a  b");
    }
}
