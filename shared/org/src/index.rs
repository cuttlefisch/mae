//! Lookahead without rescanning.
//!
//! Org decides whether `#+begin_X` opens a block by searching forward for its
//! `#+end_X` before the container's limit (the next headline, for a section).
//! Done naively that is a rescan per candidate line, which is how other
//! parsers went quadratic on hostile input. Here every closing line is indexed
//! once, by kind and name, in sorted line order; each question is a binary
//! search. Building the index is one pass over the lines.

use crate::scan;
use crate::text::Source;
use std::collections::HashMap;

pub(crate) struct Index {
    block_ends: HashMap<String, Vec<usize>>,
    latex_ends: HashMap<String, Vec<usize>>,
    drawer_ends: Vec<usize>,
    dynamic_ends: Vec<usize>,
    dynamic_ends_colon: Vec<usize>,
    headlines: Vec<usize>,
    /// For `headlines[k]`, the line where its subtree ends (exclusive), or
    /// the line count.
    subtree_ends: Vec<usize>,
    footnotes: Vec<usize>,
    /// Lines `i` where `i` and `i + 1` are blank and both end in a newline.
    double_blanks: Vec<usize>,
    /// For each line, the first line at or after it that is not an
    /// affiliated keyword (a run of them is collected in O(1), not rescanned
    /// once per line, which is quadratic on a long run of orphans).
    affiliated_run_end: Vec<usize>,
    /// For each line, the first line at or after it that cannot belong to a
    /// table.el table (blank, or not starting with `+` or `|`).
    non_tableel_from: Vec<usize>,
}

impl Index {
    pub fn build(src: &Source) -> Index {
        let mut ix = Index::empty();
        let mut levels = Vec::new();
        for i in 0..src.lines.len() {
            ix.record_line(src, i, &mut levels);
        }
        ix.subtree_ends = subtree_ends(&ix.headlines, &levels, src.lines.len());
        ix.affiliated_run_end = suffix_first(src, |s| scan::affiliated(s).is_none());
        ix.non_tableel_from = suffix_first(src, |s| {
            let r = s.trim_start_matches([' ', '\t']);
            !(r.starts_with('+') || r.starts_with('|'))
        });
        ix
    }

    fn empty() -> Index {
        Index {
            block_ends: HashMap::new(),
            latex_ends: HashMap::new(),
            drawer_ends: Vec::new(),
            dynamic_ends: Vec::new(),
            dynamic_ends_colon: Vec::new(),
            headlines: Vec::new(),
            subtree_ends: Vec::new(),
            footnotes: Vec::new(),
            double_blanks: Vec::new(),
            affiliated_run_end: Vec::new(),
            non_tableel_from: Vec::new(),
        }
    }

    fn record_line(&mut self, src: &Source, i: usize, levels: &mut Vec<usize>) {
        let s = src.content(i);
        if let Some(level) = scan::headline_level(s) {
            self.headlines.push(i);
            levels.push(level);
        }
        if let Some(name) = scan::block_end_name(s) {
            self.block_ends
                .entry(name.to_uppercase())
                .or_default()
                .push(i);
        }
        if let Some(name) = scan::latex_end_name(s) {
            self.latex_ends
                .entry(name.to_uppercase())
                .or_default()
                .push(i);
        }
        if scan::is_drawer_end(s) {
            self.drawer_ends.push(i);
        }
        if scan::is_dynamic_end(s) {
            self.dynamic_ends.push(i);
        }
        if scan::is_dynamic_end_colon(s) {
            self.dynamic_ends_colon.push(i);
        }
        if scan::footnote_definition(s).is_some() {
            self.footnotes.push(i);
        }
        if i + 1 < src.lines.len() && is_blank_with_eol(src, i) && is_blank_with_eol(src, i + 1) {
            self.double_blanks.push(i);
        }
    }

    /// The first `#+end_NAME` line at or after `from` that ends by `limit`.
    pub fn block_end(&self, src: &Source, name: &str, from: usize, limit: usize) -> Option<usize> {
        let v = self.block_ends.get(&name.to_uppercase())?;
        first_within(src, v, from, limit)
    }

    /// The first `\end{NAME}` line at or after `from` that ends by `limit`.
    pub fn latex_end(&self, src: &Source, name: &str, from: usize, limit: usize) -> Option<usize> {
        let v = self.latex_ends.get(&name.to_uppercase())?;
        first_within(src, v, from, limit)
    }

    pub fn drawer_end(&self, src: &Source, from: usize, limit: usize) -> Option<usize> {
        first_within(src, &self.drawer_ends, from, limit)
    }

    pub fn dynamic_end(&self, src: &Source, from: usize, limit: usize) -> Option<usize> {
        first_within(src, &self.dynamic_ends, from, limit)
    }

