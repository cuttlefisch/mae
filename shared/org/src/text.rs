//! The source as lines, and the character classes Org's regexps use.
//!
//! A line ends at `\n` or `\r\n`; a lone `\r` is content. Emacs decodes a
//! CRLF file with DOS line endings by default, so treating `\r\n` as one line
//! ending is what an Org user sees. A leading BOM is trivia: Emacs strips it on
//! decode, so it must not push the first line off column 0.

/// One line of the source, as byte offsets into the whole text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Line {
    /// First byte of the line.
    pub start: usize,
    /// End of the line's content (before `\n` or `\r\n`).
    pub end: usize,
    /// First byte after the line ending (`end` itself on a last line without one).
    pub next: usize,
}

/// The text plus its line table.
pub(crate) struct Source<'a> {
    pub text: &'a str,
    pub lines: Vec<Line>,
    /// Length of the leading BOM (0 or 3).
    pub bom: usize,
}

const BOM: &str = "\u{feff}";

impl<'a> Source<'a> {
    pub fn new(text: &'a str) -> Source<'a> {
        let bom = if text.starts_with(BOM) { BOM.len() } else { 0 };
        Source {
            text,
            lines: split_lines(text, bom),
            bom,
        }
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    /// Content of line `i` (without its line ending).
    pub fn content(&self, i: usize) -> &'a str {
        let l = self.lines[i];
        &self.text[l.start..l.end]
    }

    /// Index of the line containing byte `offset` (the last line for EOF).
    pub fn line_of(&self, offset: usize) -> usize {
        match self.lines.binary_search_by(|l| l.start.cmp(&offset)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        }
    }

    /// Start of the line after the one containing `offset`, or EOF.
    pub fn next_line_start(&self, offset: usize) -> usize {
        if self.lines.is_empty() {
            return self.len();
        }
        self.lines[self.line_of(offset)].next.max(offset)
    }

    pub fn is_blank(&self, i: usize) -> bool {
        is_blank(self.content(i))
    }

    /// `org-element`'s post-blank rule: `skip-chars-forward " \r\t\n"` up to
    /// `limit`, then EOF if there, else the start of the line reached.
    pub fn skip_blank(&self, pos: usize, limit: usize) -> usize {
        let q = self.skip_ws_forward(pos, limit);
        if q >= self.len() {
            q
        } else {
            self.lines[self.line_of(q)].start.max(pos)
        }
    }

    /// The planning/clock variant of [`Source::skip_blank`]: after skipping,
    /// back up over spaces and tabs, so a final whitespace-only line without a
    /// line ending is not consumed.
    pub fn skip_blank_bol(&self, pos: usize, limit: usize) -> usize {
        let q = self.skip_ws_forward(pos, limit);
        let b = self.text.as_bytes();
        let mut r = q;
        while r > pos && matches!(b[r - 1], b' ' | b'\t') {
            r -= 1;
        }
        let at_bol = r == 0 || b[r - 1] == b'\n' || r == self.bom;
        if at_bol {
            r
        } else {
            q
        }
    }

    fn skip_ws_forward(&self, pos: usize, limit: usize) -> usize {
        let b = self.text.as_bytes();
        let limit = limit.min(b.len());
        let mut q = pos;
        while q < limit && matches!(b[q], b' ' | b'\t' | b'\r' | b'\n') {
            q += 1;
        }
        q
    }

    /// `skip-chars-backward " \r\t\n"` from `pos` (not below `floor`), then
    /// `line-beginning-position 2`: the start of the line after the last
    /// non-blank text, or after `floor`'s line when there is none.
    pub fn after_last_nonblank(&self, pos: usize, floor: usize) -> usize {
        let b = self.text.as_bytes();
        let mut q = pos;
        while q > floor && matches!(b[q - 1], b' ' | b'\t' | b'\r' | b'\n') {
            q -= 1;
        }
        self.next_line_start(q)
    }
}

fn split_lines(text: &str, from: usize) -> Vec<Line> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut start = from;
    while start < bytes.len() {
        let nl = bytes[start..].iter().position(|&b| b == b'\n');
        let (end, next) = match nl {
            Some(rel) => {
                let lf = start + rel;
                let end = if lf > start && bytes[lf - 1] == b'\r' {
                    lf - 1
                } else {
                    lf
                };
                (end, lf + 1)
            }
            None => (bytes.len(), bytes.len()),
        };
        lines.push(Line { start, end, next });
        start = next;
    }
    lines
}

