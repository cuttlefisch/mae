//! Entities (`\alpha`, `\_ `) and LaTeX fragments (`\(..\)`, `\[..\]`,
//! `$$..$$`, `$..$`, `\macro[opt]{arg}`).

use super::index::Needle;
use super::{InlineCx, Obj};
use crate::chars::{is_alpha, is_delim, is_space};
use crate::entities::ENTITY_NAMES;
use crate::kind::SyntaxKind::*;

fn is_entity(name: &str) -> bool {
    ENTITY_NAMES.binary_search(&name).is_ok()
}

/// `org-element-entity-parser`: `\` then `_` and spaces, or a name
/// (`there4`, `sup[123]`, `frac[13][24]` or ASCII letters, tried in that
/// order, ignoring case) followed by a line end, `{}` or a non-letter. The
/// name must be one `org-entity-get` knows (case-sensitively); the regexp is
/// not retried with another alternative when it is not.
pub(crate) fn entity(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let b = cx.bytes();
    let name = i + 1;
    let (name_end, brackets) = if cx.starts(name, end, "_ ") {
        let e = name + 1 + b[name + 1..end].iter().take_while(|&&c| c == b' ').count();
        (e, false)
    } else {
        entity_name(cx, name, end)?
    };
    if !is_entity(&cx.src.text[name..name_end]) {
        return None;
    }
    let value_end = name_end + if brackets { 2 } else { 0 };
    let obj_end = cx.skip_blanks(value_end, end);
    Some(
        Obj::new(ENTITY, i, obj_end)
            .tok(MARKUP, i, name)
            .tok(NAME, name, name_end)
            .tok(MARKUP, name_end, value_end)
            .blank(value_end),
    )
}

/// The name alternatives of the entity regexp, each followed by group 2:
/// `(or eol "{}" (not letter))`. Returns the name's end and whether `{}`
/// followed.
fn entity_name(cx: &InlineCx, n: usize, end: usize) -> Option<(usize, bool)> {
    let b = cx.bytes();
    let at = |k: usize| b.get(k).copied().filter(|_| k < end);
    let in_set = |k: usize, set: &[u8]| at(k).is_some_and(|c| set.contains(&c));
    let mut tries = Vec::with_capacity(4);
    if cx.starts_ci(n, end, "there4") {
        tries.push(n + 6);
    }
    if cx.starts_ci(n, end, "sup") && in_set(n + 3, b"123") {
        tries.push(n + 4);
    }
    if cx.starts_ci(n, end, "frac") && in_set(n + 4, b"13") && in_set(n + 5, b"24") {
        tries.push(n + 6);
    }
    let letters = b[n..end]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    if letters > 0 {
        tries.push(n + letters);
    }
    tries.into_iter().find_map(|e| {
        if cx.at_eol(e, end) {
            Some((e, false))
        } else if cx.starts(e, end, "{}") {
            Some((e, true))
        } else {
            cx.ch(e, end).filter(|&c| !is_alpha(c)).map(|_| (e, false))
        }
    })
}

/// `org-element-latex-fragment-parser`.
pub(crate) fn latex_fragment(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    let after = fragment_end(cx, i, end, floor)?;
    let obj_end = cx.skip_blanks(after, end);
    Some(
        Obj::new(LATEX_FRAGMENT, i, obj_end)
            .tok(VALUE, i, after)
            .blank(after),
    )
}

fn fragment_end(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<usize> {
    let b = cx.bytes();
    let second = b.get(i + 1).copied().filter(|_| i + 1 < end);
    if b[i] != b'$' {
        return match second {
            Some(b'(') => cx.ix.next(Needle::CloseParen, i, end).map(|p| p + 2),
            Some(b'[') => cx.ix.next(Needle::CloseBracket, i, end).map(|p| p + 2),
            _ => latex_macro(cx, i, end),
        };
    }
    if second == Some(b'$') {
        return cx.ix.next(Needle::Dollars, i + 2, end).map(|p| p + 2);
    }
    if cx.ch_before(i, floor) == Some('$')
        || cx
            .ch(i + 1, end)
            .is_some_and(|c| matches!(c, ' ' | '\t' | '\n' | ',' | '.' | ';'))
    {
        return None;
    }
    let p = cx.ix.next(Needle::Dollar, i + 1, end)?;
    if cx
        .ch_before(p, floor)
        .is_some_and(|c| matches!(c, ' ' | '\t' | '\n' | ',' | '.'))
    {
        return None;
    }
    let ok = cx.at_eol(p + 1, end)
        || cx
            .ch(p + 1, end)
            .is_some_and(|c| is_delim(c) || is_space(c) || c == '\'');
    ok.then_some(p + 1)
}

/// `\\[a-zA-Z]+\*?\(\(\[[^][\n{}]*\]\)\|\({[^{}\n]*}\)\)*`: where it ends.
fn latex_macro(cx: &InlineCx, i: usize, end: usize) -> Option<usize> {
    let b = cx.bytes();
    let letters = b[i + 1..end]
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    if letters == 0 {
        return None;
    }
    let mut k = i + 1 + letters;
    if k < end && b[k] == b'*' {
        k += 1;
    }
    loop {
        let (close, forbidden): (u8, &[u8]) = match b.get(k).filter(|_| k < end) {
            Some(b'[') => (b']', b"[]{}"),
            Some(b'{') => (b'}', b"{}"),
            _ => return Some(k),
        };
        let mut q = k + 1;
        while q < end && !forbidden.contains(&b[q]) && cx.eol_len(q) == 0 {
            q += 1;
        }
        if q < end && b[q] == close {
            k = q + 1;
        } else {
            return Some(k);
        }
    }
}
