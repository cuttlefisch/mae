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
