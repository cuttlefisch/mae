//! Citations (`[cite/STYLE:PREFIX;@key SUFFIX;...;SUFFIX]`) and their
//! references (`org-element-citation-parser`,
//! `org-element-citation-reference-parser`).
//!
//! Org parses a citation's common prefix and suffix, and each reference's
//! own prefix and suffix, as objects whose parent is the citation (or the
//! reference) although they are not its contents; here they are child
//! regions of the same node, around the contents.

use super::index::{is_cite_key_char, Bracket, Needle};
use super::{InlineCx, Obj};
use crate::chars::is_alnum;
use crate::kind::SyntaxKind::*;

/// `org-element-citation-prefix-re`: `[cite`, an optional `/STYLE`
/// (`[/_-[:alnum:]]+`), `:` and blanks (`[\t\n ]*`). Returns (colon, start).
fn prefix(cx: &InlineCx, i: usize, end: usize) -> Option<(usize, usize)> {
    if !cx.starts_ci(i, end, "[cite") {
        return None;
    }
    let mut k = i + 5;
    if cx.bytes().get(k) == Some(&b'/') && k < end {
        let s = k + 1;
        let mut q = s;
        while let Some(c) = cx.ch(q, end).filter(|&c| "/_-".contains(c) || is_alnum(c)) {
            q += c.len_utf8();
        }
        if q == s {
            return None;
        }
        k = q;
    }
    if !cx.starts(k, end, ":") {
        return None;
    }
    let mut start = k + 1;
    while cx
        .ch(start, end)
        .is_some_and(|c| matches!(c, '\t' | '\n' | ' '))
    {
        start = cx.step(start);
    }
    Some((k, start))
}

/// `@KEY` at or after `from`, ending by `bound`: (where `@` is, key end).
fn key(cx: &InlineCx, from: usize, bound: usize) -> Option<(usize, usize)> {
    let at = cx.ix.next(Needle::CiteKey, from, bound)?;
    let mut q = at + 1;
    while let Some(c) = cx.ch(q, bound).filter(|&c| is_cite_key_char(c)) {
        q += c.len_utf8();
    }
    Some((at, q))
}

pub(crate) fn citation(cx: &InlineCx, i: usize, end: usize) -> Option<Obj> {
    let (colon, start) = prefix(cx, i, end)?;
    let close = cx.ix.pair(Bracket::Square, i, end)?.close;
    let (_, first_key_end) = key(cx, start, close)?;
    let semi = cx.ix.prev(Needle::Semicolon, start, first_key_end);
    let cb = semi.map_or(start, |s| s + 1);
    // `skip-chars-backward " \r\t\n"` from the closing bracket.
    let b = cx.bytes();
    let mut last = close;
    while last > first_key_end && matches!(b[last - 1], b' ' | b'\r' | b'\t' | b'\n') {
        last -= 1;
    }
    let suffix = cx
        .ix
        .prev(Needle::Semicolon, first_key_end, last)
        .filter(|&s| key(cx, s, last).is_none());
    let ce = suffix.map_or(last, |s| s + 1);
    let obj_end = cx.skip_blanks(close + 1, end);
    let mut o = Obj::new(CITATION, i, obj_end)
        .tok(MARKUP, i, colon + 1)
        .tok(WHITESPACE, colon + 1, start);
    if let Some(s) = semi {
        o = o
            .objects(start, s, CITATION_REFERENCE)
            .tok(MARKUP, s, s + 1);
    }
    Some(
        o.contents(cb, ce)
            .objects(ce, last, CITATION_REFERENCE)
            .tok(WHITESPACE, last, close)
            .tok(MARKUP, close, close + 1)
            .blank(close + 1),
    )
}

/// A reference runs from `from` (its prefix) through `@KEY` and its suffix to
/// the next `;` (included) or the end of the citation's contents.
pub(crate) fn citation_reference(cx: &InlineCx, from: usize, end: usize) -> Option<Obj> {
    let (at, key_end) = key(cx, from, end)?;
    let sep = cx.ix.next(Needle::Semicolon, key_end, end);
    let (suffix_end, ref_end) = sep.map_or((end, end), |s| (s, s + 1));
    Some(
        Obj::new(CITATION_REFERENCE, from, ref_end)
            .objects(from, at, CITATION_REFERENCE)
            .tok(MARKUP, at, at + 1)
            .tok(NAME, at + 1, key_end)
            .objects(key_end, suffix_end, CITATION_REFERENCE)
            .tok(MARKUP, suffix_end, ref_end),
    )
}
