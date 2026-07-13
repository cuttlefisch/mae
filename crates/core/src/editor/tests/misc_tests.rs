use super::*;
use crate::buffer::Buffer;
use std::fs;

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
    let mut stats = crate::editor::perf::PerfStats::default();
    for i in 0..10 {
        stats.record_frame((i + 1) * 1000);
    }
    // Average of 1000..10000 = 5500
    assert_eq!(stats.avg_frame_time_us, 5500);
    assert_eq!(stats.frame_time_us, 10000);
}

#[test]
fn perf_stats_default_zeroed() {
    let stats = crate::editor::perf::PerfStats::default();
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

// ---- display_buffer_for_agent tests ----

#[test]
fn test_switch_non_conv_normal_window() {
    // When focused window is NOT conversation, it still avoids stealing focus
    // by splitting or using another window.
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    assert!(!editor.is_conversation_buffer(editor.active_buffer_idx()));
    let ok = editor.display_buffer_for_agent(1);
    assert!(ok);
    // Focus remains on buffer 0
    assert_eq!(editor.active_buffer_idx(), 0);
    // Buffer 1 is now visible in another window (the split)
    assert!(editor.window_mgr.iter_windows().any(|w| w.buffer_idx == 1));
}

#[test]
fn test_switch_non_conv_routes_to_other_window() {
    // With a split, if conversation is focused, the new buffer goes to the other pane.
    let mut editor = Editor::new();
    // Create a conversation buffer.
    let conv_idx = editor.ensure_conversation_buffer_idx();
    editor.switch_to_buffer(conv_idx);
    // Split vertically so there are two windows.
    let area = editor.default_area();
    let new_id = editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 0, area)
        .expect("split should succeed");
    // Focus the conversation window (not the new split).
    // The focused window should still be on conv_idx after split — split
    // doesn't change focus.
    assert_eq!(editor.active_buffer_idx(), conv_idx);
    // Add a third buffer and route it.
    editor.buffers.push(Buffer::new());
    let target_idx = editor.buffers.len() - 1;
    let ok = editor.display_buffer_for_agent(target_idx);
    assert!(ok);
    // Focused window should STILL show conversation.
    assert_eq!(editor.active_buffer_idx(), conv_idx);
    // The other window should show the target buffer.
    let other_win = editor
        .window_mgr
        .window(new_id)
        .expect("split window exists");
    assert_eq!(other_win.buffer_idx, target_idx);
}

#[test]
fn test_switch_non_conv_auto_splits() {
    // Single *AI* window: auto-splits to keep conversation visible.
    let mut editor = Editor::new();
    let conv_idx = editor.ensure_conversation_buffer_idx();
    editor.switch_to_buffer(conv_idx);
    assert_eq!(editor.window_mgr.window_count(), 1);
    // Add a target buffer.
    editor.buffers.push(Buffer::new());
    let target_idx = editor.buffers.len() - 1;
    let ok = editor.display_buffer_for_agent(target_idx);
    assert!(ok);
    // Should have split into 2 windows.
    assert_eq!(editor.window_mgr.window_count(), 2);
}

#[test]
fn test_open_file_non_conv_preserves_ai() {
    // open_file_non_conversation with *AI* focused keeps conversation visible.
    let mut editor = Editor::new();
    let conv_idx = editor.ensure_conversation_buffer_idx();
    editor.switch_to_buffer(conv_idx);
    // Create a temp file.
    let dir = std::env::temp_dir().join("mae_test_open_non_conv");
    let _ = fs::create_dir_all(&dir);
    let file_path = dir.join("test.txt");
    fs::write(&file_path, "hello").unwrap();
    editor.open_file_non_conversation(file_path.to_str().unwrap());
    // Focused window should still show conversation.
    assert_eq!(editor.active_buffer_idx(), conv_idx);
    // Cleanup.
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_focus_hooks_fired() {
    let mut editor = Editor::new();
    // Register dummy functions so fire_hook actually queues something
    editor.hooks.add("focus-out", "dummy-fn");
    editor.hooks.add("focus-in", "dummy-fn");

    // Create a split so we can switch focus
    editor.buffers.push(Buffer::new());
    let area = editor.default_area();
    editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 1, area)
        .unwrap();

    editor.execute_command("focus-right");
    let hooks: Vec<_> = editor
        .pending_hook_evals
        .iter()
        .map(|(h, _)| h.as_str())
        .collect();
    assert!(hooks.contains(&"focus-out"));
    assert!(hooks.contains(&"focus-in"));
}

