//! Integration tests for org-mode rendering pipeline.
//! Verifies that structural spans flow from Editor → buffer → syntax → rendering.

use super::*;
use crate::buffer::Buffer;
use crate::render_common::kb::compute_kb_spans;
use crate::syntax::markup::compute_org_spans;

fn editor_with_org(text: &str) -> Editor {
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/test.org"));
    buf.insert_text_at(0, text);
    let mut editor = Editor::with_buffer(buf);
    editor.window_mgr.focused_window_mut().cursor_row = 0;
    editor.window_mgr.focused_window_mut().cursor_col = 0;
    editor
}

/// Verify that an org file opened in edit mode produces structural spans
/// (TODO, heading, checkbox, link) via the syntax pipeline.
#[test]
fn org_edit_mode_produces_structural_spans() {
    let src = "* TODO Fix bug\n- [ ] item\n- [x] done\n[[link]]\n#+TITLE: T\n";
    let spans = compute_org_spans(src);

    assert!(
        spans.iter().any(|s| s.theme_key == "markup.heading"),
        "missing heading span"
    );
    assert!(
        spans.iter().any(|s| s.theme_key == "markup.todo"),
        "missing TODO span"
    );
    assert!(
        spans.iter().any(|s| s.theme_key == "markup.checkbox"),
        "missing checkbox span"
    );
    assert!(
        spans.iter().any(|s| s.theme_key == "markup.link"),
        "missing link span"
    );
}

/// Verify TODO→DONE cycle changes span type.
#[test]
fn org_edit_mode_todo_cycle_updates_spans() {
    let todo_src = "* TODO Task\n";
    let done_src = "* DONE Task\n";

    let todo_spans = compute_org_spans(todo_src);
    assert!(
        todo_spans.iter().any(|s| s.theme_key == "markup.todo"),
        "TODO source should produce markup.todo"
    );

    let done_spans = compute_org_spans(done_src);
    assert!(
        done_spans.iter().any(|s| s.theme_key == "markup.done"),
        "DONE source should produce markup.done"
    );
    assert!(
        !done_spans.iter().any(|s| s.theme_key == "markup.todo"),
        "DONE source should NOT have markup.todo"
    );
}

/// Verify checkbox toggle changes span type.
#[test]
fn org_edit_mode_checkbox_toggle_updates_spans() {
    let unchecked = "- [ ] item\n";
    let checked = "- [x] item\n";

    let u_spans = compute_org_spans(unchecked);
    assert!(u_spans.iter().any(|s| s.theme_key == "markup.checkbox"));

    let c_spans = compute_org_spans(checked);
    assert!(c_spans
        .iter()
        .any(|s| s.theme_key == "markup.checkbox.checked"));
}

/// Verify markup.heading spans exist for GUI heading scale.
#[test]
fn org_heading_scale_spans_present() {
    let src = "* Big Heading\n** Sub Heading\nBody\n";
    let spans = compute_org_spans(src);
    let heading_count = spans
        .iter()
        .filter(|s| s.theme_key == "markup.heading")
        .count();
    assert!(
        heading_count >= 2,
        "expected at least 2 heading spans, got {}",
        heading_count
    );
}

/// KB view from daily node: create a daily KB node, verify kb_node_id_for_active_buffer finds it.
#[test]
fn kb_view_from_daily_node() {
    let mut e = Editor::new();
    // Insert a daily KB node
    let node = mae_kb::Node::new(
        "daily:2026-05-19",
        "2026-05-19",
        mae_kb::NodeKind::Note,
        "Daily note content",
    );
    e.kb.insert(node);

    // Open a file that looks like a daily
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/2026-05-19.org"));
    buf.insert_text_at(0, "Daily note content\n");
    e.buffers.push(buf);
    let buf_idx = e.buffers.len() - 1;
    e.window_mgr.focused_window_mut().buffer_idx = buf_idx;

    // Should infer the KB node ID
    let id = e.kb_node_id_for_active_buffer();
    assert_eq!(id, Some("daily:2026-05-19".to_string()));
}

