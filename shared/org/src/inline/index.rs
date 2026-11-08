//! Positions found once per document, so forward searches are lookups.
//!
//! Every object parser that Org writes as a forward search (`search-forward`,
//! `re-search-forward`, `scan-lists`) asks this index instead: the answer is a
//! binary search over positions recorded in one pass, so a run of unclosed
//! openers costs O(log n) each rather than a scan to the end of the region.

use crate::chars::{is_space, is_word};
use crate::text::{LineEnding, Source};

/// The six emphasis marks, in a fixed order.
pub(crate) const MARKS: [u8; 6] = *b"*/_+=~";

/// Literal needles a parser searches forward (or backward) for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Needle {
    /// `$`
    Dollar,
    /// `$$` (overlapping occurrences: `$$$` has two)
    Dollars,
    /// `\)`
    CloseParen,
    /// `\]`
    CloseBracket,
    /// `@@`
    AtAt,
    /// `)}}}`, the end of a macro's arguments
    MacroClose,
    /// `;`
    Semicolon,
    /// `]` or `>`: what ends a timestamp's raw value
    TsClose,
    /// `@` followed by a citation-key character
    CiteKey,
    /// `>`
    Gt,
    /// `)`
    RParen,
    /// What ends an inline babel call's name (`[^ \t\n[(]+`): a blank, `[`,
    /// `(` or a line ending.
    CallStop,
    /// What ends an inline src block's language (`[^ \t\n[{]+`): a blank,
    /// `[`, `{` or a line ending.
    SrcStop,
}

const NEEDLES: usize = 13;

impl Needle {
    fn slot(self) -> usize {
        self as usize
    }

    fn len(self) -> usize {
        match self {
            Needle::Dollar | Needle::Semicolon | Needle::TsClose | Needle::Gt => 1,
            Needle::RParen | Needle::CallStop | Needle::SrcStop => 1,
            Needle::Dollars | Needle::CloseParen | Needle::CloseBracket | Needle::AtAt => 2,
            Needle::CiteKey => 2, // `@` and at least one key byte
            Needle::MacroClose => 4,
        }
    }
}

/// A bracket kind `scan-lists` pairs (with a syntax table where only that
/// kind of bracket is a paren and everything else is a word character).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bracket {
    Square,
    Round,
    Curly,
}

impl Bracket {
    fn of(b: u8) -> Option<(Bracket, bool)> {
        match b {
            b'[' => Some((Bracket::Square, true)),
            b']' => Some((Bracket::Square, false)),
            b'(' => Some((Bracket::Round, true)),
            b')' => Some((Bracket::Round, false)),
            b'{' => Some((Bracket::Curly, true)),
            b'}' => Some((Bracket::Curly, false)),
            _ => None,
        }
    }
}

/// An opening bracket's match: where it closes, and how deeply brackets of
/// its kind nest inside it, itself included (`{a{b}}` has depth 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pair {
    pub open: usize,
    pub close: usize,
    pub depth: u32,
    /// Do its direct children (pairs of its kind) all have the same depth?
    /// True when it has none.
    pub uniform: bool,
}

pub(crate) struct InlineIndex {
    /// Per mark: positions `m` where the mark could close emphasis given the
    /// REAL text after it (`(not space) MARK (post | line-end)`). A closer at
    /// a region's last byte is valid whatever follows; callers check that.
    closers: [Vec<usize>; 6],
    /// Positions of every `]]`.
    double_close: Vec<usize>,
    needles: [Vec<usize>; NEEDLES],
    /// Per bracket kind, every matched pair, sorted by opening position.
    pairs: [Vec<Pair>; 3],
    /// For the `k`th `>`: how many ASCII digits end just before the char
    /// that precedes it (the `[0-9]+` of a repeater's `+[0-9]+[dwmy]>`).
    /// Runs before distinct `>`s are disjoint, so this is one pass.
    gt_digits: Vec<u32>,
}

/// The closer's post set: `(any space ?- ?. ?, ?\; ?: ?! ?? ?' ?\" ?\) ?\} ?\\ ?\[)`.
pub(crate) fn is_post(c: char) -> bool {
    is_space(c) || "-.,;:!?'\")}\\[".contains(c)
}

/// Per byte, 1 + its index in [`MARKS`], or 0 for a byte that is no mark.
const MARK_SLOTS: [u8; 256] = {
    let mut t = [0u8; 256];
    let mut k = 0;
    while k < MARKS.len() {
        t[MARKS[k] as usize] = k as u8 + 1;
        k += 1;
    }
    t
};

