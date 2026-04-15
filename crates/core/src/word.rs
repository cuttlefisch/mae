use ropey::Rope;

/// Character classification for vi-style word motions.
#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum CharClass {
    Word,
    Punctuation,
    Whitespace,
    Newline,
}

/// Classify a character for word motion purposes.
pub fn classify(ch: char) -> CharClass {
    if ch == '\n' || ch == '\r' {
        CharClass::Newline
    } else if ch.is_whitespace() {
        CharClass::Whitespace
    } else if ch.is_alphanumeric() || ch == '_' {
        CharClass::Word
    } else {
        CharClass::Punctuation
    }
}

/// Maximum valid cursor position in a rope (last char index, or 0 if empty).
fn max_pos(rope: &Rope) -> usize {
    rope.len_chars().saturating_sub(1)
}

/// vi `w` — move to start of next word.
///
/// Algorithm: skip chars of same class as current, then skip whitespace.
/// Newlines act as word boundaries (stop at them).
pub fn word_start_forward(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    if len == 0 || pos >= max_pos(rope) {
        return max_pos(rope);
    }

    let mut p = pos;
    let start_class = classify(rope.char(p));

    // If on a newline, just advance past it
    if start_class == CharClass::Newline {
        p += 1;
        return p.min(max_pos(rope));
    }

    // Skip chars of same class
    while p < len {
        let ch = rope.char(p);
        if classify(ch) != start_class {
            break;
        }
        p += 1;
    }

    // Skip whitespace (not newlines)
    while p < len {
        let ch = rope.char(p);
        let cls = classify(ch);
        if cls != CharClass::Whitespace {
            break;
        }
        p += 1;
    }

    p.min(max_pos(rope))
}

/// vi `b` — move to start of previous word.
pub fn word_start_backward(rope: &Rope, pos: usize) -> usize {
    if rope.len_chars() == 0 || pos == 0 {
        return 0;
    }

    let mut p = pos;

    // Move back one to look at the char before cursor
    p -= 1;

    // Skip whitespace/newlines backward
    while p > 0
        && matches!(
            classify(rope.char(p)),
            CharClass::Whitespace | CharClass::Newline
        )
    {
        p -= 1;
    }

    if p == 0 {
        return 0;
    }

    // Now on a word/punct char. Skip backward over same class.
    let target_class = classify(rope.char(p));
    while p > 0 {
        let prev_class = classify(rope.char(p - 1));
        if prev_class != target_class {
            break;
        }
        p -= 1;
    }

    p
}

/// vi `e` — move to end of current/next word.
pub fn word_end_forward(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    if len == 0 || pos >= max_pos(rope) {
        return max_pos(rope);
    }

    let mut p = pos;

    // If not at start, advance one to avoid staying on current word end
    if p + 1 < len {
        p += 1;
    }

    // Skip whitespace/newlines
    while p < len
        && matches!(
            classify(rope.char(p)),
            CharClass::Whitespace | CharClass::Newline
        )
    {
        p += 1;
    }

    if p >= len {
        return max_pos(rope);
    }

    // Now on a word/punct char. Advance to end of this class.
    let target_class = classify(rope.char(p));
    while p + 1 < len {
        let next_class = classify(rope.char(p + 1));
        if next_class != target_class {
            break;
        }
        p += 1;
    }

    p.min(max_pos(rope))
}

/// vi `W` — move to start of next WORD (whitespace-delimited).
pub fn big_word_start_forward(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    if len == 0 || pos >= max_pos(rope) {
        return max_pos(rope);
    }

    let mut p = pos;

    // Skip non-whitespace
    while p < len {
        let cls = classify(rope.char(p));
        if cls == CharClass::Whitespace || cls == CharClass::Newline {
            break;
        }
        p += 1;
    }

    // Skip whitespace/newlines
    while p < len {
        let cls = classify(rope.char(p));
        if cls != CharClass::Whitespace && cls != CharClass::Newline {
            break;
        }
        p += 1;
    }

    p.min(max_pos(rope))
}

