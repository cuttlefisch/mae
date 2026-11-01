//! The one tree builder. Every output is a reading of what this produces.
//!
//! A page is built TWICE with the same code: the first pass finds where
//! every anchor lands (headline ids, custom ids, targets, radio targets),
//! the second renders links against that table. Ids are claimed in walk
//! order by the same functions in both passes, so they agree by
//! construction, and a same-page link can only name an id that exists.

use super::anchors::Ids;
use super::babel::Babel;
use super::notes::Notes;
use super::tree::{push, text, Element, Node, Tag};
use super::{RenderDiagnostic, RenderOptions, Resolver, RENDER_MAX_DEPTH};
use crate::kind::SyntaxKind::{self, *};
use crate::kind::{SyntaxElement, SyntaxNode};
use crate::{Document, LineEnding, SemanticLink, Semantics, Span, Transclusion};
use rowan::NodeOrToken;
use std::collections::{HashMap, HashSet};

/// What is being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// A page: ids, footnotes, same-page links.
    Page,
    /// A title, alias or property value shown anywhere: inline only, no id,
    /// no footnote, no same-page link.
    Meta,
    /// Search text: a page without footnote markers, and links shown by
    /// their description only.
    Text,
}

/// Where same-page anchors are, found by the first pass.
#[derive(Debug, Clone, Default)]
pub(crate) struct Table {
    pub custom_ids: HashMap<String, String>,
    pub titles: HashMap<String, String>,
    /// Every heading's `h-` anchor.
    pub headings: HashSet<String>,
    pub targets: HashMap<String, String>,
    pub radios: HashMap<String, String>,
    pub nodes: HashMap<String, String>,
    /// Anchor id → what a link without a description that lands there
    /// shows: a heading's (or KB node's) title as rendered inline-only, a
    /// target's text (which its id already spells).
    pub shown: HashMap<String, String>,
}

/// A built page.
pub(crate) struct Page {
    pub body: Vec<Node>,
    pub table: Table,
    pub diagnostics: Vec<RenderDiagnostic>,
}

pub(crate) struct Builder<'a> {
    pub opts: &'a RenderOptions,
    pub resolver: &'a Resolver,
    pub mode: Mode,
    skip: HashSet<(u32, u32, SyntaxKind)>,
    links: HashMap<u32, &'a SemanticLink>,
    transclusions: HashMap<u32, &'a Transclusion>,
    /// Headline and item starts → the KB node id they carry.
    pub node_ids: HashMap<u32, &'a str>,
    pub done: HashSet<String>,
    pub ids: Ids,
    /// The previous pass's anchors (empty in the first pass).
    pub table: Table,
    /// This pass's anchors.
    pub found: Table,
    pub notes: Notes,
    /// What Org's export removes for babel's `:exports` (see `babel`).
    pub babel: Babel,
    /// The document's line-ending convention: where verbatim text splits
    /// into lines.
    pub eol: LineEnding,
    /// The offset of the text being rendered in the document: `Some(0)` for
    /// the document, a value's offset for a keyword or property value, and
    /// `None` for text with no place in it (an alias), whose links are
    /// classified on their own.
    base: Option<u32>,
    /// Render only what lies in this span (a node's body).
    scope: Option<Span>,
    /// Inside a link's description: a nested link or footnote reference
    /// (Org allows one inside emphasis there) must not become an `<a>`
    /// inside an `<a>`, which an HTML parser would restructure.
    pub in_link: bool,
    depth: usize,
    pub diagnostics: Vec<RenderDiagnostic>,
}

/// Builds the page of `doc` in `mode` (two passes).
pub(crate) fn page(
    doc: &Document,
    sem: &Semantics,
    opts: &RenderOptions,
    resolver: &Resolver,
    mode: Mode,
    scope: Option<Span>,
) -> Page {
    let root = doc.syntax();
    let mut first = Builder::new(doc, sem, opts, resolver, mode, Table::default());
    first.scope = scope;
    first.run(&root);
    let mut second = Builder::new(doc, sem, opts, resolver, mode, first.found);
    second.scope = scope;
    let body = second.run(&root);
    Page {
        body,
        table: second.found,
        diagnostics: second.diagnostics,
    }
}

