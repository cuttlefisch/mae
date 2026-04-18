//! Debug panel view state — navigation and expansion state for the `*Debug*` buffer.
//!
//! Mirrors `help_view.rs`: the panel is a read-only buffer populated from
//! `DebugState`, with interactive navigation (j/k to move, Enter to
//! select/expand, q to close). `DebugView` tracks which line the cursor
//! is on, which variables are expanded, and lazy-loaded child variables.

use std::collections::{HashMap, HashSet};

use crate::debug::Variable;

/// What semantic item a rendered line in the debug buffer represents.
/// The renderer and key handler use this to style and interact with lines.
#[derive(Debug, Clone, PartialEq)]
pub enum DebugLineItem {
    /// Section header (e.g. "Threads", "Call Stack", "Locals").
    SectionHeader(String),
    /// A thread entry. Carries the thread id.
    Thread(i64),
    /// A stack frame entry. Carries the frame id.
    Frame(i64),
    /// A variable entry within a scope.
    Variable {
        scope: String,
        name: String,
        depth: usize,
        variables_reference: i64,
    },
    /// A line from the output log.
    OutputLine(usize),
    /// Blank separator line.
    Blank,
}

/// View state for the `*Debug*` buffer.
#[derive(Debug, Clone)]
pub struct DebugView {
    /// Index of the currently selected line in the buffer.
    pub cursor_index: usize,
    /// Which stack frame is focused (for source navigation).
    pub selected_frame_id: Option<i64>,
    /// Set of `variables_reference` values that are expanded in the tree.
    pub expanded_vars: HashSet<i64>,
    /// Lazy-loaded child variables keyed by parent's `variables_reference`.
    pub child_variables: HashMap<i64, Vec<Variable>>,
    /// Maps buffer line index → semantic item. Rebuilt on each populate.
    pub line_map: Vec<DebugLineItem>,
    /// When true, show the output log instead of the state view.
    pub show_output: bool,
}

impl DebugView {
    pub fn new() -> Self {
        DebugView {
            cursor_index: 0,
            selected_frame_id: None,
            expanded_vars: HashSet::new(),
            child_variables: HashMap::new(),
            line_map: Vec::new(),
            show_output: false,
        }
    }

    /// Toggle expansion of a variable by its `variables_reference`.
    /// Returns true if the variable is now expanded (i.e. was collapsed).
    pub fn toggle_expand(&mut self, var_ref: i64) -> bool {
        if self.expanded_vars.contains(&var_ref) {
            self.expanded_vars.remove(&var_ref);
            false
        } else {
            self.expanded_vars.insert(var_ref);
            true
        }
    }

    /// Whether a variable is currently expanded.
    pub fn is_expanded(&self, var_ref: i64) -> bool {
        self.expanded_vars.contains(&var_ref)
    }

    /// Get the line item under the cursor, if any.
    pub fn cursor_item(&self) -> Option<&DebugLineItem> {
        self.line_map.get(self.cursor_index)
    }

    /// Move cursor to the next interactive line (skipping headers/blanks).
    pub fn move_down(&mut self) {
        let len = self.line_map.len();
        if len == 0 {
            return;
        }
        let mut next = self.cursor_index + 1;
        while next < len {
            if self.is_interactive(next) {
                self.cursor_index = next;
                return;
            }
            next += 1;
        }
        // If no interactive line found below, stay put.
    }

    /// Move cursor to the previous interactive line (skipping headers/blanks).
    pub fn move_up(&mut self) {
        if self.cursor_index == 0 || self.line_map.is_empty() {
            return;
        }
        let mut prev = self.cursor_index - 1;
        loop {
            if self.is_interactive(prev) {
                self.cursor_index = prev;
                return;
            }
            if prev == 0 {
                break;
            }
            prev -= 1;
        }
    }

    /// Clamp cursor to valid range after line_map rebuild.
    pub fn clamp_cursor(&mut self) {
        if self.line_map.is_empty() {
            self.cursor_index = 0;
        } else if self.cursor_index >= self.line_map.len() {
            self.cursor_index = self.line_map.len() - 1;
        }
    }

