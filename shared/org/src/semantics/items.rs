//! MAE's list-item nodes (a documented extension to Org).
//!
//! org-roam users give a list item its own `:ID:` with a drawer right under
//! the item's first line. Org reads that as a plain drawer named PROPERTIES
//! inside the item. MAE reads it as the item's property drawer when:
//! - it is a direct child of the item, and either its first element or its
//!   second after one paragraph (the item's first line, its title);
//! - its name is `PROPERTIES` (any case);
//! - every non-blank line in it is a property line (`:KEY: value`).
//!
//! A drawer that fails any of these is an ordinary drawer, so unrendered.

use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::span::Span;

/// One `:KEY: value` line of an item's property drawer.
pub(crate) struct ItemProperty {
    /// The key as written (not yet canonical).
    pub key: String,
    /// The value, trimmed of blanks.
    pub value: String,
    /// The value's start in the document.
    pub value_start: usize,
}

/// Whether `drawer` is placed where an item's property drawer may be: a
/// direct child of an item, its first element or its second after one
/// paragraph. O(1): only the item's first two elements are looked at.
fn placed(drawer: &SyntaxNode) -> bool {
    let Some(item) = drawer.parent().filter(|p| p.kind() == ITEM) else {
        return false;
    };
    let mut elements = item.children().filter(|c| c.kind() != ITEM_TAG_CONTENT);
    match elements.next() {
        Some(first) if first == *drawer => true,
        Some(first) if first.kind() == PARAGRAPH => elements.next().as_ref() == Some(drawer),
        _ => false,
    }
}

/// Whether `drawer` is an item's property drawer (see the module doc).
pub(crate) fn is_item_property_drawer(drawer: &SyntaxNode) -> bool {
    item_property_drawer(drawer).is_some()
}

/// The properties of `drawer` if it is an item's property drawer, in order.
/// The cost is the drawer's own size: a drawer that is not in place, or not
/// named PROPERTIES, is rejected before its lines are read.
pub(crate) fn item_property_drawer(drawer: &SyntaxNode) -> Option<Vec<ItemProperty>> {
    if drawer.kind() != DRAWER || !placed(drawer) {
        return None;
    }
    if !drawer_name(drawer).eq_ignore_ascii_case("PROPERTIES") {
        return None;
    }
    let begin = drawer.children().find(|c| c.kind() == BLOCK_BEGIN)?;
    let end = drawer.children().find(|c| c.kind() == BLOCK_END)?;
    let text = drawer.to_string();
    let base = usize::from(drawer.text_range().start());
    let from = usize::from(begin.text_range().end()) - base;
    let to = usize::from(end.text_range().start()) - base;
    let mut out = Vec::new();
    let mut pos = base + from;
    for seg in text[from..to].split_inclusive(['\n', '\r']) {
        let line = seg.trim_end_matches(['\n', '\r']);
        let at = pos;
        pos += seg.len();
        if line.trim_matches([' ', '\t']).is_empty() {
            continue;
        }
        out.push(property_line(line, at)?);
    }
    Some(out)
}

/// An item's property drawer and its properties, if it has one. Only the
/// first two elements can be the drawer (see the module doc).
pub(crate) fn item_drawer(item: &SyntaxNode) -> Option<(SyntaxNode, Vec<ItemProperty>)> {
    item.children()
        .filter(|c| c.kind() != ITEM_TAG_CONTENT)
        .take(2)
        .find_map(|c| item_property_drawer(&c).map(|p| (c, p)))
}

/// The name of a drawer as written (`LOGBOOK` in `:LOGBOOK:`).
pub(crate) fn drawer_name(drawer: &SyntaxNode) -> String {
    drawer
        .children()
        .find(|c| c.kind() == BLOCK_BEGIN)
        .and_then(|b| {
            b.children_with_tokens()
                .filter_map(|e| e.into_token())
                .find(|t| t.kind() == NAME)
        })
        .map_or_else(String::new, |t| t.text().to_string())
}

/// `:KEY: value` as the tree's property lines read it: the key is the
/// non-blank run after the first colon, ending in a colon; the value is the
/// rest, trimmed of blanks. `at` is the line's start in the document.
fn property_line(line: &str, at: usize) -> Option<ItemProperty> {
    let blank = [' ', '\t'];
    let indent = line.len() - line.trim_start_matches(blank).len();
    let rest = line[indent..].strip_prefix(':')?;
    let n = rest.find(blank).unwrap_or(rest.len());
    let key = rest[..n].strip_suffix(':').filter(|k| !k.is_empty())?;
    let after = &rest[n..];
    let lead = after.len() - after.trim_start_matches(blank).len();
    Some(ItemProperty {
        key: key.to_string(),
        value: after.trim_matches(blank).to_string(),
        value_start: at + indent + 1 + n + lead,
    })
}

/// The item's own text before its property drawer: its tag and first
/// paragraph, as plain text (`tag :: text` for a descriptive item).
pub(crate) fn item_title(item: &SyntaxNode, drawer: &SyntaxNode) -> Option<String> {
    let (tag, first) = item_title_parts(item, drawer);
    let tag = tag
        .map(|t| super::plain::plain_text(&t))
        .filter(|t| !t.is_empty());
    let first = first
        .map(|p| super::plain::plain_text(&p))
        .filter(|t| !t.is_empty());
    match (tag, first) {
        (Some(t), Some(f)) => Some(format!("{t} :: {f}")),
        (t, f) => t.or(f),
    }
}

/// The nodes an item's title is read from: its tag, and its first
/// paragraph before the property drawer. A renderer draws the title from
/// these same nodes.
pub(crate) fn item_title_parts(
    item: &SyntaxNode,
    drawer: &SyntaxNode,
) -> (Option<SyntaxNode>, Option<SyntaxNode>) {
    let tag = item.children().find(|c| c.kind() == ITEM_TAG_CONTENT);
    let first = item
        .children()
        .take_while(|c| c != drawer)
        .find(|c| c.kind() == PARAGRAPH);
    (tag, first)
}

/// An item node's level: its headline's level (0 in the zeroth section) plus
/// the number of items it is nested in, itself included.
pub(crate) fn item_level(item: &SyntaxNode) -> usize {
    let mut items = 0;
    for a in item.ancestors() {
        match a.kind() {
            ITEM => items += 1,
            HEADLINE => {
                return items + crate::views::Headline::cast(a).map_or(0, |h| h.level());
            }
            _ => {}
        }
    }
    items
}

/// An item node's body: from after its property drawer to the item's end.
pub(crate) fn item_body(item: &SyntaxNode, drawer: &SyntaxNode) -> Span {
    Span::new(
        usize::from(drawer.text_range().end()),
        usize::from(item.text_range().end()),
    )
}
