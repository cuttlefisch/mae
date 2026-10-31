//! The typed view of a link object: what `org-element-link-parser` reports
//! as `:type`, `:path`, `:raw-link`, `:format`, `:search-option`,
//! `:application`, and the description (its contents).
//!
//! Read from the tree only. A bracket link's type depends on the registered
//! link types (`[[foo:x]]` is fuzzy unless `foo` is one), which the tree does
//! not record, so [`Link::info`] takes them: pass the
//! [`crate::ParseOptions::link_types`] the document was parsed with. It also
//! depends on the document's `#+LINK:` abbreviations, which
//! [`Link::info`] collects from the tree each call; to resolve many links,
//! collect them once with [`LinkAbbrevs::of`] and use [`Link::info_with`].
//!
//! Not modelled: `org-link-abbrev-alist` (global, nil in `emacs -Q`),
//! abbreviations whose replacement calls a function (`%(fn)`: Org refuses
//! them unless the function is marked safe, and deletes them from the
//! buffer's list, so later links may see an older duplicate; here they never
//! expand), `#+LINK:` lines in a `#+SETUPFILE:`, and
//! `org-link-translation-function` (nil in `emacs -Q`).

use crate::kind::SyntaxKind::*;
use crate::kind::{SyntaxElement, SyntaxNode};
pub use abbrev::LinkAbbrevs;

mod abbrev;

/// How a link is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LinkFormat {
    /// `https://x.org`, or radio-link text.
    Plain,
    /// `[[path]]` or `[[path][description]]`.
    Bracket,
    /// `<https://x.org>`.
    Angle,
}

impl LinkFormat {
    /// Org's name for the format (`:format`).
    pub fn as_str(self) -> &'static str {
        match self {
            LinkFormat::Plain => "plain",
            LinkFormat::Bracket => "bracket",
            LinkFormat::Angle => "angle",
        }
    }
}

/// A link's properties as `org-element-link-parser` computes them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LinkInfo {
    /// `:type`: a registered type as written (`https`, `HTTPS`, `file`), or
    /// `radio`, `fuzzy`, `custom-id`, `coderef`.
    pub link_type: String,
    /// `:path`, after file-link processing (no `::search`, `///` folded).
    pub path: String,
    /// `:raw-link`: the link as written, unescaped, newlines as one space
    /// (bracket); the whole text (plain); `type:path` as written (angle).
    pub raw_link: String,
    pub format: LinkFormat,
    /// `:search-option`, after `::` in a file link.
    pub search_option: Option<String>,
    /// `:application`, the `APP` of `file+APP:`.
    pub application: Option<String>,
    /// `:type-explicit-p`: the type was written (`https:`, `file:`).
    pub type_explicit: bool,
}

/// A link object (`LINK` node).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Link(SyntaxNode);

impl Link {
    /// The view of `node`, if it is a link.
    pub fn cast(node: SyntaxNode) -> Option<Link> {
        (node.kind() == LINK).then_some(Link(node))
    }

    /// The underlying node.
    pub fn syntax(&self) -> &SyntaxNode {
        &self.0
    }

    /// A radio link: text matching a `<<<radio target>>>`.
    pub fn is_radio(&self) -> bool {
        !self.tokens().any(|t| t.kind() == LINK_PATH)
    }

    pub fn format(&self) -> LinkFormat {
        match self
            .0
            .first_token()
            .map(|t| (t.kind(), t.text().to_string()))
        {
            Some((MARKUP, t)) if t == "[[" => LinkFormat::Bracket,
            Some((MARKUP, t)) if t == "<" => LinkFormat::Angle,
            _ => LinkFormat::Plain,
        }
    }

    fn tokens(&self) -> impl Iterator<Item = crate::kind::SyntaxToken> {
        self.0.children_with_tokens().filter_map(|e| e.into_token())
    }

    /// The text from the first `LINK_PATH` token to the last, line endings
    /// read as `\n` (as Emacs's buffer holds them).
    pub(crate) fn path_text(&self) -> String {
        let mut out = String::new();
        let mut pending = String::new();
        let mut started = false;
        for t in self.tokens() {
            match t.kind() {
                LINK_PATH => {
                    out.push_str(&pending);
                    pending.clear();
                    out.push_str(t.text());
                    started = true;
                }
                NEWLINE if started => pending.push('\n'),
                _ if started => pending.push_str(t.text()),
                _ => {}
            }
        }
        out
    }

