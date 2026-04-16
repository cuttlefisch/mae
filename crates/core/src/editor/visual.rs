use crate::{Mode, VisualType};

use super::Editor;

impl Editor {
    // --- Visual mode ---

    /// Enter visual mode, recording the anchor at the current cursor position.
    pub fn enter_visual_mode(&mut self, vtype: VisualType) {
        let win = self.window_mgr.focused_window();
        self.visual_anchor_row = win.cursor_row;
        self.visual_anchor_col = win.cursor_col;
        self.mode = Mode::Visual(vtype);
    }

    /// Compute the ordered char-offset range for the current visual selection.
    /// Returns `(start, end)` where `start..end` is the selected range.
    pub fn visual_selection_range(&self) -> (usize, usize) {
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();

        match self.mode {
            Mode::Visual(VisualType::Line) => {
                let min_row = self.visual_anchor_row.min(win.cursor_row);
                let max_row = self.visual_anchor_row.max(win.cursor_row);
                let start = buf.rope().line_to_char(min_row);
                let end = if max_row + 1 < buf.line_count() {
                    buf.rope().line_to_char(max_row + 1)
                } else {
                    buf.rope().len_chars()
                };
                (start, end)
            }
            _ => {
                // Charwise
                let anchor = buf.char_offset_at(self.visual_anchor_row, self.visual_anchor_col);
                let cursor = buf.char_offset_at(win.cursor_row, win.cursor_col);
                let start = anchor.min(cursor);
                let end = (anchor.max(cursor) + 1).min(buf.rope().len_chars());
                (start, end)
            }
        }
    }

    /// Delete the visual selection, storing it in the default register.
    pub fn visual_delete(&mut self) {
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            return;
        }
        let idx = self.active_buffer_idx();
        let text = self.buffers[idx].text_range(start, end);
        self.buffers[idx].delete_range(start, end);
        self.save_delete(text);
        // Move cursor to start of deleted range
        let rope = self.buffers[idx].rope();
        let new_row = rope.char_to_line(start.min(rope.len_chars().saturating_sub(1).max(0)));
        let line_start = rope.line_to_char(new_row);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = start.saturating_sub(line_start);
        win.clamp_cursor(&self.buffers[idx]);
        self.mode = Mode::Normal;
    }

    /// Yank the visual selection into the default register without deleting.
    pub fn visual_yank(&mut self) {
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            return;
        }
        let idx = self.active_buffer_idx();
        let text = self.buffers[idx].text_range(start, end);
        self.save_yank(text);
        // Move cursor to start of selection
        let rope = self.buffers[idx].rope();
        let new_row = rope.char_to_line(start);
        let line_start = rope.line_to_char(new_row);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = start - line_start;
        self.mode = Mode::Normal;
    }

    /// Change the visual selection: delete it and enter insert mode.
    pub fn visual_change(&mut self) {
        self.visual_delete();
        self.mode = Mode::Insert;
    }

    /// Save the current visual state for `gv` (reselect-visual).
    pub fn save_visual_state(&mut self) {
        let win = self.window_mgr.focused_window();
        if let Mode::Visual(vtype) = self.mode {
            self.last_visual = Some((
                self.visual_anchor_row,
                self.visual_anchor_col,
                win.cursor_row,
                win.cursor_col,
                vtype,
            ));
        }
    }

    /// Swap cursor and anchor in visual mode (o key).
    pub fn visual_swap_ends(&mut self) {
        let win = self.window_mgr.focused_window_mut();
        let (ar, ac) = (self.visual_anchor_row, self.visual_anchor_col);
        self.visual_anchor_row = win.cursor_row;
        self.visual_anchor_col = win.cursor_col;
        win.cursor_row = ar;
        win.cursor_col = ac;
    }

    /// Indent all lines in the visual selection by 4 spaces.
    pub fn visual_indent(&mut self) {
        self.save_visual_state();
        let win = self.window_mgr.focused_window();
        let min_row = self.visual_anchor_row.min(win.cursor_row);
        let max_row = self.visual_anchor_row.max(win.cursor_row);
        let idx = self.active_buffer_idx();
        for row in min_row..=max_row {
            let line_start = self.buffers[idx].rope().line_to_char(row);
            self.buffers[idx].insert_text_at(line_start, "    ");
        }
        self.mode = Mode::Normal;
    }

    /// Dedent all lines in the visual selection by up to 4 spaces.
    pub fn visual_dedent(&mut self) {
        self.save_visual_state();
        let win = self.window_mgr.focused_window();
        let min_row = self.visual_anchor_row.min(win.cursor_row);
        let max_row = self.visual_anchor_row.max(win.cursor_row);
        let idx = self.active_buffer_idx();
        // Process in reverse so char offsets stay valid.
        for row in (min_row..=max_row).rev() {
            let line_start = self.buffers[idx].rope().line_to_char(row);
            let line_text = self.buffers[idx].line_text(row);
            let spaces: usize = line_text.chars().take(4).take_while(|c| *c == ' ').count();
            if spaces > 0 {
                self.buffers[idx].delete_range(line_start, line_start + spaces);
            }
        }
        self.mode = Mode::Normal;
    }

    /// Join all lines in the visual selection.
    pub fn visual_join(&mut self) {
        self.save_visual_state();
        let win = self.window_mgr.focused_window();
        let min_row = self.visual_anchor_row.min(win.cursor_row);
        let max_row = self.visual_anchor_row.max(win.cursor_row);
        let join_count = max_row - min_row;
        // Position cursor at min_row for joining.
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = min_row;
        for _ in 0..join_count {
            self.join_line();
        }
        self.mode = Mode::Normal;
    }

    /// Replace visual selection with register contents without clobbering the register.
    pub fn visual_paste(&mut self) {
        self.save_visual_state();
        // Read paste text before the delete so we don't lose it.
        let paste = self.paste_text();
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            return;
        }
        let idx = self.active_buffer_idx();
        // Delete the selection (save to black-hole by using active_register = '_').
        self.active_register = Some('_');
        let text = self.buffers[idx].text_range(start, end);
        self.buffers[idx].delete_range(start, end);
        self.save_delete(text);
        // Insert paste text at the deletion point.
        if let Some(ref paste_text) = paste {
            self.buffers[idx].insert_text_at(start, paste_text);
            let end_pos = start + paste_text.chars().count().saturating_sub(1);
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line(end_pos.min(rope.len_chars().saturating_sub(1)));
            let line_start = rope.line_to_char(new_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = end_pos.saturating_sub(line_start);
        } else {
            // No paste text — just position cursor at start.
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line(start.min(rope.len_chars().saturating_sub(1)));
            let line_start = rope.line_to_char(new_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = start.saturating_sub(line_start);
        }
        self.mode = Mode::Normal;
    }

    /// Uppercase the visual selection text.
    pub fn visual_uppercase(&mut self) {
        self.save_visual_state();
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            return;
        }
        let idx = self.active_buffer_idx();
        let text = self.buffers[idx].text_range(start, end);
        let upper = text.to_uppercase();
        self.buffers[idx].delete_range(start, end);
        self.buffers[idx].insert_text_at(start, &upper);
        self.mode = Mode::Normal;
    }

    /// Lowercase the visual selection text.
    pub fn visual_lowercase(&mut self) {
        self.save_visual_state();
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            return;
        }
        let idx = self.active_buffer_idx();
        let text = self.buffers[idx].text_range(start, end);
        let lower = text.to_lowercase();
        self.buffers[idx].delete_range(start, end);
        self.buffers[idx].insert_text_at(start, &lower);
        self.mode = Mode::Normal;
    }
}