// --- from project_tests ---

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
fn project_switch_palette_empty_opens_palette() {
    let mut editor = Editor::new();
    editor.dispatch_builtin("project-switch");
    // Even with no recent projects, the palette opens so the user can type a path
    assert!(editor.command_palette.is_some());
    let palette = editor.command_palette.as_ref().unwrap();
    assert_eq!(
        palette.purpose,
        crate::command_palette::PalettePurpose::SwitchProject
    );
    assert!(palette.entries.is_empty());
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
// State stack (push/pop) tests
// ---------------------------------------------------------------------------

#[test]
fn save_and_restore_state_basic() {
    let mut editor = Editor::new();
    assert!(editor.state_stack.is_empty());

    // Save state with 1 buffer
    let depth = editor.save_state();
    assert_eq!(depth, 1);
    assert_eq!(editor.buffers.len(), 1);

    // Open a new buffer
    let mut buf = Buffer::new();
    buf.name = "test.txt".into();
    editor.buffers.push(buf);
    assert_eq!(editor.buffers.len(), 2);

    // Restore should close the new buffer
    let result = editor.restore_state();
    assert!(result.is_ok());
    let msg = result.unwrap();
    assert!(msg.contains("closed 1 buffer"));
    assert_eq!(editor.buffers.len(), 1);
    assert!(editor.state_stack.is_empty());
}

#[test]
fn restore_state_empty_stack() {
    let mut editor = Editor::new();
    let result = editor.restore_state();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("empty"));
}

#[test]
fn save_and_restore_preserves_original_buffers() {
    let mut editor = Editor::new();
    editor.buffers[0].name = "*AI*".into();

    let mut buf2 = Buffer::new();
    buf2.name = "existing.rs".into();
    editor.buffers.push(buf2);

    editor.save_state();

    // Open test buffers
    for name in &["test1.txt", "test2.txt", "*Help*"] {
        let mut b = Buffer::new();
        b.name = name.to_string();
        editor.buffers.push(b);
    }
    assert_eq!(editor.buffers.len(), 5);

    editor.restore_state().unwrap();
    assert_eq!(editor.buffers.len(), 2);
    assert_eq!(editor.buffers[0].name, "*AI*");
    assert_eq!(editor.buffers[1].name, "existing.rs");
}

#[test]
fn save_and_restore_preserves_conversation_pair() {
    use crate::editor::ConversationPair;

    let mut editor = Editor::new();
    editor.buffers[0].name = "*AI*".into();
    let mut input_buf = Buffer::new();
    input_buf.name = "*ai-input*".into();
    editor.buffers.push(input_buf);

    // Simulate a conversation pair
    editor.ai.conversation_pair = Some(ConversationPair {
        output_buffer_idx: 0,
        input_buffer_idx: 1,
        output_window_id: 100,
        input_window_id: 101,
    });

    editor.save_state();

    // Mutate: clear the pair and add a test buffer
    editor.ai.conversation_pair = None;
    let mut test_buf = Buffer::new();
    test_buf.name = "test.txt".into();
    editor.buffers.push(test_buf);

    editor.restore_state().unwrap();

    // Conversation pair should be restored with correct (possibly remapped) indices
    let pair = editor
        .ai
        .conversation_pair
        .as_ref()
        .expect("pair should be restored");
    assert_eq!(editor.buffers[pair.output_buffer_idx].name, "*AI*");
    assert_eq!(editor.buffers[pair.input_buffer_idx].name, "*ai-input*");
    assert_eq!(pair.output_window_id, 100);
    assert_eq!(pair.input_window_id, 101);
}

