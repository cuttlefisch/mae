use super::*;
use crate::keymap::parse_key_seq;
use crate::{LookupResult, Mode};

#[test]
fn change_word_forward_deletes_word_enters_insert() {
    let mut editor = editor_with_text("hello world test");
    editor.dispatch_builtin("change-word-forward");
    // "hello " should be deleted, leaving "world test"
    let text = editor.active_buffer().rope().to_string();
    assert!(text.starts_with("world test"));
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn change_to_line_end_deletes_to_eol_enters_insert() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5; // at the space
    editor.dispatch_builtin("change-to-line-end");
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "hello");
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn change_to_line_start_deletes_to_sol_enters_insert() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5; // at the space
    editor.dispatch_builtin("change-to-line-start");
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, " world");
    assert_eq!(editor.mode, Mode::Insert);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
}

// ===== Replace char tests =====

#[test]
fn replace_char_replaces_under_cursor() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_char_motion("replace-char", 'X');
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "Xello");
}

#[test]
fn replace_char_does_not_change_mode() {
    let mut editor = editor_with_text("hello");
    assert_eq!(editor.mode, Mode::Normal);
    editor.dispatch_char_motion("replace-char", 'X');
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn replace_char_at_end_of_line() {
    let mut editor = editor_with_text("hello");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 4; // at 'o'
    editor.dispatch_char_motion("replace-char", 'Z');
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "hellZ");
}

// ===== Dot repeat tests =====

#[test]
fn dot_repeats_delete_line() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("delete-line");
    assert_eq!(editor.active_buffer().rope().to_string(), "line2\nline3");
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.active_buffer().rope().to_string(), "line3");
}

#[test]
fn dot_repeats_delete_char() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("delete-char-forward");
    assert_eq!(editor.active_buffer().rope().to_string(), "ello");
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.active_buffer().rope().to_string(), "llo");
}

#[test]
fn dot_repeats_replace_char() {
    let mut editor = editor_with_text("abcde");
    editor.dispatch_char_motion("replace-char", 'X');
    assert_eq!(editor.active_buffer().rope().to_string(), "Xbcde");
    // Move right then repeat
    let buf = &editor.buffers[editor.active_buffer_idx()];
    editor.window_mgr.focused_window_mut().move_right(buf);
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.active_buffer().rope().to_string(), "XXcde");
}

#[test]
fn dot_repeats_change_word() {
    let mut editor = editor_with_text("hello world test");
    // Change word forward (deletes "hello ") and enters insert mode
    editor.dispatch_builtin("change-word-forward");
    assert_eq!(editor.mode, Mode::Insert);
    // Simulate typing "XX" in insert mode
    let idx = editor.active_buffer_idx();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[idx].insert_char(win, 'X');
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[idx].insert_char(win, 'X');
    // Exit insert mode
    editor.dispatch_builtin("enter-normal-mode");
    assert_eq!(editor.mode, Mode::Normal);
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "XXworld test");

    // Move cursor to 'w' (col 2) for the next word
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 2;
    // Now dot-repeat should change-word "world " and insert "XX"
    editor.dispatch_builtin("dot-repeat");
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "XXXXtest");
}

#[test]
fn dot_repeat_no_previous_does_nothing() {
    let mut editor = editor_with_text("hello");
    // No previous edit recorded
    editor.dispatch_builtin("dot-repeat");
    // Buffer should be unchanged
    assert_eq!(editor.active_buffer().rope().to_string(), "hello");
}

// ===== Keybinding tests =====

#[test]
fn normal_keymap_has_change_bindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap exists");
    // cc is linewise special (kept)
    assert_eq!(
        normal.lookup(&parse_key_seq("cc")),
        LookupResult::Exact("change-line")
    );
    // C is still directly bound
    assert_eq!(
        normal.lookup(&parse_key_seq("C")),
        LookupResult::Exact("change-to-line-end")
    );
    // c is now operator-pending (prefix because cc exists)
    assert_eq!(normal.lookup(&parse_key_seq("c")), LookupResult::Prefix);
}

#[test]
fn normal_keymap_has_replace_binding() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap exists");
    assert_eq!(
        normal.lookup(&parse_key_seq("r")),
        LookupResult::Exact("replace-char-await")
    );
}

#[test]
fn normal_keymap_has_dot_repeat_binding() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap exists");
    assert_eq!(
        normal.lookup(&parse_key_seq(".")),
        LookupResult::Exact("dot-repeat")
    );
}

#[test]
fn replace_char_await_sets_pending() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("replace-char-await");
    assert_eq!(
        editor.pending_char_command,
        Some("replace-char".to_string())
    );
}

// ===== Count prefix tests (Phase 3e M4) =====
