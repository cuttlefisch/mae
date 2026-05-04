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

#[test]
fn save_and_restore_preserves_conversation_pair() {
    use crate::editor::ConversationPair;

    let mut editor = Editor::new();
    editor.buffers[0].name = "*AI*".into();
    let mut input_buf = Buffer::new();
    input_buf.name = "*ai-input*".into();
    editor.buffers.push(input_buf);

    // Simulate a conversation pair
    editor.conversation_pair = Some(ConversationPair {
        output_buffer_idx: 0,
        input_buffer_idx: 1,
        output_window_id: 100,
        input_window_id: 101,
    });

    editor.save_state();

    // Mutate: clear the pair and add a test buffer
    editor.conversation_pair = None;
    let mut test_buf = Buffer::new();
    test_buf.name = "test.txt".into();
    editor.buffers.push(test_buf);

    editor.restore_state().unwrap();

    // Conversation pair should be restored with correct (possibly remapped) indices
    let pair = editor
        .conversation_pair
        .as_ref()
        .expect("pair should be restored");
    assert_eq!(editor.buffers[pair.output_buffer_idx].name, "*AI*");
    assert_eq!(editor.buffers[pair.input_buffer_idx].name, "*ai-input*");
    assert_eq!(pair.output_window_id, 100);
    assert_eq!(pair.input_window_id, 101);
}

#[test]
fn self_test_active_flag_defaults_false() {
    let editor = Editor::new();
    assert!(!editor.self_test_active);
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
// MarkupFlavor resolution
// ---------------------------------------------------------------------------

#[test]
fn effective_markup_flavor_md_file() {
    use crate::syntax::{Language, MarkupFlavor};
    let mut ed = Editor::new();
    ed.buffers[0].set_file_path(std::path::PathBuf::from("test.md"));
    ed.syntax.set_language(0, Language::Markdown);
    assert_eq!(ed.effective_markup_flavor(0), MarkupFlavor::Markdown);
}

#[test]
fn effective_markup_flavor_render_markup_off() {
    use crate::syntax::{Language, MarkupFlavor};
    let mut ed = Editor::new();
    ed.buffers[0].set_file_path(std::path::PathBuf::from("test.md"));
    ed.syntax.set_language(0, Language::Markdown);
    ed.render_markup = false;
    assert_eq!(ed.effective_markup_flavor(0), MarkupFlavor::None);
}

#[test]
fn effective_markup_flavor_help_buffer() {
    use crate::syntax::MarkupFlavor;
    let mut ed = Editor::new();
    ed.buffers[0].kind = crate::buffer::BufferKind::Help;
    assert_eq!(ed.effective_markup_flavor(0), MarkupFlavor::Markdown);
}

#[test]
fn effective_markup_flavor_plain_text() {
    use crate::syntax::{Language, MarkupFlavor};
    let mut ed = Editor::new();
    ed.buffers[0].set_file_path(std::path::PathBuf::from("test.rs"));
    ed.syntax.set_language(0, Language::Rust);
    assert_eq!(ed.effective_markup_flavor(0), MarkupFlavor::None);
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

// -- Redraw level tests -------------------------------------------------------

#[test]
fn mark_cursor_moved_sets_cursor_only() {
    let mut editor = Editor::new();
    editor.clear_redraw();
    assert_eq!(editor.redraw_level, crate::redraw::RedrawLevel::None);
    editor.mark_cursor_moved();
    assert_eq!(editor.redraw_level, crate::redraw::RedrawLevel::CursorOnly);
}

#[test]
fn mark_lines_dirty_merges_ranges() {
    let mut editor = Editor::new();
    editor.clear_redraw();
    editor.mark_lines_dirty(5, 10);
    assert_eq!(editor.dirty_line_range, Some((5, 10)));
    editor.mark_lines_dirty(2, 7);
    assert_eq!(editor.dirty_line_range, Some((2, 10)));
    assert_eq!(
        editor.redraw_level,
        crate::redraw::RedrawLevel::PartialLines
    );
}

#[test]
fn clear_redraw_resets() {
    let mut editor = Editor::new();
    editor.mark_full_redraw();
    editor.mark_lines_dirty(0, 5);
    editor.clear_redraw();
    assert_eq!(editor.redraw_level, crate::redraw::RedrawLevel::None);
    assert_eq!(editor.dirty_line_range, None);
}

#[test]
fn mark_scrolled_subsumes_cursor_only() {
    let mut editor = Editor::new();
    editor.clear_redraw();
    editor.mark_cursor_moved();
    editor.mark_scrolled();
    assert_eq!(editor.redraw_level, crate::redraw::RedrawLevel::Scroll);
}

// -- Parameterized hook fires test -------------------------------------------

#[test]
fn fire_parameterized_hook() {
    let mut editor = Editor::new();
    editor.hooks.add("buffer-open:rust", "rust-hook-fn");
    editor.fire_hook("buffer-open:rust");
    assert_eq!(editor.pending_hook_evals.len(), 1);
    assert_eq!(editor.pending_hook_evals[0].0, "buffer-open:rust");
    assert_eq!(editor.pending_hook_evals[0].1, "rust-hook-fn");
}

// ---------------------------------------------------------------------------
// Checkbox toggle tests
// ---------------------------------------------------------------------------

#[test]
fn toggle_unchecked_to_checked() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.insert_text_at(0, "- [ ] item one\n- [ ] item two\n");
    editor.toggle_checkbox_at_cursor();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        line.contains("[x]"),
        "expected [x] after toggle, got: {}",
        line
    );
}

