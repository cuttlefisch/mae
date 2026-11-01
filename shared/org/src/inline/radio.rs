//! Radio links: text matching one of the document's radio targets
//! (`<<<target>>>`), as `org-target-link-regexp` matches it.
//!
//! Org builds one regexp from every radio target in the buffer (found when
//! `org-mode` starts, with no radio links active), longest target first:
//! `\(?:^\|[^[:alnum:]]\|\c|\)\(T1\|T2\|…\)\(?:$\|[^[:alnum:]]\|\c|\)`,
//! where each target is quoted and each run of spaces in it matches any
//! non-empty run of whitespace (`\s-+`), and the match ignores case. The
//! object lexer bounds its search just past the next match, so a radio link
//! is parsed when no other object starts before it. At a given start the
//! regexp picks the FIRST target, in that order, whose match (with its
//! after-context, inside the region) succeeds.
//!
//! When that regexp would be `org-target-link-regexp-limit` (8192) chars or
//! longer, Org cannot compile it (a larger limit makes Emacs signal "Regular
//! expression too big") and splits the targets, in order, into several
//! regexps (`org-target-link-regexps`; see [`partitions`]). The lexer's
//! search then takes, among each regexp's leftmost match, the one that ENDS
//! first, the earliest regexp on a tie (`org--re-list-search-forward`); the
//! link parser takes the first regexp, in order, that matches where it
//! stands (`org--re-list-looking-at`). The split is a function of the text
//! and a constant, so it is reproduced exactly; the limit is declared in
//! [`crate::ParseOptions::radio_regexp_limit`].
//!
//! Searching that regexp from every position, as Emacs does, costs
//! O(text × targets × length); recording every (start, target) hit costs
//! O(text^1.5) memory on targets `a`, `a a`, `a a a`, … Here nothing is
//! enumerated per hit. The text is read as symbols — a char, case folded,
//! or a whole whitespace run — and the targets are a trie over the same
//! symbols. For each symbol the deepest trie node the text from there
//! reaches is found once for the whole document (matching statistics over a
//! suffix automaton of the reversed targets: linear). The targets matching
//! at a start are then exactly the target nodes on that one trie path, and
//! which of them the regexp picks is a property of the path:
//!
//! - a target that ends INSIDE the path is followed by the path's next
//!   symbol, so its after-context is fixed by the trie edge (a whitespace
//!   run or a border char passes); the first such target in regexp order is
//!   precomputed per node;
//! - the target at the path's last node is followed by text off the trie:
//!   its after-context is checked at the position;
//! - a region ending inside the path cuts it: the node at the cut depth is
//!   a level-ancestor query (jump pointers, O(log)).
//!
//! Targets are of three shapes. SIMPLE ones have only spaces as whitespace
//! (each space run is `\s-+`, which takes a whole text run since it sits
//! between non-blank chars): a trie match is a regexp match. LITERAL ones
//! have other whitespace (a tab, a no-break space) but no space, and start
//! and end with a non-blank char: each of their whitespace stretches must
//! equal the text's run exactly, so they get their own trie over symbols
//! where a run is its exact content. MIXED ones (spaces and other
//! whitespace in one stretch, or whitespace at either end, so a match may
//! start or end inside a run) are found on the first trie's path and
//! checked by a backtracking match of the exact regexp. A mixed target
//! always holds whitespace other than a space, so where the text has none
//! within reach no mixed target is tried at all; targets whose items are
//! identical are tried once; and a chain of targets sharing a leading
//! whitespace stretch is skipped at once when that stretch fails. What is
//! NOT indexed: many mixed targets on one node that differ only in a
//! stretch (`x \ty`, `x \t\ty`, …), over text holding such whitespace. Each
//! is tried at each candidate, and such a document spends the budget.
//!
//! WORK BUDGET. Every step of the search (each candidate visited, each
//! match recorded or dropped, each trie or exact-match step) is charged to
//! a work budget proportional to the text ([`budget_for`]); their memo of failed states is capped in proportion to
//! the text too, so memory is bounded however hostile the input. When the
//! budget runs out the rule is: NO FURTHER RADIO LINK is made, in this
//! region or any later one (the lexer goes on as if the document had no
//! radio targets), and the parse reports
//! [`crate::DiagnosticKind::RadioLinkBudget`]. Links found before that
//! point are Org's; none after it is a different link than Org's, only an
//! absent one. A link the search has already found is still parsed: the
//! search records what the link parser will read there, so the parser never
//! repeats work it could run out of. Running out never ends a region's
//! objects early.

use super::{InlineCx, Obj};
use crate::chars::{canon, is_alnum, is_linebreak, is_space};
use crate::kind::SyntaxKind::*;
use crate::span::ContentsTable;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

/// No node / no target.
const NONE: u32 = u32::MAX;
/// The symbol a whitespace run reads as in the first trie.
const W: u32 = u32::MAX;
/// Exact whitespace-run contents, as symbols of the second trie, start here
/// (above every char).
const RUN_BASE: u32 = 0x0020_0000;
/// The length of Org's `before-re` and `after-re` in
/// `org-update-radio-target-regexp` (measured: 28 each).
const BORDERS_LEN: usize = 56;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Item {
    /// A char, case-folded.
    Lit(char),
    /// `\s-+`, from a run of spaces in the target.
    Run,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Simple,
    Literal,
    Mixed,
}

struct Target {
    items: Vec<Item>,
    shape: Shape,
    /// Emacs's `length` of the target's text (chars), which Org's split
    /// counts.
    chars: usize,
    /// Mixed targets that start with whitespace followed by a non-blank
    /// char: that leading stretch and the char, as a group id (see
    /// `Radio::leads`); `NONE` otherwise.
    lead: u32,
}

/// The work ran out (see the module doc's budget rule).
#[derive(Debug, Clone, Copy)]
struct OutOfWork;

type Work<T> = Result<T, OutOfWork>;

pub(crate) struct Radio {
    /// The targets' values as written, in regexp order (the document's
    /// radio targets, [`crate::Document::radio_targets`]).
    values: Vec<String>,
    targets: Vec<Target>,
    /// Leading stretches of mixed targets (each with the char after it):
    /// checked as targets of their own, ids `targets.len() + i`.
    leads: Vec<Vec<Item>>,
    /// The regexp (of Org's split) each target is in; all 0 when one regexp.
    part: Vec<u32>,
    /// How many regexps.
    parts: usize,
    /// Byte start of each symbol of the text, then the text's end.
    sym: Vec<usize>,
    /// Whether each symbol is a whitespace run.
    ws: Vec<bool>,
    /// For each symbol, how many symbols on the nearest run holding
    /// whitespace other than a space is (`u32::MAX`: none).
    odd_next: Vec<u32>,
    /// Simple targets, and mixed ones (by their run-collapsed symbols).
    plain: Matcher,
    /// Literal targets, runs read as their exact content.
    literal: Matcher,
    /// Byte positions where some target may start, sorted.
    starts: Vec<usize>,
    /// Work left (see the module doc).
    budget: Cell<u64>,
    exhausted: Cell<bool>,
    /// Where the lexer last asked for a radio link: once the work runs out,
    /// where radio links stop.
    asked_at: Cell<usize>,
    memo: RefCell<Memo>,
    /// The last search's link, as the link parser will read it there.
    answer: Cell<Option<Answer>>,
}

/// What the link parser reads at `at` (position, region floor and end).
#[derive(Debug, Clone, Copy)]
struct Answer {
    at: (usize, usize, usize),
    link: Option<Match>,
}

/// Exact-match states known to fail, per region end; capped (see
/// [`Memo::note`]). Keys pack (target, item, region end, position) into
/// 128 bits: every offset fits 32 (rowan's text size).
struct Memo {
    failed: HashSet<u128>,
    /// A `\s-+` item at the start of a text run: the least start at which it
    /// failed. Starting later in the same run offers a subset of the ends.
    run_failed: HashMap<u128, u32>,
    /// Entries kept at most; past it both maps start over. Forgetting only
    /// costs work (charged to the budget), never a different answer.
    cap: usize,
}

impl Memo {
    fn new(text_len: usize) -> Memo {
        Memo {
            failed: HashSet::new(),
            run_failed: HashMap::new(),
            cap: (text_len / 2).max(4_096),
        }
    }

    /// Makes room for one more entry.
    fn note(&mut self) {
        if self.failed.len() + self.run_failed.len() >= self.cap {
            self.failed.clear();
            self.run_failed.clear();
        }
    }
}

fn key(id: u32, item: usize, end: usize, pos: usize) -> u128 {
    (u128::from(id) << 96) | ((item as u128) << 64) | ((end as u128) << 32) | pos as u128
}

/// Where a frame's search for radio links has got to: candidates before
/// `next` were rejected for good; with split regexps, what the frame's
/// searches have learnt so far ([`Window`]).
#[derive(Debug, Default)]
pub(crate) struct Cursor {
    next: usize,
    window: Option<Box<Window>>,
}

/// A split search's state for one frame (one region, whose searches start
/// at non-decreasing positions), so that a candidate is visited once and
/// examined at most once however many searches the lexer makes in the
/// region (review B3: every search restarted at the region's first match,
/// so N objects before a long match visited its candidates N times).
///
/// A search's answer is, of each regexp's first match at or after the
/// search start (its FRONT), the one ending first. Candidates are visited
/// in order. One is EXAMINED (every regexp's first target there found) unless
/// every regexp that could match there already has a front, which is then
/// earlier: it is only noted as SKIPPED (the old per-search shortcut, kept,
/// so that a long match's inside costs nothing). When the search start
/// passes a regexp's front, its next match is its next examined one, or an
/// earlier skipped candidate: those are examined, in order, until it is
/// found. So no skipped candidate lies before the front of a regexp that
/// could match it, every candidate is examined at most once, and each
/// front is that regexp's first match.
#[derive(Debug, Default)]
struct Window {
    /// The next candidate (an index into `Radio::starts`) to visit.
    scan: usize,
    /// Examined candidates with a match, at or after the search start:
    /// position → each regexp matching there with its first target.
    hits: std::collections::BTreeMap<usize, PartMatches>,
    /// Per regexp, the positions in `hits` holding it.
    of: HashMap<u32, std::collections::BTreeSet<usize>>,
    /// Visited candidates not examined, in order.
    skipped: std::collections::VecDeque<usize>,
    /// Each regexp's front: its position and match end, and all of them by
    /// `(end, regexp)`.
    front: HashMap<u32, (usize, usize)>,
    fronts: std::collections::BTreeSet<(usize, u32)>,
    /// The regexps with a front, as a bitset.
    covered: Vec<u64>,
    /// The last answer: the best match and what the link parser reads at
    /// its start (a function of the position alone).
    last: Option<(Match, Option<Match>)>,
}

impl Window {
    fn set_front(&mut self, j: u32, p: usize, m0: usize) {
        self.front.insert(j, (p, m0));
        self.fronts.insert((m0, j));
        let (w, b) = (j as usize / 64, j % 64);
        if self.covered.len() <= w {
            self.covered.resize(w + 1, 0);
        }
        self.covered[w] |= 1 << b;
    }

    fn clear_front(&mut self, j: u32) {
        if let Some((_, m0)) = self.front.remove(&j) {
            self.fronts.remove(&(m0, j));
            if let Some(w) = self.covered.get_mut(j as usize / 64) {
                *w &= !(1 << (j % 64));
            }
        }
    }

    /// Does every regexp in `lo..=hi` have a front?
    fn all_covered(&self, lo: u32, hi: u32) -> bool {
        (lo..=hi).all(|j| {
            self.covered
                .get(j as usize / 64)
                .is_some_and(|w| w >> (j % 64) & 1 == 1)
        })
    }

    /// Records examined candidate `p`'s table. A regexp without a front
    /// and not `moving` gets it here: no examined candidate before `p`
    /// holds it, and no skipped one could. A moving regexp's front is
    /// found by the caller.
    fn record(&mut self, p: usize, table: PartMatches, moving: &HashSet<u32>) {
        if table.is_empty() {
            return;
        }
        for &(j, _, m) in &table {
            self.of.entry(j).or_default().insert(p);
            if !self.front.contains_key(&j) && !moving.contains(&j) {
                self.set_front(j, p, m.m0);
            }
        }
        self.hits.insert(p, table);
    }

