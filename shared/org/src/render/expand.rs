//! Org's macros, expanded as Org's export expands them (ADR-112 D1, D4).
//!
//! `org-export-as` (Org 9.7, `org-export--annotate-info`) works on a copy
//! of the buffer, in this order: it expands `#+INCLUDE:` (MAE reads no
//! file: an include stays a keyword, never rendered), deletes the COMMENT
//! subtrees, collects the macro templates (`org-macro-initialize-templates`),
//! replaces every macro by its expansion (`org-macro-replace-all`, with the
//! export's parsed keywords), and only then runs babel and parses what the
//! buffer has become. So does this module: [`prepare`] expands the text,
//! and the renderer renders the parse of the expanded text (whatever an
//! expansion made of it — a comment line, a COMMENT subtree, a `:noexport:`
//! tag, an inline src block completed by the text after the macro — is
//! read as Org reads it). [`crate::semantics`] stays on the source: the KB
//! projection is of what was written.
//!
//! As Org does:
//! - **Templates.** `#+MACRO: NAME TEMPLATE` lines outside COMMENT subtrees,
//!   the FIRST of a name winning (names compared case-insensitively, but
//!   collected case-sensitively, so `#+MACRO: Title x` does not override a
//!   `title` that `#+TITLE:` leaves empty); `author`, `email`, `title` and
//!   `date` from their keywords (`author` and `title` every line joined),
//!   overriding a `#+MACRO:` of the name when the keyword exists;
//!   `keyword`, `n`, `property` and `time` built in, after every
//!   `#+MACRO:`. `results` without a template is left for babel.
//! - **Where.** A macro is expanded where it is a macro object (not in
//!   verbatim, code, a src or example block, a comment, a link path, a
//!   property value…), not under a COMMENT headline, and anywhere in the
//!   value of a keyword the export parses (`TITLE`, `DATE`, `AUTHOR`,
//!   `SUBTITLE`: ox-html's) or of an `EXPORT_` property of one, where Org
//!   forces recognition. An expansion is scanned again where it stands, so
//!   the macros in it expand before the next one, and `{{{n}}}` counts in
//!   that order.
//! - **Arguments.** Trimmed, every run of blanks one space, split at each
//!   comma not escaped (`org-macro-extract-arguments`); `$N` is the N-th
//!   argument or nothing, and `$0` the FIRST (`(nth -1 args)`).
//! - **Failure.** Org's export stops with no output on an undefined macro
//!   or a circular expansion (the same macro, same arguments, at the same
//!   place again). So does this: the document renders nothing, with a
//!   [`RenderDiagnostic::MacroFailed`].
//!
//! Not as Org does (registered: `macros`):
//! - An `(eval …)` template, `{{{property}}}` with a search location or a
//!   special property, `{{{date(FORMAT)}}}` on a timestamp, a
//!   `format-time-string` directive outside the supported set: Org
//!   evaluates or looks up what MAE cannot from the text, so the document
//!   renders nothing ([`MacroFailure::Unsupported`]).
//! - `time` and `modification-time` format a FIXED time, 1970-01-01
//!   00:00:00 UTC, and `input-file` is empty: the output depends on the
//!   text only (ADR-112 D4). Each use is reported
//!   ([`RenderDiagnostic::MacroEnvironment`]).
//! - Expansion is bounded linearly in the source ([`WORK_PER_BYTE`],
//!   [`SIZE_PER_BYTE`]); past it the document renders nothing
//!   ([`MacroFailure::Budget`]), where Org would run (or loop) on.
//! - Org scans the buffer once, deciding at each macro whether it is one
//!   in the buffer as it is then; MAE decides on the parse of the text as
//!   it is before each round, and re-parses until no macro is left. An
//!   expansion that changes how LATER text parses (opens a comment block,
//!   makes a later headline COMMENT) can make Org skip a later macro MAE
//!   expands: a later `{{{n}}}` then counts differently, or a later
//!   undefined macro fails the document. What renders is still the parse
//!   of the expanded text.

use super::{MacroFailure, RenderDiagnostic};
use crate::kind::SyntaxKind::*;
use crate::kind::SyntaxNode;
use crate::views::{Headline, Keyword};
use crate::{Document, LineEnding, Span};
use rowan::WalkEvent;
use std::collections::{HashMap, HashSet};

/// The work an expansion may do: this much, plus [`WORK_PER_BYTE`] per
/// byte of source. Work is every byte parsed, scanned or produced.
pub(crate) const WORK_BASE: u64 = 1 << 22;
/// See [`WORK_BASE`].
pub(crate) const WORK_PER_BYTE: u64 = 64;
/// The size an expanded text may reach: this much, plus [`SIZE_PER_BYTE`]
/// per byte of source.
pub(crate) const SIZE_BASE: usize = 1 << 20;
/// See [`SIZE_BASE`].
pub(crate) const SIZE_PER_BYTE: usize = 32;
/// The keywords ox-html parses, where Org forces macro recognition.
pub(crate) const PARSED_KEYWORDS: &[&str] = &["TITLE", "DATE", "AUTHOR", "SUBTITLE"];

/// `org-special-properties` and `CATEGORY`: values Org computes, not
/// reads; of them only `ITEM` and `TODO` of a headline are read here.
const SPECIAL_PROPERTIES: &[&str] = &[
    "ALLTAGS",
    "BLOCKED",
    "CATEGORY",
    "CLOCKSUM",
    "CLOCKSUM_T",
    "CLOSED",
    "DEADLINE",
    "FILE",
    "PRIORITY",
    "SCHEDULED",
    "TAGS",
    "TIMESTAMP",
    "TIMESTAMP_IA",
];

#[cfg(test)]
thread_local! {
    /// Work charged by expansions (see [`WORK_BASE`]).
    pub(crate) static WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One piece of an expanded text that replaced a piece of the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Piece {
    src: (usize, usize),
    out: (usize, usize),
}

/// Where an expanded text came from: its replaced pieces, in order; the
/// text between them is copied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Map {
    pieces: Vec<Piece>,
}

impl Map {
    /// Position `p` of the source (`forward`) or of the expanded text in
    /// the other: exact in copied text; inside a replaced piece, the start
    /// of what it became (`start`), else its end.
    fn locate(&self, p: usize, start: bool, forward: bool) -> usize {
        let ends = |pc: &Piece| {
            if forward {
                (pc.src, pc.out)
            } else {
                (pc.out, pc.src)
            }
        };
        let k = self.pieces.partition_point(|pc| {
            let from = ends(pc).0;
            if start {
                from.0 <= p
            } else {
                from.0 < p
            }
        });
        let Some(pc) = k.checked_sub(1).map(|k| &self.pieces[k]) else {
            return p;
        };
        let (from, to) = ends(pc);
        let inside = if start { p < from.1 } else { p <= from.1 };
        if inside {
            if start {
                to.0
            } else {
                to.1
            }
        } else {
            to.1 + (p - from.1)
        }
    }

