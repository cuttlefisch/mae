use super::*;
use crate::buffer::Buffer;
use crate::keymap::parse_key_seq;
use crate::{LookupResult, Mode, VisualType};

#[test]
fn visual_char_mode_sets_anchor() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 3;
    editor.dispatch_builtin("enter-visual-char");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
    assert_eq!(editor.visual_anchor_row, 0);
    assert_eq!(editor.visual_anchor_col, 3);
}

#[test]
fn visual_line_mode_sets_anchor() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    editor.dispatch_builtin("enter-visual-line");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
    assert_eq!(editor.visual_anchor_row, 1);
}

#[test]
fn visual_escape_returns_to_normal() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("enter-visual-char");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
    editor.dispatch_builtin("enter-normal-mode");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_v_toggles_off() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("enter-visual-char");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
    editor.dispatch_builtin("enter-visual-char");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_big_v_toggles_off() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("enter-visual-line");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
    editor.dispatch_builtin("enter-visual-line");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_v_switches_from_line() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("enter-visual-line");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
    editor.dispatch_builtin("enter-visual-char");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
}

#[test]
fn visual_big_v_switches_from_char() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("enter-visual-char");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
    editor.dispatch_builtin("enter-visual-line");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
}

#[test]
fn visual_char_range_forward() {
    let mut editor = editor_with_text("hello world");
    editor.dispatch_builtin("enter-visual-char");
    // anchor at 0, cursor moves to col 5
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5;
    let (start, end) = editor.visual_selection_range();
    assert_eq!(start, 0);
    assert_eq!(end, 6); // includes char at cursor
}

#[test]
fn visual_char_range_backward() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5;
    editor.dispatch_builtin("enter-visual-char");
    // anchor at col 5, move cursor backward
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 2;
    let (start, end) = editor.visual_selection_range();
    assert_eq!(start, 2);
    assert_eq!(end, 6); // includes char at anchor
}

#[test]
fn visual_line_range_single() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("enter-visual-line");
    let (start, end) = editor.visual_selection_range();
    // Line 0: "line1\n" = chars 0..6
    assert_eq!(start, 0);
    assert_eq!(end, 6);
}

#[test]
fn visual_line_range_multi() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("enter-visual-line");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    let (start, end) = editor.visual_selection_range();
    // Lines 0-2: all text = "line1\nline2\nline3" = 17 chars
    assert_eq!(start, 0);
    assert_eq!(end, 17);
}

#[test]
fn visual_line_range_backward() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    editor.dispatch_builtin("enter-visual-line");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    let (start, end) = editor.visual_selection_range();
    assert_eq!(start, 0);
    assert_eq!(end, 17);
}

#[test]
fn visual_movement_extends_selection() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("enter-visual-char");
    // Move down
    let buf = &editor.buffers[editor.active_buffer_idx()];
    editor.window_mgr.focused_window_mut().move_down(buf);
    let (start, end) = editor.visual_selection_range();
    // Anchor at (0,0), cursor at (1,0) → chars 0..7 (includes char at cursor)
    assert_eq!(start, 0);
    assert!(end > 1); // selection extends past first char
}

#[test]
fn visual_word_motion_extends() {
    let mut editor = editor_with_text("hello world test");
    editor.dispatch_builtin("enter-visual-char");
    let buf = &editor.buffers[editor.active_buffer_idx()];
    editor
        .window_mgr
        .focused_window_mut()
        .move_word_forward(buf);
    let (start, end) = editor.visual_selection_range();
    assert_eq!(start, 0);
    assert!(end >= 6); // at least "hello " selected
}

#[test]
fn visual_delete_charwise() {
    let mut editor = editor_with_text("hello world");
    // Select "llo" (cols 2-4)
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 2;
    editor.dispatch_builtin("enter-visual-char");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 4;
    editor.visual_delete();
    assert_eq!(editor.active_buffer().rope().to_string(), "he world");
    assert_eq!(editor.registers.get(&'"').unwrap(), "llo");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_delete_linewise() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("enter-visual-line");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    editor.visual_delete();
    assert_eq!(editor.active_buffer().rope().to_string(), "line3");
    let reg = editor.registers.get(&'"').unwrap();
    assert!(reg.contains("line1"));
    assert!(reg.contains("line2"));
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_yank_charwise() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 0;
    editor.dispatch_builtin("enter-visual-char");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 4;
    editor.visual_yank();
    assert_eq!(editor.registers.get(&'"').unwrap(), "hello");
    // Text unchanged
    assert_eq!(editor.active_buffer().rope().to_string(), "hello world");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_yank_linewise() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("enter-visual-line");
    editor.visual_yank();
    assert_eq!(editor.registers.get(&'"').unwrap(), "line1\n");
    // Text unchanged
    assert_eq!(
        editor.active_buffer().rope().to_string(),
        "line1\nline2\nline3"
    );
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_change_charwise() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 0;
    editor.dispatch_builtin("enter-visual-char");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 4;
    editor.visual_change();
    assert_eq!(editor.active_buffer().rope().to_string(), " world");
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn visual_delete_cursor_position() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 2;
    editor.dispatch_builtin("enter-visual-char");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 6;
    editor.visual_delete();
    // Cursor should be at start of deleted range (col 2)
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_col, 2);
}

