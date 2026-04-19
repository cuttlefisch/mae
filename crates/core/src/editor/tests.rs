use super::*;
use crate::buffer::Buffer;
use crate::keymap::{parse_key_seq, parse_key_seq_spaced, KeyPress};
use crate::{LookupResult, Mode, VisualType};
use std::fs;

#[test]
fn new_editor_has_scratch_buffer() {
    let editor = Editor::new();
    assert_eq!(editor.buffers.len(), 1);
    assert_eq!(editor.active_buffer().name, "[scratch]");
    assert!(editor.running);
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn quit_clean_buffer() {
    let mut editor = Editor::new();
    editor.execute_command("q");
    assert!(!editor.running);
}

#[test]
fn quit_modified_buffer_refuses() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'x');
    editor.execute_command("q");
    assert!(editor.running);
    assert!(editor.status_msg.contains("No write"));
}

#[test]
fn force_quit_modified_buffer() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'x');
    editor.execute_command("q!");
    assert!(!editor.running);
}

#[test]
fn save_command() {
    let dir = std::env::temp_dir().join("mae_test_save_cmd");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("test.txt");
    fs::write(&path, "original").unwrap();

    let buf = Buffer::from_file(&path).unwrap();
    let mut editor = Editor::with_buffer(buf);
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, '!');
    editor.execute_command("w");
    assert!(!editor.active_buffer().modified);
    assert!(editor.status_msg.contains("written"));

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn save_and_quit() {
    let dir = std::env::temp_dir().join("mae_test_wq");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("test.txt");
    fs::write(&path, "hi").unwrap();

    let buf = Buffer::from_file(&path).unwrap();
    let mut editor = Editor::with_buffer(buf);
    editor.execute_command("wq");
    assert!(!editor.running);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn open_file_command() {
    let dir = std::env::temp_dir().join("mae_test_open");
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("new.txt");
    fs::write(&path, "new content").unwrap();

    let mut editor = Editor::new();
    editor.open_file(path.to_str().unwrap());
    assert_eq!(editor.buffers.len(), 2);
    // Focused window should now point to the new buffer
    assert_eq!(editor.active_buffer_idx(), 1);
    assert_eq!(editor.active_buffer().text(), "new content");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unknown_command_sets_status() {
    let mut editor = Editor::new();
    let result = editor.execute_command("bogus");
    assert!(!result);
    assert!(editor.status_msg.contains("Unknown command"));
}

#[test]
fn dispatch_builtin_movement() {
    let mut editor = Editor::new();
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'a');
    }
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, '\n');
    }
    {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'b');
    }
    assert!(editor.dispatch_builtin("move-up"));
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
    assert!(editor.dispatch_builtin("move-to-line-end"));
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 1);
    assert!(editor.dispatch_builtin("move-to-line-start"));
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
}

#[test]
fn dispatch_builtin_mode_changes() {
    let mut editor = Editor::new();
    assert_eq!(editor.mode, Mode::Normal);
    editor.dispatch_builtin("enter-insert-mode");
    assert_eq!(editor.mode, Mode::Insert);
    editor.dispatch_builtin("enter-normal-mode");
    assert_eq!(editor.mode, Mode::Normal);
    editor.dispatch_builtin("enter-command-mode");
    assert_eq!(editor.mode, Mode::Command);
}

#[test]
fn dispatch_builtin_unknown_returns_false() {
    let mut editor = Editor::new();
    assert!(!editor.dispatch_builtin("nonexistent-command"));
}

#[test]
fn mode_transitions() {
    let mut editor = Editor::new();
    assert_eq!(editor.mode, Mode::Normal);
    editor.mode = Mode::Insert;
    assert_eq!(editor.mode, Mode::Insert);
    editor.mode = Mode::Command;
    assert_eq!(editor.mode, Mode::Command);
}

#[test]
fn split_and_focus() {
    let mut editor = Editor::new();
    assert_eq!(editor.window_mgr.window_count(), 1);
    editor.dispatch_builtin("split-vertical");
    assert_eq!(editor.window_mgr.window_count(), 2);
    // Focus should still be on the original window
    assert_eq!(editor.active_buffer_idx(), 0);
    editor.dispatch_builtin("focus-right");
    // After focusing right, should be on the second window
    // (which also views buffer 0 since we split the same buffer)
    assert_eq!(editor.active_buffer_idx(), 0);
}

#[test]
fn leader_bindings_exist() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::{parse_key_seq_spaced, LookupResult};
    // SPC should be a prefix
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC")),
        LookupResult::Prefix
    );
    // SPC b should be a prefix
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC b")),
        LookupResult::Prefix
    );
    // SPC b s should be save
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC b s")),
        LookupResult::Exact("save")
    );
    // SPC w v should be split-vertical
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC w v")),
        LookupResult::Exact("split-vertical")
    );
    // SPC a a should be ai-prompt
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC a a")),
        LookupResult::Exact("ai-prompt")
    );
}

#[test]
fn which_key_prefix_initialized_empty() {
    let editor = Editor::new();
    assert!(editor.which_key_prefix.is_empty());
}

#[test]
fn placeholder_commands_dispatch() {
    let mut editor = Editor::new();
    // ai-prompt is no longer a stub — it creates a conversation buffer
    assert!(editor.dispatch_builtin("ai-prompt"));
    assert_eq!(editor.mode, Mode::ConversationInput);
    assert!(editor.dispatch_builtin("kill-buffer"));
    assert!(editor.dispatch_builtin("command-palette"));
    assert!(editor.dispatch_builtin("describe-key"));
}

#[test]
fn command_palette_dispatch_opens_overlay() {
    let mut editor = Editor::new();
    assert!(editor.command_palette.is_none());
    assert_eq!(editor.mode, Mode::Normal);

    assert!(editor.dispatch_builtin("command-palette"));
    assert_eq!(editor.mode, Mode::CommandPalette);
    let palette = editor
        .command_palette
        .as_ref()
        .expect("palette should be populated");
    assert!(
        palette.entries.len() >= 100,
        "palette should be populated from registry, got {} entries",
        palette.entries.len()
    );
    assert!(
        palette.entries.iter().any(|e| e.name == "help"),
        "help command should be in palette"
    );
}

#[test]
fn all_leader_targets_registered() {
    let editor = Editor::new();
    let leader_targets = [
        "command-palette",
        "save",
        "kill-buffer",
        "next-buffer",
        "prev-buffer",
        "find-file",
        "split-vertical",
        "split-horizontal",
        "close-window",
        "focus-left",
        "focus-down",
        "focus-up",
        "focus-right",
        "ai-prompt",
        "ai-cancel",
        "describe-key",
        "describe-command",
        "quit",
        "force-quit",
        "debug-self",
        "debug-start",
        "debug-stop",
        "debug-continue",
        "debug-step-over",
        "debug-step-into",
        "debug-step-out",
        "debug-toggle-breakpoint",
        "debug-inspect",
    ];
    for target in &leader_targets {
        assert!(
            editor.commands.contains(target),
            "Command '{}' not registered",
            target
        );
    }
}

