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
    editor.vi.count_prefix = Some(2);
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
    editor.vi.count_prefix = Some(3);
    editor.dispatch_builtin("indent-line");
    assert_eq!(editor.buffers[0].text(), "    aaa\n    bbb\n    ccc");
}

#[test]
fn dedent_with_count_multiple() {
    let mut editor = editor_with_text("    aaa\n    bbb\n    ccc");
    editor.vi.count_prefix = Some(3);
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
    editor.vi.count_prefix = Some(3);
    editor.dispatch_builtin("toggle-case");
    assert_eq!(editor.buffers[0].text(), "HELlo");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 3);
}

// --- Undo grouping: multi-step edits should undo in one step ---

#[test]
fn toggle_case_single_undo() {
    let mut editor = editor_with_text("abc");
    editor.dispatch_builtin("toggle-case");
    assert_eq!(editor.buffers[0].text(), "Abc");
    editor.dispatch_builtin("undo");
    assert_eq!(editor.buffers[0].text(), "abc");
}

#[test]
fn join_line_single_undo() {
    let mut editor = editor_with_text("hello\n  world");
    editor.dispatch_builtin("join-lines");
    assert_eq!(editor.buffers[0].text(), "hello world");
    editor.dispatch_builtin("undo");
    assert_eq!(editor.buffers[0].text(), "hello\n  world");
}

#[test]
fn replace_char_single_undo() {
    let mut editor = editor_with_text("abc");
    editor.dispatch_char_motion("replace-char", 'x');
    assert_eq!(editor.buffers[0].text(), "xbc");
    editor.dispatch_builtin("undo");
    assert_eq!(editor.buffers[0].text(), "abc");
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
fn fill_paragraph_basic() {
    let text = "This is a very long line that should be wrapped at the fill column so it fits within eighty columns properly.";
    let mut editor = editor_with_text(text);
    editor.fill_column = 40;
    editor.dispatch_builtin("fill-paragraph");
    let result = editor.buffers[0].text();
    // Every line should be <= 40 chars
    for line in result.lines() {
        assert!(
            line.len() <= 40,
            "Line too long: {:?} ({})",
            line,
            line.len()
        );
    }
    // Content should be preserved (modulo whitespace)
    let words_before: Vec<&str> = text.split_whitespace().collect();
    let words_after: Vec<&str> = result.split_whitespace().collect();
    assert_eq!(words_before, words_after);
}

#[test]
fn fill_paragraph_preserves_list_indent() {
    let text = "  - This is a list item with a very long description that spans many words.\n";
    let mut editor = editor_with_text(text);
    editor.fill_column = 40;
    editor.dispatch_builtin("fill-paragraph");
    let result = editor.buffers[0].text();
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines.len() >= 2, "Should wrap into multiple lines");
    assert!(lines[0].starts_with("  - "), "First line keeps list marker");
    if lines.len() > 1 {
        assert!(
            lines[1].starts_with("    "),
            "Continuation indented past marker"
        );
    }
}

#[test]
fn fill_paragraph_undo() {
    let text = "short line one\nshort line two\nshort line three\n";
    let mut editor = editor_with_text(text);
    editor.fill_column = 80;
    editor.dispatch_builtin("fill-paragraph");
    // Should join lines
    let filled = editor.buffers[0].text();
    assert!(filled.lines().count() <= 2);
    editor.dispatch_builtin("undo");
    assert_eq!(editor.buffers[0].text(), text);
}

#[test]
fn alternate_file_switches() {
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    editor.buffers[1].name = "second".to_string();
    editor.dispatch_builtin("next-buffer");
    assert_eq!(editor.active_buffer_idx(), 1);
    assert_eq!(editor.vi.alternate_buffer_idx, Some(0));
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.active_buffer_idx(), 0);
    assert_eq!(editor.vi.alternate_buffer_idx, Some(1));
}

#[test]
fn alternate_file_none_is_noop() {
    let mut editor = Editor::new();
    assert!(editor.vi.alternate_buffer_idx.is_none());
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
    assert_eq!(editor.vi.command_history, vec!["w"]);
}

#[test]
fn command_history_no_duplicates_consecutive() {
    let mut editor = Editor::new();
    editor.push_command_history("w");
    editor.push_command_history("w");
    assert_eq!(editor.vi.command_history.len(), 1);
}

#[test]
fn command_history_allows_non_consecutive_duplicates() {
    let mut editor = Editor::new();
    editor.push_command_history("w");
    editor.push_command_history("q");
    editor.push_command_history("w");
    assert_eq!(editor.vi.command_history.len(), 3);
}

#[test]
fn command_history_prev_recalls() {
    let mut editor = Editor::new();
    editor.push_command_history("first");
    editor.push_command_history("second");
    editor.command_history_prev();
    assert_eq!(editor.vi.command_line, "second");
    editor.command_history_prev();
    assert_eq!(editor.vi.command_line, "first");
}

#[test]
fn command_history_next_clears() {
    let mut editor = Editor::new();
    editor.push_command_history("first");
    editor.push_command_history("second");
    editor.command_history_prev();
    editor.command_history_prev();
    assert_eq!(editor.vi.command_line, "first");
    editor.command_history_next();
    assert_eq!(editor.vi.command_line, "second");
    editor.command_history_next();
    assert_eq!(editor.vi.command_line, "");
}

#[test]
fn command_history_empty_is_noop() {
    let mut editor = Editor::new();
    editor.command_history_prev();
    assert_eq!(editor.vi.command_line, "");
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

#[test]
fn shift_i_enters_insert_at_first_non_blank() {
    let mut editor = ed_with_text("    hello world");
    // Start cursor in the middle of the line
    editor.window_mgr.focused_window_mut().cursor_col = 8;
    assert_eq!(editor.mode, Mode::Normal);
    editor.dispatch_builtin("enter-insert-mode-bol");
    assert_eq!(editor.mode, Mode::Insert);
    // Cursor should be at first non-blank (column 4)
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 4);
}
