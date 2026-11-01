//! Footnotes: numbered in order of first reference, drawn in one section.
//!
//! Which definitions exist is [`crate::Semantics::footnotes`] (the rendered
//! definition elements; the first one of a label is its definition). A
//! labelled reference with no such definition uses its own inline
//! definition (`[fn:L:text]`) if it has one; with neither it is undefined:
//! it renders nothing, with a [`super::RenderDiagnostic::FootnoteUndefined`]
//! (Org's export stops there, "Definition not found for footnote"; a draft
//! with one stays publishable, and a consumer can refuse it). Definitions
//! render in number order, and a
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
    defs: std::rc::Rc<HashMap<String, SyntaxNode>>,
    numbers: HashMap<String, usize>,
    /// Per note (number - 1): its source, how many references it has, and
    /// whether one is outside [`Builder::blank`].
    queue: Vec<(NoteSource, usize, bool)>,
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
    /// Each footnote label's definition element.
    pub fn defs(doc: &Document, sem: &Semantics) -> HashMap<String, SyntaxNode> {
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
        defs
    }

    pub fn with_defs(defs: std::rc::Rc<HashMap<String, SyntaxNode>>) -> Notes {
        Notes {
            defs,
            ..Notes::default()
        }
    }

    fn add(&mut self, source: NoteSource) -> usize {
        self.queue.push((source, 0, false));
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
        // An undefined reference (no definition in the exported document,
        // none inline: the definition may sit in a COMMENT or `:noexport:`
        // subtree) shows nothing, not even its label.
        let Some(k) = number else {
            // The reference itself, its post-blank excluded.
            let at = Span::of(n).start as usize;
            let d = super::RenderDiagnostic::FootnoteUndefined(Span::new(at, at + raw(n).len()));
            self.diagnostics.push(d);
            return;
        };
        if let Some(l) = label {
            self.notes.numbers.insert(l, k);
        }
        let note = &mut self.notes.queue[k - 1];
        note.1 += 1;
        note.2 |= !self.in_blank;
        if note.2 {
            self.found.outside_notes.insert(k);
        }
        let id = match note.1 {
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

    /// Every note, in number order (rendering one may add more). With a
    /// [`Builder::blank`] span, a note defined in it and referenced only
    /// from it is the blank text's own: rendered (what it references is
    /// numbered) and thrown away.
    pub fn footnotes_section(&mut self) -> Option<Element> {
        if self.mode == Mode::Meta {
            return None;
        }
        // A note is drawn whole wherever it is defined, the blank span
        // included, unless it is the blank text's own.
        let blank = self.take_blank();
        let mut notes = Vec::new();
        let mut i = 0;
        while i < self.notes.queue.len() {
            let (source, _, outside) = self.notes.queue[i].clone();
            i += 1;
            let at = match &source {
                NoteSource::Element(d) => Span::of(d),
                NoteSource::Inline(r) => Span::of(r),
            };
            let inside = blank.is_some_and(|b| at.start >= b.start && at.end <= b.end);
            if inside && !outside && !self.table.outside_notes.contains(&i) {
                let was = std::mem::replace(&mut self.in_blank, true);
                self.note(i, &source);
                self.in_blank = was;
                continue;
            }
            notes.push(self.note(i, &source).into());
        }
        self.set_blank(blank);
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
