use crate::Mode;

use super::{EditRecord, Editor};

impl Editor {
    /// Insert text after the current cursor line. Used by `:read` and `:r`.
    /// Sets a status message with the number of lines inserted.
    pub fn insert_lines_after_cursor(&mut self, text: &str) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let row = win.cursor_row;
        let buf = &self.buffers[idx];

        let line_count = buf.rope().len_lines();
        let insert_pos = if row + 1 >= line_count {
            // At or past the last line — append at end
            buf.rope().len_chars()
        } else {
            buf.rope().line_to_char(row + 1)
        };

        // Ensure we start on a new line if needed
        let needs_newline = if insert_pos > 0 {
            buf.rope().char(insert_pos - 1) != '\n'
        } else {
            false
        };

        let trimmed = text.trim_end_matches('\n');
        let to_insert = if needs_newline {
            format!("\n{}\n", trimmed)
        } else {
            format!("{}\n", trimmed)
        };

        self.buffers[idx].insert_text_at(insert_pos, &to_insert);

        let inserted_lines = text.lines().count();
        self.set_status(format!("{} lines inserted", inserted_lines));
    }
}

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
    /// If the current buffer is a Shell buffer, enters ShellInsert instead.
    pub(crate) fn enter_insert_for_change(&mut self, command: &str) {
        let idx = self.active_buffer_idx();
        if self.buffers[idx].kind == crate::BufferKind::Shell {
            self.set_mode(Mode::ShellInsert);
            return;
        }
        let win = self.window_mgr.focused_window();
        let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        self.insert_start_offset = Some(offset);
        self.insert_initiated_by = Some(command.to_string());
        self.set_mode(Mode::Insert);
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
            // Capture the edit position for `g;` / `g,` navigation.
            self.record_change();
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
        self.record_change();
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
        self.record_change();
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
            | "change-to-line-start"
            | "substitute-char"
            | "change-next-match"
            | "change-prev-match" => {
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
                self.set_mode(Mode::Normal);
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
                self.set_mode(Mode::Normal);
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

    /// Apply a pending operator after a motion has moved the cursor.
    /// Called from the key handling layer after a motion completes while
    /// `pending_operator` is set. Computes the range between the saved
    /// start position and the current cursor, then applies d/c/y.
    pub fn apply_pending_operator(&mut self) {
        self.apply_pending_operator_for_motion("");
    }

    /// Apply the pending operator with knowledge of which motion triggered it.
    pub fn apply_pending_operator_for_motion(&mut self, motion_cmd: &str) {
        let Some(op) = self.pending_operator.take() else {
            return;
        };
        let Some((start_row, start_col)) = self.operator_start.take() else {
            return;
        };
        self.operator_count = None; // consumed — clean up
        let linewise = self.last_motion_linewise;
        let exclusive = Self::is_exclusive_motion(motion_cmd);
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let (end_row, end_col) = (win.cursor_row, win.cursor_col);

        let rope = self.buffers[idx].rope();
        let rope_len = rope.len_chars();
        if rope_len == 0 {
            return;
        }

        let (from, to) = if linewise {
            // Linewise: expand to full lines
            let min_row = start_row.min(end_row);
            let max_row = start_row.max(end_row);
            let from = self.buffers[idx].rope().line_to_char(min_row);
            let to = if max_row + 1 < self.buffers[idx].line_count() {
                self.buffers[idx].rope().line_to_char(max_row + 1)
            } else {
                rope_len
            };
            (from, to)
        } else {
            // Characterwise: use char offsets
            let start_off = self.buffers[idx].char_offset_at(start_row, start_col);
            let end_off = self.buffers[idx].char_offset_at(end_row, end_col);
            if start_off <= end_off {
                if exclusive {
                    // Exclusive: end position not included (w, b, 0, ^)
                    (start_off, end_off)
                } else {
                    // Inclusive: end position included (e, $, %, G, gg, f, t)
                    (start_off, (end_off + 1).min(rope_len))
                }
            } else {
                if exclusive {
                    (end_off, start_off)
                } else {
                    (end_off, (start_off + 1).min(rope_len))
                }
            }
        };

        if from >= to {
            return;
        }

        let text = self.buffers[idx].text_range(from, to);

        match op.as_str() {
            "d" => {
                self.buffers[idx].delete_range(from, to);
                self.save_delete(text);
                // Position cursor at start of deleted range
                let rope = self.buffers[idx].rope();
                let clamped = from.min(rope.len_chars().saturating_sub(1));
                let new_row = rope.char_to_line(clamped);
                let line_start = rope.line_to_char(new_row);
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row = new_row;
                win.cursor_col = clamped.saturating_sub(line_start);
                win.clamp_cursor(&self.buffers[idx]);
                self.record_edit("operator-delete");
            }
            "c" => {
                self.buffers[idx].delete_range(from, to);
                self.save_delete(text);
                // Position cursor at start of deleted range
                let rope = self.buffers[idx].rope();
                let clamped = from.min(rope.len_chars());
                let new_row = if rope.len_chars() == 0 {
                    0
                } else {
                    rope.char_to_line(clamped.min(rope.len_chars().saturating_sub(1)))
                };
                let line_start = if rope.len_chars() == 0 {
                    0
                } else {
                    rope.line_to_char(new_row)
                };
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row = new_row;
                win.cursor_col = clamped.saturating_sub(line_start);
                self.enter_insert_for_change("operator-change");
            }
            "y" => {
                self.save_yank(text);
                // Restore cursor to start position
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row = start_row.min(end_row);
                win.cursor_col =
                    if start_row < end_row || (start_row == end_row && start_col <= end_col) {
                        start_col
                    } else {
                        end_col
                    };
                self.set_status("yanked");
            }
            "s" => {
                // ys{motion}: stash the range for the upcoming char-await
                // that wraps it with a delimiter pair (surround.rs).
                self.pending_surround_range = Some((from, to));
                self.pending_char_command = Some("surround-motion".to_string());
            }
            _ => {}
        }
    }

    /// Returns true if the given command is a motion that can follow an operator.
    pub fn is_motion_command(cmd: &str) -> bool {
        matches!(
            cmd,
            "move-up"
                | "move-down"
                | "move-left"
                | "move-right"
                | "move-to-line-start"
                | "move-to-line-end"
                | "move-to-first-line"
                | "move-to-last-line"
                | "move-word-forward"
                | "move-word-backward"
                | "move-word-end"
                | "move-big-word-forward"
                | "move-big-word-backward"
                | "move-big-word-end"
                | "move-word-end-backward"
                | "move-big-word-end-backward"
                | "move-to-first-non-blank"
                | "move-line-next-non-blank"
                | "move-line-prev-non-blank"
                | "move-matching-bracket"
                | "move-paragraph-forward"
                | "move-paragraph-backward"
                | "move-screen-top"
                | "move-screen-middle"
                | "move-screen-bottom"
                | "scroll-half-up"
                | "scroll-half-down"
                | "scroll-page-up"
                | "scroll-page-down"
                | "search-next"
                | "search-prev"
        )
    }

    /// Returns true if the given motion command operates on full lines.
    pub fn is_linewise_motion(cmd: &str) -> bool {
        matches!(
            cmd,
            "move-up"
                | "move-down"
                | "move-to-first-line"
                | "move-to-last-line"
                | "move-paragraph-forward"
                | "move-paragraph-backward"
                | "move-line-next-non-blank"
                | "move-line-prev-non-blank"
                | "move-screen-top"
                | "move-screen-middle"
                | "move-screen-bottom"
                | "scroll-half-up"
                | "scroll-half-down"
                | "scroll-page-up"
                | "scroll-page-down"
        )
    }

    /// Returns true if the motion is exclusive (end position is NOT included).
    /// Exclusive motions: w, W, b, B, 0, ^, $, search-next, search-prev.
    /// Note: `$` (`move-to-line-end`) is exclusive in our implementation because
    /// `move_to_line_end` sets cursor to `line_len` (one past the last character),
    /// so excluding end_off correctly stops at the last char (matching vim's `d$`).
    /// Inclusive motions: e, E, %, f, t, G, gg, ge, gE, etc.
    pub fn is_exclusive_motion(cmd: &str) -> bool {
        matches!(
            cmd,
            "move-word-forward"
                | "move-word-backward"
                | "move-big-word-forward"
                | "move-big-word-backward"
                | "move-to-line-start"
                | "move-to-line-end"
                | "move-to-first-non-blank"
                | "search-next"
                | "search-prev"
        )
    }
}
