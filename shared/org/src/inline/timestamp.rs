//! `org-element-timestamp-parser`: only the extent, which is all the tree
//! needs (the typed fields are a view's job).
//!
//! A timestamp exists where `org-element--timestamp-regexp` matches:
//! - `[[<]DATE\(?: .*?\)?[]>]` (`org-ts-regexp-both`; DATE is
//!   `[0-9]{4}-[0-9]{2}-[0-9]{2}`, and the brackets need not agree),
//! - `<[0-9]+-[0-9]+-[0-9]+[^>\n]+?\+[0-9]+[dwmy]>`, or
//! - `<%%\(?:([^>\n]+)\)\([^\n>]*\)>` (a diary sexp).
//!
//! Its extent is then the raw-value regexp's, `[<[]\(%%\)?.*?[]>]` — so it
//! ends at the FIRST `]` or `>` on the line — optionally followed by
//! `--` and a second `org-ts-regexp-both` timestamp (a range).

use super::index::Needle;
use super::{InlineCx, Obj};
use crate::kind::SyntaxKind::*;

pub(crate) fn timestamp(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let valid = both(cx, i, end).is_some()
        || (cx.bytes()[i] == b'<' && (repeater(cx, i, end) || diary(cx, i, end)));
    if !valid {
        return None;
    }
    let first = close_on_line(cx, Needle::TsClose, i + 1, end)? + 1;
    let value_end = if cx.starts(first, end, "--") {
        both(cx, first + 2, end).unwrap_or(first)
    } else {
        first
    };
    let obj_end = cx.skip_blanks(value_end, end);
    Some(
        Obj::new(TIMESTAMP, i, obj_end)
            .tok(VALUE, i, value_end)
            .blank(value_end),
    )
}

/// The first `needle` at or after `from` on the line of `from`.
fn close_on_line(cx: &InlineCx, needle: Needle, from: usize, end: usize) -> Option<usize> {
    let le = cx.line_end(from, end);
    cx.ix.next(needle, from, le)
}

/// `org-ts-regexp-both` at `i`: where the match ends.
fn both(cx: &InlineCx, i: usize, end: usize) -> Option<usize> {
    let b = cx.bytes();
    if !matches!(b.get(i), Some(b'<' | b'[')) {
        return None;
    }
    let d = date(cx, i + 1, end)?;
    match b.get(d).filter(|_| d < end)? {
        b']' | b'>' => Some(d + 1),
        b' ' => close_on_line(cx, Needle::TsClose, d + 1, end).map(|p| p + 1),
        _ => None,
    }
}

/// `[0-9]{4}-[0-9]{2}-[0-9]{2}` at `k`: where it ends.
fn date(cx: &InlineCx, k: usize, end: usize) -> Option<usize> {
    let b = cx.bytes().get(k..(k + 10).min(end))?;
    let shape = b"dddd-dd-dd";
    (b.len() == 10
        && b.iter().zip(shape).all(|(c, s)| match s {
            b'd' => c.is_ascii_digit(),
            _ => c == s,
        }))
    .then_some(k + 10)
}

/// `[0-9]+` at `k`: where the run ends, if it is not empty.
fn digits(cx: &InlineCx, k: usize, end: usize) -> Option<usize> {
    let n = cx.bytes()[k.min(end)..end]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count();
    (n > 0).then_some(k + n)
}

/// `<[0-9]+-[0-9]+-[0-9]+[^>\n]+?\+[0-9]+[dwmy]>` at `i` (`[dwmy]` ignoring
/// case). The lazy middle cannot cross a `>`, so the match ends at the first
/// `>` on the line, which must be preceded by `+DIGITS[dwmy]` with at least
/// one char between the third number's first digit and that `+`.
fn repeater(cx: &InlineCx, i: usize, end: usize) -> bool {
    let b = cx.bytes();
    let dash = |k: Option<usize>| k.filter(|&k| k < end && b[k] == b'-').map(|k| k + 1);
    let Some(third) = dash(digits(cx, i + 1, end)).and_then(|k| dash(digits(cx, k, end))) else {
        return false;
    };
    if digits(cx, third, end).is_none() {
        return false;
    }
    let Some(gt) = close_on_line(cx, Needle::Gt, third + 1, end) else {
        return false;
    };
    if !b[gt - 1].eq_ignore_ascii_case(&b'd')
        && !matches!(b[gt - 1].to_ascii_lowercase(), b'w' | b'm' | b'y')
    {
        return false;
    }
    let run = b[third..gt - 1]
        .iter()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .count();
    // The digits start at `first`; the `+` before them must leave the third
    // number one digit and the lazy middle one char: `+` at `third + 2` or later.
    let first = gt - 1 - run;
    run > 0 && first >= third + 3 && b[first - 1] == b'+'
}

/// `<%%\(?:([^>\n]+)\)\([^\n>]*\)>`: the first `>` on the line closes it,
/// and a `)` must come at least two chars after the `(`.
fn diary(cx: &InlineCx, i: usize, end: usize) -> bool {
    if !cx.starts(i, end, "<%%(") {
        return false;
    }
    close_on_line(cx, Needle::Gt, i + 4, end)
        .is_some_and(|gt| cx.ix.prev(Needle::RParen, i + 5, gt).is_some())
}