// ---- Window Group + Conversation tests ----
// ---------------------------------------------------------------------------

#[test]
fn conversation_creates_group() {
    let mut editor = Editor::new();
    editor.open_conversation_buffer();
    let pair = editor
        .ai
        .conversation_pair
        .as_ref()
        .expect("pair should exist");
    assert!(
        editor.window_mgr.is_in_group(pair.output_window_id),
        "output window should be in a group"
    );
    assert!(
        editor.window_mgr.is_in_group(pair.input_window_id),
        "input window should be in a group"
    );
    assert_eq!(
        editor.window_mgr.group_label(pair.output_window_id),
        Some("conversation")
    );
}

#[test]
fn split_from_conversation_wraps_group() {
    let mut editor = Editor::new();
    editor.open_conversation_buffer();
    let pair = editor.ai.conversation_pair.as_ref().unwrap().clone();
    // Focus the input window and split to open a new buffer.
    editor.window_mgr.set_focused(pair.input_window_id);
    let area = editor.default_area();
    let new_id = editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 0, area)
        .expect("split should succeed");
    // The new window should be outside the conversation group.
    assert!(!editor.window_mgr.is_in_group(new_id));
    // The conversation windows should still be in the group.
    assert!(editor.window_mgr.is_in_group(pair.output_window_id));
    assert!(editor.window_mgr.is_in_group(pair.input_window_id));
}

// --- Bug regression: AI-opened buffer triggers full redraw (syntax highlighting)
#[test]
fn display_buffer_for_agent_triggers_full_redraw() {
    let mut editor = Editor::new();
    // Create a second buffer to switch to.
    editor.buffers.push(Buffer::new());
    let new_idx = editor.buffers.len() - 1;

    // Reset redraw level to None.
    editor.clear_redraw();
    assert_eq!(editor.redraw_level, crate::redraw::RedrawLevel::None);

    // Simulate AI opening a buffer.
    editor.display_buffer_for_agent(new_idx);

    // Must escalate to Full so syntax spans are computed for the new buffer.
    assert_eq!(
        editor.redraw_level,
        crate::redraw::RedrawLevel::Full,
        "display_buffer_for_agent must trigger Full redraw for syntax highlighting"
    );
}

// AI target dispatch tests
// ---------------------------------------------------------------------------

#[test]
fn ai_active_buffer_idx_defaults_to_focused() {
    let editor = Editor::new();
    assert_eq!(editor.ai_active_buffer_idx(), editor.active_buffer_idx());
}

#[test]
fn ai_active_buffer_idx_uses_target_when_set() {
    let mut editor = Editor::new();
    // Add a second buffer
    editor.buffers.push(Buffer::new());
    editor.ai.target_buffer_idx = Some(1);
    assert_eq!(editor.ai_active_buffer_idx(), 1);
    assert_eq!(editor.active_buffer_idx(), 0); // focused is still 0
}

#[test]
fn ai_cursor_row_defaults_to_focused_window() {
    let mut editor = Editor::new();
    editor.window_mgr.focused_window_mut().cursor_row = 5;
    assert_eq!(editor.ai_cursor_row(), 5);
}

#[test]
fn ai_cursor_row_uses_target_window() {
    let mut editor = Editor::new();
    editor.buffers.push(Buffer::new());
    let area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let new_win_id = editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 1, area)
        .unwrap();
    // Set cursor in the new window
    if let Some(w) = editor
        .window_mgr
        .iter_windows_mut()
        .find(|w| w.id == new_win_id)
    {
        w.cursor_row = 42;
        w.buffer_idx = 1;
    }
    // Focus stays on original window
    let original_id = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id != new_win_id)
        .unwrap()
        .id;
    editor.window_mgr.set_focused(original_id);
    editor.ai.target_window_id = Some(new_win_id);

    assert_eq!(editor.ai_cursor_row(), 42);
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0); // focused is still 0
}