    /// The description: the link's contents (a bracket link's text between
    /// `][` and `]]`, or a radio link's text), as the tokens and object
    /// nodes that make it up. `None` for a link without one.
    pub fn description(&self) -> Option<Vec<SyntaxElement>> {
        let all: Vec<SyntaxElement> = self.0.children_with_tokens().collect();
        let is_markup = |e: &SyntaxElement, s: &str| {
            e.as_token()
                .is_some_and(|t| t.kind() == MARKUP && t.text() == s)
        };
        let (from, to) = if self.is_radio() {
            let from = usize::from(all.first().is_some_and(|e| e.kind() == MARKUP));
            let to = all.len() - usize::from(all.last().is_some_and(|e| e.kind() == WHITESPACE));
            (from, to)
        } else if self.format() == LinkFormat::Bracket {
            let from = all.iter().position(|e| is_markup(e, "]["))? + 1;
            let to = all.iter().rposition(|e| is_markup(e, "]]"))?;
            (from, to)
        } else {
            return None;
        };
        (from < to).then(|| all[from..to].to_vec())
    }

    /// The description as text (line endings as written), if any.
    pub fn description_text(&self) -> Option<String> {
        self.description()
            .map(|els| els.iter().map(|e| e.to_string()).collect())
    }

    /// Org's view of the link, given the registered link types (and the
    /// `#+LINK:` abbreviations of the document the link is in).
    pub fn info<S: AsRef<str>>(&self, link_types: &[S]) -> LinkInfo {
        let root = self.0.ancestors().last().unwrap_or_else(|| self.0.clone());
        self.info_with(link_types, &LinkAbbrevs::of(&root))
    }

    /// [`Link::info`] with the document's abbreviations already collected.
    pub fn info_with<S: AsRef<str>>(&self, link_types: &[S], abbrevs: &LinkAbbrevs) -> LinkInfo {
        let format = self.format();
        let mut info = if self.is_radio() {
            let text: String = self
                .description()
                .unwrap_or_default()
                .iter()
                .map(eol_as_newline)
                .collect();
            LinkInfo::untyped("radio", text, format)
        } else {
            let raw = self.path_text();
            match format {
                LinkFormat::Bracket => bracket_info(&raw, link_types, abbrevs),
                LinkFormat::Plain => typed(&raw, link_types, format, |p| p.to_string()),
                LinkFormat::Angle => typed(&raw, link_types, format, |p| strip_breaks(p, "")),
            }
        };
        file_processing(&mut info);
        info
    }
}

/// An element's text with its line endings read as `\n`.
fn eol_as_newline(e: &SyntaxElement) -> String {
    match e {
        rowan::NodeOrToken::Token(t) if t.kind() == NEWLINE => "\n".to_string(),
        rowan::NodeOrToken::Token(t) => t.text().to_string(),
        rowan::NodeOrToken::Node(n) => n
            .descendants_with_tokens()
            .filter_map(|d| d.into_token())
            .map(|t| {
                if t.kind() == NEWLINE {
                    "\n".to_string()
                } else {
                    t.text().to_string()
                }
            })
            .collect(),
    }
}

impl LinkInfo {
    fn untyped(link_type: &str, path: String, format: LinkFormat) -> LinkInfo {
        LinkInfo {
            link_type: link_type.to_string(),
            raw_link: path.clone(),
            path,
            format,
            search_option: None,
            application: None,
            type_explicit: false,
        }
    }
}

/// The registered type `raw` starts with (ignoring case, as
/// `case-fold-search` does), followed by `:`: its length.
fn type_prefix<S: AsRef<str>>(raw: &str, link_types: &[S]) -> Option<usize> {
    link_types
        .iter()
        .map(AsRef::as_ref)
        .filter(|t| {
            raw.len() > t.len()
                && raw.as_bytes()[..t.len()].eq_ignore_ascii_case(t.as_bytes())
                && raw.as_bytes()[t.len()] == b':'
        })
        .map(str::len)
        .max()
}

