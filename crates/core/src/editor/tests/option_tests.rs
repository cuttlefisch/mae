//! Editor option and configuration tests.

use super::*;

#[test]
fn self_test_active_flag_defaults_false() {
    let editor = Editor::new();
    assert!(!editor.self_test_active);
}

// --- #305: set_status dedups consecutive-identical entries ---

fn status_entries_matching(editor: &Editor, msg: &str) -> usize {
    editor
        .message_log
        .entries()
        .iter()
        .filter(|e| e.target == "status" && e.message == msg)
        .count()
}

#[test]
fn set_status_repeated_identical_value_logs_once() {
    let mut editor = Editor::new();
    let before = status_entries_matching(&editor, "X");
    editor.set_status("X");
    editor.set_status("X");
    editor.set_status("X");
    assert_eq!(
        status_entries_matching(&editor, "X") - before,
        1,
        "three consecutive identical calls must log exactly one entry"
    );
}

#[test]
fn set_status_non_consecutive_repeat_logs_both() {
    // Adversarial: only CONSECUTIVE repeats are deduped. Re-raising an
    // earlier value after something else was shown in between must still
    // log — guards against an over-eager global (not consecutive-only) dedup.
    let mut editor = Editor::new();
    let before = status_entries_matching(&editor, "X");
    editor.set_status("X");
    editor.set_status("Y");
    editor.set_status("X");
    assert_eq!(
        status_entries_matching(&editor, "X") - before,
        2,
        "non-consecutive repeats of the same value must both log"
    );
}

#[test]
fn set_status_empty_string_still_does_not_log() {
    let mut editor = Editor::new();
    let count_before = editor.message_log.entries().len();
    editor.set_status("");
    assert_eq!(editor.message_log.entries().len(), count_before);
}

#[test]
fn effective_word_wrap_uses_buffer_local() {
    let mut editor = Editor::new();
    // Global default: off
    assert!(!editor.word_wrap);
    assert!(!editor.effective_word_wrap());

    // Create conversation buffer — has word_wrap=true locally
    let conv_idx = editor.ensure_conversation_buffer_idx();
    editor.switch_to_buffer(conv_idx);
    assert!(editor.effective_word_wrap());

    // Switch back to text buffer — no local override, uses global
    editor.switch_to_buffer(0);
    assert!(!editor.effective_word_wrap());

    // Set global to true
    editor.word_wrap = true;
    assert!(editor.effective_word_wrap());
}

#[test]
fn setlocal_word_wrap_command() {
    let mut editor = Editor::new();
    assert!(!editor.word_wrap);
    assert!(!editor.effective_word_wrap());

    // :setlocal word_wrap true
    let result = editor.set_local_option("word_wrap", "true");
    assert!(result.is_ok());
    assert!(editor.effective_word_wrap());

    // Global is still false
    assert!(!editor.word_wrap);

    // Buffer-local is set
    assert_eq!(editor.buffers[0].local_options.word_wrap, Some(true));
}

#[test]
fn word_wrap_for_specific_buffer() {
    let mut editor = Editor::new();
    editor.word_wrap = false;

    // Buffer 0 (text) has no override
    assert!(!editor.word_wrap_for(0));

    // Create conversation buffer with local override
    let conv_idx = editor.ensure_conversation_buffer_idx();
    assert!(editor.word_wrap_for(conv_idx));
}

// ---------------------------------------------------------------------------
// Buffer-local options: break_indent, show_break, heading_scale
// ---------------------------------------------------------------------------

#[test]
fn setlocal_break_indent() {
    let mut editor = Editor::new();
    assert!(editor.break_indent); // global default true
    let result = editor.set_local_option("break_indent", "false");
    assert!(result.is_ok());
    assert!(!editor.break_indent_for(0));
    assert!(editor.break_indent); // global unchanged
}

#[test]
fn setlocal_heading_scale() {
    let mut editor = Editor::new();
    assert!(editor.heading_scale); // global default true
    let result = editor.set_local_option("heading_scale", "false");
    assert!(result.is_ok());
    assert!(!editor.heading_scale_for(0));
}

