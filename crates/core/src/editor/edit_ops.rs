use crate::Mode;

use super::{EditRecord, Editor};

impl Editor {
    /// Join current line with the next: remove newline, collapse leading whitespace to one space.
    pub(crate) fn join_line(&mut self) {
        let idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let line_count = self.buffers[idx].line_count();
        if row + 1 >= line_count {
            return; // last line, nothing to join
        }
        // Find the newline at end of current line
        let line_start = self.buffers[idx].rope().line_to_char(row);
        let line_chars = self.buffers[idx].rope().line(row).len_chars();
        let newline_pos = line_start + line_chars - 1; // position of '\n'

        // Count leading whitespace on next line
        let next_line_text = self.buffers[idx].line_text(row + 1);
        let leading_ws: usize = next_line_text
            .chars()
            .take_while(|c| c.is_whitespace() && *c != '\n')
            .count();

        // Delete from newline through leading whitespace of next line
        let delete_end = newline_pos + 1 + leading_ws;
        self.buffers[idx].delete_range(newline_pos, delete_end);

        // Insert a single space (unless current line was empty or next line was empty after stripping)
        let next_remaining = &next_line_text[next_line_text
            .char_indices()
            .nth(leading_ws)
            .map(|(i, _)| i)
            .unwrap_or(next_line_text.len())..];
        let next_has_content = !next_remaining.is_empty() && next_remaining != "\n";
        if next_has_content {
            self.buffers[idx].insert_text_at(newline_pos, " ");
        }
    }

    /// Toggle the case of the character under the cursor and advance.
    pub(crate) fn toggle_case_at_cursor(&mut self) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let row = win.cursor_row;
        let col = win.cursor_col;
        let line_len = self.buffers[idx].line_len(row);
        if col >= line_len {
            return;
        }
        let offset = self.buffers[idx].char_offset_at(row, col);
        let ch = self.buffers[idx].rope().char(offset);
        let toggled: String = if ch.is_uppercase() {
            ch.to_lowercase().collect()
        } else {
            ch.to_uppercase().collect()
        };
        self.buffers[idx].delete_range(offset, offset + 1);
        self.buffers[idx].insert_text_at(offset, &toggled);
        // Advance cursor
        let win = self.window_mgr.focused_window_mut();
        let new_line_len = self.buffers[idx].line_len(row);
        if col + 1 < new_line_len {
            win.cursor_col = col + 1;
        }
    }

    /// Enter insert mode from a change command, recording state for dot-repeat.
    pub(crate) fn enter_insert_for_change(&mut self, command: &str) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        self.insert_start_offset = Some(offset);
        self.insert_initiated_by = Some(command.to_string());
        self.mode = Mode::Insert;
    }

    /// Called when exiting insert mode to finalize the dot-repeat record.
    /// Captures any text that was typed during the insert session.
    pub fn finalize_insert_for_repeat(&mut self) {
        if let (Some(cmd), Some(start_offset)) = (
            self.insert_initiated_by.take(),
            self.insert_start_offset.take(),
        ) {
            let idx = self.active_buffer_idx();
            let win = self.window_mgr.focused_window();
            let current_offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
            // The cursor is at the end of what was inserted (or beyond).
            // In insert mode the cursor is *after* the last inserted char,
            // so the inserted text is start_offset..current_offset.
            let inserted = if current_offset > start_offset {
                Some(self.buffers[idx].text_range(start_offset, current_offset))
            } else {
                None
            };
            self.last_edit = Some(EditRecord {
                command: cmd,
                inserted_text: inserted,
                char_arg: None,
                count: None,
            });
            // Notify LSP of the full insert-session content so queries during
            // the next idle tick reflect typed text.
            self.lsp_notify_did_change();
            // Invalidate tree-sitter cache so the next render reparses.
            let buf_idx = self.active_buffer_idx();
            self.syntax.invalidate(buf_idx);
        }
    }

    /// Record a non-insert edit for dot-repeat (delete, paste, etc.).
    /// Also invalidates cached search matches since buffer content changed,
    /// and queues an LSP didChange so language servers stay in sync with
    /// the dirty buffer.
    pub fn record_edit(&mut self, command: &str) {
        self.search_state.matches.clear();
        self.last_edit = Some(EditRecord {
            command: command.to_string(),
            inserted_text: None,
            char_arg: None,
            count: None,
        });
        self.lsp_notify_did_change();
        let buf_idx = self.active_buffer_idx();
        self.syntax.invalidate(buf_idx);
    }

    /// Record a non-insert edit with count for dot-repeat.
    /// Also invalidates cached search matches since buffer content changed,
    /// and queues an LSP didChange so language servers stay in sync.
    pub(crate) fn record_edit_with_count(&mut self, command: &str, count: Option<usize>) {
        self.search_state.matches.clear();
        self.last_edit = Some(EditRecord {
            command: command.to_string(),
            inserted_text: None,
            char_arg: None,
            count,
        });
        self.lsp_notify_did_change();
        let buf_idx = self.active_buffer_idx();
        self.syntax.invalidate(buf_idx);
    }

    /// Replay the last recorded edit (dot-repeat).
    pub(crate) fn replay_last_edit(&mut self) {
        let record = match self.last_edit.clone() {
            Some(r) => r,
            None => return,
        };

        // Restore count prefix from the recorded edit so the repeated
        // dispatch uses the same count as the original.
        self.count_prefix = record.count;

        match record.command.as_str() {
            "replace-char" => {
                if let Some(ch) = record.char_arg {
                    self.dispatch_char_motion("replace-char", ch);
                }
            }
            "change-line"
            | "change-word-forward"
            | "change-to-line-end"
            | "change-to-line-start" => {
                // Re-dispatch the change command (which enters insert mode)
                self.dispatch_builtin(&record.command);
                // Now we need to insert the recorded text and return to normal mode
                if let Some(ref text) = record.inserted_text {
                    let idx = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window();
                    let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                    self.buffers[idx].insert_text_at(offset, text);
                    // Move cursor past inserted text
                    let new_offset = offset + text.chars().count();
                    let rope = self.buffers[idx].rope();
                    let new_row =
                        rope.char_to_line(new_offset.min(rope.len_chars().saturating_sub(1)));
                    let line_start = rope.line_to_char(new_row);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = new_row;
                    win.cursor_col = new_offset.saturating_sub(line_start);
                }
                // Exit insert mode without recording (would overwrite the repeat record)
                self.mode = Mode::Normal;
                self.insert_initiated_by = None;
                self.insert_start_offset = None;
                // Restore the last_edit since dispatch_builtin would have set up
                // insert_initiated_by, and we need to preserve the original record
                self.last_edit = Some(record);
            }
            "open-line-below" | "open-line-above" => {
                self.dispatch_builtin(&record.command);
                if let Some(ref text) = record.inserted_text {
                    let idx = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window();
                    let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                    self.buffers[idx].insert_text_at(offset, text);
                    let new_offset = offset + text.chars().count();
                    let rope = self.buffers[idx].rope();
                    let new_row =
                        rope.char_to_line(new_offset.min(rope.len_chars().saturating_sub(1)));
                    let line_start = rope.line_to_char(new_row);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = new_row;
                    win.cursor_col = new_offset.saturating_sub(line_start);
                }
                self.mode = Mode::Normal;
                self.insert_initiated_by = None;
                self.insert_start_offset = None;
                self.last_edit = Some(record);
            }
            _ => {
                // Simple commands: delete-line, delete-char-forward, paste-after, etc.
                self.dispatch_builtin(&record.command);
            }
        }
    }
}
