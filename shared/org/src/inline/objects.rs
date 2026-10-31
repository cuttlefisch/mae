//! The small object parsers of `org-element`: table cells, statistics
//! cookies, line breaks, export snippets, macros, targets, radio targets and
//! footnote references. Each reads at most its own extent, or asks the
//! [`super::InlineIndex`] where its closer is.

use super::index::{Bracket, Needle};
use super::{InlineCx, Obj};
use crate::chars::is_word;
use crate::kind::SyntaxKind::*;

/// `[ \t]*\(.*?\)[ \t]*\(?:|\|$\)` at `i`: a cell runs to the next `|` (or
/// the end of the row's contents), its contents without surrounding blanks.
/// Always succeeds, possibly with empty contents.
pub(crate) fn table_cell(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let b = cx.bytes();
    let cb = cx.skip_blanks(i, end);
    let le = cx.line_end(cb, end);
    let bar = b[cb..le].iter().position(|&c| c == b'|').map(|k| cb + k);
    let stop = bar.unwrap_or(le);
    let mut ce = stop;
    while ce > cb && matches!(b[ce - 1], b' ' | b'\t') {
        ce -= 1;
    }
    let cell_end = bar.map_or(le, |p| p + 1);
    Some(
        Obj::new(TABLE_CELL, i, cell_end)
            .tok(WHITESPACE, i, cb)
            .contents(cb, ce)
            .tok(WHITESPACE, ce, stop)
            .tok(MARKUP, stop, cell_end),
    )
}

/// `\[[0-9]*\(%\|/[0-9]*\)\]`.
pub(crate) fn statistics_cookie(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let b = cx.bytes();
    let digits = |k: usize| k + b[k..end].iter().take_while(|c| c.is_ascii_digit()).count();
    let k = digits(i + 1);
    let k = match b.get(k).filter(|_| k < end)? {
        b'%' => k + 1,
        b'/' => digits(k + 1),
        _ => return None,
    };
    (k < end && b[k] == b']').then_some(())?;
    let value_end = k + 1;
    let obj_end = cx.skip_blanks(value_end, end);
    Some(
        Obj::new(STATISTICS_COOKIE, i, obj_end)
            .tok(VALUE, i, value_end)
            .blank(value_end),
    )
}

/// `\\\\[ \t]*$`, not preceded by a backslash; it ends at the start of the
/// next line.
pub(crate) fn line_break(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    if cx.ch_before(i, floor) == Some('\\') {
        return None;
    }
    let q = cx.skip_blanks(i + 2, end);
    if !cx.at_eol(q, end) {
        return None;
    }
    let next = if q >= end {
        end
    } else {
        (q + cx.eol_len(q)).min(end)
    };
    Some(
        Obj::new(LINE_BREAK, i, next)
            .tok(MARKUP, i, i + 2)
            .tok(WHITESPACE, i + 2, next),
    )
}

/// `@@BACKEND:VALUE@@`: BACKEND is `[-A-Za-z0-9]+`; VALUE runs to the next `@@`.
pub(crate) fn export_snippet(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let b = cx.bytes();
    let name = i + 2;
    let colon = name
        + b[name..end]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-')
            .count();
    if colon == name || colon >= end || b[colon] != b':' {
        return None;
    }
    let close = cx.ix.next(Needle::AtAt, colon + 1, end)?;
    let value_end = close + 2;
    let obj_end = cx.skip_blanks(value_end, end);
    Some(
        Obj::new(EXPORT_SNIPPET, i, obj_end)
            .tok(MARKUP, i, name)
            .tok(NAME, name, colon)
            .tok(MARKUP, colon, colon + 1)
            .tok(VALUE, colon + 1, close)
            .tok(MARKUP, close, value_end)
            .blank(value_end),
    )
}

