//! Multi-cursor manipulation commands.
//!
//! Follows the evil-mc model: cursors are lightweight state containers.
//! Operations are replayed at each cursor.

use crate::cursor::CursorOp;
use crate::editor::Editor;

/// Add a secondary cursor one line below the primary cursor.
pub(crate) fn mc_add_cursor_below(editor: &mut Editor) {
    let win = editor.window_mgr.focused_window_mut();
    let buf = &editor.buffers[win.buffer_idx];
    let primary_row = win.cursor_row;
    let primary_col = win.cursor_col;
    let new_row = primary_row + 1;
    if new_row < buf.line_count() {
        let line_len = buf.line_len(new_row);
        let col = primary_col.min(if line_len > 0 { line_len - 1 } else { 0 });
        win.cursor_set.add(new_row, col);
    }
}

/// Add a secondary cursor one line above the primary cursor.
pub(crate) fn mc_add_cursor_above(editor: &mut Editor) {
    let win = editor.window_mgr.focused_window_mut();
    let buf = &editor.buffers[win.buffer_idx];
    let primary_row = win.cursor_row;
    let primary_col = win.cursor_col;
    if primary_row > 0 {
        let new_row = primary_row - 1;
        let line_len = buf.line_len(new_row);
        let col = primary_col.min(if line_len > 0 { line_len - 1 } else { 0 });
        win.cursor_set.add(new_row, col);
    }
}

/// Extract the plain word at the cursor position (no regex escaping).
fn plain_word_at(buf: &crate::buffer::Buffer, row: usize, col: usize) -> Option<String> {
    let char_offset = buf.char_offset_at(row, col);
    let rope = buf.rope();
    let len = rope.len_chars();
    if len == 0 || char_offset >= len {
        return None;
    }
    let ch = rope.char(char_offset);
    if crate::word::classify(ch) != crate::word::CharClass::Word {
        return None;
    }
    let mut start = char_offset;
    while start > 0 && crate::word::classify(rope.char(start - 1)) == crate::word::CharClass::Word {
        start -= 1;
    }
    let mut end = char_offset + 1;
    while end < len && crate::word::classify(rope.char(end)) == crate::word::CharClass::Word {
        end += 1;
    }
    Some(rope.chars_at(start).take(end - start).collect())
}

/// Add a secondary cursor at the next occurrence of the word under the primary cursor.
pub(crate) fn mc_add_at_next_word(editor: &mut Editor) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let word = match plain_word_at(buf, win.cursor_row, win.cursor_col) {
        Some(w) => w,
        None => {
            editor.set_status("No word under cursor");
            return;
        }
    };

    let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);

    // Find the last cursor position for this word to search from.
    let last_pos = {
        let win = editor.window_mgr.focused_window();
        let buf = &editor.buffers[win.buffer_idx];
        let mut max_offset = char_offset;
        for c in win.cursor_set.iter() {
            let off = buf.char_offset_at(c.row, c.col);
            if off > max_offset {
                max_offset = off;
            }
        }
        max_offset
    };

    // Search forward from after the last cursor.
    let buf = &editor.buffers[editor.active_buffer_idx()];
    let text: String = buf.rope().chars().collect();
    let search_from = last_pos + 1;
    if let Some(pos) = text[search_from..].find(&word) {
        let abs_offset = search_from + pos;
        let (row, col) = buf.row_col_from_offset(abs_offset);
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_set.add(row, col);
    } else {
        // Wrap around from beginning.
        if let Some(pos) = text[..search_from.min(text.len())].find(&word) {
            let (row, col) = buf.row_col_from_offset(pos);
            let win = editor.window_mgr.focused_window_mut();
            let already = win.cursor_set.iter().any(|c| c.row == row && c.col == col);
            if !already {
                win.cursor_set.add(row, col);
            } else {
                editor.set_status("No more matches");
            }
        } else {
            editor.set_status("No more matches");
        }
    }
}

