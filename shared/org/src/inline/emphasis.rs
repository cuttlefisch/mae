//! `org-element--parse-generic-emphasis`: bold, italic, underline,
//! strike-through, verbatim and code.

use super::{InlineCx, Obj};
use crate::chars::is_space;
use crate::kind::SyntaxKind::{self, *};

fn mark_of(kind: SyntaxKind) -> u8 {
    match kind {
        BOLD => b'*',
        ITALIC => b'/',
        UNDERLINE => b'_',
        STRIKE_THROUGH => b'+',
        VERBATIM => b'=',
        _ => b'~',
    }
}

/// Emphasis opening at `origin`: preceded by a line start or one of
/// `space - ( ' " {`, followed by a non-space; closed by the FIRST
/// `(not space) MARK (post | line-end)` from `origin + 1`, searched within the
/// region. The object includes the blanks after its closing mark.
pub(crate) fn emphasis(
    cx: &InlineCx,
    origin: usize,
    end: usize,
    floor: usize,
    kind: SyntaxKind,
) -> Option<Obj> {
    let mark = mark_of(kind);
    let pre_ok = cx.at_bol(origin, floor)
        || cx
            .char_before(origin, floor)
            .is_some_and(|c| is_space(c) || "-('\"{".contains(c));
    let after_ok = origin + 1 < end && cx.char_at(origin + 1).is_some_and(|c| !is_space(c));
    if !(pre_ok && after_ok) {
        return None;
    }
    let close = closer(cx, mark, origin + 2, end)?;
    let value_end = close + 1;
    let obj_end = cx.skip_blanks(value_end, end);
    Some(if matches!(kind, VERBATIM | CODE) {
        Obj::opaque(kind, origin, (origin + 1, close), value_end, obj_end)
    } else {
        Obj::delimited(kind, origin, (origin + 1, close), value_end, obj_end)
    })
}

/// The first closing `mark` at or after `from` within `[.., end)`: an indexed
/// closer, or the region's last byte (where `line-end` matches the end of the
/// narrowed region whatever text follows it).
fn closer(cx: &InlineCx, mark: u8, from: usize, end: usize) -> Option<usize> {
    let indexed = cx.ix.closer(mark, from, end);
    let last = end.checked_sub(1).filter(|&m| {
        m >= from && cx.bytes()[m] == mark && cx.char_before(m, 0).is_some_and(|c| !is_space(c))
    });
    match (indexed, last) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}
