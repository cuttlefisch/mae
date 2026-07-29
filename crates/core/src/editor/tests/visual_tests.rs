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
    assert_eq!(editor.vi.visual_anchor_row, 0);
    assert_eq!(editor.vi.visual_anchor_col, 3);
}

#[test]
fn visual_line_mode_sets_anchor() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    editor.dispatch_builtin("enter-visual-line");
    assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
    assert_eq!(editor.vi.visual_anchor_row, 1);
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
    assert_eq!(editor.vi.registers.get(&'"').unwrap(), "llo");
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
    let reg = editor.vi.registers.get(&'"').unwrap();
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
    assert_eq!(editor.vi.registers.get(&'"').unwrap(), "hello");
    // Text unchanged
    assert_eq!(editor.active_buffer().rope().to_string(), "hello world");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_yank_linewise() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("enter-visual-line");
    editor.visual_yank();
    assert_eq!(editor.vi.registers.get(&'"').unwrap(), "line1\n");
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
    assert_eq!(editor.vi.registers.get(&'"').unwrap(), "h");
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
    assert_eq!(editor.vi.registers.get(&'"').unwrap(), "hello world");
}

// --- from visual_ops_tests ---

#[test]
fn gv_reselect_visual() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "line one\nline two\nline three\n");
    let mut editor = Editor::with_buffer(buf);
    // Enter visual mode at (0, 2)
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 2;
    }
    editor.enter_visual_mode(VisualType::Char);
    // Move cursor to (1, 3)
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 1;
        w.cursor_col = 3;
    }
    // Exit visual with Esc
    editor.dispatch_builtin("enter-normal-mode");
    assert_eq!(editor.mode, Mode::Normal);
    assert!(editor.vi.last_visual.is_some());
    // Now reselect with gv
    editor.dispatch_builtin("reselect-visual");
    assert!(matches!(editor.mode, Mode::Visual(VisualType::Char)));
    assert_eq!(editor.vi.visual_anchor_row, 0);
    assert_eq!(editor.vi.visual_anchor_col, 2);
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 3);
}

#[test]
fn visual_swap_ends() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "abcdef\n");
    let mut editor = Editor::with_buffer(buf);
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 1;
    }
    editor.enter_visual_mode(VisualType::Char);
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_col = 4;
    }
    // Anchor=1, cursor=4. After swap: anchor=4, cursor=1.
    editor.visual_swap_ends();
    assert_eq!(editor.vi.visual_anchor_col, 4);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 1);
}

#[test]
fn visual_indent_dedent() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "aaa\nbbb\nccc\n");
    let mut editor = Editor::with_buffer(buf);
    // Select lines 0-1 in visual line mode
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    editor.enter_visual_mode(VisualType::Line);
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 1;
    }
    editor.visual_indent();
    assert_eq!(editor.mode, Mode::Normal);
    assert_eq!(editor.active_buffer().line_text(0), "    aaa\n");
    assert_eq!(editor.active_buffer().line_text(1), "    bbb\n");
    // ccc should be untouched
    assert_eq!(editor.active_buffer().line_text(2), "ccc\n");

    // Now dedent lines 0-1
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
    }
    editor.enter_visual_mode(VisualType::Line);
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 1;
    }
    editor.visual_dedent();
    assert_eq!(editor.active_buffer().line_text(0), "aaa\n");
    assert_eq!(editor.active_buffer().line_text(1), "bbb\n");
}

#[test]
fn visual_uppercase_lowercase() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello world\n");
    let mut editor = Editor::with_buffer(buf);
    // Select "hello" (chars 0..5)
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    editor.enter_visual_mode(VisualType::Char);
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_col = 4; // 0..=4 = "hello"
    }
    editor.visual_uppercase();
    assert_eq!(editor.mode, Mode::Normal);
    assert!(editor.active_buffer().text().starts_with("HELLO world"));

    // Now lowercase it back
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    editor.enter_visual_mode(VisualType::Char);
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_col = 4;
    }
    editor.visual_lowercase();
    assert!(editor.active_buffer().text().starts_with("hello world"));
}