impl<'a> Builder<'a> {
    pub fn new(
        doc: &Document,
        sem: &'a Semantics,
        opts: &'a RenderOptions,
        resolver: &'a Resolver,
        mode: Mode,
        table: Table,
    ) -> Builder<'a> {
        let skip = sem
            .unrendered
            .iter()
            .map(|u| (u.span.start, u.span.end, u.element))
            .collect();
        let links = sem.links.iter().map(|l| (l.span.start, l)).collect();
        let transclusions = sem
            .transclusions
            .iter()
            .map(|t| (t.span.start, t))
            .collect();
        let node_ids = sem
            .nodes
            .iter()
            .filter(|n| n.origin != crate::NodeOrigin::File)
            .map(|n| (n.span.start, n.id.as_str()))
            .collect();
        let done = doc
            .todo_sequences()
            .iter()
            .flat_map(|s| s.done.iter().cloned())
            .collect();
        Builder {
            opts,
            resolver,
            mode,
            skip,
            links,
            transclusions,
            node_ids,
            done,
            ids: Ids::default(),
            table,
            found: Table::default(),
            notes: Notes::new(doc, sem),
            babel: Babel::new(doc),
            eol: doc.line_ending(),
            base: Some(0),
            scope: None,
            in_link: false,
            depth: 0,
            diagnostics: Vec::new(),
        }
    }

    fn run(&mut self, root: &SyntaxNode) -> Vec<Node> {
        let mut body = Vec::new();
        self.elements(root, &mut body);
        // A note is drawn wherever it is defined: a scope (a node's body)
        // bounds what references it, not where its definition may be.
        let scope = self.scope.take();
        let notes = self.footnotes_section();
        self.scope = scope;
        if let Some(section) = notes {
            push(&mut body, section.into());
        }
        body
    }

    /// Whether the semantics skipped `n`.
    pub fn skipped(&self, n: &SyntaxNode) -> bool {
        let s = Span::of(n);
        self.skip.contains(&(s.start, s.end, n.kind()))
    }

    /// The semantics' reading of the link object starting at `start` (in
    /// the text being rendered).
    pub fn semantic_link(&self, start: u32) -> Option<&'a SemanticLink> {
        let at = self.base?.checked_add(start)?;
        self.links.get(&at).copied()
    }

    /// Where `n` (a node of the text being rendered) is in the document.
    pub fn position(&self, n: &SyntaxNode) -> Option<u32> {
        self.base?.checked_add(Span::of(n).start)
    }

    /// Runs `f` on text at `base` (see the field).
    pub fn with_base<R>(&mut self, base: Option<usize>, f: impl FnOnce(&mut Self) -> R) -> R {
        let base = base.map(|b| u32::try_from(b).unwrap_or(u32::MAX));
        let old = std::mem::replace(&mut self.base, base);
        let r = f(self);
        self.base = old;
        r
    }

    /// The text `f` renders inline-only ([`Mode::Meta`]: no id claimed, no
    /// footnote numbered, no same-page link), whatever this builder's mode:
    /// the same in both passes, so what is derived from it (a heading's
    /// slug) is too.
    pub fn meta_text(&mut self, f: impl FnOnce(&mut Self, &mut Vec<Node>)) -> String {
        let mode = std::mem::replace(&mut self.mode, Mode::Meta);
        let in_link = std::mem::replace(&mut self.in_link, false);
        let mut out = Vec::new();
        f(self, &mut out);
        self.mode = mode;
        self.in_link = in_link;
        crate::semantics::plain::collapse(&super::text::text_of(&out))
    }

    /// Runs `f` one level deeper, or records `n` as too deep and skips it.
    pub fn deeper(&mut self, n: &SyntaxNode, f: impl FnOnce(&mut Self)) {
        if self.depth >= RENDER_MAX_DEPTH {
            self.diagnostics
                .push(RenderDiagnostic::TooDeep(Span::of(n)));
            return;
        }
        self.depth += 1;
        f(self);
        self.depth -= 1;
    }

    /// The elements under `parent`, in order.
    pub fn elements(&mut self, parent: &SyntaxNode, out: &mut Vec<Node>) {
        for c in parent.children_with_tokens() {
            match c {
                NodeOrToken::Node(n) => self.deeper(&n, |b| b.element(&n, out)),
                NodeOrToken::Token(t) if matches!(t.kind(), OPAQUE | UNPARSED) => {
                    let r = t.text_range();
                    let span = Span::new(r.start().into(), r.end().into());
                    self.diagnostics.push(RenderDiagnostic::Unparsed(span));
                }
                NodeOrToken::Token(_) => {}
            }
        }
    }

    fn element(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        // Hidden means hidden: an element the semantics skip, or one Org's
        // export removes for its `:exports`, is not rendered with anything
        // attached to it (its affiliated keywords: a caption, the targets
        // and footnote references in it).
        if self.skipped(n) || self.babel.hides(n) {
            return;
        }
        if let Some(scope) = self.scope {
            let s = Span::of(n);
            if s.end <= scope.start || s.start >= scope.end {
                return;
            }
            if s.start < scope.start || s.end > scope.end {
                // Across the scope's edge: a container contributes what is
                // inside the scope; any other element belongs to the node it
                // STARTS in, whole or not at all, so no element is split or
                // counted in two nodes' text.
                if is_container(n.kind()) {
                    return self.elements(n, out);
                }
                if s.start < scope.start {
                    return;
                }
            }
        }
        match n.kind() {
            DOCUMENT | SECTION => self.elements(n, out),
            HEADLINE => self.headline(n, out),
            PARAGRAPH => self.captioned(n, out, |b, out| b.paragraph(n, out)),
            PLAIN_LIST => self.list(n, out),
            TABLE => self.table(n, out),
            QUOTE_BLOCK => self.container(n, Element::new(Tag::Blockquote), out),
            CENTER_BLOCK => self.container(n, Element::new(Tag::Div).class("org-center"), out),
            DYNAMIC_BLOCK => {
                let div = Element::new(Tag::Div).class("org-dynamic-block");
                self.container(n, div, out)
            }
            SPECIAL_BLOCK => self.special_block(n, out),
            VERSE_BLOCK => self.verse(n, out),
            SRC_BLOCK => self.captioned(n, out, |b, out| b.src(n, out)),
            EXAMPLE_BLOCK => self.captioned(n, out, |b, out| b.verbatim(n, "org-example", out)),
            FIXED_WIDTH => self.fixed_width(n, out),
            LATEX_ENVIRONMENT => self.latex_environment(n, out),
            HORIZONTAL_RULE => push(out, Element::new(Tag::Hr).into()),
            KEYWORD => self.keyword(n, out),
            PROPERTY_DRAWER | DRAWER => self.drawer(n, out),
            // Only reached in a scope that starts inside a list.
            ITEM => self.deeper(n, |b| b.item(n, false, out)),
            _ => {}
        }
    }

    /// A greater block: its elements inside `wrapper`.
    fn container(&mut self, n: &SyntaxNode, wrapper: Element, out: &mut Vec<Node>) {
        let mut inner = Vec::new();
        self.elements(n, &mut inner);
        push(out, wrapper.with(inner).into());
    }

    fn special_block(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let mut div = Element::new(Tag::Div).class("org-special-block");
        let name = crate::Block::cast(n.clone())
            .map(|b| b.name())
            .unwrap_or_default();
        if super::links::is_valid_block_name(&name) {
            div = div.attr(super::Attr::DataOrgBlock, name);
        }
        self.container(n, div, out)
    }

    /// `render` wrapped in a figure when `n` has a `#+CAPTION`.
    fn captioned(
        &mut self,
        n: &SyntaxNode,
        out: &mut Vec<Node>,
        render: impl FnOnce(&mut Self, &mut Vec<Node>),
    ) {
        // A paragraph that shows nothing (only an export snippet, or an
        // inline src block its `:exports` removes) takes its caption with
        // it, as the caption of a removed block goes with the block.
        if n.kind() == PARAGRAPH && self.shows_nothing(n) {
            return;
        }
        let caption = self.caption(n);
        if caption.is_empty() {
            return render(self, out);
        }
        let mut inner = Vec::new();
        render(self, &mut inner);
        let cap = Element::new(Tag::Figcaption).with(caption);
        let mut children = inner;
        children.push(cap.into());
        let fig = Element::new(Tag::Figure).class("org-figure").with(children);
        push(out, fig.into());
    }

    /// The objects of `n`'s `#+CAPTION` values, joined by a space.
    pub fn caption(&mut self, n: &SyntaxNode) -> Vec<Node> {
        let mut out = Vec::new();
        let values = n
            .children()
            .filter(|c| c.kind() == AFFILIATED_KEYWORD && is_caption(c))
            .flat_map(|c| c.children().filter(|v| v.kind() == AFFILIATED_VALUE))
            .collect::<Vec<_>>();
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                push(&mut out, text(" "));
            }
            self.objects(v, &mut out);
        }
        trim_text(&mut out);
        out
    }

    /// Whether paragraph `n` renders to nothing: blanks, affiliated
    /// keywords, and objects that are never shown.
    fn shows_nothing(&self, n: &SyntaxNode) -> bool {
        n.children_with_tokens().all(|c| match c {
            NodeOrToken::Token(t) => t.text().chars().all(char::is_whitespace),
            NodeOrToken::Node(o) => {
                matches!(
                    o.kind(),
                    AFFILIATED_KEYWORD | EXPORT_SNIPPET | INLINE_BABEL_CALL
                ) || self.babel_hides_object(&o)
            }
        })
    }

    fn paragraph(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let mut inner = Vec::new();
        self.objects(n, &mut inner);
        trim_text(&mut inner);
        if inner.is_empty() {
            return;
        }
        push(out, Element::new(Tag::P).with(inner).into());
    }

    fn keyword(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        let Some(t) = self.transclusions.get(&Span::of(n).start).copied() else {
            return;
        };
        let link = self.transclusion(n, t);
        let p = Element::new(Tag::P).class("org-transclusion").with(link);
        push(out, p.into());
    }
}