    /// The source span an expanded span came from.
    pub fn to_source(&self, s: Span) -> Span {
        let a = self.locate(s.start as usize, true, false);
        let b = self.locate(s.end as usize, false, false).max(a);
        Span::new(a, b)
    }

    /// The expanded span a source span became.
    pub fn to_expanded(&self, s: Span) -> Span {
        let a = self.locate(s.start as usize, true, true);
        let b = self.locate(s.end as usize, false, true).max(a);
        Span::new(a, b)
    }

    /// This map followed by one round's `edits` (`src`: a range of this
    /// map's expanded text; `out`: what it became). Each region of the
    /// round's input that an edit or an earlier piece covers (overlapping
    /// ones merged) becomes one piece, from where it came from.
    fn then(&self, edits: &[Piece]) -> Map {
        let step = Map {
            pieces: edits.to_vec(),
        };
        // Intervals of the round's input: (start, end, the empty earlier
        // piece it is, if it is one). Empty pieces first at a tie: one at
        // an edit's start is before it, not in it.
        let mut spans: Vec<(usize, usize, Option<Piece>)> = edits
            .iter()
            .map(|e| (e.src.0, e.src.1, None))
            .chain(self.pieces.iter().map(|pc| {
                let empty = (pc.out.0 == pc.out.1).then_some(*pc);
                (pc.out.0, pc.out.1, empty)
            }))
            .collect();
        spans.sort_by_key(|&(a, b, e)| (a, e.is_none(), b));
        let mut pieces = Vec::with_capacity(spans.len());
        let mut group: Option<(usize, usize)> = None;
        let flush = |g: (usize, usize), pieces: &mut Vec<Piece>| {
            pieces.push(Piece {
                src: (
                    self.locate(g.0, true, false),
                    self.locate(g.1, false, false),
                ),
                out: (step.locate(g.0, true, true), step.locate(g.1, false, true)),
            })
        };
        for (a, b, empty) in spans {
            match (empty, group) {
                (Some(_), Some((g0, g1))) if g0 < a && a < g1 => {}
                (Some(pc), g) => {
                    if let Some(g) = g.filter(|g| g.1 <= a) {
                        flush(g, &mut pieces);
                        group = None;
                    }
                    let x = step.locate(a, true, true);
                    pieces.push(Piece {
                        src: pc.src,
                        out: (x, x),
                    });
                }
                (None, Some((g0, g1))) if a < g1 => group = Some((g0, g1.max(b))),
                (None, g) => {
                    if let Some(g) = g {
                        flush(g, &mut pieces);
                    }
                    group = Some((a, b));
                }
            }
        }
        if let Some(g) = group {
            flush(g, &mut pieces);
        }
        Map { pieces }
    }
}

/// A source text expanded: the text, where it came from, and what was
/// substituted for the environment.
#[derive(Debug, Clone)]
pub(crate) struct Expansion {
    pub text: String,
    pub map: Map,
    pub notes: Vec<RenderDiagnostic>,
}

/// A template, as `org-macro-initialize-templates` installs it.
#[derive(Debug, Clone)]
enum Template {
    Text(String),
    /// `(eval …)`: Org evaluates it.
    Eval,
    /// `date` when `#+DATE:` is one timestamp: `{{{date(FORMAT)}}}`
    /// formats it (unsupported), `{{{date}}}` is the value.
    Date(String),
    Keyword,
    N,
    Property,
    Time,
    InputFile,
    ModificationTime,
}

/// One macro of a round's text.
#[derive(Debug, Clone)]
struct Occurrence {
    start: usize,
    /// Its end, post-blank excluded (Org keeps the blanks after a macro).
    end: usize,
    name: String,
    /// The text between the parentheses; `None` without them.
    args: Option<String>,
    /// In a parsed keyword or `EXPORT_` property, where Org finds macros
    /// by its regexp, not as objects.
    forced: bool,
    /// The span of the headline whose title holds it.
    title_of: Option<(usize, usize)>,
}

/// Why an expansion stopped, and where (in the round's text).
type Stop = (MacroFailure, usize, usize);

/// A macro found in a text: its start and end, name and raw arguments.
type Found = (usize, usize, String, Option<String>);

/// Expands the macros of `doc`, or `Ok(None)` when it has none to expand.
pub(crate) fn expand(doc: &Document) -> Result<Option<Expansion>, RenderDiagnostic> {
    let source = doc.syntax().to_string();
    let engine = Engine::new(doc, source.len(), templates(doc, &source));
    engine.run(doc, source, Map::default(), HashSet::new())
}

/// Org's second expansion, after babel (`org-macro-replace-all` with only
/// `results` as `$1`): every results macro of `doc` (the parse of an
/// expansion whose map is `map`, of a source of `source_len` bytes) that
/// babel keeps becomes its first argument, in the text, so what it becomes
/// is read as Org reads it (a `:noexport:` tag, a COMMENT keyword). Those
/// babel removes (starting at an offset in `removed`) stay, for the
/// renderer to remove. Any other macro then found is undefined, as in Org.
pub(crate) fn expand_results(
    doc: &Document,
    map: Map,
    source_len: usize,
    removed: HashSet<usize>,
) -> Result<Option<Expansion>, RenderDiagnostic> {
    let text = doc.syntax().to_string();
    let templates = HashMap::from([("results".to_string(), Template::Text("$1".into()))]);
    Engine::new(doc, source_len, templates).run(doc, text, map, removed)
}

