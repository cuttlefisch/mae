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

    pub(crate) fn git_status(&mut self) {
        self.git_command_to_buffer(&["status"], "*git-status*");
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

    pub(crate) fn git_log(&mut self) {
        self.git_command_to_buffer(&["log", "--oneline", "-50"], "*git-log*");
    }
}
