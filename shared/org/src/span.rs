//! Byte spans owned by this crate, and each element's contents range.
//!
//! [`Span`] is plain data (two `u32` byte offsets) so diagnostics and
//! contents can cross a JSON or WASM boundary without dragging rowan's types
//! into a consumer's API. The tree itself is still rowan's (see `mae_org::rowan`).

use crate::element::{Body, Element};
use crate::kind::SyntaxKind::{self, *};
use crate::kind::SyntaxNode;
use crate::text::{ws_len, Source};

/// A half-open byte range `[start, end)` into the parsed text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub(crate) fn new(start: usize, end: usize) -> Span {
        let at = |x: usize| u32::try_from(x).unwrap_or(u32::MAX);
        Span {
            start: at(start.min(end)),
            end: at(end),
        }
    }

    /// The span of a node or token of the tree.
    pub fn of(node: &SyntaxNode) -> Span {
        let r = node.text_range();
        Span {
            start: r.start().into(),
            end: r.end().into(),
        }
    }

    pub fn len(&self) -> u32 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// As a `usize` range, for slicing the parsed text.
    pub fn range(&self) -> std::ops::Range<usize> {
        self.start as usize..self.end as usize
    }
}

/// Every element's contents range, keyed by (start, end, kind): a node's
/// span and kind identify it uniquely, since two elements of one kind never
/// share a span.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ContentsTable {
    rows: Vec<(u32, u32, u16, Span)>,
}

impl ContentsTable {
    pub fn push(&mut self, kind: SyntaxKind, begin: usize, end: usize, contents: Span) {
        let s = Span::new(begin, end);
        self.rows.push((s.start, s.end, kind as u16, contents));
    }

    pub fn finish(&mut self) {
        self.rows.sort_unstable_by_key(|r| (r.0, r.1, r.2));
    }

    pub fn get(&self, node: &SyntaxNode) -> Option<Span> {
        let s = Span::of(node);
        let key = (s.start, s.end, node.kind() as u16);
        self.rows
            .binary_search_by_key(&key, |r| (r.0, r.1, r.2))
            .ok()
            .map(|i| self.rows[i].3)
    }
}

/// `org-element`'s `:contents-begin`/`:contents-end` for `el`, or `None`
/// where Org gives none (lesser elements, empty greater ones).
pub(crate) fn contents_of(el: &Element, src: &Source) -> Option<(usize, usize)> {
    let nonempty = |a: usize, b: usize| (a < b).then_some((a, b));
    match el.kind {
        SECTION => Some((el.begin, el.end)),
        PARAGRAPH | PLAIN_LIST => Some((el.post_aff, el.body_end)),
        TABLE if el.body == Body::Children => Some((el.post_aff, el.body_end)),
        // Verse contents exist even when empty (they hold objects).
        VERSE_BLOCK => Some((el.head_end, el.body_end)),
        HEADLINE | ITEM | FOOTNOTE_DEFINITION if el.body == Body::Children => {
            nonempty(el.head_end, el.body_end)
        }
        CENTER_BLOCK | QUOTE_BLOCK | SPECIAL_BLOCK | DYNAMIC_BLOCK | DRAWER | PROPERTY_DRAWER => {
            nonempty(el.head_end, el.body_end)
        }
        TABLE_ROW => table_row_contents(el, src),
        _ => None,
    }
}

/// A standard row's contents run from after its first `|` to the end of the
/// line without trailing blanks; a rule row (`|-`) has none.
fn table_row_contents(el: &Element, src: &Source) -> Option<(usize, usize)> {
    let line = src.lines[src.line_of(el.begin)];
    let s = &src.text[line.start..line.end];
    let r = &s[ws_len(s)..];
    if r.starts_with("|-") {
        return None;
    }
    let bar = s.find('|')?;
    let end = line.start + crate::text::trim_end_blank(s).len();
    Some((line.start + bar + 1, end.max(line.start + bar + 1)))
}
