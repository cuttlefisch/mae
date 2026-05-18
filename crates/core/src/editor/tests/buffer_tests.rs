use super::*;
use crate::buffer::Buffer;

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
fn install_dashboard_inserts_at_front() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    assert_eq!(editor.buffers.len(), 2);
    assert_eq!(editor.buffers[0].kind, crate::BufferKind::Dashboard);
    assert_eq!(editor.buffers[0].name, "[dashboard]");
    assert_eq!(editor.buffers[1].name, "[scratch]");
    assert_eq!(editor.active_buffer_idx(), 0);
}

#[test]
fn dashboard_command_finds_existing() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    // Switch away from dashboard.
    editor.window_mgr.focused_window_mut().buffer_idx = 1;
    assert_eq!(editor.active_buffer().name, "[scratch]");
    // :dashboard should return to it.
    editor.execute_command("dashboard");
    assert_eq!(editor.active_buffer().kind, crate::BufferKind::Dashboard);
}

#[test]
fn dashboard_command_creates_if_missing() {
    let mut editor = Editor::new();
    // No dashboard installed.
    assert_eq!(editor.buffers.len(), 1);
    editor.execute_command("dashboard");
    assert_eq!(editor.buffers.len(), 2);
    assert_eq!(editor.active_buffer().kind, crate::BufferKind::Dashboard);
}

#[test]
fn toggle_scratch_buffer_switches() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    // From dashboard, toggle should go to scratch.
    editor.execute_command("toggle-scratch-buffer");
    assert_eq!(editor.active_buffer().name, "[scratch]");
    // From scratch, toggle should go back.
    editor.execute_command("toggle-scratch-buffer");
    assert_ne!(editor.active_buffer().name, "[scratch]");
}

#[test]
fn dashboard_blocks_insert_and_visual_mode() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    assert_eq!(editor.mode, crate::Mode::Normal);

    // Insert mode should be blocked.
    editor.set_mode(crate::Mode::Insert);
    assert_eq!(editor.mode, crate::Mode::Normal);

    // Visual mode should be blocked.
    editor.set_mode(crate::Mode::Visual(crate::VisualType::Char));
    assert_eq!(editor.mode, crate::Mode::Normal);

    // Command mode should still work (needed for : commands).
    editor.set_mode(crate::Mode::Command);
    assert_eq!(editor.mode, crate::Mode::Command);
    editor.set_mode(crate::Mode::Normal);
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
    // Should now be on a valid non-sidebar buffer (either 0 or 1).
    let idx = editor.active_buffer_idx();
    assert!(idx < editor.buffers.len());
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

// --- Buffer switch position preservation ---

#[test]
fn switch_buffer_preserves_cursor_position() {
    let mut editor = Editor::new();
    editor.buffers[0].replace_rope(ropey::Rope::from_str("line 0\nline 1\nline 2\nline 3\n"));
    let mut buf1 = Buffer::new();
    buf1.replace_rope(ropey::Rope::from_str("other file\n"));
    editor.buffers.push(buf1);

    // Move cursor in buffer 0 to (2, 3)
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 3;
    win.scroll_offset = 1;

    // Switch to buffer 1
    editor.switch_to_buffer(1);
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);

    // Switch back to buffer 0 — position should be restored
    editor.switch_to_buffer(0);
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 2);
    assert_eq!(win.cursor_col, 3);
    assert_eq!(win.scroll_offset, 1);
}

#[test]
fn switch_buffer_clamps_to_shrunk_file() {
    let mut editor = Editor::new();
    editor.buffers[0].replace_rope(ropey::Rope::from_str("line 0\nline 1\nline 2\n"));
    let mut buf1 = Buffer::new();
    buf1.replace_rope(ropey::Rope::from_str("x\n"));
    editor.buffers.push(buf1);

    // Position at line 2 in buffer 0
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 3;

    // Switch away, shrink buffer 0
    editor.switch_to_buffer(1);
    editor.buffers[0].replace_rope(ropey::Rope::from_str("short\n"));

    // Switch back — should clamp to valid position
    editor.switch_to_buffer(0);
    let win = editor.window_mgr.focused_window();
    assert!(win.cursor_row < editor.buffers[0].line_count());
}

#[test]
fn next_prev_buffer_preserves_position() {
    let mut editor = Editor::new();
    editor.buffers[0].replace_rope(ropey::Rope::from_str("aaa\nbbb\nccc\n"));
    let mut buf1 = Buffer::new();
    buf1.replace_rope(ropey::Rope::from_str("xxx\n"));
    editor.buffers.push(buf1);

    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    // next-buffer saves state + cycles
    editor.dispatch_builtin("next-buffer");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
    // Switch back
    editor.dispatch_builtin("prev-buffer");
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
}

// Dashboard dismiss on split tests
// ---------------------------------------------------------------------------

#[test]
fn dashboard_default_stays_on_split() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    // Default: replaceable_kinds is empty (dashboard stays)
    assert!(!editor.is_kind_replaceable(crate::BufferKind::Dashboard));

    // Create a Help buffer and display it (Help uses ReuseOrSplit)
    let mut help_buf = Buffer::new();
    help_buf.kind = crate::BufferKind::Kb;
    help_buf.name = "[help]".into();
    editor.buffers.push(help_buf);
    let help_idx = editor.buffers.len() - 1;

    // Set layout area large enough for splits
    editor.last_layout_area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    editor.display_buffer(help_idx);

    // Dashboard window should still exist (Doom parity)
    let has_dashboard_win = editor.window_mgr.iter_windows().any(|w| {
        w.buffer_idx < editor.buffers.len()
            && editor.buffers[w.buffer_idx].kind == crate::BufferKind::Dashboard
    });
    assert!(
        has_dashboard_win,
        "Dashboard should stay when dismiss_on_split=false"
    );
}