    /// The first examined match of regexp `j`: position and end.
    fn first_of(&self, j: u32) -> Option<(usize, usize)> {
        let p = *self.of.get(&j)?.first()?;
        let m0 = self.hits[&p].iter().find(|e| e.0 == j)?.2.m0;
        Some((p, m0))
    }

    /// The best match: the front ending first, the earliest regexp on a tie.
    fn best(&self) -> Option<Match> {
        let &(_, j) = self.fronts.first()?;
        let (p, _) = self.front[&j];
        self.hits[&p].iter().find(|e| e.0 == j).map(|e| e.2)
    }
}

/// A match of the regexp: group 1 `[p, g1)`, the whole match ending at `m0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Match {
    pub p: usize,
    pub g1: usize,
    pub m0: usize,
}

/// A split's matches at one position: (regexp, its first target there, the
/// match), one per regexp that matches.
type PartMatches = Vec<(u32, u32, Match)>;

/// Where the matches at a position go: the first target in regexp order
/// (one regexp), or the first of each regexp (a split).
enum Sink<'a> {
    First(&'a mut Option<(u32, Match)>),
    Parts(&'a mut PartMatches, &'a [u32]),
}

impl Sink<'_> {
    /// No target at or past this index is wanted.
    fn bound(&self) -> u32 {
        match self {
            Sink::First(best) => best.map_or(NONE, |b| b.0),
            Sink::Parts(..) => NONE,
        }
    }

    fn wants(&self, t: u32) -> bool {
        match self {
            Sink::First(best) => best.is_none_or(|b| t < b.0),
            Sink::Parts(out, part) => {
                let j = part[t as usize];
                out.iter().find(|e| e.0 == j).is_none_or(|e| t < e.1)
            }
        }
    }

    fn take(&mut self, t: u32, m: Match) {
        if !self.wants(t) {
            return;
        }
        match self {
            Sink::First(best) => **best = Some((t, m)),
            Sink::Parts(out, part) => {
                let j = part[t as usize];
                match out.iter_mut().find(|e| e.0 == j) {
                    Some(e) => *e = (j, t, m),
                    None => out.push((j, t, m)),
                }
            }
        }
    }
}

impl Radio {
    /// The document's radio targets, from a parse without radio links (as
    /// `org-update-radio-target-regexp` finds them), or `None`.
    /// `parse_all` parses every object container of the document into the
    /// table it is given.
    pub fn find(
        cx: &InlineCx,
        parse_all: impl FnOnce(&InlineCx, &mut ContentsTable),
    ) -> Option<Radio> {
        if !cx.src.text.contains("<<<") {
            return None;
        }
        let mut table = ContentsTable::default();
        parse_all(cx, &mut table);
        let mut seen = HashSet::new();
        let mut values: Vec<&str> = Vec::new();
        for s in table.spans_of(RADIO_TARGET) {
            let v = &cx.src.text[s.range()];
            if seen.insert(v) {
                values.push(v);
            }
        }
        // `cl-pushnew` builds the list newest first; `sort` is stable.
        values.reverse();
        values.sort_by_key(|v| std::cmp::Reverse(v.chars().count()));
        (!values.is_empty()).then(|| Radio::build(cx, &values))
    }

    pub(crate) fn build(cx: &InlineCx, values: &[&str]) -> Radio {
        let mut targets: Vec<Target> = values.iter().map(|v| compile(v)).collect();
        let part = partitions(values, &targets, cx.radio_limit);
        let parts = part.last().map_or(1, |&j| j as usize + 1);
        let leads = assign_leads(&mut targets);
        let text = Symbols::read(cx);
        let mut plain_seqs = Vec::new();
        let mut literal_seqs = Vec::new();
        for (t, target) in targets.iter().enumerate() {
            let t = t as u32;
            match target.shape {
                Shape::Simple => plain_seqs.push((t, collapsed(target), true)),
                Shape::Mixed => plain_seqs.push((t, collapsed(target), false)),
                Shape::Literal => {
                    // A stretch the text never holds cannot match: skip it.
                    if let Some(seq) = exact_symbols(target, &text.runs) {
                        literal_seqs.push((t, seq, true));
                    }
                }
            }
        }
        let plain = Matcher::new(&plain_seqs, &text.plain, &targets, &part);
        let literal = Matcher::new(&literal_seqs, &text.exact, &targets, &part);
        let n = text.ws.len();
        let mut starts = Vec::new();
        for k in 0..n {
            let any_plain = plain.trie.any[plain.deep[k] as usize];
            if text.ws[k] {
                // Only a mixed target can start in a run, at any char of it.
                if any_plain {
                    let mut q = text.sym[k];
                    while q < text.sym[k + 1] {
                        starts.push(q);
                        q = cx.step(q);
                    }
                }
            } else if any_plain || literal.trie.any[literal.deep[k] as usize] {
                starts.push(text.sym[k]);
            }
        }
        let mut odd_next = vec![u32::MAX; n];
        for k in (0..n).rev() {
            odd_next[k] = if text.odd[k] {
                0
            } else {
                odd_next
                    .get(k + 1)
                    .map_or(u32::MAX, |&o| o.saturating_add(1))
            };
        }
        Radio {
            values: values.iter().map(|v| v.to_string()).collect(),
            targets,
            leads,
            part,
            parts,
            sym: text.sym,
            ws: text.ws,
            odd_next,
            plain,
            literal,
            starts,
            budget: Cell::new(budget_for(cx.src.len())),
            exhausted: Cell::new(false),
            asked_at: Cell::new(0),
            memo: RefCell::new(Memo::new(cx.src.len())),
            answer: Cell::new(None),
        }
    }

    /// The targets' values, in regexp order.
    pub fn values(&self) -> &[String] {
        &self.values
    }

    /// Where radio links stop, if the work budget ran out: no radio link
    /// starts at or after this position (see the module doc).
    pub fn exhausted_at(&self) -> Option<usize> {
        self.exhausted.get().then(|| self.asked_at.get())
    }

    /// Where the next radio link's text starts at or after `from` (Org bounds
    /// `org-element--object-lex`'s search one char past it).
    pub fn next_link(
        &self,
        cx: &InlineCx,
        from: usize,
        end: usize,
        floor: usize,
        cur: &mut Cursor,
    ) -> Option<usize> {
        let s0 = if cx.at_bol(from, floor) {
            from
        } else {
            from - cx.char_before(from, floor).map_or(1, char::len_utf8)
        };
        let m = self.search(cx, s0, (floor, end), cur)?;
        if m.p >= from {
            return Some(m.p);
        }
        // Matched at the start of the line just behind us. Org handles a
        // one-char link there by searching again from the match's end; a
        // longer one makes Org return an object starting before point,
        // overlapping the previous one (registered divergence), which no
        // tree can hold, so it is skipped.
        let lbp = cx.src.lines[cx.src.line_of(from)].start.max(floor);
        let after_lbp = lbp + cx.char_at(lbp).map_or(1, char::len_utf8);
        let again = if from == after_lbp && m.g1 == from {
            m.m0
        } else {
            from
        };
        let m = self.search(cx, again, (floor, end), cur)?;
        (m.p >= from).then_some(m.p)
    }

    /// The first match whose regexp starts at or after `s0` (with a split,
    /// the one ending first), recording what the link parser will read at
    /// its start. `None` once the work has run out.
    fn search(
        &self,
        cx: &InlineCx,
        s0: usize,
        region: (usize, usize),
        cur: &mut Cursor,
    ) -> Option<Match> {
        self.answer.set(None);
        if self.exhausted.get() {
            return None;
        }
        self.asked_at.set(s0);
        let found = if self.parts == 1 {
            self.search_one(cx, s0, region, cur)
                .map(|m| m.map(|m| (m, Some(m))))
        } else {
            self.search_split(cx, s0, region, cur)
        };
        let (m, link) = found.ok().flatten()?;
        self.answer.set(Some(Answer {
            at: (m.p, region.0, region.1),
            link,
        }));
        Some(m)
    }

    /// One regexp: the first candidate at or after `s0` that matches.
    fn search_one(
        &self,
        cx: &InlineCx,
        s0: usize,
        (floor, end): (usize, usize),
        cur: &mut Cursor,
    ) -> Work<Option<Match>> {
        // Candidates this frame skipped are rejected for good: a later
        // search starts at or after `s0`, and whether a candidate `p >= s0`
        // matches does not depend on `s0` (at `p == s0` only `^` can, which
        // does not look at it). So the cursor only moves forward. Each
        // candidate tried is charged (`first_at`).
        self.charge()?;
        let first = self.starts.partition_point(|&p| p < s0);
        let mut k = cur.next.max(first);
        while let Some(&p) = self.starts.get(k).filter(|&&p| p < end) {
            #[cfg(test)]
            tests::VISITS.with(|v| v.set(v.get() + 1));
            if let Some(m) = self.first_at(cx, p, s0, (floor, end))? {
                cur.next = k;
                return Ok(Some(m));
            }
            k += 1;
        }
        cur.next = k;
        Ok(None)
    }

    /// Split regexps (`org--re-list-search-forward`): each regexp's leftmost
    /// match from `s0`, and of those the one whose match ends first, the
    /// earliest regexp on a tie; then what the link parser reads at its
    /// start (`org--re-list-looking-at`, the first regexp matching there).
    ///
    /// Incremental per frame ([`Window`]). The scan stops once no candidate
    /// can end first (a match at or after the best end ends after it).
    /// Whether a candidate at `p > s0` matches does not depend on `s0` (the
    /// char before it is a border or `p` starts a line, whichever `s0`); at
    /// `p == s0` only a line start can, so a candidate there that is not
    /// one is dropped for good, as a later search starts at or after `s0`.
    fn search_split(
        &self,
        cx: &InlineCx,
        s0: usize,
        (floor, end): (usize, usize),
        cur: &mut Cursor,
    ) -> Work<Option<(Match, Option<Match>)>> {
        self.charge()?;
        let w = cur.window.get_or_insert_with(Box::default);
        let passed = |p: usize| p < s0 || (p == s0 && !cx.at_bol(p, floor));
        let moved = self.drop_passed(w, &passed)?;
        self.refront(cx, w, moved, (floor, end))?;
        w.scan = w.scan.max(self.starts.partition_point(|&p| p < s0));
        while let Some(&p) = self.starts.get(w.scan).filter(|&&p| p < end) {
            if w.best().is_some_and(|b| p >= b.m0) {
                break;
            }
            self.visit()?;
            w.scan += 1;
            if passed(p) {
                continue;
            }
            match self.regexps_at(p, end) {
                Some((lo, hi)) if !w.all_covered(lo, hi) => {
                    let table = self.examine(cx, p, (floor, end))?;
                    w.record(p, table, &HashSet::new());
                }
                // No regexp can match here, or each that can has an earlier
                // front: nothing to learn unless a front moves past it.
                Some(_) => w.skipped.push_back(p),
                None => {}
            }
        }
        let Some(m) = w.best() else {
            return Ok(None);
        };
        if let Some((lm, link)) = w.last.filter(|l| l.0 == m) {
            return Ok(Some((lm, link)));
        }
        // An examined candidate's table is the link parser's at the same
        // position: it reaches it from a line start or past a border too.
        let known = |q: usize| w.hits.get(&q).cloned();
        let link = self.looking_at_with(cx, m.p, (floor, end), &known)?;
        w.last = Some((m, link));
        Ok(Some((m, link)))
    }

    /// One candidate visited: charged (and counted, under test).
    fn visit(&self) -> Work<()> {
        #[cfg(test)]
        tests::VISITS.with(|v| v.set(v.get() + 1));
        self.charge()
    }

    /// Drops the matches and skipped candidates the search start has
    /// `passed`; the regexps whose fronts were among them.
    fn drop_passed(&self, w: &mut Window, passed: &dyn Fn(usize) -> bool) -> Work<HashSet<u32>> {
        let mut moved = HashSet::new();
        while let Some((&p, _)) = w.hits.first_key_value().filter(|(&p, _)| passed(p)) {
            self.visit()?;
            let table = w.hits.remove(&p).unwrap_or_default();
            for (j, _, _) in table {
                w.of.entry(j).or_default().remove(&p);
                if w.front.get(&j).is_some_and(|f| f.0 == p) {
                    w.clear_front(j);
                    moved.insert(j);
                }
            }
        }
        while w.skipped.front().is_some_and(|&p| passed(p)) {
            self.visit()?;
            w.skipped.pop_front();
        }
        Ok(moved)
    }