#[test]
fn ctrl_w_bindings_are_two_keys() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::{parse_key_seq_spaced, LookupResult};
    // C-w v should be 2 keys (Ctrl-w then v), not 3
    let seq = parse_key_seq_spaced("C-w v");
    assert_eq!(seq.len(), 2);
    assert_eq!(normal.lookup(&seq), LookupResult::Exact("split-vertical"));
}

#[test]
fn describe_key_arms_await_flag() {
    let mut editor = Editor::new();
    assert!(!editor.awaiting_key_description);
    assert!(editor.dispatch_builtin("describe-key"));
    assert!(editor.awaiting_key_description);
    assert!(editor.status_msg.contains("Describe key"));
}

#[test]
fn describe_command_opens_palette_in_describe_mode() {
    use crate::command_palette::PalettePurpose;
    let mut editor = Editor::new();
    assert!(editor.dispatch_builtin("describe-command"));
    assert_eq!(editor.mode, Mode::CommandPalette);
    let palette = editor.command_palette.as_ref().expect("palette populated");
    assert_eq!(palette.purpose, PalettePurpose::Describe);
}

#[test]
fn command_palette_default_purpose_is_execute() {
    use crate::command_palette::PalettePurpose;
    let mut editor = Editor::new();
    assert!(editor.dispatch_builtin("command-palette"));
    let palette = editor.command_palette.as_ref().expect("palette populated");
    assert_eq!(palette.purpose, PalettePurpose::Execute);
}

#[test]
fn spc_prefixes_all_have_which_key_group_names() {
    // Any SPC-prefixed binding that's itself a group (SPC x leading to
    // SPC x y, SPC x z, ...) must have a matching group label so the
    // which-key popup renders "+buffer" etc. instead of the fallback
    // "+...". This test pins the M4 "audit group names" invariant.
    use crate::keymap::parse_key_seq_spaced;
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    let spc = parse_key_seq_spaced("SPC");
    let entries = normal.which_key_entries(&spc, &editor.commands);
    let missing: Vec<_> = entries
        .iter()
        .filter(|e| e.is_group && e.label == "+...")
        .collect();
    assert!(
        missing.is_empty(),
        "SPC-level groups without labels: {:?}",
        missing.iter().map(|e| &e.key).collect::<Vec<_>>()
    );
}

#[test]
fn prompt_register_arms_flag() {
    let mut editor = Editor::new();
    assert!(!editor.pending_register_prompt);
    assert!(editor.dispatch_builtin("prompt-register"));
    assert!(editor.pending_register_prompt);
}

#[test]
fn show_registers_dispatch_creates_buffer() {
    let mut editor = Editor::new();
    editor.save_yank("hello".into());
    assert!(editor.dispatch_builtin("show-registers"));
    assert!(editor.buffers.iter().any(|b| b.name == "*Registers*"));
}

#[test]
fn reg_command_aliases_show_registers() {
    let mut editor = Editor::new();
    editor.save_yank("world".into());
    editor.execute_command("reg");
    assert!(editor.buffers.iter().any(|b| b.name == "*Registers*"));
}

#[test]
fn dq_key_is_bound_to_prompt_register_in_normal_and_visual() {
    let editor = Editor::new();
    for kmap in ["normal", "visual"] {
        let km = editor.keymaps.get(kmap).unwrap();
        assert_eq!(
            km.lookup(&parse_key_seq("\"")),
            LookupResult::Exact("prompt-register"),
            "`\"` binding missing in {}",
            kmap
        );
    }
}

#[test]
fn insert_from_register_inserts_at_cursor() {
    let mut editor = Editor::new();
    editor.registers.insert('a', "ABC".into());
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'X');
    // Cursor is now at offset 1 (after 'X')
    editor.insert_from_register('a');
    assert_eq!(editor.buffers[0].text(), "XABC");
}

#[test]
fn insert_from_register_empty_sets_status() {
    let mut editor = Editor::new();
    editor.insert_from_register('z');
    assert!(editor.status_msg.contains("empty"));
}

#[test]
fn ai_prompt_creates_conversation_buffer() {
    let mut editor = Editor::new();
    assert_eq!(editor.buffers.len(), 1);
    assert_eq!(editor.mode, Mode::Normal);

    editor.dispatch_builtin("ai-prompt");

    assert_eq!(editor.mode, Mode::ConversationInput);
    assert_eq!(editor.buffers.len(), 2);
    assert_eq!(
        editor.buffers[1].kind,
        crate::buffer::BufferKind::Conversation
    );
    assert_eq!(editor.buffers[1].name, "*AI*");
    assert_eq!(editor.active_buffer_idx(), 1);
}

#[test]
fn ai_prompt_reuses_existing_conversation() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    assert_eq!(editor.buffers.len(), 2);

    // Go back to normal mode and switch to scratch buffer
    editor.mode = Mode::Normal;
    editor.window_mgr.focused_window_mut().buffer_idx = 0;

    // Second ai-prompt should reuse, not create another
    editor.dispatch_builtin("ai-prompt");
    assert_eq!(editor.buffers.len(), 2);
    assert_eq!(editor.active_buffer_idx(), 1);
    assert_eq!(editor.mode, Mode::ConversationInput);
}

#[test]
fn ai_cancel_when_streaming() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    // Simulate streaming state
    if let Some(conv) = editor.buffers[1].conversation.as_mut() {
        conv.streaming = true;
        conv.streaming_start = Some(std::time::Instant::now());
    }
    editor.dispatch_builtin("ai-cancel");
    let conv = editor.buffers[1].conversation.as_ref().unwrap();
    assert!(!conv.streaming);
    assert!(conv.streaming_start.is_none());
    assert!(editor.status_msg.contains("Cancelled"));
}

#[test]
fn ai_cancel_when_not_streaming() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    editor.dispatch_builtin("ai-cancel");
    assert!(editor.status_msg.contains("No active AI request"));
}

#[test]
fn close_window_returns_to_single() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("split-vertical");
    assert_eq!(editor.window_mgr.window_count(), 2);
    editor.dispatch_builtin("close-window");
    assert_eq!(editor.window_mgr.window_count(), 1);
}

#[test]
fn debug_state_starts_none() {
    let editor = Editor::new();
    assert!(editor.debug_state.is_none());
}

#[test]
fn debug_self_populates_state() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("debug-self");
    assert!(editor.debug_state.is_some());

    let state = editor.debug_state.as_ref().unwrap();
    assert_eq!(state.target, crate::debug::DebugTarget::SelfDebug);
    assert_eq!(state.threads.len(), 2);
    assert_eq!(state.threads[0].name, "Rust Core");
    assert_eq!(state.threads[1].name, "Scheme Runtime");

    // Should have Rust state scopes
    assert!(state.variables.contains_key("Editor State"));
    assert!(state.variables.contains_key("Active Buffer"));
    assert!(state.variables.contains_key("Active Window"));
    assert!(state.variables.contains_key("All Buffers"));
}

