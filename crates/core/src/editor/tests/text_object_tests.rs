use super::*;
use crate::keymap::parse_key_seq;
use crate::{LookupResult, Mode, VisualType};

#[test]
fn delete_inner_parens() {
    let mut editor = editor_with_text("foo(bar)baz");
    // Move cursor inside parens: col 4 = 'b'
    editor.window_mgr.focused_window_mut().cursor_col = 4;
    editor.delete_text_object('(', true);
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "foo()baz");
    assert_eq!(editor.registers.get(&'"'), Some(&"bar".to_string()));
}

#[test]
fn delete_around_parens() {
    let mut editor = editor_with_text("foo(bar)baz");
    editor.window_mgr.focused_window_mut().cursor_col = 4;
    editor.delete_text_object('(', false);
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "foobaz");
    assert_eq!(editor.registers.get(&'"'), Some(&"(bar)".to_string()));
}

#[test]
fn change_inner_quotes() {
    let mut editor = editor_with_text("say \"hello\"");
    // Move cursor inside quotes: col 5 = 'h'
    editor.window_mgr.focused_window_mut().cursor_col = 5;
    editor.change_text_object('"', true);
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "say \"\"");
    assert_eq!(editor.mode, Mode::Insert);
    assert_eq!(editor.registers.get(&'"'), Some(&"hello".to_string()));
}

#[test]
fn yank_inner_braces() {
    let mut editor = editor_with_text("{ code }");
    // cursor at col 2 = 'c'
    editor.window_mgr.focused_window_mut().cursor_col = 2;
    editor.yank_text_object('{', true);
    assert_eq!(editor.registers.get(&'"'), Some(&" code ".to_string()));
    // Buffer unchanged
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "{ code }");
}

#[test]
fn delete_inner_word() {
    let mut editor = editor_with_text("hello world");
    // cursor at col 0 = 'h'
    editor.delete_text_object('w', true);
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, " world");
    assert_eq!(editor.registers.get(&'"'), Some(&"hello".to_string()));
}

#[test]
fn delete_around_word() {
    let mut editor = editor_with_text("hello world");
    // cursor at col 0 = 'h', around word includes trailing space
    editor.delete_text_object('w', false);
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "world");
}

#[test]
fn visual_select_inner_parens() {
    let mut editor = editor_with_text("(abc)");
    editor.enter_visual_mode(VisualType::Char);
    // cursor at col 2 = 'b'
    editor.window_mgr.focused_window_mut().cursor_col = 2;
    editor.visual_select_text_object('(', true);
    // Anchor should be at start of inner (col 1), cursor at end (col 3)
    assert_eq!(editor.visual_anchor_col, 1);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_col, 3);
}

#[test]
fn text_object_dispatch_method() {
    let mut editor = editor_with_text("foo(bar)baz");
    editor.window_mgr.focused_window_mut().cursor_col = 4;
    assert!(editor.dispatch_text_object("delete-inner-object", '('));
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "foo()baz");
}

#[test]
fn text_object_dispatch_unknown_returns_false() {
    let mut editor = editor_with_text("hello");
    assert!(!editor.dispatch_text_object("unknown-command", '('));
}

#[test]
fn normal_keymap_has_text_object_bindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    // di → delete-inner-object
    assert_eq!(
        normal.lookup(&parse_key_seq("di")),
        LookupResult::Exact("delete-inner-object")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("da")),
        LookupResult::Exact("delete-around-object")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("ci")),
        LookupResult::Exact("change-inner-object")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("ca")),
        LookupResult::Exact("change-around-object")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("yi")),
        LookupResult::Exact("yank-inner-object")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("ya")),
        LookupResult::Exact("yank-around-object")
    );
}

#[test]
fn visual_keymap_has_text_object_bindings() {
    let editor = Editor::new();
    let visual = editor.keymaps.get("visual").unwrap();
    // In visual mode, 'i' is a prefix for text objects
    // Since there are no longer bindings starting with just 'i',
    // it should be an exact match
    assert_eq!(
        visual.lookup(&parse_key_seq("i")),
        LookupResult::Exact("visual-inner-object")
    );
    assert_eq!(
        visual.lookup(&parse_key_seq("a")),
        LookupResult::Exact("visual-around-object")
    );
}

#[test]
fn text_object_commands_registered() {
    let editor = Editor::new();
    assert!(editor.commands.contains("delete-inner-object"));
    assert!(editor.commands.contains("delete-around-object"));
    assert!(editor.commands.contains("change-inner-object"));
    assert!(editor.commands.contains("change-around-object"));
    assert!(editor.commands.contains("yank-inner-object"));
    assert!(editor.commands.contains("yank-around-object"));
    assert!(editor.commands.contains("visual-inner-object"));
    assert!(editor.commands.contains("visual-around-object"));
}

#[test]
fn delete_inner_word_cursor_position() {
    // After deleting inner word, cursor should be at start of deleted range
    let mut editor = editor_with_text("hello world");
    editor.window_mgr.focused_window_mut().cursor_col = 7; // on 'o' in 'world'
    editor.delete_text_object('w', true);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_col, 6); // start of 'world'
}

#[test]
fn yank_inner_brackets_no_modification() {
    let mut editor = editor_with_text("[items]");
    editor.window_mgr.focused_window_mut().cursor_col = 3;
    editor.yank_text_object('[', true);
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "[items]"); // unchanged
    assert_eq!(editor.registers.get(&'"'), Some(&"items".to_string()));
}

#[test]
fn text_object_no_match_is_noop() {
    let mut editor = editor_with_text("hello world");
    editor.delete_text_object('(', true);
    // Nothing should change
    let text = editor.buffers[0].rope().to_string();
    assert_eq!(text, "hello world");
    assert!(!editor.registers.contains_key(&'"'));
}

// -----------------------------------------------------------------------
// M6/M7 tests
// -----------------------------------------------------------------------
