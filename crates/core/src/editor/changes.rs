//! Change list (Practical Vim ch. 9 — "Traverse the Change List").
//!
//! Vim tracks the position of every recent edit and exposes `g;` (backward)
//! and `g,` (forward) to navigate through those positions. `:changes`
//! prints the list for inspection.
//!
//! Structurally analogous to [`super::jumps`]: a bounded `Vec<ChangeEntry>`
//! with an index cursor, dedupe-on-push, and buffer-path resolution on
//! restore. The distinction is *what triggers a push*:
//!
//! - Jump list: significant motions (`G`, `/`, `%`, marks, LSP jumps, …)
//! - Change list: edits (`record_edit`, `finalize_insert_for_repeat`)
//!
//! So `Ctrl-o` takes you back through where you *looked*, and `g;` takes
//! you back through where you *changed* — a surprisingly useful
//! distinction when you've been jumping around a file between edits.

use std::path::PathBuf;

use super::Editor;

/// Maximum number of entries. Vim's default is 500; 100 mirrors our
/// [`super::jumps::JUMP_LIST_CAP`] for consistency and keeps memory
/// predictable in long sessions.
pub const CHANGE_LIST_CAP: usize = 100;

/// A recorded edit location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEntry {
    /// Path of the buffer at record time. `None` for scratch buffers.
    pub path: Option<PathBuf>,
    /// Buffer index at record time; fallback for scratch buffers.
    pub buffer_idx: usize,
    pub row: usize,
    pub col: usize,
}

impl Editor {
    /// Snapshot the focused window's cursor as a [`ChangeEntry`].
    fn current_change_entry(&self) -> ChangeEntry {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        ChangeEntry {
            path: self.buffers[idx].file_path().map(|p| p.to_path_buf()),
            buffer_idx: idx,
            row: win.cursor_row,
            col: win.cursor_col,
        }
    }

