use super::super::Editor;

impl Editor {
    /// Dispatch git-related commands. Returns `Some(true)` if handled.
    pub(super) fn dispatch_git(&mut self, name: &str) -> Option<bool> {
        match name {
            "git-status" => {
                self.git_status();
            }
            "git-stage" => {
                let win = self.window_mgr.focused_window();
                let idx = self.active_buffer_idx();
                let path = if let Some(view) = self.buffers[idx].git_status_view() {
                    if let Some(line) = view.lines.get(win.cursor_row) {
                        line.file_path.clone()
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(p) = path {
                    self.git_stage_file(&p);
                }
            }
            "git-unstage" => {
                let win = self.window_mgr.focused_window();
                let idx = self.active_buffer_idx();
                let path = if let Some(view) = self.buffers[idx].git_status_view() {
                    if let Some(line) = view.lines.get(win.cursor_row) {
                        line.file_path.clone()
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(p) = path {
                    self.git_unstage_file(&p);
                }
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
            "git-toggle-section" => {
                self.git_toggle_section();
            }
            "git-discard" => {
                self.git_discard_file();
            }
            "git-amend" => {
                self.git_amend();
            }
            "git-status-toggle" => {
                self.git_toggle_section();
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
            "git-blame" => self.git_blame(),
            "git-diff" => self.git_diff(),
            _ => return None,
        }
        Some(true)
    }
}
