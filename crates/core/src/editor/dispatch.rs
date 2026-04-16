use crate::buffer::Buffer;
use crate::file_picker::FilePicker;
use crate::theme::bundled_theme_names;
use crate::window::{Direction, SplitDirection};
use crate::{Mode, VisualType};

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

        match name {
            // Movement — operates on focused window + its buffer
            "move-up" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_up(buf);
                }
            }
            "move-down" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_down(buf);
                }
            }
            "move-left" => {
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_left();
                }
            }
            "move-right" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_right(buf);
                }
            }
            "move-to-line-start" => {
                self.window_mgr.focused_window_mut().move_to_line_start();
            }
            "move-to-line-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr.focused_window_mut().move_to_line_end(buf);
            }
            "move-to-first-line" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                if let Some(target) = count {
                    // ngg = go to line n (1-indexed)
                    let row = (target.saturating_sub(1)).min(buf.line_count().saturating_sub(1));
                    self.window_mgr.focused_window_mut().cursor_row = row;
                    self.window_mgr.focused_window_mut().clamp_cursor(buf);
                } else {
                    self.window_mgr.focused_window_mut().move_to_first_line(buf);
                }
            }
            "move-to-last-line" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                if let Some(target) = count {
                    // nG = go to line n (1-indexed)
                    let row = (target.saturating_sub(1)).min(buf.line_count().saturating_sub(1));
                    self.window_mgr.focused_window_mut().cursor_row = row;
                    self.window_mgr.focused_window_mut().clamp_cursor(buf);
                } else {
                    self.window_mgr.focused_window_mut().move_to_last_line(buf);
                }
            }
            "move-word-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_forward(buf);
                }
            }
            "move-word-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_backward(buf);
                }
            }
            "move-word-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_end(buf);
                }
            }
            "move-big-word-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_forward(buf);
                }
            }
            "move-big-word-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_backward(buf);
                }
            }
            "move-big-word-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_big_word_end(buf);
                }
            }
            "move-matching-bracket" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr
                    .focused_window_mut()
                    .move_matching_bracket(buf);
            }
            "move-paragraph-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_paragraph_forward(buf);
                }
            }
            "move-paragraph-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_paragraph_backward(buf);
                }
            }
            "find-char-forward-await" => {
                self.pending_char_command = Some("find-char-forward".to_string());
                self.pending_char_count = n;
            }
            "find-char-backward-await" => {
                self.pending_char_command = Some("find-char-backward".to_string());
                self.pending_char_count = n;
            }
            "till-char-forward-await" => {
                self.pending_char_command = Some("till-char-forward".to_string());
                self.pending_char_count = n;
            }
            "till-char-backward-await" => {
                self.pending_char_command = Some("till-char-backward".to_string());
                self.pending_char_count = n;
            }

            // Scroll commands
            "scroll-half-up" => {
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().scroll_half_up(vh);
                }
            }
            "scroll-half-down" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .scroll_half_down(buf, vh);
                }
            }
            "scroll-page-up" => {
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().scroll_page_up(vh);
                }
            }
            "scroll-page-down" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .scroll_page_down(buf, vh);
                }
            }
            "scroll-center" => {
                let vh = self.viewport_height;
                self.window_mgr.focused_window_mut().scroll_center(vh);
            }
            "scroll-top" => {
                self.window_mgr.focused_window_mut().scroll_cursor_top();
            }
            "scroll-bottom" => {
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .scroll_cursor_bottom(vh);
            }
            // Screen-relative cursor
            "move-screen-top" => {
                self.window_mgr.focused_window_mut().move_to_screen_top();
            }
            "move-screen-middle" => {
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .move_to_screen_middle(vh);
            }
            "move-screen-bottom" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .move_to_screen_bottom(buf, vh);
            }

            // Editing
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
                    self.registers.insert('"', all_deleted);
                }
                self.record_edit_with_count("delete-line", count);
            }
            "delete-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                // Find the Nth word boundary
                let mut end = start;
                for _ in 0..n {
                    end = crate::word::word_start_forward(self.buffers[idx].rope(), end);
                }
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.registers.insert('"', text);
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
                    self.registers.insert('"', text);
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
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_col = 0;
                }
                self.record_edit("delete-to-line-start");
            }
            "yank-line" => {
                let idx = self.active_buffer_idx();
                let start_row = self.window_mgr.focused_window().cursor_row;
                let line_count = self.buffers[idx].line_count();
                let end_row = (start_row + n).min(line_count);
                let mut yanked = String::new();
                for row in start_row..end_row {
                    yanked.push_str(&self.buffers[idx].line_text(row));
                }
                if !yanked.is_empty() {
                    self.registers.insert('"', yanked);
                    let yanked_count = end_row - start_row;
                    self.set_status(format!(
                        "{} line{} yanked",
                        yanked_count,
                        if yanked_count == 1 { "" } else { "s" }
                    ));
                }
            }
            "yank-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let end = crate::word::word_start_forward(self.buffers[idx].rope(), start);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.registers.insert('"', text);
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
                    self.registers.insert('"', text);
                }
            }
            "yank-to-line-start" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let end = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.registers.insert('"', text);
                }
            }
            "paste-after" => {
                if let Some(text) = self.registers.get(&'"').cloned() {
                    let idx = self.active_buffer_idx();
                    let is_linewise = text.ends_with('\n');
                    for _ in 0..n {
                        if is_linewise {
                            // Insert on line below
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
                            // Move cursor to end of pasted text
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
                if let Some(text) = self.registers.get(&'"').cloned() {
                    let idx = self.active_buffer_idx();
                    let is_linewise = text.ends_with('\n');
                    for _ in 0..n {
                        if is_linewise {
                            // Insert on line above
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
            "enter-insert-mode" => self.mode = Mode::Insert,
            "enter-insert-mode-after" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr.focused_window_mut().move_right(buf);
                self.mode = Mode::Insert;
            }
            "enter-insert-mode-eol" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr.focused_window_mut().move_to_line_end(buf);
                self.mode = Mode::Insert;
            }
            "enter-normal-mode" => {
                if self.mode == Mode::Insert {
                    // Finalize dot-repeat record before adjusting cursor
                    self.finalize_insert_for_repeat();
                    let win = self.window_mgr.focused_window_mut();
                    if win.cursor_col > 0 {
                        win.cursor_col -= 1;
                    }
                }
                self.mode = Mode::Normal;
            }
            "enter-command-mode" => {
                self.mode = Mode::Command;
                self.command_line.clear();
            }

            // Window management
            "split-vertical" => {
                let buf_idx = self.active_buffer_idx();
                let area = self.default_area();
                match self
                    .window_mgr
                    .split(SplitDirection::Vertical, buf_idx, area)
                {
                    Ok(_) => {}
                    Err(e) => self.set_status(e),
                }
            }
            "split-horizontal" => {
                let buf_idx = self.active_buffer_idx();
                let area = self.default_area();
                match self
                    .window_mgr
                    .split(SplitDirection::Horizontal, buf_idx, area)
                {
                    Ok(_) => {}
                    Err(e) => self.set_status(e),
                }
            }
            "close-window" => {
                if self
                    .window_mgr
                    .close(self.window_mgr.focused_id())
                    .is_none()
                {
                    self.set_status("Cannot close last window");
                }
            }
            "focus-left" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Left, area);
            }
            "focus-right" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Right, area);
            }
            "focus-up" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Up, area);
            }
            "focus-down" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Down, area);
            }

            // Diagnostics
            "view-messages" => {
                self.open_messages_buffer();
            }

            // Placeholder commands (stubs for leader key tree)
            "command-palette" => self.set_status("Not yet implemented: command-palette"),
            "next-buffer" => {
                if self.buffers.len() <= 1 {
                    return true;
                }
                let prev_idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                win.buffer_idx = (win.buffer_idx + 1) % self.buffers.len();
                win.cursor_row = 0;
                win.cursor_col = 0;
                self.alternate_buffer_idx = Some(prev_idx);
                let name = self.buffers[win.buffer_idx].name.clone();
                self.set_status(format!("Buffer: {}", name));
            }
            "prev-buffer" => {
                if self.buffers.len() <= 1 {
                    return true;
                }
                let prev_idx = self.active_buffer_idx();
                let count = self.buffers.len();
                let win = self.window_mgr.focused_window_mut();
                win.buffer_idx = (win.buffer_idx + count - 1) % count;
                win.cursor_row = 0;
                win.cursor_col = 0;
                self.alternate_buffer_idx = Some(prev_idx);
                let name = self.buffers[win.buffer_idx].name.clone();
                self.set_status(format!("Buffer: {}", name));
            }
            "kill-buffer" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].modified {
                    self.set_status("Buffer has unsaved changes (save first or use :q!)");
                } else if self.buffers.len() <= 1 {
                    // Notify LSP before clobbering the only buffer.
                    self.lsp_notify_did_close_for_buffer(0);
                    // Replace with empty scratch — never have 0 buffers
                    self.buffers[0] = Buffer::new();
                    self.syntax.remove(0);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                    self.set_status("Buffer killed — [scratch]");
                } else {
                    // Notify LSP before removing the buffer.
                    self.lsp_notify_did_close_for_buffer(idx);
                    self.buffers.remove(idx);
                    self.syntax.shift_after_remove(idx);
                    // Fix all window buffer_idx references
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
            "switch-buffer" => {
                let list: Vec<String> = self
                    .buffers
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let modified = if b.modified { " [+]" } else { "" };
                        let current = if i == self.active_buffer_idx() {
                            ">"
                        } else {
                            " "
                        };
                        format!("{}{} {}{}", current, i, b.name, modified)
                    })
                    .collect();
                self.set_status(list.join("  |  "));
            }
            "find-file" => {
                let root = std::env::current_dir().unwrap_or_default();
                self.file_picker = Some(FilePicker::scan(&root));
                self.mode = Mode::FilePicker;
            }
            "recent-files" => self.set_status("Not yet implemented: recent-files"),
            "ai-prompt" => {
                self.open_conversation_buffer();
            }
            "ai-cancel" => {
                // Mark streaming as stopped in conversation buffer.
                // Actual channel cancel is handled by the binary (AiCommand::Cancel).
                let status = match self.conversation_mut() {
                    Some(conv) if conv.streaming => {
                        conv.streaming = false;
                        conv.streaming_start = None;
                        conv.push_system("[cancelled]");
                        "[AI] Cancelled"
                    }
                    Some(_) => "No active AI request to cancel",
                    None => "No AI conversation active",
                };
                self.set_status(status);
            }
            "describe-key" => self.set_status("Not yet implemented: describe-key"),
            "describe-command" => self.set_status("Not yet implemented: describe-command"),
            "set-theme" => {
                // Stub: in full implementation, opens command-line for theme name.
                // For now, set status with available themes.
                let names = bundled_theme_names().join(", ");
                self.set_status(format!("Available themes: {} — use :theme <name>", names));
            }
            "cycle-theme" => {
                self.cycle_theme();
            }

            // Debug commands
            "debug-self" => {
                self.start_self_debug();
            }
            "debug-start" => {
                // Concrete `dap_start_session(...)` is called from the
                // command-line handler (`:debug-start <adapter> <program>`)
                // or from the AI agent tool. Hitting this bare key-bound
                // command without args just prompts for them.
                self.set_status(
                    "Usage: :debug-start <adapter> <program> — or use AI to start a DAP session",
                );
            }
            "debug-stop" => {
                if self.debug_state.is_some() {
                    // If it's a live DAP session, queue a disconnect.
                    let is_dap = matches!(
                        self.debug_state.as_ref().map(|s| &s.target),
                        Some(crate::debug::DebugTarget::Dap { .. })
                    );
                    if is_dap {
                        self.dap_disconnect(true);
                    } else {
                        self.debug_state = None;
                        self.set_status("Debug session ended");
                    }
                } else {
                    self.set_status("No active debug session");
                }
            }
            "debug-continue" | "debug-step-over" | "debug-step-into" | "debug-step-out" => {
                if self.debug_state.is_none() {
                    self.set_status("No active debug session");
                } else {
                    match name {
                        "debug-continue" => self.dap_continue(),
                        "debug-step-over" => self.dap_step(crate::StepKind::Over),
                        "debug-step-into" => self.dap_step(crate::StepKind::In),
                        "debug-step-out" => self.dap_step(crate::StepKind::Out),
                        _ => unreachable!(),
                    }
                }
            }
            "debug-toggle-breakpoint" => {
                self.dap_toggle_breakpoint_at_cursor();
            }
            "debug-inspect" => {
                if self.debug_state.is_some() {
                    self.set_status("Debug inspect: use :debug-eval <expr> or AI agent");
                } else {
                    self.set_status("No active debug session");
                }
            }

            // Visual mode
            "enter-visual-char" => match self.mode {
                Mode::Visual(VisualType::Char) => self.mode = Mode::Normal,
                Mode::Visual(VisualType::Line) => self.mode = Mode::Visual(VisualType::Char),
                _ => self.enter_visual_mode(VisualType::Char),
            },
            "enter-visual-line" => match self.mode {
                Mode::Visual(VisualType::Line) => self.mode = Mode::Normal,
                Mode::Visual(VisualType::Char) => self.mode = Mode::Visual(VisualType::Line),
                _ => self.enter_visual_mode(VisualType::Line),
            },
            "visual-delete" => self.visual_delete(),
            "visual-yank" => self.visual_yank(),
            "visual-change" => self.visual_change(),

            // Text object operators — set pending char command to await object char
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

            // Search
            "search-forward-start" => {
                self.search_state.direction = crate::SearchDirection::Forward;
                self.search_input.clear();
                self.mode = Mode::Search;
            }
            "search-backward-start" => {
                self.search_state.direction = crate::SearchDirection::Backward;
                self.search_input.clear();
                self.mode = Mode::Search;
            }
            "search-next" => {
                for _ in 0..n {
                    self.jump_to_next_match(true);
                }
            }
            "search-prev" => {
                for _ in 0..n {
                    self.jump_to_next_match(false);
                }
            }
            "search-word-under-cursor" => {
                self.search_word_at_cursor();
            }
            "clear-search-highlight" => {
                self.search_state.highlight_active = false;
            }

            // Change operators — delete range, then enter insert mode
            "change-line" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let row = win.cursor_row;
                let line_start = self.buffers[idx].rope().line_to_char(row);
                let line_len = self.buffers[idx].line_len(row);
                if line_len > 0 {
                    let text = self.buffers[idx].text_range(line_start, line_start + line_len);
                    self.buffers[idx].delete_range(line_start, line_start + line_len);
                    self.registers.insert('"', text);
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
                    self.registers.insert('"', text);
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
                    self.registers.insert('"', text);
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
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_col = 0;
                }
                self.enter_insert_for_change("change-to-line-start");
            }

            // Replace char (pending — next key replaces char under cursor)
            "replace-char-await" => {
                self.pending_char_command = Some("replace-char".to_string());
            }

            // Marks: `m<letter>` sets, `'<letter>` jumps. Pending-char
            // pattern — the next keypress is consumed by dispatch_char_motion.
            "set-mark-await" => {
                self.pending_char_command = Some("set-mark".to_string());
            }
            "jump-mark-await" => {
                self.pending_char_command = Some("jump-mark".to_string());
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
                let start_row = self.window_mgr.focused_window().cursor_row;
                let line_count = self.buffers[idx].line_count();
                let end_row = (start_row + n).min(line_count);
                for row in start_row..end_row {
                    let line_start = self.buffers[idx].rope().line_to_char(row);
                    self.buffers[idx].insert_text_at(line_start, "    ");
                }
                self.record_edit_with_count("indent-line", count);
            }
            "dedent-line" => {
                let idx = self.active_buffer_idx();
                let start_row = self.window_mgr.focused_window().cursor_row;
                let line_count = self.buffers[idx].line_count();
                let end_row = (start_row + n).min(line_count);
                for row in start_row..end_row {
                    let line_start = self.buffers[idx].rope().line_to_char(row);
                    let line_text = self.buffers[idx].line_text(row);
                    let spaces: usize = line_text.chars().take(4).take_while(|c| *c == ' ').count();
                    if spaces > 0 {
                        self.buffers[idx].delete_range(line_start, line_start + spaces);
                    }
                }
                // Clamp cursor col after dedent
                let idx2 = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                win.clamp_cursor(&self.buffers[idx2]);
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
                let idx = self.active_buffer_idx();
                let row = self.window_mgr.focused_window().cursor_row;
                let line_start = self.buffers[idx].rope().line_to_char(row);
                let line_len = self.buffers[idx].line_len(row);
                if line_len > 0 {
                    let text = self.buffers[idx].text_range(line_start, line_start + line_len);
                    let upper = text.to_uppercase();
                    self.buffers[idx].delete_range(line_start, line_start + line_len);
                    self.buffers[idx].insert_text_at(line_start, &upper);
                }
                self.record_edit("uppercase-line");
            }
            "lowercase-line" => {
                let idx = self.active_buffer_idx();
                let row = self.window_mgr.focused_window().cursor_row;
                let line_start = self.buffers[idx].rope().line_to_char(row);
                let line_len = self.buffers[idx].line_len(row);
                if line_len > 0 {
                    let text = self.buffers[idx].text_range(line_start, line_start + line_len);
                    let lower = text.to_lowercase();
                    self.buffers[idx].delete_range(line_start, line_start + line_len);
                    self.buffers[idx].insert_text_at(line_start, &lower);
                }
                self.record_edit("lowercase-line");
            }

            // Alternate file
            "alternate-file" => {
                if let Some(alt_idx) = self.alternate_buffer_idx {
                    if alt_idx < self.buffers.len() {
                        let current = self.active_buffer_idx();
                        self.alternate_buffer_idx = Some(current);
                        let win = self.window_mgr.focused_window_mut();
                        win.buffer_idx = alt_idx;
                        win.cursor_row = 0;
                        win.cursor_col = 0;
                        let name = self.buffers[alt_idx].name.clone();
                        self.set_status(format!("Buffer: {}", name));
                    }
                }
            }

            // LSP navigation (Phase 4a M2)
            "lsp-goto-definition" => self.lsp_request_definition(),
            "lsp-find-references" => self.lsp_request_references(),
            "lsp-hover" => self.lsp_request_hover(),

            // LSP diagnostics (Phase 4a M3)
            "lsp-next-diagnostic" => self.jump_next_diagnostic(),
            "lsp-prev-diagnostic" => self.jump_prev_diagnostic(),
            "lsp-show-diagnostics" => self.show_diagnostics_buffer(),

            // Tree-sitter structural editing (Phase 4b M3)
            "syntax-select-node" => {
                self.syntax_select_node();
            }
            "syntax-expand-selection" => {
                self.syntax_expand_selection();
            }
            "syntax-contract-selection" => {
                self.syntax_contract_selection();
            }

            // File operations
            "save" => self.save_current_buffer(),
            "quit" => {
                self.execute_command("q");
            }
            "force-quit" => {
                self.execute_command("q!");
            }
            "save-and-quit" => {
                self.execute_command("wq");
            }

            _ => return false,
        }
        true
    }
}
