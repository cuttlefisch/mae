//! Editor option and configuration tests.

use super::*;

#[test]
fn self_test_active_flag_defaults_false() {
    let editor = Editor::new();
    assert!(!editor.self_test_active);
}

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
    ed.buffers[0].insert_text_at(0, "visit https://mae.invalid for info");
    // Move cursor to the URL
    let win = ed.window_mgr.focused_window_mut();
    win.cursor_col = 10; // within "https://mae.invalid"
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
    ed.buffers[0].kind = crate::buffer::BufferKind::Kb;
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
    editor.buffers[idx].replace_rope(ropey::Rope::from_str("[Click here](http://mae.invalid)\n"));
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
    assert_eq!(dialog.fields[0].value, "http://mae.invalid");
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
    assert_eq!(
        def.config_key.as_deref(),
        Some("performance.large_file_lines")
    );
}

#[test]
fn mode_report_includes_language() {
    use crate::syntax::Language;
    let mut ed = Editor::new();
    let buf_idx = ed.active_buffer_idx();
    ed.syntax.set_language(buf_idx, Language::Org);
    ed.show_mode_report();

    // The mode report is in the last buffer
    let report_idx = ed.buffers.len() - 1;
    let content = ed.buffers[report_idx].text();
    assert!(
        content.contains("Language:  org"),
        "mode report should include 'Language:  org', got:\n{}",
        content
    );
}