#[test]
fn visual_yank_cursor_position() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 6;
    editor.dispatch_builtin("enter-visual-char");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 2;
    editor.visual_yank();
    // Cursor should move to start of selection
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_col, 2);
}

#[test]
fn visual_select_entire_buffer() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    // gg (already at top), then V, then G
    editor.dispatch_builtin("enter-visual-line");
    let buf = &editor.buffers[editor.active_buffer_idx()];
    editor
        .window_mgr
        .focused_window_mut()
        .move_to_last_line(buf);
    let (start, end) = editor.visual_selection_range();
    assert_eq!(start, 0);
    assert_eq!(end, 17); // entire buffer
}

#[test]
fn visual_empty_selection_single_char() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("enter-visual-char");
    // Immediately yank (no movement) → should yank char under cursor
    editor.visual_yank();
    assert_eq!(editor.registers.get(&'"').unwrap(), "h");
}

#[test]
fn visual_keymap_has_movements() {
    let editor = Editor::new();
    let visual = editor.keymaps.get("visual").expect("visual keymap exists");
    // Check a few movement keys
    assert_eq!(
        visual.lookup(&parse_key_seq("h")),
        LookupResult::Exact("move-left")
    );
    assert_eq!(
        visual.lookup(&parse_key_seq("j")),
        LookupResult::Exact("move-down")
    );
    assert_eq!(
        visual.lookup(&parse_key_seq("w")),
        LookupResult::Exact("move-word-forward")
    );
    assert_eq!(
        visual.lookup(&parse_key_seq("b")),
        LookupResult::Exact("move-word-backward")
    );
}

#[test]
fn visual_keymap_has_operators() {
    let editor = Editor::new();
    let visual = editor.keymaps.get("visual").expect("visual keymap exists");
    assert_eq!(
        visual.lookup(&parse_key_seq("d")),
        LookupResult::Exact("visual-delete")
    );
    assert_eq!(
        visual.lookup(&parse_key_seq("y")),
        LookupResult::Exact("visual-yank")
    );
    assert_eq!(
        visual.lookup(&parse_key_seq("c")),
        LookupResult::Exact("visual-change")
    );
    assert_eq!(
        visual.lookup(&parse_key_seq("x")),
        LookupResult::Exact("visual-delete")
    );
}

#[test]
fn normal_keymap_has_v_and_big_v() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap exists");
    assert_eq!(
        normal.lookup(&parse_key_seq("v")),
        LookupResult::Exact("enter-visual-char")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("V")),
        LookupResult::Exact("enter-visual-line")
    );
}

// ===== Change operator tests =====

#[test]
fn change_line_clears_and_enters_insert() {
    let mut editor = editor_with_text("hello world\nsecond line");
    editor.dispatch_builtin("change-line");
    // Line content should be cleared
    assert_eq!(editor.active_buffer().line_text(0), "\n");
    // Should be in insert mode
    assert_eq!(editor.mode, Mode::Insert);
    // Cursor should be at col 0
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
}

#[test]
fn change_line_sets_register() {
    let mut editor = editor_with_text("hello world\nsecond line");
    editor.dispatch_builtin("change-line");
    assert_eq!(editor.registers.get(&'"').unwrap(), "hello world");
}

// --- from visual_ops_tests ---

#[test]
fn gv_reselect_visual() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "line one\nline two\nline three\n");
    let mut ed = Editor::with_buffer(buf);
    // Enter visual mode at (0, 2)
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 2;
    }
    ed.enter_visual_mode(VisualType::Char);
    // Move cursor to (1, 3)
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 1;
        w.cursor_col = 3;
    }
    // Exit visual with Esc
    ed.dispatch_builtin("enter-normal-mode");
    assert_eq!(ed.mode, Mode::Normal);
    assert!(ed.last_visual.is_some());
    // Now reselect with gv
    ed.dispatch_builtin("reselect-visual");
    assert!(matches!(ed.mode, Mode::Visual(VisualType::Char)));
    assert_eq!(ed.visual_anchor_row, 0);
    assert_eq!(ed.visual_anchor_col, 2);
    assert_eq!(ed.window_mgr.focused_window().cursor_row, 1);
    assert_eq!(ed.window_mgr.focused_window().cursor_col, 3);
}

