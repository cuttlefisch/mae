//! Blocks, drawers, dynamic blocks, LaTeX environments and footnote
//! definitions: the elements whose extent depends on a closing line further
//! down. Each closing line is found through the lookahead index, bounded by the
//! container's limit, so a `#+begin_X` whose `#+end_X` lies past the next
//! headline is not a block at all: its opening line becomes paragraph text
//! (Org's column-0 rule; oracle-confirmed), with a diagnostic.

use crate::diagnostic::DiagnosticKind;
use crate::element::{Body, Ctx, Element};
use crate::index::Separator;
use crate::kind::SyntaxKind::{self, *};
use crate::paragraph::paragraph;
use crate::scan;

/// `#+begin_NAME` … `#+end_NAME`. `name` is the name's byte range in the line.
pub(crate) fn block(
    cx: &mut Ctx,
    j: usize,
    aff: usize,
    limit: usize,
    name: (usize, usize),
) -> Element {
    let src = cx.src;
    let line = src.content(j);
    let name = &line[name.0..name.1];
    let (kind, greater) = block_kind(name);
    let Some(e) = cx.ix.block_end(src, name, j + 1, limit) else {
        cx.diagnose(
            DiagnosticKind::UnterminatedBlock,
            src.lines[j].start,
            src.lines[j].end,
        );
        return paragraph(cx, aff, src.lines[j].start, limit);
    };
    let body = match (greater, e > j + 1) {
        (_, false) => Body::None,
        (true, true) => Body::Children,
        (false, true) => Body::Opaque,
    };
    delimited(cx, kind, j, e, aff, limit, body)
}

/// The block kind for a `#+begin_` name, and whether its contents are
/// elements (greater) or raw (lesser). Unknown names are special blocks,
/// whose contents Org parses as elements.
fn block_kind(name: &str) -> (SyntaxKind, bool) {
    match name.to_ascii_uppercase().as_str() {
        "CENTER" => (CENTER_BLOCK, true),
        "QUOTE" => (QUOTE_BLOCK, true),
        "COMMENT" => (COMMENT_BLOCK, false),
        "EXAMPLE" => (EXAMPLE_BLOCK, false),
        "EXPORT" => (EXPORT_BLOCK, false),
        "SRC" => (SRC_BLOCK, false),
        "VERSE" => (VERSE_BLOCK, false),
        _ => (SPECIAL_BLOCK, true),
    }
}

/// `:NAME:` … `:END:`.
pub(crate) fn drawer(cx: &mut Ctx, j: usize, aff: usize, limit: usize) -> Element {
    let src = cx.src;
    let Some(e) = cx.ix.drawer_end(src, j + 1, limit) else {
        cx.diagnose(
            DiagnosticKind::UnterminatedDrawer,
            src.lines[j].start,
            src.lines[j].end,
        );
        return paragraph(cx, aff, src.lines[j].start, limit);
    };
    let body = if e > j + 1 {
        Body::Children
    } else {
        Body::None
    };
    delimited(cx, DRAWER, j, e, aff, limit, body)
}

/// `#+BEGIN: name` … `#+END:`.
pub(crate) fn dynamic_block(cx: &mut Ctx, j: usize, aff: usize, limit: usize) -> Element {
    let src = cx.src;
    let Some(e) = cx.ix.dynamic_end(src, j + 1, limit) else {
        cx.diagnose(
            DiagnosticKind::UnterminatedBlock,
            src.lines[j].start,
            src.lines[j].end,
        );
        return paragraph(cx, aff, src.lines[j].start, limit);
    };
    let body = if e > j + 1 {
        Body::Children
    } else {
        Body::None
    };
    delimited(cx, DYNAMIC_BLOCK, j, e, aff, limit, body)
}

/// An element opened on line `j` and closed on line `e`.
fn delimited(
    cx: &Ctx,
    kind: SyntaxKind,
    j: usize,
    e: usize,
    aff: usize,
    limit: usize,
    body: Body,
) -> Element {
    let src = cx.src;
    let head_end = src.lines[j].next;
    let body_end = src.lines[e].start;
    let end = src.skip_blank(src.lines[e].next, limit);
    Element {
        kind,
        begin: aff,
        post_aff: src.lines[j].start,
        head_end,
        body_end,
        end,
        body,
        list: None,
    }
}

/// `\begin{NAME}` … `\end{NAME}`, possibly on one line. Kept whole as text.
pub(crate) fn latex_environment(
    cx: &mut Ctx,
    j: usize,
    aff: usize,
    limit: usize,
    name: &str,
) -> Element {
    let src = cx.src;
    let Some(e) = cx.ix.latex_end(src, name, j, limit) else {
        return paragraph(cx, aff, src.lines[j].start, limit);
    };
    let code_end = src.lines[e].next;
    Element::leaf(
        LATEX_ENVIRONMENT,
        aff,
        src.lines[j].start,
        code_end,
        src.skip_blank(code_end, limit),
    )
}

/// `org-element-footnote-definition-parser`: `[fn:LABEL]` at column 0, up to
/// the next headline, footnote definition or two blank lines.
pub(crate) fn footnote_definition(cx: &Ctx, j: usize, aff: usize, limit: usize) -> Element {
    let src = cx.src;
    let post_aff = src.lines[j].start;
    let end = footnote_end(cx, j, post_aff, limit);
    let label_end = post_aff + scan::footnote_definition(src.content(j)).unwrap_or(0);
    let q = skip_ws_to(cx, label_end, end);
    if q >= end {
        let head_end = src.lines[j].next.min(end);
        return Element::leaf(FOOTNOTE_DEFINITION, aff, post_aff, head_end, end);
    }
    let ql = src.line_of(q);
    let cb = if ql == j { q } else { src.lines[ql].start };
    let ce = src.after_last_nonblank(end, post_aff).max(cb);
    Element {
        kind: FOOTNOTE_DEFINITION,
        begin: aff,
        post_aff,
        head_end: cb,
        body_end: ce,
        end,
        body: Body::Children,
        list: None,
    }
}

fn footnote_end(cx: &Ctx, j: usize, post_aff: usize, limit: usize) -> usize {
    let src = cx.src;
    match cx.ix.footnote_separator(src, j, limit) {
        None => limit,
        Some((l, Separator::Headline)) => src.lines[l].start,
        Some((l, Separator::Footnote)) => {
            // End before any affiliated keywords above the next definition.
            let mut k = l - 1;
            while src.lines[k].start > post_aff && scan::affiliated(src.content(k)).is_some() {
                k -= 1;
            }
            src.lines[k].next
        }
        Some((l, Separator::Blank)) => {
            let q = skip_ws_to(cx, src.lines[l].start, limit);
            if q >= limit {
                limit
            } else {
                src.lines[src.line_of(q)].start
            }
        }
    }
}

/// `skip-chars-forward " \r\t\n"` bounded by `limit`.
fn skip_ws_to(cx: &Ctx, pos: usize, limit: usize) -> usize {
    let b = cx.src.text.as_bytes();
    let limit = limit.min(b.len());
    let mut q = pos;
    while q < limit && matches!(b[q], b' ' | b'\t' | b'\r' | b'\n') {
        q += 1;
    }
    q
}
