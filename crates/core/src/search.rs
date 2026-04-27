use regex::Regex;
use ropey::Rope;

use crate::word;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchDirection {
    #[default]
    Forward,
    Backward,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchMatch {
    pub start: usize, // char offset (inclusive)
    pub end: usize,   // char offset (exclusive)
}

#[derive(Debug, Default)]
pub struct SearchState {
    pub pattern: String,
    pub regex: Option<Regex>,
    pub direction: SearchDirection,
    pub matches: Vec<SearchMatch>,
    pub highlight_active: bool,
}

pub struct SubstituteCommand {
    pub whole_buffer: bool,
    pub pattern: String,
    pub replacement: String,
    pub global: bool,
}

/// Maximum number of search matches to collect. Prevents OOM and freezes
/// on patterns that match nearly every character in large buffers.
const MAX_SEARCH_MATCHES: usize = 10_000;

/// Find all matches in the rope text. Returns char-offset ranges.
/// Capped at [`MAX_SEARCH_MATCHES`] to prevent freezes on large buffers.
pub fn find_all(rope: &Rope, regex: &Regex) -> Vec<SearchMatch> {
    // For very large buffers (>5 MB), use a line-by-line approach to avoid
    // materializing the entire rope into one contiguous String.
    let total_chars = rope.len_chars();
    if total_chars > 500_000 {
        return find_all_chunked(rope, regex);
    }

    let text: String = rope.chars().collect();
    let mut matches = Vec::new();
    // Build a byte-offset → char-offset lookup in O(n) instead of O(n) per match.
    let byte_to_char = build_byte_to_char_map(&text);
    for m in regex.find_iter(&text) {
        matches.push(SearchMatch {
            start: byte_to_char[m.start()],
            end: byte_to_char[m.end()],
        });
        if matches.len() >= MAX_SEARCH_MATCHES {
            break;
        }
    }
    matches
}

/// Build a byte-offset → char-offset lookup table in O(n).
fn build_byte_to_char_map(text: &str) -> Vec<usize> {
    let mut map = Vec::with_capacity(text.len() + 1);
    let mut char_idx = 0usize;
    for (byte_idx, ch) in text.char_indices() {
        while map.len() <= byte_idx {
            map.push(char_idx);
        }
        char_idx += 1;
        let next_byte = byte_idx + ch.len_utf8();
        while map.len() < next_byte {
            map.push(char_idx);
        }
    }
    while map.len() <= text.len() {
        map.push(char_idx);
    }
    map
}

/// Line-by-line search for large buffers. Avoids allocating the entire rope
/// as a contiguous String. Capped at MAX_SEARCH_MATCHES.
fn find_all_chunked(rope: &Rope, regex: &Regex) -> Vec<SearchMatch> {
    let mut matches = Vec::new();
    let mut char_offset = 0usize;
    for line_idx in 0..rope.len_lines() {
        let line = rope.line(line_idx);
        let line_str: String = line.chars().collect();
        let byte_to_char = build_byte_to_char_map(&line_str);
        for m in regex.find_iter(&line_str) {
            matches.push(SearchMatch {
                start: char_offset + byte_to_char[m.start()],
                end: char_offset + byte_to_char[m.end()],
            });
            if matches.len() >= MAX_SEARCH_MATCHES {
                return matches;
            }
        }
        char_offset += line.len_chars();
    }
    matches
}

/// Find the match containing `char_offset` if any, otherwise the nearest
/// match in `direction`. Wraps around the buffer when exhausted.
///
/// This is the `gn` / `gN` primitive from Practical Vim tip 86: the match
/// "at or after/before" the cursor, so pressing `gn` while already inside
/// a match selects that match instead of skipping past it.
pub fn find_match_at_or_adjacent(
    rope: &Rope,
    regex: &Regex,
    char_offset: usize,
    direction: SearchDirection,
) -> Option<SearchMatch> {
    let matches = find_all(rope, regex);
    if matches.is_empty() {
        return None;
    }
    match direction {
        SearchDirection::Forward => matches
            .iter()
            .find(|m| m.end > char_offset)
            .copied()
            .or_else(|| matches.first().copied()),
        SearchDirection::Backward => matches
            .iter()
            .rev()
            .find(|m| m.start <= char_offset)
            .copied()
            .or_else(|| matches.last().copied()),
    }
}

/// Find the next match after char_offset in the given direction. Wraps if requested.
pub fn find_next(
    rope: &Rope,
    regex: &Regex,
    char_offset: usize,
    direction: SearchDirection,
    wrap: bool,
) -> Option<SearchMatch> {
    let matches = find_all(rope, regex);
    if matches.is_empty() {
        return None;
    }

    match direction {
        SearchDirection::Forward => {
            // Find first match with start > char_offset
            if let Some(m) = matches.iter().find(|m| m.start > char_offset) {
                return Some(*m);
            }
            if wrap {
                return Some(matches[0]);
            }
            None
        }
        SearchDirection::Backward => {
            // Find last match with start < char_offset
            if let Some(m) = matches.iter().rev().find(|m| m.start < char_offset) {
                return Some(*m);
            }
            if wrap {
                return matches.last().copied();
            }
            None
        }
    }
}

/// Parse ":s/old/new/g" or ":%s/old/new/g".
pub fn parse_substitute(cmd: &str) -> Result<SubstituteCommand, String> {
    let cmd = cmd.trim();
    let (whole_buffer, rest) = if let Some(stripped) = cmd.strip_prefix('%') {
        (true, stripped)
    } else {
        (false, cmd)
    };

    let rest = rest.strip_prefix("s/").ok_or("Expected s/ prefix")?;

    // Find the closing delimiters — handle escaped slashes
    let parts = split_substitute_parts(rest)?;
    if parts.len() < 2 {
        return Err("Expected s/pattern/replacement/[flags]".to_string());
    }

    let pattern = &parts[0];
    let replacement = &parts[1];
    let flags = if parts.len() > 2 { &parts[2] } else { "" };

    if pattern.is_empty() {
        return Err("Empty search pattern".to_string());
    }

    Ok(SubstituteCommand {
        whole_buffer,
        pattern: pattern.to_string(),
        replacement: replacement.to_string(),
        global: flags.contains('g'),
    })
}

/// Split "pattern/replacement/flags" on unescaped `/`.
fn split_substitute_parts(s: &str) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(&next) = chars.peek() {
                current.push(ch);
                current.push(next);
                chars.next();
            }
        } else if ch == '/' {
            parts.push(current.clone());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    // Remaining text is the last part (flags or empty)
    parts.push(current);
    Ok(parts)
}

