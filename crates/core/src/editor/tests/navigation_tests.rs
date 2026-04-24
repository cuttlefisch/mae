use super::*;
use crate::buffer::Buffer;
use crate::keymap::parse_key_seq;
use crate::{Mode, VisualType};

#[test]
fn word_forward_dispatch() {
    let mut editor = editor_with_text("hello world");
    editor.dispatch_builtin("move-word-forward");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 6);
}

#[test]
fn word_backward_dispatch() {
    let mut editor = editor_with_text("hello world");
    editor.window_mgr.focused_window_mut().cursor_col = 6;
    editor.dispatch_builtin("move-word-backward");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
}

#[test]
fn word_end_dispatch() {
    let mut editor = editor_with_text("hello world");
    editor.dispatch_builtin("move-word-end");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 4);
}

#[test]
fn matching_bracket_dispatch() {
    let mut editor = editor_with_text("(hello)");
    editor.dispatch_builtin("move-matching-bracket");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 6);
}

#[test]
fn find_char_dispatch() {
    let mut editor = editor_with_text("hello world");
    editor.dispatch_char_motion("find-char-forward", 'o');
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 4);
}

// --- Yank/Paste ---

#[test]
fn yank_line_and_paste_after() {
    let mut editor = editor_with_text("aaa\nbbb\n");
    editor.dispatch_builtin("yank-line");
    assert_eq!(editor.registers.get(&'"'), Some(&"aaa\n".to_string()));
    editor.dispatch_builtin("paste-after");
    assert_eq!(editor.buffers[0].text(), "aaa\naaa\nbbb\n");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
}

#[test]
fn yank_line_and_paste_before() {
    let mut editor = editor_with_text("aaa\nbbb\n");
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.window_mgr.focused_window_mut().cursor_col = 0;
    editor.dispatch_builtin("yank-line");
    assert_eq!(editor.registers.get(&'"'), Some(&"bbb\n".to_string()));
    editor.dispatch_builtin("paste-before");
    assert_eq!(editor.buffers[0].text(), "aaa\nbbb\nbbb\n");
}

#[test]
fn delete_line_copies_to_register_then_paste_restores() {
    let mut editor = editor_with_text("aaa\nbbb\nccc\n");
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.window_mgr.focused_window_mut().cursor_col = 0;
    editor.dispatch_builtin("delete-line");
    assert_eq!(editor.buffers[0].text(), "aaa\nccc\n");
    assert_eq!(editor.registers.get(&'"'), Some(&"bbb\n".to_string()));
    // Paste it back
    editor.window_mgr.focused_window_mut().cursor_row = 0;
    editor.dispatch_builtin("paste-after");
    assert_eq!(editor.buffers[0].text(), "aaa\nbbb\nccc\n");
}

#[test]
fn delete_word_forward() {
    let mut editor = editor_with_text("hello world");
    editor.dispatch_builtin("delete-word-forward");
    assert_eq!(editor.buffers[0].text(), "world");
    assert_eq!(editor.registers.get(&'"'), Some(&"hello ".to_string()));
}

#[test]
fn delete_to_line_end() {
    let mut editor = editor_with_text("hello world");
    editor.window_mgr.focused_window_mut().cursor_col = 5;
    editor.dispatch_builtin("delete-to-line-end");
    assert_eq!(editor.buffers[0].text(), "hello");
    assert_eq!(editor.registers.get(&'"'), Some(&" world".to_string()));
}

#[test]
fn delete_to_line_start() {
    let mut editor = editor_with_text("hello world");
    editor.window_mgr.focused_window_mut().cursor_col = 5;
    editor.dispatch_builtin("delete-to-line-start");
    assert_eq!(editor.buffers[0].text(), " world");
    assert_eq!(editor.registers.get(&'"'), Some(&"hello".to_string()));
}

#[test]
fn yank_word_does_not_modify_buffer() {
    let mut editor = editor_with_text("hello world");
    editor.dispatch_builtin("yank-word-forward");
    assert_eq!(editor.buffers[0].text(), "hello world");
    assert_eq!(editor.registers.get(&'"'), Some(&"hello ".to_string()));
}

#[test]
fn yank_to_line_end() {
    let mut editor = editor_with_text("hello world");
    editor.window_mgr.focused_window_mut().cursor_col = 6;
    editor.dispatch_builtin("yank-to-line-end");
    assert_eq!(editor.registers.get(&'"'), Some(&"world".to_string()));
}

