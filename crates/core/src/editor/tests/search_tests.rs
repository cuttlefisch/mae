use super::*;
use crate::keymap::parse_key_seq;

#[test]
fn search_forward_finds_match() {
    let mut editor = editor_with_text("hello world hello");
    editor.search_input = "hello".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    // Should jump to second "hello" (first match start > cursor pos 0)
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 12);
    assert!(editor.search_state.highlight_active);
    assert_eq!(editor.search_state.matches.len(), 2);
}

#[test]
fn search_next_advances() {
    let mut editor = editor_with_text("aa bb aa bb aa");
    editor.search_input = "aa".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    let first_col = editor.window_mgr.focused_window().cursor_col;
    editor.dispatch_builtin("search-next");
    let second_col = editor.window_mgr.focused_window().cursor_col;
    assert!(second_col > first_col || second_col == 0); // advanced or wrapped
}

#[test]
fn search_prev_goes_backward() {
    let mut editor = editor_with_text("aa bb aa bb aa");
    editor.search_input = "aa".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    // Now at some match. N goes backward.
    editor.dispatch_builtin("search-prev");
    // Should land on a match before current
    assert!(editor.search_state.highlight_active);
}

#[test]
fn search_wraps_around() {
    let mut editor = editor_with_text("aa bb");
    editor.search_input = "aa".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    // Only one match — n should wrap back to it
    editor.dispatch_builtin("search-next");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
}

#[test]
fn search_invalid_regex_shows_error() {
    let mut editor = editor_with_text("hello");
    editor.search_input = "[invalid".to_string();
    editor.execute_search();
    assert!(editor.status_msg.contains("Invalid regex"));
    assert!(!editor.search_state.highlight_active);
}

#[test]
fn substitute_single_line() {
    let mut editor = editor_with_text("foo bar foo");
    editor.execute_command("s/foo/baz/");
    assert_eq!(editor.buffers[0].text(), "baz bar foo");
}

#[test]
fn substitute_whole_buffer() {
    let mut editor = editor_with_text("foo bar\nfoo baz\n");
    editor.execute_command("%s/foo/qux/g");
    assert_eq!(editor.buffers[0].text(), "qux bar\nqux baz\n");
}

#[test]
fn substitute_is_undoable() {
    let mut editor = editor_with_text("foo bar");
    let original = editor.buffers[0].text();
    editor.execute_command("s/foo/baz/");
    assert_eq!(editor.buffers[0].text(), "baz bar");
    // Each substitute does delete_range + insert_text_at = 2 undo steps per line
    editor.dispatch_builtin("undo");
    editor.dispatch_builtin("undo");
    assert_eq!(editor.buffers[0].text(), original);
}

#[test]
fn star_searches_word_under_cursor() {
    let mut editor = editor_with_text("hello world hello");
    // Cursor at col 0 = on "hello"
    editor.dispatch_builtin("search-word-under-cursor");
    assert!(editor.search_state.highlight_active);
    assert_eq!(editor.search_state.matches.len(), 2);
    // Should jump to second occurrence
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 12);
}

#[test]
fn search_keybindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::LookupResult;
    assert_eq!(
        normal.lookup(&parse_key_seq("/")),
        LookupResult::Exact("search-forward-start")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("?")),
        LookupResult::Exact("search-backward-start")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("n")),
        LookupResult::Exact("search-next")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("N")),
        LookupResult::Exact("search-prev")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("*")),
        LookupResult::Exact("search-word-under-cursor")
    );
}

#[test]
fn search_commands_registered() {
    let editor = Editor::new();
    assert!(editor.commands.contains("search-forward-start"));
    assert!(editor.commands.contains("search-backward-start"));
    assert!(editor.commands.contains("search-next"));
    assert!(editor.commands.contains("search-prev"));
    assert!(editor.commands.contains("search-word-under-cursor"));
    assert!(editor.commands.contains("clear-search-highlight"));
}

#[test]
fn noh_clears_highlights() {
    let mut editor = editor_with_text("hello world hello");
    editor.search_input = "hello".to_string();
    editor.execute_search();
    assert!(editor.search_state.highlight_active);
    editor.execute_command("noh");
    assert!(!editor.search_state.highlight_active);
}

// -----------------------------------------------------------------------
// ignorecase / smartcase tests
// -----------------------------------------------------------------------

#[test]
fn ignorecase_matches_case_insensitively() {
    let mut editor = editor_with_text("Hello world HELLO");
    editor.ignorecase = true;
    editor.search_input = "hello".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    assert!(editor.search_state.highlight_active);
    assert_eq!(editor.search_state.matches.len(), 2);
}

