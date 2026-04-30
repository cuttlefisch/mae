use super::super::Editor;

impl Editor {
    /// Dispatch git-related commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_git(&mut self, name: &str) -> Option<bool> {
        // Guard: commands that require git-status buffer context
        let requires_git_status = matches!(
            name,
            "git-stage"
                | "git-unstage"
                | "git-toggle-fold"
                | "git-toggle-section"
                | "git-discard"
                | "git-next-hunk"
                | "git-prev-hunk"
                | "git-status-toggle"
                | "git-status-open"
                | "git-stash-pop"
                | "git-stash-apply"
                | "git-stash-drop"
        );
        if requires_git_status {
            let idx = self.active_buffer_idx();
            if self.buffers[idx].kind != crate::buffer::BufferKind::GitStatus {
                self.set_status("Requires git-status buffer");
                return Some(true);
            }
        }

        match name {
            "git-status" => {
                self.git_status();
            }
            "git-stage" => {
                self.git_stage_at_cursor();
            }
            "git-unstage" => {
                self.git_unstage_at_cursor();
            }
            "git-stage-all" => {
                self.git_stage_file(".");
            }
            "git-unstage-all" => {
                self.git_unstage_file(".");
            }
            "git-commit" => {
                self.git_commit();
            }
            "git-log" => {
                self.git_log();
            }
            "git-toggle-fold" | "git-toggle-section" | "git-status-toggle" => {
                self.git_toggle_fold();
            }
            "git-discard" => {
                self.git_discard_at_cursor();
            }
            "git-amend" => {
                self.git_amend();
            }
            "git-status-open" => {
                let win = self.window_mgr.focused_window();
                let idx = self.active_buffer_idx();
                let (path, repo_root) = if let Some(view) = self.buffers[idx].git_status_view() {
                    if let Some(line) = view.lines.get(win.cursor_row) {
                        (line.file_path.clone(), Some(view.repo_root.clone()))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };

                if let (Some(p), Some(root)) = (path, repo_root) {
                    let full_path = root.join(p);
                    self.open_file(full_path);
                }
            }
            "git-next-hunk" => self.git_next_hunk(),
            "git-prev-hunk" => self.git_prev_hunk(),
            "git-push" => self.git_push(),
            "git-pull" => self.git_pull(),
            "git-fetch" => self.git_fetch(),
            "git-branch-switch" => self.git_branch_switch_palette(),
            "git-branch-create" => {
                // Uses command-line input; prompt via status
                self.set_status("Use :git-branch-create <name>");
            }
            "git-branch-delete" => {
                self.set_status("Use :git-branch-delete <name>");
            }
            "git-stash-push" => self.git_stash_push(),
            "git-stash-pop" => self.git_stash_pop(),
            "git-stash-apply" => self.git_stash_apply(),
            "git-stash-drop" => self.git_stash_drop(),
            "git-blame" => self.git_blame(),
            "git-diff" => self.git_diff(),
            _ => return None,
        }
        Some(true)
    }
}