#[test]
fn multiple_yanks_overwrite_register() {
    let mut editor = editor_with_text("aaa\nbbb\n");
    editor.dispatch_builtin("yank-line");
    assert_eq!(editor.registers.get(&'"'), Some(&"aaa\n".to_string()));
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.dispatch_builtin("yank-line");
    assert_eq!(editor.registers.get(&'"'), Some(&"bbb\n".to_string()));
}

#[test]
fn paste_in_empty_buffer() {
    let mut editor = Editor::new();
    editor.registers.insert('"', "hello".to_string());
    editor.dispatch_builtin("paste-after");
    assert_eq!(editor.buffers[0].text(), "hello");
}

// --- Buffer management ---

// --- from changelist_tests ---

#[test]
fn change_list_keybindings_registered() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::LookupResult;
    assert_eq!(
        normal.lookup(&parse_key_seq("g;")),
        LookupResult::Exact("change-backward")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("g,")),
        LookupResult::Exact("change-forward")
    );
}

#[test]
fn change_list_records_on_edit() {
    // Any call into `record_edit` should append the cursor position to
    // the change list. Use an edit that doesn't require extra machinery:
    // paste from the default register.
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "abc\ndef\n");
    let mut ed = Editor::with_buffer(buf);
    ed.registers.insert('"', "X".into());
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 1;
        w.cursor_col = 1;
    }
    ed.dispatch_builtin("paste-after");
    assert_eq!(ed.changes.len(), 1);
    assert_eq!(ed.changes[0].row, 1);
}

#[test]
fn g_semi_dispatches_to_change_backward() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "one\ntwo\nthree\n");
    let mut ed = Editor::with_buffer(buf);
    // Seed two change entries manually.
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 1;
    }
    ed.record_change();
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 2;
        w.cursor_col = 2;
    }
    ed.record_change();
    // Move cursor somewhere else, then dispatch g;.
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 1;
        w.cursor_col = 0;
    }
    ed.dispatch_builtin("change-backward");
    let w = ed.window_mgr.focused_window();
    assert_eq!((w.cursor_row, w.cursor_col), (2, 2));
}

#[test]
fn ex_changes_opens_scratch_buffer() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "a\nb\n");
    let mut ed = Editor::with_buffer(buf);
    ed.execute_command("changes");
    assert!(ed.buffers.iter().any(|b| b.name == "*Changes*"));
}

#[test]
fn at_colon_repeats_last_ex_command() {
    // `@:` should re-run the most recent ex command. Use :noh which has
    // an observable side-effect (search_state.highlight_active = false).
    let mut ed = Editor::new();
    ed.search_state.highlight_active = true;
    ed.push_command_history("noh");
    // Run :noh once to populate last command
    ed.execute_command("noh");
    assert!(!ed.search_state.highlight_active);
    ed.search_state.highlight_active = true;
    // Now simulate @:
    ed.dispatch_char_motion("replay-macro", ':');
    assert!(!ed.search_state.highlight_active);
}

#[test]
fn at_colon_without_history_sets_status() {
    let mut ed = Editor::new();
    ed.dispatch_char_motion("replay-macro", ':');
    assert!(
        ed.status_msg.contains("No previous command"),
        "expected empty-history message, got: {:?}",
        ed.status_msg
    );
}

// --- from gf_tests ---

#[test]
fn gf_keybinding_registered() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::LookupResult;
    assert_eq!(
        normal.lookup(&parse_key_seq("gf")),
        LookupResult::Exact("goto-file-under-cursor")
    );
}

#[test]
fn gf_command_registered() {
    let editor = Editor::new();
    assert!(editor.commands.contains("goto-file-under-cursor"));
}

#[test]
fn gf_opens_file_under_cursor() {
    // Write a target file to a tempdir, reference it from a scratch
    // buffer, and invoke gf via dispatch.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("target.txt");
    std::fs::write(&target, "contents\n").unwrap();
    let target_str = target.to_string_lossy().into_owned();

    let mut buf = Buffer::new();
    buf.insert_text_at(0, &format!("see {} for more\n", target_str));
    let mut ed = Editor::with_buffer(buf);
    // Put cursor inside the path.
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        // Column in "see <path>..." — position on the first char of the path.
        w.cursor_col = 4;
    }
    ed.dispatch_builtin("goto-file-under-cursor");
    // The target buffer should now be active.
    let active_name = ed.active_buffer().name.clone();
    assert_eq!(active_name, "target.txt", "status: {:?}", ed.status_msg);
}

#[test]
fn gf_status_when_no_filename() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "   \n");
    let mut ed = Editor::with_buffer(buf);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    ed.dispatch_builtin("goto-file-under-cursor");
    assert!(
        ed.status_msg.contains("no filename"),
        "status: {:?}",
        ed.status_msg
    );
}

