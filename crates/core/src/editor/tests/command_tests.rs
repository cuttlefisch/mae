use super::*;
use crate::buffer::Buffer;
use crate::keymap::parse_key_seq;
use crate::{LookupResult, Mode};
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
fn set_status_echoes_to_message_log() {
    let mut editor = Editor::new();
    editor.set_status("hello from test");
    let entries = editor.message_log.entries();
    assert!(
        entries
            .iter()
            .any(|e| e.message.contains("hello from test")),
        "message_log should contain status message"
    );
}

#[test]
fn set_status_empty_does_not_log() {
    let mut editor = Editor::new();
    let before = editor.message_log.entries().len();
    editor.set_status("");
    assert_eq!(editor.message_log.entries().len(), before);
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
    // SPC a a should be open-ai-agent
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC a a")),
        LookupResult::Exact("open-ai-agent")
    );
    // SPC a p should be ai-prompt
    assert_eq!(
        normal.lookup(&parse_key_seq_spaced("SPC a p")),
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

// --- from keymap_tests ---

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

// --- from completion_tests ---

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