/// vi `B` — move to start of previous WORD (whitespace-delimited).
pub fn big_word_start_backward(rope: &Rope, pos: usize) -> usize {
    if rope.len_chars() == 0 || pos == 0 {
        return 0;
    }

    let mut p = pos - 1;

    // Skip whitespace/newlines backward
    while p > 0
        && matches!(
            classify(rope.char(p)),
            CharClass::Whitespace | CharClass::Newline
        )
    {
        p -= 1;
    }

    if p == 0 {
        return 0;
    }

    // Skip non-whitespace backward
    while p > 0 {
        let cls = classify(rope.char(p - 1));
        if cls == CharClass::Whitespace || cls == CharClass::Newline {
            break;
        }
        p -= 1;
    }

    p
}

/// vi `E` — move to end of current/next WORD (whitespace-delimited).
pub fn big_word_end_forward(rope: &Rope, pos: usize) -> usize {
    let len = rope.len_chars();
    if len == 0 || pos >= max_pos(rope) {
        return max_pos(rope);
    }

    let mut p = pos;

    // Advance one to move off current position
    if p + 1 < len {
        p += 1;
    }

    // Skip whitespace/newlines
    while p < len
        && matches!(
            classify(rope.char(p)),
            CharClass::Whitespace | CharClass::Newline
        )
    {
        p += 1;
    }

    if p >= len {
        return max_pos(rope);
    }

    // Advance to end of non-whitespace
    while p + 1 < len {
        let cls = classify(rope.char(p + 1));
        if cls == CharClass::Whitespace || cls == CharClass::Newline {
            break;
        }
        p += 1;
    }

    p.min(max_pos(rope))
}

/// vi `f` — find char forward on current line (inclusive).
/// Returns the column (not char offset) of the target, or None.
pub fn find_char_forward(rope: &Rope, line: usize, col: usize, target: char) -> Option<usize> {
    if line >= rope.len_lines() {
        return None;
    }
    let line_slice = rope.line(line);
    let line_len = line_slice.len_chars();
    // Exclude trailing newline
    let end = if line_len > 0 && line_slice.char(line_len - 1) == '\n' {
        line_len - 1
    } else {
        line_len
    };

    ((col + 1)..end).find(|&c| line_slice.char(c) == target)
}

/// vi `F` — find char backward on current line (inclusive).
pub fn find_char_backward(rope: &Rope, line: usize, col: usize, target: char) -> Option<usize> {
    if line >= rope.len_lines() {
        return None;
    }
    let line_slice = rope.line(line);
    if col == 0 {
        return None;
    }

    let search_end = col.min(line_slice.len_chars());
    (0..search_end)
        .rev()
        .find(|&c| line_slice.char(c) == target)
}

/// vi `t` — find char forward, stop one before (till).
pub fn find_char_forward_till(rope: &Rope, line: usize, col: usize, target: char) -> Option<usize> {
    find_char_forward(rope, line, col, target).map(|c| c.saturating_sub(1).max(col))
}

/// vi `T` — find char backward, stop one after (till).
pub fn find_char_backward_till(
    rope: &Rope,
    line: usize,
    col: usize,
    target: char,
) -> Option<usize> {
    find_char_backward(rope, line, col, target).map(|c| (c + 1).min(col))
}