#[test]
fn search_word_backward_hash() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "foo bar foo baz foo\n");
    let mut editor = Editor::with_buffer(buf);
    // Place cursor on last "foo" (col 16)
    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 16;
    }
    editor.dispatch_builtin("search-word-under-cursor-backward");
    // Should search backward, landing on the "foo" before the cursor.
    // The search direction should be backward.
    assert_eq!(
        editor.search_state.direction,
        crate::search::SearchDirection::Backward
    );
    // Cursor should have moved to a different "foo".
    let col = editor.window_mgr.focused_window().cursor_col;
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
    let mut editor = Editor::new();
    // Create a conversation buffer with a few rendered lines.
    let idx = editor.ensure_conversation_buffer_idx();
    {
        let buf = &mut editor.buffers[idx];
        let conv = buf.conversation_mut().unwrap();
        conv.push_user("hello");
        conv.push_assistant("world\nsecond line");
    }
    editor.buffers[idx].sync_conversation_rope();
    // Point the focused window at the conversation buffer.
    let win = editor.window_mgr.focused_window_mut();
    win.buffer_idx = idx;
    win.cursor_row = 0;
    win.cursor_col = 0;

    // Enter V-line mode on row 0, then move down one line.
    editor.enter_visual_mode(VisualType::Line);
    editor.dispatch_builtin("move-down");

    let (start, end) = editor.visual_selection_range();
    // Two full lines selected — offsets should span at least 2 lines of rope.
    assert!(end > start, "selection range should be non-empty");
    let rope = editor.buffers[idx].rope();
    let text = rope.slice(start..end).to_string();
    // Should contain content from both selected lines.
    assert!(
        text.contains('\n'),
        "V-line across 2 rows should span a newline, got: {:?}",
        text
    );
}

// --- #364: multi-cursor visual-mode bulk operators ---

/// The literal #364 repro: `move-to-first-line`, `mc-add-cursor-below` x3,
/// `enter-visual-line`, `visual-uppercase` -- all 4 lines must uppercase,
/// not just the primary's (line 1).
#[test]
fn mc_visual_uppercase_affects_all_cursor_lines() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.dispatch_builtin("move-to-first-line");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");
    assert_eq!(editor.window_mgr.focused_window().cursor_set.len(), 4);

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_uppercase();

    let idx = editor.active_buffer_idx();
    let text = editor.buffers[idx].rope().to_string();
    assert_eq!(
        text, "LINE ONE\nLINE TWO\nLINE THREE\nLINE FOUR\n",
        "all 4 cursor lines must uppercase, not just the primary's"
    );
    assert_eq!(editor.mode, Mode::Normal);
}

/// Same setup with `visual_delete` — adversarial: descending-order
/// processing is what prevents this from deleting the wrong lines or
/// corrupting offsets. A passing test here is worthless if the ranges were
/// computed in the wrong order and merely happened to still delete SOME 4
/// lines — assert the buffer is EXACTLY empty, not just "shorter".
#[test]
fn mc_visual_delete_removes_all_cursor_lines_exactly() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.dispatch_builtin("move-to-first-line");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_delete();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "",
        "all 4 lines must be deleted, buffer must be exactly empty"
    );
    assert_eq!(editor.mode, Mode::Normal);
    assert!(
        editor.window_mgr.focused_window().cursor_set.is_single(),
        "cursor_set should collapse to a single cursor after the lines it tracked are gone"
    );
}

/// Same setup with `visual_yank` — proves ascending-order text composition
/// (top-to-bottom reading), not just "yank ran without crashing".
#[test]
fn mc_visual_yank_captures_all_cursor_lines_in_order() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.dispatch_builtin("move-to-first-line");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_yank();

    let register = editor.vi.registers.get(&'"').cloned().unwrap_or_default();
    assert_eq!(register, "line one\nline two\nline three\nline four\n");
    // Yank must not delete anything.
    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "line one\nline two\nline three\nline four\n"
    );
}