    /// Append the current cursor position to the change list.
    ///
    /// Called from each edit-recording entry point (`record_edit`,
    /// `record_edit_with_count`, `finalize_insert_for_repeat`). Dedupes
    /// against the most recent entry and truncates any forward history
    /// — editing invalidates the redo stack the same way it does in
    /// vim's native implementation.
    ///
    /// The dedupe check compares the `(buffer_idx, row, col)` tuple
    /// before allocating a `PathBuf` — this is a hot path (every edit,
    /// keystroke-exit) and the path is redundant with the buffer index
    /// within a single session.
    pub(crate) fn record_change(&mut self) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let row = win.cursor_row;
        let col = win.cursor_col;
        self.vi.changes.truncate(self.vi.change_idx);
        if let Some(last) = self.vi.changes.last() {
            if last.buffer_idx == idx && last.row == row && last.col == col {
                return;
            }
        }
        // Only materialize the path clone when we're actually going to push.
        let path = self.buffers[idx].file_path().map(|p| p.to_path_buf());
        self.vi.changes.push(ChangeEntry {
            path,
            buffer_idx: idx,
            row,
            col,
        });
        if self.vi.changes.len() > CHANGE_LIST_CAP {
            let overflow = self.vi.changes.len() - CHANGE_LIST_CAP;
            self.vi.changes.drain(..overflow);
        }
        self.vi.change_idx = self.vi.changes.len();
    }

    /// `g;` — navigate backward through the change list. No-op at the
    /// oldest entry.
    ///
    /// Like the jump list, the first backward step after a run of
    /// non-edit motions pushes the current position so `g,` can return.
    pub fn change_backward(&mut self, n: usize) {
        for _ in 0..n {
            if self.vi.change_idx == 0 {
                self.set_status("At oldest change");
                return;
            }
            if self.vi.change_idx == self.vi.changes.len() {
                let current = self.current_change_entry();
                if self.vi.changes.last() != Some(&current) {
                    self.vi.changes.push(current);
                }
            }
            self.vi.change_idx -= 1;
            self.restore_change_at_idx();
        }
    }

    /// `g,` — navigate forward through the change list. No-op at the
    /// newest entry.
    pub fn change_forward(&mut self, n: usize) {
        for _ in 0..n {
            if self.vi.change_idx + 1 >= self.vi.changes.len() {
                self.set_status("At newest change");
                return;
            }
            self.vi.change_idx += 1;
            self.restore_change_at_idx();
        }
    }

    /// Move the focused window to `self.vi.changes[self.vi.change_idx]`.
    ///
    /// Mirrors `restore_jump_at_idx`: resolve by path first so re-opened
    /// files still work, fall back to the stored index for scratch
    /// buffers, clamp past-EOF positions.
    fn restore_change_at_idx(&mut self) {
        let entry = self.vi.changes[self.vi.change_idx].clone();
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

        if self.window_mgr.focused_window().buffer_idx != target_idx {
            if self.is_conversation_buffer(self.active_buffer_idx()) {
                self.display_buffer_and_focus(target_idx);
            } else {
                self.window_mgr.focused_window_mut().buffer_idx = target_idx;
            }
        }

        let line_count = self.buffers[target_idx].display_line_count();
        let row = entry.row.min(line_count.saturating_sub(1));
        let col_max = self.buffers[target_idx].line_len(row);
        let col = entry.col.min(col_max);

        let vh = self.focused_viewport_height();
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
        win.scroll_center(vh);
    }

    /// Open `*Changes*` scratch buffer listing recorded change positions.
    ///
    /// Mirrors `:jumps` convention: newest-first so the most recent
    /// edits appear at top. The current `change_idx` is marked with
    /// `>` in the leftmost column.
    pub fn show_changes_buffer(&mut self) {
        let mut body = String::new();
        body.push_str(&format!(
            "*Changes*  {} entries  (idx {})\n\n",
            self.vi.changes.len(),
            self.vi.change_idx
        ));
        if self.vi.changes.is_empty() {
            body.push_str("No recorded changes.\n");
        } else {
            body.push_str("    # line  col  file\n");
            // Show newest at top — iterate in reverse with 0 = newest.
            for (i, entry) in self.vi.changes.iter().enumerate().rev() {
                let marker = if i == self.vi.change_idx { ">" } else { " " };
                let display_path = entry
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| format!("[buffer {}]", entry.buffer_idx));
                // Offset from newest so users can eyeball "g; N times".
                let offset = self.vi.changes.len().saturating_sub(1) - i;
                body.push_str(&format!(
                    "{}  {:3}  {:4}  {:3}  {}\n",
                    marker,
                    offset,
                    entry.row + 1,
                    entry.col + 1,
                    display_path
                ));
            }
        }

        let existing = self.buffers.iter().position(|b| b.name == "*Changes*");
        let idx = if let Some(i) = existing {
            self.buffers[i].replace_contents(&body);
            i
        } else {
            let mut buf = crate::buffer::Buffer::new();
            buf.replace_contents(&body);
            buf.name = "*Changes*".into();
            self.buffers.push(buf);
            self.buffers.len() - 1
        };
        self.display_buffer(idx);
        self.set_status(format!("Changes: {} entries", self.vi.changes.len()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn editor_with_bulk_text(s: &str) -> Editor {
        let mut buf = Buffer::new();
        buf.insert_text_at(0, s);
        Editor::with_buffer(buf)
    }

    fn set_cursor(editor: &mut Editor, row: usize, col: usize) {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = row;
        win.cursor_col = col;
    }

    #[test]
    fn record_change_appends_entry() {
        let mut editor = editor_with_bulk_text("a\nb\nc\n");
        set_cursor(&mut editor, 1, 0);
        editor.record_change();
        assert_eq!(editor.vi.changes.len(), 1);
        assert_eq!(editor.vi.change_idx, 1);
    }

    #[test]
    fn record_change_dedupes_consecutive() {
        let mut editor = editor_with_bulk_text("a\nb\n");
        editor.record_change();
        editor.record_change();
        assert_eq!(editor.vi.changes.len(), 1);
    }

    #[test]
    fn g_semi_walks_back_through_edits() {
        let mut editor = editor_with_bulk_text("a\nb\nc\nd\n");
        set_cursor(&mut editor, 0, 0);
        editor.record_change();
        set_cursor(&mut editor, 1, 0);
        editor.record_change();
        set_cursor(&mut editor, 2, 0);
        editor.record_change();

        // Simulate moving the cursor (not an edit) then g;
        set_cursor(&mut editor, 3, 0);
        editor.change_backward(1);
        let w = editor.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (2, 0));

        editor.change_backward(1);
        let w = editor.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (1, 0));
    }

    #[test]
    fn g_comma_returns_forward() {
        let mut editor = editor_with_bulk_text("aaaaaaa\nbbbbbbb\nccccccc\n");
        set_cursor(&mut editor, 0, 0);
        editor.record_change();
        set_cursor(&mut editor, 1, 0);
        editor.record_change();
        set_cursor(&mut editor, 2, 5);

        editor.change_backward(1);
        editor.change_forward(1);
        let w = editor.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (2, 5));
    }

    #[test]
    fn change_backward_at_oldest_is_noop() {
        let mut editor = editor_with_bulk_text("a\nb\n");
        editor.change_backward(1);
        let w = editor.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (0, 0));
    }

    #[test]
    fn new_edit_truncates_forward_history() {
        let mut editor = editor_with_bulk_text("a\nb\nc\nd\n");
        set_cursor(&mut editor, 0, 0);
        editor.record_change();
        set_cursor(&mut editor, 1, 0);
        editor.record_change();
        set_cursor(&mut editor, 2, 0);
        editor.record_change();

        editor.change_backward(2);
        // New edit here discards the two forward entries.
        set_cursor(&mut editor, 3, 1);
        editor.record_change();

        // g, should be a no-op — no forward history.
        set_cursor(&mut editor, 0, 0);
        editor.change_forward(1);
        let w = editor.window_mgr.focused_window();
        assert_eq!((w.cursor_row, w.cursor_col), (0, 0));
    }

    #[test]
    fn change_list_bounded() {
        let mut editor = editor_with_bulk_text("x\n");
        for i in 0..(CHANGE_LIST_CAP + 10) {
            set_cursor(&mut editor, 0, i % 2);
            editor.record_change();
        }
        assert!(editor.vi.changes.len() <= CHANGE_LIST_CAP);
    }

    #[test]
    fn show_changes_buffer_empty() {
        let mut editor = editor_with_bulk_text("a\n");
        editor.show_changes_buffer();
        let buf = editor
            .buffers
            .iter()
            .find(|b| b.name == "*Changes*")
            .unwrap();
        assert!(buf.text().contains("No recorded changes"));
    }

    #[test]
    fn show_changes_buffer_lists_entries() {
        let mut editor = editor_with_bulk_text("a\nb\nc\n");
        set_cursor(&mut editor, 0, 0);
        editor.record_change();
        set_cursor(&mut editor, 2, 1);
        editor.record_change();
        editor.show_changes_buffer();
        let buf = editor
            .buffers
            .iter()
            .find(|b| b.name == "*Changes*")
            .unwrap();
        let text = buf.text();
        assert!(text.contains("2 entries"));
        // Both rows visible (1-indexed).
        assert!(text.contains("   1 "));
        assert!(text.contains("   3 "));
    }

    #[test]
    fn restore_change_clamps_past_eof() {
        let mut editor = editor_with_bulk_text("one\ntwo\nthree\n");
        set_cursor(&mut editor, 2, 3);
        editor.record_change();
        set_cursor(&mut editor, 0, 0);

        // Truncate the buffer so the recorded row no longer exists.
        let buf = &mut editor.buffers[0];
        let total = buf.rope().len_chars();
        let trim = buf.rope().line_to_char(1);
        buf.delete_range(trim, total);

        editor.change_backward(1);
        let w = editor.window_mgr.focused_window();
        assert!(w.cursor_row < editor.buffers[0].display_line_count());
    }
}