impl Engine {
    /// Rounds over `text` (parsed as `doc`; `map` from the source) until
    /// no macro is left to expand, skipping the occurrences starting in
    /// `skip`. `Ok(None)` when there was none.
    fn run(
        mut self,
        doc: &Document,
        mut text: String,
        mut map: Map,
        mut skip: HashSet<usize>,
    ) -> Result<Option<Expansion>, RenderDiagnostic> {
        let mut found = occurrences(&doc.syntax(), &self.templates);
        found.retain(|o| !skip.contains(&o.start));
        if found.is_empty() {
            return Ok(None);
        }
        let mut round_doc: Option<Document> = None;
        let mut notes = Vec::new();
        loop {
            let root = round_doc.as_ref().unwrap_or(doc).syntax();
            let (next, pieces) = self
                .round(&root, &text, &found)
                .map_err(|(f, a, b)| failed(&map, f, a, b))?;
            for &(a, b) in &self.notes {
                notes.push(RenderDiagnostic::MacroEnvironment(
                    map.to_source(Span::new(a, b)),
                ));
            }
            self.notes.clear();
            map = map.then(&pieces);
            let step = Map { pieces };
            skip = skip
                .into_iter()
                .map(|s| step.locate(s, true, true))
                .collect();
            text = next;
            let whole = |f| failed(&map, f, 0, text.len());
            if text.len() > self.size_limit {
                return Err(whole(MacroFailure::Budget));
            }
            if step.pieces.is_empty() {
                break;
            }
            self.charge(text.len()).map_err(|(f, ..)| whole(f))?;
            let d = doc.reparse(&text);
            found = occurrences(&d.syntax(), &self.templates);
            found.retain(|o| !skip.contains(&o.start));
            round_doc = Some(d);
            if found.is_empty() {
                break;
            }
        }
        Ok(Some(Expansion { text, map, notes }))
    }
}

fn failed(map: &Map, f: MacroFailure, a: usize, b: usize) -> RenderDiagnostic {
    RenderDiagnostic::MacroFailed(f, map.to_source(Span::new(a, b)))
}

/// Every macro Org's export expands in the parse `root`, in order: macro
/// objects and macros in parsed keywords, not under a COMMENT headline,
/// and not a `results` macro left to babel.
fn occurrences(root: &SyntaxNode, templates: &HashMap<String, Template>) -> Vec<Occurrence> {
    let text = root.to_string();
    let mut out = Vec::new();
    let mut walk = root.preorder();
    while let Some(ev) = walk.next() {
        let WalkEvent::Enter(n) = ev else { continue };
        match n.kind() {
            HEADLINE if Headline::cast(n.clone()).is_some_and(|h| h.is_commented()) => {
                walk.skip_subtree()
            }
            MACRO => out.extend(object_occurrence(&n)),
            KEYWORD => {
                let key = Keyword::cast(n.clone()).map(|k| k.key().to_ascii_uppercase());
                if key.is_some_and(|k| PARSED_KEYWORDS.contains(&k.as_str())) {
                    forced_in(&n, &text, &mut out);
                }
            }
            NODE_PROPERTY if is_export_property(&n) => forced_in(&n, &text, &mut out),
            _ => {}
        }
    }
    out.retain(|o| templates.contains_key(&fold(&o.name)) || fold(&o.name) != "results");
    out
}

/// A macro object's occurrence (post-blank excluded).
fn object_occurrence(n: &SyntaxNode) -> Option<Occurrence> {
    let start = usize::from(n.text_range().start());
    let mut name = None;
    let mut args = None;
    let mut end = start;
    let mut open = false;
    // The arguments are every token between the parentheses (a value
    // across lines is several).
    for t in n.children_with_tokens().filter_map(|e| e.into_token()) {
        match t.kind() {
            NAME => name = Some(t.text().to_string()),
            MARKUP if t.text() == "(" => open = true,
            MARKUP => {}
            WHITESPACE if !open || t.text_range().end() == n.text_range().end() => continue,
            _ if open => args.get_or_insert_with(String::new).push_str(t.text()),
            _ => continue,
        }
        end = usize::from(t.text_range().end());
    }
    let title_of = n
        .ancestors()
        .find(|a| a.kind() == HEADLINE_TITLE)
        .and_then(|t| t.parent())
        .map(|h| {
            (
                usize::from(h.text_range().start()),
                usize::from(h.text_range().end()),
            )
        });
    Some(Occurrence {
        start,
        end,
        name: name?,
        args: if open {
            Some(args.unwrap_or_default())
        } else {
            None
        },
        forced: false,
        title_of,
    })
}

/// Whether node property `n` is an `EXPORT_` property of a parsed keyword
/// (`EXPORT_TITLE`, `EXPORT_TITLE+`, any case).
pub(crate) fn is_export_property(n: &SyntaxNode) -> bool {
    let key = n
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == NAME)
        .map(|t| t.text().to_ascii_uppercase())
        .unwrap_or_default();
    let key = key.strip_suffix('+').unwrap_or(&key);
    key.strip_prefix("EXPORT_")
        .is_some_and(|k| PARSED_KEYWORDS.contains(&k))
}

/// The macros Org's regexp finds in the value of keyword or property `n`
/// of `text` (a macro's arguments may run past the line, as Org's parser
/// reads them).
fn forced_in(n: &SyntaxNode, text: &str, out: &mut Vec<Occurrence>) {
    let Some(v) = n
        .children_with_tokens()
        .filter_map(|e| e.into_token())
        .find(|t| t.kind() == VALUE)
    else {
        return;
    };
    let r = v.text_range();
    for (a, b, name, args) in raw_macros(text, r.start().into(), r.end().into()) {
        out.push(Occurrence {
            start: a,
            end: b,
            name,
            args,
            forced: true,
            title_of: None,
        });
    }
}

/// `org-element-macro-parser` at `i` of `s`: `{{{NAME}}}` or
/// `{{{NAME(ARGS)}}}`, NAME `[a-zA-Z][-a-zA-Z0-9_]*`, ARGS the shortest
/// text without a NUL before `)}}}`. Its end, name and arguments.
fn macro_at(s: &str, i: usize) -> Option<(usize, String, Option<String>)> {
    let b = s.as_bytes();
    let n = i + 3;
    if !s[i..].starts_with("{{{") || !b.get(n).is_some_and(u8::is_ascii_alphabetic) {
        return None;
    }
    let ne = n + b[n..]
        .iter()
        .take_while(|c| c.is_ascii_alphanumeric() || **c == b'-' || **c == b'_')
        .count();
    let name = s[n..ne].to_string();
    if b.get(ne) == Some(&b'(') {
        if let Some(k) = s[ne + 1..].find(")}}}") {
            let args = &s[ne + 1..ne + 1 + k];
            if !args.contains('\0') {
                return Some((ne + 1 + k + 4, name, Some(args.to_string())));
            }
        }
    }
    s[ne..].starts_with("}}}").then(|| (ne + 3, name, None))
}