#[test]
fn setlocal_show_break() {
    let mut editor = Editor::new();
    let result = editor.set_local_option("show_break", ">>> ");
    assert!(result.is_ok());
    assert_eq!(editor.show_break_for(0), ">>> ");
    assert_eq!(editor.show_break, "↪ "); // global unchanged
}

// ---------------------------------------------------------------------------
// open-link-at-cursor: URL and file path detection under cursor
// ---------------------------------------------------------------------------

#[test]
fn open_link_at_cursor_no_link() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "just plain text here");
    editor.dispatch_builtin("open-link-at-cursor");
    assert!(editor.status_msg.contains("No link"));
}

#[test]
fn open_link_at_cursor_detects_url() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(0, "visit https://mae.invalid for info");
    // Move cursor to the URL
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_col = 10; // within "https://mae.invalid"
    editor.dispatch_builtin("open-link-at-cursor");
    // URL opens externally, status shows "Opening ..."
    assert!(editor.status_msg.contains("Opening"));
}

#[test]
fn handle_link_click_navigates_to_line() {
    let mut editor = Editor::new();
    // Create a temp file
    let dir = std::env::temp_dir().join("mae_test_link_click");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("test.txt");
    std::fs::write(&file, "line1\nline2\nline3\nline4\nline5\n").unwrap();

    // Simulate clicking a file:line link
    let target = format!("{}:3:1", file.display());
    editor.handle_link_click(&target);

    // Should have opened the file and navigated to line 3 (row 2, 0-indexed)
    let win = editor.window_mgr.focused_window();
    assert_eq!(win.cursor_row, 2);

    let _ = std::fs::remove_dir_all(&dir);
}

// --- #293: KB-aware link-click resolution ---

#[test]
fn handle_link_click_resolves_kb_node_to_kb_view() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "user:link-target",
        "Link Target",
        mae_kb::NodeKind::Note,
        "body",
    ));
    editor.handle_link_click("user:link-target");
    assert_eq!(
        editor.buffers[editor.active_buffer_idx()].kind,
        crate::BufferKind::Kb,
        "a KB-shaped target must open the *KB* view, not attempt a file open"
    );
    assert_eq!(editor.kb_view().unwrap().current, "user:link-target");
    assert!(!editor.status_msg.to_lowercase().contains("error"));
}

#[test]
fn handle_link_click_strips_id_prefix() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "user:link-target",
        "Link Target",
        mae_kb::NodeKind::Note,
        "body",
    ));
    // org-roam-style `id:` prefix, as chain-fill/daily-note links use.
    editor.handle_link_click("id:user:link-target");
    assert_eq!(
        editor.buffers[editor.active_buffer_idx()].kind,
        crate::BufferKind::Kb
    );
    assert_eq!(editor.kb_view().unwrap().current, "user:link-target");
}

#[test]
fn handle_link_click_source_file_mode_opens_raw_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("note1.org"),
        ":PROPERTIES:\n:ID: click-src-test\n:END:\n#+title: Click Src Test\n\nBody.\n",
    )
    .unwrap();

    let mut editor = Editor::new();
    editor.config_dir_override = Some(dir.path().join("cfgdir"));
    editor.data_dir_override = Some(dir.path().join("datadir"));
    editor.kb_register("ClickSrcTest", dir.path()).unwrap();
    editor.kb_link_follow_mode = "source-file".to_string();

    editor.handle_link_click("click-src-test");

    assert_ne!(
        editor.buffers[editor.active_buffer_idx()].kind,
        crate::BufferKind::Kb,
        "source-file mode must open the raw file, not switch into the *KB* view"
    );
    let opened = editor.buffers.iter().any(|b| {
        b.file_path()
            .map(|p| p.ends_with("note1.org"))
            .unwrap_or(false)
    });
    assert!(opened, "should have opened the node's raw source file");
}

