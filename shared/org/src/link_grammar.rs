//! The one link grammar (ADR-112 D2, ADR-030): what a link's path means.
//!
//! The tree decides where a link is and splits a bracket link's PATH from its
//! DESCRIPTION exactly as `org-element` does (the path stops at the first
//! unescaped `]`, so `][` is found by the tree, never searched for in text).
//! [`classify_link_with`] reads that path, in one fixed order:
//!
//! 1. **Normalise like Org** (`org-element-link-parser`): a line break inside
//!    the path, with the blanks around it, becomes one space, and
//!    `org-link-unescape` halves every backslash run before `[`, `]` or the
//!    end. NOT trimmed: Org reads the type from the path's first character,
//!    so `[[ id:x]]` is the fuzzy link ` id:x`, not an id link.
//! 2. **Kind, as `org-element-link-parser` decides the type**, in its order:
//!    a path that is a file name (`/…`, `~…`, `./…`, `../…`) is a file link,
//!    [`LinkKind::Other`]; then a known scheme `TYPE:` (ASCII
//!    case-insensitive, as Org matches it: `Mailto:`, `ID:`): `id` is a KB
//!    link with the prefix stripped, the [`LinkTypes`] web schemes (`http`,
//!    `https`: `[[https:x]]` is web, as in Org) are [`LinkKind::Web`], `mailto`
//!    is [`LinkKind::Mailto`], every other listed scheme [`LinkKind::Other`];
//!    then `(REF)` is a [`LinkKind::Coderef`], `#ID` a [`LinkKind::CustomId`],
//!    and `*TITLE` a [`LinkKind::Heading`] (Org's fuzzy search restricted to
//!    headlines). These three are local targets in the same file, never KB
//!    edges. Anything else (a bare id, `concept:x`, a fuzzy name, an unknown
//!    scheme) is a KB link. A web, mail, other or local target is kept whole
//!    (normalised, trimmed): no store form, query or fragment is read from it
//!    (`?` and `#` belong to the URL), except a file link's search option.
//!    The scheme lists are an EXCLUDE list on purpose: an org-roam id is
//!    often a bare UUID with no namespace, so an include list would silently
//!    drop real edges, while a missed scheme only costs a broken-link report
//!    (the reasoning of the `@ai-caution` on mae-kb's `EXTERNAL_LINK_SCHEMES`,
//!    which [`OTHER_SCHEMES`] supersedes).
//!
//! Then, on a KB path (after `id:`):
//!
//! 3. **Store form** `T|D` (MAE's own storage of `[[T][D]]`): the first `|`
//!    ends the target; `D` is [`LinkClass::description`]. Before `?` and `#`,
//!    so a description may contain either (`[[x|What? C# notes]]`).
//! 4. **Query**: the first `?` starts the ADR-030 query, to the end. Before
//!    the search option: the query is MAE's metadata about the EDGE, outside
//!    Org's path like the store form, so `[[id:x::*h?rel=r]]` keeps its
//!    relation. A search string cannot contain `?` (it starts the query).
//! 5. **Search option** (`id:` links only): the first `::` ends the id and
//!    the rest is [`LinkClass::search`], as `org-id-open` reads it
//!    (`"::\\(.*\\)\\'"`, org-id.el) and org-roam stores it. Org 9.7 writes this
//!    form by default (`org-id-link-use-context`). A file link (`file:`,
//!    `file+sys:`, `file+emacs:`, or a file name) is split the same way, as
//!    `org-element-link-parser` does (its `:search-option`); its target is
//!    the path without the option.
//! 6. **Fragment**: the first `#` before the query (and, for an id link,
//!    before `::`) starts the fragment (`NODE_ID [#FRAGMENT] [?QUERY]`,
//!    ADR-030's order; a `#` after `::` belongs to the search, so
//!    `[[id:x::#custom]]` keeps Org's custom-id search).
//!
//! Every trim is Org's (`org-trim`: space, tab, LF, CR), never Unicode
//! whitespace: a no-break space is content, as in Emacs.
//!
//! MAE had three split orders before this (pinned in
//! `tests/link_grammar.rs`): mae-kb's projector split `][` then `|`, then `?`
//! then `#`; the exporter searched `][` past the link's own `]]`; the help
//! renderer split `|` before `][` and kept the `id:` prefix. mae-kb also kept
//! `::search` in the target (`x::*h`, an id that never exists).

use std::sync::OnceLock;