#[test]
fn toggle_checked_to_unchecked() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.insert_text_at(0, "- [x] done item\n");
    editor.toggle_checkbox_at_cursor();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        line.contains("[ ]"),
        "expected [ ] after toggle, got: {}",
        line
    );
}

#[test]
fn toggle_on_non_checkbox_noop() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.insert_text_at(0, "Just a regular line\n");
    editor.toggle_checkbox_at_cursor();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert_eq!(line, "Just a regular line\n");
}

#[test]
fn progress_cookie_fraction_updates() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.insert_text_at(0, "* Tasks [0/2]\n- [ ] first\n- [x] second\n");
    // Cursor on line 1 (first checkbox), toggle it
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    editor.toggle_checkbox_at_cursor();
    let heading: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        heading.contains("[2/2]"),
        "expected [2/2] after toggling first item, got: {}",
        heading
    );
}

#[test]
fn progress_cookie_percentage_updates() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.insert_text_at(0, "* Tasks [0%]\n- [ ] first\n- [ ] second\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    editor.toggle_checkbox_at_cursor();
    let heading: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        heading.contains("[50%]"),
        "expected [50%] after toggling one of two, got: {}",
        heading
    );
}

#[test]
fn markdown_checkbox_toggle() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.insert_text_at(0, "- [ ] md item\n");
    editor.toggle_checkbox_at_cursor();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        line.contains("[x]"),
        "markdown checkbox toggle should work, got: {}",
        line
    );
}

#[test]
fn enter_in_org_keymap_maps_to_smart_enter() {
    let ed = Editor::new();
    let keymap = ed.keymaps.get("org").unwrap();
    let result = keymap.lookup(&[crate::keymap::KeyPress::special(crate::keymap::Key::Enter)]);
    assert!(
        matches!(result, crate::LookupResult::Exact("smart-enter")),
        "Enter in org keymap should map to smart-enter, got: {:?}",
        result
    );
}

#[test]
fn enter_in_markdown_keymap_maps_to_smart_enter() {
    let ed = Editor::new();
    let keymap = ed.keymaps.get("markdown").unwrap();
    let result = keymap.lookup(&[crate::keymap::KeyPress::special(crate::keymap::Key::Enter)]);
    assert!(
        matches!(result, crate::LookupResult::Exact("smart-enter")),
        "Enter in markdown keymap should map to smart-enter, got: {:?}",
        result
    );
}

// --- TODO cycling tests ---

#[test]
fn todo_cycle_toggles_todo_done() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "* TODO Buy milk\n");
    editor.org_todo_cycle();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        line.starts_with("* DONE Buy milk"),
        "TODO should become DONE, got: {line}"
    );
    editor.org_todo_cycle();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        line.starts_with("* TODO Buy milk"),
        "DONE should become TODO, got: {line}"
    );
}

#[test]
fn todo_cycle_never_removes_keyword() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "* DONE task\n");
    editor.org_todo_cycle();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        line.contains("TODO") || line.contains("DONE"),
        "keyword should never be removed, got: {line}"
    );
}

#[test]
fn todo_cycle_undo_is_single_step() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "* TODO task\n");
    let original: String = editor.buffers[0].rope().to_string();
    editor.org_todo_cycle();
    // A single undo should restore the original
    editor.dispatch_builtin("undo");
    let after_undo: String = editor.buffers[0].rope().to_string();
    assert_eq!(original, after_undo, "single undo should restore original");
}

#[test]
fn todo_cycle_works_on_markdown_headings() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "## My heading\n");
    editor.org_todo_cycle();
    let line: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(line.starts_with("## TODO My heading"), "got: {line}");
}

