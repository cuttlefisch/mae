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
    // Split pair: *AI* (output) + *ai-input* (input) = 3 buffers total
    assert_eq!(editor.buffers.len(), 3);
    assert_eq!(
        editor.buffers[1].kind,
        crate::buffer::BufferKind::Conversation
    );
    assert_eq!(editor.buffers[1].name, "*AI*");
    assert_eq!(editor.buffers[2].name, "*ai-input*");
    // Active buffer is the input buffer
    assert_eq!(editor.active_buffer_idx(), 2);
}

#[test]
fn ai_prompt_reuses_existing_conversation() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    // Split pair: *AI* + *ai-input* = 3 buffers
    assert_eq!(editor.buffers.len(), 3);

    // Go back to normal mode and switch to scratch buffer
    editor.mode = Mode::Normal;
    editor.window_mgr.focused_window_mut().buffer_idx = 0;

    // Second ai-prompt should reuse, not create another
    editor.dispatch_builtin("ai-prompt");
    assert_eq!(editor.buffers.len(), 3);
    // Active buffer is the input buffer
    assert_eq!(editor.active_buffer_idx(), 2);
    assert_eq!(editor.mode, Mode::ConversationInput);
}

#[test]
fn ai_cancel_when_streaming() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    // Simulate streaming state
    if let Some(conv) = editor.buffers[1].conversation_mut() {
        conv.streaming = true;
        conv.streaming_start = Some(std::time::Instant::now());
    }
    editor.dispatch_builtin("ai-cancel");
    let conv = editor.buffers[1].conversation().unwrap();
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
fn ai_prompt_creates_split_pair() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    let pair = editor
        .conversation_pair
        .as_ref()
        .expect("pair should exist");
    assert_eq!(pair.output_buffer_idx, 1);
    assert_eq!(pair.input_buffer_idx, 2);
    assert_eq!(editor.buffers[1].name, "*AI*");
    assert_eq!(editor.buffers[2].name, "*ai-input*");
    // Two windows: output (top) + input (bottom)
    assert!(editor.window_mgr.window(pair.output_window_id).is_some());
    assert!(editor.window_mgr.window(pair.input_window_id).is_some());
}

#[test]
fn ai_prompt_input_cursor_follows_text() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    let pair = editor.conversation_pair.as_ref().unwrap().clone();

    // Should be in ConversationInput mode with focus on input window.
    assert_eq!(editor.mode, Mode::ConversationInput);
    assert_eq!(editor.window_mgr.focused_id(), pair.input_window_id);
    assert_eq!(editor.active_buffer_idx(), pair.input_buffer_idx);

    // Cursor starts at (0, 0).
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 0);
    assert_eq!(win.cursor_col, 0);

    // Type some characters.
    let buf = &mut editor.buffers[pair.input_buffer_idx];
    let win = editor.window_mgr.focused_window_mut();
    buf.insert_char(win, 'h');
    buf.insert_char(win, 'i');

    // Cursor should have advanced to col 2.
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_col, 2, "cursor_col should follow typed text");
    assert_eq!(win.cursor_row, 0);

    // Buffer should contain "hi".
    assert_eq!(editor.buffers[pair.input_buffer_idx].text(), "hi");
}

#[test]
fn ai_input_newline_survives_clamp_all_cursors() {
    // Regression: clamp_all_cursors used display_line_count() which excluded the
    // trailing phantom line after '\n', clamping cursor from row 1 back to row 0.
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    let pair = editor.conversation_pair.as_ref().unwrap().clone();
    let buf = &mut editor.buffers[pair.input_buffer_idx];
    let win = editor.window_mgr.focused_window_mut();
    buf.insert_char(win, 'h');
    buf.insert_char(win, 'i');
    buf.insert_char(win, '\n');
    assert_eq!(win.cursor_row, 1);

    editor.clamp_all_cursors(); // pre-render safety net

    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 1, "newline cursor must survive clamping");
}

