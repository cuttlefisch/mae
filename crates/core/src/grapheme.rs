use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Count grapheme clusters in a string slice.
///
/// This is the correct unit for cursor movement — one "move right"
/// should advance by one grapheme, not one char. A grapheme cluster
/// may contain multiple chars (e.g., emoji ZWJ sequences, combining marks).
pub fn grapheme_count(s: &str) -> usize {
    s.graphemes(true).count()
}

/// Get display width of a string (accounting for CJK, emoji, combining marks).
///
/// This is the correct unit for screen column positioning. A CJK character
/// is 2 cells wide; a combining mark is 0 cells wide; ASCII is 1 cell wide.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Convert a grapheme index to a byte offset in the string.
///
/// Returns the byte offset of the start of grapheme at `grapheme_idx`,
/// or the string length if `grapheme_idx >= grapheme_count(s)`.
pub fn grapheme_to_byte_offset(s: &str, grapheme_idx: usize) -> usize {
    s.grapheme_indices(true)
        .nth(grapheme_idx)
        .map(|(byte_off, _)| byte_off)
        .unwrap_or(s.len())
}

/// Convert a grapheme index to a char offset in the string.
///
/// Returns the char offset corresponding to the start of the grapheme
/// at `grapheme_idx`, or total char count if out of bounds.
pub fn grapheme_to_char_offset(s: &str, grapheme_idx: usize) -> usize {
    let byte_off = grapheme_to_byte_offset(s, grapheme_idx);
    s[..byte_off].chars().count()
}

/// Get the display width of the first `grapheme_idx` graphemes of a string.
///
/// Used by the renderer to convert a cursor column (grapheme index) to
/// a screen column (display width).
pub fn display_width_up_to_grapheme(s: &str, grapheme_idx: usize) -> usize {
    s.graphemes(true)
        .take(grapheme_idx)
        .map(UnicodeWidthStr::width)
        .sum()
}

/// Get the grapheme count of a ropey line (excluding trailing newline).
///
/// Convenience for cursor movement: line length in graphemes, not chars.
pub fn line_grapheme_count(line: &ropey::RopeSlice) -> usize {
    let s: String = line.chars().collect();
    let trimmed = s.trim_end_matches('\n');
    grapheme_count(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_grapheme_count() {
        assert_eq!(grapheme_count("hello"), 5);
        assert_eq!(grapheme_count(""), 0);
        assert_eq!(grapheme_count(" "), 1);
    }

    #[test]
    fn ascii_display_width() {
        assert_eq!(display_width("hello"), 5);
        assert_eq!(display_width(""), 0);
    }

    #[test]
    fn cjk_display_width() {
        // Each CJK character is 2 cells wide
        assert_eq!(display_width("你好"), 4);
        assert_eq!(display_width("世界"), 4);
        assert_eq!(grapheme_count("你好"), 2);
    }

    #[test]
    fn emoji_display_width() {
        // Basic emoji are typically 2 cells wide
        assert_eq!(grapheme_count("👋"), 1);
        assert_eq!(display_width("👋"), 2);
    }

    #[test]
    fn combining_character() {
        // é can be e + combining acute accent (2 chars, 1 grapheme)
        let s = "e\u{0301}"; // e + combining acute accent
        assert_eq!(grapheme_count(s), 1);
        assert_eq!(s.chars().count(), 2); // but 2 chars
    }

    #[test]
    fn mixed_ascii_cjk_emoji() {
        let s = "hi你好👋";
        assert_eq!(grapheme_count(s), 5); // h, i, 你, 好, 👋
                                          // h=1, i=1, 你=2, 好=2, 👋=2 = 8
        assert_eq!(display_width(s), 8);
    }

    #[test]
    fn grapheme_to_char_offset_ascii() {
        assert_eq!(grapheme_to_char_offset("hello", 0), 0);
        assert_eq!(grapheme_to_char_offset("hello", 2), 2);
        assert_eq!(grapheme_to_char_offset("hello", 5), 5);
    }

    #[test]
    fn grapheme_to_char_offset_combining() {
        let s = "e\u{0301}x"; // é (2 chars) + x (1 char)
        assert_eq!(grapheme_to_char_offset(s, 0), 0); // start of é
        assert_eq!(grapheme_to_char_offset(s, 1), 2); // start of x (char offset 2)
    }

    #[test]
    fn display_width_up_to_grapheme_cjk() {
        let s = "a你b好c";
        // a=1, 你=2, b=1, 好=2, c=1
        assert_eq!(display_width_up_to_grapheme(s, 0), 0);
        assert_eq!(display_width_up_to_grapheme(s, 1), 1); // after 'a'
        assert_eq!(display_width_up_to_grapheme(s, 2), 3); // after '你'
        assert_eq!(display_width_up_to_grapheme(s, 3), 4); // after 'b'
        assert_eq!(display_width_up_to_grapheme(s, 4), 6); // after '好'
        assert_eq!(display_width_up_to_grapheme(s, 5), 7); // after 'c'
    }

    #[test]
    fn line_grapheme_count_strips_newline() {
        let rope = ropey::Rope::from_str("hello\nworld\n");
        assert_eq!(line_grapheme_count(&rope.line(0)), 5);
        assert_eq!(line_grapheme_count(&rope.line(1)), 5);
    }

    #[test]
    fn line_grapheme_count_cjk() {
        let rope = ropey::Rope::from_str("你好世界\n");
        assert_eq!(line_grapheme_count(&rope.line(0)), 4);
    }
}
