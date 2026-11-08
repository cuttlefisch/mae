//! `org-element-link-parser`: bracket, angle and plain links (radio links
//! are tried first, in `objects`). The regexps are `org-link-make-regexps`'.

use super::{radio, InlineCx, Obj};
use crate::chars::is_word;
use crate::kind::SyntaxKind::*;

/// A registered link type followed by `:` at `i` (case-insensitive, as
/// `case-fold-search` is on): the offset just after the colon.
pub(crate) fn type_at(cx: &InlineCx, i: usize, end: usize) -> Option<usize> {
    let r = cx.src.text.get(i..end)?.as_bytes();
    cx.link_types.iter().find_map(|t| {
        let n = t.len();
        (r.len() > n && r[..n].eq_ignore_ascii_case(t.as_bytes()) && r[n] == b':')
            .then_some(i + n + 1)
    })
}

pub(crate) fn link(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    if let Some(o) = radio::link(cx, i, end, floor) {
        return Some(o);
    }
    let b = cx.bytes();
    if b[i] == b'[' && b.get(i + 1) == Some(&b'[') {
        return bracket(cx, i, end);
    }
    if let Some(o) = plain(cx, i, end) {
        return Some(o);
    }
    if b[i] == b'<' {
        return angle(cx, i, end);
    }
    None
}

/// `[[PATH]]` or `[[PATH][DESCRIPTION]]`. PATH excludes unescaped brackets;
/// DESCRIPTION is the shortest text (at least one char) before `]]`.
fn bracket(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let b = cx.bytes();
    let path_end = scan_bracket_path(cx, i + 2, end);
    if path_end == i + 2 || b.get(path_end) != Some(&b']') || path_end >= end {
        return None;
    }
    match b.get(path_end + 1).filter(|_| path_end + 1 < end) {
        Some(b']') => {
            let value_end = path_end + 2;
            let obj_end = cx.skip_blanks(value_end, end);
            Some(
                Obj::new(LINK, i, obj_end)
                    .tok(MARKUP, i, i + 2)
                    .tok(LINK_PATH, i + 2, path_end)
                    .tok(MARKUP, path_end, value_end)
                    .blank(value_end),
            )
        }
        Some(b'[') => {
            let desc = path_end + 2;
            let close = cx.ix.double_close(desc + 1, end)?;
            let value_end = close + 2;
            let obj_end = cx.skip_blanks(value_end, end);
            Some(
                Obj::new(LINK, i, obj_end)
                    .tok(MARKUP, i, i + 2)
                    .tok(LINK_PATH, i + 2, path_end)
                    .tok(MARKUP, path_end, desc)
                    .contents(desc, close)
                    .tok(MARKUP, close, value_end)
                    .blank(value_end),
            )
        }
        _ => None,
    }
}

/// `(one-or-more (or (not (any "[]\\")) (and "\\" (zero-or-more "\\\\")
/// (any "[]")) (and (one-or-more "\\") (not (any "[]")))))`: where the path
/// stops. A run of backslashes escapes a bracket only if it is odd.
fn scan_bracket_path(cx: &InlineCx, from: usize, end: usize) -> usize {
    let b = cx.bytes();
    let mut i = from;
    while i < end {
        match b[i] {
            b'[' | b']' => break,
            b'\\' => {
                let k = b[i..end].iter().take_while(|&&c| c == b'\\').count();
                let j = i + k;
                match b.get(j).filter(|_| j < end) {
                    Some(b'[' | b']') if k % 2 == 1 => i = j + 1,
                    Some(b'[' | b']') => {
                        i = j;
                        break;
                    }
                    Some(_) => i = j + cx.char_len(j),
                    // `\+[^][]` needs a char after the run; pairs still match.
                    None => {
                        i += k - k % 2;
                        break;
                    }
                }
            }
            _ => i += cx.char_len(i),
        }
    }
    i
}

/// `<TYPE:PATH>`: PATH has no `>`; a line break inside it must be followed
/// (after blanks) by a char that is not `>`, a blank or a newline.
fn angle(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let b = cx.bytes();
    let mut j = type_at(cx, i + 1, end)?;
    loop {
        while j < end && b[j] != b'>' && cx.eol_len(j) == 0 {
            j += 1;
        }
        if j >= end {
            return None;
        }
        if b[j] == b'>' {
            break;
        }
        j = cx.skip_blanks(j + cx.eol_len(j), end);
        if j >= end || matches!(b[j], b'>' | b' ' | b'\t') || cx.eol_len(j) > 0 {
            return None;
        }
    }
    let value_end = j + 1;
    let obj_end = cx.skip_blanks(value_end, end);
    Some(
        Obj::new(LINK, i, obj_end)
            .tok(MARKUP, i, i + 1)
            .tok(LINK_PATH, i + 1, j)
            .tok(MARKUP, j, value_end)
            .blank(value_end),
    )
}

/// `\<TYPE:` then the path: units (a char outside `[] \t\n()<>`, or a
/// parenthesised group nested at most twice), at least two, the last of which
/// may end a path (a char that is not punctuation, a space, a tab or a line
/// ending; or `/`; or a group).
fn plain(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let start = type_at(cx, i, end)?;
    let mut j = start;
    let (mut count, mut last_ok) = (0usize, None);
    while let Some((next, can_end)) = unit(cx, j, end) {
        count += 1;
        if can_end && count >= 2 {
            last_ok = Some(next);
        }
        j = next;
    }
    let value_end = last_ok?;
    let obj_end = cx.skip_blanks(value_end, end);
    Some(
        Obj::new(LINK, i, obj_end)
            .tok(LINK_PATH, i, value_end)
            .blank(value_end),
    )
}

fn is_nsb(c: char) -> bool {
    !matches!(c, '[' | ']' | ' ' | '\t' | '\n' | '(' | ')' | '<' | '>')
}

/// Emacs `[:punct:]`: ASCII punctuation, and any non-ASCII char without word syntax.
fn is_punct(c: char) -> bool {
    if c.is_ascii() {
        c.is_ascii_punctuation()
    } else {
        !is_word(c)
    }
}

/// One path unit at `j`: where it ends, and whether it may end the path.
fn unit(cx: &InlineCx, j: usize, end: usize) -> Option<(usize, bool)> {
    if j >= end {
        return None;
    }
    let c = cx.ch(j, end)?;
    if is_nsb(c) {
        // `[^[:punct:] \t\n]` or `/`: `-` is punctuation, so it cannot end a path.
        let can_end = (!is_punct(c) && !matches!(c, ' ' | '\t' | '\n')) || c == '/';
        return Some((j + c.len_utf8(), can_end));
    }
    paren_group(cx, j, end).map(|e| (e, true))
}

/// `[<([] (0+ (or NSB ([<([] (0+ NSB) [])>]))) [])>]`.
fn paren_group(cx: &InlineCx, j: usize, end: usize) -> Option<usize> {
    let is_open = |c: char| matches!(c, '<' | '(' | '[');
    let is_close = |c: char| matches!(c, ']' | ')' | '>');
    if !is_open(cx.ch(j, end)?) {
        return None;
    }
    let mut k = j + 1;
    loop {
        let c = cx.ch(k, end)?;
        if is_close(c) {
            return Some(k + 1);
        }
        if is_nsb(c) {
            k += c.len_utf8();
        } else if is_open(c) {
            k += 1;
            while let Some(d) = cx.ch(k, end).filter(|&d| is_nsb(d)) {
                k += d.len_utf8();
            }
            if !cx.ch(k, end).is_some_and(is_close) {
                return None;
            }
            k += 1;
        } else {
            return None;
        }
    }
}
