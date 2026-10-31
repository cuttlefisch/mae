//! `org-element--object-lex`: where the next object starts, and which parsers
//! are tried there, in Org's order.
//!
//! Org searches forward for `org-element--object-regexp` (the positions that
//! MAY start an object), tries the parsers for that position, and on failure
//! moves one character on. When the document has radio targets and links are
//! allowed, the search is bounded just past the next radio link, which is
//! parsed if no other object starts before it.

use super::{babel, cite, emphasis, latex, link, objects, script, timestamp};
use super::{Frame, InlineCx, Obj, Restriction};
use crate::chars::{is_alnum, is_space, word_start};
use crate::kind::SyntaxKind::{self, *};

/// The first object in the frame's region at or after its cursor.
pub(super) fn next_object(cx: &InlineCx, f: &mut Frame) -> Option<Obj> {
    let (from, end, floor, r) = (f.cursor, f.end, f.floor, f.restriction);
    if r.allows(TABLE_CELL) {
        return objects::table_cell(cx, from, end);
    }
    if r.allows(CITATION_REFERENCE) {
        return cite::citation_reference(cx, from, end);
    }
    // The next radio link's start: candidates may not extend past the char
    // after it (Org's bound is one char past the link's text).
    let radio = match cx.radio {
        Some(radio) if r.allows(LINK) => radio.next_link(cx, from, end, floor, &mut f.radio),
        _ => None,
    };
    let lim = radio.map_or(end, |p| cx.step(p));
    let mut i = from;
    while i < lim {
        if is_candidate(cx, i, end, lim, floor) {
            if let Some(o) = dispatch(cx, i, end, floor, r) {
                return Some(o);
            }
        }
        i += cx.char_at(i).map_or(1, char::len_utf8);
    }
    // No object before the radio link: parse it (Org calls the link parser
    // one character before the bound).
    link::link(cx, radio?, end, floor)
}

/// The char at `i` if it is before `lim` (a match may not extend past it;
/// `lim` is a char boundary).
fn fit(cx: &InlineCx, i: usize, lim: usize) -> Option<char> {
    cx.ch(i, lim)
}

/// Does `org-element--object-regexp` match at `i` without consuming past
/// `lim`? (`$` and `\<` still look at the region up to `end`.)
fn is_candidate(cx: &InlineCx, i: usize, end: usize, lim: usize, floor: usize) -> bool {
    let b = cx.bytes();
    let second = || fit(cx, i + 1, lim);
    match b[i] {
        b'_' | b'^' if second().is_some_and(|c| "-{(*+.,".contains(c) || is_alnum(c)) => true,
        b'*' | b'~' | b'=' | b'+' | b'_' | b'/' if second().is_some_and(|c| !is_space(c)) => true,
        b'[' => bracket_candidate(cx, i, lim),
        b'@' => cx.starts(i, lim, "@@"),
        b'{' => cx.starts(i, lim, "{{{"),
        b'<' => {
            cx.starts(i, lim, "<%%")
                || matches!(second(), Some('<' | '0'..='9'))
                || type_name_at(cx, i + 1, lim).is_some()
        }
        b'$' => true,
        b'\\' => backslash_candidate(cx, i, end, lim),
        _ => plain_link_candidate(cx, i, lim, floor) || babel_candidate(cx, i, lim),
    }
}

/// `\[\(?:cite[:/]\|fn:\|[0-9]\|%\]\|/[0-9]*\]\|\[\)`.
fn bracket_candidate(cx: &InlineCx, i: usize, lim: usize) -> bool {
    let b = cx.bytes();
    let j = i + 1;
    if cx.starts_ci(j, lim, "cite:") || cx.starts_ci(j, lim, "cite/") || cx.starts_ci(j, lim, "fn:")
    {
        return true;
    }
    match b.get(j).filter(|_| j < lim) {
        Some(b'0'..=b'9' | b'[') => true,
        Some(b'%') => cx.starts(j, lim, "%]"),
        Some(b'/') => {
            let k = j
                + 1
                + b[j + 1..lim]
                    .iter()
                    .take_while(|c| c.is_ascii_digit())
                    .count();
            k < lim && b[k] == b']'
        }
        _ => false,
    }
}

/// `\\\(?:[a-zA-Z[(]\|\\[ \t]*$\|_ +\)`.
fn backslash_candidate(cx: &InlineCx, i: usize, end: usize, lim: usize) -> bool {
    let b = cx.bytes();
    let Some(&c) = b.get(i + 1).filter(|_| i + 1 < lim) else {
        return false;
    };
    match c {
        b'a'..=b'z' | b'A'..=b'Z' | b'[' | b'(' => true,
        // `$` may look past the bound; the blanks may not.
        b'\\' => {
            let q = cx.skip_blanks(i + 2, end);
            q <= lim && cx.at_eol(q, end)
        }
        b'_' => b.get(i + 2) == Some(&b' ') && i + 3 <= lim,
        _ => false,
    }
}

