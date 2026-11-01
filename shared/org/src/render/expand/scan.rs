//! `org-macro-replace-all`, sequentially: each macro is judged against the
//! buffer as it stands when the scan reaches it (the expanded text before
//! it, the unexpanded text after), replaced, and the scan goes on from
//! where its expansion starts.
//!
//! The buffer is the text of its last full parse (`dtext`, parsed as `d`)
//! with the edits made since. The scan goes unit by unit: a unit is the
//! element whose objects hold a macro (a paragraph's contents, a headline's
//! title, a table row, a parsed keyword's value). An expansion that holds a
//! line ending, or stands at the start of a line, can change what every
//! later line is: the whole buffer is parsed again. Any other can change
//! only its unit's objects: the unit alone is read again from its start,
//! unless the expansion is a plain word between blanks, which changes no
//! object around it. Every read is charged to the budget.

use super::syntax::{fold, object_macros, raw_macros};
use super::value::Template;
use super::{Expansion, Found, MacroFailure, Map, Piece, Stop, PARSED_KEYWORDS};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::views::{Headline, Keyword};
use crate::{Document, LineEnding, RenderDiagnostic, Span};
use rowan::WalkEvent;
use std::collections::{HashMap, HashSet};

/// What kind of element a unit is, which decides how it is read again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A paragraph's contents: its objects.
    Paragraph,
    /// A headline's title (its headline's span, and its line's): the line
    /// can become COMMENT.
    Title {
        headline: (usize, usize),
        line: (usize, usize),
    },
    /// A table row.
    Row,
    /// A parsed keyword's or `EXPORT_` property's value: Org's regexp.
    Forced,
    /// Anything else (a caption, an item's tag, a verse block): the whole
    /// buffer is read again after each expansion.
    Other,
}

/// The element holding a macro: its kind and span in `dtext`, and whether
/// it starts a line (only blanks before it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unit {
    kind: Kind,
    span: (usize, usize),
    line_start: bool,
}

/// A macro of the current parse and the unit holding it.
#[derive(Debug, Clone)]
struct Occurrence {
    found: Found,
    unit: Unit,
}

pub(super) struct Engine {
    pub(super) templates: HashMap<String, Template>,
    pub(super) counters: HashMap<String, u64>,
    pub(super) record: HashSet<(usize, String)>,
    pub(super) work: u64,
    pub(super) work_limit: u64,
    pub(super) size_limit: usize,
    /// Source spans of the macros expanded with a value from the
    /// environment.
    pub(super) notes: Vec<RenderDiagnostic>,
    /// Whether the macro being expanded did.
    pub(super) environment: bool,
    pub(super) opts: crate::ParseOptions,
    pub(super) eol: LineEnding,
    /// The buffer's last full parse, and its text.
    pub(super) d: Document,
    pub(super) dtext: String,
    /// The source → `dtext` map, and the edits since (`dtext` → buffer).
    pub(super) map: Map,
    pub(super) segment: Vec<Piece>,
    /// Macros (by start in `dtext`) not to expand, and COMMENT subtrees
    /// expansions made (in `dtext`).
    pub(super) skip: HashSet<usize>,
    pub(super) commented: Vec<(usize, usize)>,
    /// Caches of the current parse.
    pub(super) lines: Option<Vec<String>>,
    pub(super) keywords: HashMap<String, Option<String>>,
    pub(super) properties: HashMap<usize, Vec<(String, String)>>,
}

/// How a unit's scan ended.
enum Flow {
    /// The unit is done; the scan goes on after it.
    Done,
    /// The buffer must be parsed again; the scan goes on at this offset.
    Reparse(String, usize),
}

/// A unit being scanned: its text is `done` + `pending` + `dtext[rest..end]`.
struct Cursor {
    unit: Unit,
    /// Where the unit's text starts in the buffer.
    at: usize,
    done: String,
    pending: String,
    rest: usize,
    /// The expansion `pending` holds: what it replaced in `dtext`, and where
    /// it starts in the buffer.
    piece: Option<((usize, usize), usize)>,
}

impl Cursor {
    fn text(&self, dtext: &str) -> String {
        let mut s = String::with_capacity(self.done.len() + self.pending.len());
        s.push_str(&self.done);
        s.push_str(&self.pending);
        s.push_str(&dtext[self.rest..self.unit.span.1]);
        s
    }

