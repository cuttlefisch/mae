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

mod babel;
mod cite;
mod emphasis;
mod index;
mod latex;
mod lex;
mod link;
mod objects;
pub(crate) mod radio;
#[cfg(test)]
mod radio_reference;
mod script;
mod timestamp;

pub(crate) use index::InlineIndex;

use crate::kind::SyntaxKind::{self, *};
use crate::span::{ContentsTable, Span};
use crate::text::{LineEnding, Source};
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

/// One piece of an object, in order: a token, or a region parsed as the
/// objects of a container of the given kind (an object's contents, or a
/// citation's prefix and suffix, which Org parses but keeps outside them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Part {
    Tok(SyntaxKind, usize, usize),
    Objects(usize, usize, SyntaxKind),
}

/// An object found at a position: its extent, its contents range as Org
/// reports it (possibly empty, as in `[fn::]`), and its parts, which tile
/// `[begin, end)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Obj {
    pub kind: SyntaxKind,
    pub begin: usize,
    pub end: usize,
    pub contents: Option<(usize, usize)>,
    pub parts: Vec<Part>,
}

impl Obj {
    /// An object with no contents; add its parts with [`Obj::tok`].
    pub fn new(kind: SyntaxKind, begin: usize, end: usize) -> Obj {
        Obj {
            kind,
            begin,
            end,
            contents: None,
            parts: Vec::new(),
        }
    }

    /// Appends a token (empty ones are dropped).
    pub fn tok(mut self, kind: SyntaxKind, a: usize, b: usize) -> Obj {
        if a < b {
            self.parts.push(Part::Tok(kind, a, b));
        }
        self
    }

    /// Appends a region parsed as the objects of a `container`.
    pub fn objects(mut self, a: usize, b: usize, container: SyntaxKind) -> Obj {
        if a < b {
            self.parts.push(Part::Objects(a, b, container));
        }
        self
    }

    /// Appends the object's own contents (recorded even when empty).
    pub fn contents(mut self, a: usize, b: usize) -> Obj {
        self.contents = Some((a, b));
        let kind = self.kind;
        self.objects(a, b, kind)
    }

    /// The post-blank `[a, end)` as a final `WHITESPACE` token.
    pub fn blank(self, a: usize) -> Obj {
        let end = self.end;
        self.tok(WHITESPACE, a, end)
    }

    /// An object whose text between `value` bounds is opaque, delimited by
    /// markup, followed by post-blank up to `end`.
    pub fn opaque(
        kind: SyntaxKind,
        begin: usize,
        value: (usize, usize),
        value_end: usize,
        end: usize,
    ) -> Obj {
        Obj::new(kind, begin, end)
            .tok(MARKUP, begin, value.0)
            .tok(TEXT, value.0, value.1)
            .tok(MARKUP, value.1, value_end)
            .blank(value_end)
    }

    /// An object with parsed contents between `open` and `close` markup.
    pub fn delimited(
        kind: SyntaxKind,
        begin: usize,
        contents: (usize, usize),
        value_end: usize,
        end: usize,
    ) -> Obj {
        Obj::new(kind, begin, end)
            .tok(MARKUP, begin, contents.0)
            .contents(contents.0, contents.1)
            .tok(MARKUP, contents.1, value_end)
            .blank(value_end)
    }
}

/// Document-wide inputs to inline parsing.
#[derive(Clone, Copy)]
pub(crate) struct InlineCx<'a> {
    pub src: &'a Source<'a>,
    pub ix: &'a InlineIndex,
    /// Registered link types (`org-link-types`), longest first.
    pub link_types: &'a [String],
    /// The document's radio targets (`org-target-link-regexp`), if any.
    pub radio: Option<&'a radio::Radio>,
    /// Where inline parsing reports diagnostics.
    pub diagnostics: &'a std::cell::RefCell<Vec<crate::Diagnostic>>,
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

    /// Length of the line ending starting at `i` (0 if there is none).
    pub fn eol_len(&self, i: usize) -> usize {
        let b = self.bytes();
        match (self.src.eol, b.get(i)) {
            (LineEnding::Lf, Some(b'\n')) | (LineEnding::Cr, Some(b'\r')) => 1,
            (LineEnding::CrLf, Some(b'\n')) => 1,
            (LineEnding::CrLf, Some(b'\r')) if b.get(i + 1) == Some(&b'\n') => 2,
            _ => 0,
        }
    }

    /// The char at `i` as Emacs's buffer holds it — a line ending is one
    /// `\n` — or `None` at or past `end`.
    pub fn ch(&self, i: usize, end: usize) -> Option<char> {
        if i >= end {
            None
        } else if self.eol_len(i) > 0 {
            Some('\n')
        } else {
            self.char_at(i)
        }
    }

    /// Where the char at `i` (as [`InlineCx::ch`] reads it) ends.
    pub fn step(&self, i: usize) -> usize {
        match self.eol_len(i) {
            0 => i + self.char_at(i).map_or(1, char::len_utf8),
            n => i + n,
        }
    }

    /// The char before `i` as Emacs's buffer holds it, `None` at `floor`.
    pub fn ch_before(&self, i: usize, floor: usize) -> Option<char> {
        if i <= floor {
            return None;
        }
        let b = self.bytes();
        let eol = match self.src.eol {
            LineEnding::Cr => b[i - 1] == b'\r',
            _ => b[i - 1] == b'\n',
        };
        if eol {
            Some('\n')
        } else {
            self.char_before(i, floor)
        }
    }

    /// `$` at `i` in a region ending at `end`: a line end or the region's end.
    pub fn at_eol(&self, i: usize, end: usize) -> bool {
        i >= end || self.eol_len(i) > 0
    }

    /// `(bolp)` within a region starting at `floor`.
    pub fn at_bol(&self, i: usize, floor: usize) -> bool {
        i == floor || self.ch_before(i, floor) == Some('\n')
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

    /// Does the region text at `i` start with ASCII `lit`, ignoring ASCII
    /// case (`case-fold-search`; no non-ASCII char folds to ASCII)?
    pub fn starts_ci(&self, i: usize, end: usize, lit: &str) -> bool {
        let r = self.bytes().get(i..end).unwrap_or_default();
        r.len() >= lit.len() && r[..lit.len()].eq_ignore_ascii_case(lit.as_bytes())
    }

    /// Does the region text at `i` start with `lit` exactly?
    pub fn starts(&self, i: usize, end: usize, lit: &str) -> bool {
        self.bytes()
            .get(i..end)
            .is_some_and(|r| r.starts_with(lit.as_bytes()))
    }
}