#[test]
fn gf_status_when_file_missing() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "/nonexistent/path/xyzzy.txt\n");
    let mut ed = Editor::with_buffer(buf);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 5;
    }
    ed.dispatch_builtin("goto-file-under-cursor");
    assert!(
        ed.status_msg.contains("not found"),
        "status: {:?}",
        ed.status_msg
    );
}

// --- Vim quick-wins ---

#[test]
fn repeat_find_semicolon_after_f() {
    // "hello world" — f'o' should land on first 'o' (col 4), then ';' on second 'o' (col 7)
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello world\n");
    let mut ed = Editor::with_buffer(buf);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    // f then 'o'
    ed.dispatch_builtin("find-char-forward-await");
    ed.dispatch_char_motion("find-char-forward", 'o');
    assert_eq!(ed.window_mgr.focused_window().cursor_col, 4);
    // ; should repeat
    ed.dispatch_builtin("repeat-find");
    assert_eq!(ed.window_mgr.focused_window().cursor_col, 7);
}

#[test]
fn repeat_find_reverse_comma_after_f() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello world\n");
    let mut ed = Editor::with_buffer(buf);
    {
        let w = ed.window_mgr.focused_window_mut();
        w.cursor_row = 0;
        w.cursor_col = 0;
    }
    // f 'o' lands on col 4
    ed.dispatch_builtin("find-char-forward-await");
    ed.dispatch_char_motion("find-char-forward", 'o');
    assert_eq!(ed.window_mgr.focused_window().cursor_col, 4);
    // ; lands on col 7
    ed.dispatch_builtin("repeat-find");
    assert_eq!(ed.window_mgr.focused_window().cursor_col, 7);
    // , (reverse) goes back to col 4
    ed.dispatch_builtin("repeat-find-reverse");
    assert_eq!(ed.window_mgr.focused_window().cursor_col, 4);
}

// --- from motion_tests ---
#[test]
fn caret_moves_to_first_non_blank() {
    let mut editor = ed_with_text("    hello\n");
    editor.dispatch_builtin("move-to-first-non-blank");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 4);
}

#[test]
fn caret_on_unindented_line_lands_at_zero() {
    let mut editor = ed_with_text("hello\n");
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 3;
    }
    editor.dispatch_builtin("move-to-first-non-blank");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
}

#[test]
fn plus_moves_down_to_first_non_blank() {
    let mut editor = ed_with_text("first\n    second\nthird\n");
    editor.dispatch_builtin("move-line-next-non-blank");
    let w = editor.window_mgr.focused_window();
    assert_eq!((w.cursor_row, w.cursor_col), (1, 4));
}

#[test]
fn minus_moves_up_to_first_non_blank() {
    let mut editor = ed_with_text("    first\nsecond\nthird\n");
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 2;
        win.cursor_col = 0;
    }
    editor.dispatch_builtin("move-line-prev-non-blank");
    let w = editor.window_mgr.focused_window();
    assert_eq!((w.cursor_row, w.cursor_col), (1, 0));
    editor.dispatch_builtin("move-line-prev-non-blank");
    let w = editor.window_mgr.focused_window();
    assert_eq!((w.cursor_row, w.cursor_col), (0, 4));
}

#[test]
fn plus_with_count_moves_n_lines() {
    let mut editor = ed_with_text("a\nb\nc\n    d\ne\n");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-line-next-non-blank");
    let w = editor.window_mgr.focused_window();
    assert_eq!((w.cursor_row, w.cursor_col), (3, 4));
}

#[test]
fn ge_moves_to_end_of_prev_word() {
    let mut editor = ed_with_text("foo bar baz\n");
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 8; // 'b' of 'baz'
    }
    editor.dispatch_builtin("move-word-end-backward");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 6); // 'r' of 'bar'
    editor.dispatch_builtin("move-word-end-backward");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 2); // 'o' of 'foo'
}

#[test]
fn big_ge_treats_punctuation_as_word() {
    let mut editor = ed_with_text("foo.bar baz\n");
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 8; // 'b' of 'baz'
    }
    editor.dispatch_builtin("move-big-word-end-backward");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 6); // 'r' of 'foo.bar'
}

#[test]
fn substitute_char_deletes_and_enters_insert() {
    let mut editor = ed_with_text("abc\n");
    editor.dispatch_builtin("substitute-char");
    assert_eq!(editor.mode, Mode::Insert);
    assert_eq!(editor.active_buffer().text(), "bc\n");
    // Yanked char preserved in default register
    assert_eq!(editor.registers.get(&'"').map(String::as_str), Some("a"));
}

