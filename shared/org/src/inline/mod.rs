//! Inline objects: `org-element--parse-objects` and `org-element--object-lex`,
//! without recursion and without rescanning.
//!
//! Org narrows to a container, searches forward for the next position that
//! MAY start an object (`org-element--object-regexp`), tries the parsers for
//! that position in a fixed order, and on failure moves one character on.
//! An object's contents are parsed the same way, narrowed to them, with the
//! container's restriction set. Here the narrowing is an explicit stack of
//! frames, capped like element nesting, and every forward search a parser
//! makes is a lookup in [`InlineIndex`], so hostile text cannot make the
//! search quadratic the way it does in Emacs.

mod emphasis;
mod index;
mod link;
mod objects;

pub(crate) use index::InlineIndex;

use crate::chars::{is_space, is_word};
use crate::kind::SyntaxKind::{self, *};
use crate::span::{ContentsTable, Span};
use crate::text::Source;
use crate::tokens::Event;

/// A set of object kinds allowed in a container (`org-element-object-restrictions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Restriction(u32);

const OBJECTS: [SyntaxKind; 24] = [
    BOLD,
    CITATION,
    CITATION_REFERENCE,
    CODE,
    ENTITY,
    EXPORT_SNIPPET,
    FOOTNOTE_REFERENCE,
    INLINE_BABEL_CALL,
    INLINE_SRC_BLOCK,
    ITALIC,
    LINE_BREAK,
    LATEX_FRAGMENT,
    LINK,
    MACRO,
    RADIO_TARGET,
    STATISTICS_COOKIE,
    STRIKE_THROUGH,
    SUBSCRIPT,
    SUPERSCRIPT,
    TABLE_CELL,
    TARGET,
    TIMESTAMP,
    UNDERLINE,
    VERBATIM,
];

impl Restriction {
    fn of(kinds: &[SyntaxKind]) -> Restriction {
        Restriction(
            kinds
                .iter()
                .map(|&k| 1 << (k as u16 - 2000))
                .fold(0, |a, b| a | b),
        )
    }

    fn all() -> Restriction {
        Restriction::of(&OBJECTS)
    }

    fn without(self, kinds: &[SyntaxKind]) -> Restriction {
        Restriction(self.0 & !Restriction::of(kinds).0)
    }

    pub fn allows(self, kind: SyntaxKind) -> bool {
        (kind as u16) >= 2000 && self.0 & (1 << (kind as u16 - 2000)) != 0
    }

    /// The objects allowed inside an element or object of `kind`.
    pub fn for_container(kind: SyntaxKind) -> Restriction {
        let minimal = Restriction::of(&[
            BOLD,
            CODE,
            ENTITY,
            ITALIC,
            LATEX_FRAGMENT,
            STRIKE_THROUGH,
            SUBSCRIPT,
            SUPERSCRIPT,
            UNDERLINE,
            VERBATIM,
        ]);
        let standard = Restriction::all().without(&[CITATION_REFERENCE, TABLE_CELL]);
        let no_break = standard.without(&[LINE_BREAK]);
        match kind {
            HEADLINE_TITLE | ITEM_TAG_CONTENT => no_break,
            CITATION => Restriction::of(&[CITATION_REFERENCE]),
            CITATION_REFERENCE => {
                no_break.without(&[CITATION, CITATION_REFERENCE, FOOTNOTE_REFERENCE, LINK])
            }
            AFFILIATED_VALUE => standard.without(&[FOOTNOTE_REFERENCE]),
            LINK => Restriction(
                minimal.0
                    | Restriction::of(&[
                        EXPORT_SNIPPET,
                        INLINE_BABEL_CALL,
                        INLINE_SRC_BLOCK,
                        MACRO,
                        STATISTICS_COOKIE,
                    ])
                    .0,
            ),
            RADIO_TARGET => minimal,
            TABLE_CELL => Restriction(
                minimal.0
                    | Restriction::of(&[
                        CITATION,
                        EXPORT_SNIPPET,
                        FOOTNOTE_REFERENCE,
                        LINK,
                        MACRO,
                        RADIO_TARGET,
                        TARGET,
                        TIMESTAMP,
                    ])
                    .0,
            ),
            TABLE_ROW => Restriction::of(&[TABLE_CELL]),
            _ => standard,
        }
    }
}

/// An object found at a position: its extent and how to cut it into tokens.
/// `head` covers `[begin, contents.0)` (or `[begin, end)` when there are no
/// contents), `tail` covers `[contents.1, end)`; both list `(kind, start, end)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Obj {
    pub kind: SyntaxKind,
    pub begin: usize,
    pub end: usize,
    pub contents: Option<(usize, usize)>,
    pub head: Vec<(SyntaxKind, usize, usize)>,
    pub tail: Vec<(SyntaxKind, usize, usize)>,
}