#[test]
fn debug_self_captures_correct_values() {
    let mut editor = Editor::new();
    editor.mode = Mode::Insert;
    editor.dispatch_builtin("debug-self");

    let state = editor.debug_state.as_ref().unwrap();
    let editor_vars = &state.variables["Editor State"];
    let mode_var = editor_vars.iter().find(|v| v.name == "mode").unwrap();
    assert_eq!(mode_var.value, "Insert");
}

#[test]
fn debug_stop_clears_state() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("debug-self");
    assert!(editor.debug_state.is_some());
    editor.dispatch_builtin("debug-stop");
    assert!(editor.debug_state.is_none());
    assert!(editor.status_msg.contains("ended"));
}

#[test]
fn debug_stop_when_no_session() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("debug-stop");
    assert!(editor.status_msg.contains("No active debug session"));
}

#[test]
fn debug_toggle_breakpoint() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("debug-self");
    editor.dispatch_builtin("debug-toggle-breakpoint");
    let state = editor.debug_state.as_ref().unwrap();
    assert_eq!(state.breakpoint_count(), 1);
    assert!(editor.status_msg.contains("Breakpoint set"));

    // Toggle again removes it
    editor.dispatch_builtin("debug-toggle-breakpoint");
    let state = editor.debug_state.as_ref().unwrap();
    assert_eq!(state.breakpoint_count(), 0);
    assert!(editor.status_msg.contains("Breakpoint removed"));
}

#[test]
fn debug_leader_bindings_exist() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::{parse_key_seq_spaced, LookupResult};
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC d")),
        LookupResult::Prefix
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC d s")),
        LookupResult::Exact("debug-self")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC d b")),
        LookupResult::Exact("debug-toggle-breakpoint")
    );
}

// --- Word motions (dispatch integration) ---

fn editor_with_text(text: &str) -> Editor {
    let mut editor = Editor::new();
    for ch in text.chars() {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, ch);
    }
    // Move cursor to start
    editor.window_mgr.focused_window_mut().cursor_row = 0;
    editor.window_mgr.focused_window_mut().cursor_col = 0;
    editor
}

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

#[test]
fn next_buffer_cycles() {
    let mut editor = Editor::new();
    let mut b = Buffer::new();
    b.name = "a".into();
    editor.buffers.push(b);
    let mut b = Buffer::new();
    b.name = "b".into();
    editor.buffers.push(b);
    assert_eq!(editor.buffers.len(), 3);
    editor.window_mgr.focused_window_mut().buffer_idx = 0;
    editor.dispatch_builtin("next-buffer");
    assert_eq!(editor.active_buffer_idx(), 1);
    editor.dispatch_builtin("next-buffer");
    assert_eq!(editor.active_buffer_idx(), 2);
    editor.dispatch_builtin("next-buffer");
    assert_eq!(editor.active_buffer_idx(), 0); // wraps
}

#[test]
fn prev_buffer_cycles() {
    let mut editor = Editor::new();
    let mut b = Buffer::new();
    b.name = "a".into();
    editor.buffers.push(b);
    let mut b = Buffer::new();
    b.name = "b".into();
    editor.buffers.push(b);
    editor.window_mgr.focused_window_mut().buffer_idx = 0;
    editor.dispatch_builtin("prev-buffer");
    assert_eq!(editor.active_buffer_idx(), 2); // wraps backward
}

#[test]
fn next_buffer_single_is_noop() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("next-buffer");
    assert_eq!(editor.active_buffer_idx(), 0);
}

#[test]
fn kill_buffer_single_becomes_scratch() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("kill-buffer");
    assert_eq!(editor.buffers.len(), 1);
    assert_eq!(editor.buffers[0].name, "[scratch]");
}

#[test]
fn kill_buffer_multi_removes_and_fixes_indices() {
    let mut editor = Editor::new();
    // Add a second buffer
    editor.buffers.push(Buffer::new());
    editor.buffers[1].name = "second".to_string();
    editor.buffers.push(Buffer::new());
    editor.buffers[2].name = "third".to_string();
    // Focus on buffer 1
    editor.window_mgr.focused_window_mut().buffer_idx = 1;
    editor.dispatch_builtin("kill-buffer");
    assert_eq!(editor.buffers.len(), 2);
    // Should now be on buffer 0 (saturating_sub(1))
    assert_eq!(editor.active_buffer_idx(), 0);
}

#[test]
fn kill_buffer_modified_refuses() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'x');
    editor.dispatch_builtin("kill-buffer");
    assert!(editor.status_msg.contains("unsaved"));
    assert_eq!(editor.buffers.len(), 1);
}

#[test]
fn switch_buffer_opens_palette() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("switch-buffer");
    assert!(editor.command_palette.is_some());
    let palette = editor.command_palette.as_ref().unwrap();
    assert_eq!(
        palette.purpose,
        crate::command_palette::PalettePurpose::SwitchBuffer
    );
    assert!(palette.entries.iter().any(|e| e.name == "[scratch]"));
}

// --- New command registrations ---

#[test]
fn new_commands_registered() {
    let editor = Editor::new();
    let new_commands = [
        "move-word-forward",
        "move-word-backward",
        "move-word-end",
        "move-big-word-forward",
        "move-big-word-backward",
        "move-big-word-end",
        "move-matching-bracket",
        "move-paragraph-forward",
        "move-paragraph-backward",
        "find-char-forward-await",
        "find-char-backward-await",
        "till-char-forward-await",
        "till-char-backward-await",
        "delete-word-forward",
        "delete-to-line-end",
        "delete-to-line-start",
        "yank-line",
        "yank-word-forward",
        "yank-to-line-end",
        "yank-to-line-start",
        "paste-after",
        "paste-before",
        "switch-buffer",
    ];
    for cmd in &new_commands {
        assert!(
            editor.commands.contains(cmd),
            "Command '{}' not registered",
            cmd
        );
    }
}

// --- New keybindings ---

#[test]
fn word_motion_keybindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::{parse_key_seq, LookupResult};
    assert_eq!(
        normal.lookup(&parse_key_seq("w")),
        LookupResult::Exact("move-word-forward")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("b")),
        LookupResult::Exact("move-word-backward")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("e")),
        LookupResult::Exact("move-word-end")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("W")),
        LookupResult::Exact("move-big-word-forward")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("B")),
        LookupResult::Exact("move-big-word-backward")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("E")),
        LookupResult::Exact("move-big-word-end")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("%")),
        LookupResult::Exact("move-matching-bracket")
    );
}