#[test]
fn substitute_char_with_count_deletes_n_chars() {
    let mut editor = ed_with_text("abcdef\n");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("substitute-char");
    assert_eq!(editor.mode, Mode::Insert);
    assert_eq!(editor.active_buffer().text(), "def\n");
}

#[test]
fn substitute_char_stops_at_line_end() {
    let mut editor = ed_with_text("ab\ncd\n");
    editor.count_prefix = Some(10);
    editor.dispatch_builtin("substitute-char");
    // Should only delete "ab" — bounded to current line, not newline
    assert_eq!(editor.active_buffer().text(), "\ncd\n");
}

#[test]
fn substitute_line_replaces_line_and_enters_insert() {
    let mut editor = ed_with_text("first line\nsecond\n");
    editor.dispatch_builtin("substitute-line");
    assert_eq!(editor.mode, Mode::Insert);
    assert_eq!(editor.active_buffer().text(), "\nsecond\n");
}

#[test]
fn gi_returns_to_last_insert_exit_position() {
    let mut editor = ed_with_text("abc def\n");
    // Enter insert at col 4 ('d'), type nothing, exit normal.
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 4;
    }
    editor.dispatch_builtin("enter-insert-mode");
    editor.dispatch_builtin("enter-normal-mode");
    // Cursor backed up by 1 on exit; last_insert_pos should reflect that.
    let expected = editor.last_insert_pos;
    assert!(expected.is_some());

    // Move cursor elsewhere
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        win.cursor_col = 0;
    }
    editor.dispatch_builtin("reinsert-at-last-position");
    assert_eq!(editor.mode, Mode::Insert);
    let w = editor.window_mgr.focused_window();
    if let Some((_, row, col)) = expected {
        assert_eq!((w.cursor_row, w.cursor_col), (row, col));
    }
}

#[test]
fn gi_without_prior_insert_just_enters_insert() {
    let mut editor = ed_with_text("abc\n");
    assert!(editor.last_insert_pos.is_none());
    editor.dispatch_builtin("reinsert-at-last-position");
    assert_eq!(editor.mode, Mode::Insert);
}

// --- Jump list (Ctrl-o / Ctrl-i) ---

#[test]
fn gg_then_ctrl_o_restores_cursor() {
    let mut editor = ed_with_text("a\nb\nc\nd\ne\n");
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 3;
        win.cursor_col = 0;
    }
    editor.dispatch_builtin("move-to-first-line");
    let w = editor.window_mgr.focused_window();
    assert_eq!(w.cursor_row, 0);

    editor.dispatch_builtin("jump-backward");
    let w = editor.window_mgr.focused_window();
    assert_eq!(w.cursor_row, 3);
}

#[test]
fn capital_g_then_ctrl_o_ctrl_i_round_trip() {
    let mut editor = ed_with_text("l0\nl1\nl2\nl3\nl4\n");
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 1;
    }
    editor.dispatch_builtin("move-to-last-line");
    let after_g = editor.window_mgr.focused_window().cursor_row;
    assert!(after_g >= 3);

    editor.dispatch_builtin("jump-backward");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);

    editor.dispatch_builtin("jump-forward");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, after_g);
}

#[test]
fn jump_backward_at_empty_list_is_noop() {
    let mut editor = ed_with_text("hello\n");
    editor.dispatch_builtin("jump-backward");
    // Cursor unchanged, no panic.
    let w = editor.window_mgr.focused_window();
    assert_eq!((w.cursor_row, w.cursor_col), (0, 0));
}

// --- Phase 3h M3: gn / gN (Practical Vim tip 86) ---

#[test]
fn gn_selects_next_match() {
    let mut editor = ed_with_text("foo bar foo bar foo\n");
    editor.search_input = "foo".to_string();
    editor.execute_search();
    // After execute_search cursor moves to first match past col 0 — which wraps to col 0
    // Position cursor between matches for clarity
    editor.window_mgr.focused_window_mut().cursor_col = 4; // on 'b' of first "bar"
    editor.dispatch_builtin("visual-select-next-match");
    // Should now be in visual char mode
    assert!(matches!(editor.mode, Mode::Visual(VisualType::Char)));
    // Anchor at match start (col 8), cursor at match end inclusive (col 10)
    assert_eq!(editor.visual_anchor_col, 8);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 10);
}

#[test]
fn gn_inside_match_selects_containing() {
    let mut editor = ed_with_text("hello world hello\n");
    editor.search_input = "hello".to_string();
    editor.execute_search();
    // Put cursor inside first match (offset 2)
    editor.window_mgr.focused_window_mut().cursor_col = 2;
    editor.dispatch_builtin("visual-select-next-match");
    assert!(matches!(editor.mode, Mode::Visual(VisualType::Char)));
    assert_eq!(editor.visual_anchor_col, 0);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 4);
}

