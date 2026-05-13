//! Org checkbox toggle and statistics cookie tests.

use super::*;

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

// --- Heading statistics cookie tests ---

#[test]
fn todo_cycle_updates_parent_frac_cookie() {
    let mut editor = Editor::new();
    editor.buffers[0].insert_text_at(
        0,
        "* Project [/]\n** TODO Task A\n** TODO Task B\n** TODO Task C\n",
    );
    // Cursor on line 1 (Task A), cycle TODO->DONE
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
    // Cycle Task A -> DONE
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
    // Cycle Task A DONE->TODO
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
    // Cycle Task A -> DONE (under Project)
    editor.window_mgr.focused_window_mut().cursor_row = 1;
    editor.org_todo_cycle();
    let parent: String = editor.buffers[0].rope().line(0).chars().collect();
    assert!(
        parent.contains("[1/1]"),
        "Should only count children under Project, got: {parent}"
    );
}
