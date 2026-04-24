use super::*;
use crate::keymap::parse_key_seq_spaced;
use crate::{LookupResult, Mode};

#[test]
fn operator_pending_d_with_move_to_last_line() {
    // dG — delete from cursor to bottom of file (linewise)
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
    // Cursor at line 1 (0-indexed)
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    win.cursor_col = 0;
    // Simulate d + G
    editor.dispatch_builtin("operator-delete");
    assert!(editor.pending_operator.is_some());
    editor.dispatch_builtin("move-to-last-line");
    editor.apply_pending_operator_for_motion("move-to-last-line");
    // Lines 1-3 deleted, only line0 remains
    assert_eq!(editor.active_buffer().rope().to_string(), "line1\n");
}

#[test]
fn operator_pending_d_with_move_to_first_line() {
    // dgg — delete from cursor to top of file (linewise)
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 0;
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-to-first-line");
    editor.apply_pending_operator_for_motion("move-to-first-line");
    // Lines 0-2 deleted, only line3 remains
    assert_eq!(editor.active_buffer().rope().to_string(), "line4\n");
}

#[test]
fn operator_pending_d_word_forward() {
    // dw — delete word via operator-pending (replaces hardcoded dw)
    let mut editor = editor_with_text("hello world test");
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-word-forward");
    editor.apply_pending_operator_for_motion("move-word-forward");
    assert_eq!(editor.active_buffer().rope().to_string(), "world test");
}