impl Obj {
    /// An object whose text between `value` bounds is opaque, delimited by
    /// markup, followed by post-blank up to `end`.
    pub fn opaque(
        kind: SyntaxKind,
        begin: usize,
        value: (usize, usize),
        value_end: usize,
        end: usize,
    ) -> Obj {
        let mut head = vec![
            (MARKUP, begin, value.0),
            (TEXT, value.0, value.1),
            (MARKUP, value.1, value_end),
        ];
        head.push((WHITESPACE, value_end, end));
        head.retain(|t| t.1 < t.2);
        Obj {
            kind,
            begin,
            end,
            contents: None,
            head,
            tail: Vec::new(),
        }
    }

    /// An object with parsed contents between `open` and `close` markup.
    pub fn delimited(
        kind: SyntaxKind,
        begin: usize,
        contents: (usize, usize),
        value_end: usize,
        end: usize,
    ) -> Obj {
        let head = vec![(MARKUP, begin, contents.0)];
        let mut tail = vec![
            (MARKUP, contents.1, value_end),
            (WHITESPACE, value_end, end),
        ];
        tail.retain(|t| t.1 < t.2);
        Obj {
            kind,
            begin,
            end,
            contents: (contents.0 < contents.1).then_some(contents),
            head,
            tail,
        }
    }
}

/// Document-wide inputs to inline parsing.
pub(crate) struct InlineCx<'a> {
    pub src: &'a Source<'a>,
    pub ix: &'a InlineIndex,
    /// Registered link types (`org-link-types`), longest first.
    pub link_types: &'a [String],
}

impl InlineCx<'_> {
    pub fn bytes(&self) -> &[u8] {
        self.src.text.as_bytes()
    }

    pub fn char_at(&self, i: usize) -> Option<char> {
        self.src.text.get(i..).and_then(|s| s.chars().next())
    }

    /// The char before byte `i`, if `i > floor`.
    pub fn char_before(&self, i: usize, floor: usize) -> Option<char> {
        (i > floor)
            .then(|| self.src.text[..i].chars().next_back())
            .flatten()
    }

    /// `(bolp)` within a region starting at `floor`.
    pub fn at_bol(&self, i: usize, floor: usize) -> bool {
        i == floor
            || self.bytes().get(i.wrapping_sub(1)) == Some(&b'\n')
            || (self.src.eol == crate::text::LineEnding::Cr
                && self.bytes().get(i.wrapping_sub(1)) == Some(&b'\r'))
    }

    /// End of the line containing `i`, not past `end` (`line-end-position`).
    pub fn line_end(&self, i: usize, end: usize) -> usize {
        let l = self.src.lines.get(self.src.line_of(i));
        l.map_or(end, |l| l.end.max(i)).min(end)
    }

    /// `skip-chars-forward " \t"` from `i`, bounded by `end`.
    pub fn skip_blanks(&self, i: usize, end: usize) -> usize {
        let b = self.bytes();
        let mut q = i;
        while q < end && (b[q] == b' ' || b[q] == b'\t') {
            q += 1;
        }
        q
    }
}

struct Frame {
    end: usize,
    restriction: Restriction,
    cursor: usize,
    /// Tokens to emit after the contents, then a `Finish`; `None` for the root.
    tail: Option<Vec<(SyntaxKind, usize, usize)>>,
    /// Where the region starts (for `bolp`).
    floor: usize,
}

/// Parses `[beg, end)` as the contents of a `container`, appending events.
/// `budget` is how many more nesting levels may be opened.
pub(crate) fn parse(
    cx: &InlineCx,
    beg: usize,
    end: usize,
    container: SyntaxKind,
    budget: usize,
    out: &mut Vec<Event>,
    contents: &mut ContentsTable,
) {
    let root = Frame {
        end,
        restriction: Restriction::for_container(container),
        cursor: beg,
        tail: None,
        floor: beg,
    };
    let mut stack = vec![root];
    while let Some(top) = stack.last_mut() {
        let found = if top.cursor < top.end {
            next_object(cx, top.cursor, top.end, top.floor, top.restriction)
        } else {
            None
        };
        let Some(obj) = found else {
            text_run(cx.src, top.cursor, top.end, out);
            if let Some(f) = stack.pop() {
                close(f, out);
            }
            continue;
        };
        text_run(cx.src, top.cursor, obj.begin, out);
        top.cursor = obj.end;
        if let Some((a, b)) = obj.contents {
            contents.push(obj.kind, obj.begin, obj.end, Span::new(a, b));
        }
        out.push(Event::Start(obj.kind));
        tokens(cx.src, &obj.head, out);
        match obj.contents {
            Some((a, b)) if stack.len() <= budget => {
                let restriction = Restriction::for_container(obj.kind);
                stack.push(Frame {
                    end: b,
                    restriction,
                    cursor: a,
                    tail: Some(obj.tail),
                    floor: a,
                });
            }
            Some((a, b)) => {
                out.push(Event::Token(OPAQUE, a, b));
                tokens(cx.src, &obj.tail, out);
                out.push(Event::Finish);
            }
            None => {
                tokens(cx.src, &obj.tail, out);
                out.push(Event::Finish);
            }
        }
    }
}