    /// A moved front is its regexp's next examined match, unless a skipped
    /// candidate before it holds one: those are examined first, in order,
    /// until none is left before it.
    fn refront(
        &self,
        cx: &InlineCx,
        w: &mut Window,
        mut moved: HashSet<u32>,
        region: (usize, usize),
    ) -> Work<()> {
        let pending: Vec<u32> = moved.iter().copied().collect();
        for j in pending {
            loop {
                let next = w.first_of(j);
                let Some(&p) = w.skipped.front().filter(|&&p| next.is_none_or(|n| p < n.0)) else {
                    if let Some((p, m0)) = next {
                        w.set_front(j, p, m0);
                    }
                    break;
                };
                self.visit()?;
                w.skipped.pop_front();
                let table = self.examine(cx, p, region)?;
                w.record(p, table, &moved);
            }
            moved.remove(&j);
        }
        Ok(())
    }

    /// Every regexp's first target matching at candidate `p` (reachable
    /// from any search start before it), each match charged.
    fn examine(&self, cx: &InlineCx, p: usize, (floor, end): (usize, usize)) -> Work<PartMatches> {
        let mut table = Vec::new();
        self.each_part_at(cx, p, floor, (floor, end), &mut table)?;
        for _ in &table {
            self.charge()?;
        }
        Ok(table)
    }

    /// The least and greatest regexp holding a target on the trie paths
    /// from `p` (cut at the region's `end`): what can match at `p`. `None`
    /// when nothing can.
    fn regexps_at(&self, p: usize, end: usize) -> Option<(u32, u32)> {
        let (k, _, reach) = self.locate(p, end)?;
        let (mut lo, mut hi) = (NONE, 0u32);
        for m in [&self.plain, &self.literal] {
            let (a, b) = m.trie.parts_up[m.trie.level_ancestor(m.deep[k], reach) as usize];
            (lo, hi) = (lo.min(a), hi.max(b));
        }
        (lo <= hi).then_some((lo, hi))
    }

    /// `org-element-link-parser`'s radio case at `i`: the regexp(s) tried
    /// from `i` at a line start, else from the char before it. The search's
    /// answer is read back without work; elsewhere, `None` once the work
    /// has run out.
    fn link_at(&self, cx: &InlineCx, i: usize, (floor, end): (usize, usize)) -> Option<Match> {
        #[cfg(test)]
        if tests::FORGET_ANSWER.with(Cell::get) {
            self.answer.set(None);
        }
        if let Some(a) = self.answer.get().filter(|a| a.at == (i, floor, end)) {
            return a.link;
        }
        if self.exhausted.get() {
            return None;
        }
        self.asked_at.set(i);
        self.looking_at(cx, i, (floor, end)).ok().flatten()
    }

    /// The link parser's match at `i` (`looking-at`, or with a split the
    /// first regexp that matches). At a line start a regexp tries `^`, then
    /// the char at `i` as the border; elsewhere the char before `i` is the
    /// border (`^` there would put the link's text before `i`, which Org
    /// never reaches; see `limit`).
    fn looking_at(&self, cx: &InlineCx, i: usize, region: (usize, usize)) -> Work<Option<Match>> {
        self.looking_at_with(cx, i, region, &|_| None)
    }

    /// [`Radio::looking_at`], with a split's per-position matches already
    /// known at some positions (`known`).
    fn looking_at_with(
        &self,
        cx: &InlineCx,
        i: usize,
        (floor, end): (usize, usize),
        known: &dyn Fn(usize) -> Option<PartMatches>,
    ) -> Work<Option<Match>> {
        let bol = cx.at_bol(i, floor);
        let border_at_i = bol && cx.ch(i, end).is_some_and(boundary);
        let s = if bol {
            i
        } else {
            i - cx.char_before(i, floor).map_or(1, char::len_utf8)
        };
        if self.parts == 1 {
            if let Some(m) = self.first_at(cx, i, s, (floor, end))? {
                return Ok(Some(m));
            }
            if border_at_i {
                return self.first_at(cx, cx.step(i), i, (floor, end));
            }
            return Ok(None);
        }
        let table = |q: usize, from: usize| -> Work<PartMatches> {
            if let Some(t) = known(q) {
                return Ok(t);
            }
            let mut t = Vec::new();
            self.each_part_at(cx, q, from, (floor, end), &mut t)?;
            Ok(t)
        };
        let at = table(i, s)?;
        let next = if border_at_i {
            table(cx.step(i), i)?
        } else {
            Vec::new()
        };
        // The first regexp matching either way; within one, `^` first.
        let a = at.iter().min_by_key(|e| e.0);
        let b = next.iter().min_by_key(|e| e.0);
        Ok(match (a, b) {
            (Some(a), Some(b)) if b.0 < a.0 => Some(b.2),
            (Some(a), _) => Some(a.2),
            (None, b) => b.map(|b| b.2),
        })
    }

    /// Can a match with group 1 at `p` be reached by a search from `s0`?
    fn reachable(&self, cx: &InlineCx, p: usize, s0: usize, floor: usize) -> bool {
        let bol_branch = p >= s0 && cx.at_bol(p, floor);
        let char_branch = p > floor
            && p - cx.char_before(p, floor).map_or(1, char::len_utf8) >= s0
            && cx.ch_before(p, floor).is_some_and(boundary);
        bol_branch || char_branch
    }

    /// The symbol `p` is in, how many symbols from it end by `end` (`fit`)
    /// and start before it (`reach`).
    fn locate(&self, p: usize, end: usize) -> Option<(usize, usize, usize)> {
        let k = self.sym.partition_point(|&s| s <= p).checked_sub(1)?;
        if k + 1 >= self.sym.len() {
            return None;
        }
        let fit = (self.sym.partition_point(|&s| s <= end) - 1).saturating_sub(k);
        let reach = self.sym.partition_point(|&s| s < end) - k;
        Some((k, fit, reach))
    }

    /// One regexp: the first target in regexp order whose match has group
    /// 1 at `p`, reached from a search start `s0`.
    fn first_at(
        &self,
        cx: &InlineCx,
        p: usize,
        s0: usize,
        region: (usize, usize),
    ) -> Work<Option<Match>> {
        self.charge()?;
        let (floor, end) = region;
        if p >= end || p < floor || !self.reachable(cx, p, s0, floor) {
            return Ok(None);
        }
        let Some((k, fit, reach)) = self.locate(p, end) else {
            return Ok(None);
        };
        let mut best: Option<(u32, Match)> = None;
        if !self.ws[k] {
            if self.sym[k] != p {
                return Ok(None);
            }
            for m in [&self.plain, &self.literal] {
                let Some((t, g1)) = m.first_at(cx, &self.sym, k, fit, end) else {
                    continue;
                };
                if best.is_some_and(|b| b.0 <= t) {
                    continue;
                }
                let Some(m0) = after(cx, g1, end) else {
                    return Ok(None);
                };
                best = Some((t, Match { p, g1, m0 }));
            }
        }
        self.mixed_walk(cx, (k, p), (reach, end), &mut Sink::First(&mut best))?;
        Ok(best.map(|(_, m)| m))
    }

    /// A split: for each regexp with a match at `p` (reached from `s0`),
    /// its first target there: `(regexp, target, match)`.
    fn each_part_at(
        &self,
        cx: &InlineCx,
        p: usize,
        s0: usize,
        (floor, end): (usize, usize),
        out: &mut PartMatches,
    ) -> Work<()> {
        self.charge()?;
        if p >= end || p < floor || !self.reachable(cx, p, s0, floor) {
            return Ok(());
        }
        let Some((k, fit, reach)) = self.locate(p, end) else {
            return Ok(());
        };
        let mut sink = Sink::Parts(out, &self.part);
        if !self.ws[k] {
            if self.sym[k] != p {
                return Ok(());
            }
            for m in [&self.plain, &self.literal] {
                self.trie_matches(cx, m, (k, p), (fit, end), &mut sink)?;
            }
        }
        self.mixed_walk(cx, (k, p), (reach, end), &mut sink)
    }

    /// The matched targets of `m`'s trie on the path from symbol `k` (cut
    /// at `fit` symbols), each offered to `sink`: the deepest node's checked
    /// against the text after it, inner ones against the path's next symbol.
    fn trie_matches(
        &self,
        cx: &InlineCx,
        m: &Matcher,
        (k, p): (usize, usize),
        (fit, end): (usize, usize),
        sink: &mut Sink,
    ) -> Work<()> {
        let tr = &m.trie;
        let c = tr.level_ancestor(m.deep[k], fit);
        let mut offer = |u: u32, inner: bool| {
            let d = tr.depth[u as usize] as usize;
            let g1 = self.sym[k + d];
            let borders = !inner || self.ws[k + d] || cx.ch(g1, end).is_some_and(boundary);
            if let Some(m0) = after(cx, g1, end).filter(|_| borders) {
                for &t in tr.own_of(u) {
                    sink.take(t, Match { p, g1, m0 });
                }
            }
        };
        if tr.has_own(c) {
            offer(c, false);
        }
        let mut u = tr.own_up_above(c);
        while u != NONE {
            self.charge()?;
            offer(u, true);
            u = tr.own_up_above(u);
        }
        Ok(())
    }

    /// Mixed targets on the run-collapsed path from symbol `k` (cut at
    /// `reach` symbols), deepest first, in regexp order within a node:
    /// each one the sink still wants gets its exact match.
    fn mixed_walk(
        &self,
        cx: &InlineCx,
        (k, p): (usize, usize),
        (reach, end): (usize, usize),
        sink: &mut Sink,
    ) -> Work<()> {
        let tr = &self.plain.trie;
        // Every mixed target holds whitespace other than a space, which only
        // a run of the text holding such a char can match.
        let odd = self.odd_next[k] as usize;
        let mut u = tr.mixed_up[tr.level_ancestor(self.plain.deep[k], reach) as usize];
        while u != NONE {
            let ui = u as usize;
            if odd >= tr.depth[ui] as usize {
                break;
            }
            #[cfg(test)]
            tests::WALKED.with(|w| w.set(w.get() + 1));
            self.charge()?;
            let lead = tr.lead_key[ui];
            if lead != NONE && !self.lead_matches(cx, lead, p, end)? {
                // Every target here starts with this stretch, and so do the
                // nodes `lead_skip` passes over.
                u = tr.lead_skip[ui];
                continue;
            }
            let (a, b) = tr.mixed[ui];
            for &t in &tr.mixed_list[a as usize..b as usize] {
                if sink.bound() <= t {
                    break;
                }
                if !sink.wants(t) {
                    continue;
                }
                #[cfg(test)]
                tests::WALKED.with(|w| w.set(w.get() + 1));
                self.charge()?;
                if let Some((g1, m0)) = self.exact(cx, t, p, end)? {
                    sink.take(t, Match { p, g1, m0 });
                }
            }
            u = match tr.parent[ui] {
                pu if pu == u => NONE,
                pu => tr.mixed_up[pu as usize],
            };
        }
        Ok(())
    }

    /// Does lead group `lead` (a leading whitespace stretch and the char
    /// after it) match from `p`?
    fn lead_matches(&self, cx: &InlineCx, lead: u32, p: usize, end: usize) -> Work<bool> {
        let id = self.targets.len() as u32 + lead;
        Ok(self.exact(cx, id, p, end)?.is_some())
    }

    /// Where the whitespace from `pos` ends, not past `end` (`pos` itself
    /// if there is none): `\s-+`'s longest extent.
    fn run_end(&self, pos: usize, end: usize) -> usize {
        match self.sym.partition_point(|&s| s <= pos).checked_sub(1) {
            Some(k) if k < self.ws.len() && self.ws[k] => self.sym[k + 1].min(end).max(pos),
            _ => pos,
        }
    }

    fn spend(&self) -> bool {
        #[cfg(test)]
        tests::WORK.with(|w| w.set(w.get() + 1));
        match self.budget.get() {
            0 => {
                self.exhausted.set(true);
                false
            }
            b => {
                self.budget.set(b - 1);
                true
            }
        }
    }

    fn charge(&self) -> Work<()> {
        if self.spend() {
            Ok(())
        } else {
            Err(OutOfWork)
        }
    }