#[test]
fn checkbox_toggle_undo_is_single_step() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "- [ ] item\n");
    let original: String = editor.buffers[0].rope().to_string();
    editor.toggle_checkbox_at_cursor();
    let checked: String = editor.buffers[0].rope().to_string();
    assert!(checked.contains("[x]"), "should be checked");
    editor.dispatch_builtin("undo");
    let after_undo: String = editor.buffers[0].rope().to_string();
    assert_eq!(original, after_undo, "single undo should restore original");
}

#[test]
fn set_font_size_updates_default() {
    let mut editor = Editor::new();
    assert_eq!(editor.gui_font_size_default, 14.0);
    editor.set_option("font_size", "18").unwrap();
    assert_eq!(editor.gui_font_size, 18.0);
    assert_eq!(
        editor.gui_font_size_default, 18.0,
        "font_size_default should track set_option"
    );
}

// --- New configurable option tests ---

#[test]
fn set_scroll_speed_clamped() {
    let mut editor = Editor::new();
    editor.set_option("scroll_speed", "0").unwrap();
    assert_eq!(editor.scroll_speed, 1); // clamped to min
    editor.set_option("scroll_speed", "100").unwrap();
    assert_eq!(editor.scroll_speed, 50); // clamped to max
    editor.set_option("scroll_speed", "5").unwrap();
    assert_eq!(editor.scroll_speed, 5);
}

#[test]
fn set_heading_scale_clamped() {
    let mut editor = Editor::new();
    editor.set_option("heading_scale_h1", "0.1").unwrap();
    assert_eq!(editor.heading_scale_h1, 0.5); // clamped
    editor.set_option("heading_scale_h1", "5.0").unwrap();
    assert_eq!(editor.heading_scale_h1, 3.0); // clamped
    editor.set_option("heading_scale_h1", "2.0").unwrap();
    assert_eq!(editor.heading_scale_h1, 2.0);
}

#[test]
fn get_new_options() {
    let editor = Editor::new();
    assert_eq!(editor.get_option("scroll_speed").unwrap().0, "3");
    assert_eq!(editor.get_option("completion_max_items").unwrap().0, "10");
    assert_eq!(
        editor.get_option("window_title").unwrap().0,
        "MAE \u{2014} Modern AI Editor"
    );
    assert_eq!(editor.get_option("heading_scale_h1").unwrap().0, "1.5");
}

// --- Edit-link command ---

#[test]
fn edit_link_opens_mini_dialog() {
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    editor.buffers[idx].replace_rope(ropey::Rope::from_str("[Click here](http://example.com)\n"));
    editor.buffers[idx].set_file_path(std::path::PathBuf::from("test.md"));
    editor.buffers[idx].local_options.link_descriptive = Some(true);
    editor.buffers[idx].recompute_display_regions(true);
    // Cursor at col 0 (on the link region)
    editor.dispatch_builtin("edit-link");
    // Should open a mini-dialog in CommandPalette mode
    assert_eq!(editor.mode, crate::Mode::CommandPalette);
    assert!(editor.mini_dialog.is_some());
    let dialog = editor.mini_dialog.as_ref().unwrap();
    assert_eq!(dialog.fields.len(), 2);
    assert_eq!(dialog.fields[0].label, "URL");
    assert_eq!(dialog.fields[0].value, "http://example.com");
    assert_eq!(dialog.fields[1].label, "Label");
    assert_eq!(dialog.fields[1].value, "Click here");
}

#[test]
fn edit_link_no_link_shows_status() {
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    editor.buffers[idx].replace_rope(ropey::Rope::from_str("plain text\n"));
    editor.dispatch_builtin("edit-link");
    // Should stay in normal mode
    assert_eq!(editor.mode, crate::Mode::Normal);
}

// --- Image-aware line_visual_rows ---

#[test]
fn line_visual_rows_normal_line_unchanged() {
    let editor = Editor::new();
    let rows = editor.line_visual_rows(0, 0);
    assert_eq!(rows, 1);
}

#[test]
fn line_visual_rows_accounts_for_image() {
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    let assets = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("assets");
    if !assets.join("test-image.png").exists() {
        return;
    }
    editor.buffers[idx].replace_rope(ropey::Rope::from_str("![img](test-image.png)\nline 2\n"));
    editor.buffers[idx].local_options.inline_images = Some(true);
    editor.buffers[idx].set_file_path(assets.join("test.md"));
    editor.buffers[idx].recompute_display_regions(true);
    editor.text_area_width = 80;
    let rows = editor.line_visual_rows(0, 0);
    assert!(
        rows > 1,
        "image line should consume multiple visual rows, got {}",
        rows
    );
    // Non-image line should still be 1
    let rows2 = editor.line_visual_rows(0, 1);
    assert_eq!(rows2, 1);
}

