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
//! checked by a backtracking match of the exact regexp, which shares its
//! failed states across calls (a run's later starts are a subset of its
//! earlier ones) and runs within a work budget proportional to the text;
//! past it they stop matching and the parse reports
//! [`crate::DiagnosticKind::RadioLinkBudget`].

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

pub(crate) struct Radio {
    targets: Vec<Target>,
    /// Byte start of each symbol of the text, then the text's end.
    sym: Vec<usize>,
    /// Whether each symbol is a whitespace run.
    ws: Vec<bool>,
    /// Simple targets, and mixed ones (by their run-collapsed symbols).
    plain: Matcher,
    /// Literal targets, runs read as their exact content.
    literal: Matcher,
    /// Byte positions where some target may start, sorted.
    starts: Vec<usize>,
    /// Work left for mixed targets' exact matches.
    budget: Cell<u64>,
    exhausted: Cell<bool>,
    memo: RefCell<Memo>,
}

/// Exact-match states of mixed targets known to fail, per region end.
#[derive(Default)]
struct Memo {
    failed: HashSet<(u32, usize, u32, usize)>,
    /// A `\s-+` item at the start of a text run: the least start at which it
    /// failed. Starting later in the same run offers a subset of the ends.
    run_failed: HashMap<(u32, usize, u32, usize), usize>,
}

/// Where a frame's search for radio links has got to: candidates before
/// `next` were rejected for good.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Cursor {
    next: usize,
}