#[test]
fn dashboard_dismissed_when_option_set() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    editor.replaceable_kinds.push(crate::BufferKind::Dashboard);

    // Create a Help buffer and display it
    let mut help_buf = Buffer::new();
    help_buf.kind = crate::BufferKind::Kb;
    help_buf.name = "[help]".into();
    editor.buffers.push(help_buf);
    let help_idx = editor.buffers.len() - 1;

    editor.last_layout_area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };
    editor.display_buffer(help_idx);

    // No window should show the dashboard anymore (it was replaced, not split)
    let has_dashboard_win = editor.window_mgr.iter_windows().any(|w| {
        w.buffer_idx < editor.buffers.len()
            && editor.buffers[w.buffer_idx].kind == crate::BufferKind::Dashboard
    });
    assert!(
        !has_dashboard_win,
        "Dashboard should be replaced when option is set"
    );

    // The window should now show the KB buffer
    let has_help_win = editor
        .window_mgr
        .iter_windows()
        .any(|w| w.buffer_idx == help_idx);
    assert!(has_help_win, "Help buffer should be visible");
}

// Replaceable window tests
// ---------------------------------------------------------------------------

#[test]
fn dashboard_replaced_by_agent_shell_when_replaceable() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    editor.replaceable_kinds.push(crate::BufferKind::Dashboard);

    // Set layout area large enough for splits
    editor.last_layout_area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };

    // Create an agent shell buffer
    let mut shell_buf = Buffer::new_shell("*AI:claude*");
    shell_buf.agent_shell = true;
    editor.buffers.push(shell_buf);
    let shell_idx = editor.buffers.len() - 1;

    // switch_to_buffer_non_conversation should replace dashboard, not split
    let ok = editor.switch_to_buffer_non_conversation(shell_idx);
    assert!(ok, "switch should succeed");

    // Should still be 1 window (dashboard replaced), not 2 (split)
    assert_eq!(
        editor.window_mgr.window_count(),
        1,
        "Dashboard should be replaced, not split alongside"
    );

    // The single window should show the shell buffer
    let win = editor.window_mgr.focused_window();
    assert_eq!(
        win.buffer_idx, shell_idx,
        "Window should show the agent shell"
    );
}

#[test]
fn dashboard_stays_when_not_replaceable() {
    let mut editor = Editor::new();
    editor.install_dashboard();
    // Default: replaceable_kinds is empty

    editor.last_layout_area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };

    let mut shell_buf = Buffer::new_shell("*AI:claude*");
    shell_buf.agent_shell = true;
    editor.buffers.push(shell_buf);
    let shell_idx = editor.buffers.len() - 1;

    let ok = editor.switch_to_buffer_non_conversation(shell_idx);
    assert!(ok, "switch should succeed");

    // Should have 2 windows (split alongside dashboard)
    assert_eq!(
        editor.window_mgr.window_count(),
        2,
        "Dashboard should stay — split alongside it"
    );
}

#[test]
fn kill_other_buffers_preserves_sidebar() {
    let mut editor = Editor::new();
    editor.install_dashboard();

    // Add a Messages buffer (sidebar kind)
    let mut msg_buf = Buffer::new();
    msg_buf.kind = crate::BufferKind::Messages;
    msg_buf.name = "*Messages*".into();
    editor.buffers.push(msg_buf);

    // Add a Debug buffer (sidebar kind)
    let mut dbg_buf = Buffer::new();
    dbg_buf.kind = crate::BufferKind::Debug;
    dbg_buf.name = "*Debug*".into();
    editor.buffers.push(dbg_buf);

    // Add two text buffers
    let mut text1 = Buffer::new();
    text1.name = "file1.rs".into();
    editor.buffers.push(text1);
    let mut text2 = Buffer::new();
    text2.name = "file2.rs".into();
    editor.buffers.push(text2);

    // Focus on one of the text buffers
    let text1_idx = editor.buffers.len() - 2;
    editor.window_mgr.focused_window_mut().buffer_idx = text1_idx;

    editor.dispatch_builtin("kill-other-buffers");

    // Dashboard, Messages, Debug should survive (sidebar kinds)
    let kinds: Vec<_> = editor.buffers.iter().map(|b| b.kind).collect();
    assert!(
        kinds.contains(&crate::BufferKind::Dashboard),
        "Dashboard should survive kill-other-buffers"
    );
    assert!(
        kinds.contains(&crate::BufferKind::Messages),
        "Messages should survive kill-other-buffers"
    );
    assert!(
        kinds.contains(&crate::BufferKind::Debug),
        "Debug should survive kill-other-buffers"
    );

    // The other text buffer (file2.rs) should be killed
    let names: Vec<_> = editor.buffers.iter().map(|b| b.name.as_str()).collect();
    assert!(
        !names.contains(&"file2.rs"),
        "Non-active text buffer should be killed"
    );
}

#[test]
fn scratch_buffer_guaranteed_after_kill() {
    let mut editor = Editor::new();
    // Start with only the default [scratch]
    assert_eq!(editor.buffers.len(), 1);

    // Replace it with a sidebar-only buffer
    editor.buffers[0].kind = crate::BufferKind::Dashboard;
    editor.buffers[0].name = "[dashboard]".into();

    editor.ensure_scratch_exists();

    // Should have created a new scratch buffer
    let has_text = editor
        .buffers
        .iter()
        .any(|b| !b.kind.is_sidebar() && b.kind != crate::BufferKind::Dashboard);
    assert!(
        has_text,
        "ensure_scratch_exists should create a [scratch] buffer"
    );
}

// --- New keybindings ---