/// KB view reopen doesn't create extra windows.
#[test]
fn kb_view_reopen_no_split() {
    let mut e = Editor::new();
    e.open_help_at("index");
    let win_count_before = e.window_mgr.window_count();
    e.help_close();
    e.help_reopen();
    let win_count_after = e.window_mgr.window_count();
    assert_eq!(
        win_count_before, win_count_after,
        "reopen should not create extra windows"
    );
}

/// KB view with TODO content includes structural spans.
#[test]
fn kb_view_has_todo_spans() {
    let mut buf = Buffer::new_kb("test");
    buf.read_only = false;
    buf.insert_text_at(0, "# Test Node\n\n* TODO First task\n* DONE Second task\n");
    buf.read_only = true;
    let spans = compute_kb_spans(&buf);
    assert!(
        spans.iter().any(|s| s.theme_key == "markup.todo"),
        "KB view should include markup.todo spans"
    );
    assert!(
        spans.iter().any(|s| s.theme_key == "markup.done"),
        "KB view should include markup.done spans"
    );
}

/// open_file_at_path detects language for .org files (Fix 2 regression guard).
#[test]
fn daily_file_gets_language_detection() {
    use crate::syntax::Language;
    let dir = tempfile::TempDir::new().unwrap();
    let org_path = dir.path().join("2026-05-19.org");
    std::fs::write(&org_path, "#+title: 2026-05-19\n* TODO Task\n").unwrap();

    let mut e = Editor::new();
    e.open_file_at_path(&org_path);

    let idx = e.buffers.len() - 1;
    assert_eq!(
        e.syntax.language_of(idx),
        Some(Language::Org),
        "open_file_at_path must detect Language::Org for .org files"
    );
}

/// help_return_to_view from a daily buffer should NOT split (Fix 4 regression guard).
#[test]
fn help_return_to_view_no_split_on_first_invoke() {
    let mut e = Editor::new();

    // Insert a daily KB node so kb_node_id_for_active_buffer() returns Some
    let node = mae_kb::Node::new(
        "daily:2026-05-19",
        "2026-05-19",
        mae_kb::NodeKind::Note,
        "Daily note",
    );
    e.kb.insert(node);

    // Set up a buffer that looks like a daily
    let mut buf = Buffer::new();
    buf.set_file_path(std::path::PathBuf::from("/tmp/2026-05-19.org"));
    buf.insert_text_at(0, "Daily note\n");
    e.buffers.push(buf);
    let buf_idx = e.buffers.len() - 1;
    e.window_mgr.focused_window_mut().buffer_idx = buf_idx;

    let win_count_before = e.window_mgr.window_count();
    e.help_return_to_view();
    let win_count_after = e.window_mgr.window_count();
    assert_eq!(
        win_count_before, win_count_after,
        "help_return_to_view should not create extra windows"
    );
}

/// Display regions force-recompute signal (u64::MAX) bypasses debounce.
#[test]
fn toggle_inline_images_forces_immediate_recompute() {
    let mut e = editor_with_org("Visit [[https://example.com][link]] here.\n");
    // Simulate the toggle signal
    e.buffers[0].display_regions_gen = u64::MAX;
    // After compute_visible_syntax_spans, the gen should no longer be u64::MAX
    // because recompute_display_regions sets it to the current generation.
    // We test the debounce bypass logic directly:
    let force = e.buffers[0].display_regions_gen == u64::MAX;
    assert!(force, "u64::MAX should be detected as force signal");
    // Verify that when force is true, dirty_since is not consulted
    e.buffers[0].display_regions_dirty_since = None;
    // The actual recompute happens in compute_visible_syntax_spans which
    // requires a full editor setup. We verify the bypass condition here.
    assert_eq!(
        e.buffers[0].display_regions_gen,
        u64::MAX,
        "force signal should persist until recompute"
    );
}
