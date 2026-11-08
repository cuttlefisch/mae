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
//! - Org judges each macro against the buffer as it stands when its scan
//!   reaches it; so does MAE (see `scan`), re-reading what an expansion
//!   can change. Not reproduced: `{{{keyword}}}` reads the buffer as last
//!   read whole (Org: as it stands), which differs only when an expansion
//!   since then made or changed a `#+KEY:` line.

mod scan;
mod syntax;
mod value;

pub(crate) use syntax::is_export_property;
#[cfg(test)]
use syntax::{extract_arguments, raw_macros, substitute};
#[cfg(test)]
use value::epoch_format;

use super::{MacroFailure, RenderDiagnostic};
use crate::{Document, Span};
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
pub(super) const SPECIAL_PROPERTIES: &[&str] = &[
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
pub(super) struct Piece {
    pub(super) src: (usize, usize),
    pub(super) out: (usize, usize),
}

/// Where an expanded text came from: its replaced pieces, in order; the
/// text between them is copied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Map {
    pub(super) pieces: Vec<Piece>,
}

impl Map {
    /// Position `p` of the source (`forward`) or of the expanded text in
    /// the other: exact in copied text; inside a replaced piece, the start
    /// of what it became (`start`), else its end.
    pub(super) fn locate(&self, p: usize, start: bool, forward: bool) -> usize {
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
    pub(super) fn then(&self, edits: &[Piece]) -> Map {
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
    /// The work it was charged.
    pub work: u64,
}

/// Why an expansion stopped, and where (in the parsed text).
type Stop = (MacroFailure, usize, usize);

/// A macro found in a text: its start and end, name and raw arguments.
type Found = (usize, usize, String, Option<String>);

/// Expands the macros of `doc` (whose text is `source`), or `Ok(None)`
/// when it has none to expand.
pub(crate) fn expand(doc: &Document, source: &str) -> Result<Option<Expansion>, RenderDiagnostic> {
    debug_assert_eq!(source.len(), usize::from(doc.syntax().text_range().len()));
    // `org-export--delete-comment-trees`: the COMMENT subtrees go first,
    // from the text, before anything else reads it.
    let (text, map) = without_comment_trees(doc, source);
    // Every macro, written or forced, opens with `{{{`: without one there
    // is nothing to expand, and what is left is the text without them.
    if !source.contains("{{{") {
        return Ok((!map.pieces.is_empty()).then_some(Expansion {
            text,
            map,
            notes: Vec::new(),
            work: 0,
        }));
    }
    let templates = value::templates(doc, source);
    if map.pieces.is_empty() {
        return scan::Engine::new(doc, source.len(), templates).run(map, HashSet::new());
    }
    let d = doc.reparse(&text);
    let engine = scan::Engine::new(&d, source.len(), templates);
    Ok(Some(engine.run(map.clone(), HashSet::new())?.unwrap_or(
        Expansion {
            text,
            map,
            notes: Vec::new(),
            work: 0,
        },
    )))
}

/// `source` (the text of `doc`) without its COMMENT subtrees (each from
/// its headline to the end of its subtree), and the map of what went.
fn without_comment_trees(doc: &Document, source: &str) -> (String, Map) {
    let mut text = String::with_capacity(source.len());
    let mut pieces = Vec::new();
    let mut at = 0;
    let mut walk = doc.syntax().preorder();
    while let Some(ev) = walk.next() {
        let rowan::WalkEvent::Enter(n) = ev else {
            continue;
        };
        if crate::Headline::cast(n.clone()).is_some_and(|h| h.is_commented()) {
            walk.skip_subtree();
            let (a, b) = (
                usize::from(n.text_range().start()),
                usize::from(n.text_range().end()),
            );
            text.push_str(&source[at..a]);
            pieces.push(Piece {
                src: (a, b),
                out: (text.len(), text.len()),
            });
            at = b;
        }
    }
    text.push_str(&source[at..]);
    (text, Map { pieces })
}

/// `text` (a link fragment) with its macros expanded with the templates
/// of `doc`; `None` when they cannot be.
pub(crate) fn expand_text(doc: &Document, text: &str) -> Option<String> {
    let source = crate::text::source_of(&doc.syntax());
    let templates = value::templates(doc, &source);
    let d = crate::parse(text, &crate::ParseOptions::default());
    let engine = scan::Engine::new(&d, text.len(), templates);
    match engine.run(Map::default(), HashSet::new()) {
        Ok(Some(e)) => Some(e.text),
        Ok(None) => Some(text.to_string()),
        Err(_) => None,
    }
}

/// What the macros of a property value expand with, as
/// `{{{property(KEY)}}}` expands them where it stands: the templates and
/// the `#+KEY:` lines of the document as written, and one budget, linear
/// in it, that every value of a render shares.
pub(crate) struct ValueCx {
    templates: HashMap<String, value::Template>,
    lines: std::rc::Rc<Vec<String>>,
    opts: crate::ParseOptions,
    left: std::cell::Cell<u64>,
}

impl ValueCx {
    pub(crate) fn new(source: &Document) -> ValueCx {
        let text = source.syntax().to_string();
        let root = source.syntax();
        let lines = value::keyword_lines(&root, &text, source.line_ending());
        ValueCx {
            templates: value::templates(source, &text),
            lines: std::rc::Rc::new(lines.into_iter().map(str::to_string).collect()),
            opts: crate::ParseOptions {
                todo_sequences: source.todo_sequences().to_vec(),
                ..crate::ParseOptions::default()
            },
            left: std::cell::Cell::new(WORK_BASE + WORK_PER_BYTE * text.len() as u64),
        }
    }

    /// The parse options of the document.
    pub(crate) fn options(&self) -> &crate::ParseOptions {
        &self.opts
    }

    /// `text` (an entry: its heading line and property drawer, then a
    /// paragraph holding the value) with its macros expanded as the
    /// export's first expansion does, `results` left; `Ok(None)` when it
    /// has none. Charged to the shared budget.
    pub(crate) fn expand(&self, text: &str) -> Result<Option<Expansion>, MacroFailure> {
        let d = crate::parse(text, &self.opts);
        let mut engine = scan::Engine::new(&d, text.len(), self.templates.clone());
        engine.outer_lines = Some(self.lines.clone());
        self.run(engine, Map::default(), HashSet::new())
    }

    /// Org's second expansion of `doc` (what [`ValueCx::expand`] made,
    /// parsed), as [`expand_results`]; charged to the shared budget.
    pub(crate) fn results(
        &self,
        doc: &Document,
        map: Map,
        removed: HashSet<usize>,
    ) -> Result<Option<Expansion>, MacroFailure> {
        let templates =
            HashMap::from([("results".to_string(), value::Template::Text("$1".into()))]);
        let len = doc.syntax().text().len().into();
        self.run(scan::Engine::new(doc, len, templates), map, removed)
    }

    fn run(
        &self,
        mut engine: scan::Engine,
        map: Map,
        skip: HashSet<usize>,
    ) -> Result<Option<Expansion>, MacroFailure> {
        let limit = engine.work_limit.min(self.left.get());
        engine.work_limit = limit;
        engine.charge(0).map_err(|(f, ..)| f)?;
        let len = engine.dtext.len() as u64;
        let r = engine.run(map, skip);
        // Nothing expanded: about one read of the text.
        let spent = match &r {
            Ok(Some(e)) => e.work,
            Ok(None) => len,
            Err(_) => limit,
        };
        self.left.set(self.left.get().saturating_sub(spent));
        r.map_err(|d| match d {
            RenderDiagnostic::MacroFailed(f, _) => f,
            _ => MacroFailure::Unsupported,
        })
    }
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
    let templates = HashMap::from([("results".to_string(), value::Template::Text("$1".into()))]);
    scan::Engine::new(doc, source_len, templates).run(map, removed)
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
        // `[ \t\r\n]+`, as `org-element-macro-parser` collapses them.
        assert_eq!(x("a\r\n b\r"), v(&["a b"]));
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
        let r = expand(&doc, text);
        (WORK.with(std::cell::Cell::get), r)
    }

    /// The work of an expansion, counted: it grows as the source does
    /// (8 times the units, about 8 times the work), every byte it produces
    /// is counted, and the reviewer's exponential shape (review round 4,
    /// B7: 15 s for 8.6 KB) stops within the bound, whatever N.
    #[test]
    fn expansion_work_is_linear_in_the_source() {
        let plain = |n: usize| {
            format!(
                "#+MACRO: m <$1>\n#+MACRO: w {{{{{{m($1)}}}}}}\n{}",
                "{{{w(x)}}} {{{n}}} text\n\n".repeat(n)
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
            assert_fails_within_the_budget(&doc);
        }
    }

    fn assert_fails_within_the_budget(doc: &str) {
        let (w, r) = work(doc);
        let limit = WORK_BASE + WORK_PER_BYTE * doc.len() as u64;
        assert!(
            matches!(
                r,
                Err(RenderDiagnostic::MacroFailed(MacroFailure::Budget, _))
            ),
            "{r:?}"
        );
        // Past the limit by at most the last charge, which is never past
        // the size bound.
        let size = (SIZE_BASE + SIZE_PER_BYTE * doc.len()) as u64;
        assert!(w <= limit + size, "{w} > {limit} + {size}");
    }

    /// Review round 6 (major 2): ordinary documents, a macro in every
    /// glossary tag, caption, changelog entry and numbered step (5,000 of
    /// each), expand with work linear in the source: 8 times the entries,
    /// at most 9 times the work. Each re-reads its own unit (the line it
    /// touches stays the same kind of line); none re-reads the buffer.
    #[test]
    fn ordinary_documents_expand_with_linear_work() {
        let prose = "This entry explains the term in a sentence or two of ordinary prose.";
        type Entry = fn(usize, &str) -> String;
        let families: [(&str, Entry); 5] = [
            ("glossary", |i, p| {
                format!("- {{{{{{term(Word{i})}}}}}} :: {p}\n")
            }),
            ("captions", |i, p| {
                format!("#+CAPTION: {{{{{{v(Figure {i}.)}}}}}}\n[[file:img{i}.png]]\n\n{p}\n\n")
            }),
            ("changelog", |i, p| {
                format!("{{{{{{issue({i})}}}}}} fixed a thing. {p}\n\n")
            }),
            ("steps", |i, p| {
                format!(
                    "Step {{{{{{n}}}}}}. Then step {{{{{{n}}}}}}, see ({{{{{{n}}}}}}). {p} {i}\n\n"
                )
            }),
            ("table", |i, _| {
                format!("| {{{{{{n}}}}}}. | {{{{{{term(w{i})}}}}}} | x |\n")
            }),
        ];
        let head = "#+MACRO: term /$1/\n#+MACRO: v $1\n#+MACRO: issue [[https://example.com/issues/$1][#$1]]\n\n* Section\n";
        for (name, entry) in families {
            let doc = |n: usize| -> String {
                let body: String = (0..n).map(|i| entry(i, prose)).collect();
                format!("{head}{body}")
            };
            let (few, r) = work(&doc(625));
            assert!(r.is_ok(), "{name}: {r:?}");
            let big = doc(5000);
            let (many, r) = work(&big);
            assert!(r.is_ok(), "{name}: {r:?}");
            assert!(many <= 9 * few, "{name}: {few} -> {many}");
            let limit = WORK_BASE + WORK_PER_BYTE * big.len() as u64;
            assert!(many * 4 <= limit, "{name}: {many} of {limit}");
        }
    }

    /// The measured limit of one unit (registered: macros): each expansion
    /// re-reads the unit holding it, and the line it touches, so N macros
    /// in one paragraph cost about N times its length. Alone in a
    /// document, a paragraph of up to 320 short macros on one line (5.4 KB)
    /// expands, or 568 on as many lines (9.7 KB); 3,200 fail closed, within
    /// the budget.
    #[test]
    fn one_paragraph_of_many_macros_is_bounded_by_the_budget() {
        let line = |n: usize| format!("{}\n", "{{{n}}} abcdefgh ".repeat(n));
        let (_, r) = work(&line(320));
        assert!(r.is_ok(), "{r:?}");
        assert_fails_within_the_budget(&line(3200));
        let lines = |n: usize| "{{{n}}} abcdefgh\n".repeat(n);
        assert!(work(&lines(568)).1.is_ok());
        assert_fails_within_the_budget(&lines(3200));
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
