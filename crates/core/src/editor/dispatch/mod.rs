mod dap;
mod edit;
mod file;
mod file_tree;
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
        // Auto-dismiss hover popup on any command that isn't hover-related.
        if self.hover_popup.is_some()
            && !matches!(name, "lsp-hover" | "hover-scroll-down" | "hover-scroll-up")
        {
            self.hover_popup = None;
        }
        // Auto-dismiss code action menu on non-code-action commands.
        if self.code_action_menu.is_some()
            && !matches!(
                name,
                "lsp-code-action"
                    | "lsp-code-action-next"
                    | "lsp-code-action-prev"
                    | "lsp-code-action-select"
                    | "lsp-code-action-dismiss"
            )
        {
            self.code_action_menu = None;
        }

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
        if let Some(v) = self.dispatch_file_tree(name) {
            return v;
        }

        false
    }

    /// Kill buffer at `idx`, handling LSP notification, window fixup, and fallback.
    fn kill_buffer_at(&mut self, idx: usize) {
        // If this buffer is part of a conversation pair, close both halves.
        if let Some(ref pair) = self.conversation_pair {
            let sibling_idx = if idx == pair.output_buffer_idx {
                Some(pair.input_buffer_idx)
            } else if idx == pair.input_buffer_idx {
                Some(pair.output_buffer_idx)
            } else {
                None
            };
            if let Some(sib) = sibling_idx {
                let pair = self.conversation_pair.take().unwrap();
                // Close the sibling's window.
                let sib_win = if sib == pair.input_buffer_idx {
                    pair.input_window_id
                } else {
                    pair.output_window_id
                };
                self.window_mgr.close(sib_win);
                // Remove both buffers (higher index first to avoid shifting).
                let (first, second) = if idx > sib { (idx, sib) } else { (sib, idx) };
                self.remove_buffer_raw(first);
                self.remove_buffer_raw(second);
                self.set_mode(crate::Mode::Normal);
                let new_idx = self.active_buffer_idx();
                let name = self.buffers[new_idx].name.clone();
                self.set_status(format!("Conversation closed — now: {}", name));
                return;
            }
        }

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

    /// Remove a buffer at index and adjust all window references. Shared by
    /// conversation pair teardown so we don't duplicate the index-shifting logic.
    fn remove_buffer_raw(&mut self, idx: usize) {
        if idx >= self.buffers.len() {
            return;
        }
        self.lsp_notify_did_close_for_buffer(idx);
        self.buffers.remove(idx);
        self.syntax.shift_after_remove(idx);
        self.adjust_ai_target_after_remove(idx);
        for win in self.window_mgr.iter_windows_mut() {
            if win.buffer_idx == idx {
                win.buffer_idx = idx
                    .saturating_sub(1)
                    .min(self.buffers.len().saturating_sub(1));
                win.cursor_row = 0;
                win.cursor_col = 0;
            } else if win.buffer_idx > idx {
                win.buffer_idx -= 1;
            }
        }
    }

    /// Focus a window in the given direction with proper hook firing and mode sync.
    fn focus_direction(&mut self, dir: Direction) {
        self.fire_hook("focus-out");
        self.save_mode_to_buffer();
        let area = self.default_area();
        self.window_mgr.focus_direction(dir, area);
        self.sync_mode_to_buffer();
        // When focusing a conversation output buffer, jump cursor to the last line
        // so the user sees the most recent content (not stranded at row 0).
        let idx = self.active_buffer_idx();
        if self.buffers[idx].kind == crate::buffer::BufferKind::Conversation {
            let last_line = self.buffers[idx].display_line_count().saturating_sub(1);
            let win = self.window_mgr.focused_window_mut();
            if win.cursor_row == 0 && last_line > 0 {
                win.cursor_row = last_line;
                win.cursor_col = 0;
                // scroll_offset is now a rope line index (same as all other buffers).
                // Set it high; the renderer clamps to total-viewport_height.
                win.scroll_offset = last_line.saturating_sub(self.viewport_height);
            }
        }
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