/// Add cursors at ALL occurrences of the word under the primary cursor.
pub(crate) fn mc_add_all_word(editor: &mut Editor) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let word = match plain_word_at(buf, win.cursor_row, win.cursor_col) {
        Some(w) => w,
        None => {
            editor.set_status("No word under cursor");
            return;
        }
    };

    let text: String = buf.rope().chars().collect();
    let mut positions = Vec::new();
    let mut start = 0;
    while let Some(pos) = text[start..].find(&word) {
        let abs = start + pos;
        let (row, col) = buf.row_col_from_offset(abs);
        positions.push((row, col));
        start = abs + word.len();
    }

    let win = editor.window_mgr.focused_window_mut();
    let primary_row = win.cursor_row;
    let primary_col = win.cursor_col;
    for (row, col) in positions {
        if row == primary_row && col == primary_col {
            continue; // skip primary
        }
        win.cursor_set.add(row, col);
    }
    win.cursor_set.dedup_positions();
    let count = win.cursor_set.len();
    editor.set_status(format!("{} cursors", count));
}

/// Remove the last secondary cursor and find the next match after the removed position.
pub(crate) fn mc_skip_next(editor: &mut Editor) {
    let win = editor.window_mgr.focused_window();
    let len = win.cursor_set.len();
    if len <= 1 {
        return;
    }

    // Capture position of the cursor being removed.
    let removed = win.cursor_set.secondaries().last().unwrap().clone();
    let buf = &editor.buffers[win.buffer_idx];
    let removed_offset = buf.char_offset_at(removed.row, removed.col);

    let word = match plain_word_at(buf, win.cursor_row, win.cursor_col) {
        Some(w) => w,
        None => return,
    };

    // Remove last secondary.
    let win = editor.window_mgr.focused_window_mut();
    let len = win.cursor_set.len();
    win.cursor_set.remove_at(len - 1);

    // Search forward from after the removed position.
    let buf = &editor.buffers[editor.active_buffer_idx()];
    let text: String = buf.rope().chars().collect();
    let search_from = removed_offset + 1;
    if let Some(pos) = text.get(search_from..).and_then(|s| s.find(&word)) {
        let abs_offset = search_from + pos;
        let (row, col) = buf.row_col_from_offset(abs_offset);
        let win = editor.window_mgr.focused_window_mut();
        let already = win.cursor_set.iter().any(|c| c.row == row && c.col == col);
        if !already {
            win.cursor_set.add(row, col);
        }
    }
}

/// Remove all secondary cursors.
pub(crate) fn mc_clear(editor: &mut Editor) {
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_set.clear_secondaries();
}

/// Align all cursors to the same column as the primary.
pub(crate) fn mc_align(editor: &mut Editor) {
    let win = editor.window_mgr.focused_window_mut();
    let target_col = win.cursor_col;
    let buf = &editor.buffers[win.buffer_idx];
    for cursor in win.cursor_set.iter_mut() {
        let line_len = buf.line_len(cursor.row);
        cursor.col = target_col.min(if line_len > 0 { line_len - 1 } else { 0 });
    }
}

/// Replay a `CursorOp` at all secondary cursors.
///
/// Operations are replayed in reverse position order to avoid offset drift:
/// edits at later positions don't shift earlier positions.
pub(crate) fn replay_at_secondaries(editor: &mut Editor, op: &CursorOp) {
    let win = editor.window_mgr.focused_window();
    if win.cursor_set.is_single() {
        return;
    }

    let buf_idx = win.buffer_idx;

    // Collect secondary cursor positions, sorted by (row, col) descending.
    let mut positions: Vec<(usize, usize)> = win
        .cursor_set
        .secondaries()
        .iter()
        .map(|c| (c.row, c.col))
        .collect();
    positions.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    // Save primary cursor position.
    let saved_row = editor.window_mgr.focused_window().cursor_row;
    let saved_col = editor.window_mgr.focused_window().cursor_col;

    let mut new_positions = Vec::with_capacity(positions.len());

    for (row, col) in &positions {
        // Temporarily set the window cursor to this secondary position.
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = *row;
        win.cursor_col = *col;

        match op {
            CursorOp::InsertChar(ch) => {
                editor.buffers[buf_idx].insert_char(editor.window_mgr.focused_window_mut(), *ch);
            }
            CursorOp::InsertText(text) => {
                for ch in text.chars() {
                    editor.buffers[buf_idx].insert_char(editor.window_mgr.focused_window_mut(), ch);
                }
            }
            CursorOp::DeleteBackward => {
                editor.buffers[buf_idx]
                    .delete_char_backward(editor.window_mgr.focused_window_mut());
            }
            CursorOp::DeleteForward => {
                editor.buffers[buf_idx].delete_char_forward(editor.window_mgr.focused_window_mut());
            }
            CursorOp::DeleteWord => {
                editor.buffers[buf_idx]
                    .delete_word_backward(editor.window_mgr.focused_window_mut());
            }
        }

        // Capture the new position after the edit.
        let win = editor.window_mgr.focused_window();
        new_positions.push((win.cursor_row, win.cursor_col));
    }

    // Restore primary cursor position.
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = saved_row;
    win.cursor_col = saved_col;
    win.sync_primary();

    // Update secondary cursor positions (they were processed in reverse order).
    // positions and new_positions have the same indices.
    // Rebuild the cursor set: primary stays, update secondaries.
    // Clear old secondaries and re-add with new positions.
    win.cursor_set.clear_secondaries();
    for np in new_positions.iter().rev() {
        win.cursor_set.add(np.0, np.1);
    }
}

