//! TEST ONLY: the radio matcher before the linear rewrite, kept verbatim as
//! the reference the rewrite is checked against (`radio::tests`). It
//! records every (start, target) hit, so it is O(text^1.5) in memory on
//! nested targets and quadratic on a blank-start target: never use it in
//! the parser.
#![allow(dead_code)]

//! Radio links: text matching one of the document's radio targets
//! (`<<<target>>>`), as `org-target-link-regexp` matches it.
//!
//! Org builds one regexp from every radio target in the buffer (found when
//! `org-mode` starts, with no radio links active), longest target first:
//! `\(?:^\|[^[:alnum:]]\|\c|\)\(T1\|T2\|…\)\(?:$\|[^[:alnum:]]\|\c|\)`,
//! where each target is quoted and each run of spaces in it matches any
//! non-empty run of whitespace (`\s-+`), and the match ignores case. The
//! object lexer bounds its search just past the next match, so a radio link
//! is parsed when no other object starts before it.
//!
//! Searching that regexp from every lexer position, as Emacs does, costs
//! O(text × targets × length). Here the matches are found once per document:
//! an Aho–Corasick automaton runs over the text with case folded and each
//! whitespace run read as one symbol, which for a target whose only
//! whitespace is spaces IS the regexp (its runs sit between non-blank chars,
//! so `\s-+` must take a whole run). A target with other whitespace (a tab, a
//! no-break space) is checked at each automaton hit by a backtracking match
//! of the exact regexp, memoised so it stays polynomial. The context checks
//! (start/end of line, the region's bounds, the alnum class) depend on the
//! region and are made when the lexer asks.

use super::InlineCx;
use crate::chars::{canon, is_alnum, is_linebreak, is_space};
use crate::kind::SyntaxKind::*;
use crate::seeded::{HashMap, HashSet};
use crate::span::ContentsTable;
use std::collections::VecDeque;

/// The symbol a whitespace run reads as.
const W: u32 = u32::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    /// A char, case-folded.
    Lit(char),
    /// `\s-+`, from a run of spaces in the target.
    Run,
}

struct Target {
    items: Vec<Item>,
    /// Only spaces as whitespace: an automaton hit is a match.
    simple: bool,
    /// Starts with a whitespace char, so it may start inside a text run.
    blank_start: bool,
}

/// A position where some target may start: the targets, in regexp order, and
/// for a simple target where its match ends (`usize::MAX`: compute it).
type Alt = (u32, usize);

pub(crate) struct Radio {
    targets: Vec<Target>,
    starts: Vec<usize>,
    ranges: Vec<(u32, u32)>,
    alts: Vec<Alt>,
}

/// Where a frame's search for radio links has got to: candidates before
/// `next` were rejected for good.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct Cursor {
    next: usize,
}

/// A match of the regexp: group 1 `[p, g1)`, the whole match ending at `m0`.
#[derive(Debug, Clone, Copy)]
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
        let mut values: Vec<&str> = Vec::new();
        for s in table.spans_of(RADIO_TARGET) {
            let v = &cx.src.text[s.range()];
            if !values.contains(&v) {
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
        let mut hits: Vec<(usize, u32, usize)> = Vec::new();
        let ac = Automaton::new(&targets);
        ac.scan(cx, &targets, &mut hits);
        hits.sort_unstable_by_key(|&(p, t, _)| (p, t));
        hits.dedup_by_key(|h| (h.0, h.1));
        let mut radio = Radio {
            targets,
            starts: Vec::new(),
            ranges: Vec::new(),
            alts: Vec::with_capacity(hits.len()),
        };
        for (p, t, g1) in hits {
            if radio.starts.last() != Some(&p) {
                radio.starts.push(p);
                let at = radio.alts.len() as u32;
                radio.ranges.push((at, at));
            }
            radio.alts.push((t, g1));
            if let Some(r) = radio.ranges.last_mut() {
                r.1 += 1;
            }
        }
        radio
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
        // longer one would make Org return an object starting before point,
        // which no tree can hold, so it is skipped.
        let lbp = cx.src.lines[cx.src.line_of(from)].start.max(floor);
        let after_lbp = lbp + cx.char_len(lbp);
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
        let bol_branch = p >= s0 && cx.at_bol(p, floor);
        let char_branch = p > floor
            && p - cx.char_before(p, floor).map_or(1, char::len_utf8) >= s0
            && cx.ch_before(p, floor).is_some_and(boundary);
        if !(bol_branch || char_branch) {
            return None;
        }
        let k = self.starts.binary_search(&p).ok()?;
        let (a, b) = self.ranges[k];
        self.alts[a as usize..b as usize]
            .iter()
            .find_map(|&(t, g1)| {
                let t = &self.targets[t as usize];
                if t.simple {
                    (g1 <= end).then_some(())?;
                    after(cx, g1, end).map(|m0| Match { p, g1, m0 })
                } else {
                    exact(cx, &t.items, p, end).map(|(g1, m0)| Match { p, g1, m0 })
                }
            })
    }
}

