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

/// Resolve the char-offset range for a text object defined by paired delimiters or quotes.
///
/// For nested delimiters `()[]{}`: searches backward for the opening delimiter
/// containing `pos`, then forward for its matching close. Handles nesting.
///
/// For quotes `"'``: finds the quote pair on the current line containing or
/// nearest to `pos`.
///
/// Returns `Some((start, end))` as a half-open range `[start, end)`.
/// - `inner = true`: contents between delimiters (exclusive of delimiters)
/// - `inner = false` (around): includes the delimiters themselves
pub fn text_object_range(
    rope: &Rope,
    pos: usize,
    obj: char,
    inner: bool,
) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    if len == 0 || pos >= len {
        return None;
    }

    match obj {
        '(' | ')' => paired_delimiter_range(rope, pos, '(', ')', inner),
        '[' | ']' => paired_delimiter_range(rope, pos, '[', ']', inner),
        '{' | '}' => paired_delimiter_range(rope, pos, '{', '}', inner),
        '<' | '>' => paired_delimiter_range(rope, pos, '<', '>', inner),
        '"' | '\'' | '`' => quote_range(rope, pos, obj, inner),
        _ => None,
    }
}

/// Find the range of a paired nesting delimiter containing `pos`.
fn paired_delimiter_range(
    rope: &Rope,
    pos: usize,
    open: char,
    close: char,
    inner: bool,
) -> Option<(usize, usize)> {
    let len = rope.len_chars();

    // If cursor is ON the close delimiter, start the backward search from
    // one position before it so we correctly find the matching open.
    let search_start = if rope.char(pos) == close && pos > 0 {
        pos - 1
    } else {
        pos
    };

    // Search backward for the nearest unmatched open delimiter
    let mut depth: i32 = 0;
    let mut found_open = None;

    let mut p = search_start;
    loop {
        let ch = rope.char(p);
        if ch == close {
            depth += 1;
        } else if ch == open {
            if depth == 0 {
                found_open = Some(p);
                break;
            }
            depth -= 1;
        }
        if p == 0 {
            break;
        }
        p -= 1;
    }

    let open_pos = found_open?;

    // Now find matching close from open_pos
    let mut depth: i32 = 1;
    let mut p = open_pos + 1;
    let mut found_close = None;
    while p < len {
        let ch = rope.char(p);
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                found_close = Some(p);
                break;
            }
        }
        p += 1;
    }

    let close_pos = found_close?;

    if inner {
        Some((open_pos + 1, close_pos))
    } else {
        Some((open_pos, close_pos + 1))
    }
}

/// Find the range of a quote pair on the current line containing `pos`.
fn quote_range(rope: &Rope, pos: usize, quote: char, inner: bool) -> Option<(usize, usize)> {
    let line_idx = rope.char_to_line(pos);
    let line_start = rope.line_to_char(line_idx);
    let line = rope.line(line_idx);
    let line_len = line.len_chars();
    let col = pos - line_start;

    // Collect all quote positions on this line (excluding trailing newline)
    let effective_len = if line_len > 0 && line.char(line_len - 1) == '\n' {
        line_len - 1
    } else {
        line_len
    };

    let mut quote_positions: Vec<usize> = Vec::new();
    for i in 0..effective_len {
        if line.char(i) == quote {
            quote_positions.push(i);
        }
    }

    // We need pairs. Find the pair that contains our cursor.
    // Iterate over consecutive pairs (0,1), (2,3), etc.
    let mut i = 0;
    while i + 1 < quote_positions.len() {
        let q_open = quote_positions[i];
        let q_close = quote_positions[i + 1];

        // Check if cursor is within (or on) this pair
        if col >= q_open && col <= q_close {
            let abs_open = line_start + q_open;
            let abs_close = line_start + q_close;
            return if inner {
                Some((abs_open + 1, abs_close))
            } else {
                Some((abs_open, abs_close + 1))
            };
        }
        i += 2;
    }

    None
}

/// Resolve the char-offset range for `iw` (inner word) or `aw` (around word).
///
/// - `inner = true` (`iw`): the word (or whitespace run) under the cursor.
/// - `inner = false` (`aw`): the word plus trailing whitespace, or leading
///   whitespace if there is no trailing whitespace.
///
/// Returns `Some((start, end))` as a half-open range.
pub fn word_object_range(rope: &Rope, pos: usize, inner: bool) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    if len == 0 || pos >= len {
        return None;
    }

    let cls = classify(rope.char(pos));

    // Find extent of same-class chars around pos
    let mut start = pos;
    while start > 0 && classify(rope.char(start - 1)) == cls {
        start -= 1;
    }
    let mut end = pos + 1;
    while end < len && classify(rope.char(end)) == cls {
        end += 1;
    }

    if !inner {
        // "around" — include trailing whitespace, or leading if no trailing
        let orig_end = end;
        while end < len && classify(rope.char(end)) == CharClass::Whitespace {
            end += 1;
        }
        if end == orig_end {
            // No trailing whitespace — try leading
            while start > 0 && classify(rope.char(start - 1)) == CharClass::Whitespace {
                start -= 1;
            }
        }
    }

    Some((start, end))
}