impl Builder<'_> {
    /// A property drawer (a headline's, the file's or an item's), when
    /// they are rendered: keys canonical, values plain text in which only
    /// bracket links are links.
    fn drawer(&mut self, n: &SyntaxNode, out: &mut Vec<Node>) {
        if !self.opts.property_drawers || self.mode == Mode::Meta {
            return;
        }
        let props: Vec<(String, String, usize)> = if n.kind() == PROPERTY_DRAWER {
            n.children()
                .filter(|c| c.kind() == NODE_PROPERTY)
                .filter_map(|p| {
                    let key = p
                        .children_with_tokens()
                        .filter_map(SyntaxElement::into_token)
                        .find(|t| t.kind() == NAME)?;
                    let key = crate::views::canonical_key(key.text());
                    let (value, at) = crate::semantics::value_token(&p).unwrap_or_default();
                    Some((key, value, at))
                })
                .collect()
        } else {
            match crate::semantics::items::item_property_drawer(n) {
                Some(ps) => ps
                    .into_iter()
                    .map(|p| (crate::views::canonical_key(&p.key), p.value, p.value_start))
                    .collect(),
                None => return,
            }
        };
        let mut dl = Vec::new();
        for (key, value, at) in props {
            push(&mut dl, Element::new(Tag::Dt).with(vec![text(&key)]).into());
            let v = self.with_base(Some(at), |b| b.property_value(&value));
            push(&mut dl, Element::new(Tag::Dd).with(v).into());
        }
        let dl = Element::new(Tag::Dl).class("org-properties").with(dl);
        push(out, dl.into());
    }
}

