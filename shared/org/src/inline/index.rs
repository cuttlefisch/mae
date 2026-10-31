//! Positions found once per document, so forward searches are lookups.

use crate::chars::is_space;

/// The six emphasis marks, in a fixed order.
pub(crate) const MARKS: [u8; 6] = *b"*/_+=~";

pub(crate) struct InlineIndex {
    /// Per mark: positions `m` where the mark could close emphasis given the
    /// REAL text after it (`(not space) MARK (post | line-end)`). A closer at
    /// a region's last byte is valid whatever follows; callers check that.
    closers: [Vec<usize>; 6],
    /// Positions of every `]]`.
    double_close: Vec<usize>,
}

/// The closer's post set: `(any space ?- ?. ?, ?\; ?: ?! ?? ?' ?\" ?\) ?\} ?\\ ?\[)`.
pub(crate) fn is_post(c: char) -> bool {
    is_space(c) || "-.,;:!?'\")}\\[".contains(c)
}

impl InlineIndex {
    pub fn build(text: &str) -> InlineIndex {
        let mut closers: [Vec<usize>; 6] = Default::default();
        let mut double_close = Vec::new();
        let mut prev: Option<char> = None;
        let mut iter = text.char_indices().peekable();
        while let Some((i, c)) = iter.next() {
            let next = iter.peek().map(|&(_, n)| n);
            if c == ']' && next == Some(']') {
                double_close.push(i);
            }
            if let Some(k) = MARKS.iter().position(|&m| m as char == c) {
                let pre_ok = prev.is_some_and(|p| !is_space(p));
                let post_ok = next.is_none_or(|n| n == '\n' || is_post(n));
                if pre_ok && post_ok {
                    closers[k].push(i);
                }
            }
            prev = Some(c);
        }
        InlineIndex {
            closers,
            double_close,
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
}