#[test]
fn dispatch_builtin_in_target_restores_focus() {
    let mut editor = editor_with_bulk_text("line one\nline two\nline three");
    editor.buffers.push(Buffer::new());
    let area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let new_win_id = editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 1, area)
        .unwrap();
    let original_id = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id != new_win_id)
        .unwrap()
        .id;
    editor.window_mgr.set_focused(original_id);
    editor.ai.target_window_id = Some(new_win_id);

    // Dispatch move-down in the target window
    editor.dispatch_builtin_in_target("move-down");

    // Focus should be restored to original
    assert_eq!(editor.window_mgr.focused_id(), original_id);
}

#[test]
fn execute_command_respects_ai_target() {
    let mut editor = editor_with_bulk_text("line one\nline two\nline three");
    editor.buffers.push(Buffer::new());
    let area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let new_win_id = editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 0, area)
        .unwrap();
    let original_id = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id != new_win_id)
        .unwrap()
        .id;
    editor.window_mgr.set_focused(original_id);
    editor.ai.target_window_id = Some(new_win_id);

    // Cursor in target window should be at 0 initially
    let target_row_before = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id == new_win_id)
        .unwrap()
        .cursor_row;
    assert_eq!(target_row_before, 0);

    // Dispatch move-down in the target window
    editor.dispatch_builtin_in_target("move-down");

    // Target window cursor should have moved
    let target_row_after = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id == new_win_id)
        .unwrap()
        .cursor_row;
    assert_eq!(target_row_after, 1);

    // Original window cursor should NOT have moved
    assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
}

// --- Async git diff tests ---

#[test]
fn git_diff_async_does_not_block_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_save.rs");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let mut editor = Editor::new();
    editor.buffers[0] = crate::buffer::Buffer::from_file(&path).unwrap();
    editor.buffers[0].insert_char(&mut editor.window_mgr.focused_window_mut().clone(), 'x');
    assert!(editor.buffers[0].modified);
    // save_current_buffer calls request_git_diff (async, non-blocking)
    editor.save_current_buffer();
    // modified must be false immediately after save — before any poll
    assert!(
        !editor.buffers[0].modified,
        "modified should be false immediately after save"
    );
}

#[test]
fn git_diff_stale_buffer_ignored() {
    let mut editor = Editor::new();
    // Create a fake pending git diff with a disconnected channel
    let (_tx, rx) = std::sync::mpsc::channel();
    drop(_tx); // disconnect
    editor.pending_git_diff = Some(crate::editor::PendingGitDiff {
        file_path: std::path::PathBuf::from("/nonexistent/file.rs"),
        receiver: rx,
    });
    // poll should not panic — just drop the stale result
    editor.poll_pending_git_diff();
    assert!(editor.pending_git_diff.is_none());
}

#[test]
fn poll_pending_git_diff_applies_result() {
    let mut editor = Editor::new();
    let path = std::path::PathBuf::from("/test/apply.rs");
    editor.buffers[0].set_file_path(path.clone());

    let (tx, rx) = std::sync::mpsc::channel();
    editor.pending_git_diff = Some(crate::editor::PendingGitDiff {
        file_path: path,
        receiver: rx,
    });

    // Send a mock result
    let mut mock_diff = std::collections::HashMap::new();
    mock_diff.insert(0, crate::render_common::gutter::GitLineStatus::Added);
    mock_diff.insert(5, crate::render_common::gutter::GitLineStatus::Modified);
    tx.send(mock_diff).unwrap();

    editor.poll_pending_git_diff();
    assert!(editor.pending_git_diff.is_none());
    assert_eq!(editor.buffers[0].git_diff_lines.len(), 2);
    assert_eq!(
        editor.buffers[0].git_diff_lines[&0],
        crate::render_common::gutter::GitLineStatus::Added
    );
}

