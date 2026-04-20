//! Git integration commands (SPC g group).
//!
//! Shell-out stubs that capture git output into read-only scratch buffers.
//! Full integration deferred to Phase 6 (Embedded Shell + Magit Parity).

use crate::buffer::Buffer;

use super::Editor;

impl Editor {
    /// Run a git command and put output in a read-only scratch buffer.
    fn git_command_to_buffer(&mut self, args: &[&str], buf_name: &str) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        match std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
        {
            Ok(output) => {
                let text = if output.status.success() {
                    String::from_utf8_lossy(&output.stdout).to_string()
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    format!("git error: {}", stderr)
                };
                // Find or create the buffer
                let idx = if let Some(i) = self.find_buffer_by_name(buf_name) {
                    self.buffers[i] = Buffer::new();
                    self.buffers[i].name = buf_name.to_string();
                    i
                } else {
                    let mut buf = Buffer::new();
                    buf.name = buf_name.to_string();
                    self.buffers.push(buf);
                    self.buffers.len() - 1
                };
                // Insert content
                let win_temp = &mut crate::window::Window::new(0, idx);
                for ch in text.chars() {
                    self.buffers[idx].insert_char(win_temp, ch);
                }
                self.buffers[idx].modified = false;
                // Switch to it
                let prev = self.active_buffer_idx();
                self.alternate_buffer_idx = Some(prev);
                self.window_mgr.focused_window_mut().buffer_idx = idx;
                self.window_mgr.focused_window_mut().cursor_row = 0;
                self.window_mgr.focused_window_mut().cursor_col = 0;
            }
            Err(e) => {
                self.set_status(format!("git: {}", e));
            }
        }
    }

    /// Refresh the cached git branch by running `git rev-parse --abbrev-ref HEAD`.
    pub fn refresh_git_branch(&mut self) {
        let dir = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok());
        self.git_branch = dir.and_then(|d| {
            std::process::Command::new("git")
                .args(["rev-parse", "--abbrev-ref", "HEAD"])
                .current_dir(&d)
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
                    } else {
                        None
                    }
                })
        });
    }

    pub fn git_status(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        let (ok, stdout, stderr) =
            self.run_git_porcelain(&["status", "--porcelain=v2", "--branch"]);
        if !ok {
            self.set_status(format!("git status failed: {}", stderr));
            return;
        }

        let mut view = crate::git_status::GitStatusView::new(root.clone());
        let mut text = String::new();

        let mut branch = "unknown".to_string();
        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        let mut untracked = Vec::new();

        for line in stdout.lines() {
            if line.starts_with("# branch.head ") {
                branch = line["# branch.head ".len()..].to_string();
            } else if line.starts_with("1 ") || line.starts_with("2 ") {
                // Changed tracked files
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 9 {
                    let staging = parts[1];
                    let path = parts[parts.len() - 1].to_string();
                    let s_char = staging.chars().next().unwrap_or('.');
                    let u_char = staging.chars().nth(1).unwrap_or('.');

                    if s_char != '.' {
                        staged.push(path.clone());
                    }
                    if u_char != '.' {
                        unstaged.push(path);
                    }
                }
            } else if line.starts_with("? ") {
                // Untracked
                untracked.push(line[2..].to_string());
            }
        }

        text.push_str(&format!("Head:     {}\n\n", branch));

        if !untracked.is_empty() {
            text.push_str("Untracked files:\n");
            for p in &untracked {
                text.push_str(&format!("  ? {}\n", p));
                view.lines.push(crate::git_status::GitStatusLine {
                    text: format!("  ? {}", p),
                    section: Some(crate::git_status::GitSection::Untracked),
                    file_path: Some(p.clone()),
                    hunk: None,
                    is_header: false,
                    is_collapsed: false,
                });
            }
            text.push('\n');
        }

        if !unstaged.is_empty() {
            text.push_str("Unstaged changes:\n");
            for p in &unstaged {
                text.push_str(&format!("  M {}\n", p));
                view.lines.push(crate::git_status::GitStatusLine {
                    text: format!("  M {}", p),
                    section: Some(crate::git_status::GitSection::Unstaged),
                    file_path: Some(p.clone()),
                    hunk: None,
                    is_header: false,
                    is_collapsed: false,
                });
            }
            text.push('\n');
        }

        if !staged.is_empty() {
            text.push_str("Staged changes:\n");
            for p in &staged {
                text.push_str(&format!("  S {}\n", p));
                view.lines.push(crate::git_status::GitStatusLine {
                    text: format!("  S {}", p),
                    section: Some(crate::git_status::GitSection::Staged),
                    file_path: Some(p.clone()),
                    hunk: None,
                    is_header: false,
                    is_collapsed: false,
                });
            }
        }

        // Find or create the buffer
        let buf_name = "*git-status*";
        let idx = if let Some(i) = self.find_buffer_by_name(buf_name) {
            self.buffers[i] = Buffer::new();
            self.buffers[i].name = buf_name.to_string();
            self.buffers[i].kind = crate::buffer::BufferKind::GitStatus;
            i
        } else {
            let mut buf = Buffer::new();
            buf.name = buf_name.to_string();
            buf.kind = crate::buffer::BufferKind::GitStatus;
            self.buffers.push(buf);
            self.buffers.len() - 1
        };

        self.buffers[idx].git_status = Some(view);
        self.buffers[idx].read_only = true;

        // Populate rope
        self.buffers[idx].insert_text_at(0, &text);
        self.buffers[idx].modified = false;

        // Switch to it
        let prev = self.active_buffer_idx();
        self.alternate_buffer_idx = Some(prev);
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.window_mgr.focused_window_mut().cursor_row = 0;
        self.window_mgr.focused_window_mut().cursor_col = 0;
        self.set_mode(crate::Mode::GitStatus);
    }

    fn run_git_porcelain(&self, args: &[&str]) -> (bool, String, String) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        match std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
        {
            Ok(output) => {
                let success = output.status.success();
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                (success, stdout, stderr)
            }
            Err(e) => (false, String::new(), e.to_string()),
        }
    }

    pub fn git_stage_file(&mut self, path: &str) {
        let (ok, _, stderr) = self.run_git_porcelain(&["add", path]);
        if ok {
            self.set_status(format!("Staged {}", path));
            self.git_status(); // Refresh
        } else {
            self.set_status(format!("git add failed: {}", stderr));
        }
    }

    pub fn git_unstage_file(&mut self, path: &str) {
        let (ok, _, stderr) = self.run_git_porcelain(&["reset", "HEAD", "--", path]);
        if ok {
            self.set_status(format!("Unstaged {}", path));
            self.git_status(); // Refresh
        } else {
            self.set_status(format!("git reset failed: {}", stderr));
        }
    }

    pub(crate) fn git_blame(&mut self) {
        let file = self
            .active_buffer()
            .file_path()
            .map(|p| p.display().to_string());
        if let Some(path) = file {
            self.git_command_to_buffer(&["blame", &path], "*git-blame*");
        } else {
            self.set_status("git blame: buffer has no file path");
        }
    }

    pub(crate) fn git_diff(&mut self) {
        self.git_command_to_buffer(&["diff"], "*git-diff*");
    }
    pub(crate) fn git_commit(&mut self) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root.clone())
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_default();

        let commit_file = root.join(".git/COMMIT_EDITMSG");
        self.open_file(&commit_file);
        self.set_status("Edit commit message and save to commit");
    }

    pub(crate) fn git_log(&mut self) {
        self.git_command_to_buffer(&["log", "--oneline", "-50"], "*git-log*");
    }
}
