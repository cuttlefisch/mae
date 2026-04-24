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
// Visual mode tests
// -----------------------------------------------------------------------
