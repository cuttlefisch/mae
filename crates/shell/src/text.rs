//! Terminal grid rows as the text that was written to them.
//!
//! A screen row is as wide as the terminal: the cells after a line's text are
//! blank, and a line longer than the screen is soft-wrapped across rows (the
//! last cell of each row it continues from carries `WRAPLINE`). Copying rows
//! as they are puts real spaces where the blank cells were and a newline
//! inside every wrapped command, so a command copied from the shell does not
//! paste back as written. [`logical_lines`] undoes both: a soft-wrapped row
//! joins the next with nothing between them, and a line ends without its
//! trailing blank cells. Every reader of shell text (select mode, a mouse
//! selection, the AI's and Scheme's shell reads) goes through it.

use alacritty_terminal::term::cell::{Cell, Flags};

/// One screen row's characters, and whether its line continues on the next
/// row (a soft wrap).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Row {
    pub text: String,
    pub wrapped: bool,
}

impl Row {
    /// Append `cell`: its character and any combining characters it carries.
    /// The spacer half of a wide character is not text, and is skipped.
    pub(crate) fn push_cell(&mut self, cell: &Cell) {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
        {
            return;
        }
        self.text.push(cell.c);
        if let Some(extra) = cell.zerowidth() {
            self.text.extend(extra.iter());
        }
    }
}

/// The logical lines `rows` hold: soft-wrapped rows joined, each line without
/// the blank cells after its text. A wrapped row keeps every cell (a space at
/// a wrap point is text); only the end of a line is trimmed. A final row that
/// says it wraps (a selection or read that stops mid-line) still ends a line.
pub(crate) fn logical_lines(rows: &[Row]) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for row in rows {
        line.push_str(&row.text);
        if !row.wrapped {
            out.push(end_line(&mut line));
        }
    }
    if rows.last().is_some_and(|r| r.wrapped) {
        out.push(end_line(&mut line));
    }
    out
}

/// `line` without its trailing blank cells (spaces, and the NULs of cells
/// never written), leaving `line` empty for the next.
fn end_line(line: &mut String) -> String {
    let text = line.trim_end_matches([' ', '\0']).to_string();
    line.clear();
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str, wrapped: bool) -> Row {
        Row {
            text: text.to_string(),
            wrapped,
        }
    }

    /// The reported defect, by hand: a short line padded to the width, and a
    /// command soft-wrapped over two rows, come back as written.
    #[test]
    fn padding_is_dropped_and_a_soft_wrap_joins_without_a_newline() {
        let rows = [
            row("$ ls        ", false),
            row("echo aaaaaa", true),
            row("bbbb        ", false),
            row("            ", false),
        ];
        assert_eq!(logical_lines(&rows), ["$ ls", "echo aaaaaabbbb", ""]);
    }

    /// A space at a wrap point is text, and a line of only blanks is an
    /// empty line, not removed.
    #[test]
    fn a_space_at_the_wrap_point_is_kept() {
        let rows = [row("abc ", true), row("def   ", false)];
        assert_eq!(logical_lines(&rows), ["abc def"]);
    }

    /// The property: any lines, wrapped onto any width and padded as a
    /// terminal pads them, read back exactly (less their own trailing
    /// spaces, which a terminal cannot tell from blank cells).
    #[test]
    fn any_lines_wrapped_at_any_width_read_back_as_written() {
        let mut seed: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let alphabet: Vec<char> = "ab -\u{e9}x".chars().collect();
        for _ in 0..500 {
            let width = 1 + next(12) as usize;
            let lines: Vec<String> = (0..1 + next(5))
                .map(|_| {
                    (0..next(30))
                        .map(|_| alphabet[next(alphabet.len() as u64) as usize])
                        .collect()
                })
                .collect();
            let mut rows = Vec::new();
            for l in &lines {
                let chars: Vec<char> = l.chars().collect();
                let chunks: Vec<&[char]> = if chars.is_empty() {
                    vec![&[][..]]
                } else {
                    chars.chunks(width).collect()
                };
                let last = chunks.len() - 1;
                for (i, c) in chunks.iter().enumerate() {
                    let mut text: String = c.iter().collect();
                    text.extend(std::iter::repeat_n(' ', width - c.len()));
                    // A terminal wraps only when the next character does
                    // not fit: a line exactly the width ends unwrapped.
                    rows.push(row(&text, i < last));
                }
            }
            let want: Vec<String> = lines
                .iter()
                .map(|l| l.trim_end_matches(' ').to_string())
                .collect();
            assert_eq!(logical_lines(&rows), want, "width {width}: {lines:?}");
        }
    }

    /// A read that stops on a wrapped row still ends its line.
    #[test]
    fn a_trailing_wrapped_row_ends_the_last_line() {
        assert_eq!(logical_lines(&[row("abc", true)]), ["abc"]);
        assert!(logical_lines(&[]).is_empty());
    }
}