#[test]
fn ai_input_newline_after_clear_survives_clamp() {
    // Regression: after clear_input_buffer (submit) + retype + newline,
    // cursor must stay on the new line through clamp_all_cursors.
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    let pair = editor.conversation_pair.as_ref().unwrap().clone();

    // Simulate what submit_conversation_prompt does: clear the input buffer.
    editor.buffers[pair.input_buffer_idx].replace_contents("");
    if let Some(win) = editor.window_mgr.window_mut(pair.input_window_id) {
        win.cursor_row = 0;
        win.cursor_col = 0;
    }

    // Retype and insert newline.
    let buf = &mut editor.buffers[pair.input_buffer_idx];
    let win = editor.window_mgr.window_mut(pair.input_window_id).unwrap();
    buf.insert_char(win, 'x');
    buf.insert_char(win, '\n');
    assert_eq!(win.cursor_row, 1);

    editor.clamp_all_cursors();

    let win = editor.window_mgr.window(pair.input_window_id).unwrap();
    assert_eq!(
        win.cursor_row, 1,
        "post-clear newline cursor must survive clamping"
    );
}

#[test]
fn ai_prompt_i_in_output_redirects_to_input() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    let pair = editor.conversation_pair.as_ref().unwrap().clone();
    // Switch to normal mode in the output window.
    editor.set_mode(Mode::Normal);
    editor.window_mgr.set_focused(pair.output_window_id);
    // The output buffer is *AI* (conversation kind).
    assert_eq!(editor.buffers[editor.active_buffer_idx()].name, "*AI*");
}

#[test]
fn kill_conversation_buffer_closes_both() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("ai-prompt");
    assert_eq!(editor.buffers.len(), 3);
    assert!(editor.conversation_pair.is_some());
    // Kill the output buffer.
    editor.set_mode(Mode::Normal);
    editor.switch_to_buffer(1);
    editor.dispatch_builtin("force-kill-buffer");
    // Both buffers and the pair should be gone.
    assert!(editor.conversation_pair.is_none());
    assert_eq!(editor.buffers.len(), 1);
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

// ===== Chained ex commands (v0.6.0) =====

#[test]
fn wa_saves_all() {
    let dir = tempfile::tempdir().unwrap();
    let p1 = dir.path().join("a.txt");
    let p2 = dir.path().join("b.txt");
    fs::write(&p1, "aaa").unwrap();
    fs::write(&p2, "bbb").unwrap();

    let mut ed = Editor::new();
    ed.open_file(p1.to_str().unwrap());
    ed.open_file(p2.to_str().unwrap());
    // Modify both
    let idx = ed.active_buffer_idx();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[idx].insert_char(win, '!');
    ed.window_mgr.focused_window_mut().buffer_idx = 1;
    let idx = ed.active_buffer_idx();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[idx].insert_char(win, '?');
    ed.execute_command("wa");
    assert!(!ed.buffers[1].modified);
    assert!(!ed.buffers[2].modified);
    assert!(ed.status_msg.contains("Saved 2"));
}

#[test]
fn qa_refuses_if_modified() {
    let mut ed = Editor::new();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[0].insert_char(win, 'x');
    ed.execute_command("qa");
    assert!(ed.running);
    assert!(ed.status_msg.contains("No write"));
}

#[test]
fn qa_quits_if_clean() {
    let mut ed = Editor::new();
    ed.execute_command("qa");
    assert!(!ed.running);
}

#[test]
fn qa_force_quits() {
    let mut ed = Editor::new();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[0].insert_char(win, 'x');
    ed.execute_command("qa!");
    assert!(!ed.running);
}

#[test]
fn wqa_saves_all_then_quits() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("c.txt");
    fs::write(&p, "ccc").unwrap();

    let mut ed = Editor::new();
    ed.open_file(p.to_str().unwrap());
    let idx = ed.active_buffer_idx();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[idx].insert_char(win, '!');
    ed.execute_command("wqa");
    assert!(!ed.running);
    assert!(!ed.buffers[1].modified);
}

#[test]
fn xa_alias() {
    let mut ed = Editor::new();
    ed.execute_command("xa");
    assert!(!ed.running);
}

// ===== Autosave (v0.6.0) =====

#[test]
fn autosave_option_registered() {
    let ed = Editor::new();
    let (val, def) = ed.get_option("autosave_interval").unwrap();
    assert_eq!(val, "0");
    assert_eq!(def.name, "autosave_interval");
}

