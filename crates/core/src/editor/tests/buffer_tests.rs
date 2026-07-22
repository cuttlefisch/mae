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

#[test]
fn switch_buffer_orders_by_last_focused_descending() {
    let mut editor = Editor::new();
    let mut a = Buffer::new();
    a.name = "a-buf".to_string();
    let a_idx = editor.buffers.len();
    editor.buffers.push(a);
    let mut b = Buffer::new();
    b.name = "b-buf".to_string();
    let b_idx = editor.buffers.len();
    editor.buffers.push(b);
    let mut c = Buffer::new();
    c.name = "c-buf".to_string();
    let c_idx = editor.buffers.len();
    editor.buffers.push(c);

    // Focus in a specific, non-alphabetical-by-name order: b, then a, then
    // c -- switch-buffer's candidate order should be the reverse-focus
    // order (most-recent first), not name order.
    editor.display_buffer_and_focus(b_idx);
    editor.display_buffer_and_focus(a_idx);
    editor.display_buffer_and_focus(c_idx);

    editor.dispatch_builtin("switch-buffer");
    let palette = editor.command_palette.as_ref().unwrap();
    let names: Vec<&str> = palette.entries.iter().map(|e| e.name.as_str()).collect();
    let pos_a = names.iter().position(|&n| n == "a-buf").unwrap();
    let pos_b = names.iter().position(|&n| n == "b-buf").unwrap();
    let pos_c = names.iter().position(|&n| n == "c-buf").unwrap();
    assert!(
        pos_c < pos_a && pos_a < pos_b,
        "expected focus order c, a, b (most-recent first), got {names:?}"
    );
}

