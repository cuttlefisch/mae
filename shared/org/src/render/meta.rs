//! KB node metadata: titles, aliases and properties, rendered inline-only
//! through the same builder as the body.

use super::build::{trim_text, Builder, Mode, Table};
use super::notes::node_at;
use super::tree::{push, text, Node};
use super::{NodeMeta, RenderOptions, Resolver};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::semantics::file_title_source;
use crate::semantics::items::{item_drawer, item_title_parts};
use crate::{Document, NodeOrigin, SemanticNode, Semantics};

pub(crate) fn nodes(
    doc: &Document,
    sem: &Semantics,
    opts: &RenderOptions,
    resolver: &Resolver,
    page: &Table,
) -> Vec<NodeMeta> {
    let root = doc.syntax();
    let mut b = Builder::new(doc, sem, opts, resolver, Mode::Meta, Table::default());
    sem.nodes
        .iter()
        .map(|n| {
            let mut title = b.node_title(&root, n);
            trim_text(&mut title);
            // As inside a link's description: links show their text, no
            // `<a>` (nor a footnote marker) is made.
            b.in_link = true;
            let mut title_unlinked = b.node_title(&root, n);
            b.in_link = false;
            trim_text(&mut title_unlinked);
            unwrap_plain(&mut title_unlinked);
            // Aliases and property values are the semantics' strings, read
            // from the node's drawer: no place of their own, but they sit in
            // the node, so their `:exports` scope is the node's (review
            // round 2: an alias read the file's and showed hidden code).
            let at = Some(n.span.start);
            // The aliases are the ALIASES value split at its commas, and a
            // split can cut a construct the whole value hides (a macro's
            // arguments hold commas too): when the whole value hides
            // anything, no alias is shown (fail closed).
            let whole = n.properties.iter().find(|(k, _)| k == "ALIASES");
            let cut = whole.is_some_and(|(_, v)| b.placeless(at, |b| b.value_hides(v)));
            let aliases = n
                .aliases
                .iter()
                .filter(|_| !cut)
                .map(|a| b.placeless(at, |b| b.inline_text(a)))
                .collect();
            let properties = n
                .properties
                .iter()
                .map(|(k, v)| {
                    let v = b.placeless(at, |b| b.property_value(v));
                    (k.clone(), v)
                })
                .collect();
            NodeMeta {
                id: n.id.clone(),
                anchor: page.nodes.get(&n.id).cloned(),
                title,
                title_unlinked,
                aliases,
                properties,
            }
        })
        .collect()
}

/// `text` as Org objects, inline-only (no document around it).
pub(crate) fn inline(text: &str, opts: &RenderOptions, resolver: &Resolver) -> Vec<Node> {
    let doc = crate::parse("", &crate::ParseOptions::default());
    let sem = Semantics::default();
    let mut b = Builder::new(&doc, &sem, opts, resolver, Mode::Meta, Table::default());
    b.placeless(None, |b| b.inline_text(text))
}

