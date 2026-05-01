//! Jump list (Practical Vim ch. 9).
//!
//! Vim's `Ctrl-o` / `Ctrl-i` navigation: each "significant" motion (search
//! accept, `G`/`gg`, `%`, paragraph jump, mark jump, LSP goto-def, diagnostic
//! jump, ...) pushes the *pre-motion* cursor position onto a per-editor
//! stack. `Ctrl-o` walks backward through the stack; `Ctrl-i` walks forward.
//!
//! Linear motions (`h`/`j`/`k`/`l`, `w`/`b`/`e`, etc.) do **not** push —
//! that's what distinguishes a "jump" from a "move" in vim semantics.
//!
//! Bounded to [`JUMP_LIST_CAP`] entries to avoid unbounded growth in long
//! sessions — matches vim's default.

use std::path::PathBuf;

use super::Editor;

/// Maximum number of entries to retain. Vim's default is also 100.
pub const JUMP_LIST_CAP: usize = 100;

/// A recorded cursor position. Carries the owning file's path so that
/// `Ctrl-o` can resurface on the correct buffer even after buffer kills /
/// reorderings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpEntry {
    /// Path of the buffer at push time. `None` for scratch buffers.
    pub path: Option<PathBuf>,
    /// Buffer index at push time — fallback for scratch buffers whose
    /// path is `None`. If the index is stale and the path doesn't match
    /// any open buffer, the entry is skipped.
    pub buffer_idx: usize,
    pub row: usize,
    pub col: usize,
}

impl Editor {
    /// Snapshot the focused window's cursor as a [`JumpEntry`].
    fn current_jump_entry(&self) -> JumpEntry {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        JumpEntry {
            path: self.buffers[idx].file_path().map(|p| p.to_path_buf()),
            buffer_idx: idx,
            row: win.cursor_row,
            col: win.cursor_col,
        }
    }

    /// Push the current cursor onto the jump list. Called at the top of
    /// each jump-worthy dispatch arm *before* the cursor moves.
    ///
    /// Dedupes against the immediately-previous entry so rapid-fire
    /// identical pushes (e.g. repeated `n` at EOF) don't bloat the list.
    /// Also truncates any "forward" history — pressing a jumping motion
    /// after `Ctrl-o` discards the redo stack, same as vim.
    pub fn record_jump(&mut self) {
        let entry = self.current_jump_entry();
        // Drop any forward history — new jump redefines the "future".
        self.jumps.truncate(self.jump_idx);
        // Dedupe against the most recent entry.
        if self.jumps.last() == Some(&entry) {
            return;
        }
        self.jumps.push(entry);
        // Enforce bound: drop from the front.
        if self.jumps.len() > JUMP_LIST_CAP {
            let overflow = self.jumps.len() - JUMP_LIST_CAP;
            self.jumps.drain(..overflow);
        }
        self.jump_idx = self.jumps.len();
    }

    /// `Ctrl-o` — navigate backward through the jump list. No-op at the
    /// oldest entry. On the first backward jump after a run of linear
    /// motions, pushes the current position so `Ctrl-i` can return.
    pub fn jump_backward(&mut self, n: usize) {
        for _ in 0..n {
            if self.jump_idx == 0 {
                return;
            }
            // First backward from the "present" — save where we are so
            // forward navigation can restore this spot.
            if self.jump_idx == self.jumps.len() {
                let current = self.current_jump_entry();
                if self.jumps.last() != Some(&current) {
                    self.jumps.push(current);
                    // jump_idx stays pointing at the original "past-end"
                    // slot, which is now the entry we just pushed.
                }
            }
            self.jump_idx -= 1;
            self.restore_jump_at_idx();
        }
    }

    /// `Ctrl-i` — navigate forward through the jump list. No-op at the
    /// newest entry.
    pub fn jump_forward(&mut self, n: usize) {
        for _ in 0..n {
            if self.jump_idx + 1 >= self.jumps.len() {
                return;
            }
            self.jump_idx += 1;
            self.restore_jump_at_idx();
        }
    }