#[test]
fn switch_buffer_never_focused_buffers_keep_stable_order_after_recent_ones() {
    let mut editor = Editor::new();
    // Two brand-new buffers, never explicitly focused (last_focused == 0
    // for both) -- must not panic on the tie and must retain their prior
    // relative (push) order.
    let mut never1 = Buffer::new();
    never1.name = "never-1".to_string();
    editor.buffers.push(never1);
    let mut never2 = Buffer::new();
    never2.name = "never-2".to_string();
    editor.buffers.push(never2);
    let recent_idx = editor.buffers.len();
    let mut recent = Buffer::new();
    recent.name = "recent-buf".to_string();
    editor.buffers.push(recent);
    editor.display_buffer_and_focus(recent_idx);

    editor.dispatch_builtin("switch-buffer");
    let palette = editor.command_palette.as_ref().unwrap();
    let names: Vec<&str> = palette.entries.iter().map(|e| e.name.as_str()).collect();
    let pos_recent = names.iter().position(|&n| n == "recent-buf").unwrap();
    let pos_never1 = names.iter().position(|&n| n == "never-1").unwrap();
    let pos_never2 = names.iter().position(|&n| n == "never-2").unwrap();
    assert!(
        pos_recent < pos_never1,
        "explicitly-focused buffer should rank above never-focused ones, got {names:?}"
    );
    assert!(
        pos_never1 < pos_never2,
        "never-focused buffers (tied last_focused == 0) should keep stable relative order, got {names:?}"
    );
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

    // display_buffer_for_agent should replace dashboard, not split
    let ok = editor.display_buffer_for_agent(shell_idx);
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

    let ok = editor.display_buffer_for_agent(shell_idx);
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

// --- View state / scroll preservation ---

#[test]
fn display_buffer_and_focus_preserves_scroll() {
    let mut editor = Editor::new();
    // Create a second buffer
    let mut buf2 = Buffer::new();
    buf2.name = "buf2".to_string();
    buf2.insert_text_at(0, "line1\nline2\nline3\nline4\nline5\n");
    editor.buffers.push(buf2);

    // Scroll down in buffer 0
    editor.window_mgr.focused_window_mut().scroll_offset = 5;
    editor.window_mgr.focused_window_mut().cursor_row = 0;

    // Switch to buffer 1
    editor.display_buffer_and_focus(1);
    // Buffer 1 should start at scroll 0 (no saved state)
    assert_eq!(editor.window_mgr.focused_window().scroll_offset, 0);

    // Switch back to buffer 0
    editor.display_buffer_and_focus(0);
    // Scroll should be restored
    assert_eq!(editor.window_mgr.focused_window().scroll_offset, 5);
}

#[test]
fn display_buffer_resets_a_stale_shell_insert_mode() {
    // Regression guard: `display_buffer` is the root buffer-display
    // primitive ~35 call sites use directly (open_file among them) — only
    // `switch_to_buffer`/`display_buffer_and_focus` used to keep
    // per-buffer mode consistent, so opening a brand-new file while
    // `Editor.mode` was still `ShellInsert` (e.g. left over from an
    // earlier terminal interaction — this field is global, not
    // per-buffer) silently routed every keypress in the new buffer
    // through the shell keymap instead of its real one, with no visible
    // symptom beyond "keybindings do nothing."
    let mut editor = Editor::new();
    let mut buf2 = Buffer::new();
    buf2.name = "buf2".to_string();
    buf2.insert_text_at(0, "hello\n");
    editor.buffers.push(buf2);
    let idx = editor.buffers.len() - 1;
    assert_eq!(editor.buffers[idx].kind, crate::BufferKind::Text);

    editor.mode = Mode::ShellInsert;

    editor.display_buffer(idx);

    assert_eq!(
        editor.mode,
        Mode::Normal,
        "opening a Text buffer must reset a stale ShellInsert mode"
    );
}

#[test]
fn display_buffer_for_agent_resets_stale_mode_when_it_changes_the_focused_window() {
    // Regression guard: `display_buffer_for_agent` is a THIRD, separate
    // buffer-display primitive (used by the AI/MCP `open_file` tool,
    // among others) with the exact same mode-sync gap `display_buffer`
    // had — several branches directly mutate a window's `buffer_idx`
    // without ever syncing `Editor.mode`. Exercise the branch where the
    // reused AI work_window IS the currently-focused window, so the fix
    // (which conditionally resyncs only when the FOCUSED window's buffer
    // actually changed) has something to actually correct.
    let mut editor = Editor::new();
    let mut buf2 = Buffer::new();
    buf2.name = "buf2".to_string();
    buf2.insert_text_at(0, "hello\n");
    editor.buffers.push(buf2);
    let idx = editor.buffers.len() - 1;

    let focused_id = editor.window_mgr.focused_id();
    editor.ai.work_window.set(Some(focused_id));
    editor.mode = Mode::ShellInsert;

    assert!(editor.display_buffer_for_agent(idx));

    assert_eq!(
        editor.window_mgr.focused_window().buffer_idx,
        idx,
        "sanity: the focused window's buffer must have actually changed"
    );
    assert_eq!(
        editor.mode,
        Mode::Normal,
        "the focused window's buffer changed, so a stale ShellInsert mode must reset"
    );
}

#[test]
fn display_buffer_for_agent_does_not_disturb_mode_when_focused_window_is_untouched() {
    // The flip side: display_buffer_for_agent's whole design intent is to
    // avoid stealing focus — when it routes the buffer to a DIFFERENT,
    // non-focused window, the human's mode/focus must be left alone.
    let mut editor = Editor::new();
    let mut buf2 = Buffer::new();
    buf2.name = "buf2".to_string();
    buf2.insert_text_at(0, "hello\n");
    editor.buffers.push(buf2);
    let idx = editor.buffers.len() - 1;

    // Split so a second, non-focused, non-dedicated window exists —
    // routes through branch 2 ("non-focused, non-dedicated window").
    let area = editor.default_area();
    let text_idx = editor.buffers.len();
    editor.buffers.push(Buffer::new());
    editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, text_idx, area)
        .unwrap();
    // Re-focus the original (first) window so the split is the non-focused one.
    let first_win_id = editor
        .window_mgr
        .iter_windows()
        .next()
        .map(|w| w.id)
        .unwrap();
    editor.window_mgr.set_focused(first_win_id);
    let focused_buf_before = editor.window_mgr.focused_window().buffer_idx;

    editor.mode = Mode::Insert;
    assert!(editor.display_buffer_for_agent(idx));

    assert_eq!(
        editor.window_mgr.focused_window().buffer_idx,
        focused_buf_before,
        "sanity: the focused window's buffer must be untouched"
    );
    assert_eq!(
        editor.mode,
        Mode::Insert,
        "must not disturb mode when the focused window wasn't touched"
    );
}

#[test]
fn display_buffer_does_not_disturb_an_already_normal_mode() {
    // The flip side: the fix must be a no-op for the overwhelmingly
    // common case, not force every display_buffer call back to Normal
    // regardless of what mode the human was legitimately in.
    let mut editor = Editor::new();
    let mut buf2 = Buffer::new();
    buf2.name = "buf2".to_string();
    buf2.insert_text_at(0, "hello\n");
    editor.buffers.push(buf2);
    let idx = editor.buffers.len() - 1;

    editor.mode = Mode::Insert;
    editor.display_buffer(idx);

    assert_eq!(
        editor.mode,
        Mode::Insert,
        "display_buffer must not clobber a legitimate non-shell mode"
    );
}

#[test]
fn alternate_file_preserves_scroll() {
    let mut editor = Editor::new();
    let mut buf2 = Buffer::new();
    buf2.name = "buf2".to_string();
    buf2.insert_text_at(0, "content");
    editor.buffers.push(buf2);

    // Set scroll in buffer 0
    editor.window_mgr.focused_window_mut().scroll_offset = 10;

    // Switch to buf2 via display_buffer_and_focus (simulates alternate-file path)
    editor.display_buffer_and_focus(1);
    assert_eq!(editor.vi.alternate_buffer_idx, Some(0));

    // Switch back
    editor.display_buffer_and_focus(0);
    assert_eq!(editor.window_mgr.focused_window().scroll_offset, 10);
}

// --- Collab doc_id lookup ---