#[test]
fn visual_swap_ends() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "abcdef\n");
    let mut ed = Editor::with_buffer(buf);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 1;
    }
    ed.enter_visual_mode(VisualType::Char);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_col = 4;
    }
    // Anchor=1, cursor=4. After swap: anchor=4, cursor=1.
    ed.visual_swap_ends();
    assert_eq!(ed.visual_anchor_col, 4);
    assert_eq!(ed.window_mgr.focused_window().cursor_col, 1);
}

#[test]
fn visual_indent_dedent() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "aaa\nbbb\nccc\n");
    let mut ed = Editor::with_buffer(buf);
    // Select lines 0-1 in visual line mode
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    ed.enter_visual_mode(VisualType::Line);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 1;
    }
    ed.visual_indent();
    assert_eq!(ed.mode, Mode::Normal);
    assert_eq!(ed.active_buffer().line_text(0), "    aaa\n");
    assert_eq!(ed.active_buffer().line_text(1), "    bbb\n");
    // ccc should be untouched
    assert_eq!(ed.active_buffer().line_text(2), "ccc\n");

    // Now dedent lines 0-1
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
    }
    ed.enter_visual_mode(VisualType::Line);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 1;
    }
    ed.visual_dedent();
    assert_eq!(ed.active_buffer().line_text(0), "aaa\n");
    assert_eq!(ed.active_buffer().line_text(1), "bbb\n");
}

#[test]
fn visual_uppercase_lowercase() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello world\n");
    let mut ed = Editor::with_buffer(buf);
    // Select "hello" (chars 0..5)
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    ed.enter_visual_mode(VisualType::Char);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_col = 4; // 0..=4 = "hello"
    }
    ed.visual_uppercase();
    assert_eq!(ed.mode, Mode::Normal);
    assert!(ed.active_buffer().text().starts_with("HELLO world"));

    // Now lowercase it back
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    ed.enter_visual_mode(VisualType::Char);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_col = 4;
    }
    ed.visual_lowercase();
    assert!(ed.active_buffer().text().starts_with("hello world"));
}

#[test]
fn search_word_backward_hash() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "foo bar foo baz foo\n");
    let mut ed = Editor::with_buffer(buf);
    // Place cursor on last "foo" (col 16)
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 16;
    }
    ed.dispatch_builtin("search-word-under-cursor-backward");
    // Should search backward, landing on the "foo" before the cursor.
    // The search direction should be backward.
    assert_eq!(
        ed.search_state.direction,
        crate::search::SearchDirection::Backward
    );
    // Cursor should have moved to a different "foo".
    let col = ed.window_mgr.focused_window().cursor_col;
    assert!(
        col < 16,
        "Expected cursor to move backward, got col={}",
        col
    );
}

#[test]
fn visual_line_selection_range_conversation_buffer() {
    // Regression: V-line in *AI* output buffer should produce correct
    // char offsets from visual_selection_range(), matching the rope lines
    // synced from the conversation.
    let mut ed = Editor::new();
    // Create a conversation buffer with a few rendered lines.
    let idx = ed.ensure_conversation_buffer_idx();
    {
        let buf = &mut ed.buffers[idx];
        let conv = buf.conversation_mut().unwrap();
        conv.push_user("hello");
        conv.push_assistant("world\nsecond line");
    }
    ed.buffers[idx].sync_conversation_rope();
    // Point the focused window at the conversation buffer.
    let win = ed.window_mgr.focused_window_mut();
    win.buffer_idx = idx;
    win.cursor_row = 0;
    win.cursor_col = 0;

    // Enter V-line mode on row 0, then move down one line.
    ed.enter_visual_mode(VisualType::Line);
    ed.dispatch_builtin("move-down");

    let (start, end) = ed.visual_selection_range();
    // Two full lines selected — offsets should span at least 2 lines of rope.
    assert!(end > start, "selection range should be non-empty");
    let rope = ed.buffers[idx].rope();
    let text = rope.slice(start..end).to_string();
    // Should contain content from both selected lines.
    assert!(
        text.contains('\n'),
        "V-line across 2 rows should span a newline, got: {:?}",
        text
    );
}
