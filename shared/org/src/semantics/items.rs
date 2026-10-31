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

/// The properties of `drawer` if it is an item's property drawer, as written
/// (keys not yet canonical), in order.
pub(crate) fn item_property_drawer(drawer: &SyntaxNode) -> Option<Vec<(String, String)>> {
    if drawer.kind() != DRAWER {
        return None;
    }
    let item = drawer.parent().filter(|p| p.kind() == ITEM)?;
    let elements: Vec<SyntaxNode> = item
        .children()
        .filter(|c| c.kind() != ITEM_TAG_CONTENT)
        .collect();
    let at = elements.iter().position(|e| e == drawer)?;
    let placed = at == 0 || (at == 1 && elements[0].kind() == PARAGRAPH);
    if !placed || !drawer_name(drawer).eq_ignore_ascii_case("PROPERTIES") {
        return None;
    }
    let begin = drawer.children().find(|c| c.kind() == BLOCK_BEGIN)?;
    let end = drawer.children().find(|c| c.kind() == BLOCK_END)?;
    let text = drawer.to_string();
    let base = usize::from(drawer.text_range().start());
    let from = usize::from(begin.text_range().end()) - base;
    let to = usize::from(end.text_range().start()) - base;
    text[from..to]
        .split(['\n', '\r'])
        .filter(|l| !l.trim_matches([' ', '\t']).is_empty())
        .map(property_line)
        .collect()
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
/// rest, trimmed.
fn property_line(line: &str) -> Option<(String, String)> {
    let blank = [' ', '\t'];
    let rest = line.trim_start_matches(blank).strip_prefix(':')?;
    let n = rest.find(blank).unwrap_or(rest.len());
    let key = rest[..n].strip_suffix(':').filter(|k| !k.is_empty())?;
    Some((key.to_string(), rest[n..].trim_matches(blank).to_string()))
}

/// The item's own text before its property drawer: its tag and first
/// paragraph, as plain text (`tag :: text` for a descriptive item).
pub(crate) fn item_title(item: &SyntaxNode, drawer: &SyntaxNode) -> Option<String> {
    let tag = item
        .children()
        .find(|c| c.kind() == ITEM_TAG_CONTENT)
        .map(|t| super::plain::plain_text(&t))
        .filter(|t| !t.is_empty());
    let first = item
        .children()
        .take_while(|c| c != drawer)
        .find(|c| c.kind() == PARAGRAPH)
        .map(|p| super::plain::plain_text(&p))
        .filter(|t| !t.is_empty());
    match (tag, first) {
        (Some(t), Some(f)) => Some(format!("{t} :: {f}")),
        (t, f) => t.or(f),
    }
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