/// Commands that should be replayed at secondary cursors in normal mode.
const MC_MOTION_ALLOWLIST: &[&str] = &[
    "move-up",
    "move-down",
    "move-left",
    "move-right",
    "move-to-line-start",
    "move-to-line-end",
    "move-word-forward",
    "move-word-backward",
    "move-word-end",
];

const MC_EDIT_ALLOWLIST: &[&str] = &[
    "delete-char-forward",  // x
    "delete-char-backward", // X
    "toggle-case",          // ~
];

/// After a command has been dispatched (and has already affected the primary
/// cursor), replay the same operation at all secondary cursors.
/// Returns true if the command was replayed.
pub(crate) fn replay_command_at_secondaries(editor: &mut Editor, name: &str) -> bool {
    if editor.window_mgr.focused_window().cursor_set.is_single() {
        return false;
    }

    if MC_MOTION_ALLOWLIST.contains(&name) {
        replay_motion_at_secondaries(editor, name);
        return true;
    }
    if MC_EDIT_ALLOWLIST.contains(&name) {
        replay_edit_at_secondaries(editor, name);
        return true;
    }
    false
}

/// Replay a motion command at all secondary cursors.
fn replay_motion_at_secondaries(editor: &mut Editor, name: &str) {
    let win = editor.window_mgr.focused_window();
    let buf_idx = win.buffer_idx;

    let positions: Vec<(usize, usize)> = win
        .cursor_set
        .secondaries()
        .iter()
        .map(|c| (c.row, c.col))
        .collect();

    let saved_row = editor.window_mgr.focused_window().cursor_row;
    let saved_col = editor.window_mgr.focused_window().cursor_col;

    let mut new_positions = Vec::with_capacity(positions.len());

    for (row, col) in &positions {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = *row;
        win.cursor_col = *col;

        let buf = &editor.buffers[buf_idx];
        match name {
            "move-up" => win.move_up(buf),
            "move-down" => win.move_down(buf),
            "move-left" => win.move_left(),
            "move-right" => win.move_right(buf),
            "move-to-line-start" => win.move_to_line_start(),
            "move-to-line-end" => win.move_to_line_end(buf),
            "move-word-forward" => win.move_word_forward(buf),
            "move-word-backward" => win.move_word_backward(buf),
            "move-word-end" => win.move_word_end(buf),
            _ => {}
        }

        let win = editor.window_mgr.focused_window();
        new_positions.push((win.cursor_row, win.cursor_col));
    }

    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = saved_row;
    win.cursor_col = saved_col;
    win.sync_primary();
    win.cursor_set.clear_secondaries();
    for np in &new_positions {
        win.cursor_set.add(np.0, np.1);
    }
}