#[test]
fn handle_link_click_relative_file_link_unaffected_by_kb_routing() {
    // Regression guard: a genuine (non-KB) relative file link must still go
    // through the ordinary file-open path, not get swallowed by the new
    // KB-aware branch.
    let dir = std::env::temp_dir().join("mae_test_non_kb_link");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join("plan.md");
    std::fs::write(&file, "# Plan\n").unwrap();

    let mut editor = Editor::new();
    editor.handle_link_click(&file.display().to_string());
    let opened = editor.buffers.iter().any(|b| {
        b.file_path()
            .map(|p| p.ends_with("plan.md"))
            .unwrap_or(false)
    });
    assert!(opened, "a real file link must still open as a file");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn kb_link_follow_mode_option_round_trip() {
    let mut editor = Editor::new();
    assert_eq!(editor.kb_link_follow_mode, "kb-view");
    editor
        .set_option("kb_link_follow_mode", "source-file")
        .unwrap();
    assert_eq!(editor.kb_link_follow_mode, "source-file");
    assert_eq!(
        editor.get_option("kb_link_follow_mode").unwrap().0,
        "source-file"
    );
    let err = editor.set_option("kb_link_follow_mode", "bogus");
    assert!(err.is_err(), "an unrecognized mode must be rejected");
}

#[test]
fn open_link_at_cursor_end_to_end_resolves_kb_link_in_plain_org_buffer() {
    // #293 end-to-end: a real org buffer (not the *KB* view) with
    // `link_descriptive` on, containing a daily-note-style `[[id][display]]`
    // link under the cursor. This is the actual dispatch chain both `gx`
    // and `smart_enter`'s Enter-to-follow-link fallback exercise via
    // `open-link-at-cursor`.
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "daily:2026-07-06",
        "2026-07-06",
        mae_kb::NodeKind::Note,
        "",
    ));
    editor.buffers[0].insert_text_at(0, "Previous: [[id:daily:2026-07-06][2026-07-06]]\n");
    editor.buffers[0].recompute_display_regions(true);

    // Cursor on the link's visible display text ("2026-07-06").
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 12;

    editor.dispatch_builtin("open-link-at-cursor");

    assert_eq!(
        editor.buffers[editor.active_buffer_idx()].kind,
        crate::BufferKind::Kb,
        "following the link must land in the *KB* view, not attempt a raw file open"
    );
    assert_eq!(editor.kb_view().unwrap().current, "daily:2026-07-06");
    assert!(!editor.status_msg.to_lowercase().contains("error"));
}

#[test]
fn gx_keybinding_exists() {
    let editor = Editor::new();
    let keymap = editor.keymaps.get("normal").unwrap();
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
    let editor = Editor::new();
    let (val, def) = editor.get_option("link_descriptive").unwrap();
    assert_eq!(val, "true");
    assert_eq!(def.name, "link_descriptive");
}

#[test]
fn render_markup_default_true() {
    let editor = Editor::new();
    let (val, def) = editor.get_option("render_markup").unwrap();
    assert_eq!(val, "true");
    assert_eq!(def.name, "render_markup");
}

#[test]
fn setlocal_link_descriptive() {
    let mut editor = Editor::new();
    assert!(editor.link_descriptive); // global default
    let result = editor.set_local_option("link_descriptive", "false");
    assert!(result.is_ok());
    assert!(!editor.link_descriptive_for(0));
    assert!(editor.link_descriptive); // global unchanged
}

#[test]
fn setlocal_render_markup() {
    let mut editor = Editor::new();
    assert!(editor.render_markup);
    let result = editor.set_local_option("render_markup", "false");
    assert!(result.is_ok());
    assert!(!editor.render_markup_for(0));
    assert!(editor.render_markup); // global unchanged
}

// ---------------------------------------------------------------------------
// MarkupFlavor resolution
// ---------------------------------------------------------------------------

#[test]
fn effective_markup_flavor_md_file() {
    use crate::syntax::{Language, MarkupFlavor};
    let mut editor = Editor::new();
    editor.buffers[0].set_file_path(std::path::PathBuf::from("test.md"));
    editor.syntax.set_language(0, Language::Markdown);
    assert_eq!(editor.effective_markup_flavor(0), MarkupFlavor::Markdown);
}

#[test]
fn effective_markup_flavor_render_markup_off() {
    use crate::syntax::{Language, MarkupFlavor};
    let mut editor = Editor::new();
    editor.buffers[0].set_file_path(std::path::PathBuf::from("test.md"));
    editor.syntax.set_language(0, Language::Markdown);
    editor.render_markup = false;
    assert_eq!(editor.effective_markup_flavor(0), MarkupFlavor::None);
}

