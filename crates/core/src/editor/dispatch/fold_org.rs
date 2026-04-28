use super::super::Editor;

impl Editor {
    /// Dispatch folding and org-mode commands. Returns `Some(true)` if handled.
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
            _ => return None,
        }
        Some(true)
    }
}
