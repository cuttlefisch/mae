//! Search text: the rendered tree read as plain text. Rendered regions
//! only, link descriptions and never targets, entities decoded, footnote
//! definitions included and their markers not.

use super::build::{page, Builder, Mode, Table};
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

/// The search text of `doc`.
pub fn text(doc: &Document, sem: &Semantics) -> String {
    let opts = RenderOptions::default();
    text_of(&page(doc, sem, &opts, &text_resolver(), Mode::Text, None).body)
}

/// The search text of KB node `id`: its title, then its body (the text of
/// its `body_span`, without the nodes nested in it). `None` if `doc` has no
/// such node.
pub fn node_text(doc: &Document, sem: &Semantics, id: &str) -> Option<String> {
    let node = sem.nodes.iter().find(|n| n.id == id)?;
    let opts = RenderOptions::default();
    let resolver = text_resolver();
    let body = page(doc, sem, &opts, &resolver, Mode::Text, Some(node.body_span)).body;
    // The title as rendered (link descriptions, no snippet), not the
    // semantics' plain title, which spells a link without one as its target.
    let mut b = Builder::new(doc, sem, &opts, &resolver, Mode::Text, Table::default());
    let title = text_of(&b.node_title(&doc.syntax(), node));
    let title = crate::semantics::plain::collapse(&title);
    let body = text_of(&body);
    Some(match (title.is_empty(), body.is_empty()) {
        (_, true) => title,
        (true, false) => body,
        (false, false) => format!("{title}\n{body}"),
    })
}