    /// The unit's text from offset `r` to `e`.
    fn slice(&self, (r, e): (usize, usize), dtext: &str) -> String {
        let (dl, pl) = (self.done.len(), self.pending.len());
        let mut s = String::new();
        if r < dl + pl {
            s.push_str(&self.pending[r.max(dl) - dl..e.min(dl + pl) - dl]);
        }
        if e > dl + pl {
            let a = self.rest + r.max(dl + pl) - dl - pl;
            s.push_str(&dtext[a..self.rest + e - dl - pl]);
        }
        s
    }

    /// Where unit offset `r` is in `dtext`, if it is untouched text.
    fn in_dtext(&self, r: usize) -> Option<usize> {
        let before = self.done.len() + self.pending.len();
        (r >= before).then(|| self.rest + r - before)
    }

    /// Replaces unit offsets `r..e` by `value` (`r` at or after `done`).
    /// The `dtext` range of the expansion `pending` now holds.
    fn splice(
        &mut self,
        (r, e): (usize, usize),
        value: String,
        dtext: &str,
        segment: &mut Vec<Piece>,
    ) -> (usize, usize) {
        let (dl, pl) = (self.done.len(), self.pending.len());
        if r >= dl + pl {
            self.close(segment);
            let k = self.rest + (r - dl - pl);
            self.done.push_str(&dtext[self.rest..k]);
            let src = (k, k + (e - r));
            self.rest = src.1;
            self.pending = value;
            self.piece = Some((src, self.at + self.done.len()));
            return src;
        }
        let k = r - dl;
        self.done.push_str(&self.pending[..k]);
        let e = e - dl;
        let ((a, mut b), start) = self.piece.expect("pending holds an expansion");
        if e <= pl {
            self.pending = format!("{value}{}", &self.pending[e..]);
        } else {
            self.rest += e - pl;
            b = self.rest;
            self.pending = value;
        }
        self.piece = Some(((a, b), start));
        (a, b)
    }

    /// Ends the expansion `pending` holds: it becomes final text.
    fn close(&mut self, segment: &mut Vec<Piece>) {
        let end = self.at + self.done.len() + self.pending.len();
        self.done.push_str(&self.pending);
        self.pending.clear();
        if let Some((src, start)) = self.piece.take() {
            segment.push(Piece {
                src,
                out: (start, end),
            });
        }
    }

    /// Whether the text just spliced in at the end of `done` stands at the
    /// start of a line (only blanks before it).
    fn at_line_start(&self, eol: char) -> bool {
        let before = self.done.trim_end_matches([' ', '\t']);
        if before.is_empty() {
            return self.unit.line_start;
        }
        before.ends_with(eol)
    }

    /// Whether the line the text just spliced in starts still starts no
    /// element: it begins with a word not followed by `:` (`CLOCK:`,
    /// `SCHEDULED:`) or `.`/`)` after digits (`1.`), or with a character
    /// no element starts with (a headline, a list item, a keyword, a block,
    /// drawer or table line, a footnote definition, a LaTeX environment, a
    /// diary sexp all start with `* # : | - + [ \ %`, a digit or a word).
    /// The line it replaces started with `{{{`, which starts none either.
    fn line_stays_text(&self, dtext: &str) -> bool {
        let rest = &dtext[self.rest..self.unit.span.1];
        let head: String = self.pending.chars().chain(rest.chars()).take(64).collect();
        let head = head.trim_start_matches([' ', '\t']);
        let word = head
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        let next = head.as_bytes().get(word).copied();
        match head.bytes().next() {
            Some(b) if b.is_ascii_alphabetic() => next != Some(b':'),
            Some(b) if b.is_ascii_digit() => !matches!(next, Some(b'.' | b')' | b':')),
            // What an element's first line may start with, or a line
            // ending (an empty line ends the paragraph).
            None
            | Some(b'*' | b'#' | b':' | b'|' | b'-' | b'+' | b'[' | b'\\' | b'%' | b'\n' | b'\r') => {
                false
            }
            Some(_) => true,
        }
    }

    /// Whether `value`, just spliced in, is a plain word between blanks
    /// (or at the start of a line, then followed by a blank or the line's
    /// end): no object around it can start, end or change with it, and a
    /// line starting with a word and a blank starts no element.
    fn inert(&self, value: &str, dtext: &str, line_start: bool) -> bool {
        let before = self.done.chars().next_back();
        let after = self.pending[value.len()..]
            .chars()
            .next()
            .or_else(|| dtext[self.rest..self.unit.span.1].chars().next());
        !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_alphanumeric())
            && (line_start || matches!(before, Some(' ' | '\t')))
            && matches!(after, None | Some(' ' | '\t' | '\n' | '\r'))
    }
}