fn close(f: Frame, out: &mut Vec<Event>) {
    if let Some(tail) = f.tail {
        let _ = f.end;
        out.extend(
            tail.iter()
                .filter(|t| t.1 < t.2)
                .map(|&(k, a, b)| Event::Token(k, a, b)),
        );
        out.push(Event::Finish);
    }
}

fn tokens(src: &Source, parts: &[(SyntaxKind, usize, usize)], out: &mut Vec<Event>) {
    for &(k, a, b) in parts.iter().filter(|t| t.1 < t.2) {
        if k == TEXT {
            text_run(src, a, b, out);
        } else {
            out.push(Event::Token(k, a, b));
        }
    }
}

/// Plain text `[a, b)`: `TEXT` per line, `NEWLINE` for line endings.
pub(crate) fn text_run(src: &Source, a: usize, b: usize, out: &mut Vec<Event>) {
    let mut pos = a;
    while pos < b {
        let Some(&l) = src.lines.get(src.line_of(pos)) else {
            break;
        };
        let content_end = l.end.clamp(pos, b);
        if pos < content_end {
            out.push(Event::Token(TEXT, pos, content_end));
        }
        let next = l.next.min(b).max(content_end);
        if content_end < next {
            out.push(Event::Token(NEWLINE, content_end, next));
        }
        if next <= pos {
            out.push(Event::Token(TEXT, pos, b));
            break;
        }
        pos = next;
    }
}

/// `org-element--object-lex`: the first object at or after `from`.
fn next_object(
    cx: &InlineCx,
    from: usize,
    end: usize,
    floor: usize,
    r: Restriction,
) -> Option<Obj> {
    if r.allows(TABLE_CELL) {
        return objects::table_cell(cx, from, end);
    }
    if r.allows(CITATION_REFERENCE) {
        return objects::citation_reference(cx, from, end);
    }
    let b = cx.bytes();
    let mut i = from;
    while i < end {
        if is_candidate(cx, i, end, floor) {
            if let Some(o) = dispatch(cx, i, end, floor, r) {
                return Some(o);
            }
        }
        i += utf8_len(b[i]);
    }
    None
}

fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

/// Does `org-element--object-regexp` match at `i`?
fn is_candidate(cx: &InlineCx, i: usize, end: usize, floor: usize) -> bool {
    let b = cx.bytes();
    let next = |k: usize| if i + k < end { Some(b[i + k]) } else { None };
    let next_char = || if i + 1 < end { cx.char_at(i + 1) } else { None };
    match b[i] {
        b'_' | b'^'
            if next_char().is_some_and(|c| "-{(*+.,".contains(c) || crate::chars::is_alnum(c)) =>
        {
            true
        }
        b'*' | b'~' | b'=' | b'+' | b'_' | b'/' if next_char().is_some_and(|c| !is_space(c)) => {
            true
        }
        b'[' => matches!(next(1), Some(b'[' | b'0'..=b'9')) || bracket_candidate(cx, i, end),
        b'@' => next(1) == Some(b'@'),
        b'{' => next(1) == Some(b'{') && next(2) == Some(b'{'),
        b'<' => {
            matches!(next(1), Some(b'%' | b'<' | b'0'..=b'9'))
                && (next(1) != Some(b'%') || next(2) == Some(b'%'))
                || link::type_at(cx, i + 1, end).is_some()
        }
        b'$' => true,
        b'\\' => backslash_candidate(cx, i, end),
        b'c' | b'C' | b's' | b'S' => {
            babel_candidate(cx, i, end) || plain_link_candidate(cx, i, end, floor)
        }
        _ => plain_link_candidate(cx, i, end, floor),
    }
}

fn bracket_candidate(cx: &InlineCx, i: usize, end: usize) -> bool {
    let r = &cx.src.text[i + 1..end];
    let starts_ci =
        |p: &str| r.len() >= p.len() && r.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    if starts_ci("cite:") || starts_ci("cite/") || starts_ci("fn:") || r.starts_with("%]") {
        return true;
    }
    // `/[0-9]*]`
    r.strip_prefix('/').is_some_and(|s| {
        s.trim_start_matches(|c: char| c.is_ascii_digit())
            .starts_with(']')
    })
}