/// Negative/boundary case (#14): cursors NOT on contiguous lines (lines 1
/// and 3 of a 4-line buffer, skipping lines 2 and 4) — must not degrade to
/// "select everything between the topmost and bottommost cursor."
#[test]
fn mc_visual_uppercase_skips_unselected_lines_between_cursors() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.dispatch_builtin("move-to-first-line");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_set.add(2, 0); // line three (0-indexed row 2), skipping lines two and four

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_uppercase();

    let idx = editor.active_buffer_idx();
    let text = editor.buffers[idx].rope().to_string();
    assert_eq!(
        text, "LINE ONE\nline two\nLINE THREE\nline four\n",
        "only the two cursor lines should uppercase; lines two and four must be untouched"
    );
}

/// Single-cursor regression guard: the existing single-cursor uppercase
/// test's exact assertions, run again through the new code path, proving
/// the `is_single()` fast path in `visual_selection_ranges()` didn't change
/// common-case behavior.
#[test]
fn mc_single_cursor_visual_uppercase_unchanged() {
    let mut editor = editor_with_text("hello world\n");
    editor.enter_visual_mode(VisualType::Char);
    editor.dispatch_builtin("move-right");
    editor.dispatch_builtin("move-right");
    editor.dispatch_builtin("move-right");
    editor.dispatch_builtin("move-right");
    editor.visual_uppercase();
    let idx = editor.active_buffer_idx();
    assert_eq!(editor.buffers[idx].rope().to_string(), "HELLO world\n");
}

// --- #368: multi-cursor visual-mode indent/dedent/join/paste ---

#[test]
fn mc_visual_indent_affects_all_cursor_lines() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.dispatch_builtin("move-to-first-line");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_indent();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "    line one\n    line two\n    line three\n    line four\n",
        "all 4 cursor lines must indent, not just the primary's"
    );
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn mc_visual_indent_skips_unselected_lines_between_cursors() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.dispatch_builtin("move-to-first-line");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_set.add(2, 0); // line three, skipping lines two and four

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_indent();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "    line one\nline two\n    line three\nline four\n",
        "only the two cursor lines should indent; lines two and four must be untouched"
    );
}

#[test]
fn mc_single_cursor_visual_indent_dedent_unchanged() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "aaa\nbbb\nccc\n");
    let mut editor = Editor::with_buffer(buf);
    editor.enter_visual_mode(VisualType::Line);
    editor.dispatch_builtin("move-down");
    editor.visual_indent();
    assert_eq!(editor.active_buffer().line_text(0), "    aaa\n");
    assert_eq!(editor.active_buffer().line_text(1), "    bbb\n");
    assert_eq!(editor.active_buffer().line_text(2), "ccc\n");

    {
        let w = editor.window_mgr.focused_window_mut();
        w.cursor_row = 0;
    }
    editor.enter_visual_mode(VisualType::Line);
    editor.dispatch_builtin("move-down");
    editor.visual_dedent();
    assert_eq!(editor.active_buffer().line_text(0), "aaa\n");
    assert_eq!(editor.active_buffer().line_text(1), "bbb\n");
}

#[test]
fn mc_visual_dedent_affects_all_cursor_lines() {
    let mut editor =
        editor_with_text("    line one\n    line two\n    line three\n    line four\n");
    editor.dispatch_builtin("move-to-first-line");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_dedent();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "line one\nline two\nline three\nline four\n",
        "all 4 cursor lines must dedent, not just the primary's"
    );
}

#[test]
fn mc_visual_dedent_skips_unselected_lines_between_cursors() {
    let mut editor =
        editor_with_text("    line one\n    line two\n    line three\n    line four\n");
    editor.dispatch_builtin("move-to-first-line");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_set.add(2, 0); // line three, skipping lines two and four

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_dedent();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "line one\n    line two\nline three\n    line four\n",
        "only the two cursor lines should dedent; lines two and four must be untouched"
    );
}