/// The index in [`MARKS`] of `c`, on one lookup.
fn mark_slot(c: u8) -> Option<usize> {
    usize::from(MARK_SLOTS[usize::from(c)]).checked_sub(1)
}

/// The bytes [`needles_at`] or a bracket pair can start at: every other
/// byte (most of any text) is passed over on one lookup.
const MAY_INDEX: [bool; 256] = {
    let mut t = [false; 256];
    let mut k = 0;
    let bytes = b"$\\@;]>)\n\r \t[({}";
    while k < bytes.len() {
        t[bytes[k] as usize] = true;
        k += 1;
    }
    t
};

/// The needles that start at byte `i`.
fn needles_at(b: &[u8], i: usize, eol: LineEnding, out: &mut Vec<Needle>) {
    let c = b[i];
    let next = b.get(i + 1).copied();
    match c {
        b'$' => {
            out.push(Needle::Dollar);
            if next == Some(b'$') {
                out.push(Needle::Dollars);
            }
        }
        b'\\' if next == Some(b')') => out.push(Needle::CloseParen),
        b'\\' if next == Some(b']') => out.push(Needle::CloseBracket),
        b'@' if next == Some(b'@') => out.push(Needle::AtAt),
        b';' => out.push(Needle::Semicolon),
        b']' => out.push(Needle::TsClose),
        b'>' => out.extend([Needle::TsClose, Needle::Gt]),
        b')' => {
            out.push(Needle::RParen);
            if b[i + 1..].starts_with(b"}}}") {
                out.push(Needle::MacroClose);
            }
        }
        _ => {}
    }
    let line_end = match eol {
        LineEnding::Lf => c == b'\n',
        LineEnding::Cr => c == b'\r',
        LineEnding::CrLf => c == b'\n' || (c == b'\r' && next == Some(b'\n')),
    };
    if line_end || matches!(c, b' ' | b'\t' | b'[') {
        out.extend([Needle::CallStop, Needle::SrcStop]);
    } else if c == b'(' {
        out.push(Needle::CallStop);
    } else if c == b'{' {
        out.push(Needle::SrcStop);
    }
}

/// `org-element-citation-key-re`'s key characters: word syntax or one of
/// ``-.:?!`'/*@+|(){}<>&_^$#%~``.
pub(crate) fn is_cite_key_char(c: char) -> bool {
    is_word(c) || "-.:?!`'/*@+|(){}<>&_^$#%~".contains(c)
}

impl InlineIndex {
    pub fn build(src: &Source) -> InlineIndex {
        let text = src.text;
        let mut closers: [Vec<usize>; 6] = Default::default();
        let mut double_close = Vec::new();
        // Byte by byte: the marks and `]` are ASCII, so a match is a char,
        // and only a match's neighbours are decoded.
        let b = text.as_bytes();
        for (i, &c) in b.iter().enumerate() {
            if c == b']' && b.get(i + 1) == Some(&b']') {
                double_close.push(i);
            }
            if let Some(k) = mark_slot(c) {
                let prev = text[..i].chars().next_back();
                let next = text[i + 1..].chars().next();
                let pre_ok = prev.is_some_and(|p| !is_space(p));
                let post_ok = next.is_none_or(|n| n == '\n' || is_post(n));
                if pre_ok && post_ok {
                    closers[k].push(i);
                }
            }
        }
        let mut ix = InlineIndex {
            closers,
            double_close,
            needles: Default::default(),
            pairs: Default::default(),
            gt_digits: Vec::new(),
        };
        ix.index_bytes(text, src.eol);
        ix.gt_digits = ix.needles[Needle::Gt.slot()]
            .iter()
            .map(|&g| {
                let run = text.as_bytes()[..g.saturating_sub(1)]
                    .iter()
                    .rev()
                    .take_while(|c| c.is_ascii_digit())
                    .count();
                u32::try_from(run).unwrap_or(u32::MAX)
            })
            .collect();
        ix.index_cite_keys(text);
        ix
    }

