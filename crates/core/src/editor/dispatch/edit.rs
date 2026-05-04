use crate::Mode;

use super::super::Editor;

impl Editor {
    /// Dispatch editing commands (delete, yank, paste, insert, change, surround, etc.).
    /// Returns `Some(true)` if handled.
    pub(super) fn dispatch_edit(
        &mut self,
        name: &str,
        count: Option<usize>,
        n: usize,
    ) -> Option<bool> {
        match name {
            "delete-char-forward" => {
                for _ in 0..n {
                    let idx = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window_mut();
                    self.buffers[idx].delete_char_forward(win);
                }
                self.record_edit_with_count("delete-char-forward", count);
            }
            "delete-char-backward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].delete_char_backward(win);
            }
            "delete-line" => {
                let idx = self.active_buffer_idx();
                let mut all_deleted = String::new();
                for _ in 0..n {
                    let win = self.window_mgr.focused_window_mut();
                    let deleted = self.buffers[idx].delete_line(win);
                    all_deleted.push_str(&deleted);
                }
                if !all_deleted.is_empty() {
                    if !all_deleted.ends_with('\n') {
                        all_deleted.push('\n');
                    }
                    self.save_delete(all_deleted);
                }
                self.record_edit_with_count("delete-line", count);
            }
            "delete-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let mut end = start;
                for _ in 0..n {
                    end = crate::word::word_start_forward(self.buffers[idx].rope(), end);
                }
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.save_delete(text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.record_edit_with_count("delete-word-forward", count);
            }
            "delete-to-line-end" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                let line_len = self.buffers[idx].line_len(win.cursor_row);
                let end = line_start + line_len;
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.save_delete(text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.record_edit("delete-to-line-end");
            }
            "delete-to-line-start" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                let end = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.save_delete(text);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_col = 0;
                }
                self.record_edit("delete-to-line-start");
            }
            "yank-line" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].kind == crate::BufferKind::Messages {
                    let entries = self.message_log.entries();
                    let win = self.window_mgr.focused_window();
                    let start_row = win.scroll_offset;
                    let end_row = (start_row + n).min(entries.len());
                    let mut yanked = String::new();
                    for e in &entries[start_row..end_row] {
                        yanked.push_str(&format!("[{}] {}: {}\n", e.level, e.target, e.message));
                    }
                    if !yanked.is_empty() {
                        self.save_yank(yanked);
                        let cnt = end_row - start_row;
                        self.set_status(format!(
                            "{} line{} yanked",
                            cnt,
                            if cnt == 1 { "" } else { "s" }
                        ));
                    }
                } else {
                    let start_row = self.window_mgr.focused_window().cursor_row;
                    let line_count = self.buffers[idx].line_count();
                    let end_row = (start_row + n).min(line_count);
                    let mut yanked = String::new();
                    for row in start_row..end_row {
                        yanked.push_str(&self.buffers[idx].line_text(row));
                    }
                    if !yanked.is_empty() {
                        if !yanked.ends_with('\n') {
                            yanked.push('\n');
                        }
                        self.save_yank(yanked);
                        let yanked_count = end_row - start_row;
                        self.set_status(format!(
                            "{} line{} yanked",
                            yanked_count,
                            if yanked_count == 1 { "" } else { "s" }
                        ));
                    }
                }
            }
            "yank-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let end = crate::word::word_start_forward(self.buffers[idx].rope(), start);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.save_yank(text);
                }
            }
            "yank-to-line-end" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                let line_len = self.buffers[idx].line_len(win.cursor_row);
                let end = line_start + line_len;
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.save_yank(text);
                }
            }
            "yank-to-line-start" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let end = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.save_yank(text);
                }
            }
            "paste-after" => {
                if let Some(text) = self.paste_text() {
                    let idx = self.active_buffer_idx();
                    let is_linewise = text.ends_with('\n');
                    for _ in 0..n {
                        if is_linewise {
                            let win = self.window_mgr.focused_window();
                            let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                            let line_len =
                                self.buffers[idx].rope().line(win.cursor_row).len_chars();
                            let insert_pos = line_start + line_len;
                            self.buffers[idx].insert_text_at(insert_pos, &text);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_row += 1;
                            win.cursor_col = 0;
                        } else {
                            let win = self.window_mgr.focused_window();
                            let offset =
                                self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                            let insert_pos = (offset + 1).min(self.buffers[idx].rope().len_chars());
                            self.buffers[idx].insert_text_at(insert_pos, &text);
                            let end_pos = insert_pos + text.chars().count() - 1;
                            let rope = self.buffers[idx].rope();
                            let new_row = rope.char_to_line(end_pos);
                            let line_start = rope.line_to_char(new_row);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_row = new_row;
                            win.cursor_col = end_pos - line_start;
                        }
                    }
                }
                self.record_edit_with_count("paste-after", count);
            }
            "paste-before" => {
                if let Some(text) = self.paste_text() {
                    let idx = self.active_buffer_idx();
                    let is_linewise = text.ends_with('\n');
                    for _ in 0..n {
                        if is_linewise {
                            let win = self.window_mgr.focused_window();
                            let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                            self.buffers[idx].insert_text_at(line_start, &text);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_col = 0;
                        } else {
                            let win = self.window_mgr.focused_window();
                            let offset =
                                self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                            self.buffers[idx].insert_text_at(offset, &text);
                            let end_pos = offset + text.chars().count() - 1;
                            let rope = self.buffers[idx].rope();
                            let new_row = rope.char_to_line(end_pos);
                            let line_start = rope.line_to_char(new_row);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_row = new_row;
                            win.cursor_col = end_pos - line_start;
                        }
                    }
                }
                self.record_edit_with_count("paste-before", count);
            }
            "open-line-below" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].open_line_below(win);
                self.enter_insert_for_change("open-line-below");
            }
            "open-line-above" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].open_line_above(win);
                self.enter_insert_for_change("open-line-above");
            }

            // Undo/Redo
            "undo" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].undo(win);
            }
            "redo" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].redo(win);
            }

            // Mode changes
            "enter-insert-mode" => {
                let idx = self.active_buffer_idx();
                use crate::buffer_mode::BufferMode;
                let mode = self.buffers[idx].kind.insert_mode();
                if mode == Mode::Insert {
                    self.buffers[idx].begin_undo_group();
                }
                self.set_mode(mode);
            }
            "enter-insert-mode-after" => {
                let idx = self.active_buffer_idx();
                use crate::buffer_mode::BufferMode;
                let mode = self.buffers[idx].kind.insert_mode();
                if mode == Mode::Insert {
                    self.window_mgr
                        .focused_window_mut()
                        .move_right(&self.buffers[idx]);
                    self.buffers[idx].begin_undo_group();
                }
                self.set_mode(mode);
            }
            "enter-insert-mode-eol" => {
                let idx = self.active_buffer_idx();
                use crate::buffer_mode::BufferMode;
                let mode = self.buffers[idx].kind.insert_mode();
                if mode == Mode::Insert {
                    self.window_mgr
                        .focused_window_mut()
                        .move_to_line_end(&self.buffers[idx]);
                    self.buffers[idx].begin_undo_group();
                }
                self.set_mode(mode);
            }
            "enter-normal-mode" => {
                self.insert_mode_oneshot_normal = false;
                if matches!(self.mode, Mode::Visual(_)) {
                    self.save_visual_state();
                }
                if self.mode == Mode::Insert {
                    self.finalize_insert_for_repeat();

                    if let Some((min_row, max_row, col)) = self.pending_block_insert.take() {
                        let idx = self.active_buffer_idx();
                        if let Some(ref edit) = self.last_edit {
                            if let Some(ref text) = edit.inserted_text {
                                if !text.is_empty() {
                                    for row in (min_row + 1..=max_row).rev() {
                                        if row < self.buffers[idx].line_count() {
                                            let line_start =
                                                self.buffers[idx].rope().line_to_char(row);
                                            let line_len = self.buffers[idx]
                                                .line_text(row)
                                                .trim_end_matches('\n')
                                                .chars()
                                                .count();
                                            let ins_col = col.min(line_len);
                                            self.buffers[idx]
                                                .insert_text_at(line_start + ins_col, text);
                                        }
                                    }
                                }
                            }
                        }
                        self.buffers[idx].end_undo_group();
                    } else {
                        let idx = self.active_buffer_idx();
                        self.buffers[idx].end_undo_group();
                    }

                    let win = self.window_mgr.focused_window_mut();
                    if win.cursor_col > 0 {
                        win.cursor_col -= 1;
                    }
                    let idx = self.active_buffer_idx();
                    let w = self.window_mgr.focused_window();
                    self.last_insert_pos = Some((idx, w.cursor_row, w.cursor_col));
                }
                self.set_mode(Mode::Normal);
            }
            "enter-command-mode" => {
                self.set_mode(Mode::Command);
                self.command_line.clear();
                self.command_cursor = 0;
            }

            // Text object operators
            "delete-inner-object"
            | "delete-around-object"
            | "change-inner-object"
            | "change-around-object"
            | "yank-inner-object"
            | "yank-around-object"
            | "visual-inner-object"
            | "visual-around-object" => {
                self.pending_char_command = Some(name.to_string());
            }

            // Operator variants on matches: d{gn,gN}, c{gn,gN}, y{gn,gN}
            "delete-next-match" => {
                self.record_jump();
                if self.visual_select_match(true) {
                    self.visual_delete();
                    self.record_edit("delete-next-match");
                }
            }
            "delete-prev-match" => {
                self.record_jump();
                if self.visual_select_match(false) {
                    self.visual_delete();
                    self.record_edit("delete-prev-match");
                }
            }
            "change-next-match" => {
                self.record_jump();
                if self.visual_select_match(true) {
                    self.visual_delete();
                    self.enter_insert_for_change("change-next-match");
                } else {
                    self.enter_insert_for_change("change-next-match");
                }
            }
            "change-prev-match" => {
                self.record_jump();
                if self.visual_select_match(false) {
                    self.visual_delete();
                    self.enter_insert_for_change("change-prev-match");
                } else {
                    self.enter_insert_for_change("change-prev-match");
                }
            }
            "yank-next-match" => {
                self.record_jump();
                if self.visual_select_match(true) {
                    self.visual_yank();
                }
            }
            "yank-prev-match" => {
                self.record_jump();
                if self.visual_select_match(false) {
                    self.visual_yank();
                }
            }

            // Change operators
            "change-line" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let row = win.cursor_row;
                let line_start = self.buffers[idx].rope().line_to_char(row);
                let line_len = self.buffers[idx].line_len(row);
                if line_len > 0 {
                    let text = self.buffers[idx].text_range(line_start, line_start + line_len);
                    self.buffers[idx].delete_range(line_start, line_start + line_len);
                    self.save_delete(text);
                }
                let win = self.window_mgr.focused_window_mut();
                win.cursor_col = 0;
                self.enter_insert_for_change("change-line");
            }
            "change-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let end = crate::word::word_start_forward(self.buffers[idx].rope(), start);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.save_delete(text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.enter_insert_for_change("change-word-forward");
            }
            "change-to-line-end" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                let line_len = self.buffers[idx].line_len(win.cursor_row);
                let end = line_start + line_len;
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.save_delete(text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.enter_insert_for_change("change-to-line-end");
            }
            "change-to-line-start" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let end = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.save_delete(text);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_col = 0;
                }
                self.enter_insert_for_change("change-to-line-start");
            }

            // Replace char
            "replace-char-await" => {
                self.pending_char_command = Some("replace-char".to_string());
            }

            // Substitute
            "substitute-char" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                let start = line_start + win.cursor_col;
                let line_end = line_start + self.buffers[idx].line_len(win.cursor_row);
                let end = (start + n).min(line_end);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.save_delete(text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.enter_insert_for_change("substitute-char");
            }
            "substitute-line" => {
                return Some(self.dispatch_builtin("change-line"));
            }

            // gi — re-enter insert at last position
            "reinsert-at-last-position" => {
                if let Some((target_idx, row, col)) = self.last_insert_pos {
                    if target_idx == self.active_buffer_idx() {
                        let idx = self.active_buffer_idx();
                        let win = self.window_mgr.focused_window_mut();
                        win.cursor_row = row;
                        win.cursor_col = col;
                        win.clamp_cursor(&self.buffers[idx]);
                    }
                }
                self.enter_insert_for_change("reinsert-at-last-position");
            }

            // Dot repeat
            "dot-repeat" => {
                self.replay_last_edit();
            }

            // Join lines
            "join-lines" => {
                for _ in 0..n {
                    self.join_line();
                }
                self.record_edit_with_count("join-lines", count);
            }

            // Indent / dedent
            "indent-line" => {
                let idx = self.active_buffer_idx();
                let is_org = self.syntax.language_of(idx) == Some(crate::syntax::Language::Org);
                let row = self.window_mgr.focused_window().cursor_row;
                let on_heading = is_org && self.buffers[idx].line_text(row).starts_with('*');
                if on_heading {
                    for _ in 0..n {
                        self.org_demote();
                    }
                } else {
                    let line_count = self.buffers[idx].line_count();
                    let end_row = (row + n).min(line_count);
                    for r in row..end_row {
                        let line_start = self.buffers[idx].rope().line_to_char(r);
                        self.buffers[idx].insert_text_at(line_start, "    ");
                    }
                }
                self.record_edit_with_count("indent-line", count);
            }
            "dedent-line" => {
                let idx = self.active_buffer_idx();
                let is_org = self.syntax.language_of(idx) == Some(crate::syntax::Language::Org);
                let row = self.window_mgr.focused_window().cursor_row;
                let on_heading = is_org && self.buffers[idx].line_text(row).starts_with('*');
                if on_heading {
                    for _ in 0..n {
                        self.org_promote();
                    }
                } else {
                    let line_count = self.buffers[idx].line_count();
                    let end_row = (row + n).min(line_count);
                    for r in row..end_row {
                        let line_start = self.buffers[idx].rope().line_to_char(r);
                        let line_text = self.buffers[idx].line_text(r);
                        let spaces: usize =
                            line_text.chars().take(4).take_while(|c| *c == ' ').count();
                        if spaces > 0 {
                            self.buffers[idx].delete_range(line_start, line_start + spaces);
                        }
                    }
                    let idx2 = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx2]);
                }
                self.record_edit_with_count("dedent-line", count);
            }

            // Case change
            "toggle-case" => {
                for _ in 0..n {
                    self.toggle_case_at_cursor();
                }
                self.record_edit_with_count("toggle-case", count);
            }
            "uppercase-line" => {
                self.transform_current_line(|t| t.to_uppercase());
                self.record_edit("uppercase-line");
            }
            "lowercase-line" => {
                self.transform_current_line(|t| t.to_lowercase());
                self.record_edit("lowercase-line");
            }

            // Registers
            "show-changes-buffer" => self.show_changes_buffer(),
            "show-registers" => self.show_registers_buffer(),
            "paste-from-yank" => {
                if let Some(text) = self.registers.get(&'0').cloned() {
                    let idx = self.active_buffer_idx();
                    let is_linewise = text.ends_with('\n');
                    for _ in 0..n {
                        if is_linewise {
                            let win = self.window_mgr.focused_window_mut();
                            let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                            let line_len =
                                self.buffers[idx].rope().line(win.cursor_row).len_chars();
                            let insert_pos = line_start + line_len;
                            self.buffers[idx].insert_text_at(insert_pos, &text);
                            win.cursor_row += 1;
                            win.cursor_col = 0;
                        } else {
                            let win = self.window_mgr.focused_window_mut();
                            let pos =
                                self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                            let insert_pos = (pos + 1).min(self.buffers[idx].rope().len_chars());
                            self.buffers[idx].insert_text_at(insert_pos, &text);
                            let new_end = insert_pos + text.len();
                            let new_row = self.buffers[idx]
                                .rope()
                                .char_to_line(new_end.saturating_sub(1));
                            let line_start = self.buffers[idx].rope().line_to_char(new_row);
                            win.cursor_row = new_row;
                            win.cursor_col = new_end.saturating_sub(1) - line_start;
                        }
                    }
                }
            }
            "prompt-register" => {
                self.pending_register_prompt = true;
                self.set_status("\"");
            }

            // Surround
            "delete-surround-await" => {
                self.pending_char_command = Some("delete-surround".to_string());
            }
            "change-surround-await" => {
                self.pending_char_command = Some("change-surround-1".to_string());
            }
            "surround-line-await" => {
                self.pending_char_command = Some("surround-line".to_string());
            }
            "surround-visual-await" => {
                self.pending_char_command = Some("surround-visual".to_string());
            }

            // Alternate file
            "alternate-file" => {
                if let Some(alt_idx) = self.alternate_buffer_idx {
                    if alt_idx < self.buffers.len() {
                        self.save_mode_to_buffer();
                        let current = self.active_buffer_idx();
                        self.alternate_buffer_idx = Some(current);
                        self.display_buffer_and_focus(alt_idx);
                        let name = self.buffers[alt_idx].name.clone();
                        self.set_status(format!("Buffer: {}", name));
                        self.sync_mode_to_buffer();
                    }
                }
            }

            // Macros
            "start-recording-await" => {
                self.pending_char_command = Some("start-recording".to_string());
            }
            "replay-macro-await" => {
                self.pending_char_count = n;
                self.pending_char_command = Some("replay-macro".to_string());
            }
            "replay-last-macro" => {
                if let Some(ch) = self.last_macro_register {
                    if let Err(e) = self.replay_macro(ch, n) {
                        self.set_status(e);
                    }
                } else {
                    self.set_status("No macro to repeat");
                }
            }

            // Scheme eval
            "eval-line" => self.eval_current_line(),
            "eval-region" => self.eval_visual_region(),
            "eval-buffer" => self.eval_current_buffer(),

            // Operator-pending mode
            "operator-delete" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("d".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
                self.operator_count = count;
            }
            "operator-change" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("c".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
                self.operator_count = count;
            }
            "operator-yank" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("y".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
                self.operator_count = count;
            }
            "operator-surround" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("s".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
                self.operator_count = count;
            }

            _ => return None,
        }
        Some(true)
    }
}
