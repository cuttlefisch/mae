//! KB node metadata: titles, aliases and properties, rendered inline-only
//! through the same builder as the body.

use super::build::{trim_text, Builder, Mode, Table};
use super::links::is_bracket;
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
            let aliases = n
                .aliases
                .iter()
                .map(|a| b.with_base(None, |b| b.inline_text(a)))
                .collect();
            let properties = n
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), b.with_base(None, |b| b.property_value(v))))
                .collect();
            NodeMeta {
                id: n.id.clone(),
                anchor: page.nodes.get(&n.id).cloned(),
                title,
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
    b.with_base(None, |b| b.inline_text(text))
}

impl Builder<'_> {
    pub fn node_title(&mut self, root: &SyntaxNode, n: &SemanticNode) -> Vec<Node> {
        let mut out = Vec::new();
        match n.origin {
            NodeOrigin::File => {
                if let Some((value, at)) = file_title_source(root) {
                    let tree = crate::green::objects(&value, PARAGRAPH);
                    self.with_base(Some(at), |b| b.objects(&tree, &mut out));
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
        let Some(item) = node_at(root, n.span, ITEM) else {
            return;
        };
        let Some((drawer, _)) = item_drawer(&item) else {
            return;
        };
        let (tag, first) = item_title_parts(&item, &drawer);
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
    pub fn property_value(&mut self, value: &str) -> Vec<Node> {
        let tree = crate::green::objects(value, PARAGRAPH);
        let mut out = Vec::new();
        for c in tree.children_with_tokens() {
            match c {
                rowan::NodeOrToken::Node(l) if l.kind() == LINK && is_bracket(&l) => {
                    self.deeper(&l, |b| b.object(&l, &mut out))
                }
                other => push(&mut out, text(&other.to_string())),
            }
        }
        trim_text(&mut out);
        out
    }
}
