mod dap;
mod edit;
mod file;
mod fold_org;
mod git;
mod lsp;
mod nav;
mod ui;
mod visual;
mod window;

use crate::buffer::Buffer;
use crate::window::Direction;

use super::Editor;

impl Editor {
    /// Dispatch a built-in command by name. Returns true if recognized.
    ///
    /// This is the shared dispatch point for human keybindings and the AI agent.
    /// Scheme-defined commands are handled by the binary (which has the SchemeRuntime).
    pub fn dispatch_builtin(&mut self, name: &str) -> bool {
        // Consume the count prefix at the top of every dispatch.
        // `count` is Some(n) if user typed a digit prefix, None if not.
        // `n` is the effective repeat count (default 1).
        let count = self.count_prefix.take();
        let n = count.unwrap_or(1);

        // Track linewise vs characterwise for operator-pending mode
        self.last_motion_linewise = Self::is_linewise_motion(name);

        // Try each category in turn. Order doesn't matter for correctness
        // (arm names are unique across categories), but we put high-frequency
        // categories first for marginal efficiency.
        if let Some(v) = self.dispatch_nav(name, count, n) {
            return v;
        }
        if let Some(v) = self.dispatch_edit(name, count, n) {
            return v;
        }
        if let Some(v) = self.dispatch_visual(name) {
            return v;
        }
        if let Some(v) = self.dispatch_file(name) {
            return v;
        }
        if let Some(v) = self.dispatch_window(name) {
            return v;
        }
        if let Some(v) = self.dispatch_ui(name) {
            return v;
        }
        if let Some(v) = self.dispatch_fold_org(name) {
            return v;
        }
        if let Some(v) = self.dispatch_git(name) {
            return v;
        }
        if let Some(v) = self.dispatch_lsp(name) {
            return v;
        }
        if let Some(v) = self.dispatch_dap(name) {
            return v;
        }

        false
    }

    /// Kill buffer at `idx`, handling LSP notification, window fixup, and fallback.
    fn kill_buffer_at(&mut self, idx: usize) {
        self.fire_hook("buffer-close");
        if self.buffers.len() <= 1 {
            self.lsp_notify_did_close_for_buffer(0);
            self.buffers[0] = Buffer::new();
            self.syntax.remove(0);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 0;
            self.set_status("Buffer killed — [scratch]");
        } else {
            self.lsp_notify_did_close_for_buffer(idx);
            self.buffers.remove(idx);
            self.syntax.shift_after_remove(idx);
            self.adjust_ai_target_after_remove(idx);
            for win in self.window_mgr.iter_windows_mut() {
                if win.buffer_idx == idx {
                    win.buffer_idx = idx.saturating_sub(1).min(self.buffers.len() - 1);
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                } else if win.buffer_idx > idx {
                    win.buffer_idx -= 1;
                }
            }
            let new_idx = self.active_buffer_idx();
            let name = self.buffers[new_idx].name.clone();
            self.set_status(format!("Buffer killed — now: {}", name));
        }
    }

    /// Focus a window in the given direction with proper hook firing and mode sync.
    fn focus_direction(&mut self, dir: Direction) {
        self.fire_hook("focus-out");
        self.save_mode_to_buffer();
        let area = self.default_area();
        self.window_mgr.focus_direction(dir, area);
        self.sync_mode_to_buffer();
        self.fire_hook("focus-in");
    }

    /// Transform the current line's text using a closure (e.g. uppercase, lowercase).
    fn transform_current_line(&mut self, f: impl FnOnce(&str) -> String) {
        let idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let line_start = self.buffers[idx].rope().line_to_char(row);
        let line_len = self.buffers[idx].line_len(row);
        if line_len > 0 {
            let text = self.buffers[idx].text_range(line_start, line_start + line_len);
            let transformed = f(&text);
            self.buffers[idx].delete_range(line_start, line_start + line_len);
            self.buffers[idx].insert_text_at(line_start, &transformed);
        }
    }
}