/// A plain or angle link: `TYPE:PATH`, the type as written.
fn typed<S: AsRef<str>>(
    raw: &str,
    link_types: &[S],
    format: LinkFormat,
    path_of: impl Fn(&str) -> String,
) -> LinkInfo {
    let n = type_prefix(raw, link_types)
        .or_else(|| raw.find(':'))
        .unwrap_or(0);
    LinkInfo {
        link_type: raw[..n].to_string(),
        path: path_of(raw.get(n + 1..).unwrap_or("")),
        raw_link: raw.to_string(),
        format,
        search_option: None,
        application: None,
        type_explicit: true,
    }
}

/// `[[PATH]]`: unescaped, newlines (with surrounding blanks) as one space,
/// then a file, a registered type, a coderef, a custom id or fuzzy.
fn bracket_info<S: AsRef<str>>(raw: &str, link_types: &[S], abbrevs: &LinkAbbrevs) -> LinkInfo {
    // One normaliser for link paths (principle #8): the link grammar's.
    let raw_link = abbrevs.expand(&crate::link_grammar::normalize_path(raw));
    let format = LinkFormat::Bracket;
    let absolute = raw_link.starts_with('/') || raw_link == "~" || raw_link.starts_with("~/");
    if absolute || raw_link.starts_with("./") || raw_link.starts_with("../") {
        return LinkInfo::untyped("file", raw_link, format);
    }
    if let Some(n) = type_prefix(&raw_link, link_types) {
        return LinkInfo {
            link_type: raw_link[..n].to_string(),
            path: raw_link[n + 1..].to_string(),
            raw_link,
            format,
            search_option: None,
            application: None,
            type_explicit: true,
        };
    }
    let mut info = if raw_link.len() >= 2 && raw_link.starts_with('(') && raw_link.ends_with(')') {
        LinkInfo::untyped(
            "coderef",
            raw_link[1..raw_link.len() - 1].to_string(),
            format,
        )
    } else if let Some(id) = raw_link.strip_prefix('#') {
        LinkInfo::untyped("custom-id", id.to_string(), format)
    } else {
        LinkInfo::untyped("fuzzy", raw_link.clone(), format)
    };
    info.raw_link = raw_link;
    info
}

/// `[ \t]*\n[ \t]*` replaced by `with`.
fn strip_breaks(s: &str, with: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(k) = rest.find('\n') {
        out.push_str(rest[..k].trim_end_matches([' ', '\t']));
        out.push_str(with);
        rest = rest[k + 1..].trim_start_matches([' ', '\t']);
    }
    out.push_str(rest);
    out
}

/// The `file`-type processing: `file+APP` gives the application, `::X` the
/// search option, and `\`///*\(.:\)?/` folds to `\1/`.
fn file_processing(info: &mut LinkInfo) {
    let t = info.link_type.to_ascii_lowercase();
    let app = if t == "file" {
        None
    } else if let Some(app) = t.strip_prefix("file+").filter(|a| !a.is_empty()) {
        // The application as written (`file+Sys` keeps its case).
        Some(info.link_type[5..5 + app.len()].to_string())
    } else {
        return;
    };
    info.application = app;
    info.link_type = "file".to_string();
    if let Some(k) = info.path.find("::") {
        info.search_option = Some(info.path[k + 2..].to_string());
        info.path.truncate(k);
    }
    info.path = fold_slashes(&info.path);
}

/// `\`///*\(.:\)?/` → `\1/`, with the regexp's backtracking order: two
/// slashes, as many more as can still leave a match, then an optional drive
/// (`c:`) and a slash. `//x` is left alone; `///x` becomes `/x`.
fn fold_slashes(path: &str) -> String {
    let b = path.as_bytes();
    let k = b.iter().take_while(|&&c| c == b'/').count();
    for j in (2..=k).rev() {
        // `(.:)` then `/`: one char (not a newline), a colon, a slash.
        if let Some(c) = path[j..].chars().next().filter(|&c| c != '\n') {
            let after = j + c.len_utf8();
            if b.get(after) == Some(&b':') && b.get(after + 1) == Some(&b'/') {
                return format!("{}/{}", &path[j..after + 1], &path[after + 2..]);
            }
        }
        if b.get(j) == Some(&b'/') {
            return format!("/{}", &path[j + 1..]);
        }
    }
    path.to_string()
}