/// What a link points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum LinkKind {
    /// A KB node (`id:UUID`, `concept:x`, a bare id, a fuzzy `[[name]]`).
    Kb,
    /// A web scheme of [`LinkTypes::web`] (`http:`, `https:`).
    Web,
    /// `mailto:`.
    Mailto,
    /// A known non-KB scheme ([`OTHER_SCHEMES`]) or a file name: never a
    /// node, never rendered as a hyperlink (ADR-112 D3's protocol allowlist
    /// is http, https and mailto).
    Other,
    /// `[[*Title]]`: a headline of this file, found by its title.
    Heading,
    /// `[[#id]]`: the element of this file with that `CUSTOM_ID`.
    CustomId,
    /// `[[(ref)]]`: a code reference in a source block of this file.
    Coderef,
}

impl LinkKind {
    /// The name used in JSON: `kb`, `web`, `mailto`, `other`, `heading`,
    /// `custom_id`, `coderef`.
    pub fn as_str(self) -> &'static str {
        match self {
            LinkKind::Kb => "kb",
            LinkKind::Web => "web",
            LinkKind::Mailto => "mailto",
            LinkKind::Other => "other",
            LinkKind::Heading => "heading",
            LinkKind::CustomId => "custom_id",
            LinkKind::Coderef => "coderef",
        }
    }
}

/// A classified link path.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct LinkClass {
    pub kind: LinkKind,
    /// For a KB link, the node id (no `id:`, store-form description, query,
    /// search option or fragment). For a heading, custom-id or coderef link,
    /// the title, id or reference (`H` of `*H`). Otherwise the whole
    /// normalised path, scheme included, without a file link's search option.
    pub target: String,
    /// `#FRAGMENT` of a KB link (a block index or heading slug); `None` when
    /// absent or empty.
    pub fragment: Option<String>,
    /// The search option after `::` of an `id:` or file link (`*h` of
    /// `id:x::*h`), as written; `None` when absent.
    pub search: Option<String>,
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

/// The schemes a web link may have.
pub const WEB_SCHEMES: &[&str] = &["http", "https"];

/// The schemes a mail link may have.
pub const MAIL_SCHEMES: &[&str] = &["mailto"];

/// Schemes that are never a KB node id, without their colon. Matched ASCII
/// case-insensitively against the text before a path's first `:`.
///
/// Org's own link types in `emacs -Q` (Org 9.7.11,
/// [`crate::DEFAULT_LINK_TYPES`]) except `id`, `http`, `https` and `mailto`;
/// then `ftps`, `attachment` and `man` (Org link types in modules `emacs -Q`
/// does not load), and the bibliography schemes `cite` (org-ref),
/// `zotero` and `roam` (org-roam's `roam:` link to a title, not an id).
///
/// @ai-caution: [kb-links] Deliberately an EXCLUDE list. Adding a scheme here
/// turns every link that starts with it into a non-edge on every peer and the
/// daemon at once; a node id that happens to start with it loses its edges
/// silently. Only add a scheme that cannot be the start of a node id (MAE's
/// ids are `namespace:slug` or bare UUIDs), and never turn this into an
/// include list: an org-roam id has no namespace to include.
pub const OTHER_SCHEMES: &[&str] = &[
    "eww",
    "rmail",
    "mhe",
    "irc",
    "info",
    "gnus",
    "docview",
    "bibtex",
    "bbdb",
    "w3m",
    "doi",
    "file+sys",
    "file+emacs",
    "shell",
    "news",
    "ftp",
    "help",
    "file",
    "elisp",
    "ftps",
    "attachment",
    "man",
    "cite",
    "zotero",
    "roam",
];

/// Which link schemes are web, mail or other (never a KB node): declared
/// data (ADR-112 D4), so a KB can extend the defaults and every peer reads
/// its links alike. Names are without the colon, matched ASCII
/// case-insensitively. A scheme in none of the lists, other than `id`, is a
/// KB link.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LinkTypes {
    pub web: Vec<String>,
    pub mail: Vec<String>,
    pub other: Vec<String>,
}

impl Default for LinkTypes {
    /// [`WEB_SCHEMES`], [`MAIL_SCHEMES`] and [`OTHER_SCHEMES`].
    fn default() -> Self {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
        LinkTypes {
            web: own(WEB_SCHEMES),
            mail: own(MAIL_SCHEMES),
            other: own(OTHER_SCHEMES),
        }
    }
}

impl LinkTypes {
    /// The defaults, built once.
    pub fn defaults() -> &'static LinkTypes {
        static DEFAULTS: OnceLock<LinkTypes> = OnceLock::new();
        DEFAULTS.get_or_init(LinkTypes::default)
    }
}

/// [`classify_link_with`] under the default [`LinkTypes`].
pub fn classify_link(raw: &str) -> LinkClass {
    classify_link_with(raw, LinkTypes::defaults())
}

/// Classifies a link path (the text between `[[` and `]]`/`][`, or a plain
/// or angle link's path). Total: any input gives a link; nothing is dropped.
/// `raw` is a path as Emacs's buffer holds it: line endings as `\n` (see
/// [`crate::Link`]'s path for a parsed link).
pub fn classify_link_with(raw: &str, types: &LinkTypes) -> LinkClass {
    classify_normalized(&normalize_path(raw), types)
}