/// A match of the regexp: group 1 `[p, g1)`, the whole match ending at `m0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Match {
    pub p: usize,
    pub g1: usize,
    pub m0: usize,
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
        let targets: Vec<Target> = values.iter().map(|v| compile(v)).collect();
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
        let plain = Matcher::new(&plain_seqs, &text.plain);
        let literal = Matcher::new(&literal_seqs, &text.exact);
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
        Radio {
            targets,
            sym: text.sym,
            ws: text.ws,
            plain,
            literal,
            starts,
            budget: Cell::new(8 * cx.src.len() as u64 + 65_536),
            exhausted: Cell::new(false),
            memo: RefCell::new(Memo::default()),
        }
    }

    /// Did a mixed target's exact match run out of work (some radio links
    /// may be missing)?
    pub fn budget_exhausted(&self) -> bool {
        self.exhausted.get()
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

    /// The first match whose regexp starts at or after `s0`.
    fn search(
        &self,
        cx: &InlineCx,
        s0: usize,
        (floor, end): (usize, usize),
        cur: &mut Cursor,
    ) -> Option<Match> {
        // Candidates this frame skipped are rejected for good: a later
        // search starts at or after `s0`, and whether a candidate `p >= s0`
        // matches does not depend on `s0` (at `p == s0` only `^` can, which
        // does not look at it). So the cursor only moves forward.
        let first = self.starts.partition_point(|&p| p < s0);
        let mut k = cur.next.max(first);
        while let Some(&p) = self.starts.get(k).filter(|&&p| p < end) {
            if let Some(m) = self.valid_at(cx, p, s0, (floor, end)) {
                cur.next = k;
                return Some(m);
            }
            k += 1;
        }
        cur.next = k;
        None
    }

    /// The regexp with group 1 at `p`, reached from a search start `s0`.
    pub(crate) fn valid_at(
        &self,
        cx: &InlineCx,
        p: usize,
        s0: usize,
        region: (usize, usize),
    ) -> Option<Match> {
        let (floor, end) = region;
        if p >= end || p < floor {
            return None;
        }
        let bol_branch = p >= s0 && cx.at_bol(p, floor);
        let char_branch = p > floor
            && p - cx.char_before(p, floor).map_or(1, char::len_utf8) >= s0
            && cx.ch_before(p, floor).is_some_and(boundary);
        if !(bol_branch || char_branch) {
            return None;
        }
        let k = self.sym.partition_point(|&s| s <= p).checked_sub(1)?;
        if k + 1 >= self.sym.len() {
            return None;
        }
        // Symbols from `k` that end by `end`, and that start before it.
        let fit = (self.sym.partition_point(|&s| s <= end) - 1).saturating_sub(k);
        let reach = self.sym.partition_point(|&s| s < end) - k;
        let mut best: Option<(u32, Match)> = None;
        if !self.ws[k] {
            if self.sym[k] != p {
                return None;
            }
            for m in [&self.plain, &self.literal] {
                if let Some((t, g1)) = m.first_at(cx, &self.sym, k, fit, end) {
                    if best.is_none_or(|b| t < b.0) {
                        let m0 = after(cx, g1, end)?;
                        best = Some((t, Match { p, g1, m0 }));
                    }
                }
            }
        }
        // Mixed targets on the run-collapsed path, deepest first, in regexp
        // order; only those ahead of the best so far are tried.
        let deep = self.plain.deep[k];
        let tr = &self.plain.trie;
        let mut u = tr.mixed_up[tr.level_ancestor(deep, reach) as usize];
        while u != NONE {
            let (a, b) = tr.mixed[u as usize];
            for &t in &tr.mixed_list[a as usize..b as usize] {
                if best.is_some_and(|b| b.0 <= t) {
                    break;
                }
                if let Some((g1, m0)) = self.exact(cx, t, p, end) {
                    best = Some((t, Match { p, g1, m0 }));
                    break;
                }
            }
            u = match tr.parent[u as usize] {
                pu if pu == u => NONE,
                pu => tr.mixed_up[pu as usize],
            };
        }
        best.map(|(_, m)| m)
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

    /// The exact regexp for mixed target `t` at `p` (group 1 and the
    /// after-context), with Emacs's backtracking order: a run tries its
    /// longest extent first. A state that fails is remembered, so no state
    /// is explored twice across calls.
    fn exact(&self, cx: &InlineCx, t: u32, p: usize, end: usize) -> Option<(usize, usize)> {
        if self.exhausted.get() {
            return None;
        }
        let mut run = Exact {
            radio: self,
            cx,
            t,
            end,
            items: &self.targets[t as usize].items,
            memo: &mut self.memo.borrow_mut(),
            stack: Vec::new(),
        };
        let mut state = (0usize, p);
        loop {
            match run.enter(state) {
                Step::Matched(m) => return Some(m),
                Step::Next(s) => state = s,
                Step::Failed => state = run.backtrack()?,
                Step::OutOfWork => return None,
            }
        }
    }
}

/// One exact match of a mixed target in progress.
struct Exact<'r, 'a> {
    radio: &'r Radio,
    cx: &'r InlineCx<'a>,
    t: u32,
    end: usize,
    items: &'r [Item],
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
    /// Tries item `k` at `pos`.
    fn enter(&mut self, (k, pos): (usize, usize)) -> Step {
        if !self.radio.spend() {
            return Step::OutOfWork;
        }
        let (cx, end) = (self.cx, self.end);
        let key = (self.t, end, k as u32, pos);
        if self.memo.failed.contains(&key) {
            return Step::Failed;
        }
        match self.items.get(k) {
            None => match after(cx, pos, end) {
                Some(m0) => return Step::Matched((pos, m0)),
                None => self.fail(k, pos),
            },
            Some(&Item::Lit(c)) => match cx.ch(pos, end) {
                Some(d) if canon(d) == c => {
                    self.stack.push((k, pos, 0, 0));
                    return Step::Next((k + 1, cx.step(pos)));
                }
                _ => self.fail(k, pos),
            },
            Some(Item::Run) => {
                // The run's extent from the symbol table: scanning it here
                // would cost its length per call.
                let q = self.radio.run_end(pos, end);
                let known = self
                    .memo
                    .run_failed
                    .get(&(self.t, end, k as u32, q))
                    .is_some_and(|&f| f <= pos);
                if q > pos && !known {
                    self.stack.push((k, pos, q, q));
                    return Step::Next((k + 1, q));
                }
            }
        }
        Step::Failed
    }

    fn fail(&mut self, k: usize, pos: usize) {
        self.memo.failed.insert((self.t, self.end, k as u32, pos));
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
                let f = self
                    .memo
                    .run_failed
                    .entry((self.t, self.end, k as u32, extent))
                    .or_insert(pos);
                *f = (*f).min(pos);
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
    Target { items, shape }
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
                s.plain.push(W);
                s.exact.push(RUN_BASE + id);
                i = j;
            } else {
                let c = canon(c) as u32;
                s.ws.push(false);
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
    fn new(seqs: &[(u32, Vec<u32>, bool)], text: &[u32]) -> Matcher {
        let trie = Trie::new(seqs);
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

struct Trie {
    next: HashMap<(u32, u32), u32>,
    parent: Vec<u32>,
    depth: Vec<u32>,
    /// Jump pointers (Myers): level ancestors in O(log depth).
    jump: Vec<u32>,
    /// The first matched target (regexp order) ending here.
    own: Vec<u32>,
    /// The first matched target ending at a proper ancestor whose next
    /// symbol toward here borders a link, and that ancestor's depth.
    inner: Vec<(u32, u32)>,
    /// Mixed targets ending here, in regexp order: a range of `mixed_list`.
    mixed: Vec<(u32, u32)>,
    mixed_list: Vec<u32>,
    /// The nearest ancestor-or-self holding mixed targets.
    mixed_up: Vec<u32>,
    /// Some target ends here or at an ancestor (the root excluded).
    any: Vec<bool>,
}

impl Trie {
    fn new(seqs: &[(u32, Vec<u32>, bool)]) -> Trie {
        let mut next: HashMap<(u32, u32), u32> = HashMap::new();
        let (mut parent, mut depth, mut edge) = (vec![0u32], vec![0u32], vec![W]);
        let mut own = vec![NONE];
        let mut mixed_of: Vec<Vec<u32>> = vec![Vec::new()];
        for (t, syms, matched) in seqs {
            let mut node = 0u32;
            for &s in syms {
                node = *next.entry((node, s)).or_insert_with(|| {
                    parent.push(node);
                    depth.push(depth[node as usize] + 1);
                    edge.push(s);
                    own.push(NONE);
                    mixed_of.push(Vec::new());
                    (parent.len() - 1) as u32
                });
            }
            let n = node as usize;
            if node == 0 {
                continue; // an empty target matches nothing (none exists)
            }
            if *matched {
                own[n] = own[n].min(*t);
            } else {
                mixed_of[n].push(*t);
            }
        }
        let count = parent.len();
        let mut tr = Trie {
            next,
            jump: vec![0; count],
            inner: vec![(NONE, 0); count],
            mixed: Vec::with_capacity(count),
            mixed_list: Vec::new(),
            mixed_up: vec![NONE; count],
            any: vec![false; count],
            parent,
            depth,
            own,
        };
        // Nodes are numbered after their parent: one pass in index order.
        for v in 0..count {
            let a = tr.mixed_list.len() as u32;
            let mut list = std::mem::take(&mut mixed_of[v]);
            list.sort_unstable();
            tr.mixed_list.extend(list);
            tr.mixed.push((a, tr.mixed_list.len() as u32));
            let held = tr.mixed_list.len() as u32 > a;
            if v == 0 {
                continue;
            }
            let p = tr.parent[v] as usize;
            let j1 = tr.jump[p] as usize;
            let j2 = tr.jump[j1] as usize;
            tr.jump[v] = if p != 0 && tr.depth[p] - tr.depth[j1] == tr.depth[j1] - tr.depth[j2] {
                j2 as u32
            } else {
                p as u32
            };
            let mut inner = tr.inner[p];
            if tr.own[p] != NONE && boundary_symbol(edge[v]) && tr.own[p] < inner.0 {
                inner = (tr.own[p], tr.depth[p]);
            }
            tr.inner[v] = inner;
            tr.mixed_up[v] = if held { v as u32 } else { tr.mixed_up[p] };
            tr.any[v] = tr.any[p] || held || tr.own[v] != NONE;
        }
        tr
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
}

/// A suffix automaton of the reversed target sequences (generalized:
/// several strings, built online with the existing-transition check).
struct Sam {
    len: Vec<u32>,
    link: Vec<u32>,
    next: Vec<Vec<(u32, u32)>>,
}

impl Sam {
    fn get(&self, v: u32, c: u32) -> Option<u32> {
        let e = &self.next[v as usize];
        e.binary_search_by_key(&c, |&(s, _)| s).ok().map(|i| e[i].1)
    }

    fn set(&mut self, v: u32, c: u32, to: u32) {
        let e = &mut self.next[v as usize];
        match e.binary_search_by_key(&c, |&(s, _)| s) {
            Ok(i) => e[i].1 = to,
            Err(i) => e.insert(i, (c, to)),
        }
    }

    fn state(&mut self, len: u32, link: u32, next: Vec<(u32, u32)>) -> u32 {
        self.len.push(len);
        self.link.push(link);
        self.next.push(next);
        (self.len.len() - 1) as u32
    }

    /// Redirects the transitions on `c` to `q` from `p` and its suffix links
    /// to a clone of `q` of length `len`.
    fn split(&mut self, mut p: u32, c: u32, q: u32, len: u32) -> u32 {
        let clone = self.state(len, self.link[q as usize], self.next[q as usize].clone());
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
        let cur = self.state(l, 0, Vec::new());
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
            next: vec![Vec::new()],
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
/// regexp matches from `i` (at a line start) or from the char before it.
pub(crate) fn link(cx: &InlineCx, i: usize, end: usize, floor: usize) -> Option<Obj> {
    let radio = cx.radio?;
    let m = if cx.at_bol(i, floor) {
        // `^` at `i`, else the char at `i` as the border.
        radio.valid_at(cx, i, i, (floor, end)).or_else(|| {
            cx.ch(i, end)
                .filter(|&c| boundary(c))
                .and_then(|_| radio.valid_at(cx, cx.step(i), i, (floor, end)))
        })
    } else {
        // The char before `i` as the border (`^` there would put the link's
        // text before `i`, which Org never reaches; see `limit`).
        let s = i - cx.char_before(i, floor).map_or(1, char::len_utf8);
        radio.valid_at(cx, i, s, (floor, end)).filter(|m| m.p == i)
    }?;
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