/// Only spaces and tabs (Org's `[ \t]*$`).
pub(crate) fn is_blank(s: &str) -> bool {
    s.bytes().all(|b| b == b' ' || b == b'\t')
}

/// Length of the leading run of spaces and tabs.
pub(crate) fn ws_len(s: &str) -> usize {
    s.bytes().take_while(|&b| b == b' ' || b == b'\t').count()
}

/// Emacs's whitespace syntax class, as `\S-` excludes it.
pub(crate) fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c' | '\x0b')
}

/// Emacs's word syntax class, approximated as Unicode alphanumerics.
pub(crate) fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

/// Length of the leading run of non-whitespace characters.
pub(crate) fn non_space_len(s: &str) -> usize {
    s.char_indices()
        .find(|&(_, c)| is_space(c))
        .map_or(s.len(), |(i, _)| i)
}

/// Display column of the first non-blank character (tabs stop every 8).
pub(crate) fn indent_column(s: &str) -> usize {
    let mut col = 0;
    for b in s.bytes() {
        match b {
            b' ' => col += 1,
            b'\t' => col = (col / 8 + 1) * 8,
            _ => break,
        }
    }
    col
}

/// `s` without trailing spaces and tabs.
pub(crate) fn trim_end_blank(s: &str) -> &str {
    s.trim_end_matches([' ', '\t'])
}

/// Case-insensitive ASCII prefix test.
pub(crate) fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<(usize, usize, usize)> {
        Source::new(text)
            .lines
            .iter()
            .map(|l| (l.start, l.end, l.next))
            .collect()
    }

    #[test]
    fn line_endings_lf_crlf_lone_cr_and_no_final_newline() {
        assert_eq!(lines(""), vec![]);
        assert_eq!(lines("a"), vec![(0, 1, 1)]);
        assert_eq!(lines("a\n"), vec![(0, 1, 2)]);
        assert_eq!(lines("a\r\nb"), vec![(0, 1, 3), (3, 4, 4)]);
        // A lone CR is content, not a line break.
        assert_eq!(lines("a\rb\n"), vec![(0, 3, 4)]);
        assert_eq!(lines("\n\n"), vec![(0, 0, 1), (1, 1, 2)]);
        // "\r\r\n": the first CR is content, the second ends the line.
        assert_eq!(lines("\r\r\n"), vec![(0, 1, 3)]);
    }

    #[test]
    fn bom_is_skipped_by_the_line_table() {
        let s = Source::new("\u{feff}* a\n");
        assert_eq!(s.bom, 3);
        assert_eq!(s.content(0), "* a");
    }

    #[test]
    fn indentation_columns_expand_tabs() {
        assert_eq!(indent_column("  x"), 2);
        assert_eq!(indent_column("\tx"), 8);
        assert_eq!(indent_column("  \tx"), 8);
        assert_eq!(indent_column("\t x"), 9);
    }

    #[test]
    fn after_last_nonblank_skips_trailing_blank_lines() {
        let s = Source::new("a\n\n  \nb\n");
        // From the start of "b" (offset 6) back over two blank lines.
        assert_eq!(s.after_last_nonblank(6, 0), 2);
        // Never below the floor.
        // Stopped by the floor: the start of the line after the floor's line.
        assert_eq!(s.after_last_nonblank(6, 4), 6);
        assert_eq!(s.after_last_nonblank(6, 3), 6);
    }
}