/// The riskiest #368 operator: `join_line()` REMOVES a line per call, so
/// processing cursor spans in the wrong order silently shifts a
/// not-yet-processed span's row numbers out from under it. Three
/// NON-ADJACENT two-line spans (rows 0-1, 2-3, 4-5) — if this were
/// (incorrectly) processed top-to-bottom, joining the first span would
/// shift the second and third spans' rows up by one, corrupting the
/// result. Assert the buffer is EXACTLY the correctly-joined text, not
/// just "some joining happened."
#[test]
fn mc_visual_join_processes_non_adjacent_cursor_spans_bottom_to_top() {
    let mut editor = editor_with_text("a1\na2\nb1\nb2\nc1\nc2\n");
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
        win.cursor_set.add(2, 0);
        win.cursor_set.add(4, 0);
    }
    editor.enter_visual_mode(VisualType::Char);
    // Extend each cursor's OWN span down by one row (anchor was stamped at
    // entry -- rows 0/2/4 respectively).
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 1;
        for (i, c) in win.cursor_set.iter_mut().enumerate() {
            match i {
                1 => c.row = 3,
                2 => c.row = 5,
                _ => {}
            }
        }
    }
    editor.visual_join();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "a1 a2\nb1 b2\nc1 c2\n",
        "all 3 non-adjacent cursor spans must join correctly regardless of order"
    );
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn mc_single_cursor_visual_join_unchanged() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "aaa\nbbb\nccc\n");
    let mut editor = Editor::with_buffer(buf);
    editor.enter_visual_mode(VisualType::Line);
    editor.dispatch_builtin("move-down");
    editor.visual_join();
    let idx = editor.active_buffer_idx();
    assert_eq!(editor.buffers[idx].rope().to_string(), "aaa bbb\nccc\n");
}

#[test]
fn mc_visual_paste_replaces_all_cursor_lines_with_the_same_register_text() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.vi.registers.insert('"', "REPLACED\n".to_string());
    editor.dispatch_builtin("move-to-first-line");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.dispatch_builtin("mc-add-cursor-below");

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_paste();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "REPLACED\nREPLACED\nREPLACED\nREPLACED\n",
        "all 4 cursor lines must be replaced with the same register text"
    );
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn mc_visual_paste_skips_unselected_lines_between_cursors() {
    let mut editor = editor_with_text("line one\nline two\nline three\nline four\n");
    editor.vi.registers.insert('"', "REPLACED\n".to_string());
    editor.dispatch_builtin("move-to-first-line");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_set.add(2, 0); // line three, skipping lines two and four

    editor.enter_visual_mode(VisualType::Line);
    editor.visual_paste();

    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.buffers[idx].rope().to_string(),
        "REPLACED\nline two\nREPLACED\nline four\n",
        "only the two cursor lines should be replaced; lines two and four must be untouched"
    );
}

#[test]
fn mc_single_cursor_visual_paste_unchanged() {
    let mut editor = editor_with_text("hello world\n");
    editor.vi.registers.insert('"', "bye".to_string());
    editor.enter_visual_mode(VisualType::Char);
    editor.dispatch_builtin("move-right");
    editor.dispatch_builtin("move-right");
    editor.dispatch_builtin("move-right");
    editor.dispatch_builtin("move-right"); // selects "hello"
    editor.visual_paste();
    let idx = editor.active_buffer_idx();
    assert_eq!(editor.buffers[idx].rope().to_string(), "bye world\n");
}

/// Adversarial (#14): `visual_paste` must black-hole the DELETED selection
/// text, not clobber the register it just read the paste text from --
/// pasting the SAME text at every cursor depends on the register surviving
/// past the first cursor's own delete.
#[test]
fn mc_visual_paste_preserves_the_source_register_across_multiple_cursors() {
    let mut editor = editor_with_text("aaa\nbbb\n");
    editor.vi.registers.insert('"', "X\n".to_string());
    editor.dispatch_builtin("move-to-first-line");
    editor.dispatch_builtin("mc-add-cursor-below");
    editor.enter_visual_mode(VisualType::Line);
    editor.visual_paste();

    assert_eq!(
        editor.vi.registers.get(&'"').cloned().unwrap_or_default(),
        "X\n",
        "the source register must still hold the pasted text after both cursors used it, \
         not the deleted selection text"
    );
    let idx = editor.active_buffer_idx();
    assert_eq!(editor.buffers[idx].rope().to_string(), "X\nX\n");
}