// ---- Window Group + Conversation tests ----
// ---------------------------------------------------------------------------

#[test]
fn conversation_creates_group() {
    let mut ed = Editor::new();
    ed.open_conversation_buffer();
    let pair = ed.conversation_pair.as_ref().expect("pair should exist");
    assert!(
        ed.window_mgr.is_in_group(pair.output_window_id),
        "output window should be in a group"
    );
    assert!(
        ed.window_mgr.is_in_group(pair.input_window_id),
        "input window should be in a group"
    );
    assert_eq!(
        ed.window_mgr.group_label(pair.output_window_id),
        Some("conversation")
    );
}

#[test]
fn split_from_conversation_wraps_group() {
    let mut ed = Editor::new();
    ed.open_conversation_buffer();
    let pair = ed.conversation_pair.as_ref().unwrap().clone();
    // Focus the input window and split to open a new buffer.
    ed.window_mgr.set_focused(pair.input_window_id);
    let area = ed.default_area();
    let new_id = ed
        .window_mgr
        .split(crate::window::SplitDirection::Vertical, 0, area)
        .expect("split should succeed");
    // The new window should be outside the conversation group.
    assert!(!ed.window_mgr.is_in_group(new_id));
    // The conversation windows should still be in the group.
    assert!(ed.window_mgr.is_in_group(pair.output_window_id));
    assert!(ed.window_mgr.is_in_group(pair.input_window_id));
}

// Table navigation tests
// ---------------------------------------------------------------------------

#[test]
fn table_next_cell_moves_cursor() {
    let mut ed = ed_with_text("| abc | def |\n| ghi | jkl |\n");
    // Position cursor in first cell (col 2 = inside " abc ")
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 2;

    ed.table_next_cell();

    let win = ed.window_mgr.focused_window();
    // Should be in the second cell of row 0
    assert_eq!(win.cursor_row, 0);
    // cursor_col should be inside second cell (past the pipe + space)
    assert!(
        win.cursor_col > 4,
        "cursor should move to second cell, got col={}",
        win.cursor_col
    );
}

#[test]
fn table_next_cell_wraps_row() {
    let mut ed = ed_with_text("| a | b |\n|---|---|\n| c | d |\n");
    // Position cursor in last cell of first row
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 6; // inside second cell

    ed.table_next_cell();

    let win = ed.window_mgr.focused_window();
    // Should wrap to first cell of next data row (skipping separator at row 1)
    assert_eq!(
        win.cursor_row, 2,
        "should wrap to row 2 (skipping separator)"
    );
}

#[test]
fn table_alignment_idempotent_via_editor() {
    let mut ed = ed_with_text("| short | x |\n| a | longer |\n");
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 2;

    // Align twice via table_next_cell (which aligns internally)
    ed.table_align();
    let text1: String = ed.buffers[0].rope().chars().collect();
    ed.table_align();
    let text2: String = ed.buffers[0].rope().chars().collect();

    assert_eq!(text1, text2, "Double alignment must be idempotent");
}

// ---------------------------------------------------------------------------
// Org table: S-Tab, separator detection, end-of-table insert, alignment
// ---------------------------------------------------------------------------

#[test]
fn blank_row_not_separator() {
    // A row with only spaces and pipes must NOT be classified as a separator.
    use crate::table;
    let rope = ropey::Rope::from_str("|     |     |\n");
    let t = table::table_at_line(&rope, 0).unwrap();
    assert!(
        !t.separators.contains(&0),
        "blank row should not be a separator"
    );
}

#[test]
fn tab_end_of_table_inserts_data_row() {
    // Tab at last cell should insert a blank data row that survives re-parse.
    let mut ed = ed_with_text("| a | b |\n| c | d |\n");
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    win.cursor_col = 8; // last cell of last row

    ed.table_next_cell();

    // Should now have 3 data rows.
    let text: String = ed.buffers[0].rope().chars().collect();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() >= 3, "should have 3+ lines, got: {text}");

    // Re-parse: the new row must be a data row, not a separator.
    let t = crate::table::table_at_line(ed.buffers[0].rope(), 0).unwrap();
    assert!(
        !t.separators.contains(&2),
        "new row must not be classified as separator"
    );
}

