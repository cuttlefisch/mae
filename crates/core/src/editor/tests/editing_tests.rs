use super::*;
use crate::buffer::Buffer;

#[test]
fn join_lines_basic() {
    let mut editor = editor_with_text("hello\nworld");
    editor.dispatch_builtin("join-lines");
    assert_eq!(editor.buffers[0].text(), "hello world");
}

#[test]
fn join_lines_strips_leading_whitespace() {
    let mut editor = editor_with_text("hello\n    world");
    editor.dispatch_builtin("join-lines");
    assert_eq!(editor.buffers[0].text(), "hello world");
}

#[test]
fn join_lines_last_line_noop() {
    let mut editor = editor_with_text("only line");
    editor.dispatch_builtin("join-lines");
    assert_eq!(editor.buffers[0].text(), "only line");
}

#[test]
fn join_lines_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("join-lines");
    assert_eq!(editor.buffers[0].text(), "line1 line2 line3");
}

#[test]
fn join_lines_empty_next_line() {
    let mut editor = editor_with_text("hello\n\nworld");
    editor.dispatch_builtin("join-lines");
    assert_eq!(editor.buffers[0].text(), "hello\nworld");
}

#[test]
fn indent_line_adds_spaces() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("indent-line");
    assert_eq!(editor.buffers[0].text(), "    hello");
}

#[test]
fn dedent_line_removes_spaces() {
    let mut editor = editor_with_text("    hello");
    editor.dispatch_builtin("dedent-line");
    assert_eq!(editor.buffers[0].text(), "hello");
}

#[test]
fn dedent_line_partial() {
    let mut editor = editor_with_text("  hello");
    editor.dispatch_builtin("dedent-line");
    assert_eq!(editor.buffers[0].text(), "hello");
}

#[test]
fn dedent_line_no_spaces_noop() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("dedent-line");
    assert_eq!(editor.buffers[0].text(), "hello");
}

#[test]
fn indent_with_count() {
    let mut editor = editor_with_text("aaa\nbbb\nccc");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("indent-line");
    assert_eq!(editor.buffers[0].text(), "    aaa\n    bbb\n    ccc");
}

#[test]
fn dedent_with_count_multiple() {
    let mut editor = editor_with_text("    aaa\n    bbb\n    ccc");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("dedent-line");
    assert_eq!(editor.buffers[0].text(), "aaa\nbbb\nccc");
}

#[test]
fn toggle_case_lower_to_upper() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("toggle-case");
    assert_eq!(editor.buffers[0].text(), "Hello");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 1);
}

#[test]
fn toggle_case_upper_to_lower() {
    let mut editor = editor_with_text("Hello");
    editor.dispatch_builtin("toggle-case");
    assert_eq!(editor.buffers[0].text(), "hello");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 1);
}

#[test]
fn toggle_case_with_count() {
    let mut editor = editor_with_text("hello");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("toggle-case");
    assert_eq!(editor.buffers[0].text(), "HELlo");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 3);
}

#[test]
fn uppercase_line() {
    let mut editor = editor_with_text("hello world");
    editor.dispatch_builtin("uppercase-line");
    assert_eq!(editor.buffers[0].text(), "HELLO WORLD");
}

#[test]
fn lowercase_line() {
    let mut editor = editor_with_text("HELLO WORLD");
    editor.dispatch_builtin("lowercase-line");
    assert_eq!(editor.buffers[0].text(), "hello world");
}

#[test]
fn alternate_file_switches() {
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    editor.buffers[1].name = "second".to_string();
    editor.dispatch_builtin("next-buffer");
    assert_eq!(editor.active_buffer_idx(), 1);
    assert_eq!(editor.alternate_buffer_idx, Some(0));
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.active_buffer_idx(), 0);
    assert_eq!(editor.alternate_buffer_idx, Some(1));
}

#[test]
fn alternate_file_none_is_noop() {
    let mut editor = Editor::new();
    assert!(editor.alternate_buffer_idx.is_none());
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.active_buffer_idx(), 0);
}

#[test]
fn alternate_file_double_toggle() {
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    editor.buffers[1].name = "second".to_string();
    editor.dispatch_builtin("next-buffer");
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.active_buffer_idx(), 0);
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.active_buffer_idx(), 1);
}

#[test]
fn command_history_records() {
    let mut editor = Editor::new();
    editor.push_command_history("w");
    assert_eq!(editor.command_history, vec!["w"]);
}

#[test]
fn command_history_no_duplicates_consecutive() {
    let mut editor = Editor::new();
    editor.push_command_history("w");
    editor.push_command_history("w");
    assert_eq!(editor.command_history.len(), 1);
}

#[test]
fn command_history_allows_non_consecutive_duplicates() {
    let mut editor = Editor::new();
    editor.push_command_history("w");
    editor.push_command_history("q");
    editor.push_command_history("w");
    assert_eq!(editor.command_history.len(), 3);
}

#[test]
fn command_history_prev_recalls() {
    let mut editor = Editor::new();
    editor.push_command_history("first");
    editor.push_command_history("second");
    editor.command_history_prev();
    assert_eq!(editor.command_line, "second");
    editor.command_history_prev();
    assert_eq!(editor.command_line, "first");
}

#[test]
fn command_history_next_clears() {
    let mut editor = Editor::new();
    editor.push_command_history("first");
    editor.push_command_history("second");
    editor.command_history_prev();
    editor.command_history_prev();
    assert_eq!(editor.command_line, "first");
    editor.command_history_next();
    assert_eq!(editor.command_line, "second");
    editor.command_history_next();
    assert_eq!(editor.command_line, "");
}

#[test]
fn command_history_empty_is_noop() {
    let mut editor = Editor::new();
    editor.command_history_prev();
    assert_eq!(editor.command_line, "");
}

#[test]
fn shell_escape_basic() {
    let mut editor = Editor::new();
    editor.execute_command("!echo hello");
    assert_eq!(editor.status_msg, "hello");
}

#[test]
fn shell_escape_empty_shows_usage() {
    let mut editor = Editor::new();
    editor.execute_command("!");
    assert!(editor.status_msg.contains("Usage"));
}