pub(crate) struct Frame {
    pub end: usize,
    pub restriction: Restriction,
    pub cursor: usize,
    /// Where the region starts (for `bolp`).
    pub floor: usize,
    /// The rest of the object this region belongs to, emitted when the
    /// region is done; `None` for the root.
    rest: Option<std::vec::IntoIter<Part>>,
    pub radio: radio::Cursor,
}

impl Frame {
    fn new(
        beg: usize,
        end: usize,
        container: SyntaxKind,
        rest: Option<std::vec::IntoIter<Part>>,
    ) -> Frame {
        Frame {
            end,
            restriction: Restriction::for_container(container),
            cursor: beg,
            floor: beg,
            rest,
            radio: radio::Cursor::default(),
        }
    }
}

/// Parses `[beg, end)` as the contents of a `container`, appending events.
/// `budget` is how many levels of objects may be opened: deeper regions are
/// plain text, so the tree stays within the nesting cap however deeply the
/// input nests, and the parse never recurses.
pub(crate) fn parse(
    cx: &InlineCx,
    beg: usize,
    end: usize,
    container: SyntaxKind,
    budget: usize,
    out: &mut Vec<Event>,
    contents: &mut ContentsTable,
) {
    // A CAPTION value sits inside its keyword line node, one level deeper
    // than a title or a tag inside their element.
    let budget = match container {
        AFFILIATED_VALUE => budget.saturating_sub(1),
        _ => budget,
    };
    let mut stack = vec![Frame::new(beg, end, container, None)];
    loop {
        // Objects at this depth would pass the nesting cap: plain text.
        let room = stack.len() <= budget;
        let Some(top) = stack.last_mut() else { break };
        let found = if top.cursor < top.end && room {
            lex::next_object(cx, top)
        } else {
            if top.cursor < top.end && lex::next_object(cx, top).is_some() {
                // Past the cap a region is plain text; say so when Org would
                // have found an object in it (a radio target there makes no
                // radio links: Org finds targets at any depth).
                cx.diagnostics.borrow_mut().push(crate::Diagnostic::new(
                    crate::DiagnosticKind::DepthLimit,
                    top.cursor,
                    top.end,
                ));
            }
            None
        };
        let Some(obj) = found else {
            text_run(cx.src, top.cursor, top.end, out);
            if let Some(rest) = stack.pop().and_then(|f| f.rest) {
                emit_parts(cx, rest, &mut stack, out);
            }
            continue;
        };
        text_run(cx.src, top.cursor, obj.begin, out);
        top.cursor = obj.end;
        if let Some((a, b)) = obj.contents {
            contents.push(obj.kind, obj.begin, obj.end, Span::new(a, b));
        }
        out.push(Event::Start(obj.kind));
        emit_parts(cx, obj.parts.into_iter(), &mut stack, out);
    }
}

/// Emits an object's parts until one is a region to parse: that region is
/// pushed as a frame holding the remaining parts. When the parts run out, the
/// object is finished.
fn emit_parts(
    cx: &InlineCx,
    mut parts: std::vec::IntoIter<Part>,
    stack: &mut Vec<Frame>,
    out: &mut Vec<Event>,
) {
    while let Some(p) = parts.next() {
        match p {
            Part::Tok(k, a, b) => token(cx.src, k, a, b, out),
            Part::Objects(a, b, container) => {
                stack.push(Frame::new(a, b, container, Some(parts)));
                return;
            }
        }
    }
    out.push(Event::Finish);
}

/// A token `[a, b)` of `kind`, cut at line endings like plain text.
fn token(src: &Source, kind: SyntaxKind, a: usize, b: usize, out: &mut Vec<Event>) {
    let mut pos = a;
    while pos < b {
        let Some(&l) = src.lines.get(src.line_of(pos)) else {
            break;
        };
        let content_end = l.end.clamp(pos, b);
        if pos < content_end {
            out.push(Event::Token(kind, pos, content_end));
        }
        let next = l.next.min(b).max(content_end);
        if content_end < next {
            out.push(Event::Token(NEWLINE, content_end, next));
        }
        if next <= pos {
            out.push(Event::Token(kind, pos, b));
            break;
        }
        pos = next;
    }
}

/// Plain text `[a, b)`: `TEXT` per line, `NEWLINE` for line endings.
pub(crate) fn text_run(src: &Source, a: usize, b: usize, out: &mut Vec<Event>) {
    token(src, TEXT, a, b, out);
}