impl Engine {
    pub(super) fn new(
        doc: &Document,
        source_len: usize,
        templates: HashMap<String, Template>,
    ) -> Engine {
        Engine {
            templates,
            counters: HashMap::new(),
            record: HashSet::new(),
            work: 0,
            work_limit: super::WORK_BASE + super::WORK_PER_BYTE * source_len as u64,
            size_limit: super::SIZE_BASE + super::SIZE_PER_BYTE * source_len,
            notes: Vec::new(),
            environment: false,
            opts: crate::ParseOptions {
                todo_sequences: doc.todo_sequences().to_vec(),
                ..crate::ParseOptions::default()
            },
            eol: doc.line_ending(),
            d: doc.clone(),
            dtext: doc.syntax().to_string(),
            map: Map::default(),
            segment: Vec::new(),
            skip: HashSet::new(),
            commented: Vec::new(),
            lines: None,
            keywords: HashMap::new(),
            properties: HashMap::new(),
        }
    }

    pub(super) fn charge(&mut self, n: usize) -> Result<(), Stop> {
        self.work += n as u64 + 1;
        #[cfg(test)]
        super::WORK.with(|w| w.set(w.get() + n as u64 + 1));
        if self.work > self.work_limit {
            return Err((MacroFailure::Budget, 0, 0));
        }
        Ok(())
    }

    fn eol_char(&self) -> char {
        match self.eol {
            LineEnding::Cr => '\r',
            _ => '\n',
        }
    }

    /// The scan over `self.d` (`map` from the source to its text), skipping
    /// the macros starting at an offset in `skip`. `Ok(None)` when it
    /// expanded nothing.
    pub(super) fn run(
        mut self,
        map: Map,
        skip: HashSet<usize>,
    ) -> Result<Option<Expansion>, RenderDiagnostic> {
        self.map = map;
        self.skip = skip;
        let mut found = self.occurrences();
        let mut out = String::with_capacity(self.dtext.len());
        let (mut tail, mut i, mut expanded) = (0, 0, false);
        while let Some(o) = found.get(i).cloned() {
            i += 1;
            if o.found.0 < tail || self.in_comment(o.found.0) {
                continue;
            }
            // Right after a re-read the scan may start inside a unit, whose
            // text before the scan is already in `out`.
            let start = o.unit.span.0.max(tail);
            out.push_str(&self.dtext[tail..start]);
            out.truncate(out.len() - (start - o.unit.span.0));
            let flow = self
                .unit(&o, &found[i - 1..], tail, &mut out)
                .map_err(|(f, a, b)| self.failed(f, a, b))?;
            expanded = true;
            match flow {
                Flow::Done => tail = o.unit.span.1,
                Flow::Reparse(buffer, at) => {
                    self.reread(buffer, at)
                        .map_err(|(f, ..)| self.failed(f, 0, self.dtext.len()))?;
                    out = self.dtext[..at].to_string();
                    tail = at;
                    found = self.occurrences();
                }
            }
            i = found.partition_point(|x| x.found.0 < tail);
        }
        if !expanded {
            return Ok(None);
        }
        out.push_str(&self.dtext[tail..]);
        let map = self.map.then(&self.segment);
        Ok(Some(Expansion {
            text: out,
            map,
            notes: self.notes,
        }))
    }

    fn failed(&self, f: MacroFailure, a: usize, b: usize) -> RenderDiagnostic {
        RenderDiagnostic::MacroFailed(f, self.map.to_source(Span::new(a, b)))
    }

    fn in_comment(&self, p: usize) -> bool {
        self.commented.iter().any(|&(a, b)| p >= a && p < b)
    }

    /// The buffer `buffer` becomes the parsed text; the scan goes on at
    /// `at` of it.
    fn reread(&mut self, buffer: String, at: usize) -> Result<(), Stop> {
        self.charge(buffer.len())?;
        if buffer.len() > self.size_limit {
            return Err((MacroFailure::Budget, 0, 0));
        }
        let step = Map {
            pieces: std::mem::take(&mut self.segment),
        };
        self.map = self.map.then(&step.pieces);
        let moved = |p: usize| step.locate(p, true, true);
        self.skip = self.skip.iter().map(|&p| moved(p)).collect();
        self.commented = self
            .commented
            .iter()
            .map(|&(a, b)| (moved(a), step.locate(b, false, true)))
            .collect();
        self.d = self.d.reparse(&buffer);
        self.dtext = buffer;
        self.lines = None;
        self.keywords.clear();
        self.properties.clear();
        let _ = at;
        Ok(())
    }