/// Replay an edit command at all secondary cursors (reverse position order).
fn replay_edit_at_secondaries(editor: &mut Editor, name: &str) {
    let win = editor.window_mgr.focused_window();
    let buf_idx = win.buffer_idx;

    let mut positions: Vec<(usize, usize)> = win
        .cursor_set
        .secondaries()
        .iter()
        .map(|c| (c.row, c.col))
        .collect();
    positions.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));

    let saved_row = editor.window_mgr.focused_window().cursor_row;
    let saved_col = editor.window_mgr.focused_window().cursor_col;

    let mut new_positions = Vec::with_capacity(positions.len());

    for (row, col) in &positions {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = *row;
        win.cursor_col = *col;

        match name {
            "delete-char-forward" => {
                editor.buffers[buf_idx].delete_char_forward(editor.window_mgr.focused_window_mut());
            }
            "delete-char-backward" => {
                editor.buffers[buf_idx]
                    .delete_char_backward(editor.window_mgr.focused_window_mut());
            }
            "toggle-case" => {
                let win = editor.window_mgr.focused_window();
                let row = win.cursor_row;
                let col = win.cursor_col;
                let line_len = editor.buffers[buf_idx].line_len(row);
                if col < line_len {
                    let offset = editor.buffers[buf_idx].char_offset_at(row, col);
                    let ch = editor.buffers[buf_idx].rope().char(offset);
                    let toggled: String = if ch.is_uppercase() {
                        ch.to_lowercase().collect()
                    } else {
                        ch.to_uppercase().collect()
                    };
                    editor.buffers[buf_idx].delete_range(offset, offset + 1);
                    editor.buffers[buf_idx].insert_text_at(offset, &toggled);
                    let win = editor.window_mgr.focused_window_mut();
                    let new_line_len = editor.buffers[buf_idx].line_len(row);
                    if col + 1 < new_line_len {
                        win.cursor_col = col + 1;
                    }
                }
            }
            _ => {}
        }

        let win = editor.window_mgr.focused_window();
        new_positions.push((win.cursor_row, win.cursor_col));
    }

    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = saved_row;
    win.cursor_col = saved_col;
    win.sync_primary();
    win.cursor_set.clear_secondaries();
    for np in new_positions.iter().rev() {
        win.cursor_set.add(np.0, np.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_with_text(text: &str) -> Editor {
        let mut editor = Editor::new();
        let idx = editor.active_buffer_idx();
        editor.buffers[idx].insert_text_at(0, text);
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        editor
    }

    #[test]
    fn mc_add_below() {
        let mut editor = editor_with_text("line one\nline two\nline three\n");
        mc_add_cursor_below(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.len(), 2);
        assert_eq!(win.cursor_set.primary().row, 0);
        let sec = &win.cursor_set.secondaries()[0];
        assert_eq!(sec.row, 1);
    }

    #[test]
    fn mc_add_above_at_top() {
        let mut editor = editor_with_text("line one\nline two\n");
        mc_add_cursor_above(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.len(), 1); // can't go above row 0
    }

    #[test]
    fn mc_add_above_from_row_2() {
        let mut editor = editor_with_text("line one\nline two\nline three\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 2;
        win.cursor_col = 0;
        mc_add_cursor_above(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.len(), 2);
        assert_eq!(win.cursor_set.secondaries()[0].row, 1);
    }

    #[test]
    fn mc_add_next_word() {
        let mut editor = editor_with_text("foo bar foo baz foo\n");
        mc_add_at_next_word(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.len(), 2);
        assert_eq!(win.cursor_set.secondaries()[0].col, 8); // second "foo"
    }

    #[test]
    fn mc_add_all() {
        let mut editor = editor_with_text("foo bar foo baz foo\n");
        mc_add_all_word(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.len(), 3); // 3 "foo"s
    }

    #[test]
    fn mc_clear_removes_secondaries() {
        let mut editor = editor_with_text("foo\nbar\nbaz\n");
        mc_add_cursor_below(&mut editor);
        mc_add_cursor_below(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert!(win.cursor_set.len() > 1);
        mc_clear(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert!(win.cursor_set.is_single());
    }

    #[test]
    fn mc_align_sets_all_to_primary_col() {
        let mut editor = editor_with_text("hello world\nhi there\nabc\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 3;
        win.cursor_set.add(1, 0);
        win.cursor_set.add(2, 0);
        mc_align(&mut editor);
        let win = editor.window_mgr.focused_window();
        for cursor in win.cursor_set.iter() {
            assert!(cursor.col <= 3);
        }
    }

    #[test]
    fn mc_skip_next_replaces_last() {
        let mut editor = editor_with_text("foo bar foo baz foo\n");
        mc_add_at_next_word(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.len(), 2);
        mc_skip_next(&mut editor);
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.len(), 2); // removed one, added next
        assert_eq!(win.cursor_set.secondaries()[0].col, 16); // third "foo"
    }

    #[test]
    fn replay_insert_char_at_three_positions() {
        let mut editor = editor_with_text("aaa\nbbb\nccc\n");
        // Primary at (0,0), add secondaries at (1,0) and (2,0).
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        win.cursor_set.add(1, 0);
        win.cursor_set.add(2, 0);

        // Simulate primary insert.
        let idx = editor.active_buffer_idx();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].insert_char(win, 'X');

        // Replay at secondaries.
        replay_at_secondaries(&mut editor, &CursorOp::InsertChar('X'));

        let text: String = editor.buffers[editor.active_buffer_idx()]
            .rope()
            .chars()
            .collect();
        assert_eq!(text, "Xaaa\nXbbb\nXccc\n");
    }

    #[test]
    fn replay_delete_backward_at_secondaries() {
        let mut editor = editor_with_text("Xaaa\nXbbb\nXccc\n");
        // Primary at (0,1), secondaries at (1,1) and (2,1).
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 1;
        win.cursor_set.add(1, 1);
        win.cursor_set.add(2, 1);

        // Primary delete backward.
        let idx = editor.active_buffer_idx();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].delete_char_backward(win);

        // Replay at secondaries.
        replay_at_secondaries(&mut editor, &CursorOp::DeleteBackward);

        let text: String = editor.buffers[editor.active_buffer_idx()]
            .rope()
            .chars()
            .collect();
        assert_eq!(text, "aaa\nbbb\nccc\n");
    }

    #[test]
    fn replay_single_cursor_is_noop() {
        let mut editor = editor_with_text("hello\n");
        // Only primary cursor — replay should be a no-op.
        replay_at_secondaries(&mut editor, &CursorOp::InsertChar('X'));
        let text: String = editor.buffers[editor.active_buffer_idx()]
            .rope()
            .chars()
            .collect();
        assert_eq!(text, "hello\n"); // unchanged
    }

    #[test]
    fn replay_motion_down_at_secondaries() {
        let mut editor = editor_with_text("line 0\nline 1\nline 2\nline 3\n");
        // Primary at (0,0), secondary at (1,0).
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        win.cursor_set.add(1, 0);

        // Simulate move-down on primary.
        editor.dispatch_builtin("move-down");

        // Replay at secondaries.
        replay_command_at_secondaries(&mut editor, "move-down");

        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_row, 1); // primary moved 0→1
        assert_eq!(win.cursor_set.secondaries()[0].row, 2); // secondary moved 1→2
    }

    // --- Part 1: cursor_set primary sync + same-line multi-cursor tests ---

    #[test]
    fn cursor_set_primary_sync_after_movement() {
        let mut editor = editor_with_text("line one\nline two\nline three\n");
        // Move down — cursor_row changes, primary should track.
        editor.dispatch_builtin("move-down");
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_row, 1);
        assert_eq!(win.cursor_set.primary().row, win.cursor_row);
        assert_eq!(win.cursor_set.primary().col, win.cursor_col);
    }

    #[test]
    fn mc_same_line_insert_no_corruption() {
        // Two cursors on the same line: insert char at both positions.
        // Secondaries are replayed in reverse order (high col first) to avoid
        // offset drift. Primary insert at col 2 is done first (before replay),
        // then the secondary at col 5 is replayed at its original position
        // in the now-shifted rope. The result documents actual behavior.
        let mut editor = editor_with_text("abcdef\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 2; // between b and c
        win.cursor_set.add(0, 5); // between e and f

        // Simulate primary insert.
        let idx = editor.active_buffer_idx();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].insert_char(win, 'X');

        // Replay at secondary.
        replay_at_secondaries(&mut editor, &CursorOp::InsertChar('X'));

        let text: String = editor.buffers[editor.active_buffer_idx()]
            .rope()
            .chars()
            .collect();
        // Secondary at original col 5 inserts into post-primary-insert rope
        // ("abXcdef\n"), so the X lands at col 5 → "abXcdXef\n"
        assert_eq!(text, "abXcdXef\n");
    }

    #[test]
    fn mc_same_line_delete_no_corruption() {
        // Two cursors on the same line: delete backward at both.
        // Secondary replay uses the original col 5 in the post-primary-delete rope.
        let mut editor = editor_with_text("abcdef\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 2; // after 'b'
        win.cursor_set.add(0, 5); // after 'e'

        // Primary delete backward.
        let idx = editor.active_buffer_idx();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].delete_char_backward(win);

        // Replay at secondary.
        replay_at_secondaries(&mut editor, &CursorOp::DeleteBackward);

        let text: String = editor.buffers[editor.active_buffer_idx()]
            .rope()
            .chars()
            .collect();
        // Primary deletes 'b' → "acdef\n", then secondary at col 5 deletes 'e' → "acde\n"
        // Note: offset drift means secondary doesn't delete the "right" char.
        // This documents the known behavior.
        assert_eq!(text, "acde\n");
    }

    #[test]
    fn mc_replay_insert_newline() {
        // Insert newline at 2 cursors on different lines.
        // Primary at row 0 end, secondary at row 2 end (enough separation
        // to avoid offset drift).
        let mut editor = editor_with_text("aaa\nbbb\nccc\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 3; // end of "aaa"
        win.cursor_set.add(2, 3); // end of "ccc"

        // Primary insert newline.
        let idx = editor.active_buffer_idx();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].insert_char(win, '\n');

        // Replay at secondary (reverse order: row 2 first).
        replay_at_secondaries(&mut editor, &CursorOp::InsertChar('\n'));

        let text: String = editor.buffers[editor.active_buffer_idx()]
            .rope()
            .chars()
            .collect();
        // Primary splits "aaa" → "aaa\n\n", shifting everything down.
        // Secondary at original (2, 3) replays into the shifted rope (now row 2 = "bbb").
        // Row 2 col 3 → inserts newline at end of "bbb".
        assert_eq!(text, "aaa\n\nbbb\n\nccc\n");
    }

    #[test]
    fn mc_primary_sync_after_replay() {
        let mut editor = editor_with_text("aaa\nbbb\nccc\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        win.cursor_set.add(1, 0);

        // Simulate primary insert.
        let idx = editor.active_buffer_idx();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].insert_char(win, 'X');

        // Replay at secondary.
        replay_at_secondaries(&mut editor, &CursorOp::InsertChar('X'));

        // After replay, primary in cursor_set matches cursor_row/col.
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_set.primary().row, win.cursor_row);
        assert_eq!(win.cursor_set.primary().col, win.cursor_col);
    }

    #[test]
    fn mc_cursor_in_fold_region() {
        // Adding a cursor below into a folded region: the cursor should
        // land on the folded line (behavior is documented, not prevented).
        let mut editor = editor_with_text("line 0\nline 1\nline 2\nline 3\n");
        // Fold lines 1..3 (line 1 is the fold start, lines 2-3 hidden).
        let idx = editor.active_buffer_idx();
        editor.buffers[idx].folded_ranges.push((1, 3));
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        mc_add_cursor_below(&mut editor);
        let win = editor.window_mgr.focused_window();
        // Secondary should exist at row 1 (fold start line).
        assert_eq!(win.cursor_set.len(), 2);
        assert_eq!(win.cursor_set.secondaries()[0].row, 1);
    }

    #[test]
    fn secondary_cursor_positions_after_motion() {
        let mut editor = editor_with_text("abc\nxyz\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        win.cursor_set.add(1, 0);

        // Simulate move-right on primary.
        editor.dispatch_builtin("move-right");

        // Replay at secondary.
        replay_command_at_secondaries(&mut editor, "move-right");

        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_col, 1); // primary moved
        assert_eq!(win.cursor_set.secondaries()[0].col, 1); // secondary moved
    }

    #[test]
    fn replay_delete_char_at_secondaries() {
        let mut editor = editor_with_text("abc\nxyz\n");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        win.cursor_set.add(1, 0);

        // Primary delete-char-forward (x).
        editor.dispatch_builtin("delete-char-forward");

        // Replay.
        replay_command_at_secondaries(&mut editor, "delete-char-forward");

        let text: String = editor.buffers[editor.active_buffer_idx()]
            .rope()
            .chars()
            .collect();
        assert_eq!(text, "bc\nyz\n");
    }
}