#[test]
fn effective_markup_flavor_help_buffer() {
    use crate::syntax::MarkupFlavor;
    let mut editor = Editor::new();
    editor.buffers[0].kind = crate::buffer::BufferKind::Kb;
    assert_eq!(editor.effective_markup_flavor(0), MarkupFlavor::Markdown);
}

#[test]
fn effective_markup_flavor_plain_text() {
    use crate::syntax::{Language, MarkupFlavor};
    let mut editor = Editor::new();
    editor.buffers[0].set_file_path(std::path::PathBuf::from("test.rs"));
    editor.syntax.set_language(0, Language::Rust);
    assert_eq!(editor.effective_markup_flavor(0), MarkupFlavor::None);
}

// ---------------------------------------------------------------------------
// Display regions
// ---------------------------------------------------------------------------

#[test]
fn display_regions_recomputed_on_edit() {
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    // Set a file path so it picks an extension
    editor.buffers[idx].set_file_path(std::path::PathBuf::from("/tmp/test.md"));
    editor.buffers[idx].insert_text_at(0, "See [docs](https://docs.rs) here\n");
    editor.buffers[idx].recompute_display_regions(true);
    assert_eq!(editor.buffers[idx].display_regions.len(), 1);
    assert_eq!(
        editor.buffers[idx].display_regions[0]
            .replacement
            .as_deref(),
        Some("docs")
    );

    // Edit the buffer — regions should be stale
    let gen_before = editor.buffers[idx].display_regions_gen;
    editor.buffers[idx].insert_text_at(0, "x");
    assert_ne!(editor.buffers[idx].generation, gen_before);

    // Recompute
    editor.buffers[idx].recompute_display_regions(true);
    assert_eq!(editor.buffers[idx].display_regions.len(), 1);
    // The region byte offsets should have shifted by 1
    assert_eq!(editor.buffers[idx].display_regions[0].byte_start, 5);
}

#[test]
fn cursor_moves_through_revealed_link_region() {
    // With org-appear, cursor moves through raw chars in a revealed region
    // (no snapping). The display_reveal_cursor suppresses concealment.
    let mut editor = Editor::new();
    let idx = editor.active_buffer_idx();
    editor.buffers[idx].set_file_path(std::path::PathBuf::from("/tmp/test.md"));
    editor.buffers[idx].insert_text_at(0, "See [docs](https://docs.rs) here\n");
    editor.buffers[idx].recompute_display_regions(true);
    assert!(!editor.buffers[idx].display_regions.is_empty());

    // Place cursor at col 5 (inside the link region [docs](url))
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 5;

    // Move right should advance by 1 char (no snapping with org-appear)
    editor.dispatch_builtin("move-right");
    let col = editor.window_mgr.focused_window().cursor_col;
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
    let mut editor = Editor::new();
    // Insert 200K chars as 2500 lines of 80 chars — below default 500K threshold
    let text: String = (0..2500).map(|_| "a".repeat(79) + "\n").collect();
    editor.buffers[0].insert_text_at(0, &text);
    assert!(!editor.should_degrade_features(0), "200K < 500K default");

    // Lower the threshold
    editor.degrade_threshold_chars = 100_000;
    editor.buffers[0].degraded = None; // clear cache
    assert!(
        editor.should_degrade_features(0),
        "200K > 100K custom threshold"
    );
}

#[test]
fn configurable_large_file_lines() {
    let mut editor = Editor::new();
    assert_eq!(editor.large_file_lines, 5_000);
    editor.large_file_lines = 100;
    assert_eq!(editor.large_file_lines, 100);
}

#[test]
fn set_option_performance_thresholds() {
    let mut editor = Editor::new();
    editor.set_option("large_file_lines", "8000").unwrap();
    assert_eq!(editor.large_file_lines, 8000);

    editor
        .set_option("degrade_threshold_chars", "1000000")
        .unwrap();
    assert_eq!(editor.degrade_threshold_chars, 1_000_000);

    editor
        .set_option("syntax_reparse_debounce_ms", "100")
        .unwrap();
    assert_eq!(editor.syntax_reparse_debounce_ms, 100);

    editor
        .set_option("display_region_debounce_ms", "200")
        .unwrap();
    assert_eq!(editor.display_region_debounce_ms, 200);

    editor
        .set_option("degrade_threshold_line_length", "20000")
        .unwrap();
    assert_eq!(editor.degrade_threshold_line_length, 20_000);
}

