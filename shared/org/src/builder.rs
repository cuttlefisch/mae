//! The iterative driver: an explicit stack of open containers.
//!
//! `org-element` parses by recursion — an element's contents are parsed by a
//! nested call. Here a greater element pushes a frame instead, so input
//! nesting never becomes call-stack depth. When the stack is as deep as
//! `ParseOptions::max_depth`, a greater element's contents are kept as one
//! opaque token with a diagnostic instead of being parsed.

use crate::diagnostic::{Diagnostic, DiagnosticKind};
use crate::element::{recognize, Body, Ctx, Element, Mode};
use crate::index::Index;
use crate::kind::SyntaxKind::{self, *};
use crate::options::{is_todo_key, ParseOptions, TodoSequence, MAX_DEPTH_CEILING};
use crate::scan;
use crate::span::{contents_of, ContentsTable, Span};
use crate::text::{trim_end_blank, Source};
use crate::tokens::{self, Emitter, Event};

struct Frame {
    kind: SyntaxKind,
    /// Where this container's contents end.
    limit: usize,
    /// Where the container itself ends (after its closing line and blanks).
    end: usize,
    /// The mode for the next child.
    mode: Mode,
    list: Option<usize>,
}

pub(crate) struct Built {
    pub events: Vec<Event>,
    pub contents: ContentsTable,
    pub diagnostics: Vec<Diagnostic>,
    pub in_file_todo: Vec<TodoSequence>,
}

/// What the text is: a whole document, or the body of a headline of the
/// given level (`parse_fragment`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Start {
    Document,
    Fragment { level: usize },
}

pub(crate) fn build(src: &Source, ix: &Index, opts: &ParseOptions, start: Start) -> Built {
    let mut b = Builder::new(src, ix, opts, start);
    while !b.stack.is_empty() {
        b.step();
    }
    b.contents.finish();
    Built {
        events: b.em.events,
        contents: b.contents,
        diagnostics: b.cx.diagnostics,
        in_file_todo: b.todo,
    }
}

struct Builder<'a> {
    cx: Ctx<'a>,
    em: Emitter<'a>,
    todo: Vec<TodoSequence>,
    contents: ContentsTable,
    stack: Vec<Frame>,
    cursor: usize,
    cap: usize,
    start: Start,
}