    /// The exact regexp for mixed target (or lead group) `id` at `p`, with
    /// Emacs's backtracking order: a run tries its longest extent first.
    /// A target's match needs its after-context; a lead group's only its
    /// items. A state that fails is remembered, so no state is explored
    /// twice across calls while the memo holds it.
    fn exact(&self, cx: &InlineCx, id: u32, p: usize, end: usize) -> Work<Option<(usize, usize)>> {
        let n = self.targets.len();
        let (items, after_context) = match self.targets.get(id as usize) {
            Some(t) => (&t.items[..], true),
            None => (&self.leads[id as usize - n][..], false),
        };
        let mut run = Exact {
            radio: self,
            cx,
            id,
            end,
            items,
            after_context,
            memo: &mut self.memo.borrow_mut(),
            stack: Vec::new(),
        };
        let mut state = (0usize, p);
        loop {
            match run.enter(state) {
                Step::Matched(m) => return Ok(Some(m)),
                Step::Next(s) => state = s,
                Step::Failed => match run.backtrack() {
                    Some(s) => state = s,
                    None => return Ok(None),
                },
                Step::OutOfWork => return Err(OutOfWork),
            }
        }
    }
}

/// One exact match of a mixed target in progress.
struct Exact<'r, 'a> {
    radio: &'r Radio,
    cx: &'r InlineCx<'a>,
    id: u32,
    end: usize,
    items: &'r [Item],
    after_context: bool,
    memo: &'r mut Memo,
    /// Frames: (item, position, for a run the end being tried and the
    /// run's extent).
    stack: Vec<(usize, usize, usize, usize)>,
}

enum Step {
    Matched((usize, usize)),
    Next((usize, usize)),
    Failed,
    OutOfWork,
}

impl Exact<'_, '_> {
    /// Tries item `k` at `pos`. A state known to fail costs no work. A
    /// state that fails at once (a char that differs, an after-context that
    /// does not hold) is not remembered: trying it again costs one compare,
    /// and the capped memo is kept for failures found by exploring.
    fn enter(&mut self, (k, pos): (usize, usize)) -> Step {
        let (cx, end) = (self.cx, self.end);
        if self.memo.failed.contains(&key(self.id, k, end, pos)) {
            return Step::Failed;
        }
        if !self.radio.spend() {
            return Step::OutOfWork;
        }
        match self.items.get(k) {
            None if !self.after_context => return Step::Matched((pos, pos)),
            None => {
                if let Some(m0) = after(cx, pos, end) {
                    return Step::Matched((pos, m0));
                }
            }
            Some(&Item::Lit(c)) => {
                if cx.ch(pos, end).is_some_and(|d| canon(d) == c) {
                    self.stack.push((k, pos, 0, 0));
                    return Step::Next((k + 1, cx.step(pos)));
                }
            }
            Some(Item::Run) => {
                // The run's extent from the symbol table: scanning it here
                // would cost its length per call.
                let q = self.radio.run_end(pos, end);
                let known = self
                    .memo
                    .run_failed
                    .get(&key(self.id, k, end, q))
                    .is_some_and(|&f| f as usize <= pos);
                if q > pos && !known {
                    self.stack.push((k, pos, q, q));
                    return Step::Next((k + 1, q));
                }
            }
        }
        Step::Failed
    }

    fn fail(&mut self, k: usize, pos: usize) {
        self.memo.note();
        self.memo.failed.insert(key(self.id, k, self.end, pos));
    }

    /// After a failed state: the next state to try, the most recent run
    /// giving up one more char; `None` when every choice failed.
    fn backtrack(&mut self) -> Option<(usize, usize)> {
        loop {
            let top = self.stack.last_mut()?;
            let (k, pos, tried, extent) = *top;
            if self.items[k] == Item::Run {
                let shorter = back(self.cx, tried);
                if shorter > pos {
                    top.2 = shorter;
                    return Some((k + 1, shorter));
                }
                let rk = key(self.id, k, self.end, extent);
                if !self.memo.run_failed.contains_key(&rk) {
                    self.memo.note();
                }
                let f = self.memo.run_failed.entry(rk).or_insert(pos as u32);
                *f = (*f).min(pos as u32);
            } else {
                self.fail(k, pos);
            }
            self.stack.pop();
        }
    }
}

/// The position of the char before `q` as [`InlineCx::ch`] reads chars (a
/// CRLF is one).
fn back(cx: &InlineCx, q: usize) -> usize {
    let b = cx.bytes();
    if q >= 2
        && b[q - 1] == b'\n'
        && b[q - 2] == b'\r'
        && cx.src.eol == crate::text::LineEnding::CrLf
    {
        return q - 2;
    }
    q - cx.char_before(q, 0).map_or(1, char::len_utf8)
}

/// The work budget: proportional to the text (a test may override it).
fn budget_for(len: usize) -> u64 {
    #[cfg(test)]
    if let Some(b) = tests::BUDGET.with(Cell::get) {
        return b;
    }
    8 * len as u64 + 65_536
}

/// `[^[:alnum:]]` or `\c|`: a char that may border a radio link.
fn boundary(c: char) -> bool {
    !is_alnum(c) || is_linebreak(c)
}

/// Does the symbol's first char border a link (a run starts with blank)?
fn boundary_symbol(s: u32) -> bool {
    s == W || s >= RUN_BASE || char::from_u32(s).is_none_or(boundary)
}

/// The after-context at `g1`: `$`, or one bordering char. Where the match ends.
fn after(cx: &InlineCx, g1: usize, end: usize) -> Option<usize> {
    if cx.at_eol(g1, end) {
        return Some(g1);
    }
    cx.ch(g1, end).filter(|&c| boundary(c)).map(|_| cx.step(g1))
}

fn compile(value: &str) -> Target {
    let mut items = Vec::new();
    for c in value.chars() {
        if c == ' ' {
            if items.last() != Some(&Item::Run) {
                items.push(Item::Run);
            }
        } else {
            items.push(Item::Lit(canon(c)));
        }
    }
    let ws_lit = |i: &Item| matches!(i, Item::Lit(c) if is_space(*c));
    let blank = |i: &Item| *i == Item::Run || ws_lit(i);
    let shape = if !items.iter().any(ws_lit) {
        Shape::Simple
    } else if !items.contains(&Item::Run)
        && !items.first().is_some_and(blank)
        && !items.last().is_some_and(blank)
    {
        Shape::Literal
    } else {
        Shape::Mixed
    };
    Target {
        items,
        shape,
        chars: value.chars().count(),
        lead: NONE,
    }
}

/// Each mixed target's lead group (`Target::lead`): its leading whitespace
/// stretch and the char after it, one id per distinct group. The groups.
fn assign_leads(targets: &mut [Target]) -> Vec<Vec<Item>> {
    let blank = |i: &Item| *i == Item::Run || matches!(i, Item::Lit(c) if is_space(*c));
    let mut leads: Vec<Vec<Item>> = Vec::new();
    let mut ids: HashMap<Vec<Item>, u32> = HashMap::new();
    for t in targets.iter_mut().filter(|t| t.shape == Shape::Mixed) {
        let Some(j) = t.items.iter().position(|i| !blank(i)).filter(|&j| j > 0) else {
            continue;
        };
        let lead = t.items[..=j].to_vec();
        let next = leads.len() as u32;
        t.lead = *ids.entry(lead.clone()).or_insert_with(|| {
            leads.push(lead);
            next
        });
    }
    leads
}

/// Org's split of the targets (in regexp order) into regexps
/// (`org-update-radio-target-regexp`, Org 9.7.11): none unless the single
/// regexp would be at least `limit` chars long; otherwise targets are taken
/// in order, and a new regexp starts before a target when the current one
/// holds some target and its length so far — the borders plus the RAW
/// lengths of its targets, as Org counts it — plus this target's reaches
/// `limit`. The regexp of each target.
fn partitions(values: &[&str], targets: &[Target], limit: usize) -> Vec<u32> {
    let mut part = vec![0u32; targets.len()];
    let whole = BORDERS_LEN
        + 2 * targets.len().saturating_sub(1)
        + values.iter().map(|v| quoted_len(v)).sum::<usize>();
    if whole < limit {
        return part;
    }
    let (mut j, mut len, mut held) = (0u32, BORDERS_LEN, false);
    for (i, t) in targets.iter().enumerate() {
        if held && len + t.chars >= limit {
            j += 1;
            len = BORDERS_LEN;
        }
        part[i] = j;
        held = true;
        len += t.chars;
    }
    part
}

/// The length of a target in Org's regexp: `regexp-quote` (a backslash
/// before each of `[*.\?+^$`), then each run of spaces as `\s-+`.
fn quoted_len(value: &str) -> usize {
    let mut n = 0;
    let mut in_spaces = false;
    for c in value.chars() {
        if c == ' ' {
            if !in_spaces {
                n += 4;
            }
            in_spaces = true;
            continue;
        }
        in_spaces = false;
        n += if "[*.\\?+^$".contains(c) { 2 } else { 1 };
    }
    n
}

/// The target as run-collapsed symbols: each maximal whitespace stretch is `W`.
fn collapsed(t: &Target) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for i in &t.items {
        let s = match *i {
            Item::Lit(c) if !is_space(c) => c as u32,
            _ => W,
        };
        if !(s == W && out.last() == Some(&W)) {
            out.push(s);
        }
    }
    out
}

/// A literal target as exact symbols (each whitespace stretch the id of
/// that exact run), or `None` if a stretch never occurs in the text.
fn exact_symbols(t: &Target, runs: &HashMap<String, u32>) -> Option<Vec<u32>> {
    let mut out = Vec::new();
    let mut stretch = String::new();
    for i in &t.items {
        let Item::Lit(c) = *i else { return None };
        if is_space(c) {
            stretch.push(c);
            continue;
        }
        if !stretch.is_empty() {
            out.push(RUN_BASE + *runs.get(&stretch)?);
            stretch.clear();
        }
        out.push(c as u32);
    }
    Some(out)
}

/// The text as symbols.
struct Symbols {
    sym: Vec<usize>,
    ws: Vec<bool>,
    /// Whether each symbol is a run holding whitespace other than a space.
    odd: Vec<bool>,
    /// Run-collapsed symbols.
    plain: Vec<u32>,
    /// Runs as their exact contents.
    exact: Vec<u32>,
    /// Exact run contents (as `InlineCx::ch` reads them) to ids.
    runs: HashMap<String, u32>,
}

impl Symbols {
    fn read(cx: &InlineCx) -> Symbols {
        let len = cx.src.len();
        let mut s = Symbols {
            sym: Vec::new(),
            ws: Vec::new(),
            odd: Vec::new(),
            plain: Vec::new(),
            exact: Vec::new(),
            runs: HashMap::new(),
        };
        let mut i = cx.src.bom;
        let mut run = String::new();
        while let Some(c) = cx.ch(i, len) {
            s.sym.push(i);
            if is_space(c) {
                run.clear();
                let mut j = i;
                while let Some(d) = cx.ch(j, len).filter(|&d| is_space(d)) {
                    run.push(canon(d));
                    j = cx.step(j);
                }
                let next = s.runs.len() as u32;
                let id = *s.runs.entry(run.clone()).or_insert(next);
                s.ws.push(true);
                s.odd.push(run.chars().any(|c| c != ' '));
                s.plain.push(W);
                s.exact.push(RUN_BASE + id);
                i = j;
            } else {
                let c = canon(c) as u32;
                s.ws.push(false);
                s.odd.push(false);
                s.plain.push(c);
                s.exact.push(c);
                i = cx.step(i);
            }
        }
        s.sym.push(len.max(cx.src.bom));
        s
    }
}

/// A trie of target symbol sequences, and for each text symbol the deepest
/// node the text from it reaches.
struct Matcher {
    trie: Trie,
    deep: Vec<u32>,
}

impl Matcher {
    /// `seqs`: (target, symbols, whether a trie match is a match).
    fn new(
        seqs: &[(u32, Vec<u32>, bool)],
        text: &[u32],
        targets: &[Target],
        part: &[u32],
    ) -> Matcher {
        let trie = Trie::new(seqs, targets, part);
        let deep = deepest(&trie, seqs, text);
        Matcher { trie, deep }
    }