fn backslash_candidate(cx: &InlineCx, i: usize, end: usize) -> bool {
    let b = cx.bytes();
    let Some(&c) = b.get(i + 1).filter(|_| i + 1 < end) else {
        return false;
    };
    if c.is_ascii_alphabetic() || c == b'[' || c == b'(' {
        return true;
    }
    if c == b'\\' {
        // `\\\\[ \t]*$`: only blanks up to the end of the line.
        let le = cx.line_end(i, end);
        return cx.src.text[i + 2..le]
            .bytes()
            .all(|x| x == b' ' || x == b'\t');
    }
    // `\_ +`
    c == b'_' && b.get(i + 2) == Some(&b' ') && i + 2 < end
}

fn babel_candidate(cx: &InlineCx, i: usize, end: usize) -> bool {
    let r = &cx.src.text[i..end];
    let starts_ci =
        |p: &str| r.len() >= p.len() && r.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes());
    starts_ci("call_") || starts_ci("src_")
}

/// `\<TYPE:` — a word start followed by a registered link type and a colon.
fn plain_link_candidate(cx: &InlineCx, i: usize, end: usize, floor: usize) -> bool {
    let word_start =
        cx.char_at(i).is_some_and(is_word) && !cx.char_before(i, floor).is_some_and(is_word);
    word_start && link::type_at(cx, i, end).is_some()
}

/// The parsers tried at a candidate, in `org-element--object-lex`'s order.
fn dispatch(cx: &InlineCx, i: usize, end: usize, floor: usize, r: Restriction) -> Option<Obj> {
    let try_ = |k: SyntaxKind, f: &dyn Fn() -> Option<Obj>| if r.allows(k) { f() } else { None };
    if babel_candidate(cx, i, end) {
        let lower = cx.bytes()[i].to_ascii_lowercase();
        return if lower == b'c' {
            try_(INLINE_BABEL_CALL, &|| {
                objects::inline_babel_call(cx, i, end)
            })
        } else {
            try_(INLINE_SRC_BLOCK, &|| objects::inline_src_block(cx, i, end))
        };
    }
    let b = cx.bytes();
    let second = b.get(i + 1).copied().filter(|_| i + 1 < end);
    match b[i] {
        b'^' => try_(SUPERSCRIPT, &|| objects::superscript(cx, i, end, floor)),
        b'_' => try_(UNDERLINE, &|| {
            emphasis::emphasis(cx, i, end, floor, UNDERLINE)
        })
        .or_else(|| try_(SUBSCRIPT, &|| objects::subscript(cx, i, end, floor))),
        b'*' => try_(BOLD, &|| emphasis::emphasis(cx, i, end, floor, BOLD)),
        b'/' => try_(ITALIC, &|| emphasis::emphasis(cx, i, end, floor, ITALIC)),
        b'~' => try_(CODE, &|| emphasis::emphasis(cx, i, end, floor, CODE)),
        b'=' => try_(VERBATIM, &|| {
            emphasis::emphasis(cx, i, end, floor, VERBATIM)
        }),
        b'+' => try_(STRIKE_THROUGH, &|| {
            emphasis::emphasis(cx, i, end, floor, STRIKE_THROUGH)
        }),
        b'@' => try_(EXPORT_SNIPPET, &|| objects::export_snippet(cx, i, end)),
        b'{' => try_(MACRO, &|| objects::macro_(cx, i, end)),
        b'$' => try_(LATEX_FRAGMENT, &|| {
            objects::latex_fragment(cx, i, end, floor)
        }),
        b'<' if second == Some(b'<') => try_(RADIO_TARGET, &|| objects::radio_target(cx, i, end))
            .or_else(|| try_(TARGET, &|| objects::target(cx, i, end))),
        b'<' => try_(TIMESTAMP, &|| objects::timestamp(cx, i, end))
            .or_else(|| try_(LINK, &|| link::link(cx, i, end, floor))),
        b'\\' if second == Some(b'\\') => try_(LINE_BREAK, &|| objects::line_break(cx, i, end)),
        b'\\' => try_(ENTITY, &|| objects::entity(cx, i, end)).or_else(|| {
            try_(LATEX_FRAGMENT, &|| {
                objects::latex_fragment(cx, i, end, floor)
            })
        }),
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
        Some(b'c') if r.allows(CITATION) => objects::citation(cx, i, end),
        Some(b'%' | b'/') if r.allows(STATISTICS_COOKIE) => objects::statistics_cookie(cx, i, end),
        _ => {
            let ts = if r.allows(TIMESTAMP) {
                objects::timestamp(cx, i, end)
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
