//! Shared fold/collapse-state operations for MAE's magit-style buffers.
//!
//! `git_status.rs`, `notifications_view.rs`, and `kb_sharing.rs` are three
//! independent implementations of the same shape: a flat `Vec<Line>` of
//! semantic lines, a `HashMap<CollapseKey, bool>` tracking which sections are
//! folded, `toggle`/`is_collapsed`/`line_at` accessors, and a build function
//! that appends `(text, Line)` pairs to a rope-text accumulator while
//! preserving fold state from the buffer's previous view. Each view's own
//! `LineKind`/`CollapseKey` enums and its domain-specific build logic (git
//! diff parsing, notification categories, KB member/role data) are genuinely
//! per-view content, not boilerplate — only the fold/collapse/line-lookup
//! operations below are identical across all three, so they live here once
//! and each view delegates to them instead of re-implementing.
//!
//! A new foldable buffer kind should call these functions rather than
//! hand-rolling a fourth copy of this pattern.

use std::collections::HashMap;
use std::hash::Hash;

/// Toggle collapse state for `key` (default state is "not collapsed").
pub fn toggle<K: Eq + Hash>(collapsed: &mut HashMap<K, bool>, key: K) {
    let entry = collapsed.entry(key).or_insert(false);
    *entry = !*entry;
}

/// Check whether `key` is currently collapsed (default `false`).
pub fn is_collapsed<K: Eq + Hash>(collapsed: &HashMap<K, bool>, key: &K) -> bool {
    collapsed.get(key).copied().unwrap_or(false)
}

/// Look up the line at a given buffer row.
pub fn line_at<L>(lines: &[L], row: usize) -> Option<&L> {
    lines.get(row)
}

/// Append `line`'s display text to the rope-text accumulator and push `line`
/// itself onto `lines` — the "push a line into the view + text" idiom
/// repeated identically across all three build functions.
pub fn push_line<L>(rope_text: &mut String, text: &str, lines: &mut Vec<L>, line: L) {
    rope_text.push_str(text);
    rope_text.push('\n');
    lines.push(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct Key(u32);

    #[test]
    fn toggle_defaults_to_expanded_then_flips() {
        let mut collapsed = HashMap::new();
        let key = Key(1);
        assert!(!is_collapsed(&collapsed, &key));
        toggle(&mut collapsed, key);
        assert!(is_collapsed(&collapsed, &key));
        toggle(&mut collapsed, key);
        assert!(!is_collapsed(&collapsed, &key));
    }

    #[test]
    fn is_collapsed_unknown_key_defaults_false() {
        let collapsed: HashMap<Key, bool> = HashMap::new();
        assert!(!is_collapsed(&collapsed, &Key(99)));
    }

    #[test]
    fn line_at_in_and_out_of_bounds() {
        let lines = vec!["a", "b", "c"];
        assert_eq!(line_at(&lines, 1), Some(&"b"));
        assert_eq!(line_at(&lines, 5), None);
    }

    #[test]
    fn push_line_appends_text_and_line() {
        let mut rope = String::new();
        let mut lines: Vec<u32> = Vec::new();
        push_line(&mut rope, "first", &mut lines, 1);
        push_line(&mut rope, "second", &mut lines, 2);
        assert_eq!(rope, "first\nsecond\n");
        assert_eq!(lines, vec![1, 2]);
    }
}