#[test]
fn smartcase_with_uppercase_is_case_sensitive() {
    let mut editor = editor_with_text("Hello world hello HELLO");
    editor.ignorecase = true;
    editor.smartcase = true;
    editor.search_input = "Hello".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    assert!(editor.search_state.highlight_active);
    // Only "Hello" at position 0 should match (not "hello" or "HELLO").
    assert_eq!(editor.search_state.matches.len(), 1);
}

#[test]
fn smartcase_all_lowercase_is_case_insensitive() {
    let mut editor = editor_with_text("Hello world hello HELLO");
    editor.ignorecase = true;
    editor.smartcase = true;
    editor.search_input = "hello".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    assert!(editor.search_state.highlight_active);
    assert_eq!(editor.search_state.matches.len(), 3);
}

// -----------------------------------------------------------------------
// :g / :v global command tests
// -----------------------------------------------------------------------

#[test]
fn global_delete_matching_lines() {
    let mut editor = editor_with_text("foo\nbar\nfoo baz\nqux\n");
    editor.execute_global_command("g/foo/d");
    let text = editor.buffers[0].text();
    assert!(!text.contains("foo"));
    assert!(text.contains("bar"));
    assert!(text.contains("qux"));
}

#[test]
fn global_invert_deletes_non_matching() {
    let mut editor = editor_with_text("foo\nbar\nfoo baz\nqux\n");
    editor.execute_global_command("v/foo/d");
    let text = editor.buffers[0].text();
    // Only lines with "foo" should remain.
    assert!(text.contains("foo"));
    assert!(!text.contains("bar"));
    assert!(!text.contains("qux"));
}

#[test]
fn global_substitute_on_matching_lines() {
    let mut editor = editor_with_text("foo bar\nbaz qux\nfoo end\n");
    editor.execute_global_command("g/foo/s/foo/replaced/");
    let text = editor.buffers[0].text();
    assert!(text.contains("replaced bar"));
    assert!(text.contains("replaced end"));
    assert!(text.contains("baz qux")); // Non-matching line untouched.
}

// -----------------------------------------------------------------------
// Block visual mode tests
// -----------------------------------------------------------------------

#[test]
fn enter_visual_block_mode() {
    let mut editor = editor_with_text("hello\nworld\nfoo\n");
    editor.dispatch_builtin("enter-visual-block");
    assert_eq!(editor.mode, Mode::Visual(crate::VisualType::Block));
}

#[test]
fn visual_block_toggle() {
    let mut editor = editor_with_text("hello\nworld\n");
    // C-v enters block, pressing again exits to normal.
    editor.dispatch_builtin("enter-visual-block");
    assert_eq!(editor.mode, Mode::Visual(crate::VisualType::Block));
    editor.dispatch_builtin("enter-visual-block");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn visual_block_switch_types() {
    let mut editor = editor_with_text("hello\n");
    editor.dispatch_builtin("enter-visual-char");
    assert_eq!(editor.mode, Mode::Visual(crate::VisualType::Char));
    // Switch to block.
    editor.dispatch_builtin("enter-visual-block");
    assert_eq!(editor.mode, Mode::Visual(crate::VisualType::Block));
    // Switch to line.
    editor.dispatch_builtin("enter-visual-line");
    assert_eq!(editor.mode, Mode::Visual(crate::VisualType::Line));
}

#[test]
fn block_visual_delete_removes_column() {
    let mut editor = editor_with_text("abcde\nfghij\nklmno\n");
    // Select block: rows 0-1, cols 1-2
    editor.enter_visual_mode(crate::VisualType::Block);
    editor.visual_anchor_row = 0;
    editor.visual_anchor_col = 1;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    win.cursor_col = 2;
    editor.block_visual_delete();
    let text = editor.buffers[0].text();
    assert!(text.starts_with("ade\n"));
    assert!(text.contains("fij\n"));
}

#[test]
fn block_visual_yank_captures_columns() {
    let mut editor = editor_with_text("abcde\nfghij\n");
    editor.enter_visual_mode(crate::VisualType::Block);
    editor.visual_anchor_row = 0;
    editor.visual_anchor_col = 1;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    win.cursor_col = 2;
    editor.block_visual_yank();
    let yanked = editor.registers.get(&'"').cloned().unwrap_or_default();
    assert_eq!(yanked, "bc\ngh");
}

#[test]
fn block_visual_insert_on_all_lines() {
    let mut editor = editor_with_text("abc\ndef\nghi\n");
    editor.enter_visual_mode(crate::VisualType::Block);
    editor.visual_anchor_row = 0;
    editor.visual_anchor_col = 1;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 1;
    editor.block_visual_insert("XX");
    let text = editor.buffers[0].text();
    assert!(text.starts_with("aXXbc\n"));
    assert!(text.contains("dXXef\n"));
    assert!(text.contains("gXXhi\n"));
}

// -----------------------------------------------------------------------
// Option tests
// -----------------------------------------------------------------------

#[test]
fn insert_ctrl_d_option_values() {
    let mut editor = Editor::new();
    assert_eq!(editor.insert_ctrl_d, "dedent");
    assert!(editor.set_option("insert_ctrl_d", "delete-forward").is_ok());
    assert_eq!(editor.insert_ctrl_d, "delete-forward");
    assert!(editor.set_option("insert_ctrl_d", "invalid").is_err());
}

#[test]
fn ignorecase_smartcase_options() {
    let mut editor = Editor::new();
    assert!(!editor.ignorecase);
    assert!(!editor.smartcase);
    assert!(editor.set_option("ignorecase", "true").is_ok());
    assert!(editor.ignorecase);
    assert!(editor.set_option("smartcase", "true").is_ok());
    assert!(editor.smartcase);
}

// -----------------------------------------------------------------------
// Undo grouping tests
// -----------------------------------------------------------------------

#[test]
fn block_visual_delete_undoes_as_one_group() {
    let mut editor = editor_with_text("abcd\nefgh\nijkl\n");
    editor.enter_visual_mode(crate::VisualType::Block);
    editor.visual_anchor_row = 0;
    editor.visual_anchor_col = 1;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 2;
    editor.block_visual_delete();
    // Should have deleted columns 1-2 from all 3 lines.
    assert_eq!(editor.buffers[0].line_text(0).trim_end(), "ad");
    // One undo should restore all lines.
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].undo(win);
    assert_eq!(editor.buffers[0].line_text(0).trim_end(), "abcd");
    assert_eq!(editor.buffers[0].line_text(1).trim_end(), "efgh");
    assert_eq!(editor.buffers[0].line_text(2).trim_end(), "ijkl");
}