    /// Move the focused window to `self.jumps[self.jump_idx]`.
    ///
    /// Resolves the entry's buffer via path first (so re-opened files
    /// still work), falling back to the stored index for scratch
    /// buffers. Clamps row/col in case the buffer shrank since push.
    /// If the target buffer is gone entirely, silently leaves the cursor
    /// where it is — the alternative (emitting an error) would be noisy
    /// for an operation users expect to be cheap.
    fn restore_jump_at_idx(&mut self) {
        let entry = self.jumps[self.jump_idx].clone();
        let target_idx = if let Some(ref path) = entry.path {
            self.buffers
                .iter()
                .position(|b| b.file_path().map(|p| p == path.as_path()).unwrap_or(false))
        } else if entry.buffer_idx < self.buffers.len()
            && self.buffers[entry.buffer_idx].file_path().is_none()
        {
            Some(entry.buffer_idx)
        } else {
            None
        };

        let Some(target_idx) = target_idx else {
            return;
        };

        // Switch buffer if necessary.
        {
            let win = self.window_mgr.focused_window_mut();
            if win.buffer_idx != target_idx {
                win.buffer_idx = target_idx;
            }
        }

        // Clamp to the buffer's current dimensions.
        let line_count = self.buffers[target_idx].display_line_count();
        let row = entry.row.min(line_count.saturating_sub(1));
        let col_max = self.buffers[target_idx].line_len(row);
        let col = entry.col.min(col_max);

        let vh = self.viewport_height;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
        win.scroll_center(vh);
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

    fn set_cursor(ed: &mut Editor, row: usize, col: usize) {
        let win = ed.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
    }

    #[test]
    fn record_jump_appends_entry() {
        let mut ed = ed_with_text("a\nb\nc\n");
        set_cursor(&mut ed, 0, 0);
        ed.record_jump();
        assert_eq!(ed.jumps.len(), 1);
        assert_eq!(ed.jump_idx, 1);
    }

    #[test]
    fn record_jump_dedupes_consecutive() {
        let mut ed = ed_with_text("a\nb\nc\n");
        set_cursor(&mut ed, 0, 0);
        ed.record_jump();
        ed.record_jump();
        assert_eq!(ed.jumps.len(), 1);
    }

    #[test]
    fn ctrl_o_restores_previous_position() {
        let mut ed = ed_with_text("line0\nline1\nline2\nline3\n");
        set_cursor(&mut ed, 0, 0);
        ed.record_jump();
        set_cursor(&mut ed, 3, 2);

        ed.jump_backward(1);
        let win = ed.window_mgr.focused_window();
        assert_eq!((win.cursor_row, win.cursor_col), (0, 0));
    }

    #[test]
    fn ctrl_i_returns_to_starting_position() {
        let mut ed = ed_with_text("line0\nline1\nline2\nline3\n");
        set_cursor(&mut ed, 0, 0);
        ed.record_jump();
        set_cursor(&mut ed, 3, 2);

        ed.jump_backward(1);
        ed.jump_forward(1);
        let win = ed.window_mgr.focused_window();
        assert_eq!((win.cursor_row, win.cursor_col), (3, 2));
    }

    #[test]
    fn ctrl_o_at_oldest_is_noop() {
        let mut ed = ed_with_text("a\nb\n");
        ed.jump_backward(1);
        let win = ed.window_mgr.focused_window();
        assert_eq!((win.cursor_row, win.cursor_col), (0, 0));
    }

    #[test]
    fn ctrl_i_at_newest_is_noop() {
        let mut ed = ed_with_text("line0\nline1\n");
        set_cursor(&mut ed, 0, 0);
        ed.record_jump();
        set_cursor(&mut ed, 1, 0);
        // With no Ctrl-o, jump_idx is already past-end — Ctrl-i does nothing.
        ed.jump_forward(1);
        let win = ed.window_mgr.focused_window();
        assert_eq!((win.cursor_row, win.cursor_col), (1, 0));
    }

    #[test]
    fn new_jump_truncates_forward_history() {
        let mut ed = ed_with_text("l0\nl1\nl2\nl3\n");
        set_cursor(&mut ed, 0, 0);
        ed.record_jump();
        set_cursor(&mut ed, 1, 0);
        ed.record_jump();
        set_cursor(&mut ed, 2, 0);
        ed.record_jump();
        set_cursor(&mut ed, 3, 0);

        // Walk back twice.
        ed.jump_backward(2);
        // Record a NEW jump — forward history (l2, l3) should drop.
        set_cursor(&mut ed, 0, 2);
        ed.record_jump();

        // Forward should be a no-op now.
        set_cursor(&mut ed, 3, 3);
        ed.jump_forward(1);
        let win = ed.window_mgr.focused_window();
        assert_eq!((win.cursor_row, win.cursor_col), (3, 3));
    }

    #[test]
    fn ctrl_o_twice_walks_back_through_history() {
        let mut ed = ed_with_text("l0\nl1\nl2\nl3\n");
        set_cursor(&mut ed, 0, 0);
        ed.record_jump();
        set_cursor(&mut ed, 1, 1);
        ed.record_jump();
        set_cursor(&mut ed, 2, 2);
        ed.record_jump();
        set_cursor(&mut ed, 3, 3);

        ed.jump_backward(1);
        let w = ed.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (2, 2));

        ed.jump_backward(1);
        let w = ed.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (1, 1));

        ed.jump_backward(1);
        let w = ed.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (0, 0));
    }

    #[test]
    fn jump_list_bounded() {
        let mut ed = ed_with_text("x\n");
        for i in 0..(JUMP_LIST_CAP + 10) {
            set_cursor(&mut ed, 0, i % 2);
            // Alternate col so dedupe doesn't collapse everything.
            ed.record_jump();
        }
        assert!(ed.jumps.len() <= JUMP_LIST_CAP);
    }

    #[test]
    fn jump_restore_clamps_past_eof() {
        let mut ed = ed_with_text("one\ntwo\nthree\nfour\n");
        set_cursor(&mut ed, 3, 2);
        ed.record_jump();
        set_cursor(&mut ed, 0, 0);

        // Delete the last two lines.
        let buf = &mut ed.buffers[0];
        let total = buf.rope().len_chars();
        let two_lines_end = buf.rope().line_to_char(2);
        buf.delete_range(two_lines_end, total);

        ed.jump_backward(1);
        let win = ed.window_mgr.focused_window();
        assert!(win.cursor_row < ed.buffers[0].display_line_count());
    }
}