    fn index_bytes(&mut self, text: &str, eol: LineEnding) {
        let b = text.as_bytes();
        // Per open bracket: its position, and the greatest and least depth
        // of its direct children so far (0 and u32::MAX: none yet).
        let mut stacks: [Vec<(usize, u32, u32)>; 3] = Default::default();
        let mut found = Vec::with_capacity(4);
        for (i, &c) in b.iter().enumerate() {
            if !MAY_INDEX[usize::from(c)] {
                continue;
            }
            found.clear();
            needles_at(b, i, eol, &mut found);
            for n in &found {
                self.needles[n.slot()].push(i);
            }
            if let Some((kind, open)) = Bracket::of(c) {
                let stack = &mut stacks[kind as usize];
                if open {
                    stack.push((i, 0, u32::MAX));
                } else if let Some((o, max, min)) = stack.pop() {
                    let depth = max + 1;
                    if let Some(parent) = stack.last_mut() {
                        parent.1 = parent.1.max(depth);
                        parent.2 = parent.2.min(depth);
                    }
                    self.pairs[kind as usize].push(Pair {
                        open: o,
                        close: i,
                        depth,
                        uniform: min == u32::MAX || min == max,
                    });
                }
            }
        }
        for p in &mut self.pairs {
            p.sort_unstable_by_key(|p| p.open);
        }
    }

    fn index_cite_keys(&mut self, text: &str) {
        let keys = &mut self.needles[Needle::CiteKey.slot()];
        for (i, _) in text.bytes().enumerate().filter(|&(_, c)| c == b'@') {
            if text[i + 1..].chars().next().is_some_and(is_cite_key_char) {
                keys.push(i);
            }
        }
    }

    /// The first indexed closer for `mark` in `[from, end)`.
    pub fn closer(&self, mark: u8, from: usize, end: usize) -> Option<usize> {
        let v = &self.closers[MARKS.iter().position(|&m| m == mark)?];
        let k = v.partition_point(|&p| p < from);
        v.get(k).copied().filter(|&p| p < end)
    }

    /// The first `]]` starting in `[from, end - 1)`.
    pub fn double_close(&self, from: usize, end: usize) -> Option<usize> {
        let k = self.double_close.partition_point(|&p| p < from);
        self.double_close.get(k).copied().filter(|&p| p + 2 <= end)
    }

    /// The first occurrence of `n` starting at or after `from` that ends
    /// by `end` (`search-forward` bounded by the region).
    pub fn next(&self, n: Needle, from: usize, end: usize) -> Option<usize> {
        let v = &self.needles[n.slot()];
        let k = v.partition_point(|&p| p < from);
        v.get(k).copied().filter(|&p| p + n.len() <= end)
    }

    /// The last occurrence of `n` starting in `[from, before)`
    /// (`search-backward` from `before` bounded by `from`; the needles it is
    /// used for are one byte long).
    pub fn prev(&self, n: Needle, from: usize, before: usize) -> Option<usize> {
        let v = &self.needles[n.slot()];
        let k = v.partition_point(|&p| p < before);
        k.checked_sub(1)
            .map(|k| v[k])
            .filter(|&p| p >= from && p + n.len() <= before)
    }

    /// How many ASCII digits end at `gt - 1`, where `gt` holds a `>`: the run
    /// just before the char that precedes the `>`.
    pub fn digits_before_gt_unit(&self, gt: usize) -> usize {
        let v = &self.needles[Needle::Gt.slot()];
        v.binary_search(&gt)
            .ok()
            .map_or(0, |k| self.gt_digits[k] as usize)
    }

    /// The bracket opening at `open`, if it is closed by `end` (`scan-lists`
    /// over the region `[.., end)`).
    pub fn pair(&self, kind: Bracket, open: usize, end: usize) -> Option<Pair> {
        let v = &self.pairs[kind as usize];
        let k = v.binary_search_by_key(&open, |p| p.open).ok()?;
        Some(v[k]).filter(|p| p.close < end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The mark table answers as a search of [`MARKS`] does, for every byte.
    #[test]
    fn mark_slots_are_the_marks_positions() {
        for c in 0..=255u8 {
            assert_eq!(mark_slot(c), MARKS.iter().position(|&m| m == c), "{c:#x}");
        }
    }

    /// Every byte the index passes over starts no needle and no bracket,
    /// whatever follows it and whatever the line ending.
    #[test]
    fn bytes_passed_over_index_nothing() {
        let mut found = Vec::new();
        for c in (0..=255u8).filter(|&c| !MAY_INDEX[usize::from(c)]) {
            assert_eq!(Bracket::of(c), None, "byte {c:#x}");
            for next in 0..=255u8 {
                for eol in [LineEnding::Lf, LineEnding::Cr, LineEnding::CrLf] {
                    for b in [&[c][..], &[c, next], &[c, next, b'}', b'}']] {
                        needles_at(b, 0, eol, &mut found);
                        assert!(found.is_empty(), "byte {c:#x} before {next:#x}: {found:?}");
                    }
                }
            }
        }
    }
}
