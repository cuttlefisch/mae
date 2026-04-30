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

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Buffer-local options (per-buffer word_wrap, line_numbers, etc.)
// ---------------------------------------------------------------------------

#[test]
fn effective_word_wrap_uses_buffer_local() {
    let mut ed = Editor::new();
    // Global default: off
    assert!(!ed.word_wrap);
    assert!(!ed.effective_word_wrap());

    // Create conversation buffer — has word_wrap=true locally
    let conv_idx = ed.ensure_conversation_buffer_idx();
    ed.switch_to_buffer(conv_idx);
    assert!(ed.effective_word_wrap());

    // Switch back to text buffer — no local override, uses global
    ed.switch_to_buffer(0);
    assert!(!ed.effective_word_wrap());

    // Set global to true
    ed.word_wrap = true;
    assert!(ed.effective_word_wrap());
}

#[test]
fn setlocal_word_wrap_command() {
    let mut ed = Editor::new();
    assert!(!ed.word_wrap);
    assert!(!ed.effective_word_wrap());

    // :setlocal word_wrap true
    let result = ed.set_local_option("word_wrap", "true");
    assert!(result.is_ok());
    assert!(ed.effective_word_wrap());

    // Global is still false
    assert!(!ed.word_wrap);

    // Buffer-local is set
    assert_eq!(ed.buffers[0].local_options.word_wrap, Some(true));
}

#[test]
fn word_wrap_for_specific_buffer() {
    let mut ed = Editor::new();
    ed.word_wrap = false;

    // Buffer 0 (text) has no override
    assert!(!ed.word_wrap_for(0));

    // Create conversation buffer with local override
    let conv_idx = ed.ensure_conversation_buffer_idx();
    assert!(ed.word_wrap_for(conv_idx));
}

// ---------------------------------------------------------------------------
// Buffer-local options: break_indent, show_break, heading_scale
// ---------------------------------------------------------------------------

#[test]
fn setlocal_break_indent() {
    let mut ed = Editor::new();
    assert!(ed.break_indent); // global default true
    let result = ed.set_local_option("break_indent", "false");
    assert!(result.is_ok());
    assert!(!ed.break_indent_for(0));
    assert!(ed.break_indent); // global unchanged
}

#[test]
fn setlocal_heading_scale() {
    let mut ed = Editor::new();
    assert!(ed.heading_scale); // global default true
    let result = ed.set_local_option("heading_scale", "false");
    assert!(result.is_ok());
    assert!(!ed.heading_scale_for(0));
}

#[test]
fn setlocal_show_break() {
    let mut ed = Editor::new();
    let result = ed.set_local_option("show_break", ">>> ");
    assert!(result.is_ok());
    assert_eq!(ed.show_break_for(0), ">>> ");
    assert_eq!(ed.show_break, "↪ "); // global unchanged
}

// ---------------------------------------------------------------------------
// open-link-at-cursor: URL and file path detection under cursor
// ---------------------------------------------------------------------------

#[test]
fn open_link_at_cursor_no_link() {
    let mut ed = Editor::new();
    ed.buffers[0].insert_text_at(0, "just plain text here");
    ed.dispatch_builtin("open-link-at-cursor");
    assert!(ed.status_msg.contains("No link"));
}

#[test]
fn open_link_at_cursor_detects_url() {
    let mut ed = Editor::new();
    ed.buffers[0].insert_text_at(0, "visit https://example.com for info");
    // Move cursor to the URL
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_col = 10; // within "https://example.com"
    ed.dispatch_builtin("open-link-at-cursor");
    // URL opens externally, status shows "Opening ..."
    assert!(ed.status_msg.contains("Opening"));
}