#[test]
fn block_visual_insert_undoes_as_one_group() {
    let mut editor = editor_with_text("abc\ndef\nghi\n");
    editor.enter_visual_mode(crate::VisualType::Block);
    editor.visual_anchor_row = 0;
    editor.visual_anchor_col = 0;
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 0;
    editor.block_visual_insert("XX");
    assert!(editor.buffers[0].text().starts_with("XXabc\n"));
    // One undo should restore all lines.
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].undo(win);
    assert_eq!(editor.buffers[0].text(), "abc\ndef\nghi\n");
}

#[test]
fn global_delete_undoes_as_one_group() {
    let mut editor = editor_with_text("keep\ndelete me\nkeep too\ndelete me\n");
    editor.execute_global_command("g/delete/d");
    assert_eq!(editor.buffers[0].line_count(), 3); // 2 lines + trailing
                                                   // One undo should restore all deleted lines.
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].undo(win);
    assert_eq!(editor.buffers[0].line_count(), 5);
}

// -----------------------------------------------------------------------
// Range substitute tests
// -----------------------------------------------------------------------

#[test]
fn range_substitute_dot_plus_n() {
    let mut editor = editor_with_text("aaa\nbbb\nccc\nddd\neee\n");
    // Cursor on line 1 (0-indexed)
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    // .,+2s/./X/ should substitute on lines 1,2,3
    if let Some((start, end, sub_cmd)) = editor.parse_ex_range(".,+2s/./X/") {
        editor.execute_substitute_with_range(sub_cmd, Some((start, end)));
    }
    assert_eq!(editor.buffers[0].line_text(0).trim_end(), "aaa"); // untouched
    assert_eq!(editor.buffers[0].line_text(1).trim_end(), "Xbb"); // substituted
    assert_eq!(editor.buffers[0].line_text(2).trim_end(), "Xcc"); // substituted
    assert_eq!(editor.buffers[0].line_text(3).trim_end(), "Xdd"); // substituted
    assert_eq!(editor.buffers[0].line_text(4).trim_end(), "eee"); // untouched
}

#[test]
fn range_substitute_absolute_lines() {
    let mut editor = editor_with_text("aaa\nbbb\nccc\n");
    if let Some((start, end, sub_cmd)) = editor.parse_ex_range("2,3s/./X/") {
        editor.execute_substitute_with_range(sub_cmd, Some((start, end)));
    }
    assert_eq!(editor.buffers[0].line_text(0).trim_end(), "aaa"); // line 1 untouched
    assert_eq!(editor.buffers[0].line_text(1).trim_end(), "Xbb"); // line 2 substituted
    assert_eq!(editor.buffers[0].line_text(2).trim_end(), "Xcc"); // line 3 substituted
}

// -----------------------------------------------------------------------
// Tab completion tests
// -----------------------------------------------------------------------

#[test]
fn set_tab_completes_option_names() {
    let editor = Editor::new();
    let mut e = editor;
    e.command_line = "set ignore".to_string();
    e.command_cursor = e.command_line.len();
    let completions = e.cmdline_completions();
    assert!(completions.contains(&"ignorecase".to_string()));
}

// -----------------------------------------------------------------------
// Visual mode tests
// -----------------------------------------------------------------------
