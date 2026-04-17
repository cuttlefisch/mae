use crate::buffer::Buffer;
use crate::command_palette::CommandPalette;
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

        // Track linewise vs characterwise for operator-pending mode
        self.last_motion_linewise = Self::is_linewise_motion(name);

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
                self.record_jump();
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
                self.record_jump();
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
            "move-word-end-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_word_end_backward(buf);
                }
            }
            "move-big-word-end-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_end_backward(buf);
                }
            }
            "move-to-first-non-blank" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr
                    .focused_window_mut()
                    .move_to_first_non_blank(buf);
            }
            "move-line-next-non-blank" => {
                // vi `+` — down n lines then first non-blank
                let buf = &self.buffers[self.active_buffer_idx()];
                let win = self.window_mgr.focused_window_mut();
                for _ in 0..n {
                    win.move_down(buf);
                }
                win.move_to_first_non_blank(buf);
            }
            "move-line-prev-non-blank" => {
                // vi `-` — up n lines then first non-blank
                let buf = &self.buffers[self.active_buffer_idx()];
                let win = self.window_mgr.focused_window_mut();
                for _ in 0..n {
                    win.move_up(buf);
                }
                win.move_to_first_non_blank(buf);
            }
            "move-matching-bracket" => {
                self.record_jump();
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr
                    .focused_window_mut()
                    .move_matching_bracket(buf);
            }
            "move-paragraph-forward" => {
                self.record_jump();
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_paragraph_forward(buf);
                }
            }
            "move-paragraph-backward" => {
                self.record_jump();
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
                    // Ensure linewise delete text ends with '\n' so paste
                    // recognizes it as linewise.
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
                // Find the Nth word boundary
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
                let start_row = self.window_mgr.focused_window().cursor_row;
                let line_count = self.buffers[idx].line_count();
                let end_row = (start_row + n).min(line_count);
                let mut yanked = String::new();
                for row in start_row..end_row {
                    yanked.push_str(&self.buffers[idx].line_text(row));
                }
                if !yanked.is_empty() {
                    // Ensure linewise yank always ends with '\n' so paste
                    // recognizes it as linewise (last line may lack trailing newline).
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
                if let Some(text) = self.paste_text() {
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
            // Repeat f/F/t/T (;/,)
            "repeat-find" => {
                if let Some((ch, ref cmd)) = self.last_find_char.clone() {
                    self.pending_char_count = n;
                    self.dispatch_char_motion(cmd, ch);
                }
            }
            "repeat-find-reverse" => {
                if let Some((ch, ref cmd)) = self.last_find_char.clone() {
                    let reversed = match cmd.as_str() {
                        "find-char-forward" => "find-char-backward",
                        "find-char-backward" => "find-char-forward",
                        "till-char-forward" => "till-char-backward",
                        "till-char-backward" => "till-char-forward",
                        _ => return true,
                    };
                    self.pending_char_count = n;
                    self.dispatch_char_motion(reversed, ch);
                }
            }

            // Reselect last visual selection (gv)
            "reselect-visual" => {
                if let Some((ar, ac, cr, cc, vtype)) = self.last_visual {
                    self.visual_anchor_row = ar;
                    self.visual_anchor_col = ac;
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = cr;
                    win.cursor_col = cc;
                    self.mode = Mode::Visual(vtype);
                }
            }

            "enter-normal-mode" => {
                if matches!(self.mode, Mode::Visual(_)) {
                    self.save_visual_state();
                }
                if self.mode == Mode::Insert {
                    // Finalize dot-repeat record before adjusting cursor
                    self.finalize_insert_for_repeat();
                    let win = self.window_mgr.focused_window_mut();
                    if win.cursor_col > 0 {
                        win.cursor_col -= 1;
                    }
                    // Record exit position for `gi` (re-enter insert at last pos).
                    let idx = self.active_buffer_idx();
                    let w = self.window_mgr.focused_window();
                    self.last_insert_pos = Some((idx, w.cursor_row, w.cursor_col));
                }
                self.mode = Mode::Normal;
            }
            "enter-command-mode" => {
                self.mode = Mode::Command;
                self.command_line.clear();
                self.command_cursor = 0;
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

            // Help / KB
            "help" => self.open_help_at("index"),
            "help-follow-link" => self.help_follow_link(),
            "help-back" => self.help_back(),
            "help-forward" => self.help_forward(),
            "help-next-link" => self.help_next_link(),
            "help-prev-link" => self.help_prev_link(),
            "help-close" => self.help_close(),
            "help-search" => {
                let nodes: Vec<(String, String)> = self
                    .kb
                    .list_ids(None)
                    .iter()
                    .filter_map(|id| self.kb.get(id).map(|n| (id.clone(), n.title.clone())))
                    .collect();
                self.command_palette = Some(
                    crate::command_palette::CommandPalette::for_help_search(&nodes),
                );
                self.mode = Mode::CommandPalette;
            }
            "help-reopen" => {
                self.help_reopen();
            }

            "command-palette" => {
                self.command_palette = Some(CommandPalette::from_registry(&self.commands));
                self.mode = Mode::CommandPalette;
            }
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
            "new-buffer" => {
                let prev_idx = self.active_buffer_idx();
                let mut buf = Buffer::new();
                let n = self
                    .buffers
                    .iter()
                    .filter(|b| b.name.starts_with("[scratch"))
                    .count();
                if n > 0 {
                    buf.name = format!("[scratch-{}]", n);
                }
                let new_idx = self.buffers.len();
                self.buffers.push(buf);
                let win = self.window_mgr.focused_window_mut();
                win.buffer_idx = new_idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
                self.alternate_buffer_idx = Some(prev_idx);
                self.set_status("New buffer");
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
            "force-kill-buffer" => {
                let idx = self.active_buffer_idx();
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
                let names: Vec<String> = self.buffers.iter().map(|b| b.name.clone()).collect();
                let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                self.command_palette = Some(crate::command_palette::CommandPalette::for_buffers(
                    &name_refs,
                ));
                self.mode = crate::Mode::CommandPalette;
            }
            "find-file" => {
                let root = std::env::current_dir().unwrap_or_default();
                self.file_picker = Some(FilePicker::scan(&root));
                self.mode = Mode::FilePicker;
            }
            // Ranger/dired-style directory browser. Opens at the active
            // buffer's parent dir (so `-` in normal mode feels spatial),
            // or cwd when the buffer has no file path yet.
            "file-browser" => {
                let start = self
                    .active_buffer()
                    .file_path()
                    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_default();
                self.file_browser = Some(crate::FileBrowser::open(&start));
                self.mode = Mode::FileBrowser;
            }
            "recent-files" => self.recent_files_palette(),
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
            "describe-key" => {
                // Arm the interactive "press a key to describe" flow.
                // The binary's key handler intercepts subsequent keypresses
                // while this flag is set, looks them up in the normal
                // keymap, and opens the bound command's help page.
                self.awaiting_key_description = true;
                self.set_status("Describe key: press a key sequence (Esc to cancel)");
            }
            "describe-command" => {
                // Reuse the command-palette overlay for fuzzy selection,
                // but flag the purpose so Enter opens the help buffer
                // instead of executing the command.
                self.command_palette = Some(CommandPalette::for_describe(&self.commands));
                self.mode = Mode::CommandPalette;
            }
            "set-theme" => {
                let names = bundled_theme_names();
                let name_refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
                self.command_palette = Some(crate::command_palette::CommandPalette::for_themes(
                    &name_refs,
                ));
                self.mode = Mode::CommandPalette;
            }
            "cycle-theme" => {
                self.cycle_theme();
            }
            "set-splash-art" => {
                self.command_palette =
                    Some(crate::command_palette::CommandPalette::for_splash_art());
                self.mode = Mode::CommandPalette;
            }

            // Debug commands
            "debug-self" => {
                self.start_self_debug();
            }
            "debug-start" => {
                // Pre-fill the command line so the user can type
                // adapter + program args interactively.
                self.mode = Mode::Command;
                self.command_line = "debug-start ".to_string();
                self.command_cursor = self.command_line.len();
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
                if let Some(state) = &self.debug_state {
                    // Show a summary of current debug state in status.
                    let thread_info = if state.threads.is_empty() {
                        "no threads".to_string()
                    } else {
                        let stopped: Vec<&str> = state
                            .threads
                            .iter()
                            .filter(|t| t.stopped)
                            .map(|t| t.name.as_str())
                            .collect();
                        if stopped.is_empty() {
                            format!("{} threads (all running)", state.threads.len())
                        } else {
                            format!("{} stopped: {}", stopped.len(), stopped.join(", "))
                        }
                    };
                    let frame_info = state
                        .stack_frames
                        .first()
                        .map(|f| format!(" | top: {}:{}", f.name, f.line))
                        .unwrap_or_default();
                    let var_count: usize = state.variables.values().map(|v| v.len()).sum();
                    self.set_status(format!(
                        "Debug: {}{}  | {} vars across {} scopes",
                        thread_info,
                        frame_info,
                        var_count,
                        state.scopes.len()
                    ));
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
            "visual-delete" => {
                self.save_visual_state();
                self.visual_delete();
            }
            "visual-yank" => {
                self.save_visual_state();
                self.visual_yank();
            }
            "visual-change" => {
                self.save_visual_state();
                self.visual_change();
            }
            "visual-indent" => self.visual_indent(),
            "visual-dedent" => self.visual_dedent(),
            "visual-join" => self.visual_join(),
            "visual-paste" => self.visual_paste(),
            "visual-swap-ends" => self.visual_swap_ends(),
            "visual-uppercase" => self.visual_uppercase(),
            "visual-lowercase" => self.visual_lowercase(),

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
                self.record_jump();
                for _ in 0..n {
                    self.jump_to_next_match(true);
                }
            }
            "search-prev" => {
                self.record_jump();
                for _ in 0..n {
                    self.jump_to_next_match(false);
                }
            }
            "search-word-under-cursor" => {
                self.record_jump();
                self.search_word_at_cursor();
            }
            "search-word-under-cursor-backward" => {
                self.record_jump();
                self.search_word_at_cursor_backward();
            }
            "clear-search-highlight" => {
                self.search_state.highlight_active = false;
            }
            // gn / gN — select next/previous match as a visual selection.
            // Practical Vim tip 86: cursor inside a match selects that match;
            // otherwise the next match in the given direction (wrapping).
            "visual-select-next-match" => {
                self.record_jump();
                self.visual_select_match(true);
            }
            "visual-select-prev-match" => {
                self.record_jump();
                self.visual_select_match(false);
            }
            // Operator variants: d{gn,gN}, c{gn,gN}, y{gn,gN}.
            // These first select the match, then apply the operator. `c`
            // is recorded for dot-repeat so that `.` re-runs cgn from the
            // new cursor position — enabling single-key global replace.
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

            // Replace char (pending — next key replaces char under cursor)
            "replace-char-await" => {
                self.pending_char_command = Some("replace-char".to_string());
            }

            // Substitute char (`s`) — delete N chars forward, enter insert.
            // Practical Vim tip 2: a single key replaces `xi` / `cl`.
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
            // Substitute line (`S`) — same as `cc`.
            "substitute-line" => {
                return self.dispatch_builtin("change-line");
            }

            // `gi` — re-enter insert mode at the last insert-exit position.
            "reinsert-at-last-position" => {
                if let Some((target_idx, row, col)) = self.last_insert_pos {
                    // Same-buffer only. If the user switched buffers, gi
                    // just enters insert at current position (vim parity).
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
            "lsp-goto-definition" => {
                self.record_jump();
                self.lsp_request_definition();
            }
            "lsp-find-references" => self.lsp_request_references(),
            "lsp-hover" => self.lsp_request_hover(),

            // LSP completion (Phase 4a M4)
            "lsp-complete" => self.lsp_request_completion(),
            "lsp-accept-completion" => self.lsp_accept_completion(),
            "lsp-dismiss-completion" => self.lsp_dismiss_completion(),
            "lsp-complete-next" => self.lsp_complete_next(),
            "lsp-complete-prev" => self.lsp_complete_prev(),

            // LSP diagnostics (Phase 4a M3)
            "lsp-next-diagnostic" => {
                self.record_jump();
                self.jump_next_diagnostic();
            }
            "lsp-prev-diagnostic" => {
                self.record_jump();
                self.jump_prev_diagnostic();
            }

            // Jump list (Practical Vim ch. 9)
            "jump-backward" => self.jump_backward(n),
            "jump-forward" => self.jump_forward(n),

            // Change list (Practical Vim ch. 9)
            "change-backward" => self.change_backward(n),
            "change-forward" => self.change_forward(n),
            "show-changes-buffer" => self.show_changes_buffer(),
            "show-registers" => self.show_registers_buffer(),
            "prompt-register" => {
                self.pending_register_prompt = true;
                self.set_status("\"");
            }

            // Surrounds: each arms `pending_char_command` for the char-await layer.
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

            // gf — open file under cursor
            "goto-file-under-cursor" => self.goto_file_under_cursor(),

            "lsp-show-diagnostics" => self.show_diagnostics_buffer(),

            // LSP code actions (stubs — queue intent, client not yet wired)
            "lsp-code-action" => {
                self.lsp_request_code_action();
            }
            "lsp-rename" => {
                // Pre-fill command line for user to enter new name
                self.mode = crate::Mode::Command;
                self.command_line = "lsp-rename ".to_string();
                self.command_cursor = self.command_line.len();
                self.set_status("Enter new name for symbol");
            }
            "lsp-format" => {
                self.lsp_request_format();
            }

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

            // Macros
            "start-recording-await" => {
                // q<letter>: await the register char via pending_char_command.
                // The binary's key-handling intercept already stops recording
                // when `q` arrives while macro_recording is true — so this arm
                // is only reached when NOT currently recording.
                self.pending_char_command = Some("start-recording".to_string());
            }
            "replay-macro-await" => {
                // @<letter>: stash the count and await the register char.
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

            // Scheme REPL (lisp machine)
            "eval-line" => self.eval_current_line(),
            "eval-region" => self.eval_visual_region(),
            "eval-buffer" => self.eval_current_buffer(),
            "open-scheme-repl" => self.open_scheme_repl(),

            // +project (SPC p)
            "project-find-file" => self.project_find_file(),
            "project-search" => self.project_search(),
            "project-browse" => self.project_browse(),
            "project-recent-files" => self.project_recent_files(),

            // +search/syntax (SPC s) — search-buffer is an alias
            "search-buffer" => {
                self.dispatch_builtin("search-forward-start");
                return true;
            }

            // +file expansions (SPC f)
            "yank-file-path" => {
                if let Some(path) = self.active_buffer().file_path() {
                    let path_str = path.display().to_string();
                    self.registers.insert('+', path_str.clone());
                    self.set_status(format!("Yanked: {}", path_str));
                } else {
                    self.set_status("Buffer has no file path");
                }
            }
            "rename-file" => {
                let path_str = self
                    .active_buffer()
                    .file_path()
                    .map(|p| p.display().to_string());
                if let Some(ps) = path_str {
                    self.mode = crate::Mode::Command;
                    self.command_line = format!("rename {}", ps);
                    self.command_cursor = self.command_line.len();
                    self.set_status("Rename file: edit path and press Enter");
                } else {
                    self.set_status("Buffer has no file path");
                }
            }
            "save-as" => {
                self.mode = crate::Mode::Command;
                self.command_line = "saveas ".to_string();
                self.command_cursor = self.command_line.len();
                self.set_status("Save as: enter path and press Enter");
            }

            // +buffer expansions (SPC b)
            "kill-other-buffers" => {
                let active = self.active_buffer_idx();
                let mut killed = 0;
                let mut i = 0;
                while i < self.buffers.len() {
                    if i != active && !self.buffers[i].modified {
                        self.buffers.remove(i);
                        if active > i {
                            // Adjust the focused window's buffer_idx
                            let win = self.window_mgr.focused_window_mut();
                            if win.buffer_idx > 0 {
                                win.buffer_idx -= 1;
                            }
                        }
                        killed += 1;
                    } else {
                        i += 1;
                    }
                }
                self.set_status(format!("Killed {} buffer(s)", killed));
            }
            "save-all-buffers" => {
                let mut saved = 0;
                let mut errors = Vec::new();
                for i in 0..self.buffers.len() {
                    if self.buffers[i].modified && self.buffers[i].file_path().is_some() {
                        match self.buffers[i].save() {
                            Ok(()) => saved += 1,
                            Err(e) => errors.push(format!("{}: {}", self.buffers[i].name, e)),
                        }
                    }
                }
                if errors.is_empty() {
                    self.set_status(format!("Saved {} buffer(s)", saved));
                } else {
                    self.set_status(format!("Saved {}, errors: {}", saved, errors.join(", ")));
                }
            }
            "revert-buffer" => {
                let idx = self.active_buffer_idx();
                if let Some(path) = self.buffers[idx].file_path().map(|p| p.to_path_buf()) {
                    match Buffer::from_file(&path) {
                        Ok(buf) => {
                            let name = buf.name.clone();
                            self.buffers[idx] = buf;
                            self.window_mgr.focused_window_mut().cursor_row = 0;
                            self.window_mgr.focused_window_mut().cursor_col = 0;
                            self.set_status(format!("Reverted: {}", name));
                        }
                        Err(e) => self.set_status(format!("Revert failed: {}", e)),
                    }
                } else {
                    self.set_status("Buffer has no file path to revert from");
                }
            }

            // +toggle (SPC t)
            "toggle-line-numbers" => {
                self.show_line_numbers = !self.show_line_numbers;
                self.set_status(format!(
                    "Line numbers: {}",
                    if self.show_line_numbers { "on" } else { "off" }
                ));
            }
            "toggle-relative-line-numbers" => {
                self.relative_line_numbers = !self.relative_line_numbers;
                self.set_status(format!(
                    "Relative line numbers: {}",
                    if self.relative_line_numbers {
                        "on"
                    } else {
                        "off"
                    }
                ));
            }
            "toggle-word-wrap" => {
                self.word_wrap = !self.word_wrap;
                self.set_status(format!(
                    "Word wrap: {}",
                    if self.word_wrap { "on" } else { "off" }
                ));
            }

            // +git (SPC g)
            "git-status" => self.git_status(),
            "git-blame" => self.git_blame(),
            "git-diff" => self.git_diff(),
            "git-log" => self.git_log(),

            // +notes (SPC n)
            "kb-find" => {
                let nodes: Vec<(String, String)> = self
                    .kb
                    .list_ids(None)
                    .iter()
                    .filter_map(|id| self.kb.get(id).map(|n| (id.clone(), n.title.clone())))
                    .collect();
                self.command_palette = Some(
                    crate::command_palette::CommandPalette::for_help_search(&nodes),
                );
                self.mode = Mode::CommandPalette;
            }

            // Operator-pending mode: bare d/c/y enter pending state
            "operator-delete" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("d".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
            }
            "operator-change" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("c".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
            }
            "operator-yank" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("y".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
            }
            "operator-surround" => {
                let win = self.window_mgr.focused_window();
                self.pending_operator = Some("s".to_string());
                self.operator_start = Some((win.cursor_row, win.cursor_col));
            }

            _ => return false,
        }
        true
    }
}