/// vi `%` — find matching bracket.
pub fn matching_bracket(rope: &Rope, pos: usize) -> Option<usize> {
    let len = rope.len_chars();
    if pos >= len {
        return None;
    }

    let ch = rope.char(pos);
    let (target, forward) = match ch {
        '(' => (')', true),
        '[' => (']', true),
        '{' => ('}', true),
        ')' => ('(', false),
        ']' => ('[', false),
        '}' => ('{', false),
        _ => return None,
    };

    let mut depth: i32 = 1;
    if forward {
        let mut p = pos + 1;
        while p < len {
            let c = rope.char(p);
            if c == target {
                depth -= 1;
                if depth == 0 {
                    return Some(p);
                }
            } else if c == ch {
                depth += 1;
            }
            p += 1;
        }
    } else {
        if pos == 0 {
            return None;
        }
        let mut p = pos - 1;
        loop {
            let c = rope.char(p);
            if c == target {
                depth -= 1;
                if depth == 0 {
                    return Some(p);
                }
            } else if c == ch {
                depth += 1;
            }
            if p == 0 {
                break;
            }
            p -= 1;
        }
    }
    None
}

/// vi `}` — move forward to next blank line (paragraph boundary).
/// Returns the line number.
pub fn paragraph_forward(rope: &Rope, line: usize) -> usize {
    let total = rope.len_lines();
    if total == 0 {
        return 0;
    }

    let mut l = line + 1;

    // Skip non-blank lines first
    while l < total && !is_blank_line(rope, l) {
        l += 1;
    }

    // Then skip blank lines
    while l < total && is_blank_line(rope, l) {
        l += 1;
    }

    // We want to land on the blank line, not past it — back up if we went past blanks
    // Actually, vi `}` lands on the first blank line after text
    // Re-implement: find next blank line after current position
    let mut l = line + 1;
    let on_blank = line < total && is_blank_line(rope, line);

    if on_blank {
        // Skip blank lines to find text
        while l < total && is_blank_line(rope, l) {
            l += 1;
        }
        // Skip text to find next blank
        while l < total && !is_blank_line(rope, l) {
            l += 1;
        }
    } else {
        // Skip text to find next blank
        while l < total && !is_blank_line(rope, l) {
            l += 1;
        }
    }

    l.min(total.saturating_sub(1))
}

/// vi `{` — move backward to previous blank line (paragraph boundary).
pub fn paragraph_backward(rope: &Rope, line: usize) -> usize {
    if rope.len_chars() == 0 || line == 0 {
        return 0;
    }

    let total = rope.len_lines();
    let mut l = line.min(total.saturating_sub(1));

    if l == 0 {
        return 0;
    }
    l -= 1;

    let on_blank = is_blank_line(rope, line);

    if on_blank {
        // Skip blank lines backward
        while l > 0 && is_blank_line(rope, l) {
            l -= 1;
        }
        // Skip text backward
        while l > 0 && !is_blank_line(rope, l) {
            l -= 1;
        }
    } else {
        // Skip text backward to find blank
        while l > 0 && !is_blank_line(rope, l) {
            l -= 1;
        }
    }

    l
}