/// Every macro `org-macro-replace-all`'s regexp (`{{{[-A-Za-z0-9_]`)
/// finds starting in `s[from..to]`, read by the macro parser on `s`, in
/// order. After one that is left (a `results` macro without template) the
/// search goes on inside it, as Org's does; after any other it goes on
/// after it.
fn raw_macros(s: &str, from: usize, to: usize) -> Vec<Found> {
    let mut out = Vec::new();
    let mut i = from;
    while let Some(k) = s.get(i..to).and_then(|r| r.find("{{{")) {
        let at = i + k;
        let next = s[at + 3..].chars().next();
        if !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            i = at + 3;
            continue;
        }
        match macro_at(s, at) {
            Some((end, name, args)) => {
                let left = fold(&name) == "results";
                out.push((at, end, name, args));
                i = if left { at + 4 } else { end };
            }
            None => i = at + 4,
        }
    }
    out
}

/// How much of a line around a macro is read to decide what it is.
const WINDOW: usize = 256;

/// The last (`tail`) or first [`WINDOW`] bytes of `s[from..]` (`tail`) or
/// `s[..from]`, cut at a character boundary.
fn window(s: &str, from: usize, tail: bool) -> &str {
    if tail {
        let mut a = from.max(s.len().saturating_sub(WINDOW));
        while !s.is_char_boundary(a) {
            a += 1;
        }
        &s[a..]
    } else {
        let mut b = from.min(WINDOW);
        while !s.is_char_boundary(b) {
            b -= 1;
        }
        &s[..b]
    }
}

fn fold(s: &str) -> String {
    crate::chars::fold(s)
}

/// `org-element-macro-parser`'s arguments then `org-macro-extract-arguments`:
/// the text trimmed, each run of blanks (space, tab, newline) one space,
/// split at each comma not escaped (a run of N backslashes before a comma
/// halved, an odd run escaping it). `None` without parentheses.
pub(crate) fn extract_arguments(raw: Option<&str>) -> Option<Vec<String>> {
    let raw = raw?.trim_matches(crate::text::is_trim);
    let mut collapsed = String::with_capacity(raw.len());
    for c in raw.chars() {
        let blank = matches!(c, ' ' | '\t' | '\n');
        if !(blank && collapsed.ends_with(' ')) {
            collapsed.push(if blank { ' ' } else { c });
        }
    }
    let mut args = vec![String::new()];
    let mut slashes = 0usize;
    for c in collapsed.chars() {
        let cur = args.last_mut().expect("one argument at least");
        match c {
            '\\' => slashes += 1,
            ',' => {
                cur.extend(std::iter::repeat_n('\\', slashes / 2));
                if slashes % 2 == 1 {
                    cur.push(',');
                } else {
                    args.push(String::new());
                }
                slashes = 0;
            }
            c => {
                cur.extend(std::iter::repeat_n('\\', slashes));
                slashes = 0;
                cur.push(c);
            }
        }
    }
    let cur = args.last_mut().expect("one argument at least");
    cur.extend(std::iter::repeat_n('\\', slashes));
    Some(args)
}

/// `org-macro-expand` on a text template: each `$N` the N-th argument or
/// nothing, `$0` the first (`(nth (1- 0) args)` is `(nth -1 args)`).
pub(crate) fn substitute(template: &str, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(k) = rest.find('$') {
        out.push_str(&rest[..k]);
        let digits = rest[k + 1..].bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            out.push('$');
            rest = &rest[k + 1..];
            continue;
        }
        let n = rest[k + 1..k + 1 + digits].parse::<usize>().ok();
        let i = n.map(|n| n.saturating_sub(1));
        if let Some(a) = i.and_then(|i| args.get(i)) {
            out.push_str(a);
        }
        rest = &rest[k + 1 + digits..];
    }
    out.push_str(rest);
    out
}

/// `(string-match-p "\\`(eval\\>" value)`: a template Org evaluates.
fn is_eval(t: &str) -> bool {
    t.strip_prefix("(eval")
        .is_some_and(|rest| !rest.starts_with(crate::chars::is_word))
}

/// `format-time-string` of the fixed time, Thursday 1970-01-01 00:00:00
/// UTC, in the C locale. `None` for a directive outside the supported set
/// (a flag, a width, `%n`, a locale's date or time).
pub(crate) fn epoch_format(fmt: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        out.push_str(match chars.next()? {
            'Y' | 'G' => "1970",
            'y' | 'g' => "70",
            'C' => "19",
            'm' | 'd' => "01",
            'e' => " 1",
            'j' => "001",
            'H' | 'M' | 'S' | 'U' | 'W' => "00",
            'I' | 'l' => "12",
            'k' => " 0",
            'p' => "AM",
            'a' => "Thu",
            'A' => "Thursday",
            'b' | 'h' => "Jan",
            'B' => "January",
            'u' | 'w' => "4",
            'V' => "01",
            'Z' => "UTC",
            'z' => "+0000",
            's' => "0",
            'N' => "000000000",
            'T' => "00:00:00",
            'R' => "00:00",
            'D' => "01/01/70",
            'F' => "1970-01-01",
            't' => "\t",
            '%' => "%",
            _ => return None,
        });
    }
    Some(out)
}

/// What a round reads besides the occurrences: the round's text and parse.
struct Round<'r> {
    root: &'r SyntaxNode,
    text: &'r str,
    /// The `#+…` lines outside COMMENT subtrees (for `keyword`).
    lines: Option<Vec<&'r str>>,
    keywords: HashMap<String, Option<String>>,
    properties: HashMap<usize, Vec<(String, String)>>,
}

struct Engine {
    templates: HashMap<String, Template>,
    counters: HashMap<String, u64>,
    record: HashSet<(usize, String)>,
    work: u64,
    work_limit: u64,
    size_limit: usize,
    /// The occurrences (in the round's text) whose expansion took a value
    /// from the environment.
    notes: Vec<(usize, usize)>,
    /// Whether the occurrence being expanded did.
    environment: bool,
    todo: Vec<crate::TodoSequence>,
    eol: LineEnding,
}

impl Engine {
    fn new(doc: &Document, source_len: usize, templates: HashMap<String, Template>) -> Engine {
        Engine {
            templates,
            counters: HashMap::new(),
            record: HashSet::new(),
            work: 0,
            work_limit: WORK_BASE + WORK_PER_BYTE * source_len as u64,
            size_limit: SIZE_BASE + SIZE_PER_BYTE * source_len,
            notes: Vec::new(),
            environment: false,
            todo: doc.todo_sequences().to_vec(),
            eol: doc.line_ending(),
        }
    }