    /// Scans unit `o.unit` from `from` (its occurrences in `found`, then
    /// whatever its expansions make), appending its text to `out`.
    fn unit(
        &mut self,
        o: &Occurrence,
        found: &[Occurrence],
        from: usize,
        out: &mut String,
    ) -> Result<Flow, Stop> {
        let unit = o.unit;
        let mut c = Cursor {
            unit,
            at: out.len(),
            done: String::new(),
            pending: String::new(),
            rest: unit.span.0,
            piece: None,
        };
        let base = unit.span.0;
        // `found` starts at this unit's first macro; a unit's macros are
        // contiguous.
        let mut cands: Vec<Found> = found
            .iter()
            .take_while(|x| x.unit == unit)
            .filter(|x| x.found.0 >= from)
            .map(|x| {
                let (a, b, n, args) = x.found.clone();
                (a - base, b - base, n, args)
            })
            .collect();
        let (mut k, mut adj) = (0, 0isize);
        while let Some((a, b, name, args)) = cands.get(k).cloned() {
            k += 1;
            let (r, e) = ((a as isize + adj) as usize, (b as isize + adj) as usize);
            let raw = c.slice((r, e), &self.dtext);
            let at = c.at + r;
            // What the expansion replaces, in `dtext`: itself, or the
            // expansion holding it.
            let here = match c.in_dtext(r) {
                Some(s) => (s, s + (e - r)),
                None => c.piece.map_or((base, base), |p| p.0),
            };
            let value = self
                .call((&name, args.as_deref(), &raw), base, at)
                .map_err(|(f, ..)| (f, here.0, here.1))?;
            if std::mem::take(&mut self.environment) {
                let s = self.map.to_source(Span::new(here.0, here.1));
                self.notes.push(RenderDiagnostic::MacroEnvironment(s));
            }
            let vlen = value.len();
            let structural = value.contains(['\n', '\r']);
            c.splice((r, e), value.clone(), &self.dtext, &mut self.segment);
            self.charge(c.pending.len())?;
            if c.at + c.done.len() + c.pending.len() > self.size_limit {
                return Err((MacroFailure::Budget, here.0, here.1));
            }
            let line_start = c.at_line_start(self.eol_char());
            let inert = !structural && c.inert(&value, &self.dtext, line_start);
            let line_start = line_start && !c.line_stays_text(&self.dtext);
            if !inert && (structural || line_start || unit.kind == Kind::Other) {
                return Ok(self.reparse_flow(c, out));
            }
            if let Kind::Title { headline, line } = unit.kind {
                let text = format!(
                    "{}{}{}",
                    &self.dtext[line.0..unit.span.0],
                    c.text(&self.dtext),
                    &self.dtext[unit.span.1..line.1]
                );
                if self.commented_line(&text)? {
                    self.commented.push(headline);
                    return Ok(self.finish(c, out));
                }
            }
            if inert {
                adj += vlen as isize - (e - r) as isize;
                continue;
            }
            cands = self.reread_unit(&c, r)?;
            k = 0;
            adj = 0;
        }
        Ok(self.finish(c, out))
    }

    /// The unit's macros at or after unit offset `r`, read again.
    fn reread_unit(&mut self, c: &Cursor, r: usize) -> Result<Vec<Found>, Stop> {
        let text = c.text(&self.dtext);
        self.charge(text.len())?;
        let all = match c.unit.kind {
            Kind::Forced => raw_macros(&text, r, text.len()),
            Kind::Row => {
                let d = crate::parse(&format!("{text}\n"), &self.opts);
                object_macros(&d.syntax())
            }
            Kind::Title { .. } => object_macros(&crate::green::objects(&text, HEADLINE_TITLE)),
            _ => object_macros(&crate::green::objects(&text, PARAGRAPH)),
        };
        Ok(all
            .into_iter()
            .filter(|m| m.0 >= r && !self.left(&m.2))
            .filter(|m| c.in_dtext(m.0).is_none_or(|p| !self.skip.contains(&p)))
            .collect())
    }

    /// Whether a macro of this name is left as it is (a results macro
    /// without template, for babel).
    fn left(&self, name: &str) -> bool {
        fold(name) == "results" && !self.templates.contains_key("results")
    }

    /// Whether headline line `line` reads as COMMENT.
    fn commented_line(&mut self, line: &str) -> Result<bool, Stop> {
        self.charge(line.len())?;
        let d = crate::parse(&format!("{line}\n"), &self.opts);
        Ok(d.headlines().next().is_some_and(|h| h.is_commented()))
    }