/// `\(?:call\|src\)_`, ignoring case.
fn babel_candidate(cx: &InlineCx, i: usize, lim: usize) -> bool {
    cx.starts_ci(i, lim, "call_") || cx.starts_ci(i, lim, "src_")
}

/// A registered link type name at `i` (no colon required: `<\(?:TYPES\)`),
/// ignoring case: where it ends.
fn type_name_at(cx: &InlineCx, i: usize, lim: usize) -> Option<usize> {
    cx.link_types
        .iter()
        .find(|t| cx.starts_ci(i, lim, t))
        .map(|t| i + t.len())
}

/// `\<TYPE:` — a word start followed by a registered link type and a colon.
/// `\<` is Emacs's, so a word char after one of another script starts a
/// word (`参见https:`); see [`word_start`].
fn plain_link_candidate(cx: &InlineCx, i: usize, lim: usize, floor: usize) -> bool {
    cx.char_at(i)
        .is_some_and(|c| word_start(cx.ch_before(i, floor), c))
        && link::type_at(cx, i, lim).is_some()
}

/// The parsers tried at a candidate, in `org-element--object-lex`'s order.
fn dispatch(cx: &InlineCx, i: usize, end: usize, floor: usize, r: Restriction) -> Option<Obj> {
    let try_ = |k: SyntaxKind, f: &dyn Fn() -> Option<Obj>| if r.allows(k) { f() } else { None };
    if cx.starts_ci(i, end, "call_") {
        return try_(INLINE_BABEL_CALL, &|| {
            babel::inline_babel_call(cx, i, end, floor)
        });
    }
    if cx.starts_ci(i, end, "src_") {
        return try_(INLINE_SRC_BLOCK, &|| {
            babel::inline_src_block(cx, i, end, floor)
        });
    }
    let b = cx.bytes();
    let second = b.get(i + 1).copied().filter(|_| i + 1 < end);
    let emph = |k: SyntaxKind| try_(k, &|| emphasis::emphasis(cx, i, end, floor, k));
    match b[i] {
        b'^' => try_(SUPERSCRIPT, &|| script::superscript(cx, i, end, floor)),
        b'_' => {
            emph(UNDERLINE).or_else(|| try_(SUBSCRIPT, &|| script::subscript(cx, i, end, floor)))
        }
        b'*' => emph(BOLD),
        b'/' => emph(ITALIC),
        b'~' => emph(CODE),
        b'=' => emph(VERBATIM),
        b'+' => emph(STRIKE_THROUGH),
        b'@' => try_(EXPORT_SNIPPET, &|| objects::export_snippet(cx, i, end)),
        b'{' => try_(MACRO, &|| objects::macro_(cx, i, end)),
        b'$' => try_(LATEX_FRAGMENT, &|| latex::latex_fragment(cx, i, end, floor)),
        b'<' if second == Some(b'<') => try_(RADIO_TARGET, &|| objects::radio_target(cx, i, end))
            .or_else(|| try_(TARGET, &|| objects::target(cx, i, end))),
        b'<' => try_(TIMESTAMP, &|| timestamp::timestamp(cx, i, end))
            .or_else(|| try_(LINK, &|| link::link(cx, i, end, floor))),
        b'\\' if second == Some(b'\\') => {
            try_(LINE_BREAK, &|| objects::line_break(cx, i, end, floor))
        }
        b'\\' => try_(ENTITY, &|| latex::entity(cx, i, end))
            .or_else(|| try_(LATEX_FRAGMENT, &|| latex::latex_fragment(cx, i, end, floor))),
        b'[' => bracket_dispatch(cx, i, end, floor, r, second),
        _ => try_(LINK, &|| link::link(cx, i, end, floor)),
    }
}

/// `pcase` on the char after `[`: each branch is guarded by the
/// restriction, and a failed guard falls through to the default branch.
fn bracket_dispatch(
    cx: &InlineCx,
    i: usize,
    end: usize,
    floor: usize,
    r: Restriction,
    second: Option<u8>,
) -> Option<Obj> {
    match second {
        Some(b'[') if r.allows(LINK) => link::link(cx, i, end, floor),
        Some(b'f') if r.allows(FOOTNOTE_REFERENCE) => objects::footnote_reference(cx, i, end),
        Some(b'c') if r.allows(CITATION) => cite::citation(cx, i, end),
        Some(b'%' | b'/') if r.allows(STATISTICS_COOKIE) => objects::statistics_cookie(cx, i, end),
        _ => {
            let ts = if r.allows(TIMESTAMP) {
                timestamp::timestamp(cx, i, end)
            } else {
                None
            };
            ts.or_else(|| {
                if r.allows(STATISTICS_COOKIE) {
                    objects::statistics_cookie(cx, i, end)
                } else {
                    None
                }
            })
        }
    }
}