#[test]
fn ai_work_window_reused_across_open_file() {
    let mut editor = Editor::new();
    // Set up a conversation so display_buffer_for_agent splits.
    let conv_idx = editor.ensure_conversation_buffer_idx();
    editor.switch_to_buffer(conv_idx);

    // Open first file — creates a split (work window).
    editor.buffers.push(Buffer::new());
    let idx1 = editor.buffers.len() - 1;
    editor.display_buffer_for_agent(idx1);
    let window_count_after_first = editor.window_mgr.window_count();
    let work_id = editor
        .ai
        .work_window
        .get_valid(&editor.window_mgr)
        .expect("should record work window");

    // Open second file — reuses the work window, no new split.
    editor.buffers.push(Buffer::new());
    let idx2 = editor.buffers.len() - 1;
    editor.display_buffer_for_agent(idx2);
    assert_eq!(
        editor.window_mgr.window_count(),
        window_count_after_first,
        "should not create additional windows"
    );
    assert_eq!(
        editor.ai.work_window.get_valid(&editor.window_mgr),
        Some(work_id)
    );
    let win = editor.window_mgr.window(work_id).unwrap();
    assert_eq!(
        win.buffer_idx, idx2,
        "work window should show the latest buffer"
    );
}

#[test]
fn ai_work_window_cleared_on_stale() {
    let mut editor = Editor::new();
    // Set a fake work window ID that doesn't exist.
    editor.ai.work_window.set(Some(999u32));

    editor.buffers.push(Buffer::new());
    let idx = editor.buffers.len() - 1;
    // Should detect stale reference and fall through to normal logic.
    let ok = editor.display_buffer_for_agent(idx);
    assert!(ok);
    // Stale ID should be cleared.
    assert_ne!(
        editor.ai.work_window.get_valid(&editor.window_mgr),
        Some(999u32)
    );
}

// --- Regression coverage for the reported cascading-splits bug (Part A of
// the DrivenWindow architecture plan): an agent-driven sequence that
// alternates BufferKind must reuse ONE driven window throughout, not
// manufacture a fresh split every time it crosses a kind boundary. Before
// `display_buffer_for_agent` was generalized past Text/Diff, this exact
// sequence (open a file, open a KB node, open a shell, open another file)
// would grow the window count by one on every non-Text/Diff step, because
// `self.ai.work_window` was never consulted for those kinds.