#[test]
fn set_option_performance_aliases() {
    let mut editor = Editor::new();
    editor.set_option("large-file-lines", "3000").unwrap();
    assert_eq!(editor.large_file_lines, 3000);

    editor
        .set_option("syntax-reparse-debounce-ms", "75")
        .unwrap();
    assert_eq!(editor.syntax_reparse_debounce_ms, 75);
}

#[test]
fn set_option_babel_compilers_apply_to_fields() {
    // Regression: babel options were registered + persisted but never applied
    // to the editor fields. `:set` (and its aliases) must now take effect.
    let mut editor = Editor::new();
    assert_eq!(editor.babel_cxx_compiler, "c++");

    editor.set_option("babel_cxx_compiler", "g++").unwrap();
    assert_eq!(editor.babel_cxx_compiler, "g++");

    editor.set_option("babel-c-compiler", "clang").unwrap();
    assert_eq!(editor.babel_c_compiler, "clang");

    editor.set_option("babel-cxx-std", "c++20").unwrap();
    assert_eq!(editor.babel_cxx_std, "c++20");

    editor.set_option("babel-timeout", "90").unwrap();
    assert_eq!(editor.babel_timeout, 90);

    editor.set_option("babel-confirm", "false").unwrap();
    assert!(!editor.babel_confirm);
}

#[test]
fn get_option_performance() {
    let editor = Editor::new();
    let (val, def) = editor.get_option("large_file_lines").unwrap();
    assert_eq!(val, "5000");
    assert_eq!(
        def.config_key.as_deref(),
        Some("performance.large_file_lines")
    );
}

#[test]
fn mode_report_includes_language() {
    use crate::syntax::Language;
    let mut editor = Editor::new();
    let buf_idx = editor.active_buffer_idx();
    editor.syntax.set_language(buf_idx, Language::Org);
    editor.show_mode_report();

    // The mode report is in the last buffer
    let report_idx = editor.buffers.len() - 1;
    let content = editor.buffers[report_idx].text();
    assert!(
        content.contains("Language:  org"),
        "mode report should include 'Language:  org', got:\n{}",
        content
    );
}

// --- PSK option tests ---

#[test]
fn collab_psk_set_and_get_masks_value() {
    let mut editor = Editor::new();
    let result = editor.set_option("collab_psk", "my-secret-key");
    assert!(result.is_ok(), "set collab_psk should succeed: {result:?}");
    assert_eq!(
        editor.collab.psk, "my-secret-key",
        "internal field should store actual key"
    );

    // get_option should mask the value (never leak key in UI)
    let (val, _def) = editor
        .get_option("collab_psk")
        .expect("option should exist");
    assert_eq!(
        val, "********",
        "get_option should mask PSK value, not return plaintext"
    );
}

#[test]
fn collab_psk_empty_returns_empty_on_get() {
    let editor = Editor::new();
    let (val, _) = editor
        .get_option("collab_psk")
        .expect("option should exist");
    assert_eq!(val, "", "empty PSK should return empty string, not mask");
}

#[test]
fn collab_psk_command_set_and_get() {
    let mut editor = Editor::new();
    let result = editor.set_option("collab_psk_command", "pass show mae/key");
    assert!(result.is_ok(), "set collab_psk_command should succeed");
    assert_eq!(editor.collab.psk_command, "pass show mae/key");

    let (val, _) = editor
        .get_option("collab_psk_command")
        .expect("option should exist");
    assert_eq!(
        val, "pass show mae/key",
        "psk_command is not a secret — should return plaintext"
    );
}

#[test]
fn collab_psk_accessible_via_scheme_alias() {
    let mut editor = Editor::new();
    // Scheme API uses hyphenated names
    let result = editor.set_option("collab-psk", "alias-test");
    assert!(result.is_ok(), "hyphenated alias should work");
    assert_eq!(editor.collab.psk, "alias-test");

    let result = editor.set_option("collab-psk-command", "echo test");
    assert!(
        result.is_ok(),
        "hyphenated alias should work for psk_command"
    );
    assert_eq!(editor.collab.psk_command, "echo test");
}

