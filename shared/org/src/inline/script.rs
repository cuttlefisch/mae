//! Subscripts and superscripts: `org-match-substring-regexp`, which Org
//! matches from the character BEFORE the `_`/`^` (it must not be whitespace).
//!
//! `\(\S-\)\([_^]\)\(` then one of: `{…}` with braces nested at most
//! `org-match-sexp-depth` (3) deep, contents inside the braces; `(…)` the same
//! with parentheses, contents INCLUDING them; `*`; or
//! `[+-]?[[:alnum:].,\\]*[[:alnum:]]` (greedy, ending at an alnum).

use super::index::Bracket;
use super::{InlineCx, Obj};
use crate::chars::{is_alnum, is_space};
use crate::kind::SyntaxKind::{self, *};

/// `org-match-sexp-depth`.
const SEXP_DEPTH: u32 = 3;

/// A subscript: never at the start of a line.
pub(crate) fn subscript(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    if cx.at_bol(i, floor) {
        return None;
    }
    let before = i - cx.char_before(i, floor).map_or(1, char::len_utf8);
    script(cx, before, end, SUBSCRIPT)
}

/// A superscript: at the start of a line Org matches from the `^` itself, so
/// `^^x` there is a superscript beginning at the second `^`.
pub(crate) fn superscript(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    let s = if cx.at_bol(i, floor) {
        i
    } else {
        i - cx.char_before(i, floor).map_or(1, char::len_utf8)
    };
    script(cx, s, end, SUPERSCRIPT)
}

/// The regexp at `s`; the object is created with `kind` whatever the mark.
fn script(cx: &InlineCx, s: usize, end: usize, kind: SyntaxKind) -> Option<Obj> {
    let c = cx.ch(s, end).filter(|&c| !is_space(c))?;
    let mark = s + c.len_utf8();
    if !matches!(cx.bytes().get(mark), Some(b'_' | b'^')) || mark >= end {
        return None;
    }
    let g = mark + 1;
    let (contents, value_end, braces) = body(cx, g, end)?;
    let obj_end = cx.skip_blanks(value_end, end);
    let open_end = if braces { g + 1 } else { g };
    Some(
        Obj::new(kind, mark, obj_end)
            .tok(MARKUP, mark, open_end)
            .contents(contents.0, contents.1)
            .tok(MARKUP, contents.1, value_end)
            .blank(value_end),
    )
}

/// Group 3 at `g`: (contents, match end, whether it is the braces form).
fn body(cx: &InlineCx, g: usize, end: usize) -> Option<((usize, usize), usize, bool)> {
    let b = cx.bytes();
    let nested = |kind: Bracket| {
        cx.ix
            .pair(kind, g, end)
            .filter(|p| p.depth <= SEXP_DEPTH)
            .map(|p| p.close)
    };
    match b.get(g).filter(|_| g < end)? {
        b'{' => return nested(Bracket::Curly).map(|c| ((g + 1, c), c + 1, true)),
        b'(' => return nested(Bracket::Round).map(|c| ((g, c + 1), c + 1, false)),
        b'*' => return Some(((g, g + 1), g + 1, false)),
        _ => {}
    }
    let k = if matches!(b[g], b'+' | b'-') {
        g + 1
    } else {
        g
    };
    let mut q = k;
    let mut last_alnum = None;
    while let Some(c) = cx.ch(q, end) {
        if is_alnum(c) {
            last_alnum = Some(q + c.len_utf8());
        } else if !matches!(c, '.' | ',' | '\\') {
            break;
        }
        q += c.len_utf8();
    }
    last_alnum.map(|e| ((g, e), e, false))
}