    /// Whether the line at `idx` is interactive (not a header or blank).
    fn is_interactive(&self, idx: usize) -> bool {
        matches!(
            self.line_map.get(idx),
            Some(
                DebugLineItem::Thread(_)
                    | DebugLineItem::Frame(_)
                    | DebugLineItem::Variable { .. }
                    | DebugLineItem::OutputLine(_)
            )
        )
    }
}

impl Default for DebugView {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_view_is_empty() {
        let v = DebugView::new();
        assert_eq!(v.cursor_index, 0);
        assert!(v.selected_frame_id.is_none());
        assert!(v.expanded_vars.is_empty());
        assert!(v.child_variables.is_empty());
        assert!(v.line_map.is_empty());
        assert!(!v.show_output);
    }

    #[test]
    fn toggle_expand() {
        let mut v = DebugView::new();
        assert!(v.toggle_expand(42));
        assert!(v.is_expanded(42));
        assert!(!v.toggle_expand(42));
        assert!(!v.is_expanded(42));
    }

    #[test]
    fn cursor_item_empty_map() {
        let v = DebugView::new();
        assert!(v.cursor_item().is_none());
    }

    #[test]
    fn cursor_item_returns_correct_item() {
        let mut v = DebugView::new();
        v.line_map = vec![
            DebugLineItem::SectionHeader("Threads".into()),
            DebugLineItem::Thread(1),
            DebugLineItem::Blank,
            DebugLineItem::Frame(100),
        ];
        v.cursor_index = 1;
        assert_eq!(v.cursor_item(), Some(&DebugLineItem::Thread(1)));
        v.cursor_index = 3;
        assert_eq!(v.cursor_item(), Some(&DebugLineItem::Frame(100)));
    }

    #[test]
    fn move_down_skips_non_interactive() {
        let mut v = DebugView::new();
        v.line_map = vec![
            DebugLineItem::Thread(1),
            DebugLineItem::SectionHeader("Call Stack".into()),
            DebugLineItem::Blank,
            DebugLineItem::Frame(100),
        ];
        v.cursor_index = 0;
        v.move_down();
        assert_eq!(v.cursor_index, 3); // Skipped header + blank
    }

    #[test]
    fn move_up_skips_non_interactive() {
        let mut v = DebugView::new();
        v.line_map = vec![
            DebugLineItem::Thread(1),
            DebugLineItem::SectionHeader("Call Stack".into()),
            DebugLineItem::Blank,
            DebugLineItem::Frame(100),
        ];
        v.cursor_index = 3;
        v.move_up();
        assert_eq!(v.cursor_index, 0); // Skipped blank + header
    }

    #[test]
    fn move_down_at_end_stays_put() {
        let mut v = DebugView::new();
        v.line_map = vec![DebugLineItem::Thread(1)];
        v.cursor_index = 0;
        v.move_down();
        assert_eq!(v.cursor_index, 0); // No more interactive lines below
    }

    #[test]
    fn move_up_at_start_stays_put() {
        let mut v = DebugView::new();
        v.line_map = vec![
            DebugLineItem::SectionHeader("X".into()),
            DebugLineItem::Thread(1),
        ];
        v.cursor_index = 1;
        v.move_up();
        assert_eq!(v.cursor_index, 1); // No interactive line above
    }

    #[test]
    fn clamp_cursor() {
        let mut v = DebugView::new();
        v.cursor_index = 10;
        v.clamp_cursor(); // empty map → 0
        assert_eq!(v.cursor_index, 0);

        v.line_map = vec![DebugLineItem::Thread(1), DebugLineItem::Frame(2)];
        v.cursor_index = 10;
        v.clamp_cursor();
        assert_eq!(v.cursor_index, 1);

        v.cursor_index = 0;
        v.clamp_cursor();
        assert_eq!(v.cursor_index, 0);
    }

    #[test]
    fn child_variables_storage() {
        let mut v = DebugView::new();
        v.child_variables.insert(
            42,
            vec![Variable {
                name: "x".into(),
                value: "1".into(),
                var_type: None,
                variables_reference: 0,
            }],
        );
        assert_eq!(v.child_variables.get(&42).unwrap().len(), 1);
    }
}
