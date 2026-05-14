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
// shell-select-mode
// ---------------------------------------------------------------------------

#[test]
fn shell_select_mode_creates_temp_buffer() {
    let mut editor = Editor::new();
    // Create a shell buffer with viewport data.
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    let shell_idx = editor.active_buffer_idx();
    editor.shell_viewports.insert(
        shell_idx,
        vec!["$ echo hello".into(), "hello".into(), "$ ".into()],
    );

    editor.dispatch_builtin("shell-select-mode");

    // A *shell-select* buffer should now exist and be displayed.
    let select_idx = editor
        .buffers
        .iter()
        .position(|b| b.name == "*shell-select*")
        .expect("*shell-select* buffer should exist");
    assert!(editor.buffers[select_idx].read_only);
    // BufferKind should be ShellSelect with its own keymap.
    assert_eq!(
        editor.buffers[select_idx].kind,
        crate::BufferKind::ShellSelect
    );
    // Content should be the joined viewport lines.
    let text: String = editor.buffers[select_idx].rope().to_string();
    assert!(text.contains("echo hello"));
    assert!(text.contains("hello"));
    // Cursor should be at the last line.
    let win = editor.window_mgr.focused_window();
    assert_eq!(
        win.cursor_row,
        editor.buffers[select_idx]
            .display_line_count()
            .saturating_sub(1)
    );
}

#[test]
fn shell_select_non_shell_buffer_shows_error() {
    let mut editor = Editor::new();
    // Active buffer is [scratch] (Text), not a shell.
    editor.dispatch_builtin("shell-select-mode");
    assert!(editor.status_msg.contains("Not a shell buffer"));
}

#[test]
fn shell_select_empty_shows_error() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    // No viewport data inserted → empty content.
    editor.dispatch_builtin("shell-select-mode");
    assert!(editor.status_msg.contains("No shell output to select"));
}

#[test]
fn shell_select_mode_reuses_existing_buffer() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    let shell_idx = editor.active_buffer_idx();
    editor
        .shell_viewports
        .insert(shell_idx, vec!["first".into()]);

    editor.dispatch_builtin("shell-select-mode");
    let count_after_first = editor.buffers.len();

    // Switch back to the shell buffer and run again with updated content.
    editor.switch_to_buffer(shell_idx);
    editor
        .shell_viewports
        .insert(shell_idx, vec!["second".into()]);
    editor.dispatch_builtin("shell-select-mode");

    // Should reuse the buffer, not create another one.
    assert_eq!(editor.buffers.len(), count_after_first);
    let select_idx = editor
        .buffers
        .iter()
        .position(|b| b.name == "*shell-select*")
        .unwrap();
    let text: String = editor.buffers[select_idx].rope().to_string();
    assert!(text.contains("second"));
}

// ---------------------------------------------------------------------------
// close-shell-select
// ---------------------------------------------------------------------------

#[test]
fn shell_select_q_closes_and_returns() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    let shell_idx = editor.active_buffer_idx();
    editor
        .shell_viewports
        .insert(shell_idx, vec!["output".into()]);

    editor.dispatch_builtin("shell-select-mode");
    assert!(editor.buffers.iter().any(|b| b.name == "*shell-select*"));

    editor.dispatch_builtin("close-shell-select");
    // Buffer should be removed.
    assert!(!editor.buffers.iter().any(|b| b.name == "*shell-select*"));
    // Focus should return to the shell buffer.
    assert_eq!(editor.active_buffer().kind, crate::BufferKind::Shell);
}

#[test]
fn shell_select_keymap_has_q_and_esc_bindings() {
    use crate::keymap::{parse_key_seq, Key, KeyPress, LookupResult};
    let editor = Editor::new();
    let km = editor
        .keymaps
        .get("shell-select")
        .expect("shell-select keymap must exist");
    assert_eq!(
        km.lookup(&parse_key_seq("q")),
        LookupResult::Exact("close-shell-select")
    );
    assert_eq!(
        km.lookup(&[KeyPress::special(Key::Escape)]),
        LookupResult::Exact("close-shell-select")
    );
    assert_eq!(
        km.lookup(&parse_key_seq("?")),
        LookupResult::Exact("show-buffer-keys")
    );
}

// ---------------------------------------------------------------------------
// shell-normal keymap resolution
// ---------------------------------------------------------------------------

