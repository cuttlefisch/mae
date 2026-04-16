use super::EditRecord;
use super::Editor;

impl Editor {
    /// Dispatch a char-argument motion (f/F/t/T/r + char). Returns true if handled.
    pub fn dispatch_char_motion(&mut self, command: &str, ch: char) -> bool {
        if command == "replace-char" {
            let idx = self.active_buffer_idx();
            let win = self.window_mgr.focused_window();
            let row = win.cursor_row;
            let col = win.cursor_col;
            let line_len = self.buffers[idx].line_len(row);
            if col < line_len {
                let offset = self.buffers[idx].char_offset_at(row, col);
                self.buffers[idx].delete_range(offset, offset + 1);
                self.buffers[idx].insert_text_at(offset, &ch.to_string());
                // Record for dot-repeat
                self.last_edit = Some(EditRecord {
                    command: "replace-char".to_string(),
                    inserted_text: None,
                    char_arg: Some(ch),
                    count: None,
                });
            }
            return true;
        }

        // Marks: `m<letter>` sets a mark, `'<letter>` jumps to it.
        // Errors surface through the status bar so the user sees why
        // a jump didn't happen (unset name, closed file, etc.).
        if command == "set-mark" {
            match self.set_mark(ch) {
                Ok(()) => self.set_status(format!("Mark '{}' set", ch)),
                Err(e) => self.set_status(e),
            }
            self.pending_char_count = 1;
            return true;
        }
        if command == "jump-mark" {
            if let Err(e) = self.jump_to_mark(ch) {
                self.set_status(e);
            }
            self.pending_char_count = 1;
            return true;
        }

        if command == "start-recording" {
            if let Err(e) = self.start_recording(ch) {
                self.set_status(e);
            }
            return true;
        }

        if command == "replay-macro" {
            let count = self.pending_char_count;
            self.pending_char_count = 1;
            // `@@` arrives as ch == '@': use the last-replayed register.
            let target = if ch == '@' {
                self.last_macro_register
            } else {
                Some(ch)
            };
            match target {
                Some(reg) => {
                    if let Err(e) = self.replay_macro(reg, count) {
                        self.set_status(e);
                    }
                }
                None => self.set_status("No macro to repeat"),
            }
            return true;
        }

        let repeat = self.pending_char_count;
        self.pending_char_count = 1;
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window_mut();
        match command {
            "find-char-forward" => {
                for _ in 0..repeat {
                    win.move_find_char(buf, ch);
                }
            }
            "find-char-backward" => {
                for _ in 0..repeat {
                    win.move_find_char_back(buf, ch);
                }
            }
            "till-char-forward" => {
                for _ in 0..repeat {
                    win.move_till_char(buf, ch);
                }
            }
            "till-char-backward" => {
                for _ in 0..repeat {
                    win.move_till_char_back(buf, ch);
                }
            }
            _ => return false,
        }
        true
    }

    /// Resolve a text object range given the object character and inner/around flag.
    /// Returns None if the object char is not recognized or no match is found.
    fn resolve_text_object(&self, obj: char, inner: bool) -> Option<(usize, usize)> {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let pos = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        let rope = self.buffers[idx].rope();
        match obj {
            'w' => crate::word::word_object_range(rope, pos, inner),
            'W' => crate::word::big_word_object_range(rope, pos, inner),
            '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' | '"' | '\'' | '`' => {
                // Map aliases: b -> (, B -> {
                let effective = match obj {
                    'b' => '(',
                    'B' => '{',
                    _ => obj,
                };
                crate::word::text_object_range(rope, pos, effective, inner)
            }
            _ => None,
        }
    }

    /// Delete a text object range, storing deleted text in the default register.
    pub fn delete_text_object(&mut self, obj: char, inner: bool) {
        if let Some((start, end)) = self.resolve_text_object(obj, inner) {
            if start >= end {
                return;
            }
            let idx = self.active_buffer_idx();
            let text = self.buffers[idx].text_range(start, end);
            self.buffers[idx].delete_range(start, end);
            self.registers.insert('"', text);
            // Move cursor to start of deleted range
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line(start.min(rope.len_chars().saturating_sub(1)));
            let line_start = rope.line_to_char(new_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = start.saturating_sub(line_start);
            win.clamp_cursor(&self.buffers[idx]);
            let cmd_name = if inner {
                "delete-inner-object"
            } else {
                "delete-around-object"
            };
            self.record_edit(cmd_name);
        }
    }

    /// Change a text object: delete the range and enter insert mode.
    pub fn change_text_object(&mut self, obj: char, inner: bool) {
        if let Some((start, end)) = self.resolve_text_object(obj, inner) {
            if start >= end {
                return;
            }
            let idx = self.active_buffer_idx();
            let text = self.buffers[idx].text_range(start, end);
            self.buffers[idx].delete_range(start, end);
            self.registers.insert('"', text);
            // Move cursor to start of deleted range
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line(start.min(rope.len_chars().saturating_sub(1)));
            let line_start = rope.line_to_char(new_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = start.saturating_sub(line_start);
            win.clamp_cursor(&self.buffers[idx]);
            let cmd_name = if inner {
                "change-inner-object"
            } else {
                "change-around-object"
            };
            self.enter_insert_for_change(cmd_name);
        }
    }

    /// Yank a text object range into the default register.
    pub fn yank_text_object(&mut self, obj: char, inner: bool) {
        if let Some((start, end)) = self.resolve_text_object(obj, inner) {
            if start >= end {
                return;
            }
            let idx = self.active_buffer_idx();
            let text = self.buffers[idx].text_range(start, end);
            self.registers.insert('"', text);
            self.set_status("yanked text object");
        }
    }

    /// Set the visual selection to cover a text object range.
    pub fn visual_select_text_object(&mut self, obj: char, inner: bool) {
        if let Some((start, end)) = self.resolve_text_object(obj, inner) {
            if start >= end {
                return;
            }
            let idx = self.active_buffer_idx();
            let rope = self.buffers[idx].rope();
            // Set anchor to start
            let start_row = rope.char_to_line(start);
            let start_line = rope.line_to_char(start_row);
            self.visual_anchor_row = start_row;
            self.visual_anchor_col = start - start_line;
            // Set cursor to end - 1 (since visual selection is inclusive)
            let end_char = end.saturating_sub(1);
            let end_row = rope.char_to_line(end_char);
            let end_line = rope.line_to_char(end_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = end_row;
            win.cursor_col = end_char - end_line;
        }
    }

    /// Dispatch a text object command that was pending a char argument.
    /// Returns true if handled.
    pub fn dispatch_text_object(&mut self, command: &str, ch: char) -> bool {
        match command {
            "delete-inner-object" => self.delete_text_object(ch, true),
            "delete-around-object" => self.delete_text_object(ch, false),
            "change-inner-object" => self.change_text_object(ch, true),
            "change-around-object" => self.change_text_object(ch, false),
            "yank-inner-object" => self.yank_text_object(ch, true),
            "yank-around-object" => self.yank_text_object(ch, false),
            "visual-inner-object" => self.visual_select_text_object(ch, true),
            "visual-around-object" => self.visual_select_text_object(ch, false),
            _ => return false,
        }
        true
    }
}