    fn charge(&mut self, n: usize) -> Result<(), Stop> {
        self.work += n as u64 + 1;
        #[cfg(test)]
        WORK.with(|w| w.set(w.get() + n as u64 + 1));
        if self.work > self.work_limit {
            return Err((MacroFailure::Budget, 0, 0));
        }
        Ok(())
    }

    /// One round: the occurrences of `text` (parsed as `root`) expanded in
    /// order, each where it then stands, as `org-macro-replace-all` does;
    /// the new text and its pieces.
    fn round(
        &mut self,
        root: &SyntaxNode,
        text: &str,
        found: &[Occurrence],
    ) -> Result<(String, Vec<Piece>), Stop> {
        let mut r = Round {
            root,
            text,
            lines: None,
            keywords: HashMap::new(),
            properties: HashMap::new(),
        };
        let nl = self.eol_char();
        let mut out = String::with_capacity(text.len());
        let mut pieces: Vec<Piece> = Vec::new();
        let mut pos = 0;
        // Where the output's current line starts.
        let mut line_out = 0;
        let mut commented: Option<(usize, usize)> = None;
        for (i, o) in found.iter().enumerate() {
            if o.start < pos || commented.is_some_and(|(a, b)| o.start >= a && o.end <= b) {
                continue;
            }
            let chunk = &text[pos..o.start];
            if let Some(k) = chunk.rfind(nl) {
                line_out = out.len() + k + 1;
            }
            out.push_str(chunk);
            pos = o.start;
            let line_end = text[o.end..].find(nl).map_or(text.len(), |k| o.end + k);
            let right = &text[o.end..line_end];
            // An expansion earlier on the line may have changed what this
            // is (`{{{id(=)}}}{{{m}}}=` makes it verbatim): read it again.
            let line_start = text[..o.start].rfind(nl).map_or(0, |k| k + 1);
            let dirty = pieces.last().is_some_and(|p| p.src.0 >= line_start);
            if dirty && !o.forced {
                let mid = &text[o.start..o.end];
                let left = window(&out, line_out, true);
                let still = self.first_macro(left, mid, right, false)?;
                if still.is_none_or(|m| m.0 != 0 || m.1 != mid.len()) {
                    continue;
                }
            }
            let at = out.len();
            self.expand_here(&mut r, o, &mut out, line_out, right)
                .map_err(|(f, ..)| (f, o.start, o.end))?;
            pieces.push(Piece {
                src: (o.start, o.end),
                out: (at, out.len()),
            });
            pos = o.end;
            if std::mem::take(&mut self.environment) {
                self.notes.push((o.start, o.end));
            }
            // Org skips the macros under a headline an expansion made
            // COMMENT: once its title's macros are expanded, read it again.
            let next_same = found.get(i + 1).is_some_and(|n| n.title_of == o.title_of);
            if let (Some(h), false) = (o.title_of, next_same) {
                let line = format!("{}{right}", &out[line_out..]);
                if self.is_commented_headline(&line)? {
                    commented = Some(h);
                }
            }
        }
        out.push_str(&text[pos..]);
        Ok((out, pieces))
    }

    /// The character that ends a line in this document.
    fn eol_char(&self) -> char {
        match self.eol {
            LineEnding::Cr => '\r',
            _ => '\n',
        }
    }

    /// Whether headline line `line` reads as COMMENT.
    fn is_commented_headline(&mut self, line: &str) -> Result<bool, Stop> {
        self.charge(line.len())?;
        let d = crate::parse(&format!("{line}\n"), &self.parse_options());
        Ok(d.headlines().next().is_some_and(|h| h.is_commented()))
    }

    fn parse_options(&self) -> crate::ParseOptions {
        crate::ParseOptions {
            todo_sequences: self.todo.clone(),
            ..crate::ParseOptions::default()
        }
    }

    /// Occurrence `o` replaced, in `out`, by its expansion: inserted, then
    /// read again from where it starts, every macro found there expanded
    /// in turn, as `org-macro-replace-all` leaves point before what it
    /// inserts. `line_out` is where `out`'s line starts; `right` is the
    /// rest of the line after the occurrence.
    fn expand_here(
        &mut self,
        r: &mut Round,
        o: &Occurrence,
        out: &mut String,
        line_out: usize,
        right: &str,
    ) -> Result<(), Stop> {
        let raw = &r.text[o.start..o.end];
        let mut pending = self.call(r, (&o.name, o.args.as_deref(), raw), o.start, out.len())?;
        loop {
            if !pending.contains("{{{") {
                out.push_str(&pending);
                return Ok(());
            }
            let left = window(out, line_out, true);
            let Some((a, b, name, args)) = self.first_macro(left, &pending, right, o.forced)?
            else {
                out.push_str(&pending);
                return Ok(());
            };
            out.push_str(&pending[..a]);
            let raw = pending[a..b].to_string();
            let v = self.call(r, (&name, args.as_deref(), &raw), o.start, out.len())?;
            pending = format!("{v}{}", &pending[b..]);
            self.charge(pending.len())?;
            if out.len() + pending.len() > self.size_limit {
                return Err((MacroFailure::Budget, 0, 0));
            }
        }
    }

    /// The first macro Org's export expands that starts in `mid` and ends
    /// in it, where `mid` stands between `left` and `right` on its line: a
    /// macro object of the line's parse, or (`forced`, in a parsed
    /// keyword) a macro Org's regexp finds. Offsets in `mid`.
    fn first_macro(
        &mut self,
        left: &str,
        mid: &str,
        right: &str,
        forced: bool,
    ) -> Result<Option<Found>, Stop> {
        let right = window(right, right.len(), false);
        let line = format!("{left}{mid}{right}");
        self.charge(line.len())?;
        let (l, m) = (left.len(), left.len() + mid.len());
        let left_to_babel =
            |n: &str| fold(n) == "results" && !self.templates.contains_key("results");
        let found: Vec<Found> = if forced {
            raw_macros(&line, 0, line.len())
        } else {
            // The line, then a paragraph: an affiliated keyword (a caption)
            // needs an element after it to be one.
            let d = crate::parse(&format!("{line}\nx\n"), &self.parse_options());
            d.syntax()
                .descendants()
                .filter(|n| n.kind() == MACRO)
                .filter_map(|n| object_occurrence(&n))
                .map(|o| (o.start, o.end, o.name, o.args))
                .collect()
        };
        Ok(found
            .into_iter()
            .filter(|(a, b, name, _)| *a >= l && *a < m && *b <= m && !left_to_babel(name))
            .map(|(a, b, name, args)| (a - l, b - l, name, args))
            .next())
    }