impl<'a> Builder<'a> {
    fn new(src: &'a Source<'a>, ix: &'a Index, opts: &ParseOptions, start: Start) -> Builder<'a> {
        let mut cx = Ctx::new(src, ix);
        // A fragment is a headline's body: its first line follows the headline
        // line, so it starts in the headline's `section` mode (planning and a
        // property drawer are allowed right at its top).
        cx.after_headline = matches!(start, Start::Fragment { .. });
        let mode = if cx.after_headline {
            Mode::Section
        } else {
            Mode::FirstSection
        };
        let mut em = Emitter::new(src);
        em.start(DOCUMENT);
        em.tok(BOM, 0, src.bom);
        let cursor = cx.start(cx.first_nonblank);
        em.lines(src.bom, cursor, TEXT);
        let root = Frame {
            kind: DOCUMENT,
            limit: src.len(),
            end: src.len(),
            mode,
            list: None,
        };
        let cap = opts.max_depth.min(MAX_DEPTH_CEILING);
        Builder {
            cx,
            em,
            todo: Vec::new(),
            contents: ContentsTable::default(),
            stack: vec![root],
            cursor,
            cap,
            start,
        }
    }

    /// Closes the innermost container if its contents are done, otherwise
    /// parses the next element in it.
    fn step(&mut self) {
        let Some(top) = self.stack.last_mut() else {
            return;
        };
        if self.cursor >= top.limit {
            if let Some(f) = self.stack.pop() {
                tail(&mut self.em, f.kind, f.limit, f.end);
                self.em.finish();
                self.cursor = f.end;
            }
            return;
        }
        let (limit, parent_mode, list) = (top.limit, top.mode, top.list);
        let src = self.cx.src;
        let el = sane(
            recognize(&mut self.cx, self.cursor, limit, parent_mode, list),
            self.cursor,
            limit,
            src,
        );
        if let Some(top) = self.stack.last_mut() {
            top.mode = parent_mode.after(el.kind);
        }
        if el.begin > self.cursor {
            self.em.lines(self.cursor, el.begin, UNPARSED);
        }
        self.record(&el);
        open(&mut self.em, &el);
        self.descend_or_close(&el, parent_mode);
    }

    /// Side data for an element: TODO declarations, contents, diagnostics.
    fn record(&mut self, el: &Element) {
        let src = self.cx.src;
        if el.kind == KEYWORD {
            collect_todo(src, el, &mut self.todo);
        }
        if let Some((a, b)) = contents_of(el, src) {
            self.contents
                .push(el.kind, el.begin, el.end, Span::new(a, b));
        }
        if let (HEADLINE, Start::Fragment { level }) = (el.kind, self.start) {
            let line = src.lines[src.line_of(el.begin)];
            if scan::headline_level(src.content(src.line_of(el.begin))).is_some_and(|l| l <= level)
            {
                self.cx.diagnose(
                    DiagnosticKind::HeadlineOutsideFragment,
                    line.start,
                    line.end,
                );
            }
        }
    }

    /// Pushes a frame for a greater element's contents, or emits the rest
    /// of the element and closes it.
    fn descend_or_close(&mut self, el: &Element, parent_mode: Mode) {
        if el.body == Body::Children && el.body_end > el.head_end {
            if self.stack.len() < self.cap {
                let list = matches!(el.kind, PLAIN_LIST | ITEM)
                    .then_some(el.list)
                    .flatten();
                let mode = parent_mode.for_children(el.kind);
                self.stack.push(Frame {
                    kind: el.kind,
                    limit: el.body_end,
                    end: el.end,
                    mode,
                    list,
                });
                self.cursor = el.head_end;
                return;
            }
            self.cx
                .diagnose(DiagnosticKind::DepthLimit, el.head_end, el.body_end);
            self.em.tok(OPAQUE, el.head_end, el.body_end);
        } else {
            body(&mut self.em, el);
        }
        tail(&mut self.em, el.kind, el.body_end, el.end);
        self.em.finish();
        self.cursor = el.end;
    }
}

/// Clamps an element into `[cursor, limit]` with ordered boundaries and at
/// least one byte, so a recogniser defect can cost structure, never
/// termination or losslessness.
fn sane(mut el: Element, cursor: usize, limit: usize, src: &Source) -> Element {
    let clamp = |x: usize| x.clamp(cursor, limit);
    el.begin = clamp(el.begin);
    el.end = clamp(el.end);
    if el.end <= el.begin {
        let next = src
            .next_line_start(el.begin)
            .clamp(el.begin + 1, limit.max(el.begin + 1));
        return Element::leaf(
            PARAGRAPH,
            el.begin,
            el.begin,
            next.min(limit),
            next.min(limit),
        );
    }
    el.post_aff = el.post_aff.clamp(el.begin, el.end);
    el.head_end = el.head_end.clamp(el.post_aff, el.end);
    el.body_end = el.body_end.clamp(el.head_end, el.end);
    el
}

/// Starts the element's node and emits its affiliated keywords and head.
fn open(em: &mut Emitter, el: &Element) {
    em.start(el.kind);
    let mut a = el.begin;
    while a < el.post_aff {
        let next = em.src.next_line_start(a).min(el.post_aff);
        em.start(AFFILIATED_KEYWORD);
        tokens::keyword_line(em, a, next, true);
        em.finish();
        a = next;
    }
    head(em, el);
}

fn head(em: &mut Emitter, el: &Element) {
    let (a, b) = (el.post_aff, el.head_end);
    if a >= b {
        return;
    }
    let first_end = em.src.next_line_start(a).min(b);
    match el.kind {
        HEADLINE => {
            let l = em.src.lines[em.src.line_of(a)];
            em.events.push(Event::HeadlineLine(l.start, l.end.min(b)));
            em.tok(NEWLINE, l.end.min(b), first_end);
            em.lines(first_end, b, TEXT);
        }
        KEYWORD | BABEL_CALL => tokens::keyword_line(em, a, b, false),
        PLANNING => tokens::planning_line(em, a, b),
        NODE_PROPERTY => tokens::property_line(em, a, b),
        ITEM => tokens::item_line(em, a, b),
        FOOTNOTE_DEFINITION => tokens::footnote_head(em, a, b),
        k if closes(k) => {
            tokens::begin_line(em, k, a, first_end);
            em.lines(first_end, b, TEXT);
        }
        _ => em.lines(a, b, TEXT),
    }
}

/// Kinds with a closing line in their tail.
fn closes(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        DRAWER
            | PROPERTY_DRAWER
            | DYNAMIC_BLOCK
            | CENTER_BLOCK
            | QUOTE_BLOCK
            | SPECIAL_BLOCK
            | SRC_BLOCK
            | EXAMPLE_BLOCK
            | EXPORT_BLOCK
            | COMMENT_BLOCK
            | VERSE_BLOCK
    )
}

fn body(em: &mut Emitter, el: &Element) {
    match el.body {
        Body::Opaque => em.tok(BLOCK_CONTENT, el.head_end, el.body_end),
        _ => em.lines(el.head_end, el.body_end, TEXT),
    }
}

/// `[from, end)`: the closing line for kinds that have one, then blank lines
/// (and `#+TBLFM:` lines for tables).
fn tail(em: &mut Emitter, kind: SyntaxKind, from: usize, end: usize) {
    if from >= end {
        return;
    }
    let mut a = from;
    if closes(kind) {
        let next = em.src.next_line_start(a).min(end);
        tokens::end_line(em, kind, a, next);
        a = next;
    }
    em.lines(a, end, TEXT);
}

/// Records an in-file `#+TODO:`/`#+SEQ_TODO:`/`#+TYP_TODO:` keyword.
fn collect_todo(src: &Source, el: &Element, out: &mut Vec<TodoSequence>) {
    let line = src.content(src.line_of(el.post_aff));
    let Some((a, b)) = scan::keyword_key(line) else {
        return;
    };
    if is_todo_key(&line[a..b]) {
        // Pushed even when empty: an empty declaration still replaces the
        // configured sequences (oracle).
        out.push(TodoSequence::parse(trim_end_blank(&line[b + 1..])));
    }
}