#[test]
fn try_autosave_saves_modified() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("auto.txt");
    fs::write(&p, "original").unwrap();

    let mut ed = Editor::new();
    ed.open_file(p.to_str().unwrap());
    let idx = ed.active_buffer_idx();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[idx].insert_char(win, '!');
    assert!(ed.buffers[idx].modified);

    ed.autosave_interval = 1;
    // Force last_autosave and last_edit_time to be old enough
    ed.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(10);
    ed.last_edit_time = std::time::Instant::now() - std::time::Duration::from_secs(10);
    let saved = ed.try_autosave();
    assert_eq!(saved, 1);
    assert!(!ed.buffers[idx].modified);
}

#[test]
fn try_autosave_skips_clean() {
    let mut ed = Editor::new();
    ed.autosave_interval = 1;
    ed.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(10);
    ed.last_edit_time = std::time::Instant::now() - std::time::Duration::from_secs(10);
    let saved = ed.try_autosave();
    assert_eq!(saved, 0);
}

#[test]
fn try_autosave_skips_non_file() {
    let mut ed = Editor::new();
    // Modify the scratch buffer (no file path)
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[0].insert_char(win, 'x');
    ed.autosave_interval = 1;
    ed.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(10);
    ed.last_edit_time = std::time::Instant::now() - std::time::Duration::from_secs(10);
    let saved = ed.try_autosave();
    assert_eq!(saved, 0);
    assert!(ed.buffers[0].modified); // still modified, not saved
}

#[test]
fn autosave_idle_debounce_skips_during_edit() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("debounce.txt");
    fs::write(&p, "original").unwrap();

    let mut ed = Editor::new();
    ed.open_file(p.to_str().unwrap());
    let idx = ed.active_buffer_idx();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[idx].insert_char(win, '!');

    ed.autosave_interval = 1;
    ed.last_autosave = std::time::Instant::now() - std::time::Duration::from_secs(10);
    // last_edit_time is very recent (just edited above) — should skip
    let saved = ed.try_autosave();
    assert_eq!(saved, 0, "should skip autosave when editing recently");
    assert!(ed.buffers[idx].modified, "buffer should still be modified");
}

// ===== Dispatch-level tests for v0.6.0 which-key parity =====

#[test]
fn focus_next_window_dispatch_cycles_focus() {
    let mut ed = Editor::new();
    ed.dispatch_builtin("split-vertical");
    assert_eq!(ed.window_mgr.window_count(), 2);
    let first = ed.window_mgr.focused_id();

    ed.dispatch_builtin("focus-next-window");
    let second = ed.window_mgr.focused_id();
    assert_ne!(first, second);

    // Wrap around
    ed.dispatch_builtin("focus-next-window");
    assert_eq!(ed.window_mgr.focused_id(), first);
}

#[test]
fn focus_next_window_single_window_noop() {
    let mut ed = Editor::new();
    let before = ed.window_mgr.focused_id();
    ed.dispatch_builtin("focus-next-window");
    assert_eq!(ed.window_mgr.focused_id(), before);
}

#[test]
fn file_info_shows_status() {
    let mut ed = Editor::new();
    ed.dispatch_builtin("file-info");
    assert!(ed.status_msg.contains("line 1 of"));
    assert!(ed.status_msg.contains("[scratch]"));
}

#[test]
fn file_info_shows_modified_flag() {
    let mut ed = Editor::new();
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[0].insert_char(win, 'x');
    ed.dispatch_builtin("file-info");
    assert!(ed.status_msg.contains("[+]"));
}

#[test]
fn file_info_shows_file_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.txt");
    fs::write(&path, "hello\nworld\n").unwrap();
    let buf = Buffer::from_file(&path).unwrap();
    let mut ed = Editor::with_buffer(buf);
    ed.dispatch_builtin("file-info");
    assert!(ed.status_msg.contains("test.txt"));
    assert!(ed.status_msg.contains("line 1 of"));
}