/// Elements whose children are elements (what a scope may cut through).
fn is_container(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        DOCUMENT
            | SECTION
            | HEADLINE
            | PLAIN_LIST
            | ITEM
            | QUOTE_BLOCK
            | CENTER_BLOCK
            | SPECIAL_BLOCK
            | DYNAMIC_BLOCK
            | DRAWER
            | PROPERTY_DRAWER
            | FOOTNOTE_DEFINITION
    )
}

/// The search-text trees of KB node `node`: its title, and its body (its
/// `body_span`) followed by every footnote either references, wherever
/// the note is defined. One builder, so a note the title and the body both
/// reference appears once.
pub(crate) fn node_text_trees(
    doc: &Document,
    sem: &Semantics,
    node: &crate::SemanticNode,
    resolver: &Resolver,
) -> (Vec<Node>, Vec<Node>) {
    let opts = RenderOptions::default();
    let root = doc.syntax();
    let mut b = Builder::new(doc, sem, &opts, resolver, Mode::Text, Table::default());
    let title = b.node_title(&root, node);
    b.scope = Some(node.body_span);
    let body = b.run(&root);
    (title, body)
}

fn is_caption(affiliated: &SyntaxNode) -> bool {
    affiliated
        .children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|t| t.kind() == KEY)
        .is_some_and(|k| k.text().eq_ignore_ascii_case("CAPTION"))
}

/// Trims blanks and line endings from the ends of the text at both ends of
/// `nodes`, dropping text that becomes empty.
pub(crate) fn trim_text(nodes: &mut Vec<Node>) {
    let blank = [' ', '\t', '\n'];
    if let Some(Node::Text(t)) = nodes.first_mut() {
        *t = t.trim_start_matches(blank).to_string();
    }
    if let Some(Node::Text(t)) = nodes.last_mut() {
        *t = t.trim_end_matches(blank).to_string();
    }
    nodes.retain(|n| !matches!(n, Node::Text(t) if t.is_empty()));
}

/// `s` as lines, each with its line ending, in the document's convention:
/// `\n` (a CR is content), `\r\n` (a lone CR is content) or `\r` (the
/// file has no LF). Verbatim text is split by this, never by `\n` alone,
/// or a CR-only file would be one line.
pub(crate) fn lines(s: &str, eol: LineEnding) -> Vec<&str> {
    match eol {
        LineEnding::Lf => s.split_inclusive('\n').collect(),
        LineEnding::Cr => s.split_inclusive('\r').collect(),
        LineEnding::CrLf => {
            let mut out = Vec::new();
            let mut rest = s;
            while let Some(i) = rest.find("\r\n") {
                out.push(&rest[..i + 2]);
                rest = &rest[i + 2..];
            }
            if !rest.is_empty() {
                out.push(rest);
            }
            out
        }
    }
}

/// `s` without its trailing line endings (in the document's convention).
pub(crate) fn trim_eol(s: &str, eol: LineEnding) -> &str {
    let end = match eol {
        LineEnding::Lf => "\n",
        LineEnding::CrLf => "\r\n",
        LineEnding::Cr => "\r",
    };
    let mut s = s;
    while let Some(r) = s.strip_suffix(end) {
        s = r;
    }
    s
}
