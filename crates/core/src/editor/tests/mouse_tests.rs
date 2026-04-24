use super::*;

#[test]
fn mouse_click_left_places_cursor() {
    let mut editor = Editor::new();
    // Insert some text so we have rows/cols to click on.
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'H');
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'e');
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'l');
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'l');
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'o');

    // Dynamic gutter: 1 line → digits=1, max(1,2)+1 = 3 cols gutter.
    // Click at row 1 (content row 0 after border offset), col 3+2 = col 5.
    editor.handle_mouse_click(1, 5, crate::input::MouseButton::Left);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 0);
    assert_eq!(win.cursor_col, 2);
}

#[test]
fn mouse_click_in_gutter_ignored() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'A');

    // Click in gutter area (col < 3 for dynamic gutter with 1 line).
    let orig_row = editor.window_mgr.focused_window().cursor_row;
    let orig_col = editor.window_mgr.focused_window().cursor_col;
    editor.handle_mouse_click(1, 1, crate::input::MouseButton::Left);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, orig_row);
    assert_eq!(win.cursor_col, orig_col);
}

#[test]
fn mouse_click_clamps_to_line_length() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'A');
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'B');

    // Click far past end of line — should clamp to last char.
    editor.handle_mouse_click(1, 100, crate::input::MouseButton::Left);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 0);
    // Line "AB" has len 2, max col = 1.
    assert!(win.cursor_col <= 1);
}

#[test]
fn mouse_click_dynamic_gutter_large_file() {
    // Regression: gutter width should scale with line count.
    // 120 lines → 3 digits → gutter = max(3,2)+1 = 4 cols.
    let mut editor = Editor::new();
    for _ in 0..120 {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, '\n');
    }
    // Move to line 0 so we can test clicking
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 0;
    win.scroll_offset = 0;

    // Click at col 4 (gutter) → should be ignored.
    editor.handle_mouse_click(1, 3, crate::input::MouseButton::Left);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_col, 0, "click in gutter should be ignored");

    // Click at col 5 (first text column) → cursor at text_col 0.
    editor.handle_mouse_click(1, 4, crate::input::MouseButton::Left);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_col, 0, "first text column after gutter");
}

#[test]
fn mouse_click_shell_buffer_routes_to_pending() {
    // Shell buffer clicks should set pending_shell_click, not manipulate rope cursor.
    let mut editor = Editor::new();
    let shell_buf = crate::buffer::Buffer::new_shell("test-shell");
    editor.buffers.push(shell_buf);
    let shell_idx = editor.buffers.len() - 1;
    editor.window_mgr.focused_window_mut().buffer_idx = shell_idx;

    editor.handle_mouse_click(5, 10, crate::input::MouseButton::Left);

    // Should have set pending_shell_click (with border offset subtracted).
    assert!(editor.pending_shell_click.is_some());
    let (row, col, _) = editor.pending_shell_click.unwrap();
    assert_eq!(row, 4); // 5 - 1 border
    assert_eq!(col, 9); // 10 - 1 border
}

#[test]
fn mouse_drag_shell_buffer_routes_to_pending() {
    let mut editor = Editor::new();
    let shell_buf = crate::buffer::Buffer::new_shell("test-shell");
    editor.buffers.push(shell_buf);
    let shell_idx = editor.buffers.len() - 1;
    editor.window_mgr.focused_window_mut().buffer_idx = shell_idx;

    editor.handle_mouse_drag(3, 7);

    assert!(editor.pending_shell_drag.is_some());
    let (row, col) = editor.pending_shell_drag.unwrap();
    assert_eq!(row, 2);
    assert_eq!(col, 6);
    // Should NOT enter Visual mode for shell buffers.
    assert!(!matches!(editor.mode, crate::Mode::Visual(_)));
}

#[test]
fn mouse_release_shell_buffer_routes_to_pending() {
    let mut editor = Editor::new();
    let shell_buf = crate::buffer::Buffer::new_shell("test-shell");
    editor.buffers.push(shell_buf);
    let shell_idx = editor.buffers.len() - 1;
    editor.window_mgr.focused_window_mut().buffer_idx = shell_idx;

    editor.handle_mouse_release(8, 15);

    assert!(editor.pending_shell_release.is_some());
    let (row, col) = editor.pending_shell_release.unwrap();
    assert_eq!(row, 7);
    assert_eq!(col, 14);
}

#[test]
fn mouse_release_text_buffer_is_noop() {
    let mut editor = Editor::new();
    editor.handle_mouse_release(5, 10);
    // Text buffer → no pending shell release.
    assert!(editor.pending_shell_release.is_none());
}

#[test]
fn mouse_scroll_up_decreases_offset() {
    let mut editor = Editor::new();
    // Set an initial scroll offset.
    let win = editor.window_mgr.focused_window_mut();
    win.scroll_offset = 30;

    editor.handle_mouse_scroll(2); // positive = scroll up
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.scroll_offset, 24); // 30 - 2*3 = 24
}

#[test]
fn mouse_scroll_down_increases_offset() {
    let mut editor = Editor::new();
    // Need enough lines for scroll to work (viewport_height defaults to 40).
    let content = (0..100)
        .map(|i| format!("line {}", i))
        .collect::<Vec<_>>()
        .join("\n");
    editor.buffers[0].replace_contents(&content);
    editor.viewport_height = 40;
    let win = editor.window_mgr.focused_window_mut();
    win.scroll_offset = 0;

    editor.handle_mouse_scroll(-2); // negative = scroll down
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.scroll_offset, 6); // 0 + 2*3 = 6
}

#[test]
fn mouse_scroll_up_saturates_at_zero() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    win.scroll_offset = 2;

    editor.handle_mouse_scroll(5); // Would go to 2 - 15 = negative
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.scroll_offset, 0);
}

#[test]
fn mouse_scroll_zero_delta_is_noop() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    win.scroll_offset = 10;

    editor.handle_mouse_scroll(0);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.scroll_offset, 10);
}

#[test]
fn mouse_right_click_is_noop() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'X');
    let orig_row = editor.window_mgr.focused_window().cursor_row;
    let orig_col = editor.window_mgr.focused_window().cursor_col;

    editor.handle_mouse_click(1, 5, crate::input::MouseButton::Right);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, orig_row);
    assert_eq!(win.cursor_col, orig_col);
}

// --- Debug mode tests ---
