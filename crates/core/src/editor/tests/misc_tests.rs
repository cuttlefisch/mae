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

#[test]
fn active_project_root_falls_back_to_editor_project() {
    let mut ed = Editor::new();
    // No project set anywhere
    assert!(ed.active_project_root().is_none());

    // Set editor-wide project
    ed.project = Some(crate::project::Project::from_root(
        std::path::PathBuf::from("/tmp"),
    ));
    assert_eq!(
        ed.active_project_root().unwrap(),
        std::path::Path::new("/tmp")
    );
}

#[test]
fn active_project_root_prefers_buffer_project() {
    let mut ed = Editor::new();
    ed.project = Some(crate::project::Project::from_root(
        std::path::PathBuf::from("/editor-wide"),
    ));
    ed.buffers[0].project_root = Some(std::path::PathBuf::from("/buffer-specific"));
    assert_eq!(
        ed.active_project_root().unwrap(),
        std::path::Path::new("/buffer-specific")
    );
}

#[test]
fn set_project_root_command() {
    let mut ed = Editor::new();
    // Valid directory
    ed.execute_command("set-project-root /tmp");
    assert_eq!(
        ed.buffers[0].project_root,
        Some(std::path::PathBuf::from("/tmp"))
    );
    assert!(ed.status_msg.contains("Project root set"));

    // Invalid directory
    ed.execute_command("set-project-root /nonexistent_mae_test_xyz");
    assert!(ed.status_msg.contains("Not a directory"));

    // No args
    ed.execute_command("set-project-root");
    assert!(ed.status_msg.contains("Usage"));
}

// ---- switch_to_buffer_non_conversation tests ----

#[test]
fn test_switch_non_conv_normal_window() {
    // When focused window is NOT conversation, it still avoids stealing focus
    // by splitting or using another window.
    let mut ed = Editor::new();
    ed.buffers.push(Buffer::new());
    assert!(!ed.is_conversation_buffer(ed.active_buffer_idx()));
    let ok = ed.switch_to_buffer_non_conversation(1);
    assert!(ok);
    // Focus remains on buffer 0
    assert_eq!(ed.active_buffer_idx(), 0);
    // Buffer 1 is now visible in another window (the split)
    assert!(ed.window_mgr.iter_windows().any(|w| w.buffer_idx == 1));
}

#[test]
fn test_switch_non_conv_routes_to_other_window() {
    // With a split, if conversation is focused, the new buffer goes to the other pane.
    let mut ed = Editor::new();
    // Create a conversation buffer.
    let conv_idx = ed.ensure_conversation_buffer_idx();
    ed.switch_to_buffer(conv_idx);
    // Split vertically so there are two windows.
    let area = ed.default_area();
    let new_id = ed
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 0, area)
        .expect("split should succeed");
    // Focus the conversation window (not the new split).
    // The focused window should still be on conv_idx after split — split
    // doesn't change focus.
    assert_eq!(ed.active_buffer_idx(), conv_idx);
    // Add a third buffer and route it.
    ed.buffers.push(Buffer::new());
    let target_idx = ed.buffers.len() - 1;
    let ok = ed.switch_to_buffer_non_conversation(target_idx);
    assert!(ok);
    // Focused window should STILL show conversation.
    assert_eq!(ed.active_buffer_idx(), conv_idx);
    // The other window should show the target buffer.
    let other_win = ed.window_mgr.window(new_id).expect("split window exists");
    assert_eq!(other_win.buffer_idx, target_idx);
}

#[test]
fn test_switch_non_conv_auto_splits() {
    // Single *AI* window: auto-splits to keep conversation visible.
    let mut ed = Editor::new();
    let conv_idx = ed.ensure_conversation_buffer_idx();
    ed.switch_to_buffer(conv_idx);
    assert_eq!(ed.window_mgr.window_count(), 1);
    // Add a target buffer.
    ed.buffers.push(Buffer::new());
    let target_idx = ed.buffers.len() - 1;
    let ok = ed.switch_to_buffer_non_conversation(target_idx);
    assert!(ok);
    // Should have split into 2 windows.
    assert_eq!(ed.window_mgr.window_count(), 2);
}

#[test]
fn test_open_file_non_conv_preserves_ai() {
    // open_file_non_conversation with *AI* focused keeps conversation visible.
    let mut ed = Editor::new();
    let conv_idx = ed.ensure_conversation_buffer_idx();
    ed.switch_to_buffer(conv_idx);
    // Create a temp file.
    let dir = std::env::temp_dir().join("mae_test_open_non_conv");
    let _ = fs::create_dir_all(&dir);
    let file_path = dir.join("test.txt");
    fs::write(&file_path, "hello").unwrap();
    ed.open_file_non_conversation(file_path.to_str().unwrap());
    // Focused window should still show conversation.
    assert_eq!(ed.active_buffer_idx(), conv_idx);
    // Cleanup.
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn test_focus_hooks_fired() {
    let mut ed = Editor::new();
    // Register dummy functions so fire_hook actually queues something
    ed.hooks.add("focus-out", "dummy-fn");
    ed.hooks.add("focus-in", "dummy-fn");

    // Create a split so we can switch focus
    ed.buffers.push(Buffer::new());
    let area = ed.default_area();
    ed.window_mgr
        .split(crate::window::SplitDirection::Vertical, 1, area)
        .unwrap();

    ed.execute_command("focus-right");
    let hooks: Vec<_> = ed
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
// Shell-insert keymap tests (Part 1: Lisp machine fix)
// ---------------------------------------------------------------------------