    /// The value of the macro `name` with raw arguments `args` (its source
    /// `raw`, standing at `at` of the output; `place` is where its
    /// occurrence is in the round's text, for `property`).
    fn call(
        &mut self,
        r: &mut Round,
        (name, args, raw): (&str, Option<&str>, &str),
        place: usize,
        at: usize,
    ) -> Result<String, Stop> {
        let key = fold(name);
        let Some(template) = self.templates.get(&key).cloned() else {
            return Err((MacroFailure::Undefined, 0, 0));
        };
        // `org-macro-replace-all`: the same macro, same arguments, at the
        // same place again is a cycle.
        if !self.record.insert((at, raw.to_string())) {
            return Err((MacroFailure::Circular, 0, 0));
        }
        self.charge(raw.len())?;
        let value = self.value(r, &template, extract_arguments(args), place)?;
        self.charge(value.len())?;
        Ok(value)
    }

    /// A template's value for `args` (`None`: no parentheses).
    fn value(
        &mut self,
        r: &mut Round,
        template: &Template,
        args: Option<Vec<String>>,
        place: usize,
    ) -> Result<String, Stop> {
        let unsupported = Err((MacroFailure::Unsupported, 0, 0));
        let all = args.unwrap_or_default();
        let arg = |i: usize| all.get(i).map(String::as_str);
        let nonblank =
            |s: Option<&str>| s.is_some_and(|s| !s.trim_matches(crate::text::is_trim).is_empty());
        Ok(match template {
            Template::Text(t) => substitute(t, &all),
            Template::Eval => return Err((MacroFailure::Eval, 0, 0)),
            Template::Date(_) if nonblank(arg(0)) => return unsupported,
            Template::Date(v) => v.clone(),
            Template::Keyword => match arg(0) {
                Some(name) if !name.contains(crate::chars::is_space) => {
                    self.keyword(r, name)?.unwrap_or_default()
                }
                _ => return unsupported,
            },
            Template::N => self.counter(arg(0), arg(1))?,
            Template::Property => match (arg(0), nonblank(arg(1))) {
                (Some(p), false) => self.property(r, p, place)?,
                _ => return unsupported,
            },
            Template::Time | Template::ModificationTime => {
                let Some(f) = arg(0).and_then(epoch_format) else {
                    return unsupported;
                };
                self.environment = true;
                f
            }
            Template::InputFile => {
                self.environment = true;
                String::new()
            }
        })
    }

    /// `org-macro--find-keyword-value NAME t` on the round's text.
    fn keyword(&mut self, r: &mut Round, name: &str) -> Result<Option<String>, Stop> {
        let key = fold(name);
        if let Some(v) = r.keywords.get(&key) {
            return Ok(v.clone());
        }
        if r.lines.is_none() {
            r.lines = Some(keyword_lines(r.root, r.text, self.eol));
        }
        let lines = r.lines.as_deref().unwrap_or_default();
        self.charge(lines.len())?;
        let v = crate::semantics::keywords::keyword_value(lines.iter().copied(), name, true);
        r.keywords.insert(key, v.clone());
        Ok(v)
    }

    /// `org-macro--counter-increment NAME ACTION`.
    fn counter(&mut self, name: Option<&str>, action: Option<&str>) -> Result<String, Stop> {
        let trim = |s: &str| s.trim_matches(crate::text::is_trim).to_string();
        let name = name.map(trim).unwrap_or_default();
        let action = action.map(trim).filter(|a| !a.is_empty());
        let cur = self.counters.get(&name).copied();
        let v = match action.as_deref() {
            None => cur.unwrap_or(0).checked_add(1),
            Some("-") => Some(cur.unwrap_or(1)),
            Some(a) if a.bytes().all(|b| b.is_ascii_digit()) => a.parse::<u64>().ok(),
            Some(_) => Some(1),
        };
        let Some(v) = v else {
            return Err((MacroFailure::Unsupported, 0, 0));
        };
        self.counters.insert(name, v);
        Ok(v.to_string())
    }

    /// `org-entry-get nil PROPERTY 'selective` at `place`: the property of
    /// the entry (headline, or the file before the first) holding it, not
    /// inherited (`org-use-property-inheritance` is nil); `KEY+` lines
    /// appended, `nil` none. A special property fails.
    fn property(&mut self, r: &mut Round, property: &str, place: usize) -> Result<String, Stop> {
        let key = crate::views::canonical_key(property);
        if SPECIAL_PROPERTIES.contains(&key.as_str()) {
            return Err((MacroFailure::Unsupported, 0, 0));
        }
        let entry = entry_at(r.root, place);
        if matches!(key.as_str(), "ITEM" | "TODO") {
            // `org-get-heading t t t t`, the TODO keyword; at the top of
            // the file Org reads neither.
            let h = entry.and_then(Headline::cast);
            let Some(h) = h else {
                return Err((MacroFailure::Unsupported, 0, 0));
            };
            return Ok(match key.as_str() {
                "ITEM" => h.title(),
                _ => h.todo().unwrap_or_default(),
            });
        }
        let start = entry
            .as_ref()
            .map_or(0, |e| usize::from(e.text_range().start()) + 1);
        if !r.properties.contains_key(&start) {
            let pairs = entry_properties(r.root, entry.as_ref());
            self.charge(pairs.iter().map(|(k, v)| k.len() + v.len()).sum())?;
            let resolved = crate::semantics::resolve_properties(pairs);
            r.properties.insert(start, resolved);
        }
        let props = &r.properties[&start];
        Ok(props
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.clone())
            .unwrap_or_default())
    }
}

/// The headline whose entry holds `place` (`None`: before the first).
fn entry_at(root: &SyntaxNode, place: usize) -> Option<SyntaxNode> {
    let offset = rowan::TextSize::try_from(place).ok()?;
    if offset >= root.text_range().end() {
        return None;
    }
    let token = root.token_at_offset(offset).right_biased()?;
    token.parent_ancestors().find(|a| a.kind() == HEADLINE)
}

/// The property lines of an entry's drawer, values trimmed.
fn entry_properties(root: &SyntaxNode, entry: Option<&SyntaxNode>) -> Vec<(String, String)> {
    let drawer = match entry {
        Some(h) => Headline::cast(h.clone()).and_then(|h| h.property_drawer()),
        None => root
            .children()
            .next()
            .filter(|s| s.kind() == SECTION)
            .and_then(|s| s.children().find_map(crate::PropertyDrawer::cast)),
    };
    drawer
        .map(|d| d.properties())
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let v = crate::link_grammar::org_trim(&p.value).to_string();
            (p.key, v)
        })
        .collect()
}

