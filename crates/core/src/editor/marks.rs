//! Named cursor marks (`m`+letter to set, `'`+letter to jump).
//!
//! Marks record a (path, row, col) triple so they survive buffer switches:
//! jumping to a mark set in another file re-focuses the window on that
//! buffer. There's no hard distinction between "local" and "global" marks
//! at this layer — lowercase/uppercase is a user convention.
//!
//! Stale marks (file no longer open, row past EOF) fail loudly rather than
//! silently teleporting to a wrong position.

use std::path::PathBuf;

use super::Editor;

/// A recorded cursor position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    /// Path of the buffer at set time, if any. `None` for scratch buffers.
    pub path: Option<PathBuf>,
    /// Buffer index at set time — only used as a fallback for scratch
    /// marks (`path == None`). Stale indices error out rather than jumping
    /// to an unrelated buffer.
    pub buffer_idx: usize,
    pub row: usize,
    pub col: usize,
}

impl Editor {
    /// Valid mark names are ASCII letters (a-zA-Z).
    pub fn is_valid_mark_char(ch: char) -> bool {
        ch.is_ascii_alphabetic()
    }

    /// Record the current cursor position under the given mark name.
    pub fn set_mark(&mut self, ch: char) -> Result<(), String> {
        if !Self::is_valid_mark_char(ch) {
            return Err(format!("Invalid mark name: '{}'", ch));
        }
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let path = self.buffers[idx].file_path().map(|p| p.to_path_buf());
        self.marks.insert(
            ch,
            Mark {
                path,
                buffer_idx: idx,
                row: win.cursor_row,
                col: win.cursor_col,
            },
        );
        Ok(())
    }

    /// Jump to the mark named `ch`. Switches buffers when the mark's path
    /// matches an open buffer; errors if the file is gone or the mark is
    /// unset.
    pub fn jump_to_mark(&mut self, ch: char) -> Result<(), String> {
        if !Self::is_valid_mark_char(ch) {
            return Err(format!("Invalid mark name: '{}'", ch));
        }
        let mark = self
            .marks
            .get(&ch)
            .cloned()
            .ok_or_else(|| format!("Mark not set: '{}'", ch))?;

        // Resolve target buffer index.
        let target_idx = if let Some(ref path) = mark.path {
            self.buffers
                .iter()
                .position(|b| b.file_path().map(|p| p == path.as_path()).unwrap_or(false))
                .ok_or_else(|| format!("Mark '{}' file is not open", ch))?
        } else if mark.buffer_idx < self.buffers.len()
            && self.buffers[mark.buffer_idx].file_path().is_none()
        {
            // Scratch mark: buffer_idx still points to an unnamed buffer.
            mark.buffer_idx
        } else {
            return Err(format!("Mark '{}' buffer is gone", ch));
        };

        // Switch focus to the target buffer if needed.
        {
            let win = self.window_mgr.focused_window_mut();
            if win.buffer_idx != target_idx {
                win.buffer_idx = target_idx;
            }
        }

        // Clamp row/col to current buffer dimensions (handles truncation
        // since the mark was set).
        let line_count = self.buffers[target_idx].display_line_count();
        let row = mark.row.min(line_count.saturating_sub(1));
        let col_max = self.buffers[target_idx].line_len(row);
        let col = mark.col.min(col_max);

        let vh = self.viewport_height;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
        win.scroll_center(vh);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn ed_with_text(s: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, s);
        Editor::with_buffer(buf)
    }

    #[test]
    fn set_and_jump_same_buffer_restores_cursor() {
        let mut ed = ed_with_text("line1\nline2\nline3\n");
        // Move to row 2, col 3
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 2;
            win.cursor_col = 3;
        }
        ed.set_mark('a').unwrap();

        // Move somewhere else.
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 0;
        }

        ed.jump_to_mark('a').unwrap();
        let win = ed.window_mgr.focused_window();
        assert_eq!(win.cursor_row, 2);
        assert_eq!(win.cursor_col, 3);
    }

    #[test]
    fn set_mark_rejects_non_alpha() {
        let mut ed = ed_with_text("hi\n");
        assert!(ed.set_mark('1').is_err());
        assert!(ed.set_mark(' ').is_err());
        assert!(ed.set_mark('!').is_err());
    }

    #[test]
    fn jump_to_mark_rejects_non_alpha() {
        let mut ed = ed_with_text("hi\n");
        assert!(ed.jump_to_mark('1').is_err());
    }

    #[test]
    fn jump_to_unset_mark_errors() {
        let mut ed = ed_with_text("hi\n");
        let err = ed.jump_to_mark('z').unwrap_err();
        assert!(err.contains("not set"));
    }

    #[test]
    fn uppercase_and_lowercase_are_distinct() {
        let mut ed = ed_with_text("line1\nline2\n");
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 0;
        }
        ed.set_mark('a').unwrap();
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 1;
        }
        ed.set_mark('A').unwrap();

        ed.jump_to_mark('a').unwrap();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 0);

        ed.jump_to_mark('A').unwrap();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 1);
    }

    #[test]
    fn jump_clamps_row_past_eof() {
        let mut ed = ed_with_text("aaa\nbbb\nccc\nddd\n");
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 3;
            win.cursor_col = 2;
        }
        ed.set_mark('e').unwrap();
        // Truncate the buffer to remove the `ddd` line entirely.
        let buf = &mut ed.buffers[0];
        let total = buf.rope().len_chars();
        let two_lines_end = buf.rope().line_to_char(2);
        buf.delete_range(two_lines_end, total);

        ed.jump_to_mark('e').unwrap();
        let win = ed.window_mgr.focused_window();
        // Row must be within the display line count (phantom line excluded).
        assert!(win.cursor_row < ed.buffers[0].display_line_count());
        assert!(win.cursor_row < 3, "was {}", win.cursor_row);
    }

    #[test]
    fn jump_clamps_col_past_eol() {
        let mut ed = ed_with_text("hello world\nhi\n");
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 10;
        }
        ed.set_mark('m').unwrap();
        // Jump into a shorter context by deleting most of line 0.
        let buf = &mut ed.buffers[0];
        buf.delete_range(2, 11); // "he\nhi\n"

        ed.jump_to_mark('m').unwrap();
        let win = ed.window_mgr.focused_window();
        assert_eq!(win.cursor_row, 0);
        // "he" is len 2, so col is clamped to 2.
        assert!(win.cursor_col <= 2);
    }

    #[test]
    fn scratch_buffer_mark_survives_same_scratch() {
        let mut ed = ed_with_text("scratch\n");
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 0;
            win.cursor_col = 5;
        }
        ed.set_mark('s').unwrap();

        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_col = 0;
        }
        ed.jump_to_mark('s').unwrap();
        assert_eq!(ed.window_mgr.focused_window().cursor_col, 5);
    }

    #[test]
    fn overwriting_mark_replaces_previous_position() {
        let mut ed = ed_with_text("line1\nline2\n");
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 0;
        }
        ed.set_mark('a').unwrap();
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 1;
        }
        ed.set_mark('a').unwrap();

        // Clear cursor, jump: should land at row 1 (the newer position).
        {
            let win = ed.window_mgr.focused_window_mut();
            win.cursor_row = 0;
        }
        ed.jump_to_mark('a').unwrap();
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 1);
    }
}
