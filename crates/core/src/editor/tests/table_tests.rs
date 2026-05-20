//! Table navigation and formatting tests.

use super::*;

#[test]
fn table_next_cell_moves_cursor() {
    let mut editor = editor_with_bulk_text("| abc | def |\n| ghi | jkl |\n");
    // Position cursor in first cell (col 2 = inside " abc ")
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 2;

    editor.table_next_cell();

    let win = editor.window_mgr.focused_window();
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
    let mut editor = editor_with_bulk_text("| a | b |\n|---|---|\n| c | d |\n");
    // Position cursor in last cell of first row
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 6; // inside second cell

    editor.table_next_cell();

    let win = editor.window_mgr.focused_window();
    // Should wrap to first cell of next data row (skipping separator at row 1)
    assert_eq!(
        win.cursor_row, 2,
        "should wrap to row 2 (skipping separator)"
    );
}

#[test]
fn table_alignment_idempotent_via_editor() {
    let mut editor = editor_with_bulk_text("| short | x |\n| a | longer |\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 2;

    // Align twice via table_next_cell (which aligns internally)
    editor.table_align();
    let text1: String = editor.buffers[0].rope().chars().collect();
    editor.table_align();
    let text2: String = editor.buffers[0].rope().chars().collect();

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
    let mut editor = editor_with_bulk_text("| a | b |\n| c | d |\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 1;
    win.cursor_col = 8; // last cell of last row

    editor.table_next_cell();

    // Should now have 3 data rows.
    let text: String = editor.buffers[0].rope().chars().collect();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines.len() >= 3, "should have 3+ lines, got: {text}");

    // Re-parse: the new row must be a data row, not a separator.
    let t = crate::table::table_at_line(editor.buffers[0].rope(), 0).unwrap();
    assert!(
        !t.separators.contains(&2),
        "new row must not be classified as separator"
    );
}

#[test]
fn tab_end_of_table_double_tap() {
    // Two Tabs at end: first adds data row, second adds another (no dashes).
    let mut editor = editor_with_bulk_text("| a | b |\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 8;

    editor.table_next_cell(); // adds row 1
    editor.table_next_cell(); // should add row 2

    let text: String = editor.buffers[0].rope().chars().collect();
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
    let mut editor = editor_with_bulk_text("| a | b |\n|---|---|\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 8; // last cell

    editor.table_next_cell();

    let text: String = editor.buffers[0].rope().chars().collect();
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
    let mut editor = editor_with_bulk_text("| a | b |\n| c | d |\n");
    editor.syntax.set_language(0, crate::syntax::Language::Org);
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 0;
    win.cursor_col = 6; // in second cell

    // heading_global_cycle is what S-Tab dispatches.
    editor.heading_global_cycle(crate::syntax::Language::Org);

    let win = editor.window_mgr.focused_window();
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
    let mut editor = editor_with_bulk_text("| Name | Price |\n|---|---:|\n| Apple | 1 |\n");
    let win = editor.window_mgr.focused_window_mut();
    win.cursor_row = 2;
    win.cursor_col = 2; // in Name cell

    editor.table_next_cell(); // move to Price cell

    let win = editor.window_mgr.focused_window();
    // Cursor should be on '1', not on leading padding space.
    let line: String = editor.buffers[0]
        .rope()
        .line(win.cursor_row)
        .chars()
        .collect();
    let ch = line.chars().nth(win.cursor_col).unwrap_or(' ');
    assert_ne!(
        ch,
        ' ',
        "cursor should land on content, not space; col={} line='{}'",
        win.cursor_col,
        line.trim()
    );
}
