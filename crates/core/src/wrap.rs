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

/// Count display columns to the start of content after a list marker.
/// For `  - item text`, returns 4 (past the `- `).
/// For non-list lines, falls back to `leading_indent_len`.
pub fn content_indent_len(chars: &[char]) -> usize {
    let ws = leading_indent_len(chars);
    let ws_chars: usize = chars
        .iter()
        .take_while(|c| **c == ' ' || **c == '\t')
        .count();
    let rest = &chars[ws_chars..];
    // Detect org/markdown list markers: `- `, `+ `, `* `, `1. `, `1) `
    if rest.len() >= 2 {
        match rest[0] {
            '-' | '+' | '*' if rest[1] == ' ' => return ws + 2,
            '0'..='9' => {
                // Numbered list: skip digits then `. ` or `) `
                let mut i = 0;
                while i < rest.len() && rest[i].is_ascii_digit() {
                    i += 1;
                }
                if i < rest.len()
                    && (rest[i] == '.' || rest[i] == ')')
                    && i + 1 < rest.len()
                    && rest[i + 1] == ' '
                {
                    return ws + i + 2;
                }
            }
            _ => {}
        }
    }
    ws
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
        content_indent_len(&chars)
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
    // Fast path: ASCII-only line shorter than text_width fits in one row.
    // byte length == char count for ASCII, and each ASCII char is 1 display column.
    if line_text.len() <= text_width && line_text.is_ascii() {
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
        content_indent_len(&chars)
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

/// Find the last buffer line that fits within `viewport_height` visual rows
/// starting from `scroll_offset`, honoring a fold-aware `next_visible_line`
/// walk and a partial-row `skip` (revealed sub-line offset) applied only to
/// the first line. This is the single source of truth for "what's the
/// bottom visible buffer row" — used both to clamp the cursor after a
/// wrapped scroll-up step (`Window::scroll_up_line_wrapped`) and to compute
/// the mouse-wheel scroll bottom (`Editor::handle_mouse_scroll`). The two
/// call sites used to each carry an independent copy of this exact walk;
/// commit `c410639f` fixed one real divergence between them by literally
/// copying the algorithm rather than sharing it, which only guarantees the
/// NEXT divergence, not that there won't be one.
///
/// `max_row` is the last valid buffer row (`display_line_count() - 1`).
/// `line_visual_rows(line)` returns the visual row count for `line` (0 skips
/// e.g. a display-region-hidden line). `next_visible_line(line)` returns the
/// next visible line after `line` (fold-aware); the walk stops once it stops
/// making forward progress, so a caller passing an identity-like function
/// can't spin forever.
pub fn last_visible_wrapped_line<R, N>(
    scroll_offset: usize,
    viewport_height: usize,
    skip: usize,
    max_row: usize,
    line_visual_rows: R,
    next_visible_line: N,
) -> usize
where
    R: Fn(usize) -> usize,
    N: Fn(usize) -> usize,
{
    let mut visual = 0;
    let mut last_fit = scroll_offset;
    let mut line = scroll_offset;
    let mut first = true;
    while line <= max_row {
        let rows = line_visual_rows(line);
        if rows > 0 {
            let effective = if first {
                rows.saturating_sub(skip)
            } else {
                rows
            };
            first = false;
            if visual + effective > viewport_height {
                break;
            }
            visual += effective;
            last_fit = line;
        }
        line = next_visible_line(line);
        if line <= last_fit {
            break;
        }
    }
    last_fit
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
        content_indent_len(&chars)
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

    // --- last_visible_wrapped_line: the shared bottom-visible-row walk,
    // consolidated out of Window::scroll_up_line_wrapped and
    // Editor::handle_mouse_scroll (churn-hotspot pass) ---

    #[test]
    fn last_visible_wrapped_line_uniform_rows_fits_exact_viewport() {
        // 20 uniform single-row lines, viewport of 5 rows from line 0 -> last
        // fitting line is 4 (0,1,2,3,4 = 5 rows exactly).
        let bottom = last_visible_wrapped_line(0, 5, 0, 19, |_| 1, |line| line + 1);
        assert_eq!(bottom, 4);
    }

    #[test]
    fn last_visible_wrapped_line_stops_before_a_line_that_would_overflow() {
        // Same as above, but viewport of 5 rows starting at line 17 (near
        // max_row=19) — only 3 lines actually fit (17,18,19).
        let bottom = last_visible_wrapped_line(17, 5, 0, 19, |_| 1, |line| line + 1);
        assert_eq!(bottom, 19);
    }

    /// The exact historical divergence shape (commit `c410639f`): a tall
    /// wrapped line partway through the viewport must not let a later line
    /// overflow the true visual bottom — a naive buffer-line-count clamp
    /// (`scroll_offset + viewport_height - 1`) overstates how many buffer
    /// lines actually fit once one of them wraps to multiple visual rows.
    #[test]
    fn last_visible_wrapped_line_clamps_for_a_tall_wrapped_line() {
        // Line 4 wraps to 3 visual rows; all others are 1 row. Viewport = 20
        // rows starting at line 0: lines 0..=3 (4 rows) + line 4 (3 rows) +
        // lines 5..=17 (13 rows) = 20 rows exactly through line 17; line 18
        // would push the total to 21 and overflow.
        let rows_of = |line: usize| if line == 4 { 3 } else { 1 };
        let bottom = last_visible_wrapped_line(0, 20, 0, 99, rows_of, |line| line + 1);
        assert_eq!(
            bottom, 17,
            "a tall wrapped line mid-viewport must reduce how many buffer \
             lines fit (2 extra rows -> the viewport bottom lands 2 buffer \
             lines earlier than a naive 1-row-per-line count would put it)"
        );
    }

    #[test]
    fn last_visible_wrapped_line_honors_skip_on_first_line_only() {
        // scroll_pixel_offset already reveals 2 of line 0's rows; line 0
        // contributes only (rows - skip) to the budget, but line 1 (the
        // SECOND line) must NOT also have skip subtracted.
        let rows_of = |line: usize| if line == 0 { 4 } else { 1 };
        // skip=2: line 0 contributes 4-2=2 rows; viewport=3 -> 1 more row fits (line 1).
        let bottom = last_visible_wrapped_line(0, 3, 2, 99, rows_of, |line| line + 1);
        assert_eq!(bottom, 1);
    }

    #[test]
    fn last_visible_wrapped_line_skips_fold_gaps_via_next_visible_line() {
        // Lines 5..10 are folded (hidden) — next_visible_line jumps 4 -> 10.
        // Visible sequence from line 0: 0,1,2,3,4,10,11,... Viewport of 5
        // rows exactly covers lines 0..=4 (5 rows); the fold-jumped line 10
        // would be the 6th row and must NOT fit.
        let next_visible = |line: usize| if line == 4 { 10 } else { line + 1 };
        let bottom = last_visible_wrapped_line(0, 5, 0, 99, |_| 1, next_visible);
        assert_eq!(
            bottom, 4,
            "the fold gap must not let extra rows sneak past the viewport budget"
        );
    }

    #[test]
    fn last_visible_wrapped_line_never_goes_below_scroll_offset() {
        // Degenerate: viewport_height=0 (or the very first line already
        // overflows) — must still return AT LEAST scroll_offset, never panic
        // or return something before it.
        let bottom = last_visible_wrapped_line(7, 0, 0, 99, |_| 5, |line| line + 1);
        assert_eq!(bottom, 7);
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
    fn content_indent_list_marker() {
        // "  - item text" → content starts at col 4 (past "  - ")
        let chars: Vec<char> = "  - item text".chars().collect();
        assert_eq!(content_indent_len(&chars), 4);
    }

    #[test]
    fn content_indent_numbered_list() {
        let chars: Vec<char> = "  1. item text".chars().collect();
        assert_eq!(content_indent_len(&chars), 5); // "  1. "
    }

    #[test]
    fn content_indent_no_marker() {
        let chars: Vec<char> = "    hello".chars().collect();
        assert_eq!(content_indent_len(&chars), 4); // falls back to leading whitespace
    }

    #[test]
    fn content_indent_plus_marker() {
        let chars: Vec<char> = "+ item".chars().collect();
        assert_eq!(content_indent_len(&chars), 2); // "+ "
    }

    #[test]
    fn content_indent_star_marker() {
        let chars: Vec<char> = "* item".chars().collect();
        assert_eq!(content_indent_len(&chars), 2); // "* "
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