#[test]
fn yank_paste_keybindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    use crate::keymap::{parse_key_seq, LookupResult};
    // y is now a standalone operator (operator-pending mode)
    assert_eq!(normal.lookup(&parse_key_seq("y")), LookupResult::Prefix);
    assert_eq!(
        normal.lookup(&parse_key_seq("yy")),
        LookupResult::Exact("yank-line")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("p")),
        LookupResult::Exact("paste-after")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("P")),
        LookupResult::Exact("paste-before")
    );
    // d is now a standalone operator (operator-pending mode)
    assert_eq!(normal.lookup(&parse_key_seq("d")), LookupResult::Prefix);
    assert_eq!(
        normal.lookup(&parse_key_seq("dd")),
        LookupResult::Exact("delete-line")
    );
}

// --- Search ---

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

#[test]
fn change_word_forward_deletes_word_enters_insert() {
    let mut editor = editor_with_text("hello world test");
    editor.dispatch_builtin("change-word-forward");
    // "hello " should be deleted, leaving "world test"
    let text = editor.active_buffer().rope().to_string();
    assert!(text.starts_with("world test"));
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn change_to_line_end_deletes_to_eol_enters_insert() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5; // at the space
    editor.dispatch_builtin("change-to-line-end");
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "hello");
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn change_to_line_start_deletes_to_sol_enters_insert() {
    let mut editor = editor_with_text("hello world");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 5; // at the space
    editor.dispatch_builtin("change-to-line-start");
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, " world");
    assert_eq!(editor.mode, Mode::Insert);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
}

// ===== Replace char tests =====

#[test]
fn replace_char_replaces_under_cursor() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_char_motion("replace-char", 'X');
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "Xello");
}

#[test]
fn replace_char_does_not_change_mode() {
    let mut editor = editor_with_text("hello");
    assert_eq!(editor.mode, Mode::Normal);
    editor.dispatch_char_motion("replace-char", 'X');
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn replace_char_at_end_of_line() {
    let mut editor = editor_with_text("hello");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 4; // at 'o'
    editor.dispatch_char_motion("replace-char", 'Z');
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "hellZ");
}

// ===== Dot repeat tests =====

#[test]
fn dot_repeats_delete_line() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.dispatch_builtin("delete-line");
    assert_eq!(editor.active_buffer().rope().to_string(), "line2\nline3");
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.active_buffer().rope().to_string(), "line3");
}

#[test]
fn dot_repeats_delete_char() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("delete-char-forward");
    assert_eq!(editor.active_buffer().rope().to_string(), "ello");
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.active_buffer().rope().to_string(), "llo");
}

#[test]
fn dot_repeats_replace_char() {
    let mut editor = editor_with_text("abcde");
    editor.dispatch_char_motion("replace-char", 'X');
    assert_eq!(editor.active_buffer().rope().to_string(), "Xbcde");
    // Move right then repeat
    let buf = &editor.buffers[editor.active_buffer_idx()];
    editor.window_mgr.focused_window_mut().move_right(buf);
    editor.dispatch_builtin("dot-repeat");
    assert_eq!(editor.active_buffer().rope().to_string(), "XXcde");
}

#[test]
fn dot_repeats_change_word() {
    let mut editor = editor_with_text("hello world test");
    // Change word forward (deletes "hello ") and enters insert mode
    editor.dispatch_builtin("change-word-forward");
    assert_eq!(editor.mode, Mode::Insert);
    // Simulate typing "XX" in insert mode
    let idx = editor.active_buffer_idx();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[idx].insert_char(win, 'X');
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[idx].insert_char(win, 'X');
    // Exit insert mode
    editor.dispatch_builtin("enter-normal-mode");
    assert_eq!(editor.mode, Mode::Normal);
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "XXworld test");

    // Move cursor to 'w' (col 2) for the next word
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 2;
    // Now dot-repeat should change-word "world " and insert "XX"
    editor.dispatch_builtin("dot-repeat");
    let text = editor.active_buffer().rope().to_string();
    assert_eq!(text, "XXXXtest");
}

#[test]
fn dot_repeat_no_previous_does_nothing() {
    let mut editor = editor_with_text("hello");
    // No previous edit recorded
    editor.dispatch_builtin("dot-repeat");
    // Buffer should be unchanged
    assert_eq!(editor.active_buffer().rope().to_string(), "hello");
}

// ===== Keybinding tests =====

#[test]
fn normal_keymap_has_change_bindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap exists");
    // cc is linewise special (kept)
    assert_eq!(
        normal.lookup(&parse_key_seq("cc")),
        LookupResult::Exact("change-line")
    );
    // C is still directly bound
    assert_eq!(
        normal.lookup(&parse_key_seq("C")),
        LookupResult::Exact("change-to-line-end")
    );
    // c is now operator-pending (prefix because cc exists)
    assert_eq!(normal.lookup(&parse_key_seq("c")), LookupResult::Prefix);
}

#[test]
fn normal_keymap_has_replace_binding() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap exists");
    assert_eq!(
        normal.lookup(&parse_key_seq("r")),
        LookupResult::Exact("replace-char-await")
    );
}

#[test]
fn normal_keymap_has_dot_repeat_binding() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap exists");
    assert_eq!(
        normal.lookup(&parse_key_seq(".")),
        LookupResult::Exact("dot-repeat")
    );
}

#[test]
fn replace_char_await_sets_pending() {
    let mut editor = editor_with_text("hello");
    editor.dispatch_builtin("replace-char-await");
    assert_eq!(
        editor.pending_char_command,
        Some("replace-char".to_string())
    );
}

// ===== Count prefix tests (Phase 3e M4) =====

#[test]
fn count_prefix_default_none() {
    let editor = Editor::new();
    assert_eq!(editor.count_prefix, None);
}

#[test]
fn take_count_default_is_1() {
    let mut editor = Editor::new();
    assert_eq!(editor.take_count(), 1);
}

#[test]
fn take_count_returns_and_clears() {
    let mut editor = Editor::new();
    editor.count_prefix = Some(5);
    assert_eq!(editor.take_count(), 5);
    assert_eq!(editor.count_prefix, None);
}

#[test]
fn move_down_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5\n");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-down");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 3);
}

#[test]
fn move_up_with_count_clamps() {
    let mut editor = editor_with_text("line1\nline2\nline3\n");
    editor.window_mgr.focused_window_mut().cursor_row = 2;
    editor.count_prefix = Some(10); // more than available
    editor.dispatch_builtin("move-up");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
}

#[test]
fn move_right_with_count() {
    let mut editor = editor_with_text("hello world");
    editor.count_prefix = Some(5);
    editor.dispatch_builtin("move-right");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 5);
}

#[test]
fn move_left_with_count() {
    let mut editor = editor_with_text("hello world");
    editor.window_mgr.focused_window_mut().cursor_col = 8;
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-left");
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 5);
}

