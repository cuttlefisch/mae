//! Shared hover popup content computation for both GUI and TUI renderers.

/// Compute word-wrapped lines from hover markdown content.
/// Splits on existing newlines, then wraps each line at `max_width`.
pub fn compute_hover_lines(contents: &str, max_width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for raw_line in contents.lines() {
        if raw_line.is_empty() {
            lines.push(String::new());
            continue;
        }
        // Simple word-wrap
        let mut current = String::new();
        for word in raw_line.split_whitespace() {
            if current.is_empty() {
                current = word.to_string();
            } else if current.len() + 1 + word.len() > max_width {
                lines.push(current);
                current = word.to_string();
            } else {
                current.push(' ');
                current.push_str(word);
            }
        }
        if !current.is_empty() {
            lines.push(current);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_short_line() {
        let lines = compute_hover_lines("hello world", 80);
        assert_eq!(lines, vec!["hello world"]);
    }

    #[test]
    fn wrap_long_line() {
        let lines = compute_hover_lines("the quick brown fox jumps over the lazy dog", 20);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].len() <= 20);
    }

    #[test]
    fn preserves_empty_lines() {
        let lines = compute_hover_lines("a\n\nb", 80);
        assert_eq!(lines, vec!["a", "", "b"]);
    }

    #[test]
    fn multiline_content() {
        let lines = compute_hover_lines("line1\nline2\nline3", 80);
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }
}
