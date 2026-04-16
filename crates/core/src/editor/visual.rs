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
        self.registers.insert('"', text);
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
        self.registers.insert('"', text);
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
}