/// The lines of `text` that start (after blanks) with `#+`, outside every
/// COMMENT subtree of its parse `root`.
fn keyword_lines<'t>(root: &SyntaxNode, text: &'t str, eol: LineEnding) -> Vec<&'t str> {
    let mut skip = Vec::new();
    let mut walk = root.preorder();
    while let Some(ev) = walk.next() {
        let WalkEvent::Enter(n) = ev else { continue };
        if Headline::cast(n.clone()).is_some_and(|h| h.is_commented()) {
            let r = n.text_range();
            skip.push((usize::from(r.start()), usize::from(r.end())));
            walk.skip_subtree();
        }
    }
    let mut out = Vec::new();
    let mut at = 0;
    let mut k = 0;
    for line in super::build::lines(text, eol) {
        while k < skip.len() && skip[k].1 <= at {
            k += 1;
        }
        let skipped = skip.get(k).is_some_and(|&(a, b)| at >= a && at < b);
        let l = super::build::trim_eol(line, eol);
        if !skipped && l.trim_start_matches([' ', '\t']).starts_with("#+") {
            out.push(l);
        }
        at += line.len();
    }
    out
}

/// The templates `org-macro-initialize-templates` installs for `doc`
/// (its text `source`), as the map a lookup reads: folded name → the
/// template the first entry of that name (any case) gives.
fn templates(doc: &Document, source: &str) -> HashMap<String, Template> {
    let root = doc.syntax();
    let lines = keyword_lines(&root, source, doc.line_ending());
    let kw = |name: &str, collect| {
        crate::semantics::keywords::keyword_value(lines.iter().copied(), name, collect)
    };
    let text = |v: String| {
        if is_eval(&v) {
            Template::Eval
        } else {
            Template::Text(v)
        }
    };
    let date = kw("DATE", false).map(|v| {
        let tree = crate::green::objects(&v, PARAGRAPH);
        let mut parts = tree.children_with_tokens();
        let one = parts.next().and_then(|p| p.into_node());
        let stamp = one.is_some_and(|n| n.kind() == TIMESTAMP) && parts.next().is_none();
        if stamp {
            Template::Date(v)
        } else {
            text(v)
        }
    });
    // `org-macro--collect-macros`: the keyword built-ins, then each
    // `#+MACRO:` pushed in front, so the list runs from the LAST line.
    let mut list: Vec<(String, Option<Template>)> = Vec::new();
    for k in crate::semantics::keywords::outside_comment_trees(&root) {
        if !k.key().eq_ignore_ascii_case("MACRO") {
            continue;
        }
        let value = k.value();
        let v = value.trim_start_matches(crate::chars::is_space);
        let end = v.find(crate::chars::is_space).unwrap_or(v.len());
        if end == 0 {
            continue;
        }
        let def = v[end..].trim_start_matches([' ', '\t']);
        list.push((v[..end].to_string(), Some(text(def.to_string()))));
    }
    list.reverse();
    list.push(("author".into(), kw("AUTHOR", true).map(text)));
    list.push(("email".into(), kw("EMAIL", false).map(text)));
    list.push(("title".into(), kw("TITLE", true).map(text)));
    list.push(("date".into(), date));
    // `org-macro--set-templates`: a later entry of the SAME name (case
    // sensitive) overrides an earlier one when it has a value; a new name
    // is pushed in front.
    let mut set: Vec<(String, Template)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for (name, value) in list {
        match (at.get(&name), value) {
            (Some(&i), Some(v)) => set[i].1 = v,
            (Some(_), None) => {}
            (None, v) => {
                at.insert(name.clone(), set.len());
                set.push((name, v.unwrap_or(Template::Text(String::new()))));
            }
        }
    }
    let builtins = [
        ("input-file", Template::InputFile),
        ("modification-time", Template::ModificationTime),
        ("keyword", Template::Keyword),
        ("n", Template::N),
        ("property", Template::Property),
        ("time", Template::Time),
    ];
    let mut map = HashMap::new();
    // `set` was pushed in front: the lookup reads it from the end.
    for (name, t) in set.into_iter().rev() {
        map.entry(fold(&name)).or_insert(t);
    }
    for (name, t) in builtins {
        map.entry(name.to_string()).or_insert(t);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    /// `org-macro-extract-arguments` after the parser's trim and collapse,
    /// and `org-macro-expand`, on Emacs 30.2's answers.
    #[test]
    fn arguments_split_and_substitute_as_org_does() {
        let x = |s: &str| extract_arguments(Some(s)).unwrap();
        assert_eq!(x("a,b"), v(&["a", "b"]));
        assert_eq!(x("a\\,b,c"), v(&["a,b", "c"]));
        assert_eq!(x("a\\\\,b"), v(&["a\\", "b"]));
        assert_eq!(x("  x \n  y ,z "), v(&["x y ", "z"]));
        assert_eq!(x(""), v(&[""]));
        assert_eq!(x(",,"), v(&["", "", ""]));
        assert_eq!(extract_arguments(None), None);
        let args = v(&["src", "_sh{x}"]);
        assert_eq!(substitute("$1$2 $3.", &args), "src_sh{x} .");
        assert_eq!(substitute("$10", &args), "");
        assert_eq!(substitute("$ and $x", &args), "$ and $x");
        // `(nth -1 args)` is the first argument (review round 4).
        assert_eq!(substitute("$0$2", &args), "src_sh{x}");
        assert_eq!(substitute("$0", &[]), "");
        assert_eq!(substitute("$99999999999999999999999", &args), "");
    }

    /// Emacs 30.2: `(format-time-string F 0 t)` under `LC_ALL=C`.
    #[test]
    fn the_fixed_time_formats_as_emacs_formats_the_epoch() {
        let cases = [
            ("%Y-%m-%d %H:%M:%S", "1970-01-01 00:00:00"),
            (
                "%y %C %G %g %e|%j %k %I %l %p",
                "70 19 1970 70  1|001  0 12 12 AM",
            ),
            (
                "%a %A %b %h %B %u %w %U %W %V",
                "Thu Thursday Jan Jan January 4 4 00 00 01",
            ),
            (
                "%Z %z %s %N %T %R %D %F %t%%",
                "UTC +0000 0 000000000 00:00:00 00:00 01/01/70 1970-01-01 \t%",
            ),
        ];
        for (f, want) in cases {
            assert_eq!(epoch_format(f).as_deref(), Some(want), "{f}");
        }
        for f in ["%n", "%c", "%-d", "%3N", "%", "%Ey"] {
            assert_eq!(epoch_format(f), None, "{f}");
        }
    }

    #[test]
    fn org_regexp_finds_macros_as_the_parser_reads_them() {
        let s = "a {{{x}}} {{{y(1,2)}}} {{{ {{{9}}} {{{results({{{z}}})}}} {{{w(\0)}}}";
        let m = raw_macros(s, 0, s.len());
        let names: Vec<_> = m.iter().map(|o| (o.2.as_str(), o.3.clone())).collect();
        assert_eq!(
            names,
            [
                ("x", None),
                ("y", Some("1,2".to_string())),
                ("results", Some("{{{z}}}".to_string())),
                ("z", None),
            ]
        );
    }

    /// Per byte of a text: `Ok(src)` copied from source byte `src`, `Err`
    /// made from that source range.
    type Origin = Result<usize, (usize, usize)>;

    /// One round of random replacements over a text whose bytes come from
    /// `origin`: the edits, and the new text's origins.
    fn random_round(
        origin: &[Origin],
        next: &mut impl FnMut(usize) -> usize,
    ) -> (Vec<Piece>, Vec<Origin>) {
        let hull = |os: &[Origin]| {
            os.iter().fold((usize::MAX, 0), |h, o| {
                let (a, b) = match o {
                    Ok(s) => (*s, s + 1),
                    Err(r) => *r,
                };
                (h.0.min(a), h.1.max(b))
            })
        };
        let (mut edits, mut out) = (Vec::new(), Vec::new());
        let mut at = 0;
        while at < origin.len() {
            if next(4) != 0 {
                out.push(origin[at]);
                at += 1;
                continue;
            }
            let end = (at + 1 + next(4)).min(origin.len());
            let n = next(4);
            let s = out.len();
            out.extend(std::iter::repeat_n(Err(hull(&origin[at..end])), n));
            edits.push(Piece {
                src: (at, end),
                out: (s, s + n),
            });
            at = end;
        }
        (edits, out)
    }

    /// Rounds of random replacements over a text, each byte's origin
    /// tracked by hand: a copied byte maps to exactly its source byte both
    /// ways, and a replaced one into the source range it was made from.
    #[test]
    fn a_map_composed_over_rounds_agrees_with_every_bytes_origin() {
        let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = |n: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as usize
        };
        for _ in 0..300 {
            let len = 1 + next(40);
            let mut origin: Vec<Origin> = (0..len).map(Ok).collect();
            let mut map = Map::default();
            for _ in 0..1 + next(3) {
                let (edits, o) = random_round(&origin, &mut next);
                map = map.then(&edits);
                origin = o;
            }
            for (p, o) in origin.iter().enumerate() {
                check_origin(&map, p, o);
            }
        }
    }

    fn check_origin(map: &Map, p: usize, o: &Origin) {
        let got = map.to_source(Span::new(p, p + 1));
        match o {
            Ok(src) => {
                assert_eq!(got, Span::new(*src, src + 1), "{map:?} at {p}");
                let back = map.to_expanded(Span::new(*src, src + 1));
                assert_eq!(back, Span::new(p, p + 1), "{map:?} at {src}");
            }
            Err((a, b)) => {
                let (a, b) = (*a as u32, *b as u32);
                assert!(got.start <= a && got.end >= b, "{map:?} at {p}: {got:?}");
            }
        }
    }

    fn work(text: &str) -> (u64, Result<Option<Expansion>, RenderDiagnostic>) {
        WORK.with(|w| w.set(0));
        let doc = crate::parse(text, &crate::ParseOptions::default());
        let r = expand(&doc);
        (WORK.with(std::cell::Cell::get), r)
    }

    /// The work of an expansion, counted: it grows as the source does
    /// (8 times the calls, about 8 times the work), every byte it produces
    /// is counted, and the reviewer's exponential shape (review round 4,
    /// B7: 15 s for 8.6 KB) stops within the bound, whatever N.
    #[test]
    fn expansion_work_is_linear_in_the_source() {
        let plain = |n: usize| {
            format!(
                "#+MACRO: m <$1>\n#+MACRO: w {{{{{{m($1)}}}}}}\n{}\n",
                "{{{w(x)}}} {{{n}}} ".repeat(n)
            )
        };
        let (few, r) = work(&plain(400));
        let out = r.unwrap().unwrap().text;
        assert!(few >= out.len() as u64, "{few} < {}", out.len());
        let (many, _) = work(&plain(3200));
        assert!(many <= 9 * few, "{few} -> {many}");
        for n in [25, 100, 400] {
            let doc = format!(
                "#+MACRO: b {}\n#+MACRO: a {}\n{}\n",
                "x ".repeat(3000),
                "{{{b}}}".repeat(255),
                "{{{a}}} ".repeat(n)
            );
            let (w, r) = work(&doc);
            let limit = WORK_BASE + WORK_PER_BYTE * doc.len() as u64;
            assert!(
                matches!(
                    r,
                    Err(RenderDiagnostic::MacroFailed(MacroFailure::Budget, _))
                ),
                "{n}: {r:?}"
            );
            // Past the limit by at most the last charge (one parse of a
            // line and its pending text).
            assert!(w <= limit + 64 * 1024, "{n}: {w} > {limit}");
        }
    }

    #[test]
    fn a_map_finds_where_each_position_came_from() {
        // `ab{{{m}}}cd` → `abXYZcd`, then `X` → ``.
        let m = Map::default().then(&[Piece {
            src: (2, 9),
            out: (2, 5),
        }]);
        assert_eq!(m.to_source(Span::new(0, 2)), Span::new(0, 2));
        assert_eq!(m.to_source(Span::new(3, 4)), Span::new(2, 9));
        assert_eq!(m.to_source(Span::new(5, 7)), Span::new(9, 11));
        assert_eq!(m.to_expanded(Span::new(9, 11)), Span::new(5, 7));
        assert_eq!(m.to_expanded(Span::new(0, 11)), Span::new(0, 7));
        let m2 = m.then(&[Piece {
            src: (2, 3),
            out: (2, 2),
        }]);
        assert_eq!(m2.to_source(Span::new(4, 6)), Span::new(9, 11));
        assert_eq!(m2.to_source(Span::new(2, 4)), Span::new(2, 9));
        assert_eq!(m2.to_expanded(Span::new(9, 11)), Span::new(4, 6));
    }
}
