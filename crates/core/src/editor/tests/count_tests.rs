use super::*;

#[test]
fn count_prefix_default_none() {
    let editor = Editor::new();
    assert_eq!(editor.count_prefix, None);
}

#[test]
fn take_count_default_is_1() {
    let mut editor = Editor::new();
    assert_eq!(editor.take_count(), 1);
}

#[test]
fn take_count_returns_and_clears() {
    let mut editor = Editor::new();
    editor.count_prefix = Some(5);
    assert_eq!(editor.take_count(), 5);
    assert_eq!(editor.count_prefix, None);
}

#[test]
fn move_down_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5\n");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-down");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 3);
}

#[test]
fn move_up_with_count_clamps() {
    let mut editor = editor_with_text("line1\nline2\nline3\n");
    editor.window_mgr.focused_window_mut().cursor_row = 2;
    editor.count_prefix = Some(10); // more than available
    editor.dispatch_builtin("move-up");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
}

#[test]
fn move_right_with_count() {
    let mut editor = editor_with_text("hello world");
    editor.count_prefix = Some(5);
    editor.dispatch_builtin("move-right");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 5);
}

#[test]
fn move_left_with_count() {
    let mut editor = editor_with_text("hello world");
    editor.window_mgr.focused_window_mut().cursor_col = 8;
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-left");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 5);
}

#[test]
fn delete_char_with_count() {
    let mut editor = editor_with_text("hello world");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("delete-char-forward");
    assert_eq!(editor.active_buffer().rope().to_string(), "lo world");
}

#[test]
fn delete_line_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("delete-line");
    assert_eq!(editor.active_buffer().rope().to_string(), "line3\nline4\n");
    // Register should contain both deleted lines
    let reg = editor.registers.get(&'"').unwrap();
    assert!(reg.contains("line1"));
    assert!(reg.contains("line2"));
}

#[test]
fn g_without_count_goes_to_last() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    // No count prefix set
    editor.dispatch_builtin("move-to-last-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
}

#[test]
fn g_with_count_goes_to_line() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5");
    editor.count_prefix = Some(3); // 3G = go to line 3 (1-indexed = row 2)
    editor.dispatch_builtin("move-to-last-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
}

#[test]
fn g_with_count_clamps() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.count_prefix = Some(100); // beyond buffer
    editor.dispatch_builtin("move-to-last-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2); // last line
}

#[test]
fn gg_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5");
    editor.window_mgr.focused_window_mut().cursor_row = 4;
    editor.count_prefix = Some(2); // 2gg = go to line 2 (1-indexed = row 1)
    editor.dispatch_builtin("move-to-first-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
}

#[test]
fn word_motion_with_count() {
    let mut editor = editor_with_text("one two three four five");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-word-forward");
    // Should skip past "one ", "two ", "three " → at "four"
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 14);
}

#[test]
fn count_consumed_after_dispatch() {
    let mut editor = editor_with_text("line1\nline2\nline3\n");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("move-down");
    assert_eq!(editor.count_prefix, None);
}

#[test]
fn yank_line_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("yank-line");
    let reg = editor.registers.get(&'"').unwrap();
    assert_eq!(reg, "line1\nline2\n");
    // Buffer unchanged
    assert_eq!(
        editor.active_buffer().rope().to_string(),
        "line1\nline2\nline3\nline4\n"
    );
}

#[test]
fn paste_after_with_count() {
    let mut editor = editor_with_text("hello");
    editor.registers.insert('"', "x".to_string());
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("paste-after");
    // "x" pasted 3 times after cursor
    assert_eq!(editor.active_buffer().rope().to_string(), "hxxxello");
}

#[test]
fn scroll_half_down_with_count() {
    let mut editor = editor_with_text(&(0..50).map(|i| format!("line{}\n", i)).collect::<String>());
    editor.viewport_height = 20;
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("scroll-half-down");
    // Should scroll down twice (half page = 10, so 20 lines)
    assert!(editor.window_mgr.focused_window().cursor_row >= 20);
}

#[test]
fn search_next_with_count() {
    let mut editor = editor_with_text("aa bb aa bb aa bb aa");
    editor.search_input = "aa".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    let first_pos = editor.window_mgr.focused_window().cursor_col;
    // Search next with count 2 (skip one match)
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("search-next");
    let final_pos = editor.window_mgr.focused_window().cursor_col;
    // Should have advanced past two matches
    assert!(final_pos != first_pos);
}

#[test]
fn delete_word_forward_with_count() {
    let mut editor = editor_with_text("one two three four");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("delete-word-forward");
    assert_eq!(editor.active_buffer().rope().to_string(), "three four");
}

#[test]
fn paragraph_motion_with_count() {
    let mut editor = editor_with_text("a\n\nb\n\nc\n\nd");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("move-paragraph-forward");
    // Two paragraph motions from line 0: first lands on blank line 1,
    // second lands on blank line 3.
    let row = editor.window_mgr.focused_window().cursor_row;
    assert_eq!(row, 3);
}

// --- Text object editor integration tests ---