#[test]
#[allow(non_snake_case)]
fn gN_selects_previous_match() {
    let mut editor = ed_with_text("foo bar foo bar foo\n");
    editor.search_input = "foo".to_string();
    editor.execute_search();
    editor.window_mgr.focused_window_mut().cursor_col = 14; // between 2nd and 3rd foo
    editor.dispatch_builtin("visual-select-prev-match");
    assert!(matches!(editor.mode, Mode::Visual(VisualType::Char)));
    // Should select the 2nd "foo" at col 8..11
    assert_eq!(editor.visual_anchor_col, 8);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 10);
}

#[test]
fn cgn_replaces_match_and_dot_repeats() {
    // Practical Vim tip 86 flow: search → cgn → type → Esc → .
    // Place cursor before any match so execute_search lands on the 1st foo.
    let mut editor = ed_with_text(".. foo bar foo bar foo\n");
    editor.search_input = "foo".to_string();
    editor.execute_search();
    // execute_search advances to first match with start > cursor (col 0),
    // which is the foo at col 3.
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 3);
    editor.dispatch_builtin("change-next-match");
    // Should be in insert mode with 1st foo (cursor-containing match) deleted
    assert_eq!(editor.mode, Mode::Insert);
    assert_eq!(editor.buffers[0].text(), "..  bar foo bar foo\n");
    // Type "BAZ" and exit
    for ch in "BAZ".chars() {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, ch);
    }
    editor.finalize_insert_for_repeat();
    editor.mode = Mode::Normal;
    assert_eq!(editor.buffers[0].text(), ".. BAZ bar foo bar foo\n");
    // Now dot-repeat — should find next match (2nd foo) and replace with BAZ
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.buffers[0].text(), ".. BAZ bar BAZ bar foo\n");
    // Dot again — 3rd foo
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.buffers[0].text(), ".. BAZ bar BAZ bar BAZ\n");
}

#[test]
fn dgn_deletes_next_match() {
    let mut editor = ed_with_text("foo bar foo\n");
    editor.search_input = "foo".to_string();
    editor.execute_search();
    editor.window_mgr.focused_window_mut().cursor_col = 0;
    editor.dispatch_builtin("delete-next-match");
    assert_eq!(editor.mode, Mode::Normal);
    assert_eq!(editor.buffers[0].text(), " bar foo\n");
    // Dot should delete the next one
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.buffers[0].text(), " bar \n");
}

#[test]
fn ygn_yanks_next_match() {
    let mut editor = ed_with_text("foo bar baz\n");
    editor.search_input = "bar".to_string();
    editor.execute_search();
    editor.window_mgr.focused_window_mut().cursor_col = 0;
    editor.dispatch_builtin("yank-next-match");
    assert_eq!(editor.mode, Mode::Normal);
    // Buffer unchanged
    assert_eq!(editor.buffers[0].text(), "foo bar baz\n");
    // Default register holds "bar"
    assert_eq!(editor.registers.get(&'"'), Some(&"bar".to_string()));
}

#[test]
fn gn_without_search_is_noop() {
    let mut editor = ed_with_text("hello world\n");
    // No search was executed
    editor.dispatch_builtin("visual-select-next-match");
    // Should stay in normal mode
    assert_eq!(editor.mode, Mode::Normal);
}

// --- File browser (ranger-style traversal) ---

#[test]
fn dispatch_file_browser_opens_overlay() {
    let mut editor = Editor::new();
    assert!(editor.file_browser.is_none());
    editor.dispatch_builtin("file-browser");
    assert!(editor.file_browser.is_some());
    assert_eq!(editor.mode, Mode::FileBrowser);
}

#[test]
fn file_browser_keybinding_registered() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::LookupResult;
    assert_eq!(
        normal.lookup(&crate::parse_key_seq_spaced("SPC f d")),
        LookupResult::Exact("file-browser")
    );
}

#[test]
fn file_browser_command_registered() {
    let editor = Editor::new();
    assert!(editor.commands.contains("file-browser"));
}

#[test]
fn gn_keybindings_registered() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::LookupResult;
    assert_eq!(
        normal.lookup(&parse_key_seq("gn")),
        LookupResult::Exact("visual-select-next-match")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("gN")),
        LookupResult::Exact("visual-select-prev-match")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("cgn")),
        LookupResult::Exact("change-next-match")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("dgn")),
        LookupResult::Exact("delete-next-match")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("ygn")),
        LookupResult::Exact("yank-next-match")
    );
}