#[test]
fn tab_end_of_table_double_tap() {
    // Two Tabs at end: first adds data row, second adds another (no dashes).
    let mut ed = ed_with_text("| a | b |\n");
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 8;

    ed.table_next_cell(); // adds row 1
    ed.table_next_cell(); // should add row 2

    let text: String = ed.buffers[0].rope().chars().collect();
    // No line should contain only dashes (no accidental separator creation).
    for line in text.lines() {
        if line.trim().starts_with('|') {
            let inner = &line.trim()[1..line.trim().len() - 1];
            let has_non_dash_content = inner
                .chars()
                .any(|c| c != '-' && c != '+' && c != '|' && c != ' ' && c != ':');
            let is_all_dashes = !inner.is_empty()
                && inner.contains('-')
                && inner
                    .chars()
                    .all(|c| c == '-' || c == '+' || c == '|' || c == ' ' || c == ':');
            if is_all_dashes && !has_non_dash_content {
                panic!("unexpected separator line created: {line}");
            }
        }
    }
}

#[test]
fn tab_inserts_before_trailing_hline() {
    // If table ends with |---|, new row goes above it.
    let mut ed = ed_with_text("| a | b |\n|---|---|\n");
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 8; // last cell

    ed.table_next_cell();

    let text: String = ed.buffers[0].rope().chars().collect();
    let lines: Vec<&str> = text.lines().collect();
    // The last table line should still be a separator.
    let last_table_line = lines.last().unwrap();
    assert!(
        last_table_line.contains("---"),
        "trailing hline should be preserved at end, got: {text}"
    );
}

#[test]
fn alignment_parsed_from_separator() {
    use crate::table::{self, ColumnAlignment};
    let rope = ropey::Rope::from_str("| L | C | R |\n|:---|:---:|---:|\n| a | b | c |\n");
    let t = table::table_at_line(&rope, 0).unwrap();
    assert_eq!(t.alignments[0], ColumnAlignment::Left);
    assert_eq!(t.alignments[1], ColumnAlignment::Center);
    assert_eq!(t.alignments[2], ColumnAlignment::Right);
}

#[test]
fn format_table_right_aligns() {
    use crate::table;
    let rope =
        ropey::Rope::from_str("| Name | Price |\n|---|---:|\n| Apple | 1 |\n| Banana | 200 |\n");
    let t = table::table_at_line(&rope, 0).unwrap();
    let formatted = table::format_table(&rope, &t);
    // The "Price" column should be right-aligned: "  1" and "200" (right-justified).
    let price_line = &formatted[2]; // "Apple" row
                                    // In a right-aligned cell, content is at the right edge.
    assert!(
        price_line.contains("   1 |") || price_line.contains("  1 |"),
        "expected right-aligned '1', got: {price_line}"
    );
}

#[test]
fn format_table_center_aligns() {
    use crate::table;
    let rope = ropey::Rope::from_str("| X |\n|:---:|\n| ab |\n| abcdef |\n");
    let t = table::table_at_line(&rope, 0).unwrap();
    let formatted = table::format_table(&rope, &t);
    // "ab" should be centered within a 6-char width column: "  ab  "
    let ab_line = &formatted[2];
    // Extract cell content between first pair of pipes
    let inner = &ab_line[1..ab_line.rfind('|').unwrap()];
    let trimmed = inner.trim();
    assert_eq!(trimmed, "ab");
    // Check padding is roughly balanced (allow off-by-one).
    let left_spaces = inner.len() - inner.trim_start().len();
    let right_spaces = inner.len() - inner.trim_end().len();
    assert!(
        (left_spaces as i32 - right_spaces as i32).abs() <= 1,
        "center padding should be balanced: left={left_spaces} right={right_spaces} in '{inner}'"
    );
}

#[test]
fn alignment_markers_preserved_on_format() {
    use crate::table;
    let rope = ropey::Rope::from_str("| L | C | R |\n|:---|:---:|---:|\n| a | b | c |\n");
    let t = table::table_at_line(&rope, 0).unwrap();
    let formatted = table::format_table(&rope, &t);
    let sep_line = &formatted[1]; // separator row
                                  // Should contain alignment markers.
    assert!(
        sep_line.contains(":") && sep_line.contains("-"),
        "separator should preserve alignment markers, got: {sep_line}"
    );
}

#[test]
fn shift_tab_navigates_prev_cell() {
    // S-Tab on a table line should dispatch table_prev_cell, not global fold.
    let mut ed = ed_with_text("| a | b |\n| c | d |\n");
    ed.syntax.set_language(0, crate::syntax::Language::Org);
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 6; // in second cell

    // heading_global_cycle is what S-Tab dispatches.
    ed.heading_global_cycle(crate::syntax::Language::Org);

    let win = ed.window_mgr.focused_window();
    // Should have moved to first cell (col ~2), not folded headings.
    assert_eq!(win.cursor_row, 0, "should stay on row 0");
    assert!(
        win.cursor_col < 5,
        "should be in first cell, got col {}",
        win.cursor_col
    );
}