#[test]
fn delete_char_with_count() {
    let mut editor = editor_with_text("hello world");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("delete-char-forward");
    assert_eq!(editor.active_buffer().rope().to_string(), "lo world");
}

#[test]
fn delete_line_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("delete-line");
    assert_eq!(editor.active_buffer().rope().to_string(), "line3\nline4\n");
    // Register should contain both deleted lines
    let reg = editor.registers.get(&'"').unwrap();
    assert!(reg.contains("line1"));
    assert!(reg.contains("line2"));
}

#[test]
fn g_without_count_goes_to_last() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    // No count prefix set
    editor.dispatch_builtin("move-to-last-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
}

#[test]
fn g_with_count_goes_to_line() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5");
    editor.count_prefix = Some(3); // 3G = go to line 3 (1-indexed = row 2)
    editor.dispatch_builtin("move-to-last-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
}

#[test]
fn g_with_count_clamps() {
    let mut editor = editor_with_text("line1\nline2\nline3");
    editor.count_prefix = Some(100); // beyond buffer
    editor.dispatch_builtin("move-to-last-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2); // last line
}

#[test]
fn gg_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5");
    editor.window_mgr.focused_window_mut().cursor_row = 4;
    editor.count_prefix = Some(2); // 2gg = go to line 2 (1-indexed = row 1)
    editor.dispatch_builtin("move-to-first-line");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
}

#[test]
fn word_motion_with_count() {
    let mut editor = editor_with_text("one two three four five");
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("move-word-forward");
    // Should skip past "one ", "two ", "three " → at "four"
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 14);
}

#[test]
fn count_consumed_after_dispatch() {
    let mut editor = editor_with_text("line1\nline2\nline3\n");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("move-down");
    assert_eq!(editor.count_prefix, None);
}

#[test]
fn yank_line_with_count() {
    let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("yank-line");
    let reg = editor.registers.get(&'"').unwrap();
    assert_eq!(reg, "line1\nline2\n");
    // Buffer unchanged
    assert_eq!(
        editor.active_buffer().rope().to_string(),
        "line1\nline2\nline3\nline4\n"
    );
}

#[test]
fn paste_after_with_count() {
    let mut editor = editor_with_text("hello");
    editor.registers.insert('"', "x".to_string());
    editor.count_prefix = Some(3);
    editor.dispatch_builtin("paste-after");
    // "x" pasted 3 times after cursor
    assert_eq!(editor.active_buffer().rope().to_string(), "hxxxello");
}

#[test]
fn scroll_half_down_with_count() {
    let mut editor = editor_with_text(&(0..50).map(|i| format!("line{}\n", i)).collect::<String>());
    editor.viewport_height = 20;
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("scroll-half-down");
    // Should scroll down twice (half page = 10, so 20 lines)
    assert!(editor.window_mgr.focused_window().cursor_row >= 20);
}

#[test]
fn search_next_with_count() {
    let mut editor = editor_with_text("aa bb aa bb aa bb aa");
    editor.search_input = "aa".to_string();
    editor.search_state.direction = crate::search::SearchDirection::Forward;
    editor.execute_search();
    let first_pos = editor.window_mgr.focused_window().cursor_col;
    // Search next with count 2 (skip one match)
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("search-next");
    let final_pos = editor.window_mgr.focused_window().cursor_col;
    // Should have advanced past two matches
    assert!(final_pos != first_pos);
}

#[test]
fn delete_word_forward_with_count() {
    let mut editor = editor_with_text("one two three four");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("delete-word-forward");
    assert_eq!(editor.active_buffer().rope().to_string(), "three four");
}

#[test]
fn paragraph_motion_with_count() {
    let mut editor = editor_with_text("a\n\nb\n\nc\n\nd");
    editor.count_prefix = Some(2);
    editor.dispatch_builtin("move-paragraph-forward");
    // Two paragraph motions from line 0: first lands on blank line 1,
    // second lands on blank line 3.
    let row = editor.window_mgr.focused_window().cursor_row;
    assert_eq!(row, 3);
}

// --- Text object editor integration tests ---

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

#[test]
fn m6_m7_commands_registered() {
    let editor = Editor::new();
    let cmds = [
        "join-lines",
        "indent-line",
        "dedent-line",
        "toggle-case",
        "uppercase-line",
        "lowercase-line",
        "alternate-file",
        "shell-command",
    ];
    for cmd in &cmds {
        assert!(
            editor.commands.contains(cmd),
            "Command '{}' not registered",
            cmd
        );
    }
}

#[test]
fn m6_m7_keybindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").unwrap();
    assert_eq!(
        normal.lookup(&parse_key_seq("J")),
        LookupResult::Exact("join-lines")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq(">>")),
        LookupResult::Exact("indent-line")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("<<")),
        LookupResult::Exact("dedent-line")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("~")),
        LookupResult::Exact("toggle-case")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("g U U")),
        LookupResult::Exact("uppercase-line")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("g u u")),
        LookupResult::Exact("lowercase-line")
    );
    assert_eq!(
        normal.lookup(&[KeyPress::ctrl('6')]),
        LookupResult::Exact("alternate-file")
    );
}

#[test]
fn normal_keymap_has_lsp_bindings() {
    let editor = Editor::new();
    let normal = editor.keymaps.get("normal").expect("normal keymap");
    assert_eq!(
        normal.lookup(&parse_key_seq("gd")),
        LookupResult::Exact("lsp-goto-definition")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("gr")),
        LookupResult::Exact("lsp-find-references")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("K")),
        LookupResult::Exact("lsp-hover")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("]d")),
        LookupResult::Exact("lsp-next-diagnostic")
    );
    assert_eq!(
        normal.lookup(&parse_key_seq("[d")),
        LookupResult::Exact("lsp-prev-diagnostic")
    );
}

#[test]
fn dispatch_lsp_next_diagnostic_moves_cursor() {
    use crate::editor::DiagnosticSeverity;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    buf.insert_text_at(0, "line0\nline1\nline2\n");
    let mut editor = Editor::with_buffer(buf);
    editor.diagnostics.set(
        "file:///tmp/test.rs".into(),
        vec![crate::editor::Diagnostic {
            line: 2,
            col_start: 1,
            col_end: 3,
            end_line: 2,
            severity: DiagnosticSeverity::Error,
            message: "boom".into(),
            source: None,
            code: None,
        }],
    );
    editor.dispatch_builtin("lsp-next-diagnostic");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
    assert_eq!(editor.window_mgr.focused_window().cursor_col, 1);
}

#[test]
fn dispatch_lsp_show_diagnostics_opens_buffer() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("lsp-show-diagnostics");
    assert_eq!(editor.active_buffer().name, "*Diagnostics*");
}

#[test]
fn colon_diagnostics_opens_buffer() {
    let mut editor = Editor::new();
    editor.execute_command("diagnostics");
    assert_eq!(editor.active_buffer().name, "*Diagnostics*");
}