/// Substitute in a single line string. Returns (new_text, count).
pub fn substitute_line(
    line: &str,
    regex: &Regex,
    replacement: &str,
    global: bool,
) -> (String, usize) {
    if global {
        let mut count = 0;
        let result = regex
            .replace_all(line, |_caps: &regex::Captures| {
                count += 1;
                replacement.to_string()
            })
            .into_owned();
        (result, count)
    } else {
        if regex.is_match(line) {
            let result = regex.replace(line, replacement).into_owned();
            (result, 1)
        } else {
            (line.to_string(), 0)
        }
    }
}

/// Extract word under cursor (for * command). Returns \bword\b pattern.
pub fn word_at_offset(rope: &Rope, char_offset: usize) -> Option<String> {
    let len = rope.len_chars();
    if len == 0 || char_offset >= len {
        return None;
    }

    let ch = rope.char(char_offset);
    if word::classify(ch) != word::CharClass::Word {
        return None;
    }

    // Find start of word
    let mut start = char_offset;
    while start > 0 && word::classify(rope.char(start - 1)) == word::CharClass::Word {
        start -= 1;
    }

    // Find end of word
    let mut end = char_offset + 1;
    while end < len && word::classify(rope.char(end)) == word::CharClass::Word {
        end += 1;
    }

    let word: String = rope.chars_at(start).take(end - start).collect();
    Some(format!(r"\b{}\b", regex::escape(&word)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- find_all ---

    #[test]
    fn find_all_simple() {
        let rope = Rope::from_str("hello world hello");
        let re = Regex::new("hello").unwrap();
        let matches = find_all(&rope, &re);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0], SearchMatch { start: 0, end: 5 });
        assert_eq!(matches[1], SearchMatch { start: 12, end: 17 });
    }

    #[test]
    fn find_all_regex() {
        let rope = Rope::from_str("abc 123 def 456");
        let re = Regex::new(r"\d+").unwrap();
        let matches = find_all(&rope, &re);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0], SearchMatch { start: 4, end: 7 });
        assert_eq!(matches[1], SearchMatch { start: 12, end: 15 });
    }

    #[test]
    fn find_all_no_match() {
        let rope = Rope::from_str("hello");
        let re = Regex::new("xyz").unwrap();
        let matches = find_all(&rope, &re);
        assert!(matches.is_empty());
    }

    #[test]
    fn find_all_empty_rope() {
        let rope = Rope::from_str("");
        let re = Regex::new("anything").unwrap();
        let matches = find_all(&rope, &re);
        assert!(matches.is_empty());
    }

    // --- find_next ---

    #[test]
    fn find_next_forward() {
        let rope = Rope::from_str("aa bb aa");
        let re = Regex::new("aa").unwrap();
        let m = find_next(&rope, &re, 0, SearchDirection::Forward, false);
        assert_eq!(m, Some(SearchMatch { start: 6, end: 8 }));
    }

    #[test]
    fn find_next_forward_from_before() {
        let rope = Rope::from_str("hello world hello");
        let re = Regex::new("hello").unwrap();
        // From pos 0, next forward should skip to second (start > 0 means start=12)
        // Actually first match start=0 is not > 0, so it skips it. Next is start=12.
        let m = find_next(&rope, &re, 0, SearchDirection::Forward, false);
        assert_eq!(m, Some(SearchMatch { start: 12, end: 17 }));
    }

    #[test]
    fn find_next_forward_skips() {
        let rope = Rope::from_str("aa bb aa bb aa");
        let re = Regex::new("aa").unwrap();
        let m = find_next(&rope, &re, 6, SearchDirection::Forward, false);
        assert_eq!(m, Some(SearchMatch { start: 12, end: 14 }));
    }

    #[test]
    fn find_next_forward_wraps() {
        let rope = Rope::from_str("aa bb aa");
        let re = Regex::new("aa").unwrap();
        // From pos 7 (past last match), wrap should return first
        let m = find_next(&rope, &re, 7, SearchDirection::Forward, true);
        assert_eq!(m, Some(SearchMatch { start: 0, end: 2 }));
    }

    #[test]
    fn find_next_backward() {
        let rope = Rope::from_str("aa bb aa");
        let re = Regex::new("aa").unwrap();
        let m = find_next(&rope, &re, 7, SearchDirection::Backward, false);
        assert_eq!(m, Some(SearchMatch { start: 6, end: 8 }));
    }

    #[test]
    fn find_next_backward_wraps() {
        let rope = Rope::from_str("aa bb aa");
        let re = Regex::new("aa").unwrap();
        // From pos 0, backward should wrap to last match
        let m = find_next(&rope, &re, 0, SearchDirection::Backward, true);
        assert_eq!(m, Some(SearchMatch { start: 6, end: 8 }));
    }

    #[test]
    fn find_next_no_match() {
        let rope = Rope::from_str("hello world");
        let re = Regex::new("xyz").unwrap();
        let m = find_next(&rope, &re, 0, SearchDirection::Forward, true);
        assert!(m.is_none());
    }

    // --- find_match_at_or_adjacent (gn / gN primitive) ---

    #[test]
    fn at_or_adjacent_forward_inside_match() {
        // Cursor inside a match should select that match (Practical Vim tip 86)
        let rope = Rope::from_str("hello world hello");
        let re = Regex::new("hello").unwrap();
        // cursor at 'e' of first hello (offset 1)
        let m = find_match_at_or_adjacent(&rope, &re, 1, SearchDirection::Forward);
        assert_eq!(m, Some(SearchMatch { start: 0, end: 5 }));
    }

    #[test]
    fn at_or_adjacent_forward_after_match() {
        let rope = Rope::from_str("hello world hello");
        let re = Regex::new("hello").unwrap();
        // cursor past first match; should get second
        let m = find_match_at_or_adjacent(&rope, &re, 6, SearchDirection::Forward);
        assert_eq!(m, Some(SearchMatch { start: 12, end: 17 }));
    }

    #[test]
    fn at_or_adjacent_forward_wraps() {
        let rope = Rope::from_str("hello world");
        let re = Regex::new("hello").unwrap();
        // cursor past only match — wrap back to it
        let m = find_match_at_or_adjacent(&rope, &re, 10, SearchDirection::Forward);
        assert_eq!(m, Some(SearchMatch { start: 0, end: 5 }));
    }

    #[test]
    fn at_or_adjacent_backward_inside_match() {
        let rope = Rope::from_str("hello world hello");
        let re = Regex::new("hello").unwrap();
        // cursor inside second match
        let m = find_match_at_or_adjacent(&rope, &re, 14, SearchDirection::Backward);
        assert_eq!(m, Some(SearchMatch { start: 12, end: 17 }));
    }

    #[test]
    fn at_or_adjacent_backward_before_match() {
        let rope = Rope::from_str("hello world hello");
        let re = Regex::new("hello").unwrap();
        // cursor before second match; backward should give first
        let m = find_match_at_or_adjacent(&rope, &re, 10, SearchDirection::Backward);
        assert_eq!(m, Some(SearchMatch { start: 0, end: 5 }));
    }

    #[test]
    fn at_or_adjacent_no_match() {
        let rope = Rope::from_str("nothing here");
        let re = Regex::new("xyz").unwrap();
        let m = find_match_at_or_adjacent(&rope, &re, 0, SearchDirection::Forward);
        assert!(m.is_none());
    }

    // --- parse_substitute ---

    #[test]
    fn parse_sub_basic() {
        let cmd = parse_substitute("s/foo/bar/").unwrap();
        assert_eq!(cmd.pattern, "foo");
        assert_eq!(cmd.replacement, "bar");
        assert!(!cmd.global);
        assert!(!cmd.whole_buffer);
    }

    #[test]
    fn parse_sub_global() {
        let cmd = parse_substitute("s/foo/bar/g").unwrap();
        assert!(cmd.global);
    }

    #[test]
    fn parse_sub_whole_buffer() {
        let cmd = parse_substitute("%s/foo/bar/g").unwrap();
        assert!(cmd.whole_buffer);
        assert!(cmd.global);
    }

    #[test]
    fn parse_sub_empty_err() {
        let result = parse_substitute("s//bar/");
        assert!(result.is_err());
    }

    #[test]
    fn parse_sub_regex() {
        let cmd = parse_substitute(r"s/\d+/NUM/g").unwrap();
        assert_eq!(cmd.pattern, r"\d+");
        assert_eq!(cmd.replacement, "NUM");
        assert!(cmd.global);
    }

    // --- substitute_line ---

    #[test]
    fn substitute_line_single() {
        let re = Regex::new("foo").unwrap();
        let (result, count) = substitute_line("foo bar foo", &re, "baz", false);
        assert_eq!(result, "baz bar foo");
        assert_eq!(count, 1);
    }

    #[test]
    fn substitute_line_global() {
        let re = Regex::new("foo").unwrap();
        let (result, count) = substitute_line("foo bar foo", &re, "baz", true);
        assert_eq!(result, "baz bar baz");
        assert_eq!(count, 2);
    }

    #[test]
    fn substitute_line_no_match() {
        let re = Regex::new("xyz").unwrap();
        let (result, count) = substitute_line("hello", &re, "abc", false);
        assert_eq!(result, "hello");
        assert_eq!(count, 0);
    }

    // --- word_at_offset ---

    #[test]
    fn word_at_offset_middle() {
        let rope = Rope::from_str("hello world");
        let pat = word_at_offset(&rope, 2).unwrap();
        assert_eq!(pat, r"\bhello\b");
    }

    #[test]
    fn word_at_offset_whitespace() {
        let rope = Rope::from_str("hello world");
        assert!(word_at_offset(&rope, 5).is_none());
    }

    #[test]
    fn word_at_offset_start() {
        let rope = Rope::from_str("hello world");
        let pat = word_at_offset(&rope, 0).unwrap();
        assert_eq!(pat, r"\bhello\b");
    }

    #[test]
    fn word_at_offset_end_of_word() {
        let rope = Rope::from_str("hello world");
        let pat = word_at_offset(&rope, 4).unwrap();
        assert_eq!(pat, r"\bhello\b");
    }
}