/// Resolve the char-offset range for `iW` (inner WORD) or `aW` (around WORD).
///
/// WORD is whitespace-delimited (anything that's not whitespace/newline).
pub fn big_word_object_range(rope: &Rope, pos: usize, inner: bool) -> Option<(usize, usize)> {
    let len = rope.len_chars();
    if len == 0 || pos >= len {
        return None;
    }

    let ch = rope.char(pos);
    let on_ws = matches!(classify(ch), CharClass::Whitespace | CharClass::Newline);

    let mut start = pos;
    let mut end = pos + 1;

    if on_ws {
        // On whitespace: the "word" is the whitespace run
        while start > 0
            && matches!(
                classify(rope.char(start - 1)),
                CharClass::Whitespace | CharClass::Newline
            )
        {
            start -= 1;
        }
        while end < len
            && matches!(
                classify(rope.char(end)),
                CharClass::Whitespace | CharClass::Newline
            )
        {
            end += 1;
        }
    } else {
        // On non-whitespace: the WORD is the non-whitespace run
        while start > 0
            && !matches!(
                classify(rope.char(start - 1)),
                CharClass::Whitespace | CharClass::Newline
            )
        {
            start -= 1;
        }
        while end < len
            && !matches!(
                classify(rope.char(end)),
                CharClass::Whitespace | CharClass::Newline
            )
        {
            end += 1;
        }
    }

    if !inner {
        // "around" — include trailing whitespace, or leading if no trailing
        let orig_end = end;
        while end < len && matches!(classify(rope.char(end)), CharClass::Whitespace) {
            end += 1;
        }
        if end == orig_end && !on_ws {
            // No trailing whitespace — try leading
            while start > 0 && matches!(classify(rope.char(start - 1)), CharClass::Whitespace) {
                start -= 1;
            }
        }
    }

    Some((start, end))
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

    // --- text_object_range: paired delimiters ---

    #[test]
    fn text_object_inner_parens() {
        let rope = Rope::from_str("foo(bar)baz");
        // cursor on 'a' (pos 5) inside parens
        assert_eq!(text_object_range(&rope, 5, '(', true), Some((4, 7)));
    }

    #[test]
    fn text_object_around_parens() {
        let rope = Rope::from_str("foo(bar)baz");
        // cursor on 'a' (pos 5)
        assert_eq!(text_object_range(&rope, 5, '(', false), Some((3, 8)));
    }

    #[test]
    fn text_object_closing_paren_also_works() {
        let rope = Rope::from_str("foo(bar)baz");
        // Using ')' as the object char should work identically
        assert_eq!(text_object_range(&rope, 5, ')', true), Some((4, 7)));
    }

    #[test]
    fn text_object_nested_parens() {
        // "(a (b) c)" — cursor on 'b' (pos 4)
        let rope = Rope::from_str("(a (b) c)");
        // inner of the innermost pair containing 'b': (b) -> inner is just "b" -> (4, 5)
        assert_eq!(text_object_range(&rope, 4, '(', true), Some((4, 5)));
    }

    #[test]
    fn text_object_nested_parens_outer() {
        // "(a (b) c)" — cursor on 'a' (pos 1)
        let rope = Rope::from_str("(a (b) c)");
        // inner of the outer pair
        assert_eq!(text_object_range(&rope, 1, '(', true), Some((1, 8)));
    }

    #[test]
    fn text_object_inner_braces() {
        let rope = Rope::from_str("fn() { body }");
        // cursor on 'b' (pos 7)
        assert_eq!(text_object_range(&rope, 7, '{', true), Some((6, 12)));
    }

    #[test]
    fn text_object_around_braces() {
        let rope = Rope::from_str("fn() { body }");
        assert_eq!(text_object_range(&rope, 7, '{', false), Some((5, 13)));
    }

    #[test]
    fn text_object_inner_brackets() {
        let rope = Rope::from_str("[a, b, c]");
        // cursor on 'b' (pos 4)
        assert_eq!(text_object_range(&rope, 4, '[', true), Some((1, 8)));
    }

    #[test]
    fn text_object_around_brackets() {
        let rope = Rope::from_str("[a, b, c]");
        assert_eq!(text_object_range(&rope, 4, ']', false), Some((0, 9)));
    }

    #[test]
    fn text_object_inner_angle_brackets() {
        let rope = Rope::from_str("Vec<String>");
        // cursor on 'S' (pos 4)
        assert_eq!(text_object_range(&rope, 4, '<', true), Some((4, 10)));
    }

    #[test]
    fn text_object_no_match_returns_none() {
        let rope = Rope::from_str("hello world");
        assert_eq!(text_object_range(&rope, 3, '(', true), None);
    }

    #[test]
    fn text_object_cursor_on_open_delim() {
        let rope = Rope::from_str("(abc)");
        // cursor on '(' itself
        assert_eq!(text_object_range(&rope, 0, '(', true), Some((1, 4)));
        assert_eq!(text_object_range(&rope, 0, '(', false), Some((0, 5)));
    }

    #[test]
    fn text_object_cursor_on_close_delim() {
        let rope = Rope::from_str("(abc)");
        // cursor on ')' — should still find the pair
        assert_eq!(text_object_range(&rope, 4, '(', true), Some((1, 4)));
    }

    // --- text_object_range: quotes ---

    #[test]
    fn text_object_inner_double_quotes() {
        let rope = Rope::from_str("he said \"hello\" there");
        // cursor on 'l' (pos 11) inside the quotes
        assert_eq!(text_object_range(&rope, 11, '"', true), Some((9, 14)));
    }

    #[test]
    fn text_object_around_double_quotes() {
        let rope = Rope::from_str("he said \"hello\" there");
        assert_eq!(text_object_range(&rope, 11, '"', false), Some((8, 15)));
    }

    #[test]
    fn text_object_inner_single_quotes() {
        let rope = Rope::from_str("say 'fine' here");
        // cursor on 'i' (pos 6) inside 'fine'
        assert_eq!(text_object_range(&rope, 6, '\'', true), Some((5, 9)));
    }

    #[test]
    fn text_object_inner_backticks() {
        let rope = Rope::from_str("use `code` here");
        // cursor on 'o' (pos 6)
        assert_eq!(text_object_range(&rope, 6, '`', true), Some((5, 9)));
    }

    #[test]
    fn text_object_quotes_no_match() {
        let rope = Rope::from_str("no quotes here");
        assert_eq!(text_object_range(&rope, 3, '"', true), None);
    }

    #[test]
    fn text_object_cursor_on_quote_char() {
        let rope = Rope::from_str("say \"hi\"");
        // cursor on opening quote (pos 4)
        assert_eq!(text_object_range(&rope, 4, '"', true), Some((5, 7)));
        assert_eq!(text_object_range(&rope, 4, '"', false), Some((4, 8)));
    }

    #[test]
    fn text_object_empty_rope() {
        let rope = Rope::from_str("");
        assert_eq!(text_object_range(&rope, 0, '(', true), None);
    }

    // --- word_object_range (iw/aw) ---

    #[test]
    fn word_object_inner_basic() {
        let rope = Rope::from_str("hello world");
        // cursor on 'e' (pos 1)
        assert_eq!(word_object_range(&rope, 1, true), Some((0, 5)));
    }

    #[test]
    fn word_object_around_basic() {
        let rope = Rope::from_str("hello world");
        // cursor on 'e' (pos 1) — word + trailing space
        assert_eq!(word_object_range(&rope, 1, false), Some((0, 6)));
    }

    #[test]
    fn word_object_around_last_word() {
        let rope = Rope::from_str("hello world");
        // cursor on 'o' (pos 8) in 'world' — no trailing space, so include leading
        assert_eq!(word_object_range(&rope, 8, false), Some((5, 11)));
    }

    #[test]
    fn word_object_inner_on_whitespace() {
        let rope = Rope::from_str("hello   world");
        // cursor on space (pos 6) — inner selects the whitespace run
        assert_eq!(word_object_range(&rope, 6, true), Some((5, 8)));
    }

    #[test]
    fn word_object_inner_punctuation() {
        let rope = Rope::from_str("foo.bar");
        // cursor on '.' (pos 3) — punct is its own class
        assert_eq!(word_object_range(&rope, 3, true), Some((3, 4)));
    }

    // --- big_word_object_range (iW/aW) ---

    #[test]
    fn big_word_object_inner() {
        let rope = Rope::from_str("foo.bar baz");
        // cursor on '.' (pos 3) — WORD includes foo.bar
        assert_eq!(big_word_object_range(&rope, 3, true), Some((0, 7)));
    }

    #[test]
    fn big_word_object_around() {
        let rope = Rope::from_str("foo.bar baz");
        // cursor on '.' (pos 3) — around includes trailing space
        assert_eq!(big_word_object_range(&rope, 3, false), Some((0, 8)));
    }

    #[test]
    fn big_word_object_around_last() {
        let rope = Rope::from_str("hello foo.bar");
        // cursor on '.' (pos 9) — no trailing ws, include leading
        assert_eq!(big_word_object_range(&rope, 9, false), Some((5, 13)));
    }

    #[test]
    fn big_word_object_inner_on_whitespace() {
        let rope = Rope::from_str("abc   def");
        // cursor on space (pos 4) — inner selects whitespace run
        assert_eq!(big_word_object_range(&rope, 4, true), Some((3, 6)));
    }
}