#[test]
fn collab_auth_mode_validates_and_sets() {
    let mut editor = Editor::new();
    assert_eq!(editor.collab.auth_mode, "psk", "default auth mode");
    assert!(editor.set_option("collab_auth_mode", "key").is_ok());
    assert_eq!(editor.collab.auth_mode, "key");
    assert!(editor.set_option("collab-auth-mode", "none").is_ok());
    assert_eq!(editor.collab.auth_mode, "none");
    assert!(
        editor.set_option("collab_auth_mode", "bogus").is_err(),
        "invalid auth mode must be rejected"
    );
}

#[test]
fn collab_host_key_policy_and_tls_options() {
    let mut editor = Editor::new();
    assert_eq!(editor.collab.host_key_policy, "prompt", "default policy");
    assert!(editor.collab.tls, "tls defaults on");
    assert!(editor
        .set_option("collab_host_key_policy", "accept-new")
        .is_ok());
    assert_eq!(editor.collab.host_key_policy, "accept-new");
    assert!(editor.set_option("collab_host_key_policy", "nope").is_err());
    assert!(editor.set_option("collab_tls", "false").is_ok());
    assert!(!editor.collab.tls);
}

#[test]
fn collab_psk_option_registered_with_config_key() {
    let editor = Editor::new();
    let def = editor
        .option_registry
        .find("collab_psk")
        .expect("collab_psk should be registered");
    assert_eq!(
        def.config_key.as_deref(),
        Some("collaboration.psk"),
        "PSK should map to collaboration.psk in config.toml"
    );
    let def = editor
        .option_registry
        .find("collab_psk_command")
        .expect("collab_psk_command should be registered");
    assert_eq!(
        def.config_key.as_deref(),
        Some("collaboration.psk_command"),
        "PSK command should map to collaboration.psk_command in config.toml"
    );
}

// --- :set-save / save_option_to_init persistence ---
//
// save_option_to_init() does real filesystem I/O keyed off XDG_CONFIG_HOME,
// so tests must serialize (env vars are process-global) and use an isolated
// tmp dir — never a shared/well-known path (principle #14 test isolation).