#[test]
fn cursor_lands_on_content_right_aligned() {
    // Tab into a right-aligned cell should place cursor on content, not padding.
    let mut ed = ed_with_text("| Name | Price |\n|---|---:|\n| Apple | 1 |\n");
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 2; // in Name cell

    ed.table_next_cell(); // move to Price cell

    let win = ed.window_mgr.focused_window();
    // Cursor should be on '1', not on leading padding space.
    let line: String = ed.buffers[0].rope().line(win.cursor_row).chars().collect();
    let ch = line.chars().nth(win.cursor_col).unwrap_or(' ');
    assert_ne!(
        ch,
        ' ',
        "cursor should land on content, not space; col={} line='{}'",
        win.cursor_col,
        line.trim()
    );
}

// ---------------------------------------------------------------------------
// Large document performance (Phase 8 M9)
// ---------------------------------------------------------------------------

#[test]
fn should_degrade_features_small_buffer() {
    let ed = Editor::new();
    assert!(
        !ed.should_degrade_features(0),
        "empty buffer should not degrade"
    );
}

#[test]
fn should_degrade_features_large_buffer() {
    let mut ed = Editor::new();
    // Insert > 500K chars
    let text = "a".repeat(600_000);
    ed.buffers[0].insert_text_at(0, &text);
    assert!(
        ed.should_degrade_features(0),
        "600K char buffer should degrade"
    );
}

#[test]
fn should_degrade_features_long_line() {
    let mut ed = Editor::new();
    // Insert a line > 10K chars (small total chars)
    let text = "x".repeat(15_000);
    ed.buffers[0].insert_text_at(0, &text);
    assert!(
        ed.should_degrade_features(0),
        "15K char line should degrade"
    );
}

#[test]
fn should_degrade_features_normal_file() {
    let mut ed = Editor::new();
    // 1000 lines × 80 chars = 80K chars, max line 80 chars
    let text: String = (0..1000)
        .map(|i| format!("Line {:04}: {}\n", i, "x".repeat(70)))
        .collect();
    ed.buffers[0].insert_text_at(0, &text);
    assert!(
        !ed.should_degrade_features(0),
        "80K normal file should not degrade"
    );
}

#[test]
fn fold_end_at_basic() {
    let mut ed = Editor::new();
    ed.buffers[0].insert_text_at(0, "a\nb\nc\nd\ne\n");
    ed.buffers[0].folded_ranges.push((1, 4));
    assert_eq!(ed.buffers[0].fold_end_at(1), Some(4));
    assert_eq!(ed.buffers[0].fold_end_at(0), None);
    assert_eq!(ed.buffers[0].fold_end_at(2), None);
}

#[test]
fn code_block_cache_populated_after_set() {
    let mut ed = Editor::new();
    ed.buffers[0].insert_text_at(0, "```rust\nfn main() {}\n```\n");
    ed.buffers[0].set_file_path(std::path::PathBuf::from("test.md"));
    ed.syntax.set_language(0, crate::syntax::Language::Markdown);
    let flavor = ed.effective_markup_flavor(0);
    let gen = ed.buffers[0].generation;
    let lines = crate::detect_code_block_lines(&ed.buffers[0], flavor);
    ed.code_block_cache.insert(
        0,
        crate::syntax::ViewportCodeBlockCache {
            generation: gen,
            flavor,
            line_start: 0,
            line_end: ed.buffers[0].line_count(),
            lines: lines.clone(),
        },
    );
    let cached = ed.code_block_cache.get(&0).unwrap();
    assert_eq!(cached.generation, gen);
    assert_eq!(cached.lines, lines);
}

#[test]
fn viewport_local_markup_spans_match_full_buffer() {
    let mut ed = Editor::new();
    let text = "* Heading\n\nSome *bold* text.\n\n#+begin_src rust\nfn main() {}\n#+end_src\n\nMore /italic/ text.\n";
    ed.buffers[0].insert_text_at(0, text);
    let flavor = crate::syntax::MarkupFlavor::Org;
    // Full-buffer spans.
    let source: String = ed.buffers[0].rope().chars().collect();
    let full_spans = crate::compute_markup_spans(&source, flavor);
    // Viewport-local spans covering the same range.
    let rope = ed.buffers[0].rope().clone();
    let line_count = rope.len_lines();
    let (_, local_spans) = crate::compute_markup_spans_for_range(&rope, flavor, 0, line_count);
    assert_eq!(full_spans.len(), local_spans.len());
    for (f, l) in full_spans.iter().zip(local_spans.iter()) {
        assert_eq!(f.byte_start, l.byte_start);
        assert_eq!(f.byte_end, l.byte_end);
        assert_eq!(f.theme_key, l.theme_key);
    }
}

