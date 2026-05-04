use super::super::Editor;

impl Editor {
    /// Dispatch folding, org-mode, and markdown commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_fold_org(&mut self, name: &str) -> Option<bool> {
        match name {
            "toggle-fold" => {
                self.toggle_fold();
            }
            "close-all-folds" => {
                self.close_all_folds();
            }
            "open-all-folds" => {
                self.open_all_folds();
            }
            // Org commands
            "org-cycle" => {
                self.org_cycle();
            }
            "org-todo-next" | "org-todo-prev" => {
                self.org_todo_cycle();
            }
            "org-open-link" | "smart-enter" => {
                self.smart_enter();
            }
            "org-promote" => {
                self.org_promote();
            }
            "org-demote" => {
                self.org_demote();
            }
            "org-move-subtree-up" => {
                self.org_move_subtree_up();
            }
            "org-move-subtree-down" => {
                self.org_move_subtree_down();
            }
            // Markdown commands
            "md-cycle" => {
                self.md_cycle();
            }
            "md-promote" => {
                self.md_promote();
            }
            "md-demote" => {
                self.md_demote();
            }
            "md-move-subtree-up" => {
                self.md_move_subtree_up();
            }
            "md-move-subtree-down" => {
                self.md_move_subtree_down();
            }
            // Narrow/widen (shared)
            "narrow-to-subtree" | "org-narrow-subtree" | "md-narrow-subtree" => {
                self.narrow_to_subtree();
            }
            "widen" | "org-widen" | "md-widen" => {
                self.widen();
            }
            "org-insert-heading" => {
                self.insert_heading(crate::syntax::Language::Org);
            }
            "md-insert-heading" => {
                self.insert_heading(crate::syntax::Language::Markdown);
            }
            "org-global-cycle" => {
                self.heading_global_cycle(crate::syntax::Language::Org);
            }
            "md-global-cycle" => {
                self.heading_global_cycle(crate::syntax::Language::Markdown);
            }
            // Priority cycling
            "org-priority-up" => {
                self.org_priority_up();
            }
            "org-priority-down" => {
                self.org_priority_down();
            }
            // Tag editing
            "org-set-tags" => {
                self.org_set_tags();
            }
            // Smart newline (list continuation)
            "insert-newline-smart" => {
                if !self.insert_smart_newline() {
                    // Fall through to normal newline insertion
                    self.insert_at_cursor("\n");
                }
            }
            // Table commands
            "table-next-cell" => {
                self.table_next_cell();
            }
            "table-prev-cell" => {
                self.table_prev_cell();
            }
            "table-align" => {
                self.table_align();
            }
            "table-insert-row" => {
                self.table_insert_row();
            }
            "table-delete-row" => {
                self.table_delete_row();
            }
            "table-insert-column" => {
                self.table_insert_column();
            }
            "table-delete-column" => {
                self.table_delete_column();
            }
            "table-move-row-up" => {
                self.table_move_row_up();
            }
            "table-move-row-down" => {
                self.table_move_row_down();
            }
            // Display region link navigation
            "text-next-link" => {
                self.text_next_link();
            }
            "text-prev-link" => {
                self.text_prev_link();
            }
            _ => return None,
        }
        Some(true)
    }
}