/// `[^[:alnum:]]` or `\c|`: a char that may border a radio link.
fn boundary(c: char) -> bool {
    !is_alnum(c) || is_linebreak(c)
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
    Target {
        simple: !items.iter().any(ws_lit),
        blank_start: items.first().is_some_and(ws_lit),
        items,
    }
}

/// The target as automaton symbols: each maximal whitespace stretch is `W`.
fn symbols(t: &Target) -> Vec<u32> {
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

/// The exact regexp for one target at `p` (group 1 and the after-context),
/// with Emacs's backtracking order: a run tries its longest extent first.
/// Each (item, position) state is entered once: the search returns at the
/// first success, so a state seen again has already failed.
fn exact(cx: &InlineCx, items: &[Item], p: usize, end: usize) -> Option<(usize, usize)> {
    let mut seen: HashSet<(usize, usize)> = HashSet::default();
    let mut choices: Vec<(usize, Vec<usize>)> = Vec::new();
    let (mut k, mut pos) = (0, p);
    loop {
        // Advance until this path fails (a success returns).
        while seen.insert((k, pos)) {
            match items.get(k) {
                None => match after(cx, pos, end) {
                    Some(m0) => return Some((pos, m0)),
                    None => break,
                },
                Some(&Item::Lit(c)) => match cx.ch(pos, end) {
                    Some(d) if canon(d) == c => {
                        pos = cx.step(pos);
                        k += 1;
                    }
                    _ => break,
                },
                Some(Item::Run) => {
                    let mut ends = Vec::new();
                    let mut q = pos;
                    while cx.ch(q, end).is_some_and(is_space) {
                        q = cx.step(q);
                        ends.push(q);
                    }
                    let Some(longest) = ends.pop() else {
                        break;
                    };
                    choices.push((k + 1, ends));
                    k += 1;
                    pos = longest;
                }
            }
        }
        // Backtrack: the most recent run gives up one more char.
        loop {
            let (next_k, ends) = choices.last_mut()?;
            if let Some(e) = ends.pop() {
                k = *next_k;
                pos = e;
                break;
            }
            choices.pop();
        }
    }
}

/// Aho–Corasick over symbols.
struct Automaton {
    goto_: HashMap<(u32, u32), u32>,
    fail: Vec<u32>,
    /// Targets whose symbols end at this node.
    out: Vec<Vec<u32>>,
    /// The nearest node on the fail chain with outputs.
    dict: Vec<Option<u32>>,
    lens: Vec<usize>,
    max_len: usize,
}

impl Automaton {
    fn new(targets: &[Target]) -> Automaton {
        let mut a = Automaton {
            goto_: HashMap::default(),
            fail: vec![0],
            out: vec![Vec::new()],
            dict: vec![None],
            lens: Vec::with_capacity(targets.len()),
            max_len: 0,
        };
        for (t, target) in targets.iter().enumerate() {
            let syms = symbols(target);
            let mut node = 0u32;
            for &s in &syms {
                node = match a.goto_.get(&(node, s)) {
                    Some(&n) => n,
                    None => {
                        let n = a.fail.len() as u32;
                        a.fail.push(0);
                        a.out.push(Vec::new());
                        a.dict.push(None);
                        a.goto_.insert((node, s), n);
                        n
                    }
                };
            }
            a.out[node as usize].push(t as u32);
            a.max_len = a.max_len.max(syms.len());
            a.lens.push(syms.len());
        }
        a.link();
        a
    }

    /// Fail and dictionary links, breadth first.
    fn link(&mut self) {
        let mut children: Vec<Vec<(u32, u32)>> = vec![Vec::new(); self.fail.len()];
        for (&(from, s), &to) in &self.goto_ {
            children[from as usize].push((s, to));
        }
        let mut queue: VecDeque<u32> = children[0].iter().map(|&(_, n)| n).collect();
        while let Some(n) = queue.pop_front() {
            for &(s, child) in &children[n as usize] {
                let mut f = self.fail[n as usize];
                let target = loop {
                    if let Some(&g) = self.goto_.get(&(f, s)) {
                        break g;
                    }
                    if f == 0 {
                        break 0;
                    }
                    f = self.fail[f as usize];
                };
                self.fail[child as usize] = target;
                let ft = target as usize;
                self.dict[child as usize] = if self.out[ft].is_empty() {
                    self.dict[ft]
                } else {
                    Some(target)
                };
                queue.push_back(child);
            }
        }
    }

    fn step(&self, mut node: u32, s: u32) -> u32 {
        loop {
            if let Some(&n) = self.goto_.get(&(node, s)) {
                return n;
            }
            if node == 0 {
                return 0;
            }
            node = self.fail[node as usize];
        }
    }

    /// Every (start, target, end) hit in the text. Symbols are chars (case
    /// folded) and whitespace runs; a ring keeps the last `max_len` symbols'
    /// extents so a hit's start is known.
    fn scan(&self, cx: &InlineCx, targets: &[Target], hits: &mut Vec<(usize, u32, usize)>) {
        let n = self.max_len.max(1);
        let mut ring: Vec<(usize, usize)> = vec![(0, 0); n];
        let (mut count, mut node, mut i) = (0usize, 0u32, cx.src.bom);
        let len = cx.src.len();
        while i < len {
            let (sym, j) = match cx.ch(i, len) {
                Some(c) if is_space(c) => {
                    let mut j = cx.step(i);
                    while cx.ch(j, len).is_some_and(is_space) {
                        j = cx.step(j);
                    }
                    (W, j)
                }
                Some(c) => (canon(c) as u32, cx.step(i)),
                None => break,
            };
            ring[count % n] = (i, j);
            count += 1;
            node = self.step(node, sym);
            let mut at = Some(node).filter(|&x| !self.out[x as usize].is_empty());
            at = at.or(self.dict[node as usize]);
            while let Some(x) = at {
                for &t in &self.out[x as usize] {
                    let first = ring[(count - self.lens[t as usize]) % n];
                    record(cx, &targets[t as usize], t, first, j, hits);
                }
                at = self.dict[x as usize];
            }
            i = j;
        }
    }
}

/// Records where target `t` may start, given the extent of the first symbol
/// of its hit and where the hit ends.
fn record(
    cx: &InlineCx,
    target: &Target,
    t: u32,
    first: (usize, usize),
    hit_end: usize,
    hits: &mut Vec<(usize, u32, usize)>,
) {
    if target.simple {
        hits.push((first.0, t, hit_end));
    } else if target.blank_start {
        let mut q = first.0;
        while q < first.1 {
            hits.push((q, t, usize::MAX));
            q = cx.step(q);
        }
    } else {
        hits.push((first.0, t, usize::MAX));
    }
}