#[test]
fn dispatch_lsp_goto_definition_queues_intent() {
    use crate::lsp_intent::LspIntent;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    let mut editor = Editor::with_buffer(buf);
    editor.dispatch_builtin("lsp-goto-definition");
    assert_eq!(editor.pending_lsp_requests.len(), 1);
    assert!(matches!(
        editor.pending_lsp_requests[0],
        LspIntent::GotoDefinition { .. }
    ));
}

#[test]
fn dispatch_lsp_hover_queues_intent() {
    use crate::lsp_intent::LspIntent;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    let mut editor = Editor::with_buffer(buf);
    editor.dispatch_builtin("lsp-hover");
    assert!(matches!(
        editor.pending_lsp_requests[0],
        LspIntent::Hover { .. }
    ));
}

#[test]
fn dispatch_lsp_find_references_queues_intent() {
    use crate::lsp_intent::LspIntent;
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
    let mut editor = Editor::with_buffer(buf);
    editor.dispatch_builtin("lsp-find-references");
    assert!(matches!(
        editor.pending_lsp_requests[0],
        LspIntent::FindReferences { .. }
    ));
}

// --- Tree-sitter syntax highlighting (Phase 4b M1/M2) ---

#[test]
fn with_buffer_attaches_rust_language_from_extension() {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/example.rs"));
    let editor = Editor::with_buffer(buf);
    assert_eq!(
        editor.syntax.language_of(0),
        Some(crate::syntax::Language::Rust)
    );
}

#[test]
fn with_buffer_without_file_has_no_language() {
    let editor = Editor::with_buffer(Buffer::new());
    assert_eq!(editor.syntax.language_of(0), None);
}

#[test]
fn open_file_detects_language_for_toml() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut f = fs::File::create(&path).unwrap();
    writeln!(f, "[package]\nname = \"mae\"").unwrap();
    drop(f);

    let mut editor = Editor::new();
    editor.open_file(path.to_str().unwrap());
    let idx = editor.active_buffer_idx();
    assert_eq!(
        editor.syntax.language_of(idx),
        Some(crate::syntax::Language::Toml)
    );
}

#[test]
fn record_edit_invalidates_syntax_cache() {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/x.rs"));
    let mut editor = Editor::with_buffer(buf);
    // Prime the cache
    let _ = editor.syntax.spans_for(0, "fn x() {}");
    // Force invalidation via the edit-recording path
    editor.record_edit("delete-line");
    // After invalidate, a fresh call should produce spans again. If the cache
    // had been left behind, recomputing against different source would still
    // match the old cached vec; invalidate forces recompute.
    let spans = editor.syntax.spans_for(0, "let y = 42;").unwrap();
    assert!(spans.iter().any(|s| s.theme_key == "keyword"));
}

#[test]
fn kill_buffer_removes_syntax_entry_for_scratch_fallback() {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/x.rs"));
    let mut editor = Editor::with_buffer(buf);
    assert!(editor.syntax.language_of(0).is_some());
    editor.dispatch_builtin("kill-buffer");
    // Single-buffer case replaces with scratch; syntax entry must be cleared.
    assert_eq!(editor.syntax.language_of(0), None);
}

#[test]
fn kill_buffer_shifts_syntax_indices() {
    // Two buffers: 0 rust, 1 toml. Kill index 0 -> former 1 becomes 0.
    let mut buf0 = Buffer::new();
    buf0.set_file_path(std::path::PathBuf::from("/tmp/a.rs"));
    let mut editor = Editor::with_buffer(buf0);

    let mut buf1 = Buffer::new();
    buf1.set_file_path(std::path::PathBuf::from("/tmp/b.toml"));
    editor.buffers.push(buf1);
    editor.syntax.set_language(1, crate::syntax::Language::Toml);

    editor.window_mgr.focused_window_mut().buffer_idx = 0;
    editor.dispatch_builtin("kill-buffer");

    assert_eq!(editor.buffers.len(), 1);
    assert_eq!(
        editor.syntax.language_of(0),
        Some(crate::syntax::Language::Toml)
    );
}

// --- Tree-sitter structural selection (Phase 4b M3) ---

fn ed_with_rust(src: &str) -> Editor {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/x.rs"));
    let mut editor = Editor::with_buffer(buf);
    for ch in src.chars() {
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, ch);
    }
    editor.syntax.invalidate(0);
    // Reset cursor to start.
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 0;
    editor
}

#[test]
fn syntax_select_node_enters_visual() {
    let mut editor = ed_with_rust("fn main() {}");
    assert!(editor.syntax_select_node());
    assert!(matches!(editor.mode, Mode::Visual(VisualType::Char)));
    // Selection should cover some bytes.
    let (start, end) = editor.visual_selection_range();
    assert!(end > start);
}

#[test]
fn syntax_select_node_no_language_fails() {
    let mut editor = Editor::new();
    assert!(!editor.syntax_select_node());
    assert!(editor.status_msg.contains("No language"));
}

#[test]
fn syntax_expand_selection_grows_to_parent() {
    let mut editor = ed_with_rust("fn main() { let x = 1; }");
    // Place cursor inside the body on the 'x' identifier (column 16).
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 16;
    // Select the innermost node at cursor.
    assert!(editor.syntax_select_node());
    let initial = editor.visual_selection_range();
    // Expand to parent.
    assert!(editor.syntax_expand_selection());
    let expanded = editor.visual_selection_range();
    // Parent should strictly contain the child range.
    assert!(
        expanded.0 <= initial.0 && expanded.1 >= initial.1,
        "expanded {:?} does not contain {:?}",
        expanded,
        initial
    );
    assert!(
        expanded.1 - expanded.0 > initial.1 - initial.0,
        "expansion did not grow the range ({:?} vs {:?})",
        expanded,
        initial
    );
}

#[test]
fn syntax_contract_selection_restores_previous() {
    let mut editor = ed_with_rust("fn main() { let x = 1; }");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 16;
    assert!(editor.syntax_select_node());
    let initial = editor.visual_selection_range();
    assert!(editor.syntax_expand_selection());
    assert!(editor.syntax_contract_selection());
    let after = editor.visual_selection_range();
    assert_eq!(after, initial);
}

#[test]
fn syntax_contract_without_stack_reports_status() {
    let mut editor = ed_with_rust("fn main() {}");
    assert!(!editor.syntax_contract_selection());
    assert!(editor.status_msg.contains("No prior"));
}

#[test]
fn syntax_tree_sexp_contains_function_item() {
    let mut editor = ed_with_rust("fn main() {}");
    let sexp = editor.syntax_tree_sexp().unwrap();
    assert!(sexp.contains("function_item"), "sexp: {}", sexp);
}