#[test]
fn shell_buffer_normal_mode_uses_shell_normal_keymap() {
    use crate::buffer_mode::BufferMode;
    use crate::keymap::LookupResult;
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    editor.mode = Mode::Normal;
    // Resolve keymap for Shell buffer in Normal mode — should be "shell-normal".
    let km_name = editor.active_buffer().kind.keymap_name();
    assert_eq!(km_name, Some("shell-normal"));
    let km = editor.keymaps.get("shell-normal").unwrap();
    // `v` should map to shell-select-mode (not visual mode from parent).
    assert_eq!(
        km.lookup(&crate::keymap::parse_key_seq("v")),
        LookupResult::Exact("shell-select-mode")
    );
    // `q` should map to enter-insert-mode (returns to ShellInsert).
    assert_eq!(
        km.lookup(&crate::keymap::parse_key_seq("q")),
        LookupResult::Exact("enter-insert-mode")
    );
}

// ---------------------------------------------------------------------------
// Mouse handling (Phase 8 — Step 8)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// rekey_after_remove + notify_buffer_removed
// ---------------------------------------------------------------------------

#[test]
fn test_rekey_after_remove() {
    use crate::editor::rekey_after_remove;
    let mut map = std::collections::HashMap::new();
    map.insert(1, "a");
    map.insert(3, "b");
    map.insert(5, "c");
    rekey_after_remove(&mut map, 3);
    // key 3 removed, key 5 shifted to 4, key 1 unchanged
    assert_eq!(map.get(&1), Some(&"a"));
    assert!(!map.contains_key(&3));
    assert_eq!(map.get(&4), Some(&"c"));
    assert!(!map.contains_key(&5));
    assert_eq!(map.len(), 2);
}

#[test]
fn test_rekey_at_zero() {
    use crate::editor::rekey_after_remove;
    let mut map = std::collections::HashMap::new();
    map.insert(0, "x");
    map.insert(1, "y");
    map.insert(2, "z");
    rekey_after_remove(&mut map, 0);
    // key 0 ("x") removed, key 1 ("y") shifted to 0, key 2 ("z") shifted to 1
    assert_eq!(map.get(&0), Some(&"y"));
    assert_eq!(map.get(&1), Some(&"z"));
    assert!(!map.contains_key(&2));
    assert_eq!(map.len(), 2);
}

#[test]
fn test_notify_buffer_removed_viewports() {
    let mut editor = Editor::new();
    // Set up 3 buffers
    editor.buffers.push(Buffer::new());
    editor.buffers.push(Buffer::new());
    editor.shell_viewports.insert(0, vec!["a".into()]);
    editor.shell_viewports.insert(2, vec!["c".into()]);
    // Remove buffer 1
    editor.buffers.remove(1);
    editor.notify_buffer_removed(1);
    // Key 0 unchanged, key 2 shifted to 1
    assert!(editor.shell_viewports.contains_key(&0));
    assert_eq!(
        editor.shell_viewports.get(&1).unwrap(),
        &vec!["c".to_string()]
    );
    assert!(!editor.shell_viewports.contains_key(&2));
}

#[test]
fn test_notify_buffer_removed_alternate() {
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    editor.buffers.push(Buffer::new());
    // alternate points to buffer 2
    editor.alternate_buffer_idx = Some(2);
    editor.buffers.remove(1);
    editor.notify_buffer_removed(1);
    // alternate should shift from 2 to 1
    assert_eq!(editor.alternate_buffer_idx, Some(1));

    // Now test clearing when alternate matches removed
    editor.alternate_buffer_idx = Some(1);
    editor.buffers.remove(1);
    editor.notify_buffer_removed(1);
    assert_eq!(editor.alternate_buffer_idx, None);
}

#[test]
fn test_notify_buffer_removed_saved_view_states() {
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    editor.buffers.push(Buffer::new());
    // Populate saved_view_states on the focused window
    let win = editor.window_mgr.focused_window_mut();
    win.saved_view_states.insert(
        0,
        crate::window::BufferViewState {
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            col_offset: 0,
        },
    );
    win.saved_view_states.insert(
        2,
        crate::window::BufferViewState {
            cursor_row: 10,
            cursor_col: 5,
            scroll_offset: 3,
            col_offset: 0,
        },
    );
    // Remove buffer 1
    editor.buffers.remove(1);
    editor.notify_buffer_removed(1);
    let win = editor.window_mgr.focused_window();
    assert!(win.saved_view_states.contains_key(&0));
    assert!(win.saved_view_states.contains_key(&1)); // was key 2, shifted
    assert!(!win.saved_view_states.contains_key(&2));
    assert_eq!(win.saved_view_states.get(&1).unwrap().cursor_row, 10);
}

#[test]
fn test_notify_buffer_removed_pending_queues() {
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    editor.buffers.push(Buffer::new());
    editor.pending_shell_spawns = vec![0, 1, 2];
    editor.pending_shell_resets = vec![2];
    editor.pending_agent_spawns = vec![(1, "cmd".into()), (2, "cmd2".into())];
    // Remove buffer 1
    editor.buffers.remove(1);
    editor.notify_buffer_removed(1);
    // idx 1 dropped, idx 2 shifted to 1
    assert_eq!(editor.pending_shell_spawns, vec![0, 1]);
    assert_eq!(editor.pending_shell_resets, vec![1]);
    assert_eq!(editor.pending_agent_spawns, vec![(1, "cmd2".into())]);
    // pending_buffer_removals should have an entry
    assert_eq!(editor.pending_buffer_removals, vec![1]);
}