impl Builder<'_> {
    pub fn node_title(&mut self, root: &SyntaxNode, n: &SemanticNode) -> Vec<Node> {
        let mut out = Vec::new();
        match n.origin {
            NodeOrigin::File => {
                if let Some((value, at)) = file_title_source(root) {
                    let tree = crate::green::objects(&value, PARAGRAPH);
                    self.with_base(at, |b| b.objects(&tree, &mut out));
                }
            }
            NodeOrigin::Headline => {
                let title = node_at(root, n.span, HEADLINE)
                    .and_then(|h| h.children().find(|c| c.kind() == HEADLINE_TITLE));
                if let Some(t) = title {
                    self.objects(&t, &mut out);
                }
            }
            NodeOrigin::Item => self.item_title(root, n, &mut out),
        }
        out
    }

    fn item_title(&mut self, root: &SyntaxNode, n: &SemanticNode, out: &mut Vec<Node>) {
        if let Some(item) = node_at(root, n.span, ITEM) {
            self.item_node_title(&item, out);
        }
    }

    /// A KB item's title: its tag and its first paragraph, rendered.
    pub fn item_node_title(&mut self, item: &SyntaxNode, out: &mut Vec<Node>) {
        let Some((drawer, _)) = item_drawer(item) else {
            return;
        };
        let (tag, first) = item_title_parts(item, &drawer);
        let mut tag_nodes = Vec::new();
        if let Some(t) = tag {
            self.objects(&t, &mut tag_nodes);
        }
        trim_text(&mut tag_nodes);
        let mut para = Vec::new();
        if let Some(p) = first {
            self.objects(&p, &mut para);
        }
        trim_text(&mut para);
        let both = !tag_nodes.is_empty() && !para.is_empty();
        tag_nodes.into_iter().for_each(|t| push(out, t));
        if both {
            push(out, text(" :: "));
        }
        para.into_iter().for_each(|t| push(out, t));
    }

    /// `s` parsed as objects and rendered.
    pub fn inline_text(&mut self, s: &str) -> Vec<Node> {
        let tree = crate::green::objects(s, PARAGRAPH);
        let mut out = Vec::new();
        self.objects(&tree, &mut out);
        trim_text(&mut out);
        out
    }

    /// A property value: plain text in which only bracket links are links.
    ///
    /// Org reads a value as text and shows it as written. MAE reads it as
    /// objects and renders them inline-only through the one object path
    /// (so whatever the body hides is hidden at any depth: an inline src
    /// block or babel call, an export snippet, a macro that renders
    /// nothing, a citation's hidden parts), every inline src block hidden
    /// whatever its `:exports`; then keeps only its text, bracket links
    /// (and images) as rendered (registered: `property-value-babel`).
    pub fn property_value(&mut self, value: &str) -> Vec<Node> {
        let tree = crate::green::objects(value, PARAGRAPH);
        let mode = std::mem::replace(&mut self.mode, Mode::Meta);
        let in_value = std::mem::replace(&mut self.in_value, true);
        let mut rendered = Vec::new();
        self.objects(&tree, &mut rendered);
        self.mode = mode;
        self.in_value = in_value;
        let mut out = Vec::new();
        flatten_value(rendered, &mut out);
        trim_text(&mut out);
        out
    }
}

impl Builder<'_> {
    /// Whether `value`, read as objects at this builder's place, holds
    /// anything the renderer hides: an inline src block or results macro
    /// its `:exports` hides, a babel call, an export snippet, or a macro
    /// that renders nothing.
    pub fn value_hides(&self, value: &str) -> bool {
        let tree = crate::green::objects(value, PARAGRAPH);
        let mut objects = tree.descendants().skip(1);
        objects.any(|d| {
            matches!(d.kind(), INLINE_BABEL_CALL | EXPORT_SNIPPET)
                || self.babel_hides_object(&d)
                || (d.kind() == MACRO && self.macro_hidden(&d))
        })
    }
}

/// A rendered property value as its text, but for the links: an `<a>`, an
/// `<img>` and an unresolved link's span stay as rendered; every other
/// element is replaced by its children.
fn flatten_value(nodes: Vec<Node>, out: &mut Vec<Node>) {
    use super::tree::Tag;
    for n in nodes {
        match n {
            Node::Element(e)
                if matches!(e.tag, Tag::A | Tag::Img)
                    || (e.tag == Tag::Span
                        && e.get(super::tree::Attr::Class) == Some("org-unresolved")) =>
            {
                push(out, Node::Element(e))
            }
            Node::Element(e) => flatten_value(e.children, out),
            t => push(out, t),
        }
    }
}

/// Replaces each class-less `<span>` (a link shown as plain text) by its
/// children, recursively, so an unlinked title is the link text itself.
fn unwrap_plain(nodes: &mut Vec<Node>) {
    let old = std::mem::take(nodes);
    for n in old {
        match n {
            Node::Element(mut e) => {
                unwrap_plain(&mut e.children);
                if e.tag == super::tree::Tag::Span && e.attrs.is_empty() {
                    e.children.into_iter().for_each(|c| push(nodes, c));
                } else {
                    push(nodes, Node::Element(e));
                }
            }
            t => push(nodes, t),
        }
    }
}