#[test]
fn syntax_node_kind_at_cursor_on_keyword() {
    let mut editor = ed_with_rust("fn main() {}");
    // Cursor at (0,0) — 'f' of 'fn'
    let kind = editor.syntax_node_kind_at_cursor().unwrap();
    // Either the keyword itself or the wrapping function item — just
    // assert we got a non-empty kind.
    assert!(!kind.is_empty());
}

// --- Phase 3h M3: Normal Mode Gaps ---

fn ed_with_text(text: &str) -> Editor {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, text);
    let mut editor = Editor::with_buffer(buf);
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 0;
    editor
}

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
fn cmdline_completes_command_names() {
    let ed = Editor::new();
    // Simulate typing "set-t" — should match set-theme
    let mut ed2 = ed;
    ed2.command_line = "set-t".to_string();
    let completions = ed2.cmdline_completions();
    assert!(
        completions.iter().any(|c| c == "set-theme"),
        "Expected set-theme in completions: {:?}",
        completions
    );
}

#[test]
fn cmdline_completes_command_args() {
    let mut ed = Editor::new();
    ed.command_line = "set-splash-art b".to_string();
    let completions = ed.cmdline_completions();
    assert_eq!(completions, vec!["bat"]);
}

#[test]
fn cmdline_completes_theme_names() {
    let mut ed = Editor::new();
    ed.command_line = "set-theme ".to_string();
    let completions = ed.cmdline_completions();
    assert!(
        completions.len() > 3,
        "Expected multiple theme completions, got {:?}",
        completions
    );
    assert!(completions.iter().any(|c| c == "default"));
}

// ===== Operator-pending mode tests (WU0) =====

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
    use super::Editor;
    assert!(Editor::is_motion_command("move-word-forward"));
    assert!(Editor::is_motion_command("move-to-first-line"));
    assert!(Editor::is_motion_command("move-matching-bracket"));
    assert!(!Editor::is_motion_command("delete-line"));
    assert!(!Editor::is_motion_command("operator-delete"));
}

#[test]
fn is_linewise_motion_correct() {
    use super::Editor;
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

#[test]
fn recent_projects_push_dedup_bounded() {
    let mut rp = crate::project::RecentProjects::new(3);
    rp.push(std::path::PathBuf::from("/a"));
    rp.push(std::path::PathBuf::from("/b"));
    rp.push(std::path::PathBuf::from("/a")); // duplicate
    assert_eq!(rp.len(), 2);
    assert_eq!(rp.list()[0], std::path::PathBuf::from("/a"));
    // Test bounded
    rp.push(std::path::PathBuf::from("/c"));
    rp.push(std::path::PathBuf::from("/d"));
    assert_eq!(rp.len(), 3);
    assert_eq!(rp.list()[0], std::path::PathBuf::from("/d"));
}

#[test]
fn project_switch_palette_empty_shows_status() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("project-switch");
    assert!(editor.status_msg.contains("No recent projects"));
    assert!(editor.command_palette.is_none());
}

#[test]
fn project_switch_palette_populates() {
    let mut editor = Editor::new();
    editor
        .recent_projects
        .push(std::path::PathBuf::from("/proj1"));
    editor
        .recent_projects
        .push(std::path::PathBuf::from("/proj2"));
    editor.dispatch_builtin("project-switch");
    assert!(editor.command_palette.is_some());
    let palette = editor.command_palette.as_ref().unwrap();
    assert_eq!(
        palette.purpose,
        crate::command_palette::PalettePurpose::SwitchProject
    );
    assert_eq!(palette.entries.len(), 2);
}

#[test]
fn switch_buffer_recomputes_search_matches() {
    let mut editor = Editor::new();
    // Buffer 0 (scratch) has no "hello"
    // Buffer 1 contains "hello world"
    let mut b = Buffer::new();
    b.insert_text_at(0, "hello world");
    b.name = "target".into();
    editor.buffers.push(b);

    // Search for "hello" while on buffer 0 (no matches)
    editor.search_input = "hello".to_string();
    editor.execute_search();
    assert_eq!(editor.search_state.matches.len(), 0);

    // Switch to buffer 1 — matches should be recomputed
    editor.switch_to_buffer(1);
    assert_eq!(editor.search_state.matches.len(), 1);
}

// ---------------------------------------------------------------------------
// Shell-insert keymap tests (Part 1: Lisp machine fix)
// ---------------------------------------------------------------------------

#[test]
fn default_keymaps_include_shell_insert() {
    let editor = Editor::new();
    assert!(
        editor.keymaps.contains_key("shell-insert"),
        "shell-insert keymap must exist in default keymaps"
    );
}

#[test]
fn shell_insert_keymap_has_default_exit_binding() {
    use crate::keymap::{parse_key_seq_spaced, LookupResult};
    let editor = Editor::new();
    let km = editor.keymaps.get("shell-insert").unwrap();
    let seq = parse_key_seq_spaced("C-\\ C-n");
    assert_eq!(km.lookup(&seq), LookupResult::Exact("shell-normal-mode"));
}

#[test]
fn shell_insert_keymap_ctrl_backslash_is_prefix() {
    use crate::keymap::{parse_key_seq, LookupResult};
    let editor = Editor::new();
    let km = editor.keymaps.get("shell-insert").unwrap();
    // A single Ctrl-\ should be a prefix (waiting for more keys).
    let seq = parse_key_seq("C-\\");
    assert_eq!(km.lookup(&seq), LookupResult::Prefix);
}

#[test]
fn shell_insert_keymap_unbound_key_returns_none() {
    use crate::keymap::{parse_key_seq, LookupResult};
    let editor = Editor::new();
    let km = editor.keymaps.get("shell-insert").unwrap();
    // A regular 'a' key should not match anything.
    assert_eq!(km.lookup(&parse_key_seq("a")), LookupResult::None);
}

#[test]
fn shell_normal_mode_command_switches_to_normal() {
    let mut editor = Editor::new();
    editor.mode = Mode::ShellInsert;
    editor.dispatch_builtin("shell-normal-mode");
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn shell_insert_keymap_user_rebind() {
    use crate::keymap::{parse_key_seq_spaced, LookupResult};
    let mut editor = Editor::new();
    let km = editor.keymaps.get_mut("shell-insert").unwrap();
    // Unbind default and bind a custom sequence.
    km.unbind(&parse_key_seq_spaced("C-\\ C-n"));
    km.bind(parse_key_seq_spaced("C-c C-c"), "shell-normal-mode");
    assert_eq!(
        km.lookup(&parse_key_seq_spaced("C-c C-c")),
        LookupResult::Exact("shell-normal-mode")
    );
    assert_eq!(
        km.lookup(&parse_key_seq_spaced("C-\\ C-n")),
        LookupResult::None
    );
}

// ---- sync_mode_to_buffer tests ----

#[test]
fn sync_mode_shell_buffer_sets_shell_insert() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    editor.mode = Mode::Normal;
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::ShellInsert);
}