/// Check if a line is blank (empty or whitespace-only).
fn is_blank_line(rope: &Rope, line: usize) -> bool {
    if line >= rope.len_lines() {
        return true;
    }
    let line_slice = rope.line(line);
    for i in 0..line_slice.len_chars() {
        let ch = line_slice.char(i);
        if ch == '\n' || ch == '\r' {
            continue;
        }
        if !ch.is_whitespace() {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- CharClass ---

    #[test]
    fn classify_word_chars() {
        assert_eq!(classify('a'), CharClass::Word);
        assert_eq!(classify('Z'), CharClass::Word);
        assert_eq!(classify('0'), CharClass::Word);
        assert_eq!(classify('_'), CharClass::Word);
    }

    #[test]
    fn classify_punctuation() {
        assert_eq!(classify('.'), CharClass::Punctuation);
        assert_eq!(classify('+'), CharClass::Punctuation);
        assert_eq!(classify('('), CharClass::Punctuation);
        assert_eq!(classify(';'), CharClass::Punctuation);
    }

    #[test]
    fn classify_whitespace_and_newline() {
        assert_eq!(classify(' '), CharClass::Whitespace);
        assert_eq!(classify('\t'), CharClass::Whitespace);
        assert_eq!(classify('\n'), CharClass::Newline);
        assert_eq!(classify('\r'), CharClass::Newline);
    }

    // --- word_start_forward (w) ---

    #[test]
    fn w_word_to_punct() {
        let rope = Rope::from_str("hello.world");
        // from 'h' → should skip 'hello' and land on '.'
        assert_eq!(word_start_forward(&rope, 0), 5);
    }

    #[test]
    fn w_word_space_word() {
        let rope = Rope::from_str("hello world");
        assert_eq!(word_start_forward(&rope, 0), 6);
    }

    #[test]
    fn w_at_end_of_buffer() {
        let rope = Rope::from_str("hi");
        assert_eq!(word_start_forward(&rope, 1), 1);
    }

    #[test]
    fn w_across_newline() {
        let rope = Rope::from_str("hello\nworld");
        // From 'h', skip 'hello', hit newline → stop at newline
        let pos = word_start_forward(&rope, 0);
        assert_eq!(pos, 5); // lands on '\n'
                            // From newline, advance past it
        let pos2 = word_start_forward(&rope, 5);
        assert_eq!(pos2, 6); // lands on 'w'
    }

    #[test]
    fn w_empty_rope() {
        let rope = Rope::from_str("");
        assert_eq!(word_start_forward(&rope, 0), 0);
    }

    #[test]
    fn w_multiple_spaces() {
        let rope = Rope::from_str("a   b");
        assert_eq!(word_start_forward(&rope, 0), 4); // skip 'a', skip spaces, land on 'b'
    }

    // --- word_start_backward (b) ---

    #[test]
    fn b_basic() {
        let rope = Rope::from_str("hello world");
        assert_eq!(word_start_backward(&rope, 6), 0); // from 'w' back to 'h'
    }

    #[test]
    fn b_at_start() {
        let rope = Rope::from_str("hello");
        assert_eq!(word_start_backward(&rope, 0), 0);
    }

    #[test]
    fn b_from_space() {
        let rope = Rope::from_str("hello world");
        // pos 5 is space, move back to start of 'hello'
        assert_eq!(word_start_backward(&rope, 6), 0);
    }

    #[test]
    fn b_punct_to_word() {
        let rope = Rope::from_str("hello.world");
        assert_eq!(word_start_backward(&rope, 6), 5); // from 'w' → '.'? No, '.' is punct, 'w' is word → back to 'w' at 6
                                                      // Actually from pos 6 ('w'), back one is '.', which is punct, skip punct → just '.' → back to start of punct = 5
                                                      // Then from 5 (which is '.'), go back: that's after the word 'hello'
    }

    // --- word_end_forward (e) ---

    #[test]
    fn e_basic() {
        let rope = Rope::from_str("hello world");
        assert_eq!(word_end_forward(&rope, 0), 4); // 'o' in 'hello'
    }

    #[test]
    fn e_from_end_of_word() {
        let rope = Rope::from_str("hello world");
        assert_eq!(word_end_forward(&rope, 4), 10); // skip to end of 'world'
    }

    #[test]
    fn e_at_end() {
        let rope = Rope::from_str("hi");
        assert_eq!(word_end_forward(&rope, 1), 1);
    }

    // --- big word motions (W/B/E) ---

    #[test]
    fn big_w_skips_punct() {
        let rope = Rope::from_str("hello.world next");
        assert_eq!(big_word_start_forward(&rope, 0), 12); // skips 'hello.world' + space → 'next'
    }

    #[test]
    fn big_b_skips_punct() {
        let rope = Rope::from_str("hello.world next");
        assert_eq!(big_word_start_backward(&rope, 12), 0); // from 'n' back to 'h'
    }

    #[test]
    fn big_e_skips_punct() {
        let rope = Rope::from_str("hello.world next");
        assert_eq!(big_word_end_forward(&rope, 0), 10); // end of 'hello.world'
    }

    // --- find_char_forward (f) ---

    #[test]
    fn f_finds_char() {
        let rope = Rope::from_str("hello world\n");
        assert_eq!(find_char_forward(&rope, 0, 0, 'o'), Some(4));
    }

    #[test]
    fn f_not_found() {
        let rope = Rope::from_str("hello\n");
        assert_eq!(find_char_forward(&rope, 0, 0, 'z'), None);
    }

    #[test]
    fn f_does_not_cross_line() {
        let rope = Rope::from_str("hello\nworld\n");
        assert_eq!(find_char_forward(&rope, 0, 0, 'w'), None);
    }

    // --- find_char_backward (F) ---

    #[test]
    fn big_f_finds_char() {
        let rope = Rope::from_str("hello world\n");
        assert_eq!(find_char_backward(&rope, 0, 10, 'o'), Some(7));
    }

    #[test]
    fn big_f_not_found() {
        let rope = Rope::from_str("hello\n");
        assert_eq!(find_char_backward(&rope, 0, 4, 'z'), None);
    }

    // --- find_char_forward_till (t) ---

    #[test]
    fn t_stops_before() {
        let rope = Rope::from_str("hello world\n");
        assert_eq!(find_char_forward_till(&rope, 0, 0, 'o'), Some(3));
    }

    // --- find_char_backward_till (T) ---

    #[test]
    fn big_t_stops_after() {
        let rope = Rope::from_str("hello world\n");
        assert_eq!(find_char_backward_till(&rope, 0, 10, 'o'), Some(8));
    }

    // --- matching_bracket (%) ---

    #[test]
    fn bracket_match_parens() {
        let rope = Rope::from_str("(hello)");
        assert_eq!(matching_bracket(&rope, 0), Some(6));
        assert_eq!(matching_bracket(&rope, 6), Some(0));
    }

    #[test]
    fn bracket_match_nested() {
        let rope = Rope::from_str("((a)(b))");
        assert_eq!(matching_bracket(&rope, 0), Some(7));
        assert_eq!(matching_bracket(&rope, 1), Some(3));
        assert_eq!(matching_bracket(&rope, 4), Some(6));
    }

    #[test]
    fn bracket_match_braces() {
        let rope = Rope::from_str("{[()]}");
        assert_eq!(matching_bracket(&rope, 0), Some(5));
        assert_eq!(matching_bracket(&rope, 1), Some(4));
        assert_eq!(matching_bracket(&rope, 2), Some(3));
    }

    #[test]
    fn bracket_no_match() {
        let rope = Rope::from_str("(unclosed");
        assert_eq!(matching_bracket(&rope, 0), None);
    }

    #[test]
    fn bracket_non_bracket_char() {
        let rope = Rope::from_str("hello");
        assert_eq!(matching_bracket(&rope, 0), None);
    }

    // --- paragraph motions ---

    #[test]
    fn paragraph_forward_basic() {
        let rope = Rope::from_str("aaa\nbbb\n\nccc\nddd\n");
        // From line 0, should land on the blank line (line 2)
        assert_eq!(paragraph_forward(&rope, 0), 2);
    }

    #[test]
    fn paragraph_backward_basic() {
        let rope = Rope::from_str("aaa\nbbb\n\nccc\nddd\n");
        // From line 3 ('ccc'), should land on blank line (line 2)
        assert_eq!(paragraph_backward(&rope, 3), 2);
    }

    #[test]
    fn paragraph_forward_from_blank() {
        let rope = Rope::from_str("aaa\n\nbbb\n\nccc\n");
        // From blank line 1, skip blanks, skip text, find next blank
        let result = paragraph_forward(&rope, 1);
        assert_eq!(result, 3); // next blank line
    }

    #[test]
    fn paragraph_backward_at_start() {
        let rope = Rope::from_str("aaa\nbbb\n");
        assert_eq!(paragraph_backward(&rope, 0), 0);
    }
}
