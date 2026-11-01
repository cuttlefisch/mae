//! Footnotes: numbered in order of first reference, drawn in one section.
//!
//! Which definitions exist is [`crate::Semantics::footnotes`] (the rendered
//! definition elements; the first one of a label is its definition). A
//! labelled reference with no such definition uses its own inline
//! definition (`[fn:L:text]`) if it has one; with neither it is dangling and
//! shows its source text. Definitions render in number order, and a
//! reference inside one numbers a new note after it: a worklist, so a chain
//! of N notes costs O(N).

use super::build::{trim_text, Builder, Mode};
use super::objects::raw;
use super::tree::{push, text, Attr, Element, Node, Tag};
use crate::kind::SyntaxKind::*;
use crate::kind::{SyntaxElement, SyntaxNode};
use crate::semantics::regions::reference_label;
use crate::{Document, Semantics, Span};
use std::collections::HashMap;

/// Where a note's text comes from.
#[derive(Debug, Clone)]
pub(crate) enum NoteSource {
    /// A `FOOTNOTE_DEFINITION` element.
    Element(SyntaxNode),
    /// A reference with an inline definition.
    Inline(SyntaxNode),
}

#[derive(Debug, Default)]
pub(crate) struct Notes {
    defs: HashMap<String, SyntaxNode>,
    numbers: HashMap<String, usize>,
    /// Per note (number - 1): its source and how many references it has.
    queue: Vec<(NoteSource, usize)>,
}

/// The node of kind `kind` exactly at `span`, if there is one.
pub(crate) fn node_at(
    root: &SyntaxNode,
    span: Span,
    kind: crate::SyntaxKind,
) -> Option<SyntaxNode> {
    let range = rowan::TextRange::new(span.start.into(), span.end.into());
    if range.end() > root.text_range().end() {
        return None;
    }
    let start = match root.covering_element(range) {
        rowan::NodeOrToken::Node(n) => n,
        rowan::NodeOrToken::Token(t) => t.parent()?,
    };
    start
        .ancestors()
        .take_while(|a| a.text_range() == range || a.text_range().contains_range(range))
        .find(|a| a.kind() == kind && a.text_range() == range)
}

impl Notes {
    pub fn new(doc: &Document, sem: &Semantics) -> Notes {
        let root = doc.syntax();
        let mut defs = HashMap::new();
        for f in &sem.footnotes {
            if defs.contains_key(&f.label) {
                continue;
            }
            if let Some(n) = node_at(&root, f.span, FOOTNOTE_DEFINITION) {
                defs.insert(f.label.clone(), n);
            }
        }
        Notes {
            defs,
            ..Notes::default()
        }
    }

    fn add(&mut self, source: NoteSource) -> usize {
        self.queue.push((source, 0));
        self.queue.len()
    }
}

/// The elements of a reference's inline definition: between the `:` that
/// opens it and the closing `]`. `None` for a plain reference.
fn inline_parts(n: &SyntaxNode) -> Option<Vec<SyntaxElement>> {
    let parts: Vec<SyntaxElement> = n.children_with_tokens().collect();
    let is = |e: &SyntaxElement, s: &str| {
        e.as_token()
            .is_some_and(|t| t.kind() == MARKUP && t.text() == s)
    };
    let open = parts.iter().position(|e| is(e, ":"))?;
    let close = parts.iter().rposition(|e| is(e, "]"))?;
    (open < close).then(|| parts[open + 1..close].to_vec())
}

impl Builder<'_> {
    pub fn footnote_reference(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        // Not in titles (shown anywhere) nor inside a link's description
        // (its `<a>` would nest in the link's).
        if self.mode == Mode::Meta || self.in_link {
            return;
        }
        let label = reference_label(n);
        let inline = inline_parts(n).is_some();
        let known = label
            .as_ref()
            .and_then(|l| self.notes.numbers.get(l).copied());
        let number = match (known, &label) {
            (Some(k), _) => Some(k),
            (None, Some(l)) => match self.notes.defs.get(l).cloned() {
                Some(d) => Some(self.notes.add(NoteSource::Element(d))),
                None if inline => Some(self.notes.add(NoteSource::Inline(n.clone()))),
                None => None,
            },
            (None, None) if inline => Some(self.notes.add(NoteSource::Inline(n.clone()))),
            (None, None) => None,
        };
        // A dangling reference (no definition anywhere, none inline) is its
        // source text, `[fn:LABEL]`, as Org exports it: a label holds no
        // object, so nothing hidden can be spelled there.
        let Some(k) = number else {
            return push(out, text(&raw(n)));
        };
        if let Some(l) = label {
            self.notes.numbers.insert(l, k);
        }
        let count = &mut self.notes.queue[k - 1].1;
        *count += 1;
        let id = match *count {
            1 => format!("fnr-{k}"),
            c => format!("fnr-{k}-{c}"),
        };
        if self.mode == Mode::Text {
            return;
        }
        let a = Element::new(Tag::A)
            .attr(Attr::Id, id)
            .attr(Attr::Href, format!("#fn-{k}"))
            .with(vec![text(&k.to_string())]);
        let sup = Element::new(Tag::Sup)
            .class("org-footnote-ref")
            .with(vec![a.into()]);
        push(out, sup.into());
    }

    /// Every note, in number order (rendering one may add more).
    pub fn footnotes_section(&mut self) -> Option<Element> {
        if self.mode == Mode::Meta {
            return None;
        }
        let mut notes = Vec::new();
        let mut i = 0;
        while i < self.notes.queue.len() {
            let source = self.notes.queue[i].0.clone();
            i += 1;
            notes.push(self.note(i, &source).into());
        }
        (!notes.is_empty()).then(|| {
            Element::new(Tag::Section)
                .class("org-footnotes")
                .with(notes)
        })
    }

    fn note(&mut self, k: usize, source: &NoteSource) -> Element {
        let mut body = Vec::new();
        if self.mode == Mode::Page {
            let back = Element::new(Tag::A)
                .class("org-footnote-back")
                .attr(Attr::Href, format!("#fnr-{k}"))
                .with(vec![text(&k.to_string())]);
            push(
                &mut body,
                Element::new(Tag::Sup).with(vec![back.into()]).into(),
            );
        }
        match source {
            NoteSource::Element(d) => self.elements(d, &mut body),
            NoteSource::Inline(r) => {
                let mut inner = Vec::new();
                for e in inline_parts(r).unwrap_or_default() {
                    match e {
                        rowan::NodeOrToken::Token(t) => push(&mut inner, text(t.text())),
                        rowan::NodeOrToken::Node(o) => {
                            self.deeper(&o, |b| b.object(&o, &mut inner))
                        }
                    }
                }
                trim_text(&mut inner);
                push(&mut body, Element::new(Tag::P).with(inner).into());
            }
        }
        Element::new(Tag::Div)
            .class("org-footnote")
            .attr(Attr::Id, format!("fn-{k}"))
            .with(body)
    }
}