    /// The first target in regexp order that this trie matches from symbol
    /// `k` within `fit` symbols (those ending by the region's `end`): its
    /// index and group 1's end.
    fn first_at(
        &self,
        cx: &InlineCx,
        sym: &[usize],
        k: usize,
        fit: usize,
        end: usize,
    ) -> Option<(u32, usize)> {
        let tr = &self.trie;
        let c = tr.level_ancestor(self.deep[k], fit) as usize;
        let mut best: Option<(u32, usize)> = None;
        let (t, d) = tr.inner[c];
        if t != NONE {
            best = Some((t, sym[k + d as usize]));
        }
        let own = tr.own[c];
        if own != NONE && best.is_none_or(|b| own < b.0) {
            let g1 = sym[k + tr.depth[c] as usize];
            if after(cx, g1, end).is_some() {
                best = Some((own, g1));
            }
        }
        best
    }
}

/// A trie just built from its sequences: the trie (lists still empty),
/// each node's edge symbol, and the matched and mixed targets ending there.
type Inserted = (Trie, Vec<u32>, Vec<Vec<u32>>, Vec<Vec<u32>>);

struct Trie {
    next: HashMap<(u32, u32), u32>,
    parent: Vec<u32>,
    depth: Vec<u32>,
    /// Jump pointers (Myers): level ancestors in O(log depth).
    jump: Vec<u32>,
    /// The first matched target (regexp order) ending here.
    own: Vec<u32>,
    /// Every matched target ending here that is the first of its regexp
    /// (of Org's split) to end here: a range of `own_list`. One regexp: just
    /// `own`.
    own_at: Vec<(u32, u32)>,
    own_list: Vec<u32>,
    /// The nearest ancestor-or-self where a matched target ends.
    own_up: Vec<u32>,
    /// The first matched target ending at a proper ancestor whose next
    /// symbol toward here borders a link, and that ancestor's depth.
    inner: Vec<(u32, u32)>,
    /// Mixed targets ending here, in regexp order, those with the same items
    /// in the same regexp kept once: a range of `mixed_list`.
    mixed: Vec<(u32, u32)>,
    mixed_list: Vec<u32>,
    /// The nearest ancestor-or-self holding mixed targets.
    mixed_up: Vec<u32>,
    /// The lead group (see `Target::lead`) shared by every mixed target
    /// here, if they share one; else `NONE`.
    lead_key: Vec<u32>,
    /// For a node holding mixed targets: the nearest ancestor holding mixed
    /// targets that does not share its lead group (`NONE`: none).
    lead_skip: Vec<u32>,
    /// Some target ends here or at an ancestor (the root excluded).
    any: Vec<bool>,
    /// The least and greatest regexp (of Org's split) holding a target
    /// that ends here or at an ancestor (`NONE`, 0 when none).
    parts_up: Vec<(u32, u32)>,
}

impl Trie {
    fn new(seqs: &[(u32, Vec<u32>, bool)], targets: &[Target], part: &[u32]) -> Trie {
        let (mut tr, edge, mut owns, mut mixed_of) = Trie::insert(seqs);
        // Nodes are numbered after their parent: one pass in index order.
        for v in 0..tr.parent.len() {
            let (held, here) = tr.fill_lists(
                v,
                std::mem::take(&mut owns[v]),
                std::mem::take(&mut mixed_of[v]),
                targets,
                part,
            );
            if v > 0 {
                tr.link_node(v, edge[v], held, here);
            }
        }
        tr
    }

    /// The trie of `seqs`, with each node's edge symbol and the matched and
    /// mixed targets ending at it.
    fn insert(seqs: &[(u32, Vec<u32>, bool)]) -> Inserted {
        let mut next: HashMap<(u32, u32), u32> = HashMap::new();
        let (mut parent, mut depth, mut edge) = (vec![0u32], vec![0u32], vec![W]);
        let mut owns: Vec<Vec<u32>> = vec![Vec::new()];
        let mut mixed_of: Vec<Vec<u32>> = vec![Vec::new()];
        for (t, syms, matched) in seqs {
            let mut node = 0u32;
            for &s in syms {
                node = *next.entry((node, s)).or_insert_with(|| {
                    parent.push(node);
                    depth.push(depth[node as usize] + 1);
                    edge.push(s);
                    owns.push(Vec::new());
                    mixed_of.push(Vec::new());
                    (parent.len() - 1) as u32
                });
            }
            let n = node as usize;
            if node == 0 {
                continue; // an empty target matches nothing (none exists)
            }
            if *matched {
                owns[n].push(*t);
            } else {
                mixed_of[n].push(*t);
            }
        }
        let count = parent.len();
        let tr = Trie {
            next,
            jump: vec![0; count],
            own: vec![NONE; count],
            own_at: Vec::with_capacity(count),
            own_list: Vec::new(),
            own_up: vec![NONE; count],
            inner: vec![(NONE, 0); count],
            mixed: Vec::with_capacity(count),
            mixed_list: Vec::new(),
            mixed_up: vec![NONE; count],
            lead_key: vec![NONE; count],
            lead_skip: vec![NONE; count],
            any: vec![false; count],
            parts_up: vec![(NONE, 0); count],
            parent,
            depth,
        };
        (tr, edge, owns, mixed_of)
    }

    /// Node `v`'s own target lists: matched targets, the first per regexp;
    /// mixed ones, one per (regexp, items). Whether it holds mixed targets,
    /// and the range of regexps of its targets.
    fn fill_lists(
        &mut self,
        v: usize,
        mut owns: Vec<u32>,
        mut mixed: Vec<u32>,
        targets: &[Target],
        part: &[u32],
    ) -> (bool, (u32, u32)) {
        owns.sort_unstable();
        owns.dedup_by_key(|t| part[*t as usize]);
        self.own[v] = owns.first().copied().unwrap_or(NONE);
        let here = owns.iter().chain(&mixed).fold((NONE, 0u32), |h, &t| {
            let j = part[t as usize];
            (h.0.min(j), h.1.max(j))
        });
        let a = self.own_list.len() as u32;
        self.own_list.extend(owns);
        self.own_at.push((a, self.own_list.len() as u32));
        let a = self.mixed_list.len() as u32;
        mixed.sort_unstable();
        let mut kept: HashSet<(u32, &[Item])> = HashSet::new();
        mixed.retain(|&t| kept.insert((part[t as usize], &targets[t as usize].items[..])));
        let leads: HashSet<u32> = mixed.iter().map(|&t| targets[t as usize].lead).collect();
        self.mixed_list.extend(mixed);
        self.mixed.push((a, self.mixed_list.len() as u32));
        let held = self.mixed_list.len() as u32 > a;
        if held && leads.len() == 1 {
            self.lead_key[v] = leads.into_iter().next().unwrap_or(NONE);
        }
        (held, here)
    }

    /// Node `v`'s pointers from its parent's (computed first): jump,
    /// inner target, nearest holders, lead skip, regexp range.
    fn link_node(&mut self, v: usize, edge: u32, held: bool, here: (u32, u32)) {
        let p = self.parent[v] as usize;
        let j1 = self.jump[p] as usize;
        let j2 = self.jump[j1] as usize;
        self.jump[v] =
            if p != 0 && self.depth[p] - self.depth[j1] == self.depth[j1] - self.depth[j2] {
                j2 as u32
            } else {
                p as u32
            };
        let mut inner = self.inner[p];
        if self.own[p] != NONE && boundary_symbol(edge) && self.own[p] < inner.0 {
            inner = (self.own[p], self.depth[p]);
        }
        self.inner[v] = inner;
        self.own_up[v] = if self.own[v] != NONE {
            v as u32
        } else {
            self.own_up[p]
        };
        let up = self.mixed_up[p];
        self.mixed_up[v] = if held { v as u32 } else { up };
        if held {
            let key = self.lead_key[v];
            let same = up != NONE && key != NONE && self.lead_key[up as usize] == key;
            self.lead_skip[v] = if same {
                self.lead_skip[up as usize]
            } else {
                up
            };
        }
        self.any[v] = self.any[p] || held || self.own[v] != NONE;
        let up = self.parts_up[p];
        self.parts_up[v] = (here.0.min(up.0), here.1.max(up.1));
    }

    /// The ancestor-or-self of `v` at depth `min(depth(v), d)`.
    fn level_ancestor(&self, mut v: u32, d: usize) -> u32 {
        let d = d.min(u32::MAX as usize) as u32;
        while self.depth[v as usize] > d {
            let j = self.jump[v as usize];
            v = if self.depth[j as usize] >= d {
                j
            } else {
                self.parent[v as usize]
            };
        }
        v
    }

    fn has_own(&self, v: u32) -> bool {
        self.own[v as usize] != NONE
    }

    /// The matched targets ending at `v` (one per regexp).
    fn own_of(&self, v: u32) -> &[u32] {
        let (a, b) = self.own_at[v as usize];
        &self.own_list[a as usize..b as usize]
    }

    /// The nearest proper ancestor of `v` where a matched target ends.
    fn own_up_above(&self, v: u32) -> u32 {
        if v == 0 {
            NONE
        } else {
            self.own_up[self.parent[v as usize] as usize]
        }
    }
}

/// A suffix automaton of the reversed target sequences (generalized:
/// several strings, built online with the existing-transition check).
///
/// Transitions live in one hash map, so adding one is O(1) whatever the
/// alphabet: a sorted per-state list made each insert O(distinct symbols),
/// and one target of K distinct chars in ascending order O(K × length).
/// Each state also lists its own symbols, for cloning.
struct Sam {
    len: Vec<u32>,
    link: Vec<u32>,
    next: HashMap<(u32, u32), u32>,
    syms: Vec<Vec<u32>>,
}

impl Sam {
    fn get(&self, v: u32, c: u32) -> Option<u32> {
        self.next.get(&(v, c)).copied()
    }

    fn set(&mut self, v: u32, c: u32, to: u32) {
        if self.next.insert((v, c), to).is_none() {
            self.syms[v as usize].push(c);
        }
    }

    fn state(&mut self, len: u32, link: u32) -> u32 {
        self.len.push(len);
        self.link.push(link);
        self.syms.push(Vec::new());
        (self.len.len() - 1) as u32
    }

    /// Redirects the transitions on `c` to `q` from `p` and its suffix links
    /// to a clone of `q` of length `len`.
    fn split(&mut self, mut p: u32, c: u32, q: u32, len: u32) -> u32 {
        let clone = self.state(len, self.link[q as usize]);
        for i in 0..self.syms[q as usize].len() {
            let s = self.syms[q as usize][i];
            let to = self.next[&(q, s)];
            self.set(clone, s, to);
        }
        while p != NONE && self.get(p, c) == Some(q) {
            self.set(p, c, clone);
            p = self.link[p as usize];
        }
        self.link[q as usize] = clone;
        clone
    }

    fn extend(&mut self, last: u32, c: u32) -> u32 {
        let l = self.len[last as usize] + 1;
        if let Some(q) = self.get(last, c) {
            if self.len[q as usize] == l {
                return q;
            }
            return self.split(last, c, q, l);
        }
        let cur = self.state(l, 0);
        let mut p = last;
        while p != NONE && self.get(p, c).is_none() {
            self.set(p, c, cur);
            p = self.link[p as usize];
        }
        if p != NONE {
            let q = self.get(p, c).unwrap_or(0);
            self.link[cur as usize] = if self.len[p as usize] + 1 == self.len[q as usize] {
                q
            } else {
                self.split(p, c, q, self.len[p as usize] + 1)
            };
        }
        cur
    }

    /// The automaton of the sequences reversed, and the state of each whole
    /// reversed sequence.
    fn of_reversed(seqs: &[(u32, Vec<u32>, bool)]) -> (Sam, Vec<u32>) {
        let mut sam = Sam {
            len: vec![0],
            link: vec![NONE],
            next: HashMap::new(),
            syms: vec![Vec::new()],
        };
        let ends = seqs
            .iter()
            .map(|(_, syms, _)| syms.iter().rev().fold(0, |last, &s| sam.extend(last, s)))
            .collect();
        (sam, ends)
    }

    /// Shortest length of a state's strings.
    fn lo(&self, x: u32) -> usize {
        self.len[self.link[x as usize] as usize] as usize + 1
    }
}