    pub fn dynamic_end_colon(&self, src: &Source, from: usize, limit: usize) -> Option<usize> {
        first_within(src, &self.dynamic_ends_colon, from, limit)
    }

    /// The first headline line strictly after line `i`.
    pub fn next_headline(&self, i: usize) -> Option<usize> {
        first_at_or_after(&self.headlines, i + 1).map(|k| self.headlines[k])
    }

    /// The line where the subtree of the headline on line `i` ends.
    pub fn subtree_end(&self, i: usize) -> Option<usize> {
        self.headlines
            .binary_search(&i)
            .ok()
            .map(|k| self.subtree_ends[k])
    }

    /// `org-element--footnote-separator` searched from the end of line `i`:
    /// the first later headline, footnote definition or double blank line
    /// whose match ends by `limit`. Returns the line and which it was.
    pub fn footnote_separator(
        &self,
        src: &Source,
        i: usize,
        limit: usize,
    ) -> Option<(usize, Separator)> {
        let line_ok = |l: usize| src.lines[l].end <= limit;
        let hl = first_at_or_after(&self.headlines, i + 1)
            .map(|k| self.headlines[k])
            .filter(|&l| line_ok(l));
        let fd = first_at_or_after(&self.footnotes, i + 1)
            .map(|k| self.footnotes[k])
            .filter(|&l| line_ok(l));
        let bl = first_at_or_after(&self.double_blanks, i + 1)
            .map(|k| self.double_blanks[k])
            .filter(|&l| src.lines[l + 1].next <= limit);
        let candidates = [
            (hl, Separator::Headline),
            (fd, Separator::Footnote),
            (bl, Separator::Blank),
        ];
        candidates
            .into_iter()
            .filter_map(|(l, s)| l.map(|l| (l, s)))
            .min_by_key(|&(l, _)| l)
    }

    pub fn affiliated_run_end(&self, i: usize) -> usize {
        self.affiliated_run_end.get(i).copied().unwrap_or(i)
    }

    pub fn non_tableel_from(&self, i: usize) -> usize {
        self.non_tableel_from.get(i).copied().unwrap_or(i)
    }

    pub fn is_double_blank(&self, i: usize) -> bool {
        self.double_blanks.binary_search(&i).is_ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Separator {
    Headline,
    Footnote,
    Blank,
}

/// For each line `i`, the first line `m >= i` whose content satisfies `stop`
/// (the line count if none): one backward pass.
fn suffix_first(src: &Source, stop: impl Fn(&str) -> bool) -> Vec<usize> {
    let n = src.lines.len();
    let mut out = vec![n; n];
    let mut next = n;
    for i in (0..n).rev() {
        if stop(src.content(i)) {
            next = i;
        }
        out[i] = next;
    }
    out
}

fn is_blank_with_eol(src: &Source, i: usize) -> bool {
    let l = src.lines[i];
    l.next > l.end && src.is_blank(i)
}

fn first_at_or_after(v: &[usize], from: usize) -> Option<usize> {
    let k = v.partition_point(|&l| l < from);
    (k < v.len()).then_some(k)
}

fn first_within(src: &Source, v: &[usize], from: usize, limit: usize) -> Option<usize> {
    let line = v[first_at_or_after(v, from)?];
    (src.lines[line].end <= limit).then_some(line)
}

/// For each headline, the nearest later headline of the same or a higher
/// level (fewer stars), by a monotonic stack: O(n) overall.
fn subtree_ends(headlines: &[usize], levels: &[usize], n_lines: usize) -> Vec<usize> {
    let mut ends = vec![n_lines; headlines.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    for k in (0..headlines.len()).rev() {
        while stack.last().is_some_and(|&(level, _)| level > levels[k]) {
            stack.pop();
        }
        if let Some(&(_, line)) = stack.last() {
            ends[k] = line;
        }
        stack.push((levels[k], headlines[k]));
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtree_ends_find_the_next_same_or_higher_level() {
        let src = Source::new("* a\n** b\n*** c\n** d\n* e\n");
        let ix = Index::build(&src);
        assert_eq!(ix.subtree_end(0), Some(4));
        assert_eq!(ix.subtree_end(1), Some(3));
        assert_eq!(ix.subtree_end(2), Some(3));
        assert_eq!(ix.subtree_end(3), Some(4));
        assert_eq!(ix.subtree_end(4), Some(5));
    }

    #[test]
    fn block_end_respects_the_limit() {
        let src = Source::new("#+begin_quote\nx\n* h\n#+end_quote\n");
        let ix = Index::build(&src);
        assert_eq!(ix.block_end(&src, "quote", 1, src.len()), Some(3));
        // The section limit is the headline on line 2: the end is out of reach.
        assert_eq!(ix.block_end(&src, "QUOTE", 1, src.lines[2].start), None);
        assert_eq!(ix.block_end(&src, "src", 0, src.len()), None);
    }
}