#[test]
fn viewport_local_code_blocks_match_full_buffer() {
    let mut ed = Editor::new();
    let text = "Line 1\n```rust\nfn main() {}\n```\nLine 5\n```\nmore code\n```\nLine 9\n";
    ed.buffers[0].insert_text_at(0, text);
    let flavor = crate::syntax::MarkupFlavor::Markdown;
    let full = crate::detect_code_block_lines(&ed.buffers[0], flavor);
    // Viewport-local for middle range (lines 2..7).
    let local = crate::detect_code_block_lines_for_range(&ed.buffers[0], flavor, 2, 7);
    assert_eq!(local.len(), 5);
    for (rel_idx, &flag) in local.iter().enumerate() {
        assert_eq!(flag, full[2 + rel_idx], "mismatch at line {}", 2 + rel_idx);
    }
}

#[test]
fn viewport_local_code_blocks_backward_scan() {
    let mut ed = Editor::new();
    // Code block starts at line 1, continues through line 3.
    let text = "Line 0\n#+begin_src rust\nfn foo() {}\n#+end_src\nLine 4\n";
    ed.buffers[0].insert_text_at(0, text);
    let flavor = crate::syntax::MarkupFlavor::Org;
    // Request only lines 2..4 — backward scan must detect we're inside a code block.
    let local = crate::detect_code_block_lines_for_range(&ed.buffers[0], flavor, 2, 4);
    assert_eq!(local.len(), 2);
    assert!(local[0], "line 2 should be inside code block");
    assert!(
        local[1],
        "line 3 (#+end_src) should be marked as code block"
    );
}

#[test]
fn markup_cache_covers_method() {
    let cache = crate::syntax::MarkupCache {
        generation: 5,
        flavor: crate::syntax::MarkupFlavor::Org,
        line_start: 100,
        line_end: 400,
        byte_offset: 0,
        spans: vec![],
    };
    assert!(cache.covers(5, crate::syntax::MarkupFlavor::Org, 150, 350));
    assert!(cache.covers(5, crate::syntax::MarkupFlavor::Org, 100, 400));
    assert!(!cache.covers(5, crate::syntax::MarkupFlavor::Org, 50, 200));
    assert!(!cache.covers(5, crate::syntax::MarkupFlavor::Org, 300, 500));
    assert!(!cache.covers(6, crate::syntax::MarkupFlavor::Org, 150, 350));
    assert!(!cache.covers(5, crate::syntax::MarkupFlavor::Markdown, 150, 350));
}

// --- Heading statistics cookie tests ---

#[test]
fn todo_cycle_updates_parent_frac_cookie() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(
        0,
        "* Project [/]\n** TODO Task A\n** TODO Task B\n** TODO Task C\n",
    );
    // Cursor on line 1 (Task A), cycle TODO→DONE
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.org_todo_cycle();
    let parent: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        parent.contains("[1/3]"),
        "Parent cookie should be [1/3], got: {parent}"
    );
}

#[test]
fn todo_cycle_updates_parent_pct_cookie() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "* Project [%]\n** TODO Task A\n** TODO Task B\n");
    // Cycle Task A → DONE
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.org_todo_cycle();
    let parent: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        parent.contains("[50%]"),
        "Parent cookie should be [50%], got: {parent}"
    );
}

#[test]
fn todo_cycle_all_done_updates_cookie() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "* Project [/]\n** TODO Task A\n** TODO Task B\n");
    // Cycle both to DONE
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.org_todo_cycle();
    editor.window_mgr.focused_window_mut().cursor_row = 2;
    editor.org_todo_cycle();
    let parent: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        parent.contains("[2/2]"),
        "Parent cookie should be [2/2], got: {parent}"
    );
}

#[test]
fn todo_cycle_done_back_to_todo_decrements_cookie() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "* Project [/]\n** DONE Task A\n** TODO Task B\n");
    // Cycle Task A DONE→TODO
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.org_todo_cycle();
    let parent: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        parent.contains("[0/2]"),
        "Parent cookie should be [0/2], got: {parent}"
    );
}

