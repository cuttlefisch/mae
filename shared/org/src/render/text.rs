//! Search text: the rendered tree read as plain text. Rendered regions
//! only, link descriptions and never targets, entities decoded, footnote
//! definitions included and their markers not.

use super::build::{node_text_trees, page_with, Mode};
use super::tree::{Node, Tag};
use super::{RenderOptions, Resolver};
use crate::{Document, Semantics};

/// The plain text of `nodes`: block elements on their own lines, table
/// cells separated by a tab, images nothing.
pub fn text_of(nodes: &[Node]) -> String {
    let mut out = String::new();
    for n in nodes {
        walk(n, &mut out);
    }
    out.trim_matches('\n').to_string()
}

fn newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn walk(n: &Node, out: &mut String) {
    let e = match n {
        Node::Text(t) => return out.push_str(t),
        Node::Element(e) => e,
    };
    match e.tag {
        // The builder follows every `<br>` with its line ending as text.
        Tag::Br => return,
        Tag::Img => return,
        Tag::Td | Tag::Th if !out.ends_with('\n') && !out.is_empty() => out.push('\t'),
        _ => {}
    }
    let block = e.tag.is_block();
    if block {
        newline(out);
    }
    for c in &e.children {
        walk(c, out);
    }
    if block {
        newline(out);
    }
}

fn text_resolver() -> Resolver {
    Resolver {
        placeholder: String::new(),
        ..Resolver::default()
    }
}

/// The search text of `doc`, its macros expanded as [`super::render`]
/// expands them (empty when they cannot be).
pub fn text(doc: &Document, sem: &Semantics) -> String {
    let p = super::prepared::prepare(doc, sem);
    if p.failed.is_some() {
        return String::new();
    }
    let opts = RenderOptions::default();
    let r = text_resolver();
    text_of(&page_with(&p.doc, p.cx(), &opts, &r, Mode::Text, None).body)
}

/// The search text of KB node `id`: its title, then its body (the text of
/// its `body_span`, without the nodes nested in it), then the text of every
/// footnote the title or body references, wherever the note is defined.
/// An element that crosses the edge of `body_span` belongs to the node it
/// starts in: it is in this text whole if it starts inside, else not at
/// all. `None` if `doc` has no such node; empty when its macros cannot be
/// expanded, or their expansion removed it.
pub fn node_text(doc: &Document, sem: &Semantics, id: &str) -> Option<String> {
    sem.nodes.iter().find(|n| n.id == id)?;
    let p = super::prepared::prepare(doc, sem);
    if p.failed.is_some() {
        return Some(String::new());
    }
    let Some(node) = p.sem.nodes.iter().find(|n| n.id == id) else {
        return Some(String::new());
    };
    // The title as rendered (link descriptions, no snippet), not the
    // semantics' plain title, which spells a link without one as its target.
    let (title, body) = node_text_trees(&p, node, &text_resolver());
    let title = text_of(&title);
    let title = crate::semantics::plain::collapse(&title);
    let body = text_of(&body);
    Some(match (title.is_empty(), body.is_empty()) {
        (_, true) => title,
        (true, false) => body,
        (false, false) => format!("{title}\n{body}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Span;

    /// `node_text`'s scope never cuts an element in this crate's own
    /// `body_span`s (they start and end on element boundaries), so the rule
    /// for one that does is pinned on the scope directly.
    fn scoped(doc: &str, start: u32, end: u32) -> String {
        let d = crate::parse(doc, &crate::ParseOptions::default());
        let sem = crate::semantics(&d);
        let opts = RenderOptions::default();
        let scope = Some(Span { start, end });
        let cx = super::super::build::DocCx::new(&d, &sem);
        text_of(&page_with(&d, cx, &opts, &text_resolver(), Mode::Text, scope).body)
    }

    #[test]
    fn an_element_across_the_scope_edge_belongs_to_the_node_it_starts_in() {
        let doc = "first para\nstill first\n\nsecond para\n\n- item one\n- item two\n";
        // Starts before the scope: not in it, even though it ends inside.
        assert_eq!(scoped(doc, 5, 38), "second para");
        // Starts inside: whole, even though it ends after the scope.
        assert_eq!(scoped(doc, 0, 5), "first para\nstill first");
        // A list is a container: the items inside the scope, each whole.
        let at = doc.find("- item two").unwrap() as u32;
        assert_eq!(scoped(doc, at - 3, doc.len() as u32), "item two");
    }
}