#[test]
fn save_all_and_quit_saves_then_quits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    fs::write(&path, "original").unwrap();
    let buf = Buffer::from_file(&path).unwrap();
    let mut ed = Editor::with_buffer(buf);
    // Modify the buffer
    let win = ed.window_mgr.focused_window_mut();
    ed.buffers[0].insert_char(win, '!');
    assert!(ed.buffers[0].modified);

    ed.dispatch_builtin("save-all-and-quit");
    // Should have saved and set running = false
    assert!(!ed.running);
    let content = fs::read_to_string(&path).unwrap();
    assert!(content.contains("!"));
}

#[test]
fn copy_this_file_enters_command_mode() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.txt");
    fs::write(&path, "content").unwrap();
    let buf = Buffer::from_file(&path).unwrap();
    let mut ed = Editor::with_buffer(buf);

    ed.dispatch_builtin("copy-this-file");
    assert_eq!(ed.mode, Mode::CommandPalette);
    // Should open a MiniDialog with the source path pre-filled.
    assert!(ed.mini_dialog.is_some());
}

#[test]
fn copy_this_file_no_path_shows_error() {
    let mut ed = Editor::new();
    ed.dispatch_builtin("copy-this-file");
    assert!(ed.status_msg.contains("no file path"));
}

#[test]
fn copy_ex_command_copies_and_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("src.txt");
    fs::write(&path, "hello").unwrap();
    let buf = Buffer::from_file(&path).unwrap();
    let mut ed = Editor::with_buffer(buf);

    let dest = dir.path().join("dst.txt");
    ed.execute_command(&format!("copy {}", dest.display()));
    assert!(dest.exists());
    assert_eq!(fs::read_to_string(&dest).unwrap(), "hello");
    // Should have opened the copy
    assert!(ed.buffers.iter().any(|b| {
        b.file_path()
            .map(|p| p.ends_with("dst.txt"))
            .unwrap_or(false)
    }));
}

#[test]
fn file_tree_open_vsplit_opens_in_split() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("test.rs"), "fn main() {}").unwrap();
    let mut ed = Editor::new();

    // Manually set up a file tree buffer
    let tree_buf = Buffer::new_file_tree(dir.path());
    let tree_buf_idx = ed.buffers.len();
    ed.buffers.push(tree_buf);
    ed.window_mgr.focused_window_mut().buffer_idx = tree_buf_idx;
    ed.file_tree_window_id = Some(ed.window_mgr.focused_id());

    // Split to have a content window
    ed.dispatch_builtin("split-vertical");
    let content_win_count = ed.window_mgr.window_count();

    // Select the test.rs file in the tree
    let ft = ed.buffers[tree_buf_idx].file_tree_mut().unwrap();
    if let Some(idx) = ft.entries.iter().position(|e| e.name == "test.rs") {
        ft.selected = idx;
    }

    // Switch back to tree window for dispatch
    ed.window_mgr.set_focused(ed.file_tree_window_id.unwrap());

    ed.dispatch_builtin("file-tree-open-vsplit");
    // Should have created a new split
    assert!(ed.window_mgr.window_count() > content_win_count);
}

#[test]
fn file_tree_reveal_on_toggle() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/util")).unwrap();
    let file_path = dir.path().join("src/util/deep.rs");
    fs::write(&file_path, "fn deep() {}").unwrap();

    let buf = Buffer::from_file(&file_path).unwrap();
    let mut ed = Editor::with_buffer(buf);
    // Editor needs a project root for file tree
    ed.project = Some(crate::project::Project::from_root(dir.path().to_path_buf()));

    ed.dispatch_builtin("file-tree-toggle");

    // Find the tree buffer
    let tree_idx = ed
        .buffers
        .iter()
        .position(|b| b.kind == crate::BufferKind::FileTree);
    if let Some(ti) = tree_idx {
        let ft = ed.buffers[ti].file_tree().unwrap();
        // Should have expanded src and src/util
        assert!(ft.expanded_dirs.contains(&dir.path().join("src")));
        assert!(ft.expanded_dirs.contains(&dir.path().join("src/util")));
        // Selected entry should be our deep file
        assert_eq!(ft.entries[ft.selected].name, "deep.rs");
    } else {
        panic!("File tree buffer not created");
    }
}

// ===== Operator-pending mode tests (WU0) =====