#[test]
fn heading_cookie_ignores_sibling_headings() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(
        0,
        "* Project [/]\n** TODO Task A\n* Other heading\n** TODO Task B\n",
    );
    // Cycle Task A → DONE (under Project)
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.org_todo_cycle();
    let parent: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        parent.contains("[1/1]"),
        "Should only count children under Project, got: {parent}"
    );
}

// ---------------------------------------------------------------------------
// Configurable performance thresholds
// ---------------------------------------------------------------------------

#[test]
fn configurable_degrade_threshold() {
    let mut ed = Editor::new();
    // Insert 200K chars as 2500 lines of 80 chars — below default 500K threshold
    let text: String = (0..2500).map(|_| "a".repeat(79) + "\n").collect();
    ed.buffers[0].insert_text_at(0, &text);
    assert!(!ed.should_degrade_features(0), "200K < 500K default");

    // Lower the threshold
    ed.degrade_threshold_chars = 100_000;
    ed.buffers[0].degraded = None; // clear cache
    assert!(
        ed.should_degrade_features(0),
        "200K > 100K custom threshold"
    );
}

#[test]
fn configurable_large_file_lines() {
    let mut ed = Editor::new();
    assert_eq!(ed.large_file_lines, 5_000);
    ed.large_file_lines = 100;
    assert_eq!(ed.large_file_lines, 100);
}

#[test]
fn set_option_performance_thresholds() {
    let mut ed = Editor::new();
    ed.set_option("large_file_lines", "8000").unwrap();
    assert_eq!(ed.large_file_lines, 8000);

    ed.set_option("degrade_threshold_chars", "1000000").unwrap();
    assert_eq!(ed.degrade_threshold_chars, 1_000_000);

    ed.set_option("syntax_reparse_debounce_ms", "100").unwrap();
    assert_eq!(ed.syntax_reparse_debounce_ms, 100);

    ed.set_option("display_region_debounce_ms", "200").unwrap();
    assert_eq!(ed.display_region_debounce_ms, 200);

    ed.set_option("degrade_threshold_line_length", "20000")
        .unwrap();
    assert_eq!(ed.degrade_threshold_line_length, 20_000);
}

#[test]
fn set_option_performance_aliases() {
    let mut ed = Editor::new();
    ed.set_option("large-file-lines", "3000").unwrap();
    assert_eq!(ed.large_file_lines, 3000);

    ed.set_option("syntax-reparse-debounce-ms", "75").unwrap();
    assert_eq!(ed.syntax_reparse_debounce_ms, 75);
}

#[test]
fn get_option_performance() {
    let ed = Editor::new();
    let (val, def) = ed.get_option("large_file_lines").unwrap();
    assert_eq!(val, "5000");
    assert_eq!(def.config_key, Some("performance.large_file_lines"));
}

#[test]
fn viewport_local_syntax_spans() {
    use crate::syntax::SyntaxMap;
    let mut sm = SyntaxMap::new();
    sm.set_language(0, crate::syntax::Language::Rust);

    let source = "fn main() {\n    let x = 1;\n    let y = 2;\n}\nfn foo() {}\n";
    let rope = ropey::Rope::from_str(source);
    let gen = 1;

    // Full-buffer parse
    let spans_full = sm.spans_for(0, source, gen).map(|s| s.to_vec());
    assert!(spans_full.is_some());

    // Reset and do viewport-local parse for lines 0..3
    sm.set_language(0, crate::syntax::Language::Rust);
    let spans_vp = sm
        .spans_for_viewport(0, &rope, gen, 0, 3)
        .map(|s| s.to_vec());
    assert!(spans_vp.is_some());

    // Viewport spans should cover byte range of lines 0..3 only
    let byte_end_line3 = rope.line_to_byte(3);
    let vp_spans = spans_vp.unwrap();
    assert!(
        vp_spans.iter().all(|s| s.byte_end <= byte_end_line3),
        "viewport spans should be within lines 0..3"
    );
}

#[test]
fn viewport_covers_tracks_range() {
    use crate::syntax::SyntaxMap;
    let mut sm = SyntaxMap::new();
    sm.set_language(0, crate::syntax::Language::Rust);

    let source = "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\nfn e() {}\n";
    let rope = ropey::Rope::from_str(source);

    sm.spans_for_viewport(0, &rope, 1, 1, 3);
    assert!(sm.viewport_covers(0, 1, 3));
    assert!(!sm.viewport_covers(0, 0, 3)); // 0 < viewport_line_start=1
    assert!(!sm.viewport_covers(0, 1, 5)); // 5 > viewport_line_end=3
}

// Shell-insert keymap tests (Part 1: Lisp machine fix)
// ---------------------------------------------------------------------------