#[test]
fn operator_pending_d_to_line_end() {
    // d$ — delete to end of line via operator-pending
    let mut editor = editor_with_text("hello world\nsecond\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5;
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-to-line-end");
    editor.apply_pending_operator_for_motion("move-to-line-end");
    // move-to-line-end is exclusive (col = line_len = past last char)
    // so [5, 11) = " world" is deleted, leaving "hello\nsecond\n"
    assert_eq!(editor.active_buffer().rope().to_string(), "hello\nsecond\n");
}

#[test]
fn operator_pending_d_to_line_start() {
    // d0 — delete to start of line via operator-pending
    let mut editor = editor_with_text("hello world\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5;
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-to-line-start");
    editor.apply_pending_operator_for_motion("move-to-line-start");
    assert_eq!(editor.active_buffer().rope().to_string(), " world\n");
}

#[test]
fn operator_pending_y_to_first_line() {
    // ygg — yank from cursor to top of file
    let mut editor = editor_with_text("line1\nline2\nline3\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 0;
    editor.dispatch_builtin("operator-yank");
    editor.dispatch_builtin("move-to-first-line");
    editor.apply_pending_operator_for_motion("move-to-first-line");
    // Buffer unchanged
    assert_eq!(
        editor.active_buffer().rope().to_string(),
        "line1\nline2\nline3\n"
    );
    // Register should have yanked lines 0-2
    let yanked = editor.registers.get(&'"').unwrap();
    assert_eq!(yanked, "line1\nline2\nline3\n");
    // Cursor at start position (row 0 after yank restores to min)
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
}

#[test]
fn operator_pending_c_to_last_line() {
    // cG — delete to bottom and enter insert mode
    let mut editor = editor_with_text("line1\nline2\nline3\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    win.cursor_col = 0;
    editor.dispatch_builtin("operator-change");
    editor.dispatch_builtin("move-to-last-line");
    editor.apply_pending_operator_for_motion("move-to-last-line");
    // Lines 1-2 deleted
    assert_eq!(editor.active_buffer().rope().to_string(), "line1\n");
    // Should be in insert mode
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn operator_pending_d_paragraph() {
    // d} — delete to next paragraph boundary
    let mut editor = editor_with_text("line1\nline2\n\nline4\nline5\n");
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-paragraph-forward");
    editor.apply_pending_operator_for_motion("move-paragraph-forward");
    // First paragraph deleted (linewise)
    assert_eq!(editor.active_buffer().rope().to_string(), "line4\nline5\n");
}

#[test]
fn operator_pending_dd_still_works() {
    // dd is a linewise special, not operator-pending
    let mut editor = editor_with_text("line1\nline2\nline3\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    editor.dispatch_builtin("delete-line");
    assert_eq!(editor.active_buffer().rope().to_string(), "line1\nline3\n");
}

#[test]
fn operator_pending_cc_still_works() {
    // cc is a linewise special, not operator-pending
    let mut editor = editor_with_text("hello\nworld\n");
    editor.dispatch_builtin("change-line");
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn operator_pending_yy_still_works() {
    // yy is a linewise special, not operator-pending
    let mut editor = editor_with_text("line1\nline2\n");
    editor.dispatch_builtin("yank-line");
    let yanked = editor.registers.get(&'"').unwrap();
    assert_eq!(yanked, "line1\n");
}

#[test]
fn operator_pending_text_objects_unaffected() {
    // di( should still work via text object dispatch
    let mut editor = editor_with_text("fn(hello, world)");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5; // inside parens
    editor.dispatch_text_object("delete-inner-object", '(');
    assert_eq!(editor.active_buffer().rope().to_string(), "fn()");
}

#[test]
fn operator_pending_d_word_backward() {
    // db — delete word backward via operator-pending
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 6; // on 'w' (start of "world")
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-word-backward");
    editor.apply_pending_operator_for_motion("move-word-backward");
    // b goes to col 0, exclusive range [0,6) deletes "hello "
    assert_eq!(editor.active_buffer().rope().to_string(), "world");
}

#[test]
fn operator_pending_y_word() {
    // yw — yank word via operator-pending
    let mut editor = editor_with_text("hello world");
    editor.dispatch_builtin("operator-yank");
    editor.dispatch_builtin("move-word-forward");
    editor.apply_pending_operator_for_motion("move-word-forward");
    let yanked = editor.registers.get(&'"').unwrap();
    assert_eq!(yanked, "hello ");
    // Buffer unchanged
    assert_eq!(editor.active_buffer().rope().to_string(), "hello world");
}

#[test]
fn operator_pending_d_matching_bracket() {
    // d% — delete to matching bracket
    let mut editor = editor_with_text("fn(a, b)");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 2; // on '('
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-matching-bracket");
    editor.apply_pending_operator_for_motion("move-matching-bracket");
    // Should delete from '(' to ')'
    assert_eq!(editor.active_buffer().rope().to_string(), "fn");
}

#[test]
fn is_motion_command_covers_all_motions() {
    assert!(Editor::is_motion_command("move-word-forward"));
    assert!(Editor::is_motion_command("move-to-first-line"));
    assert!(Editor::is_motion_command("move-matching-bracket"));
    assert!(!Editor::is_motion_command("delete-line"));
    assert!(!Editor::is_motion_command("operator-delete"));
}

#[test]
fn is_linewise_motion_correct() {
    assert!(Editor::is_linewise_motion("move-to-first-line"));
    assert!(Editor::is_linewise_motion("move-to-last-line"));
    assert!(Editor::is_linewise_motion("move-paragraph-forward"));
    assert!(!Editor::is_linewise_motion("move-word-forward"));
    assert!(!Editor::is_linewise_motion("move-to-line-end"));
}

#[test]
fn spc_c_group_has_code_bindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC c d")),
        LookupResult::Exact("lsp-goto-definition")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC c a")),
        LookupResult::Exact("lsp-code-action")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC c R")),
        LookupResult::Exact("lsp-rename")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC c f")),
        LookupResult::Exact("lsp-format")
    );
}

#[test]
fn lsp_code_action_no_file_shows_status() {
    let mut editor = Editor::new();
    editor.lsp_request_code_action();
    assert!(editor.status_msg.contains("no file path"));
}

#[test]
fn lsp_format_no_file_shows_status() {
    let mut editor = Editor::new();
    editor.lsp_request_format();
    assert!(editor.status_msg.contains("no file path"));
}

#[test]
fn lsp_rename_enters_command_mode() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("lsp-rename");
    assert_eq!(editor.mode, Mode::Command);
    assert!(editor.command_line.starts_with("lsp-rename "));
}

// ---- WU1: Count prefix with operators ----

#[test]
fn operator_count_3dj_deletes_4_lines() {
    // 3dj: operator_count=3, motion j has no count → multiply 3*1=3
    // In the real key handler, operator_count is multiplied with motion count
    // and set as count_prefix before dispatch. Here we simulate that.
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5\n");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("operator-delete");
    assert_eq!(editor.operator_count, Some(3));
    assert!(editor.pending_operator.is_some());
    // Simulate what key_handling does: multiply op_count * motion_count
    let op_count = editor.operator_count.take().unwrap();
    let motion_count = editor.count_prefix.unwrap_or(1);
    editor.count_prefix = Some(op_count * motion_count); // 3*1=3
    editor.dispatch_builtin("move-down"); // moves 3 lines
    editor.apply_pending_operator_for_motion("move-down");
    assert_eq!(editor.active_buffer().rope().to_string(), "line5\n");
}

#[test]
fn operator_count_d3j_deletes_4_lines() {
    // d3j: no operator count, motion count=3
    // The count_prefix is set before the motion dispatch — dispatch_builtin
    // consumes it and repeats move-down 3 times.
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5\n");
    editor.dispatch_builtin("operator-delete");
    assert!(editor.operator_count.is_none());
    // Motion j with count=3: set count_prefix, then dispatch (which consumes it)
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-down"); // dispatch_builtin repeats 3 times
    editor.apply_pending_operator_for_motion("move-down");
    assert_eq!(editor.active_buffer().rope().to_string(), "line5\n");
}

#[test]
fn operator_count_saved_on_delete() {
    let mut editor = editor_with_text("hello\nworld\n");
    editor.count_prefix = Some(5);
    editor.dispatch_builtin("operator-delete");
    assert_eq!(editor.operator_count, Some(5));
}

#[test]
fn operator_count_saved_on_change() {
    let mut editor = editor_with_text("hello\nworld\n");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("operator-change");
    assert_eq!(editor.operator_count, Some(2));
}

#[test]
fn operator_count_saved_on_yank() {
    let mut editor = editor_with_text("hello\nworld\n");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("operator-yank");
    assert_eq!(editor.operator_count, Some(3));
}

#[test]
fn operator_count_saved_on_surround() {
    let mut editor = editor_with_text("hello\nworld\n");
    editor.count_prefix = Some(4);
    editor.dispatch_builtin("operator-surround");
    assert_eq!(editor.operator_count, Some(4));
}

#[test]
fn operator_count_none_without_count() {
    let mut editor = editor_with_text("hello\nworld\n");
    editor.dispatch_builtin("operator-delete");
    assert!(editor.operator_count.is_none());
}

#[test]
fn operator_count_cleared_on_apply() {
    let mut editor = editor_with_text("hello world");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-word-forward");
    editor.apply_pending_operator_for_motion("move-word-forward");
    assert!(editor.operator_count.is_none());
}

// ---- WU2: Motion classification fixes ----

#[test]
fn move_to_line_end_deletes_to_eol() {
    // d$ — delete from cursor to end of line (exclusive because cursor goes
    // past last char, so the range [5, 11) correctly deletes " world")
    let mut editor = editor_with_text("hello world\nsecond\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5; // at ' '
    editor.dispatch_builtin("operator-delete");
    editor.dispatch_builtin("move-to-line-end");
    editor.apply_pending_operator_for_motion("move-to-line-end");
    assert_eq!(editor.active_buffer().rope().to_string(), "hello\nsecond\n");
}

#[test]
fn search_next_is_exclusive() {
    assert!(Editor::is_exclusive_motion("search-next"));
    assert!(Editor::is_exclusive_motion("search-prev"));
}

#[test]
fn scroll_motions_are_linewise() {
    assert!(Editor::is_linewise_motion("scroll-half-up"));
    assert!(Editor::is_linewise_motion("scroll-half-down"));
    assert!(Editor::is_linewise_motion("scroll-page-up"));
    assert!(Editor::is_linewise_motion("scroll-page-down"));
}

#[test]
fn text_object_clears_pending_operator() {
    // di( should not leave dangling pending_operator
    let mut editor = editor_with_text("fn(hello, world)");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 3; // inside parens
    editor.dispatch_text_object("delete-inner-object", '(');
    assert!(editor.pending_operator.is_none());
    assert!(editor.operator_start.is_none());
    assert!(editor.operator_count.is_none());
}

// ---- WU5: Project switching ----