/// The suffix automaton's states whose strings end some reversed target:
/// their strings are prefixes of targets, so they map to trie nodes.
struct PrefixStates {
    /// Per state: where its nodes start in `nodes` (`NONE`: not a prefix
    /// state), and how many it has (lengths `lo..=len`).
    off: Vec<u32>,
    count: Vec<u32>,
    nodes: Vec<u32>,
    /// Per state: the nearest prefix state on its suffix-link chain.
    nearest: Vec<u32>,
}

impl PrefixStates {
    fn new(sam: &Sam, ends: &[u32], trie: &Trie, seqs: &[(u32, Vec<u32>, bool)]) -> PrefixStates {
        let states = sam.len.len();
        let mut p = PrefixStates {
            off: vec![NONE; states],
            count: vec![0; states],
            nodes: vec![0],
            nearest: vec![0; states],
        };
        p.off[0] = 0;
        for ((_, syms, _), &end) in seqs.iter().zip(ends) {
            // The trie path of this sequence, by depth.
            let mut path = vec![0u32];
            for &s in syms {
                let v = path[path.len() - 1];
                path.push(trie.next.get(&(v, s)).copied().unwrap_or(0));
            }
            // Every string of a state that ends a reversed target is no
            // longer than that target (a longer one cannot end where it
            // ends), so `hi == len`. A filled state's chain is filled.
            let mut x = end;
            while x != 0 && p.off[x as usize] == NONE {
                let (lo, hi) = (sam.lo(x), (sam.len[x as usize] as usize).min(syms.len()));
                debug_assert_eq!(hi, sam.len[x as usize] as usize);
                p.off[x as usize] = p.nodes.len() as u32;
                p.count[x as usize] = (hi + 1).saturating_sub(lo) as u32;
                p.nodes.extend((lo..=hi).map(|l| path[l]));
                x = sam.link[x as usize];
            }
        }
        let mut order: Vec<u32> = (0..states as u32).collect();
        order.sort_unstable_by_key(|&x| sam.len[x as usize]);
        for x in order {
            let xi = x as usize;
            p.nearest[xi] = if p.off[xi] != NONE {
                x
            } else {
                p.nearest[sam.link[xi] as usize]
            };
        }
        p
    }

    /// The trie node of the string of length `l` in state `x`, or of the
    /// longest string of the nearest prefix state when `x` is not one.
    fn node(&self, sam: &Sam, x: u32, l: usize) -> u32 {
        let (x, l) = if self.off[x as usize] != NONE {
            (x, l)
        } else {
            let a = self.nearest[x as usize];
            (a, sam.len[a as usize] as usize)
        };
        if x == 0 {
            return 0;
        }
        let i = l
            .saturating_sub(sam.lo(x))
            .min(self.count[x as usize].saturating_sub(1) as usize);
        self.nodes[self.off[x as usize] as usize + i]
    }
}

/// For each text symbol, the deepest trie node the text from it reaches:
/// the longest prefix of the text there that is a prefix of some target.
/// Reading the text right to left against the suffix automaton of the
/// reversed targets gives, at each symbol, the longest such prefix that is
/// a SUBSTRING of a reversed target (matching statistics); a state whose
/// strings end some reversed target holds only prefixes of that target, so
/// the nearest such state on the suffix-link chain gives the answer, and its
/// strings map to trie nodes.
fn deepest(trie: &Trie, seqs: &[(u32, Vec<u32>, bool)], text: &[u32]) -> Vec<u32> {
    let (sam, ends) = Sam::of_reversed(seqs);
    let prefixes = PrefixStates::new(&sam, &ends, trie, seqs);
    let mut deep = vec![0u32; text.len()];
    let (mut cur, mut l) = (0u32, 0usize);
    for i in (0..text.len()).rev() {
        loop {
            if let Some(nx) = sam.get(cur, text[i]) {
                cur = nx;
                l += 1;
                break;
            }
            if cur == 0 {
                l = 0;
                break;
            }
            cur = sam.link[cur as usize];
            l = sam.len[cur as usize] as usize;
        }
        deep[i] = prefixes.node(&sam, cur, l);
    }
    deep
}

/// `org-element-link-parser`'s first case: a radio link at `i` when the
/// regexp (with a split, the first of the regexps) matches from `i` at a
/// line start, or from the char before it.
pub(crate) fn link(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    let m = cx.radio?.link_at(cx, i, (floor, end))?;
    let obj_end = cx.skip_blanks(m.g1, end);
    Some(
        Obj::new(LINK, i, obj_end)
            .tok(MARKUP, i, m.p)
            .contents(m.p, m.g1)
            .blank(m.g1),
    )
}

#[cfg(test)]
mod tests {
    use super::super::radio_reference as reference;
    use super::*;
    use crate::inline::InlineIndex;
    use crate::text::Source;
    use proptest::prelude::*;

    impl Radio {
        /// One regexp: the match with group 1 at `p` from `s0`, if any.
        fn valid_at(
            &self,
            cx: &InlineCx,
            p: usize,
            s0: usize,
            region: (usize, usize),
        ) -> Option<Match> {
            self.first_at(cx, p, s0, region).ok().flatten()
        }
    }

    /// The chars the generated targets and texts are made of: case pairs,
    /// a border char, a line-breakable (Han) char, space, tab, no-break
    /// space, ideographic space, line feed.
    const ALPHABET: &[char] = &['a', 'b', 'A', '-', '中', ' ', '\t', '\u{a0}', '\u{3000}'];

