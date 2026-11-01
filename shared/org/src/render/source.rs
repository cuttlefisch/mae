//! Org source: the normal form of a document, and the form a KB node is
//! edited in (ADR-092 D3). The lossless print is `doc.syntax().to_string()`.
//!
//! Normalisation changes only what Emacs itself does not distinguish, so a
//! normalised text has the semantics of the original (pinned by
//! `tests/render_source.rs`):
//! - **Line endings become `\n`.** Emacs decodes a DOS or mac file to `\n`
//!   and keeps a CR that is not a line ending as content; so does this. One
//!   exception: a DOS file in which every line ending follows a content CR
//!   (`a\r\r\n`) has no LF spelling that reads back the same (`a\r\n` is
//!   read as DOS again, losing the CR), so it keeps its CRLF endings.
//! - **A leading BOM is dropped** (Emacs strips it on decode), unless a
//!   second follows it: that one is content, and dropping the first would
//!   make it trivia.
//! - **A final line ending is added** to a non-empty text without one.
//! - Nothing else. Property keys keep their spelling (Org reads them
//!   case-insensitively, and rewriting `:id:` to `:ID:` would be a diff on
//!   every save of a file written in lower case); trailing blanks, blank
//!   lines and link text are kept: ADR-092 asks for identity on the link
//!   grammar, never a normaliser.
//!
//! A node's edit form ([`node_source`]) is its `body_span` text with line
//! endings as `\n`; [`with_line_ending`] is its exact inverse, for writing
//! an edited body back into its file without changing the file's
//! convention (one lone LF in a DOS file makes Emacs read the WHOLE file as
//! unix, turning every CR into content).

use crate::{Document, LineEnding, SemanticNode};

/// `text` in its document's convention, with line endings as `\n`.
fn to_lf(text: &str, eol: LineEnding) -> String {
    match eol {
        LineEnding::Lf => text.to_string(),
        LineEnding::CrLf => text.replace("\r\n", "\n"),
        LineEnding::Cr => text.replace('\r', "\n"),
    }
}

/// The normal form of an org document (see the module doc). Idempotent.
pub fn normalise(text: &str) -> String {
    // One BOM is trivia; a second is content, and would become trivia if
    // the first were dropped, so then both stay.
    let text = match text.strip_prefix('\u{feff}') {
        Some(rest) if !rest.starts_with('\u{feff}') => rest,
        _ => text,
    };
    let eol = LineEnding::detect(text);
    let end = match eol {
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
    };
    let mut text = text.to_string();
    if !text.is_empty() && !text.ends_with(end) {
        text.push_str(end);
    }
    let lf = to_lf(&text, eol);
    if LineEnding::detect(&lf) == LineEnding::Lf {
        lf
    } else {
        text
    }
}

/// The text a KB node is edited in: its body (`body_span`), line endings
/// as `\n`. Nothing else changes.
pub fn node_source(doc: &Document, node: &SemanticNode) -> String {
    let text = doc.syntax().to_string();
    let body = text.get(node.body_span.range()).unwrap_or("");
    to_lf(body, doc.line_ending())
}

/// `text` (line endings `\n`, as [`node_source`] gives) in the convention
/// `eol`: the inverse of [`node_source`] on a body of a file with `eol`.
pub fn with_line_ending(text: &str, eol: LineEnding) -> String {
    match eol {
        LineEnding::Lf => text.to_string(),
        LineEnding::CrLf => text.replace('\n', "\r\n"),
        LineEnding::Cr => text.replace('\n', "\r"),
    }
}

/// Why [`replace_body`] refused an edit. Nothing is written when it does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum BodyEditError {
    /// `text` is not the text `doc` was parsed from.
    StaleDocument,
    /// The node's `body_span` is not a range of `text`.
    NoSuchBody,
    /// Line `line` (0-based) of the new body would be read as a headline,
    /// which would end the body and move every later node. Refused rather
    /// than escaped: write it comma-escaped (`,* text`) inside a block, as
    /// Org does, or as text that does not start with stars.
    HeadlineLine { line: usize },
    /// The edit would change how the whole file's line endings are read
    /// (e.g. remove the one bare LF of an LF file whose other LFs follow a
    /// CR, making Emacs read it as DOS). A guard: no node can be in such a
    /// file today, as every drawer line that makes one ends in a bare LF.
    LineEnding,
    /// The edit would change node `id`: another node, or this node's own
    /// title, properties, tags or place (a list item's body can end its
    /// list, for one).
    ChangesNode { id: String },
}

impl std::fmt::Display for BodyEditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BodyEditError::StaleDocument => write!(f, "the text is not the parsed document's"),
            BodyEditError::NoSuchBody => write!(f, "the node's body is not in the text"),
            BodyEditError::HeadlineLine { line } => {
                write!(f, "line {line} of the new body would be a headline")
            }
            BodyEditError::LineEnding => {
                write!(
                    f,
                    "the edit would change how the file's line endings are read"
                )
            }
            BodyEditError::ChangesNode { id } => write!(f, "the edit would change node {id:?}"),
        }
    }
}

