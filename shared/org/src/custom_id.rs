//! Stable heading anchors: minting a `CUSTOM_ID` for the headlines that lack
//! one, and finding the ones a copy has duplicated.
//!
//! A `CUSTOM_ID` gives a headline an export anchor without making it a KB
//! node (an `:ID:` does). These functions compute the edits; the caller
//! applies them as an ordinary author edit and supplies the randomness, so
//! everything here is pure. Every value written must be a lowercase UUID v4
//! ([`is_custom_id_uuid`]): no caller can put a newline, a `:END:` or any
//! other structure into a drawer through them.
//!
//! Placement is Org's (`org-entry-put`), pinned against Emacs by
//! `tests/custom_id.rs`:
//! - a headline with no property drawer gets one directly after its
//!   headline line, or after its planning line when it has one;
//! - a headline whose drawer lacks the key gets the line before `:END:`;
//! - a headline whose drawer has the key with an empty value gets the value
//!   filled in.
//!
//! A headline is skipped when it has a non-empty `CUSTOM_ID` already, or a
//! non-empty `ID` (it is a KB node, and a page anchors a node by its id).
//! Only what the parser reads as a headline counts, as in Org: a `* ` at
//! column 0 starts a headline even inside a block (an indented one never
//! does), and the file-level drawer belongs to no headline.
//!
//! Offsets are UTF-8 byte offsets into the text, like every span here.

use std::collections::BTreeMap;
use std::fmt;

use crate::kind::SyntaxKind::*;
use crate::kind::{SyntaxNode, SyntaxToken};
use crate::views::{canonical_key, Headline};
use crate::{parse, LineEnding, ParseOptions, Span};

/// One replacement of `text[start..end]` by `insert`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub start: u32,
    pub end: u32,
    pub insert: String,
}

/// One headline's `CUSTOM_ID`: where its value is and where its headline
/// line is (the line, without its line ending).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CustomIdOccurrence {
    pub value: Span,
    pub heading: Span,
}

/// Why no edit was made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CustomIdError {
    /// A value to write is not a lowercase UUID v4.
    NotAUuid(String),
    /// The edits need more values than were supplied.
    TooFewUuids { needed: usize, got: usize },
    /// No headline's `CUSTOM_ID` value starts at this offset.
    NotACustomIdValue(u32),
}

impl fmt::Display for CustomIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CustomIdError::NotAUuid(v) => {
                write!(f, "not a lowercase UUID v4: {}", crate::json_string(v))
            }
            CustomIdError::TooFewUuids { needed, got } => {
                write!(f, "{needed} uuids needed, {got} supplied")
            }
            CustomIdError::NotACustomIdValue(at) => {
                write!(f, "no headline's CUSTOM_ID value starts at byte {at}")
            }
        }
    }
}

impl std::error::Error for CustomIdError {}

/// Whether `s` is a lowercase UUID v4: `xxxxxxxx-xxxx-4xxx-Nxxx-xxxxxxxxxxxx`,
/// lowercase hex, version 4, variant `N` one of `8`, `9`, `a`, `b`.
pub fn is_custom_id_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    let hex = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
    b.len() == 36
        && b.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => hex(c),
        })
        && b[14] == b'4'
        && matches!(b[19], b'8' | b'9' | b'a' | b'b')
}

/// The headlines in scope that need a `CUSTOM_ID`, in document order, each
/// as its headline line. With `ranges`, a headline is in scope when its
/// headline line starts inside one (`start <= at < end`).
pub fn headings_missing_custom_id(
    text: &str,
    options: &ParseOptions,
    ranges: Option<&[Span]>,
) -> Vec<Span> {
    missing(text, options, ranges)
        .iter()
        .map(heading_line)
        .collect()
}