#[test]
fn handle_link_click_navigates_to_line() {
    let mut ed = Editor::new();
    // Create a temp file
    let dir = std::env::temp_dir().join("mae_test_link_click");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("test.txt");
    std::fs::write(&file, "line1\nline2\nline3\nline4\nline5\n").unwrap();

    // Simulate clicking a file:line link
    let target = format!("{}:3:1", file.display());
    ed.handle_link_click(&target);

    // Should have opened the file and navigated to line 3 (row 2, 0-indexed)
    let win = ed.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gx_keybinding_exists() {
    let ed = Editor::new();
    let keymap = ed.keymaps.get("normal").unwrap();
    let result = keymap.lookup(&crate::keymap::parse_key_seq("gx"));
    assert!(matches!(
        result,
        crate::LookupResult::Exact("open-link-at-cursor")
    ));
}

// ---------------------------------------------------------------------------
// link_descriptive / render_markup options
// ---------------------------------------------------------------------------

#[test]
fn link_descriptive_default_true() {
    let ed = Editor::new();
    let (val, def) = ed.get_option("link_descriptive").unwrap();
    assert_eq!(val, "true");
    assert_eq!(def.name, "link_descriptive");
}

#[test]
fn render_markup_default_true() {
    let ed = Editor::new();
    let (val, def) = ed.get_option("render_markup").unwrap();
    assert_eq!(val, "true");
    assert_eq!(def.name, "render_markup");
}

#[test]
fn setlocal_link_descriptive() {
    let mut ed = Editor::new();
    assert!(ed.link_descriptive); // global default
    let result = ed.set_local_option("link_descriptive", "false");
    assert!(result.is_ok());
    assert!(!ed.link_descriptive_for(0));
    assert!(ed.link_descriptive); // global unchanged
}

#[test]
fn setlocal_render_markup() {
    let mut ed = Editor::new();
    assert!(ed.render_markup);
    let result = ed.set_local_option("render_markup", "false");
    assert!(result.is_ok());
    assert!(!ed.render_markup_for(0));
    assert!(ed.render_markup); // global unchanged
}

// ---------------------------------------------------------------------------
// Display regions
// ---------------------------------------------------------------------------

#[test]
fn display_regions_recomputed_on_edit() {
    let mut ed = Editor::new();
    let idx = ed.active_buffer_idx();
    // Set a file path so it picks an extension
    ed.buffers[idx].set_file_path(std::path::PathBuf::from("/tmp/test.md"));
    ed.buffers[idx].insert_text_at(0, "See [docs](https://docs.rs) here\n");
    ed.buffers[idx].recompute_display_regions(true);
    assert_eq!(ed.buffers[idx].display_regions.len(), 1);
    assert_eq!(
        ed.buffers[idx].display_regions[0].replacement.as_deref(),
        Some("docs")
    );

    // Edit the buffer — regions should be stale
    let gen_before = ed.buffers[idx].display_regions_gen;
    ed.buffers[idx].insert_text_at(0, "x");
    assert_ne!(ed.buffers[idx].generation, gen_before);

    // Recompute
    ed.buffers[idx].recompute_display_regions(true);
    assert_eq!(ed.buffers[idx].display_regions.len(), 1);
    // The region byte offsets should have shifted by 1
    assert_eq!(ed.buffers[idx].display_regions[0].byte_start, 5);
}

#[test]
fn cursor_moves_through_revealed_link_region() {
    // With org-appear, cursor moves through raw chars in a revealed region
    // (no snapping). The display_reveal_cursor suppresses concealment.
    let mut ed = Editor::new();
    let idx = ed.active_buffer_idx();
    ed.buffers[idx].set_file_path(std::path::PathBuf::from("/tmp/test.md"));
    ed.buffers[idx].insert_text_at(0, "See [docs](https://docs.rs) here\n");
    ed.buffers[idx].recompute_display_regions(true);
    assert!(!ed.buffers[idx].display_regions.is_empty());

    // Place cursor at col 5 (inside the link region [docs](url))
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 5;

    // Move right should advance by 1 char (no snapping with org-appear)
    ed.dispatch_builtin("move-right");
    let col = ed.window_mgr.focused_window().cursor_col;
    assert_eq!(
        col, 6,
        "cursor should move normally through revealed region"
    );
}

// Shell-insert keymap tests (Part 1: Lisp machine fix)
// ---------------------------------------------------------------------------
