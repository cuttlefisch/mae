//! Markdown-specific heading operations.

use super::Editor;

impl Editor {
    /// Three-state heading cycle for markdown buffers (TAB).
    pub fn md_cycle(&mut self) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Markdown) {
            return;
        }
        // Tab on a table line → cell navigation instead of heading fold.
        let row = self.window_mgr.focused_window().cursor_row;
        if crate::table::table_at_line(self.buffers[buf_idx].rope(), row).is_some() {
            self.table_next_cell();
            return;
        }
        self.heading_cycle(crate::syntax::Language::Markdown);
    }

    /// Promote markdown heading (thin wrapper).
    pub fn md_promote(&mut self) {
        self.heading_promote(crate::syntax::Language::Markdown);
    }

    /// Demote markdown heading (thin wrapper).
    pub fn md_demote(&mut self) {
        self.heading_demote(crate::syntax::Language::Markdown);
    }

    /// Move markdown subtree down (thin wrapper).
    pub fn md_move_subtree_down(&mut self) {
        self.heading_move_subtree_down(crate::syntax::Language::Markdown);
    }

    /// Move markdown subtree up (thin wrapper).
    pub fn md_move_subtree_up(&mut self) {
        self.heading_move_subtree_up(crate::syntax::Language::Markdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    fn md_editor(text: &str) -> Editor {
        let mut ed = Editor::new();
        ed.buffers[0].insert_text_at(0, text);
        ed.syntax.set_language(0, Language::Markdown);
        ed
    }

    #[test]
    fn heading_level_markdown() {
        assert_eq!(Editor::heading_level("# H1", Language::Markdown), 1);
        assert_eq!(Editor::heading_level("## H2", Language::Markdown), 2);
        assert_eq!(Editor::heading_level("### H3", Language::Markdown), 3);
        assert_eq!(
            Editor::heading_level("Not a heading", Language::Markdown),
            0
        );
        assert_eq!(Editor::heading_level("##nospace", Language::Markdown), 0);
    }

    #[test]
    fn md_promote_removes_hash() {
        let mut ed = md_editor("## Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.md_promote();
        assert_eq!(ed.buffers[0].text(), "# Heading\nBody\n");
    }

    #[test]
    fn md_demote_adds_hash() {
        let mut ed = md_editor("# Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.md_demote();
        assert_eq!(ed.buffers[0].text(), "## Heading\nBody\n");
    }

    #[test]
    fn md_subtree_range() {
        let ed = md_editor("# H1\nBody\n## Sub\nSub body\n# H2\n");
        let range = ed.heading_subtree_range(0, Language::Markdown);
        assert_eq!(range, Some((0, 4)));
        let range = ed.heading_subtree_range(2, Language::Markdown);
        assert_eq!(range, Some((2, 4)));
    }

    #[test]
    fn md_cycle_three_state() {
        let mut ed = md_editor("# H1\nBody\n## Sub\nSub body\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        // SUBTREE → FOLDED
        ed.md_cycle();
        assert!(ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0));
        // FOLDED → CHILDREN
        ed.md_cycle();
        assert!(!ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0));
        assert!(ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 2));
        // CHILDREN → SUBTREE
        ed.md_cycle();
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn md_move_subtree_down() {
        let mut ed = md_editor("# H1\nBody1\n# H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.md_move_subtree_down();
        assert_eq!(ed.buffers[0].text(), "# H2\nBody2\n# H1\nBody1\n");
    }

    #[test]
    fn md_close_all_folds() {
        let mut ed = md_editor("# H1\nBody1\n## H2\nBody2\n");
        ed.close_all_folds();
        assert!(!ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn md_open_all_folds() {
        let mut ed = md_editor("# H1\nBody1\n## H2\nBody2\n");
        ed.close_all_folds();
        ed.open_all_folds();
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }
}