/// `{{{NAME}}}` or `{{{NAME(ARGS)}}}`: NAME is `[a-zA-Z][-a-zA-Z0-9_]*`;
/// ARGS is the shortest text (newlines included) before `)}}}`.
pub(crate) fn macro_(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let b = cx.bytes();
    let name = i + 3;
    if !b.get(name).is_some_and(u8::is_ascii_alphabetic) || name >= end {
        return None;
    }
    let n = name
        + 1
        + b[name + 1..end]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-' || **c == b'_')
            .count();
    let obj = |value_end: usize| {
        let obj_end = cx.skip_blanks(value_end, end);
        Obj::new(MACRO, i, obj_end)
            .tok(MARKUP, i, name)
            .tok(NAME, name, n)
    };
    if b.get(n) == Some(&b'(') {
        if let Some(close) = cx.ix.next(Needle::MacroClose, n + 1, end) {
            let o = obj(close + 4);
            let ve = close + 4;
            return Some(
                o.tok(MARKUP, n, n + 1)
                    .tok(VALUE, n + 1, close)
                    .tok(MARKUP, close, ve)
                    .blank(ve),
            );
        }
    }
    cx.starts(n, end, "}}}")
        .then(|| obj(n + 3).tok(MARKUP, n, n + 3).blank(n + 3))
}

/// The value of `org-target-regexp` starting at `v`: border chars
/// (`[^<>\n\r \t]`) at both ends, `[^<>\n\r]` between, and then `>>`
/// (`close` of them). Returns where the value ends.
fn target_value(cx: &InlineCx, v: usize, end: usize, close: &str) -> Option<usize> {
    let b = cx.bytes();
    let stop = v + b[v..end]
        .iter()
        .take_while(|c| !matches!(c, b'<' | b'>' | b'\n' | b'\r'))
        .count();
    let border = |c: u8| c != b' ' && c != b'\t';
    let ok = stop > v && border(b[v]) && border(b[stop - 1]) && cx.starts(stop, end, close);
    ok.then_some(stop)
}

/// `<<TARGET>>`.
pub(crate) fn target(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let stop = target_value(cx, i + 2, end, ">>")?;
    let obj_end = cx.skip_blanks(stop + 2, end);
    Some(Obj::opaque(TARGET, i, (i + 2, stop), stop + 2, obj_end))
}

/// `<<<TARGET>>>`: its contents hold minimal objects.
pub(crate) fn radio_target(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    if !cx.starts(i, end, "<<<") {
        return None;
    }
    let stop = target_value(cx, i + 3, end, ">>>")?;
    let obj_end = cx.skip_blanks(stop + 3, end);
    Some(Obj::delimited(
        RADIO_TARGET,
        i,
        (i + 3, stop),
        stop + 3,
        obj_end,
    ))
}

/// `org-footnote-re`: `[fn:LABEL]` (standard), `[fn:LABEL:DEFINITION]` or
/// `[fn::DEFINITION]` (inline), closed by the `]` matching the `[`.
pub(crate) fn footnote_reference(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    if !cx.starts_ci(i, end, "[fn:") {
        return None;
    }
    let b = cx.bytes();
    let label = i + 4;
    let mut l = label;
    while let Some(c) = cx
        .ch(l, end)
        .filter(|&c| c == '-' || c == '_' || is_word(c))
    {
        l += c.len_utf8();
    }
    let inline = match b.get(l).filter(|_| l < end) {
        Some(b':') => true,
        Some(b']') if l > label => false,
        _ => return None,
    };
    let close = cx.ix.pair(Bracket::Square, i, end)?.close;
    let obj_end = cx.skip_blanks(close + 1, end);
    let o = Obj::new(FOOTNOTE_REFERENCE, i, obj_end)
        .tok(MARKUP, i, label)
        .tok(NAME, label, l);
    let o = if inline {
        o.tok(MARKUP, l, l + 1).contents(l + 1, close)
    } else {
        o
    };
    let markup_from = if inline { close } else { l };
    Some(o.tok(MARKUP, markup_from, close + 1).blank(close + 1))
}