/// [`classify_link_with`] on a path already normalised (and, for a
/// bracket link, its `#+LINK` abbreviation expanded: see
/// [`crate::LinkAbbrevs::bracket_path`]). Normalising twice would unescape
/// twice.
pub(crate) fn classify_normalized(path: &str, types: &LinkTypes) -> LinkClass {
    if is_file_name(path) {
        return file_link(path);
    }
    let scheme = path.split_once(':').map(|(s, _)| s);
    let is =
        |list: &[String]| scheme.is_some_and(|s| list.iter().any(|t| t.eq_ignore_ascii_case(s)));
    if scheme.is_some_and(|s| s.eq_ignore_ascii_case("id")) {
        return kb_link(&path[3..], true);
    }
    if is(&types.web) {
        return LinkClass::whole(LinkKind::Web, org_trim(path));
    }
    if is(&types.mail) {
        return LinkClass::whole(LinkKind::Mailto, org_trim(path));
    }
    if is(&types.other) {
        let file = scheme.is_some_and(|s| {
            let s = s.to_ascii_lowercase();
            s == "file" || s.starts_with("file+")
        });
        if file {
            return file_link(path);
        }
        return LinkClass::whole(LinkKind::Other, org_trim(path));
    }
    if path.len() >= 2 && path.starts_with('(') && path.ends_with(')') {
        return LinkClass::whole(LinkKind::Coderef, org_trim(&path[1..path.len() - 1]));
    }
    if let Some(rest) = path.strip_prefix('#') {
        return LinkClass::whole(LinkKind::CustomId, org_trim(rest));
    }
    if let Some(rest) = path.strip_prefix('*') {
        return LinkClass::whole(LinkKind::Heading, org_trim(rest));
    }
    kb_link(path, false)
}

/// Is `path` a file name to Org (`org-element-link-parser` asks
/// `file-name-absolute-p`, and checks `./` and `../`)? `/` and `~`
/// paths are absolute. For `~user` Emacs's answer depends on the machine
/// (whether that user exists); here it is always a file name, so the
/// answer is the same on every peer (registered: `objects-link-view.org`).
pub(crate) fn is_file_name(path: &str) -> bool {
    ["/", "~", "./", "../"].iter().any(|p| path.starts_with(p))
}

/// Org's `org-trim`: space, tab, LF and CR at both ends; any other
/// character (a no-break space) is content.
pub(crate) fn org_trim(s: &str) -> &str {
    s.trim_matches([' ', '\t', '\n', '\r'])
}

impl LinkClass {
    fn whole(kind: LinkKind, target: &str) -> LinkClass {
        LinkClass {
            kind,
            target: target.to_string(),
            fragment: None,
            search: None,
            rel: DEFAULT_REL.to_string(),
            weight: 1.0,
            confidence: 1.0,
            attrs: Vec::new(),
            description: None,
        }
    }
}

/// A file link: the path without its `::search` option, which Org splits at
/// the first `::`.
fn file_link(path: &str) -> LinkClass {
    let (target, search) = split_search(path);
    let mut c = LinkClass::whole(LinkKind::Other, org_trim(target));
    c.search = search;
    c
}

/// Splits at the first `::`: (before, the rest).
fn split_search(path: &str) -> (&str, Option<String>) {
    match path.split_once("::") {
        Some((t, s)) => (t, Some(s.to_string())),
        None => (path, None),
    }
}

/// Steps 3–6 of the grammar on a KB path (after `id:`); `id` says whether
/// it had one, which is when `::` starts a search option.
fn kb_link(path: &str, id: bool) -> LinkClass {
    let (target, description) = match path.split_once('|') {
        Some((t, d)) => (t, Some(org_trim(d).to_string())),
        None => (path, None),
    };
    let (target, query) = match target.split_once('?') {
        Some((t, q)) => (t, Some(q)),
        None => (target, None),
    };
    let (target, search) = if id {
        split_search(target)
    } else {
        (target, None)
    };
    let (target, fragment) = match target.split_once('#') {
        Some((t, f)) => (t, Some(org_trim(f)).filter(|f| !f.is_empty())),
        None => (target, None),
    };
    let q = query.map(parse_query).unwrap_or_default();
    LinkClass {
        kind: LinkKind::Kb,
        target: org_trim(target).to_string(),
        fragment: fragment.map(str::to_string),
        search,
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
    /// The recognised keys written with a value refused (`rel`, `w`, `c`).
    rejected: Vec<&'static str>,
}

/// A link relation name: a word of lower-case letters, `_` and `-`,
/// `[a-z][a-z_-]{0,63}`. MAE's vocabulary is open (a KB registers its
/// `rel_types`, ADR-030/101), so this is a shape: the one every relation
/// MAE seeds has (`references`, `part_of`, `categorized_under` …),
/// ADR-030's own (`teaches`, `cites`) and hyphenated ones
/// (`depends-on`); an id or a number (any digit) is none. The renderer emits `data-org-rel` only for one; the
/// semantics keep a `rel=` as written, as the KB's own projection does
/// (the KB validates relations when it writes them, ADR-101).
pub fn is_valid_rel(rel: &str) -> bool {
    let b = rel.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || matches!(c, b'_' | b'-'))
}

