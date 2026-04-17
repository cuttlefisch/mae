//! Word-wrap helpers shared between core (gj/gk dispatch) and renderer.

/// Characters considered word boundaries for wrapping (neovim `breakat`).
pub fn is_break_char(ch: char) -> bool {
    matches!(
        ch,
        ' ' | '\t'
            | '-'
            | ','
            | ';'
            | ':'
            | '!'
            | '?'
            | '/'
            | '\\'
            | '|'
            | '&'
            | '+'
            | '{'
            | '}'
            | '('
            | ')'
            | '['
            | ']'
            | '<'
            | '>'
    )
}

/// Find the best wrap break point at or before `limit` within `chars[start..]`.
/// Returns the index (in `chars`) where the next display line should start.
/// Prefers breaking after a word-boundary char; falls back to hard break at `limit`.
pub fn find_wrap_break(chars: &[char], start: usize, limit: usize) -> usize {
    let end = (start + limit).min(chars.len());
    if end >= chars.len() {
        return chars.len();
    }
    // Search backward from end for a break character.
    // Don't search further back than half the limit to avoid overly short lines.
    let min_pos = start + limit / 2;
    for i in (min_pos..end).rev() {
        if is_break_char(chars[i]) {
            return i + 1; // break *after* the space/punctuation
        }
    }
    // No good break point — hard break.
    end
}

/// Count leading whitespace characters in a slice.
pub fn leading_indent_len(chars: &[char]) -> usize {
    chars
        .iter()
        .take_while(|c| **c == ' ' || **c == '\t')
        .count()
}

/// Compute the display row and column for a given buffer column within a wrapped line.
///
/// Returns `(display_row_offset, display_col)` where `display_row_offset` is how many
/// display rows down from the first row of this line, and `display_col` is the column
/// within that display row (not including gutter/indent/showbreak prefix).
pub fn wrap_cursor_position(
    line_text: &str,
    cursor_col: usize,
    text_width: usize,
    break_indent: bool,
    show_break_width: usize,
) -> (usize, usize) {
    if text_width == 0 {
        return (0, cursor_col);
    }
    let chars: Vec<char> = line_text
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect();
    let full_count = chars.len();
    if full_count == 0 {
        return (0, 0);
    }
    let indent_len = if break_indent {
        leading_indent_len(&chars)
    } else {
        0
    };
    let cont_prefix_w = indent_len + show_break_width;
    let cont_text_w = if text_width > cont_prefix_w {
        text_width - cont_prefix_w
    } else {
        text_width
    };

    let mut pos = 0;
    let mut row = 0;
    loop {
        let avail = if row == 0 { text_width } else { cont_text_w };
        let end = find_wrap_break(&chars, pos, avail);
        if cursor_col < end || end >= full_count {
            return (row, cursor_col.saturating_sub(pos));
        }
        pos = end;
        row += 1;
    }
}

/// Count total display rows consumed by a wrapped line.
pub fn wrap_line_display_rows(
    line_text: &str,
    text_width: usize,
    break_indent: bool,
    show_break_width: usize,
) -> usize {
    if text_width == 0 {
        return 1;
    }
    let chars: Vec<char> = line_text
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect();
    let full_count = chars.len();
    if full_count == 0 {
        return 1;
    }
    let indent_len = if break_indent {
        leading_indent_len(&chars)
    } else {
        0
    };
    let cont_prefix_w = indent_len + show_break_width;
    let cont_text_w = if text_width > cont_prefix_w {
        text_width - cont_prefix_w
    } else {
        text_width
    };

    let mut pos = 0;
    let mut rows = 0;
    loop {
        let avail = if rows == 0 { text_width } else { cont_text_w };
        let end = find_wrap_break(&chars, pos, avail);
        rows += 1;
        if end >= full_count {
            return rows;
        }
        pos = end;
    }
}

/// Compute the buffer column for the start of a given wrap display row.
pub fn wrap_row_start_col(
    line_text: &str,
    target_row: usize,
    text_width: usize,
    break_indent: bool,
    show_break_width: usize,
) -> usize {
    if text_width == 0 || target_row == 0 {
        return 0;
    }
    let chars: Vec<char> = line_text
        .chars()
        .filter(|c| *c != '\n' && *c != '\r')
        .collect();
    let full_count = chars.len();
    if full_count == 0 {
        return 0;
    }
    let indent_len = if break_indent {
        leading_indent_len(&chars)
    } else {
        0
    };
    let cont_prefix_w = indent_len + show_break_width;
    let cont_text_w = if text_width > cont_prefix_w {
        text_width - cont_prefix_w
    } else {
        text_width
    };

    let mut pos = 0;
    let mut row = 0;
    loop {
        let avail = if row == 0 { text_width } else { cont_text_w };
        let end = find_wrap_break(&chars, pos, avail);
        row += 1;
        if row > target_row || end >= full_count {
            return pos;
        }
        pos = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundary_break() {
        let chars: Vec<char> = "hello world foo bar".chars().collect();
        // Width 12: "hello world " fits, break after space at index 12
        let brk = find_wrap_break(&chars, 0, 12);
        assert_eq!(brk, 12); // after the space in "hello world "
    }

    #[test]
    fn hard_break_no_boundary() {
        let chars: Vec<char> = "abcdefghijklmnop".chars().collect();
        let brk = find_wrap_break(&chars, 0, 10);
        assert_eq!(brk, 10); // hard break, no word boundary
    }

    #[test]
    fn wrap_cursor_on_first_row() {
        let (row, col) = wrap_cursor_position("hello world foo", 3, 80, false, 0);
        assert_eq!(row, 0);
        assert_eq!(col, 3);
    }

    #[test]
    fn wrap_display_rows_short_line() {
        assert_eq!(wrap_line_display_rows("short", 80, false, 0), 1);
    }

    #[test]
    fn wrap_display_rows_empty() {
        assert_eq!(wrap_line_display_rows("", 80, false, 0), 1);
    }

    #[test]
    fn leading_indent() {
        let chars: Vec<char> = "    hello".chars().collect();
        assert_eq!(leading_indent_len(&chars), 4);
    }
}