    fn compare(text: &str, values: &[String], regions: &[(usize, usize)]) -> Result<(), String> {
        let src = Source::new(text);
        let ix = InlineIndex::build(&src);
        let types: Vec<String> = Vec::new();
        let diagnostics = std::cell::RefCell::new(Vec::new());
        let cx = InlineCx {
            src: &src,
            ix: &ix,
            link_types: &types,
            radio: None,
            radio_limit: crate::ParseOptions::ORG_TARGET_LINK_REGEXP_LIMIT,
            diagnostics: &diagnostics,
        };
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let new = Radio::build(&cx, &refs);
        let old = reference::Radio::build(&cx, &refs);
        let bounds: Vec<usize> = (0..=text.len())
            .filter(|&i| text.is_char_boundary(i))
            .collect();
        let mut all = vec![(0, text.len())];
        all.extend(regions.iter().map(|&(a, b)| {
            let (a, b) = (bounds[a % bounds.len()], bounds[b % bounds.len()]);
            (a.min(b), a.max(b))
        }));
        for (floor, end) in all {
            for &p in bounds.iter().filter(|&&p| p >= floor && p < end) {
                let prev = bounds.iter().rev().find(|&&q| q < p).copied();
                let mut s0s = vec![p, floor];
                s0s.extend(prev.filter(|&q| q >= floor));
                for s0 in s0s {
                    let a = new.valid_at(&cx, p, s0, (floor, end));
                    let b = old.valid_at(&cx, p, s0, (floor, end));
                    let a = a.map(|m| (m.p, m.g1, m.m0));
                    let b = b.map(|m| (m.p, m.g1, m.m0));
                    if a != b {
                        return Err(format!(
                            "text {text:?} targets {values:?} region {floor}..{end} p {p} s0 {s0}: new {a:?} reference {b:?}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Org's split, transliterated from `org-update-radio-target-regexp`
    /// on its own (the regexp built as a string, then measured), for the
    /// split comparison below.
    fn org_split(values: &[String], limit: usize) -> Vec<Vec<String>> {
        let quote = |v: &str| {
            let mut q = String::new();
            for c in v.chars() {
                if "[*.\\?+^$".contains(c) {
                    q.push('\\');
                }
                q.push(c);
            }
            let mut out = String::new();
            let mut spaces = false;
            for c in q.chars() {
                if c == ' ' {
                    if !spaces {
                        out.push_str("\\s-+");
                    }
                    spaces = true;
                } else {
                    spaces = false;
                    out.push(c);
                }
            }
            out
        };
        let before = "\\(?:^\\|[^[:alnum:]]\\|\\c|\\)\\(";
        let after = "\\)\\(?:$\\|[^[:alnum:]]\\|\\c|\\)";
        let quoted: Vec<String> = values.iter().map(|v| quote(v)).collect();
        let whole = format!("{before}{}{after}", quoted.join("\\|"));
        if whole.chars().count() < limit {
            return vec![values.to_vec()];
        }
        let mut targets: std::collections::VecDeque<String> = values.iter().cloned().collect();
        let (mut out, mut sub, mut current): (Vec<Vec<String>>, Vec<String>, Option<usize>) =
            (Vec::new(), Vec::new(), None);
        while !targets.is_empty() || !sub.is_empty() {
            let cur = current.unwrap_or(0);
            let next_len = targets.front().map_or(0, |t| t.chars().count());
            if !sub.is_empty() && (targets.is_empty() || cur + next_len >= limit) {
                out.push(std::mem::take(&mut sub));
                current = None;
            }
            if current.is_none() {
                current = Some(before.chars().count() + after.chars().count());
            }
            if let Some(t) = targets.pop_front() {
                sub.push(t);
            }
            let last = if sub.is_empty() {
                0
            } else {
                sub[sub.len() - 1].chars().count()
            };
            current = current.map(|c| c + last);
        }
        out
    }

    /// The split search and link parser against the reference matcher run
    /// once per regexp of Org's split, combined as `org--re-list-search-
    /// forward` (the leftmost match of each, the one ending first, the
    /// earliest on a tie) and `org--re-list-looking-at` (the first regexp
    /// matching) do. Every region, search start and link position; the
    /// searches of a region also share one cursor, as a frame's do.
    fn compare_split(
        text: &str,
        values: &[String],
        regions: &[(usize, usize)],
        limit: usize,
    ) -> Result<(), String> {
        let src = Source::new(text);
        let ix = InlineIndex::build(&src);
        let types: Vec<String> = Vec::new();
        let diagnostics = std::cell::RefCell::new(Vec::new());
        let cx = InlineCx {
            src: &src,
            ix: &ix,
            link_types: &types,
            radio: None,
            radio_limit: limit,
            diagnostics: &diagnostics,
        };
        let (new, olds) = split_and_reference(&cx, values, limit)?;
        let bounds: Vec<usize> = (0..=text.len())
            .filter(|&i| text.is_char_boundary(i))
            .collect();
        let mut all = vec![(0, text.len())];
        all.extend(regions.iter().map(|&(a, b)| {
            let (a, b) = (bounds[a % bounds.len()], bounds[b % bounds.len()]);
            (a.min(b), a.max(b))
        }));
        let what = |region: (usize, usize)| {
            format!("text {text:?} targets {values:?} limit {limit} region {region:?}")
        };
        for region in all {
            let inside: Vec<usize> = bounds
                .iter()
                .copied()
                .filter(|&p| p >= region.0 && p <= region.1)
                .collect();
            split_searches(&cx, &new, &olds, region, &inside)
                .map_err(|e| format!("{} {e}", what(region)))?;
            split_links(&cx, &new, &olds, region, &inside)
                .map_err(|e| format!("{} {e}", what(region)))?;
        }
        Ok(())
    }

    fn triple(m: Option<Match>) -> Option<(usize, usize, usize)> {
        m.map(|m| (m.p, m.g1, m.m0))
    }

    /// The split mae-org makes, checked against Org's; and the reference
    /// matcher for each of Org's regexps.
    fn split_and_reference(
        cx: &InlineCx,
        values: &[String],
        limit: usize,
    ) -> Result<(Radio, Vec<reference::Radio>), String> {
        let refs: Vec<&str> = values.iter().map(String::as_str).collect();
        let new = Radio::build(cx, &refs);
        let groups = org_split(values, limit);
        let mine: Vec<Vec<u32>> = (0..new.parts)
            .map(|j| {
                (0..values.len() as u32)
                    .filter(|&t| new.part[t as usize] == j as u32)
                    .collect()
            })
            .collect();
        let mut i = 0u32;
        let theirs: Vec<Vec<u32>> = groups
            .iter()
            .map(|g| {
                let v = (i..i + g.len() as u32).collect();
                i += g.len() as u32;
                v
            })
            .collect();
        if mine != theirs {
            return Err(format!(
                "split of {values:?} at {limit}: {mine:?} vs Org's {theirs:?}"
            ));
        }
        let olds = groups
            .iter()
            .map(|g| {
                let r: Vec<&str> = g.iter().map(String::as_str).collect();
                reference::Radio::build(cx, &r)
            })
            .collect();
        Ok((new, olds))
    }

    /// Every search start in `region`: each regexp's leftmost match, then
    /// the one ending first; with a fresh cursor and with a shared one; and
    /// the link the search records is the one the link parser computes.
    fn split_searches(
        cx: &InlineCx,
        new: &Radio,
        olds: &[reference::Radio],
        (floor, end): (usize, usize),
        inside: &[usize],
    ) -> Result<(), String> {
        let mut shared = Cursor::default();
        for &s0 in inside {
            let mut want: Option<(usize, usize, (usize, usize, usize))> = None;
            for (j, old) in olds.iter().enumerate() {
                let first = inside
                    .iter()
                    .filter(|&&p| p >= s0 && p < end)
                    .find_map(|&p| old.valid_at(cx, p, s0, (floor, end)));
                let better = |m: &reference::Match| want.is_none_or(|w| (m.m0, j) < (w.0, w.1));
                if let Some(m) = first.filter(better) {
                    want = Some((m.m0, j, (m.p, m.g1, m.m0)));
                }
            }
            let want = want.map(|w| w.2);
            let found = new.search(cx, s0, (floor, end), &mut Cursor::default());
            if let Some(m) = found {
                let recorded = new.answer.get().filter(|a| a.at == (m.p, floor, end));
                let fresh = new.looking_at(cx, m.p, (floor, end)).ok().flatten();
                if recorded.map(|a| triple(a.link)) != Some(triple(fresh)) {
                    return Err(format!(
                        "from {s0}: recorded link {:?}, fresh {:?}",
                        recorded.map(|a| triple(a.link)),
                        triple(fresh)
                    ));
                }
            }
            let got = triple(found);
            let got_shared = triple(new.search(cx, s0, (floor, end), &mut shared));
            if got != want || got_shared != want {
                return Err(format!(
                    "search from {s0}: new {got:?} (shared cursor {got_shared:?}) reference {want:?}"
                ));
            }
        }
        Ok(())
    }

    /// The link parser at every position of `region`: the first regexp
    /// that matches there, `^` before the border char at a line start.
    fn split_links(
        cx: &InlineCx,
        new: &Radio,
        olds: &[reference::Radio],
        (floor, end): (usize, usize),
        inside: &[usize],
    ) -> Result<(), String> {
        for &i in inside.iter().filter(|&&i| i < end) {
            let bol = cx.at_bol(i, floor);
            let at = |old: &reference::Radio| {
                if bol {
                    old.valid_at(cx, i, i, (floor, end)).or_else(|| {
                        cx.ch(i, end)
                            .filter(|&c| boundary(c))
                            .and_then(|_| old.valid_at(cx, cx.step(i), i, (floor, end)))
                    })
                } else {
                    let s = i - cx.char_before(i, floor).map_or(1, char::len_utf8);
                    old.valid_at(cx, i, s, (floor, end))
                }
            };
            let want = olds.iter().find_map(at).map(|m| (m.p, m.g1, m.m0));
            let got = triple(new.looking_at(cx, i, (floor, end)).ok().flatten());
            if got != want {
                return Err(format!("link at {i}: new {got:?} reference {want:?}"));
            }
        }
        Ok(())
    }

    fn target() -> impl Strategy<Value = String> {
        prop::collection::vec(prop::sample::select(ALPHABET), 1..6)
            .prop_map(|cs| cs.into_iter().collect::<String>())
            // `org-target-regexp`'s border chars are not space or tab.
            .prop_filter("border", |s| {
                let ok = |c: Option<char>| c.is_some_and(|c| c != ' ' && c != '\t');
                ok(s.chars().next()) && ok(s.chars().last())
            })
    }

    fn text() -> impl Strategy<Value = String> {
        let mut chars = ALPHABET.to_vec();
        chars.push('\n');
        prop::collection::vec(prop::sample::select(chars), 0..40)
            .prop_map(|cs| cs.into_iter().collect())
    }

    /// Targets in regexp order, as `Radio::find` sorts them.
    fn ordered(mut v: Vec<String>) -> Vec<String> {
        let mut seen = HashSet::new();
        v.retain(|s| seen.insert(s.clone()));
        v.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
        v
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(3000))]

        /// The linear matcher agrees with the reference (the previous,
        /// hit-enumerating matcher) at every start, search start and region.
        #[test]
        fn agrees_with_the_reference_matcher(
            values in prop::collection::vec(target(), 1..6),
            text in text(),
            regions in prop::collection::vec((0usize..64, 0usize..64), 0..4),
        ) {
            let values = ordered(values);
            prop_assert_eq!(compare(&text, &values, &regions), Ok(()));
        }

        /// Texts made of the targets themselves, so long matches are common.
        #[test]
        fn agrees_with_the_reference_on_texts_of_targets(
            values in prop::collection::vec(target(), 1..6),
            picks in prop::collection::vec((0usize..6, prop::sample::select(ALPHABET)), 0..12),
            regions in prop::collection::vec((0usize..96, 0usize..96), 0..4),
        ) {
            let values = ordered(values);
            let text: String = picks
                .iter()
                .map(|&(i, sep)| format!("{}{sep}", values[i % values.len()]))
                .collect();
            prop_assert_eq!(compare(&text, &values, &regions), Ok(()));
        }
    }

    thread_local! {
        /// A test's override of the work budget.
        pub(super) static BUDGET: Cell<Option<u64>> = const { Cell::new(None) };
        /// Mixed-target nodes and targets visited (every parse adds).
        pub(super) static WALKED: Cell<u64> = const { Cell::new(0) };
        /// Every step of the radio search (every charge to the budget).
        pub(super) static WORK: Cell<u64> = const { Cell::new(0) };
        /// Every candidate visit of the searches (charged or not).
        pub(super) static VISITS: Cell<u64> = const { Cell::new(0) };
        /// Fault injection: the link parser forgets what the search found,
        /// so it can run out of work where the search did not.
        pub(super) static FORGET_ANSWER: Cell<bool> = const { Cell::new(false) };
    }

    /// Nodes and targets the mixed-target walk visits in one parse of `text`
    /// (with no budget limit) under a radio regexp limit, and whether the
    /// budget ran out.
    fn walked(text: &str, limit: usize) -> (u64, bool) {
        WALKED.with(|w| w.set(0));
        let (_, exhausted) = objects_with(text, u64::MAX, limit);
        (WALKED.with(Cell::get), exhausted.is_some())
    }

    /// Review F3 (a), (b): the work per candidate must not grow with the
    /// targets. Visits are counted, not timed: on the families below they
    /// grew as N^1.5 (√N targets on one node or one chain, visited at every
    /// candidate), so 8x the input made x22.6 the visits; constant work per
    /// candidate makes x8. Each family runs as one regexp, and split into
    /// several (limit 2048: the larger size's chains then span several
    /// regexps of many targets each), where every candidate inside a long
    /// match would otherwise walk the same chain again.
    #[test]
    fn mixed_target_work_per_candidate_is_bounded() {
        let case_variants = |n: usize| {
            let word = "abcdefghijklmnop";
            let targets: String = (0..(n as f64).sqrt() as usize)
                .map(|v| {
                    let w: String = word
                        .chars()
                        .enumerate()
                        .map(|(i, c)| {
                            if v >> i & 1 == 1 {
                                c.to_ascii_uppercase()
                            } else {
                                c
                            }
                        })
                        .collect();
                    format!("<<<\u{a0} {w}>>>\n")
                })
                .collect();
            // Every variant's run-collapsed form matches and its lead (no-break
            // space, space, `a`) too; the after-context (`x`) fails.
            format!("{targets}\n{}\n", "\u{a0} abcdefghijklmnopx ".repeat(n))
        };
        let nested = |n: usize, unit: &str| {
            let targets: String = (1..=(n as f64).sqrt() as usize)
                .map(|j| format!("<<<\u{a0}{}>>>\n", " a".repeat(j)))
                .collect();
            format!("{targets}\n{}\n", unit.repeat(n))
        };
        // `a`, then ` \ta` repeated: no lead, so only the text's lack of
        // whitespace other than spaces rules the chain out.
        let no_lead = |n: usize| {
            let targets: String = (1..=(n as f64).sqrt() as usize)
                .map(|j| format!("<<<a{}>>>\n", " \ta".repeat(j)))
                .collect();
            format!("{targets}\n{}\n", "a ".repeat(n))
        };
        type Gen = Box<dyn Fn(usize) -> String>;
        let families: [(&str, Gen); 4] = [
            ("case variants on one node", Box::new(case_variants)),
            (
                "nested chain, lead fails",
                Box::new(move |n| nested(n, "\t a")),
            ),
            (
                "nested chain, lead matches",
                Box::new(move |n| nested(n, "\u{a0} a")),
            ),
            (
                "nested chain without a lead, spaces only",
                Box::new(no_lead),
            ),
        ];
        for (name, gen) in families {
            for limit in [usize::MAX, 2048] {
                let (small, large) = (gen(1_000), gen(8_000));
                let ((w1, e1), (w8, e8)) = (walked(&small, limit), walked(&large, limit));
                assert!(!e1 && !e8, "{name}, limit {limit}: budget ran out");
                let bytes = large.len() as f64 / small.len() as f64;
                let ratio = w8 as f64 / (w1.max(1) as f64 * bytes);
                assert!(
                    ratio < 1.5,
                    "{name}, limit {limit}: visits {w1} -> {w8} for x{bytes:.1} bytes (x{ratio:.2})"
                );
                assert!(
                    w8 <= 4 * large.len() as u64,
                    "{name}, limit {limit}: {w8} visits for {} bytes",
                    large.len()
                );
            }
        }
    }

    /// The radio search's steps in one parse of `text` (no budget limit)
    /// under a radio regexp limit, and whether the budget ran out.
    fn work(text: &str, limit: usize) -> (u64, bool) {
        WORK.with(|w| w.set(0));
        let (_, exhausted) = objects_with(text, u64::MAX, limit);
        (WORK.with(Cell::get), exhausted.is_some())
    }

    /// Review B3: with split regexps, every lexer call in a region rescanned
    /// the candidates from the region's first match to the best match's
    /// end (and skipped ones were not charged), so N objects before a long
    /// match cost N x its length: 108 KB took 31 s with no diagnostic. The
    /// search must examine each candidate O(1) times per region, every step
    /// charged. Counted, not timed: the steps per input byte are bounded,
    /// and grow no faster than the input.
    type WorkGen = Box<dyn Fn(usize) -> String>;

    /// The families `radio_work_per_input_byte_is_bounded` counts: name,
    /// radio regexp limit, generator.
    fn work_families() -> Vec<(&'static str, usize, WorkGen)> {
        let fill = "q".repeat(8150);
        // Org's limit: the 8150-char target alone passes it, so these split.
        let org = crate::ParseOptions::ORG_TARGET_LINK_REGEXP_LIMIT;
        vec![
            // The reviewer's gen3: a lone no-break-space target makes every
            // char of a long run a candidate; `a b` matches across it.
            (
                "objects before a long run (split)",
                org,
                Box::new(move |n| {
                    format!(
                        "<<<{fill}>>>\n\n<<<a b>>> <<<\u{a0}>>>\n\n{}a{}b\n",
                        "*x* ".repeat(n),
                        " ".repeat(n)
                    )
                }),
            ),
            // gen2: a long simple target sharing its regexp with `a`.
            (
                "objects before a long target (split)",
                org,
                Box::new(|n| {
                    let l = vec!["a"; n].join(" ");
                    format!(
                        "<<<{}>>>\n\n<<<{l}>>> <<<a>>>\n\n{}{l}\n",
                        "q".repeat(8150),
                        "*x* ".repeat(n)
                    )
                }),
            ),
            (
                "radio links",
                usize::MAX,
                Box::new(|n| format!("<<<ab>>>\n{}\n", "ab ".repeat(n))),
            ),
            (
                "long radio target",
                usize::MAX,
                Box::new(|n| format!("<<<{}b>>>\n{}\n", "a ".repeat(n), "a ".repeat(n))),
            ),
            (
                "radio candidates rejected, split",
                2048,
                Box::new(|n| {
                    format!(
                        "<<<{}>>>\n<<<ab>>>\n{}{} ab\n",
                        "q".repeat(2100),
                        "*a* ".repeat(n),
                        "xab".repeat(n)
                    )
                }),
            ),
            (
                "nested targets, split",
                2048,
                Box::new(|n| {
                    let k = (n as f64).sqrt() as usize;
                    let targets: String = (1..=k)
                        .map(|j| format!("<<<{}>>>\n", vec!["a"; j].join(" ")))
                        .collect();
                    format!(
                        "<<<{}>>>\n{targets}\n{}\n",
                        "q".repeat(2100),
                        "a ".repeat(n)
                    )
                }),
            ),
        ]
    }

    #[test]
    fn radio_work_per_input_byte_is_bounded() {
        let families = work_families();
        let mut failures = Vec::new();
        for (name, limit, gen) in families {
            let (small, large) = (gen(500), gen(4_000));
            let ((w1, e1), (w8, e8)) = (work(&small, limit), work(&large, limit));
            let per_byte = w8 as f64 / large.len() as f64;
            // Against the 8x of the repeated part (the fixed targets would
            // flatter a per-byte ratio): linear is about 1, quadratic 8.
            let growth = w8 as f64 / (w1.max(1) as f64 * 8.0);
            let line = format!(
                "{name}: {w1} -> {w8} steps for {} -> {} bytes, {per_byte:.2}/byte, growth x{growth:.2}",
                small.len(),
                large.len()
            );
            eprintln!("{line}");
            if e1 || e8 || per_byte >= 8.0 || growth >= 1.5 {
                failures.push(line);
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    /// "Every step charged" (review B3: the split search's skipped
    /// candidates were not, so its work escaped the budget): under any
    /// budget, the searches visit at most that many candidates, so the
    /// budget bounds the time. Visits are counted where they happen,
    /// whether charged or not.
    #[test]
    fn every_candidate_visit_is_charged() {
        let org = crate::ParseOptions::ORG_TARGET_LINK_REGEXP_LIMIT;
        let fill = "q".repeat(8150);
        let n = 3_000;
        let docs = [
            format!(
                "<<<{fill}>>>\n\n<<<a b>>> <<<\u{a0}>>>\n\n{}a{}b\n",
                "*x* ".repeat(n),
                " ".repeat(n)
            ),
            format!(
                "<<<{fill}>>>\n\n<<<{l}>>> <<<a>>>\n\n{}{l}\n",
                "*x* ".repeat(n),
                l = vec!["a"; 200].join(" ")
            ),
            format!("<<<ab>>>\n{}{} ab\n", "*a* ".repeat(n), "xab".repeat(n)),
        ];
        for (i, doc) in docs.iter().enumerate() {
            for budget in [50u64, 500, 5_000, 50_000] {
                VISITS.with(|v| v.set(0));
                objects_with(doc, budget, org);
                let visits = VISITS.with(Cell::get);
                assert!(
                    visits <= budget + 1,
                    "doc {i}, budget {budget}: {visits} visits"
                );
            }
        }
    }

    /// `(kind, begin, end)` of every object in `text` under a work budget,
    /// and where the budget ran out, if it did.
    fn objects_with_budget(
        text: &str,
        budget: u64,
    ) -> (Vec<(String, usize, usize)>, Option<usize>) {
        objects_with(
            text,
            budget,
            crate::ParseOptions::ORG_TARGET_LINK_REGEXP_LIMIT,
        )
    }

    /// The same, with a radio regexp limit.
    fn objects_with(
        text: &str,
        budget: u64,
        limit: usize,
    ) -> (Vec<(String, usize, usize)>, Option<usize>) {
        BUDGET.with(|b| b.set(Some(budget)));
        let options = crate::ParseOptions {
            radio_regexp_limit: limit,
            ..crate::ParseOptions::default()
        };
        let doc = crate::parse(text, &options);
        BUDGET.with(|b| b.set(None));
        let exhausted = doc
            .diagnostics()
            .iter()
            .find(|d| d.kind == crate::DiagnosticKind::RadioLinkBudget)
            .map(|d| d.span.start as usize);
        let objects = doc
            .syntax()
            .descendants()
            .filter(|n| {
                matches!(
                    n.kind(),
                    LINK | BOLD | RADIO_TARGET | ITALIC | FOOTNOTE_REFERENCE
                )
            })
            .map(|n| {
                let r = n.text_range();
                (format!("{:?}", n.kind()), r.start().into(), r.end().into())
            })
            .collect();
        (objects, exhausted)
    }

    /// Review F1's cause: the link parser re-ran the search's exact match,
    /// and the budget could run out in between. What the search found is
    /// parsed without work, even with none left.
    #[test]
    fn the_link_the_search_found_is_parsed_with_no_work_left() {
        let text = "<<<a\t b>>>\n\nx a\t  b y\n";
        let src = Source::new(text);
        let ix = InlineIndex::build(&src);
        let types: Vec<String> = Vec::new();
        let diagnostics = std::cell::RefCell::new(Vec::new());
        let mut cx = InlineCx {
            src: &src,
            ix: &ix,
            link_types: &types,
            radio: None,
            radio_limit: crate::ParseOptions::ORG_TARGET_LINK_REGEXP_LIMIT,
            diagnostics: &diagnostics,
        };
        let radio = Radio::build(&cx, &["a\t b"]);
        cx.radio = Some(&radio);
        let (floor, end) = (12, text.len());
        let p = radio.next_link(&cx, floor, end, floor, &mut Cursor::default());
        assert_eq!(p, Some(14));
        radio.budget.set(0);
        let o = link(&cx, 14, end, floor).expect("the search's link");
        assert_eq!((o.begin, o.end), (14, 20));
        assert_eq!(radio.exhausted_at(), None);
        // Asked anywhere else (here: another region), the work is needed,
        // and there is none.
        assert!(link(&cx, 14, end, 13).is_none());
        assert_eq!(radio.exhausted_at(), Some(14));
    }

    /// Defence in depth: were the link parser ever to fail where the search
    /// matched, the lexer goes on past the link instead of ending the
    /// region. Forced here by making the parser forget the search.
    #[test]
    fn a_failed_radio_parse_never_ends_a_frame() {
        let text = "<<<\u{a0} abc>>> <<<Abc>>> <<<a\t b>>>\n\n\
                    \u{a0} abc  abc x\u{a0} abc\n\n\
                    x a\t b *bold* [[id:keep]] abc /i/ tail[fn:1]\n";
        let (full, _) = objects_with_budget(text, u64::MAX);
        let is_radio =
            |o: &(String, usize, usize)| o.0 == "LINK" && !text[o.1..o.2].starts_with("[[");
        let others: Vec<_> = full.iter().filter(|o| !is_radio(o)).cloned().collect();
        FORGET_ANSWER.with(|f| f.set(true));
        let lost: Vec<u64> = (0..400)
            .filter(|&b| {
                let (got, _) = objects_with_budget(text, b);
                got.into_iter().filter(|o| !is_radio(o)).collect::<Vec<_>>() != others
            })
            .collect();
        FORGET_ANSWER.with(|f| f.set(false));
        assert!(lost.is_empty(), "objects lost at budgets {lost:?}");
    }

    /// Review F1: running out of work must never end a frame. At every
    /// point where the budget can run out, the objects that are not radio
    /// links are all still found, and radio links are either Org's or
    /// absent from the point of exhaustion on — never different ones.
    #[test]
    fn running_out_of_work_never_ends_a_frame_or_changes_a_link() {
        let text = "<<<\u{a0} abc>>> <<<Abc>>> <<<a\t b>>>\n\n\
                    \u{a0} abc  abc x\u{a0} abc\n\n\
                    x a\t b *bold* [[id:keep]] abc /i/ tail[fn:1]\n";
        let (full, exhausted) = objects_with_budget(text, u64::MAX);
        assert_eq!(exhausted, None);
        let is_radio_link =
            |o: &(String, usize, usize)| o.0 == "LINK" && !text[o.1..o.2].starts_with("[[");
        let others: Vec<_> = full.iter().filter(|o| !is_radio_link(o)).cloned().collect();
        let radio: Vec<_> = full.iter().filter(|o| is_radio_link(o)).cloned().collect();
        assert!(radio.len() >= 3, "{full:?}");
        let (mut lost, mut changed, mut misplaced) = (Vec::new(), Vec::new(), Vec::new());
        for budget in 0..400 {
            let (got, from) = objects_with_budget(text, budget);
            let got_others: Vec<_> = got.iter().filter(|o| !is_radio_link(o)).cloned().collect();
            if got_others != others {
                lost.push(budget);
            }
            // The radio links made are a prefix of Org's.
            let got_radio: Vec<_> = got.iter().filter(|o| is_radio_link(o)).cloned().collect();
            if got_radio.len() > radio.len() || got_radio != radio[..got_radio.len()] {
                changed.push(budget);
            }
            // The diagnostic says where links stop: exactly Org's links that
            // start before it are made.
            let before: Vec<_> = radio
                .iter()
                .filter(|o| from.is_none_or(|f| o.1 < f))
                .cloned()
                .collect();
            if got_radio != before {
                misplaced.push(budget);
            }
        }
        assert!(
            misplaced.is_empty(),
            "diagnostic span wrong at budgets {misplaced:?}"
        );
        assert!(lost.is_empty(), "objects lost at budgets {lost:?}");
        assert!(
            changed.is_empty(),
            "radio links changed at budgets {changed:?}"
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1500))]

        /// Split regexps (a small limit, so a few short targets split)
        /// agree with Org's composition of one regexp per part.
        #[test]
        fn split_regexps_agree_with_the_reference_composition(
            values in prop::collection::vec(target(), 1..7),
            picks in prop::collection::vec((0usize..7, prop::sample::select(ALPHABET)), 0..10),
            noise in text(),
            regions in prop::collection::vec((0usize..96, 0usize..96), 0..3),
            limit in 56usize..72,
        ) {
            let values = ordered(values);
            let mut text: String = picks
                .iter()
                .map(|&(i, sep)| format!("{}{sep}", values[i % values.len()]))
                .collect();
            text.extend(noise.chars().take(12));
            prop_assert_eq!(compare_split(&text, &values, &regions, limit), Ok(()));
        }
    }

    /// Hand-picked splits: a long target in the first regexp and a short
    /// one ending earlier in the second (the case Org's split changes), a
    /// tie on the end, and a mixed target in a later regexp.
    #[test]
    fn split_regexps_agree_on_chosen_cases() {
        let cases: &[(&str, &[&str], usize)] = &[
            ("x foo bar zz y", &["foo bar zz", "bar"], 60),
            ("x foo bar zz y", &["foo bar zz", "bar", "zz"], 60),
            ("a b c", &["a b c", "b c", "c"], 57),
            ("ab ab", &["ab", "AB"], 56),
            ("x \u{a0} a\t b", &["\u{a0} a", "a\t b", "a"], 58),
            ("a\nb-c", &["a b", "b-c"], 57),
        ];
        for (text, values, limit) in cases {
            let values: Vec<String> = values.iter().map(|s| s.to_string()).collect();
            assert_eq!(
                compare_split(text, &values, &[(1, 5), (0, 3)], *limit),
                Ok(())
            );
        }
    }

    /// Fixed cases worth keeping whatever proptest draws.
    #[test]
    fn agrees_with_the_reference_on_chosen_cases() {
        let cases: &[(&str, &[&str])] = &[
            ("a b c", &["a b c", "a b", "a"]),
            ("a  b x", &["a  b", "a b"]),
            ("a\u{a0} b", &["a\u{a0} b", "a\u{a0}"]),
            ("x \u{a0}\u{a0} xa \u{a0} x", &["\u{a0} x"]),
            ("中ab文 xab ab", &["ab"]),
            ("a\tb a\tb\ta", &["a\tb\ta", "a\tb"]),
            ("a\n\tb", &["a\tb", "a b"]),
            ("A B", &["a b"]),
            ("a-b", &["a-b", "b", "a"]),
        ];
        for (text, values) in cases {
            let values: Vec<String> = values.iter().map(|s| s.to_string()).collect();
            assert_eq!(compare(text, &values, &[(1, 4), (0, 3)]), Ok(()));
        }
    }

    /// The edge rule for inner targets reads the folded char: folding must
    /// never change whether a char borders a link.
    #[test]
    fn folding_keeps_the_border_class() {
        for u in 0..=0x10FFFFu32 {
            if let Some(c) = char::from_u32(u) {
                assert_eq!(boundary(c), boundary(canon(c)), "U+{u:04X}");
            }
        }
    }
}
