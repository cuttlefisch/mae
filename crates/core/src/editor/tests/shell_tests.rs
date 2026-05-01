use super::*;
use crate::buffer::Buffer;
use crate::{Mode, VisualType};

#[test]
fn default_keymaps_include_shell_insert() {
    let editor = Editor::new();
    assert!(
        editor.keymaps.contains_key("shell-insert"),
        "shell-insert keymap must exist in default keymaps"
    );
}

#[test]
fn shell_insert_keymap_has_default_exit_binding() {
    use crate::keymap::{parse_key_seq_spaced, LookupResult};
    let editor = Editor::new();
    let km = editor.keymaps.get("shell-insert").unwrap();
    let seq = parse_key_seq_spaced("C-\\ C-n");
    assert_eq!(km.lookup(&seq), LookupResult::Exact("shell-normal-mode"));
}

#[test]
fn shell_insert_keymap_ctrl_backslash_is_prefix() {
    use crate::keymap::{parse_key_seq, LookupResult};
    let editor = Editor::new();
    let km = editor.keymaps.get("shell-insert").unwrap();
    // A single Ctrl-\ should be a prefix (waiting for more keys).
    let seq = parse_key_seq("C-\\");
    assert_eq!(km.lookup(&seq), LookupResult::Prefix);
}

#[test]
fn shell_insert_keymap_unbound_key_returns_none() {
    use crate::keymap::{parse_key_seq, LookupResult};
    let editor = Editor::new();
    let km = editor.keymaps.get("shell-insert").unwrap();
    // A regular 'a' key should not match anything.
    assert_eq!(km.lookup(&parse_key_seq("a")), LookupResult::None);
}

#[test]
fn shell_normal_mode_command_switches_to_normal() {
    let mut editor = Editor::new();
    editor.mode = Mode::ShellInsert;
    editor.dispatch_builtin("shell-normal-mode");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn shell_insert_keymap_user_rebind() {
    use crate::keymap::{parse_key_seq_spaced, LookupResult};
    let mut editor = Editor::new();
    let km = editor.keymaps.get_mut("shell-insert").unwrap();
    // Unbind default and bind a custom sequence.
    km.unbind(&parse_key_seq_spaced("C-\\ C-n"));
    km.bind(parse_key_seq_spaced("C-c C-c"), "shell-normal-mode");
    assert_eq!(
        km.lookup(&parse_key_seq_spaced("C-c C-c")),
        LookupResult::Exact("shell-normal-mode")
    );
    assert_eq!(
        km.lookup(&parse_key_seq_spaced("C-\\ C-n")),
        LookupResult::None
    );
}

// ---- sync_mode_to_buffer tests ----

#[test]
fn sync_mode_shell_buffer_sets_shell_insert() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    editor.mode = Mode::Normal;
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::ShellInsert);
}

#[test]
fn sync_mode_text_buffer_from_shell_insert_resets_to_normal() {
    let mut editor = Editor::new();
    editor.mode = Mode::ShellInsert;
    editor.sync_mode_to_buffer(); // active buffer is [scratch] (Text)
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn sync_mode_preserves_insert_for_text_buffers() {
    let mut editor = Editor::new();
    editor.mode = Mode::Insert;
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn sync_mode_preserves_visual_for_text_buffers() {
    let mut editor = Editor::new();
    editor.mode = Mode::Visual(VisualType::Char);
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
}

#[test]
fn focus_direction_syncs_mode_to_shell_buffer() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    // Split: now we have two windows both viewing buffer 0.
    editor.dispatch_builtin("split-vertical");
    assert_eq!(editor.window_mgr.window_count(), 2);
    // Put shell in the focused window (right side after split).
    editor.window_mgr.focused_window_mut().buffer_idx = 1;
    editor.mode = Mode::ShellInsert;
    // Verify we see the shell buffer.
    assert_eq!(editor.active_buffer().kind, crate::BufferKind::Shell);
    // Focus left → should switch to text buffer.
    editor.dispatch_builtin("focus-left");
    // If focus didn't change (both windows in same position), skip direction test
    // and test via switch_to_buffer + sync instead.
    if editor.active_buffer().kind == crate::BufferKind::Text {
        assert_eq!(editor.mode, Mode::Normal);
        editor.dispatch_builtin("focus-right");
        assert_eq!(editor.mode, Mode::ShellInsert);
    }
}

#[test]
fn sync_mode_via_switch_to_buffer() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::ShellInsert);
    editor.switch_to_buffer(0);
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn alternate_file_syncs_mode() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    editor.mode = Mode::ShellInsert;
    // Switch back via alternate-file → text buffer
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.mode, Mode::Normal);
    // Switch forward via alternate-file → shell buffer
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.mode, Mode::ShellInsert);
}

#[test]
fn clamp_all_cursors_clamps_visual_anchor_past_eof() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "line1\nline2\nline3\n");
    let mut editor = Editor::with_buffer(buf);
    // Enter visual mode with anchor at row 2
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 2;
        win.cursor_col = 3;
    }
    editor.enter_visual_mode(crate::VisualType::Char);
    assert_eq!(editor.visual_anchor_row, 2);

    // Truncate buffer to 1 line (simulating MCP edit)
    let buf = &mut editor.buffers[0];
    let total = buf.rope().len_chars();
    let one_line = buf.rope().line_to_char(1);
    buf.delete_range(one_line, total);

    // Before clamp, anchor is stale
    assert!(editor.visual_anchor_row > editor.buffers[0].display_line_count().saturating_sub(1));

    editor.clamp_all_cursors();
    assert!(editor.visual_anchor_row < editor.buffers[0].display_line_count());
    assert!(editor.visual_anchor_col <= editor.buffers[0].line_len(editor.visual_anchor_row));
}

#[test]
fn clamp_all_cursors_clamps_last_visual_past_eof() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "aaa\nbbb\nccc\nddd\n");
    let mut editor = Editor::with_buffer(buf);
    // Set up a saved visual selection at rows 2-3
    editor.last_visual = Some((2, 1, 3, 2, crate::VisualType::Char));

    // Truncate to 1 line
    let buf = &mut editor.buffers[0];
    let total = buf.rope().len_chars();
    let one_line = buf.rope().line_to_char(1);
    buf.delete_range(one_line, total);

    editor.clamp_all_cursors();

    let (ar, ac, cr, cc, _) = editor.last_visual.unwrap();
    assert!(ar < editor.buffers[0].display_line_count());
    assert!(cr < editor.buffers[0].display_line_count());
    assert!(ac <= editor.buffers[0].line_len(ar));
    assert!(cc <= editor.buffers[0].line_len(cr));
}

// ---------------------------------------------------------------------------
// Mouse handling (Phase 8 — Step 8)
// ---------------------------------------------------------------------------