mod set_save_tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Run `f` with XDG_CONFIG_HOME pointed at a fresh tmp dir, restoring
    /// the previous value afterwards even if `f` panics.
    fn with_isolated_config_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
        let _lock = ENV_LOCK.lock().unwrap();
        let tmp = tempfile::tempdir().expect("tmpdir");
        let prev = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", tmp.path());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(tmp.path())));
        match prev {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        result.unwrap()
    }

    fn init_scm_contents(config_home: &std::path::Path) -> String {
        std::fs::read_to_string(config_home.join("mae").join("init.scm"))
            .expect("init.scm should exist")
    }

    #[test]
    fn creates_managed_section_when_init_scm_absent() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            let msg = editor.save_option_to_init("ai_chat_enabled").unwrap();
            assert!(msg.contains("ai_chat_enabled = true"));

            let content = init_scm_contents(config_home);
            assert!(content.contains(";; --- MAE managed options ---"));
            assert!(content.contains(";; --- end managed options ---"));
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    #[test]
    fn appends_second_option_into_existing_managed_section() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            editor.set_option("spell_enabled", "true").unwrap();
            editor.save_option_to_init("spell_enabled").unwrap();

            let content = init_scm_contents(config_home);
            // Exactly one managed section, both options present inside it.
            assert_eq!(content.matches(";; --- MAE managed options ---").count(), 1);
            assert_eq!(content.matches(";; --- end managed options ---").count(), 1);
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
            assert!(content.contains("(set-option! \"spell_enabled\" \"true\")"));
        });
    }

    #[test]
    fn resaving_same_option_replaces_line_instead_of_duplicating() {
        // Adversarial: re-running :set-save for an option already present
        // must overwrite its line, not append a second, conflicting one —
        // a real Scheme file would apply both sequentially and "last write
        // wins" would be silently order-dependent instead of idempotent.
        with_isolated_config_home(|_config_home| {
            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            editor.set_option("ai_chat_enabled", "false").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            let content =
                std::fs::read_to_string(dirs_config_home_path().join("mae").join("init.scm"))
                    .unwrap();
            let occurrences = content
                .lines()
                .filter(|l| {
                    l.trim_start()
                        .starts_with("(set-option! \"ai_chat_enabled\"")
                })
                .count();
            assert_eq!(
                occurrences, 1,
                "resaving must replace the existing line, not duplicate it"
            );
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"false\")"));
            assert!(!content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    /// Resolve the XDG_CONFIG_HOME path the way save_option_to_init does,
    /// for tests that need to re-read the file after the closure captured
    /// `config_home` is out of scope.
    fn dirs_config_home_path() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var("XDG_CONFIG_HOME").unwrap())
    }

    #[test]
    fn preserves_user_content_outside_managed_markers() {
        with_isolated_config_home(|config_home| {
            let mae_dir = config_home.join("mae");
            std::fs::create_dir_all(&mae_dir).unwrap();
            std::fs::write(
                mae_dir.join("init.scm"),
                "; my own config\n(define-key \"normal\" \"g g\" \"goto-first-line\")\n",
            )
            .unwrap();

            let mut editor = Editor::new();
            editor.set_option("ai_chat_enabled", "true").unwrap();
            editor.save_option_to_init("ai_chat_enabled").unwrap();

            let content = init_scm_contents(config_home);
            assert!(content.contains("; my own config"));
            assert!(content.contains("(define-key \"normal\" \"g g\" \"goto-first-line\")"));
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }

    #[test]
    fn unknown_option_errors_without_touching_filesystem() {
        with_isolated_config_home(|config_home| {
            let editor = Editor::new();
            let result = editor.save_option_to_init("not_a_real_option");
            assert!(result.is_err());
            assert!(!config_home.join("mae").join("init.scm").exists());
        });
    }

    #[test]
    fn escapes_quotes_and_backslashes_in_string_values() {
        // Adversarial: a string-valued option (e.g. a shell command) may
        // legitimately contain a `"` or `\`. An unescaped write would emit
        // invalid Scheme (or worse, silently truncate the string at the
        // embedded quote), corrupting init.scm for every subsequent load.
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            let tricky = r#"echo "hi" \ done"#;
            editor.set_option("ai_api_key_command", tricky).unwrap();
            editor.save_option_to_init("ai_api_key_command").unwrap();

            let content = init_scm_contents(config_home);
            let expected = format!(
                "(set-option! \"ai_api_key_command\" \"{}\")",
                tricky.replace('\\', "\\\\").replace('"', "\\\"")
            );
            assert!(
                content.contains(&expected),
                "expected escaped line in init.scm, got:\n{}",
                content
            );

            // Round-trip: unescaping the written literal (mirroring R7RS
            // string-escape rules: \\ -> \, \" -> ") must reproduce the
            // original value exactly. mae-core has no dependency on the
            // Scheme reader itself, so this pins the same contract the
            // reader (crates/scheme/src/reader.rs) is expected to honor
            // without pulling that crate in as a test dependency.
            let escaped = tricky.replace('\\', "\\\\").replace('"', "\\\"");
            let mut unescaped = String::with_capacity(escaped.len());
            let mut chars = escaped.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    match chars.next() {
                        Some(next) => unescaped.push(next),
                        None => panic!("dangling escape in written literal"),
                    }
                } else {
                    unescaped.push(c);
                }
            }
            assert_eq!(unescaped, tricky, "escaping must be exactly reversible");
        });
    }

    #[test]
    fn set_save_command_applies_value_then_persists() {
        with_isolated_config_home(|config_home| {
            let mut editor = Editor::new();
            assert!(!editor.ai_chat_enabled);

            editor.execute_command("set-save ai_chat_enabled true");

            assert!(
                editor.ai_chat_enabled,
                ":set-save must apply the value, not just persist it"
            );
            let content = init_scm_contents(config_home);
            assert!(content.contains("(set-option! \"ai_chat_enabled\" \"true\")"));
        });
    }
}