impl std::error::Error for BodyEditError {}

/// `text` with KB node `node`'s body (its `body_span`) replaced by
/// `new_body`, the O4 write path's splice (ADR-092 D3).
///
/// `new_body` is in the edit form [`node_source`] gives (line endings
/// `\n`). It is written in the file's own convention ([`with_line_ending`])
/// and, when not empty, always ends with a line ending, so it can never run
/// into the next headline or node; a body after a headline line that ended
/// the file without one gets that line ending first. Every byte outside
/// the body is kept.
///
/// Refused ([`BodyEditError`]) when a line of the new body would be a
/// headline, when the file's line endings would be read differently, and
/// when the result, parsed again, would change any other node or this
/// node's own metadata (checked: the text is re-parsed with the document's
/// TODO keywords and otherwise default options, and every node compared,
/// spans moved by the edit). New nodes inside the new body are allowed.
pub fn replace_body(
    text: &str,
    doc: &Document,
    node: &SemanticNode,
    new_body: &str,
) -> Result<String, BodyEditError> {
    if doc.syntax().to_string() != text {
        return Err(BodyEditError::StaleDocument);
    }
    let (s, e) = (node.body_span.start as usize, node.body_span.end as usize);
    if s > e || e > text.len() || !text.is_char_boundary(s) || !text.is_char_boundary(e) {
        return Err(BodyEditError::NoSuchBody);
    }
    let eol = doc.line_ending();
    let end = match eol {
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
    };
    let mut body = with_line_ending(new_body, eol);
    if !body.is_empty() && !body.ends_with(end) {
        body.push_str(end);
    }
    for (i, line) in super::build::lines(&body, eol).into_iter().enumerate() {
        let stars = line.len() - line.trim_start_matches('*').len();
        let rest = &line[stars..];
        if stars > 0 && (rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r'])) {
            return Err(BodyEditError::HeadlineLine { line: i });
        }
    }
    let before = &text[..s];
    let prefix = if !body.is_empty() && !before.is_empty() && !before.ends_with(end) {
        end
    } else {
        ""
    };
    let out = format!("{before}{prefix}{body}{}", &text[e..]);
    if LineEnding::detect(&out) != eol {
        return Err(BodyEditError::LineEnding);
    }
    let start = s + prefix.len();
    check_nodes(text, &out, doc, node, (s, e), (start, start + body.len()))?;
    Ok(out)
}

/// Every node of `old` but `edited` is in `new` unchanged but for its spans,
/// moved by the edit of `old[s..e]` into `new[ns..ne]`; `edited` keeps its
/// metadata and its body is exactly the new range.
fn check_nodes(
    old: &str,
    new: &str,
    doc: &Document,
    edited: &SemanticNode,
    (s, e): (usize, usize),
    (ns, ne): (usize, usize),
) -> Result<(), BodyEditError> {
    let opts = crate::ParseOptions::default().with_todo_sequences(doc.todo_sequences().to_vec());
    let before = crate::semantics(&crate::parse(old, &opts));
    let after = crate::semantics(&crate::parse(new, &opts));
    let delta = ne as i64 - e as i64;
    let moved = |p: u32| -> Option<u32> {
        let p = p as usize;
        if p >= e {
            u32::try_from(p as i64 + delta).ok()
        } else if p <= s {
            Some(p as u32)
        } else {
            None
        }
    };
    let changed = |id: &str| BodyEditError::ChangesNode { id: id.to_string() };
    for n in &before.nodes {
        let Some(m) = after.nodes.iter().find(|m| m.id == n.id) else {
            return Err(changed(&n.id));
        };
        let mut want = n.clone();
        let span = (moved(n.span.start), moved(n.span.end));
        want.span = match span {
            (Some(a), Some(b)) => crate::Span { start: a, end: b },
            _ => return Err(changed(&n.id)),
        };
        want.body_span = if n.id == edited.id {
            // Within the new range: blank lines at its start are the
            // drawer's post-blank to Org, so they may fall outside it.
            let b = m.body_span;
            let blank = |r: std::ops::Range<usize>| {
                new.get(r)
                    .is_some_and(|t| t.chars().all(char::is_whitespace))
            };
            let inside = b.start as usize >= ns && b.end as usize <= ne && b.start <= b.end;
            if !(inside && blank(ns..b.start as usize) && blank(b.end as usize..ne)) {
                return Err(changed(&n.id));
            }
            b
        } else {
            match (moved(n.body_span.start), moved(n.body_span.end)) {
                (Some(a), Some(b)) => crate::Span { start: a, end: b },
                _ => return Err(changed(&n.id)),
            }
        };
        if *m != want {
            return Err(changed(&n.id));
        }
    }
    // A node that is new must lie in the new body.
    for m in &after.nodes {
        let new_node = !before.nodes.iter().any(|n| n.id == m.id);
        if new_node && !(m.span.start as usize >= ns && m.span.end as usize <= ne) {
            return Err(changed(&m.id));
        }
    }
    Ok(())
}