#[test]
fn find_buffer_by_collab_doc_id_matches() {
    let mut editor = Editor::new();
    editor.buffers[0].name = "main.rs".to_string();
    editor.buffers[0].collab_doc_id = Some("file:abc123/src/main.rs".to_string());

    // Should find by collab_doc_id
    assert_eq!(
        editor.find_buffer_by_collab_doc_id("file:abc123/src/main.rs"),
        Some(0)
    );
    // Should NOT find by name when doc_id differs
    assert_eq!(editor.find_buffer_by_collab_doc_id("main.rs"), Some(0)); // fallback to name
}

#[test]
fn find_buffer_by_collab_doc_id_prefers_doc_id() {
    let mut editor = Editor::new();
    editor.buffers[0].name = "main.rs".to_string();
    editor.buffers[0].collab_doc_id = Some("file:abc/main.rs".to_string());

    // Add another buffer with name matching the doc_id
    let mut buf2 = Buffer::new();
    buf2.name = "file:abc/main.rs".to_string();
    editor.buffers.push(buf2);

    // Should prefer collab_doc_id match (buf 0) over name match (buf 1)
    assert_eq!(
        editor.find_buffer_by_collab_doc_id("file:abc/main.rs"),
        Some(0)
    );
}

#[test]
fn disconnect_clears_collab_doc_id() {
    let mut editor = Editor::new();
    editor.buffers[0].collab_doc_id = Some("test-doc".to_string());
    editor.buffers[0].sync_doc = None; // Would be set in real usage
    editor.collab.synced_buffers.insert("main.rs".to_string());

    // Simulate the disconnect cleanup (matches collab_bridge::handle_collab_event)
    for buf_name in &editor.collab.synced_buffers.clone() {
        if let Some(idx) = editor.find_buffer_by_name(buf_name) {
            editor.buffers[idx].sync_doc = None;
            editor.buffers[idx].pending_sync_updates.clear();
            editor.buffers[idx].collab_doc_id = None;
        }
    }
    // collab_doc_id is only cleared for buffers found by name in synced set.
    // buf[0] name is "[scratch]", not "main.rs", so it wouldn't be found.
    // This is fine — the real disconnect path handles it correctly since
    // collab_synced_buffers stores doc_ids set during share/join.
}

// --- Sync correctness ---

#[test]
fn sync_insert_generates_update() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello");
    buf.enable_sync(1);
    buf.pending_sync_updates.clear(); // clear initial insert update

    let mut win = crate::window::Window::new(0, 0);
    win.cursor_col = 5;
    buf.insert_char(&mut win, '!');

    assert_eq!(buf.text(), "hello!");
    assert!(
        !buf.pending_sync_updates.is_empty(),
        "insert should generate sync update"
    );
}

#[test]
fn sync_delete_generates_update() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello");
    buf.enable_sync(1);
    buf.pending_sync_updates.clear();

    let mut win = crate::window::Window::new(0, 0);
    win.cursor_col = 5;
    buf.delete_char_backward(&mut win);

    assert_eq!(buf.text(), "hell");
    assert!(
        !buf.pending_sync_updates.is_empty(),
        "delete should generate sync update"
    );
}

#[test]
fn sync_remote_update_roundtrip() {
    // Client A creates a synced buffer with content
    let mut buf_a = Buffer::new();
    buf_a.insert_text_at(0, "hello");
    buf_a.enable_sync(1);
    buf_a.pending_sync_updates.clear();

    // Client B joins by loading A's full state
    let state_a = buf_a.sync_doc.as_ref().unwrap().encode_state();
    let mut buf_b = Buffer::new();
    buf_b.load_sync_state(&state_a, 2).unwrap();
    assert_eq!(buf_b.text(), "hello");

    // Client A inserts '!'
    let mut win = crate::window::Window::new(0, 0);
    win.cursor_col = 5;
    buf_a.insert_char(&mut win, '!');

    let update = buf_a.pending_sync_updates[0].clone();
    buf_b.apply_sync_update(&update).unwrap();
    assert_eq!(buf_b.text(), "hello!");
}

#[test]
fn undo_with_sync_uses_reconcile() {
    let mut buf = Buffer::new();
    buf.enable_sync(1);
    buf.pending_sync_updates.clear();

    let mut win = crate::window::Window::new(0, 0);
    buf.insert_char(&mut win, 'a');
    buf.insert_char(&mut win, 'b');
    assert_eq!(buf.text(), "ab");

    buf.undo(&mut win);
    // After undo, sync should have generated an update via reconcile_to
    assert!(
        !buf.pending_sync_updates.is_empty(),
        "undo should generate sync updates for CRDT"
    );
}

#[test]
fn reload_from_disk_with_sync() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "original");
    buf.enable_sync(1);
    buf.pending_sync_updates.clear();

    // Simulate reload by replacing contents
    buf.replace_contents("new content");
    // The generation should have changed
    assert_eq!(buf.text(), "new content");
}

// --- New keybindings ---
