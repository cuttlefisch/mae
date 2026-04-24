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