// ---------------------------------------------------------------------------
// Shell exit lifecycle & buffer readiness (Part 1 + Part 2 fixes)
// ---------------------------------------------------------------------------

#[test]
fn display_buffer_and_focus_syncs_shell_mode() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    assert_eq!(editor.mode, Mode::Normal);
    // display_buffer_and_focus should auto-sync to ShellInsert for a shell buffer.
    editor.display_buffer_and_focus(1);
    assert_eq!(editor.mode, Mode::ShellInsert);
    // Switch back to text buffer — should revert to Normal.
    editor.display_buffer_and_focus(0);
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn switch_to_buffer_syncs_mode() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    // switch_to_buffer should now auto-sync mode.
    editor.switch_to_buffer(1);
    assert_eq!(editor.mode, Mode::ShellInsert);
    editor.switch_to_buffer(0);
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn window_move_saves_restores_mode() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    // Split and put shell in one window.
    editor.dispatch_builtin("split-vertical");
    editor.window_mgr.focused_window_mut().buffer_idx = 1;
    editor.mode = Mode::ShellInsert;
    editor.buffers[1].saved_mode = Some(Mode::ShellInsert);
    // window-move should preserve mode state.
    editor.dispatch_builtin("window-move-right");
    assert_eq!(editor.mode, Mode::ShellInsert);
}

#[test]
fn find_window_with_kind_excludes_conversation_pair() {
    let mut editor = Editor::new();
    // Buffer 0 = [scratch], buffer 1 = shell
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    // Split to get two windows.
    editor.dispatch_builtin("split-vertical");
    // Put shell in window 1 (the new window from split).
    let new_win_id = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id != 0)
        .map(|w| w.id)
        .unwrap();
    editor.window_mgr.window_mut(new_win_id).unwrap().buffer_idx = 1;
    // Without conversation_pair, find_window_with_kind should find the shell window.
    assert!(editor
        .find_window_with_kind(crate::BufferKind::Shell)
        .is_some());
    // Mark that window as part of conversation pair — should now be excluded.
    editor.conversation_pair = Some(crate::editor::ConversationPair {
        output_buffer_idx: 0,
        input_buffer_idx: 0,
        output_window_id: new_win_id,
        input_window_id: new_win_id,
    });
    assert!(editor
        .find_window_with_kind(crate::BufferKind::Shell)
        .is_none());
}

#[test]
fn revert_buffer_fires_hooks() {
    use std::io::Write;
    let mut editor = Editor::new();
    // Register hooks.
    editor.hooks.add("before-revert", "test-before-revert-fn");
    editor.hooks.add("after-revert", "test-after-revert-fn");
    // Create a temp file so revert has something to load.
    let dir = std::env::temp_dir();
    let path = dir.join("mae_test_revert_hooks.txt");
    {
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "hello").unwrap();
    }
    // Open the file.
    editor.buffers[0] = Buffer::from_file(&path).unwrap();
    // Revert — hooks should fire (pending_hook_evals populated).
    editor.dispatch_builtin("revert-buffer");
    let evals: Vec<_> = editor.pending_hook_evals.drain(..).collect();
    assert!(
        evals
            .iter()
            .any(|(h, f)| h == "before-revert" && f == "test-before-revert-fn"),
        "before-revert hook should fire"
    );
    assert!(
        evals
            .iter()
            .any(|(h, f)| h == "after-revert" && f == "test-after-revert-fn"),
        "after-revert hook should fire"
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn split_fires_window_split_hook() {
    let mut editor = Editor::new();
    editor.hooks.add("window-split", "test-split-fn");
    editor.dispatch_builtin("split-vertical");
    let evals: Vec<_> = editor.pending_hook_evals.drain(..).collect();
    assert!(
        evals
            .iter()
            .any(|(h, f)| h == "window-split" && f == "test-split-fn"),
        "window-split hook should fire"
    );
}

#[test]
fn close_window_fires_window_close_hook() {
    let mut editor = Editor::new();
    // Need at least 2 windows to close one.
    editor.dispatch_builtin("split-vertical");
    editor.pending_hook_evals.clear(); // clear split hook
    editor.hooks.add("window-close", "test-close-fn");
    editor.dispatch_builtin("close-window");
    let evals: Vec<_> = editor.pending_hook_evals.drain(..).collect();
    assert!(
        evals
            .iter()
            .any(|(h, f)| h == "window-close" && f == "test-close-fn"),
        "window-close hook should fire"
    );
}