/// The edits giving every headline in scope that needs one a `CUSTOM_ID`,
/// sorted by `start`, descending (apply them in order). `mint` is called
/// once per headline, in document order; nothing is returned unless every
/// value it gives is a lowercase UUID v4.
pub fn custom_id_edits(
    text: &str,
    options: &ParseOptions,
    ranges: Option<&[Span]>,
    mint: &mut dyn FnMut() -> String,
) -> Result<Vec<TextEdit>, CustomIdError> {
    let doc = parse(text, options);
    let eol = eol(doc.line_ending());
    let mut edits = Vec::new();
    for h in missing_in(&doc.syntax(), ranges) {
        let uuid = mint();
        if !is_custom_id_uuid(&uuid) {
            return Err(CustomIdError::NotAUuid(uuid));
        }
        edits.push(edit_for(&h, &uuid, eol, text.len()));
    }
    edits.reverse();
    Ok(edits)
}

/// [`custom_id_edits`] with the values given: an error, and no edits, when
/// there are fewer than needed. Extra values are not used.
pub fn custom_id_edits_with(
    text: &str,
    options: &ParseOptions,
    ranges: Option<&[Span]>,
    uuids: &[String],
) -> Result<Vec<TextEdit>, CustomIdError> {
    let needed = missing(text, options, ranges).len();
    if uuids.len() < needed {
        return Err(CustomIdError::TooFewUuids {
            needed,
            got: uuids.len(),
        });
    }
    let mut next = uuids.iter().cloned();
    custom_id_edits(text, options, ranges, &mut || {
        next.next().unwrap_or_default()
    })
}

/// The `CUSTOM_ID` values more than one headline has, as groups of their
/// occurrences in document order, the groups ordered by first occurrence.
/// It never chooses which to keep: only the caller knows which is the copy.
pub fn duplicate_custom_ids(text: &str, options: &ParseOptions) -> Vec<Vec<CustomIdOccurrence>> {
    let doc = parse(text, options);
    let mut groups: Vec<Vec<CustomIdOccurrence>> = Vec::new();
    let mut index: BTreeMap<String, usize> = BTreeMap::new();
    for h in doc.headlines() {
        let Some(value) = custom_id_value(&h) else {
            continue;
        };
        let occurrence = CustomIdOccurrence {
            value: token_span(&value),
            heading: heading_line(&h),
        };
        let i = *index.entry(value.text().to_string()).or_insert_with(|| {
            groups.push(Vec::new());
            groups.len() - 1
        });
        groups[i].push(occurrence);
    }
    groups.retain(|g| g.len() > 1);
    groups
}

/// The edit replacing the `CUSTOM_ID` value that starts at byte `at` (as
/// [`duplicate_custom_ids`] reports it) with `uuid`.
pub fn replace_custom_id(
    text: &str,
    options: &ParseOptions,
    at: u32,
    uuid: &str,
) -> Result<TextEdit, CustomIdError> {
    if !is_custom_id_uuid(uuid) {
        return Err(CustomIdError::NotAUuid(uuid.to_string()));
    }
    let doc = parse(text, options);
    let value = doc
        .headlines()
        .filter_map(|h| custom_id_value(&h))
        .find(|v| token_span(v).start == at)
        .ok_or(CustomIdError::NotACustomIdValue(at))?;
    let span = token_span(&value);
    Ok(TextEdit {
        start: span.start,
        end: span.end,
        insert: uuid.to_string(),
    })
}

fn missing(text: &str, options: &ParseOptions, ranges: Option<&[Span]>) -> Vec<Headline> {
    missing_in(&parse(text, options).syntax(), ranges)
}

fn missing_in(root: &SyntaxNode, ranges: Option<&[Span]>) -> Vec<Headline> {
    crate::views::headline_nodes(root)
        .into_iter()
        .filter_map(Headline::cast)
        .filter(|h| in_scope(h, ranges) && needs_custom_id(h))
        .collect()
}

fn in_scope(h: &Headline, ranges: Option<&[Span]>) -> bool {
    let at = Span::of(h.syntax()).start;
    ranges.is_none_or(|rs| rs.iter().any(|r| r.start <= at && at < r.end))
}

fn needs_custom_id(h: &Headline) -> bool {
    let Some(d) = h.property_drawer() else {
        return true;
    };
    let set = |k: &str| d.get(k).is_some_and(|v| !v.is_empty());
    !set("CUSTOM_ID") && !set("ID")
}