#[test]
fn agent_driven_sequence_reuses_single_window_across_buffer_kinds() {
    let mut editor = Editor::new();
    // Human already has one window open (buffer 0, the default scratch buffer).
    let human_window_count = editor.window_mgr.window_count();
    assert_eq!(human_window_count, 1);

    // 1. Agent "opens a file" — a Text buffer.
    let mut file_buf = Buffer::new();
    file_buf.name = "file1.rs".to_string();
    editor.buffers.push(file_buf);
    let file1_idx = editor.buffers.len() - 1;
    assert!(editor.display_buffer_for_agent(file1_idx));
    // First agent-triggered display creates exactly one driven window.
    assert_eq!(editor.window_mgr.window_count(), human_window_count + 1);
    let driven_window_id = editor
        .ai
        .work_window
        .get_valid(&editor.window_mgr)
        .expect("work window must be recorded after first agent display");

    // 2. Agent "opens a KB node" — a Kb buffer. Must reuse the SAME window,
    // not split again, even though Kb previously routed through an entirely
    // different, kind-based code path than Text/Diff.
    let mut kb_buf = Buffer::new();
    kb_buf.kind = crate::BufferKind::Kb;
    kb_buf.name = "*KB: concept:foo*".to_string();
    editor.buffers.push(kb_buf);
    let kb_idx = editor.buffers.len() - 1;
    assert!(editor.display_buffer_for_agent(kb_idx));
    assert_eq!(
        editor.window_mgr.window_count(),
        human_window_count + 1,
        "displaying a KB node must reuse the driven window, not split"
    );
    assert_eq!(
        editor.ai.work_window.get_valid(&editor.window_mgr),
        Some(driven_window_id),
        "the driven window identity must not change across buffer kinds"
    );
    assert_eq!(
        editor
            .window_mgr
            .window(driven_window_id)
            .unwrap()
            .buffer_idx,
        kb_idx
    );

    // 3. Agent "opens a shell" — a Shell buffer (non-agent terminal).
    let shell_buf = Buffer::new_shell("*Terminal*");
    editor.buffers.push(shell_buf);
    let shell_idx = editor.buffers.len() - 1;
    assert!(editor.display_buffer_for_agent(shell_idx));
    assert_eq!(
        editor.window_mgr.window_count(),
        human_window_count + 1,
        "displaying a shell must reuse the driven window, not split"
    );
    assert_eq!(
        editor.ai.work_window.get_valid(&editor.window_mgr),
        Some(driven_window_id)
    );

    // 4. Agent "opens another file" — back to Text.
    let mut file2_buf = Buffer::new();
    file2_buf.name = "file2.rs".to_string();
    editor.buffers.push(file2_buf);
    let file2_idx = editor.buffers.len() - 1;
    assert!(editor.display_buffer_for_agent(file2_idx));
    assert_eq!(
        editor.window_mgr.window_count(),
        human_window_count + 1,
        "cycling back to Text must still reuse the driven window"
    );
    assert_eq!(
        editor.ai.work_window.get_valid(&editor.window_mgr),
        Some(driven_window_id)
    );
    assert_eq!(
        editor
            .window_mgr
            .window(driven_window_id)
            .unwrap()
            .buffer_idx,
        file2_idx,
        "driven window should show the most recently displayed buffer"
    );
}

#[test]
fn agent_action_never_commandeers_human_kb_window() {
    // A human has deliberately navigated a KB pane open in a second window
    // (e.g. `SPC h k` to read docs). An unrelated agent action (opening a
    // file) must not silently repurpose that window — BufferKind::Kb is
    // part of `is_sidebar()` specifically to prevent this.
    let mut editor = Editor::new();
    editor.last_layout_area = crate::window::Rect {
        x: 0,
        y: 0,
        width: 120,
        height: 40,
    };

    let mut kb_buf = Buffer::new();
    kb_buf.kind = crate::BufferKind::Kb;
    kb_buf.name = "*KB: concept:human-reading*".to_string();
    editor.buffers.push(kb_buf);
    let human_kb_idx = editor.buffers.len() - 1;

    // Human splits and puts the KB buffer in the new window, then returns
    // focus to their original (Text) window — a realistic "reading docs on
    // the side" layout.
    let area = editor.default_area();
    let kb_window_id = editor
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, human_kb_idx, area)
        .expect("split should succeed");
    let original_window_id = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id != kb_window_id)
        .unwrap()
        .id;
    editor.window_mgr.set_focused(original_window_id);

    // Agent opens a file — should NOT land in the human's KB window.
    let mut file_buf = Buffer::new();
    file_buf.name = "file.rs".to_string();
    editor.buffers.push(file_buf);
    let file_idx = editor.buffers.len() - 1;
    assert!(editor.display_buffer_for_agent(file_idx));

    // The human's KB window must still show the KB buffer, untouched.
    let kb_window = editor.window_mgr.window(kb_window_id).unwrap();
    assert_eq!(
        kb_window.buffer_idx, human_kb_idx,
        "agent action must not commandeer a human's KB window"
    );

    // The agent's file must have landed somewhere else (a new split, since
    // the only other window — the original focused one — is where the
    // human's own cursor was, and the KB window is protected).
    assert!(
        editor
            .window_mgr
            .iter_windows()
            .any(|w| w.buffer_idx == file_idx),
        "the agent's file must still be displayed somewhere"
    );
}
