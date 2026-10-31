//! Inline babel calls (`call_NAME[HEADER](ARGS)[HEADER]`) and inline src
//! blocks (`src_LANG[PARAMS]{BODY}`). Brackets pair as `scan-lists` pairs
//! them (`org-element--parse-paired-brackets`): any depth, newlines allowed,
//! within the region.

use super::index::{Bracket, Needle};
use super::{InlineCx, Obj};
use crate::chars::is_word;
use crate::kind::SyntaxKind::{self, *};

/// `\<PREFIX\([^ \t\n[OPEN]+\)[OPEN[]`, case-sensitive: where the name ends.
fn head(cx: &InlineCx, i: usize, at: (usize, usize), prefix: &str, stop: Needle) -> Option<usize> {
    let (floor, end) = at;
    if !cx.starts(i, end, prefix) || cx.ch_before(i, floor).is_some_and(is_word) {
        return None;
    }
    let name = i + prefix.len();
    let n = cx.ix.next(stop, name, end).unwrap_or(end);
    (n > name && n < end && matches!(cx.bytes()[n], b'[' | b'(' | b'{')).then_some(n)
}

/// The balanced bracket at `k` of `kind`, if `k` holds its opener: the
/// position after its closer.
fn paired(cx: &InlineCx, k: usize, end: usize, kind: Bracket, open: u8) -> Option<usize> {
    (cx.bytes().get(k) == Some(&open) && k < end)
        .then(|| cx.ix.pair(kind, k, end))
        .flatten()
        .map(|p| p.close + 1)
}

/// Appends `[a, b)` as an opening bracket, `inner` and a closing bracket.
fn bracketed(o: Obj, a: usize, b: usize, inner: SyntaxKind) -> Obj {
    o.tok(MARKUP, a, a + 1)
        .tok(inner, a + 1, b - 1)
        .tok(MARKUP, b - 1, b)
}

/// `call_NAME[INSIDE-HEADER](ARGUMENTS)[END-HEADER]`: the arguments are
/// mandatory, the headers optional.
pub(crate) fn inline_babel_call(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    let n = head(cx, i, (floor, end), "call_", Needle::CallStop)?;
    let inside = paired(cx, n, end, Bracket::Square, b'[');
    let args_at = inside.unwrap_or(n);
    let args = paired(cx, args_at, end, Bracket::Round, b'(')?;
    let end_header = paired(cx, args, end, Bracket::Square, b'[');
    let value_end = end_header.unwrap_or(args);
    let obj_end = cx.skip_blanks(value_end, end);
    let mut o = Obj::new(INLINE_BABEL_CALL, i, obj_end)
        .tok(MARKUP, i, i + 5)
        .tok(NAME, i + 5, n);
    if let Some(h) = inside {
        o = bracketed(o, n, h, PARAMETERS);
    }
    o = bracketed(o, args_at, args, VALUE);
    if let Some(h) = end_header {
        o = bracketed(o, args, h, PARAMETERS);
    }
    Some(o.blank(value_end))
}

/// `src_LANG[PARAMETERS]{BODY}`: the body is mandatory.
pub(crate) fn inline_src_block(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    let n = head(cx, i, (floor, end), "src_", Needle::SrcStop)?;
    let params = paired(cx, n, end, Bracket::Square, b'[');
    let body_at = params.unwrap_or(n);
    let body = paired(cx, body_at, end, Bracket::Curly, b'{')?;
    let obj_end = cx.skip_blanks(body, end);
    let mut o = Obj::new(INLINE_SRC_BLOCK, i, obj_end)
        .tok(MARKUP, i, i + 4)
        .tok(NAME, i + 4, n);
    if let Some(p) = params {
        o = bracketed(o, n, p, PARAMETERS);
    }
    Some(bracketed(o, body_at, body, VALUE).blank(body))
}