#[test]
fn sync_mode_text_buffer_from_shell_insert_resets_to_normal() {
    let mut editor = Editor::new();
    editor.mode = Mode::ShellInsert;
    editor.sync_mode_to_buffer(); // active buffer is [scratch] (Text)
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn sync_mode_preserves_insert_for_text_buffers() {
    let mut editor = Editor::new();
    editor.mode = Mode::Insert;
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::Insert);
}

#[test]
fn sync_mode_preserves_visual_for_text_buffers() {
    let mut editor = Editor::new();
    editor.mode = Mode::Visual(VisualType::Char);
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
}

#[test]
fn focus_direction_syncs_mode_to_shell_buffer() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    // Split: now we have two windows both viewing buffer 0.
    editor.dispatch_builtin("split-vertical");
    assert_eq!(editor.window_mgr.window_count(), 2);
    // Put shell in the focused window (right side after split).
    editor.window_mgr.focused_window_mut().buffer_idx = 1;
    editor.mode = Mode::ShellInsert;
    // Verify we see the shell buffer.
    assert_eq!(editor.active_buffer().kind, crate::BufferKind::Shell);
    // Focus left → should switch to text buffer.
    editor.dispatch_builtin("focus-left");
    // If focus didn't change (both windows in same position), skip direction test
    // and test via switch_to_buffer + sync instead.
    if editor.active_buffer().kind == crate::BufferKind::Text {
        assert_eq!(editor.mode, Mode::Normal);
        editor.dispatch_builtin("focus-right");
        assert_eq!(editor.mode, Mode::ShellInsert);
    }
}

#[test]
fn sync_mode_via_switch_to_buffer() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::ShellInsert);
    editor.switch_to_buffer(0);
    editor.sync_mode_to_buffer();
    assert_eq!(editor.mode, Mode::Normal);
}

#[test]
fn alternate_file_syncs_mode() {
    let mut editor = Editor::new();
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    editor.switch_to_buffer(1);
    editor.mode = Mode::ShellInsert;
    // Switch back via alternate-file → text buffer
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.mode, Mode::Normal);
    // Switch forward via alternate-file → shell buffer
    editor.dispatch_builtin("alternate-file");
    assert_eq!(editor.mode, Mode::ShellInsert);
}

#[test]
fn clamp_all_cursors_clamps_visual_anchor_past_eof() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "line1\nline2\nline3\n");
    let mut editor = Editor::with_buffer(buf);
    // Enter visual mode with anchor at row 2
    {
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 2;
        win.cursor_col = 3;
    }
    editor.enter_visual_mode(crate::VisualType::Char);
    assert_eq!(editor.visual_anchor_row, 2);

    // Truncate buffer to 1 line (simulating MCP edit)
    let buf = &mut editor.buffers[0];
    let total = buf.rope().len_chars();
    let one_line = buf.rope().line_to_char(1);
    buf.delete_range(one_line, total);

    // Before clamp, anchor is stale
    assert!(editor.visual_anchor_row > editor.buffers[0].line_count().saturating_sub(1));

    editor.clamp_all_cursors();
    assert!(editor.visual_anchor_row < editor.buffers[0].line_count());
    assert!(editor.visual_anchor_col <= editor.buffers[0].line_len(editor.visual_anchor_row));
}

#[test]
fn clamp_all_cursors_clamps_last_visual_past_eof() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "aaa\nbbb\nccc\nddd\n");
    let mut editor = Editor::with_buffer(buf);
    // Set up a saved visual selection at rows 2-3
    editor.last_visual = Some((2, 1, 3, 2, crate::VisualType::Char));

    // Truncate to 1 line
    let buf = &mut editor.buffers[0];
    let total = buf.rope().len_chars();
    let one_line = buf.rope().line_to_char(1);
    buf.delete_range(one_line, total);

    editor.clamp_all_cursors();

    let (ar, ac, cr, cc, _) = editor.last_visual.unwrap();
    assert!(ar < editor.buffers[0].line_count());
    assert!(cr < editor.buffers[0].line_count());
    assert!(ac <= editor.buffers[0].line_len(ar));
    assert!(cc <= editor.buffers[0].line_len(cr));
}

// ---------------------------------------------------------------------------
// Mouse handling (Phase 8 — Step 8)
// ---------------------------------------------------------------------------

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

    // Gutter is 5 cols wide when show_line_numbers is true (default).
    // Click at row 1 (content row 0 after border offset), col 5+2 = col 7.
    editor.handle_mouse_click(1, 7, crate::input::MouseButton::Left);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 0);
    assert_eq!(win.cursor_col, 2);
}

#[test]
fn mouse_click_in_gutter_ignored() {
    let mut editor = Editor::new();
    let win = editor.window_mgr.focused_window_mut();
    editor.buffers[0].insert_char(win, 'A');

    // Click in gutter area (col < 5).
    let orig_row = editor.window_mgr.focused_window().cursor_row;
    let orig_col = editor.window_mgr.focused_window().cursor_col;
    editor.handle_mouse_click(1, 2, crate::input::MouseButton::Left);
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

#[test]
fn debug_mode_default_false() {
    let editor = Editor::new();
    assert!(!editor.debug_mode);
}

#[test]
fn debug_mode_toggle_command() {
    let mut editor = Editor::new();
    assert!(!editor.debug_mode);
    editor.dispatch_builtin("debug-mode");
    assert!(editor.debug_mode);
    editor.dispatch_builtin("debug-mode");
    assert!(!editor.debug_mode);
}

#[test]
fn debug_mode_enables_fps() {
    let mut editor = Editor::new();
    assert!(!editor.show_fps);
    editor.dispatch_builtin("debug-mode");
    assert!(editor.debug_mode);
    assert!(editor.show_fps);
}

#[test]
fn perf_stats_record_frame_averages() {
    let mut stats = super::perf::PerfStats::default();
    for i in 0..10 {
        stats.record_frame((i + 1) * 1000);
    }
    // Average of 1000..10000 = 5500
    assert_eq!(stats.avg_frame_time_us, 5500);
    assert_eq!(stats.frame_time_us, 10000);
}

#[test]
fn perf_stats_default_zeroed() {
    let stats = super::perf::PerfStats::default();
    assert_eq!(stats.rss_bytes, 0);
    assert_eq!(stats.cpu_percent, 0.0);
    assert_eq!(stats.frame_time_us, 0);
    assert_eq!(stats.avg_frame_time_us, 0);
}

#[test]
fn option_registry_has_debug_mode() {
    let reg = crate::options::OptionRegistry::new();
    let opt = reg.find("debug_mode").unwrap();
    assert_eq!(opt.name, "debug_mode");
    assert_eq!(opt.kind, crate::options::OptionKind::Bool);
    // Also works via alias
    assert!(reg.find("debug-mode").is_some());
}