/// The recognised query keys of KB link `path` (`rel`, `w`, `c`) written
/// with a value the renderer refuses: a relation not of
/// [`is_valid_rel`]'s shape, a weight or confidence that is not a finite
/// number (which the semantics read as absent). Empty ones count as
/// absent.
pub(crate) fn rejected_query_keys(path: &str) -> Vec<&'static str> {
    let target = path.split_once('|').map_or(path, |(t, _)| t);
    target
        .split_once('?')
        .map(|(_, q)| parse_query(q).rejected)
        .unwrap_or_default()
}

/// ADR-030's query: `&`-separated `KEY=VALUE` pairs, keys and values
/// trimmed (Org's trim). Keys are case-sensitive. A repeated recognised key: the last
/// valid value wins. Empty pairs and empty keys are skipped.
fn parse_query(query: &str) -> Query {
    let mut q = Query::default();
    for pair in query.split('&') {
        let (key, value) = match pair.split_once('=') {
            Some((k, v)) => (org_trim(k), org_trim(v)),
            None => (org_trim(pair), ""),
        };
        match key {
            "" => {}
            "rel" => {
                if !value.is_empty() && !is_valid_rel(value) {
                    q.rejected.push("rel");
                }
                q.rel = Some(value.to_string()).filter(|v| !v.is_empty()).or(q.rel)
            }
            "w" | "weight" if !value.is_empty() && unit(value).is_none() => q.rejected.push("w"),
            "w" | "weight" => q.weight = unit(value).or(q.weight),
            "c" | "conf" | "confidence" if !value.is_empty() && unit(value).is_none() => {
                q.rejected.push("c")
            }
            "c" | "conf" | "confidence" => q.confidence = unit(value).or(q.confidence),
            _ => q.attrs.push((key.to_string(), value.to_string())),
        }
    }
    q
}

/// A finite number, clamped to 0–1 (a negative zero is zero); `None` if
/// malformed or not finite.
fn unit(value: &str) -> Option<f64> {
    value
        .parse::<f64>()
        .ok()
        .filter(|f| f.is_finite())
        .map(|f| f.clamp(0.0, 1.0) + 0.0)
}

/// A path as Org displays a link without a description: normalised and
/// trimmed, scheme kept.
pub(crate) fn display_path(raw: &str) -> String {
    org_trim(&normalize_path(raw)).to_string()
}

/// `org-element-link-parser`'s normalisation of a path: `[ \t]*\n[ \t]*` →
/// one space, then `org-link-unescape`. The input is the path as Emacs's
/// buffer holds it: line endings as `\n` whatever the file's convention
/// (`Link::path_text` gives that), a CR that is content kept as a CR, as Org
/// keeps it. The single normaliser for link paths (principle #8).
pub(crate) fn normalize_path(raw: &str) -> String {
    let blank = [' ', '\t'];
    let lines: Vec<&str> = raw.split('\n').collect();
    let last = lines.len() - 1;
    let mut out = String::with_capacity(raw.len());
    for (i, line) in lines.into_iter().enumerate() {
        // Each `\n` is one match: blanks before and after it go, and every
        // match leaves exactly one space.
        let mut l = line;
        if i < last {
            l = l.trim_end_matches(blank);
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
    fn a_negative_zero_weight_is_zero() {
        // `-0` parses to -0.0, which clamps to itself and would be written
        // "-0" (a renderer's data-org-w, the semantics JSON).
        for q in ["w=-0", "w=-0.0", "c=-0"] {
            let c = classify_link(&format!("x?{q}"));
            assert!(
                c.weight.is_sign_positive() && c.confidence.is_sign_positive(),
                "{q}"
            );
        }
    }

    #[test]
    fn line_breaks_in_a_path_become_one_space() {
        assert_eq!(normalize_path("a  \n\t b"), "a b");
        assert_eq!(normalize_path("a\n\nb"), "a  b");
        // Buffer form: a CR here is content (a unix file's), kept as Org
        // keeps it; a DOS or mac file's line endings arrive as `\n`.
        assert_eq!(normalize_path("a\r\nb"), "a\r b");
        assert_eq!(normalize_path("a \rb"), "a \rb");
    }
}