/// The value token of the `CUSTOM_ID` Org reads (the last), if non-empty.
fn custom_id_value(h: &Headline) -> Option<SyntaxToken> {
    let p = h
        .property_drawer()?
        .properties()
        .into_iter()
        .rev()
        .find(|p| p.key == "CUSTOM_ID")?;
    token(&p.node, VALUE).filter(|t| !t.text().is_empty())
}

/// A headline's headline line, without its line ending.
fn heading_line(h: &Headline) -> Span {
    let start = Span::of(h.syntax()).start;
    let end = token(h.syntax(), NEWLINE).map_or(Span::of(h.syntax()).end, |t| token_span(&t).start);
    Span { start, end }
}

fn edit_for(h: &Headline, uuid: &str, eol: &str, len: usize) -> TextEdit {
    match h.property_drawer() {
        Some(d) => drawer_edit(d.syntax(), uuid, eol),
        None => new_drawer(h, uuid, eol, len),
    }
}

/// The key into an existing drawer: an empty `CUSTOM_ID` (the last) filled
/// in, else a line before `:END:`, indented like it.
fn drawer_edit(drawer: &SyntaxNode, uuid: &str, eol: &str) -> TextEdit {
    let empty = drawer
        .children()
        .filter(|n| n.kind() == NODE_PROPERTY)
        .filter(|n| token(n, NAME).is_some_and(|t| canonical_key(t.text()) == "CUSTOM_ID"))
        .last();
    if let Some(p) = empty {
        return fill_value(&p, uuid);
    }
    let end = drawer
        .children()
        .find(|n| n.kind() == BLOCK_END)
        .expect("a property drawer has an end line");
    let indent = leading_whitespace(&end);
    let at = Span::of(&end).start;
    TextEdit {
        start: at,
        end: at,
        insert: format!("{indent}:CUSTOM_ID: {uuid}{eol}"),
    }
}

/// The empty value of property line `p` set to `uuid`: everything between
/// the key's closing colon and the line ending becomes ` uuid`.
fn fill_value(p: &SyntaxNode, uuid: &str) -> TextEdit {
    let colon = p
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| t.kind() == COLON)
        .nth(1)
        .expect("a node property has two colons");
    let end = token(p, NEWLINE).map_or(Span::of(p).end, |t| token_span(&t).start);
    TextEdit {
        start: token_span(&colon).end,
        end,
        insert: format!(" {uuid}"),
    }
}

/// A new drawer after the headline line, or after the planning line.
fn new_drawer(h: &Headline, uuid: &str, eol: &str, len: usize) -> TextEdit {
    let planning = h
        .section()
        .and_then(|s| s.children().next())
        .filter(|n| n.kind() == PLANNING);
    // The end of the planning LINE: the node also holds the blank lines after
    // it, and a drawer after a blank line is not the property drawer.
    let at = match &planning {
        Some(p) => token(p, NEWLINE).map_or(Span::of(p).end, |t| token_span(&t).end),
        None => token(h.syntax(), NEWLINE).map_or(Span::of(h.syntax()).end, |t| token_span(&t).end),
    };
    // At the end of a file with no final line ending, the line before the
    // drawer needs one.
    let ends_line = at as usize == len && !ends_with_eol(h, planning.as_ref());
    let lead = if ends_line { eol } else { "" };
    TextEdit {
        start: at,
        end: at,
        insert: format!("{lead}:PROPERTIES:{eol}:CUSTOM_ID: {uuid}{eol}:END:{eol}"),
    }
}

fn ends_with_eol(h: &Headline, planning: Option<&SyntaxNode>) -> bool {
    match planning {
        Some(p) => token(p, NEWLINE).is_some(),
        None => token(h.syntax(), NEWLINE).is_some(),
    }
}

fn leading_whitespace(line: &SyntaxNode) -> String {
    line.first_token()
        .filter(|t| t.kind() == WHITESPACE)
        .map_or_else(String::new, |t| t.text().to_string())
}

fn eol(e: LineEnding) -> &'static str {
    match e {
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
    }
}

fn token(node: &SyntaxNode, kind: crate::SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == kind)
}

fn token_span(t: &SyntaxToken) -> Span {
    let r = t.text_range();
    Span {
        start: r.start().into(),
        end: r.end().into(),
    }
}
