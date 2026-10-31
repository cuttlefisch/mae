//! Events and the line tokenizers that produce them.
//!
//! The builder decides structure; this module cuts each line into tokens so
//! that every byte lands in exactly one token. Every tokenizer ends with the
//! generic [`Emitter::lines`] over whatever it did not claim, so a line that
//! does not have the expected shape still round-trips, as text.

use crate::kind::SyntaxKind::{self, *};
use crate::list::item_head;
use crate::scan;
use crate::text::{is_blank, trim_end_blank, ws_len, Source};

/// One step of a pre-order walk of the tree to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Event {
    Start(SyntaxKind),
    Finish,
    Token(SyntaxKind, usize, usize),
    /// A headline's line content `[start, end)`. Its tokens depend on the
    /// TODO keywords, which are only known once every `#+TODO:` keyword
    /// element has been seen, so they are cut when the tree is built.
    HeadlineLine(usize, usize),
}

pub(crate) struct Emitter<'a> {
    pub src: &'a Source<'a>,
    pub events: Vec<Event>,
}

impl<'a> Emitter<'a> {
    pub fn new(src: &'a Source<'a>) -> Emitter<'a> {
        Emitter {
            src,
            events: Vec::new(),
        }
    }

    pub fn start(&mut self, kind: SyntaxKind) {
        self.events.push(Event::Start(kind));
    }

    pub fn finish(&mut self) {
        self.events.push(Event::Finish);
    }

    pub fn tok(&mut self, kind: SyntaxKind, a: usize, b: usize) {
        if a < b {
            self.events.push(Event::Token(kind, a, b));
        }
    }

    /// `[a, b)` as lines: whole blank lines become `BLANK_LINE`, other line
    /// content becomes `kind` (or `WHITESPACE` when blank), and line endings
    /// become `NEWLINE`.
    pub fn lines(&mut self, a: usize, b: usize, kind: SyntaxKind) {
        if a >= b {
            return;
        }
        let mut i = self.src.line_of(a);
        while let Some(&l) = self.src.lines.get(i) {
            if l.start >= b {
                break;
            }
            let seg = a.max(l.start);
            let next = l.next.min(b);
            let content_end = l.end.min(b).max(seg);
            let blank = is_blank(&self.src.text[seg..content_end]);
            if seg == l.start && next == l.next && blank {
                self.tok(BLANK_LINE, seg, next);
            } else {
                self.tok(if blank { WHITESPACE } else { kind }, seg, content_end);
                self.tok(NEWLINE, content_end, next);
            }
            i += 1;
        }
    }

    /// The part of line `[start, line.end)` not yet claimed, then its ending,
    /// all clipped to `end`.
    fn rest(&mut self, from: usize, end: usize) {
        self.lines(from, end, TEXT);
    }
}

/// Claims consecutive tokens on one line, never past `end`.
struct Cut<'e, 'a> {
    em: &'e mut Emitter<'a>,
    base: usize,
    pos: usize,
    end: usize,
}

impl<'e, 'a> Cut<'e, 'a> {
    fn new(em: &'e mut Emitter<'a>, base: usize, end: usize) -> Cut<'e, 'a> {
        Cut {
            em,
            base,
            pos: base,
            end,
        }
    }

    /// Emits `kind` up to line-relative offset `to`.
    fn to(&mut self, kind: SyntaxKind, to: usize) {
        let e = (self.base + to).min(self.end);
        if e > self.pos {
            self.em.tok(kind, self.pos, e);
            self.pos = e;
        }
    }

    /// Emits the blanks starting at the cursor, if any.
    fn ws(&mut self) {
        let rel = self.pos - self.base;
        let s = &self.em.src.text[self.pos..self.end.max(self.pos)];
        let n = ws_len(s);
        self.to(WHITESPACE, rel + n);
    }

    fn done(self) {
        let (pos, end) = (self.pos, self.end);
        self.em.rest(pos, end);
    }
}

/// The content of the line starting at `start`.
fn line_at<'a>(src: &Source<'a>, start: usize) -> &'a str {
    let l = src.lines[src.line_of(start)];
    &src.text[start..l.end.max(start)]
}

/// `#+KEY: value` (keywords, babel calls, affiliated keywords).
pub(crate) fn keyword_line(em: &mut Emitter, start: usize, end: usize, affiliated: bool) {
    let s = line_at(em.src, start);
    let key_end = if affiliated {
        scan::affiliated(s).map(|e| s[..e].trim_end_matches([' ', '\t']).len() - 1)
    } else {
        scan::keyword_key(s).map(|(_, e)| e)
    };
    let mut c = Cut::new(em, start, end);
    if let Some(key_end) = key_end {
        let ws = ws_len(s);
        c.to(WHITESPACE, ws);
        c.to(HASH_PLUS, ws + 2);
        let name_end = s[..key_end]
            .find('[')
            .filter(|_| affiliated)
            .unwrap_or(key_end);
        c.to(KEY, name_end);
        c.to(TEXT, key_end);
        c.to(COLON, key_end + 1);
        c.ws();
        c.to(VALUE, trim_end_blank(s).len());
        c.ws();
    }
    c.done();
}

