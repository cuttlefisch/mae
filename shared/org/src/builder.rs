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
use crate::options::{is_todo_key, ParseOptions, TodoSequence};
use crate::scan;
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
    pub diagnostics: Vec<Diagnostic>,
    pub in_file_todo: Vec<TodoSequence>,
}

pub(crate) fn build(src: &Source, ix: &Index, opts: &ParseOptions) -> Built {
    let mut cx = Ctx::new(src, ix);
    let mut em = Emitter::new(src);
    let mut todo = Vec::new();
    em.start(DOCUMENT);
    em.tok(BOM, 0, src.bom);
    let mut cursor = cx.start(cx.first_nonblank);
    em.lines(src.bom, cursor, TEXT);
    let mut stack = vec![Frame {
        kind: DOCUMENT,
        limit: src.len(),
        end: src.len(),
        mode: Mode::FirstSection,
        list: None,
    }];
    while let Some(top) = stack.last_mut() {
        if cursor >= top.limit {
            if let Some(f) = stack.pop() {
                tail(&mut em, f.kind, f.limit, f.end);
                em.finish();
                cursor = f.end;
            }
            continue;
        }
        let parent_mode = top.mode;
        let el = sane(
            recognize(&mut cx, cursor, top.limit, parent_mode, top.list),
            cursor,
            top.limit,
            src,
        );
        top.mode = parent_mode.after(el.kind);
        if el.begin > cursor {
            em.lines(cursor, el.begin, UNPARSED);
        }
        if el.kind == KEYWORD {
            collect_todo(src, &el, &mut todo);
        }
        open(&mut em, &el);
        if el.body == Body::Children && el.body_end > el.head_end {
            if stack.len() >= opts.max_depth {
                cx.diagnose(DiagnosticKind::DepthLimit, el.head_end, el.body_end);
                em.tok(OPAQUE, el.head_end, el.body_end);
            } else {
                let list = matches!(el.kind, PLAIN_LIST | ITEM)
                    .then_some(el.list)
                    .flatten();
                let mode = parent_mode.for_children(el.kind);
                stack.push(Frame {
                    kind: el.kind,
                    limit: el.body_end,
                    end: el.end,
                    mode,
                    list,
                });
                cursor = el.head_end;
                continue;
            }
        } else {
            body(&mut em, &el);
        }
        tail(&mut em, el.kind, el.body_end, el.end);
        em.finish();
        cursor = el.end;
    }
    Built {
        events: em.events,
        diagnostics: cx.diagnostics,
        in_file_todo: todo,
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
        if let Some(seq) = TodoSequence::parse(trim_end_blank(&line[b + 1..])) {
            out.push(seq);
        }
    }
}
