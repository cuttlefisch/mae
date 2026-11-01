//! `org-macro-replace-all`, sequentially: each macro is judged against the
//! buffer as it stands when the scan reaches it (the expanded text before
//! it, the unexpanded text after), replaced, and the scan goes on from
//! where its expansion starts.
//!
//! The buffer is the text of its last full parse (`dtext`, parsed as `d`)
//! with the edits made since. The scan goes unit by unit: a unit is what
//! holds a macro's objects (a paragraph's or verse block's contents, a
//! caption's value, an item's tag, a headline's title, a table row, a
//! parsed keyword's value).
//! After each expansion the lines it touches are read before and after by
//! the parser's own line classifier ([`crate::element::same_line`]): if one
//! is now another kind of line (or the expansion takes lines its unit
//! cannot), what any later line is may have changed, and the whole buffer
//! is parsed again; otherwise only the unit's objects can have changed,
//! and the unit alone is read again from its start. Every read is charged
//! to the budget, so one unit with N macros costs N reads of it.

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
    /// The objects of a container: a paragraph's or verse block's contents
    /// (which may take the lines a macro's arguments span), a caption's
    /// value, an item's tag.
    Objects(crate::kind::SyntaxKind),
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
    /// Anything else: the whole buffer is read again after each expansion.
    Other,
}

/// The element holding a macro: its kind and span in `dtext`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Unit {
    kind: Kind,
    span: (usize, usize),
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
    /// The `#+KEY:` lines `{{{keyword}}}` reads instead of the parse's
    /// (a property value expanded apart from its document).
    pub(super) outer_lines: Option<std::rc::Rc<Vec<String>>>,
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
            outer_lines: None,
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
            work: self.work,
        }))
    }

    fn failed(&self, f: MacroFailure, a: usize, b: usize) -> RenderDiagnostic {
        RenderDiagnostic::MacroFailed(f, self.map.to_source(Span::new(a, b)))
    }

    /// Whether `p` is in a subtree an expansion made COMMENT. Those are
    /// pushed in scan order and none holds another (a macro inside one is
    /// not expanded), so they are sorted and disjoint.
    fn in_comment(&self, p: usize) -> bool {
        let i = self.commented.partition_point(|&(a, _)| a <= p);
        i > 0 && p < self.commented[i - 1].1
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
        // After each expansion the unit is read again from it: the next
        // macro is always the first one found.
        while let Some((r, e, name, args)) = cands.first().cloned() {
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
            let kept = self.keeps_lines(&c.text(&self.dtext), out, (r, e), &value, unit)?;
            c.splice((r, e), value, &self.dtext, &mut self.segment);
            self.charge(c.pending.len())?;
            if c.at + c.done.len() + c.pending.len() > self.size_limit {
                return Err((MacroFailure::Budget, here.0, here.1));
            }
            if !kept || unit.kind == Kind::Other {
                return Ok(self.reparse_flow(c, out));
            }
            if let Kind::Title { headline, line } = unit.kind {
                // A todo keyword or priority cookie the expansion makes
                // does not move the title for the next macro: Emacs 30.2
                // judges it in the title as it started (`r6-k10`).
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
            cands = self.reread_unit(&c, r)?;
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
            Kind::Objects(container) => object_macros(&crate::green::objects(&text, container)),
            Kind::Other => Vec::new(),
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

    /// Whether the lines the expansion of unit text `t[r..e]` into `value`
    /// touches are read as they were: the same kind of line
    /// ([`crate::element::same_line`]). A value
    /// holding a line ending makes lines, and a call whose arguments span
    /// lines takes them; only a paragraph's or verse block's contents can
    /// take lines (the lines a macro object spans are no element's start).
    fn keeps_lines(
        &mut self,
        t: &str,
        out: &str,
        (r, e): (usize, usize),
        value: &str,
        unit: Unit,
    ) -> Result<bool, Stop> {
        let eol = self.eol_char();
        let raw = &t[r..e];
        let takes = matches!(unit.kind, Kind::Objects(PARAGRAPH | VERSE_BLOCK));
        // No value holds a line ending today (arguments' blanks collapse,
        // a template is one line, Lisp is never evaluated): a guard.
        if value.contains(eol) || (raw.contains(eol) && !takes) {
            return Ok(false);
        }
        let mut prefix = String::new();
        match t[..r].rfind(eol) {
            Some(i) => prefix.push_str(&t[i + 1..r]),
            None => {
                prefix.push_str(&out[out.rfind(eol).map_or(0, |i| i + 1)..]);
                prefix.push_str(&t[..r]);
            }
        }
        let mut suffix = String::new();
        match t[e..].find(eol) {
            Some(i) => suffix.push_str(&t[e..e + i]),
            None => {
                let rest = &self.dtext[unit.span.1..];
                suffix.push_str(&t[e..]);
                suffix.push_str(&rest[..rest.find(eol).unwrap_or(rest.len())]);
            }
        }
        let before = format!("{prefix}{raw}{suffix}");
        let after = format!("{prefix}{value}{suffix}");
        self.charge(before.len() + after.len())?;
        let crlf = self.eol == LineEnding::CrLf;
        let content = |l: &'_ str| -> String {
            match l.strip_suffix('\r') {
                Some(c) if crlf => c.to_string(),
                _ => l.to_string(),
            }
        };
        // The first line: the call's bytes replaced by the value's, or,
        // when the call takes lines, the rest of the line by the rest of
        // the joined one. The lines it takes are inside its paragraph, so
        // they started no element.
        let (first, now) = (
            content(before.split(eol).next().unwrap_or("")),
            content(&after),
        );
        let at = prefix.len();
        let (old, new) = match raw.contains(eol) {
            false => (raw.len(), value.len()),
            true => (first.len() - at, now.len() - at),
        };
        Ok(crate::element::same_line(&first, &now, at, old, new))
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
            PARAGRAPH | VERSE_BLOCK => match self.d.contents(&a) {
                Some(c) if c.start <= Span::of(n).start && Span::of(n).end <= c.end => {
                    (Kind::Objects(a.kind()), (c.start as usize, c.end as usize))
                }
                _ => (Kind::Other, span(&a)),
            },
            AFFILIATED_VALUE | ITEM_TAG_CONTENT => (Kind::Objects(a.kind()), span(&a)),
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
        Unit { kind, span: sp }
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
