//! Word-wrap helpers shared between core (gj/gk dispatch) and renderer.

use unicode_width::UnicodeWidthChar;

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

/// Display width of a single character (CJK = 2, most others = 1).
pub fn char_width(ch: char) -> usize {
    ch.width().unwrap_or(0)
}

/// Find the best wrap break point within `chars[start..]` that fits in `limit`
/// display columns. Returns the char index where the next display line should start.
/// Prefers breaking after a word-boundary char; falls back to hard break.
pub fn find_wrap_break(chars: &[char], start: usize, limit: usize) -> usize {
    // Walk forward accumulating display width to find the hard-break index.
    let mut width = 0;
    let mut end = start;
    for &ch in &chars[start..] {
        let w = char_width(ch);
        if width + w > limit {
            break;
        }
        width += w;
        end += 1;
    }
    if end >= chars.len() {
        return chars.len();
    }
    // Search backward from end for a break character.
    // Don't search further back than half the consumed chars to avoid overly short lines.
    let half_chars = (end - start) / 2;
    let min_pos = start + half_chars;
    for i in (min_pos..end).rev() {
        if is_break_char(chars[i]) {
            return i + 1; // break *after* the space/punctuation
        }
    }
    // No good break point — hard break.
    end
}

/// Count leading whitespace display columns in a slice.
pub fn leading_indent_len(chars: &[char]) -> usize {
    chars
        .iter()
        .take_while(|c| **c == ' ' || **c == '\t')
        .map(|c| char_width(*c))
        .sum()
}

/// Display width of a char slice.
pub fn slice_display_width(chars: &[char]) -> usize {
    chars.iter().map(|c| char_width(*c)).sum()
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
            // Return display column (sum of char widths from pos to cursor_col)
            let display_col = slice_display_width(&chars[pos..cursor_col.min(end)]);
            return (row, display_col);
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

    #[test]
    fn cjk_char_width() {
        // CJK unified ideographs are 2 columns wide
        assert_eq!(char_width('中'), 2);
        assert_eq!(char_width('a'), 1);
        assert_eq!(char_width(' '), 1);
    }

    #[test]
    fn cjk_wrap_break() {
        // "你好世界" = 4 CJK chars = 8 display columns
        let chars: Vec<char> = "你好世界".chars().collect();
        // limit=5 cols: "你好" = 4 cols fits, "你好世" = 6 cols doesn't → break at 2
        let brk = find_wrap_break(&chars, 0, 5);
        assert_eq!(brk, 2);
    }

    #[test]
    fn cjk_mixed_wrap() {
        // "ab你好cd" = 2+4+2 = 8 display columns
        let chars: Vec<char> = "ab你好cd".chars().collect();
        // limit=6: "ab你好" = 2+4 = 6 cols → fits, break at 4
        let brk = find_wrap_break(&chars, 0, 6);
        assert_eq!(brk, 4);
    }

    #[test]
    fn slice_display_width_mixed() {
        let chars: Vec<char> = "a你b".chars().collect();
        assert_eq!(slice_display_width(&chars), 4); // 1+2+1
    }

    #[test]
    fn wrap_cursor_position_cjk() {
        // "你好世界" with text_width=5: first row fits "你好" (4 cols)
        let (row, col) = wrap_cursor_position("你好世界", 0, 5, false, 0);
        assert_eq!(row, 0);
        assert_eq!(col, 0); // display col 0
        let (row, col) = wrap_cursor_position("你好世界", 1, 5, false, 0);
        assert_eq!(row, 0);
        assert_eq!(col, 2); // "好" starts at display col 2
    }
}