/// A block or drawer's opening line, as a `BLOCK_BEGIN` node.
pub(crate) fn begin_line(em: &mut Emitter, kind: SyntaxKind, start: usize, end: usize) {
    let s = line_at(em.src, start);
    em.start(BLOCK_BEGIN);
    let mut c = Cut::new(em, start, end);
    let ws = ws_len(s);
    c.to(WHITESPACE, ws);
    match kind {
        DRAWER | PROPERTY_DRAWER => drawer_parts(&mut c, s),
        DYNAMIC_BLOCK => {
            if let Some((a, b)) = scan::dynamic_begin(s) {
                c.to(HASH_PLUS, ws + 2);
                c.to(BLOCK_MARKER, ws + 7);
                c.to(COLON, ws + 8);
                c.to(WHITESPACE, a);
                c.to(BLOCK_NAME, b);
                c.ws();
                c.to(PARAMETERS, trim_end_blank(s).len());
            }
        }
        _ => {
            if let Some((a, b)) = scan::block_begin(s) {
                c.to(HASH_PLUS, ws + 2);
                c.to(BLOCK_MARKER, a);
                c.to(BLOCK_NAME, b);
                c.ws();
                c.to(PARAMETERS, trim_end_blank(s).len());
            }
        }
    }
    c.ws();
    c.done();
    em.finish();
}

/// A block or drawer's closing line, as a `BLOCK_END` node.
pub(crate) fn end_line(em: &mut Emitter, kind: SyntaxKind, start: usize, end: usize) {
    let s = line_at(em.src, start);
    em.start(BLOCK_END);
    let mut c = Cut::new(em, start, end);
    let ws = ws_len(s);
    c.to(WHITESPACE, ws);
    match kind {
        DRAWER | PROPERTY_DRAWER => drawer_parts(&mut c, s),
        DYNAMIC_BLOCK => {
            c.to(HASH_PLUS, ws + 2);
            c.to(BLOCK_MARKER, ws + 5);
            if s[ws + 5..].starts_with(':') {
                c.to(COLON, ws + 6);
            }
        }
        _ => {
            if let Some(name) = scan::block_end_name(s) {
                c.to(HASH_PLUS, ws + 2);
                c.to(BLOCK_MARKER, ws + 6);
                c.to(BLOCK_NAME, ws + 6 + name.len());
            }
        }
    }
    c.ws();
    c.done();
    em.finish();
}

fn drawer_parts(c: &mut Cut, s: &str) {
    if let Some((a, b)) = scan::drawer_name(s) {
        c.to(COLON, a);
        c.to(NAME, b);
        c.to(COLON, b + 1);
    }
}

/// `:KEY: value` / `:KEY+: value` inside a property drawer.
pub(crate) fn property_line(em: &mut Emitter, start: usize, end: usize) {
    let s = line_at(em.src, start);
    let mut c = Cut::new(em, start, end);
    let ws = ws_len(s);
    let run = ws + 1 + crate::text::non_space_len(&s[ws + 1..]);
    if s[ws..].starts_with(':') && run > ws + 2 && s[..run].ends_with(':') {
        // The key keeps a trailing `+` (`:KEY+:`), as org-element's :key does.
        c.to(WHITESPACE, ws);
        c.to(COLON, ws + 1);
        c.to(NAME, run - 1);
        c.to(COLON, run);
        c.ws();
        c.to(VALUE, trim_end_blank(s).len());
        c.ws();
    }
    c.done();
}

/// `SCHEDULED: <…> DEADLINE: <…>`: keywords as tokens, the rest text.
pub(crate) fn planning_line(em: &mut Emitter, start: usize, end: usize) {
    let s = line_at(em.src, start);
    let mut c = Cut::new(em, start, end);
    let mut i = 0;
    while i < s.len() {
        let hit = ["CLOSED:", "DEADLINE:", "SCHEDULED:"].iter().find(|k| {
            crate::text::starts_with_ci(&s[i..], k)
                && (i == 0 || s.as_bytes()[i - 1] == b' ' || s.as_bytes()[i - 1] == b'\t')
        });
        match hit {
            Some(k) => {
                c.to(TEXT, i);
                c.to(PLANNING_KEYWORD, i + k.len());
                i += k.len();
            }
            None => i += s[i..].chars().next().map_or(1, char::len_utf8),
        }
    }
    c.done();
}

/// A list item's bullet, counter, checkbox and tag.
pub(crate) fn item_line(em: &mut Emitter, start: usize, end: usize) {
    let s = line_at(em.src, start);
    let h = item_head(s);
    let mut c = Cut::new(em, start, end);
    c.to(WHITESPACE, h.bullet.0);
    c.to(BULLET, h.bullet.1);
    c.ws();
    if let Some((_, b)) = h.counter {
        c.to(COUNTER, b);
        c.ws();
    }
    if let Some((_, b)) = h.checkbox {
        c.to(CHECKBOX, b);
        c.ws();
    }
    // An ordered item's tag is part of its contents, not its head.
    if let Some((a, b)) = h.tag.filter(|&(a, _)| h.contents_start > a) {
        c.to(TEXT, a);
        c.to(ITEM_TAG, b);
        c.to(TEXT, h.contents_start);
    }
    c.done();
}

/// `[fn:LABEL]` then whatever blanks precede the definition's contents.
pub(crate) fn footnote_head(em: &mut Emitter, start: usize, end: usize) {
    let s = line_at(em.src, start);
    let mut c = Cut::new(em, start, end);
    if let Some(n) = scan::footnote_definition(s) {
        c.to(FOOTNOTE_LABEL, n);
        c.ws();
    }
    c.done();
}
