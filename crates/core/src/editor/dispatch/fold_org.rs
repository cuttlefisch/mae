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
            "org-todo-next" => {
                self.org_todo_cycle(true);
            }
            "org-todo-prev" => {
                self.org_todo_cycle(false);
            }
            "org-open-link" => {
                self.org_open_link();
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
            _ => return None,
        }
        Some(true)
    }
}