    /// The unit is done: its text joins `out`.
    fn finish(&mut self, mut c: Cursor, out: &mut String) -> Flow {
        c.close(&mut self.segment);
        out.push_str(&c.done);
        out.push_str(&self.dtext[c.rest..c.unit.span.1]);
        Flow::Done
    }

    /// The buffer as it now is, to be parsed again, and where the scan goes
    /// on in it (where the last expansion starts).
    fn reparse_flow(&mut self, mut c: Cursor, out: &str) -> Flow {
        let at = c.at + c.done.len();
        c.close(&mut self.segment);
        let mut buffer = String::with_capacity(self.dtext.len());
        buffer.push_str(out);
        buffer.push_str(&c.done);
        buffer.push_str(&self.dtext[c.rest..]);
        Flow::Reparse(buffer, at)
    }

    /// Every macro Org's export expands in the current parse, in order,
    /// with its unit: macro objects and macros in parsed keywords, not under
    /// a COMMENT headline, not a results macro left to babel, not skipped.
    fn occurrences(&mut self) -> Vec<Occurrence> {
        let root = self.d.syntax();
        let mut out = Vec::new();
        let mut walk = root.preorder();
        while let Some(ev) = walk.next() {
            let WalkEvent::Enter(n) = ev else { continue };
            match n.kind() {
                HEADLINE if Headline::cast(n.clone()).is_some_and(|h| h.is_commented()) => {
                    walk.skip_subtree()
                }
                MACRO => {
                    let unit = self.unit_of(&n);
                    out.extend(
                        super::syntax::object_macro(&n).map(|found| Occurrence { found, unit }),
                    );
                }
                KEYWORD if is_parsed(&n) => self.forced_in(&n, &mut out),
                NODE_PROPERTY if super::syntax::is_export_property(&n) => {
                    self.forced_in(&n, &mut out)
                }
                _ => {}
            }
        }
        out.retain(|o| !self.left(&o.found.2) && !self.skip.contains(&o.found.0));
        out
    }

    /// The unit holding macro object `n`.
    fn unit_of(&self, n: &SyntaxNode) -> Unit {
        let span = |x: &SyntaxNode| {
            (
                usize::from(x.text_range().start()),
                usize::from(x.text_range().end()),
            )
        };
        let a = n
            .ancestors()
            .skip(1)
            .find(|a| (a.kind() as u16) < 2000)
            .unwrap_or_else(|| n.clone());
        let (kind, sp) = match a.kind() {
            PARAGRAPH => match self.d.contents(&a) {
                Some(c) if c.start <= Span::of(n).start && Span::of(n).end <= c.end => {
                    (Kind::Paragraph, (c.start as usize, c.end as usize))
                }
                _ => (Kind::Other, span(&a)),
            },
            HEADLINE_TITLE => {
                let h = a.parent().unwrap_or_else(|| a.clone());
                let hs = span(&h);
                let line_end = self.dtext[hs.0..hs.1]
                    .find(['\n', '\r'])
                    .map_or(hs.1, |k| hs.0 + k);
                let kind = Kind::Title {
                    headline: hs,
                    line: (hs.0, line_end),
                };
                (kind, span(&a))
            }
            TABLE_ROW => (Kind::Row, span(&a)),
            _ => (Kind::Other, span(&a)),
        };
        let before = self.dtext[..sp.0].trim_end_matches([' ', '\t']);
        let line_start = before.is_empty() || before.ends_with(['\n', '\r']);
        Unit {
            kind,
            span: sp,
            line_start,
        }
    }

    /// The macros Org's regexp finds in the value of keyword or property
    /// `n`.
    fn forced_in(&self, n: &SyntaxNode, out: &mut Vec<Occurrence>) {
        let Some(v) = n
            .children_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| t.kind() == VALUE)
        else {
            return;
        };
        let (a, b) = (
            usize::from(v.text_range().start()),
            usize::from(v.text_range().end()),
        );
        let found = raw_macros(&self.dtext, a, b);
        // Org's parser reads a macro's arguments past the line's end: the
        // unit runs to the end of the last macro.
        let end = found.iter().map(|f| f.1).max().unwrap_or(b).max(b);
        let unit = Unit {
            kind: Kind::Forced,
            span: (a, end),
            line_start: false,
        };
        for found in found {
            out.push(Occurrence { found, unit });
        }
    }
}

/// Whether keyword `n` is one the export parses (ox-html's).
fn is_parsed(n: &SyntaxNode) -> bool {
    Keyword::cast(n.clone()).is_some_and(|k| {
        let key = k.key().to_ascii_uppercase();
        PARSED_KEYWORDS.contains(&key.as_str())
    })
}
