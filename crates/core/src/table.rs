//! Table parsing, navigation, and editing for org-mode and markdown.
//!
//! Detects `| cell | cell |` tables in both syntaxes, provides cell
//! boundary maps for navigation (Tab/S-Tab), alignment, and structural
//! operations (insert/delete row/column).

use ropey::Rope;
use std::collections::HashSet;

/// Per-column text alignment parsed from separator lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnAlignment {
    Left,
    Center,
    Right,
}

/// Map of a parsed table: row/column cell boundaries.
#[derive(Debug, Clone)]
pub struct TableMap {
    /// First line index (0-based) of the table in the buffer.
    pub start_line: usize,
    /// Exclusive end line.
    pub end_line: usize,
    /// `cells[row][col]` = `(char_start, char_end)` within the line (trimmed content).
    /// char offsets are relative to the line start.
    pub cells: Vec<Vec<(usize, usize)>>,
    /// Maximum character width per column (for alignment).
    pub col_widths: Vec<usize>,
    /// Row indices (relative to start_line) that are separator lines.
    pub separators: HashSet<usize>,
    /// Per-column alignment parsed from separator lines.
    pub alignments: Vec<ColumnAlignment>,
}

/// Check if a line is a table row (starts/ends with `|`).
fn is_table_line(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() >= 2
}

/// Check if a line is a separator row (`|---|---|`).
fn is_separator_line(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return false;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    !inner.is_empty()
        && inner.contains('-')
        && inner
            .chars()
            .all(|c| c == '-' || c == '+' || c == ':' || c == '|' || c == ' ')
}

/// Parse cell boundaries for a single table line.
/// Returns `(start, end)` pairs for each cell content (trimmed),
/// where offsets are character-based within the line.
fn parse_cells(line: &str) -> Vec<(usize, usize)> {
    let mut cells = Vec::new();
    let trimmed = line.trim_end_matches('\n');
    // Find | boundaries.
    let pipe_positions: Vec<usize> = trimmed
        .char_indices()
        .filter(|(_, c)| *c == '|')
        .map(|(i, _)| i)
        .collect();
    if pipe_positions.len() < 2 {
        return cells;
    }
    for pair in pipe_positions.windows(2) {
        let start = pair[0] + 1; // char after |
        let end = pair[1]; // char of next |
                           // Trim whitespace within cell.
                           // Store raw start..end (between pipes); alignment handles padding.
        cells.push((start, end));
    }
    cells
}

/// Detect the table at a given line, or `None` if the line isn't part of a table.
pub fn table_at_line(rope: &Rope, line: usize) -> Option<TableMap> {
    if line >= rope.len_lines() {
        return None;
    }
    let line_str: String = rope.line(line).chars().collect();
    if !is_table_line(&line_str) {
        return None;
    }

    // Scan upward to find table start.
    let mut start = line;
    while start > 0 {
        let prev: String = rope.line(start - 1).chars().collect();
        if is_table_line(&prev) {
            start -= 1;
        } else {
            break;
        }
    }

    // Scan downward to find table end.
    let total_lines = rope.len_lines();
    let mut end = line + 1;
    while end < total_lines {
        let next: String = rope.line(end).chars().collect();
        if is_table_line(&next) {
            end += 1;
        } else {
            break;
        }
    }

    build_table_map(rope, start, end)
}

/// Detect all tables in a rope.
pub fn detect_tables(rope: &Rope) -> Vec<TableMap> {
    let total = rope.len_lines();
    let mut tables = Vec::new();
    let mut i = 0;
    while i < total {
        let line_str: String = rope.line(i).chars().collect();
        if is_table_line(&line_str) {
            let start = i;
            while i < total {
                let l: String = rope.line(i).chars().collect();
                if is_table_line(&l) {
                    i += 1;
                } else {
                    break;
                }
            }
            if let Some(map) = build_table_map(rope, start, i) {
                tables.push(map);
            }
        } else {
            i += 1;
        }
    }
    tables
}

/// Parse alignment markers from a separator line.
/// `:---` or `---` → Left, `:---:` → Center, `---:` → Right.
fn parse_separator_alignment(line: &str) -> Vec<ColumnAlignment> {
    let trimmed = line.trim();
    let inner = if trimmed.starts_with('|') && trimmed.ends_with('|') && trimmed.len() >= 2 {
        &trimmed[1..trimmed.len() - 1]
    } else {
        return Vec::new();
    };
    inner
        .split('|')
        .map(|seg| {
            let s = seg.trim();
            let starts_colon = s.starts_with(':');
            let ends_colon = s.ends_with(':');
            if starts_colon && ends_colon {
                ColumnAlignment::Center
            } else if ends_colon {
                ColumnAlignment::Right
            } else {
                ColumnAlignment::Left
            }
        })
        .collect()
}

fn build_table_map(rope: &Rope, start: usize, end: usize) -> Option<TableMap> {
    let mut cells = Vec::new();
    let mut separators = HashSet::new();
    let mut max_cols = 0;
    let mut alignments = Vec::new();
    let mut alignment_set = false;

    for row_idx in start..end {
        let line_str: String = rope.line(row_idx).chars().collect();
        if is_separator_line(&line_str) {
            separators.insert(row_idx - start);
            cells.push(Vec::new());
            // First separator line wins (consistent with Markdown renderers).
            if !alignment_set {
                alignments = parse_separator_alignment(&line_str);
                alignment_set = true;
            }
        } else {
            let row_cells = parse_cells(&line_str);
            max_cols = max_cols.max(row_cells.len());
            cells.push(row_cells);
        }
    }

    if cells.is_empty() || max_cols == 0 {
        return None;
    }

    // Pad alignments to max_cols (default Left for missing columns).
    alignments.resize(max_cols, ColumnAlignment::Left);

    // Compute column widths from TRIMMED content (not raw cell boundaries).
    // This prevents alignment inflation: raw boundaries include padding that
    // format_table() would then pad again.
    let mut col_widths = vec![0usize; max_cols];
    for (ri, row) in cells.iter().enumerate() {
        if separators.contains(&ri) {
            continue;
        }
        let row_line_idx = start + ri;
        let line_str: String = rope.line(row_line_idx).chars().collect();
        for (ci, &(start_c, end_c)) in row.iter().enumerate() {
            if ci < max_cols {
                let content = line_str.get(start_c..end_c).unwrap_or("").trim();
                col_widths[ci] = col_widths[ci].max(content.len());
            }
        }
    }

    Some(TableMap {
        start_line: start,
        end_line: end,
        cells,
        col_widths,
        separators,
        alignments,
    })
}

/// Parse cells from a raw line string (convenience for editor operations).
pub fn cell_at_line_raw(line: &str) -> Vec<(usize, usize)> {
    parse_cells(line)
}

/// Given a cursor position (row, col), find which cell the cursor is in.
/// Returns `(table_row_offset, cell_index)`.
pub fn cell_at_cursor(table: &TableMap, row: usize, col: usize) -> Option<(usize, usize)> {
    if row < table.start_line || row >= table.end_line {
        return None;
    }
    let table_row = row - table.start_line;
    if table.separators.contains(&table_row) {
        return None;
    }
    let cells = &table.cells[table_row];
    for (ci, &(start, end)) in cells.iter().enumerate() {
        if col >= start && col <= end {
            return Some((table_row, ci));
        }
    }
    // If past the last cell, return the last cell.
    if !cells.is_empty() {
        return Some((table_row, cells.len() - 1));
    }
    None
}

/// Format a table with aligned columns. Returns the formatted lines (with newlines).
pub fn format_table(rope: &Rope, table: &TableMap) -> Vec<String> {
    let max_cols = table.col_widths.len();
    let mut lines = Vec::new();

    for ri in 0..table.cells.len() {
        let row_line = table.start_line + ri;
        if table.separators.contains(&ri) {
            // Rebuild separator with proper widths, preserving alignment markers.
            let mut sep = String::from("|");
            for ci in 0..max_cols {
                let w = table.col_widths[ci].max(3);
                let align = table
                    .alignments
                    .get(ci)
                    .copied()
                    .unwrap_or(ColumnAlignment::Left);
                match align {
                    ColumnAlignment::Left => {
                        sep.push(' ');
                        sep.push_str(&"-".repeat(w + 1));
                    }
                    ColumnAlignment::Center => {
                        sep.push(':');
                        sep.push_str(&"-".repeat(w));
                        sep.push(':');
                    }
                    ColumnAlignment::Right => {
                        sep.push(' ');
                        sep.push_str(&"-".repeat(w));
                        sep.push(':');
                    }
                }
                sep.push('|');
            }
            sep.push('\n');
            lines.push(sep);
        } else {
            let line_str: String = rope.line(row_line).chars().collect();
            let cells = parse_cells(&line_str);
            let mut formatted = String::from("|");
            for ci in 0..max_cols {
                let content = if ci < cells.len() {
                    let (s, e) = cells[ci];
                    line_str[s..e].trim().to_string()
                } else {
                    String::new()
                };
                let w = table.col_widths[ci].max(1);
                let pad = w.saturating_sub(content.len());
                let align = table
                    .alignments
                    .get(ci)
                    .copied()
                    .unwrap_or(ColumnAlignment::Left);
                match align {
                    ColumnAlignment::Left => {
                        formatted.push(' ');
                        formatted.push_str(&content);
                        for _ in 0..pad {
                            formatted.push(' ');
                        }
                    }
                    ColumnAlignment::Right => {
                        formatted.push(' ');
                        for _ in 0..pad {
                            formatted.push(' ');
                        }
                        formatted.push_str(&content);
                    }
                    ColumnAlignment::Center => {
                        let left_pad = pad / 2;
                        let right_pad = pad - left_pad;
                        formatted.push(' ');
                        for _ in 0..left_pad {
                            formatted.push(' ');
                        }
                        formatted.push_str(&content);
                        for _ in 0..right_pad {
                            formatted.push(' ');
                        }
                    }
                }
                formatted.push_str(" |");
            }
            formatted.push('\n');
            lines.push(formatted);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope_from(s: &str) -> Rope {
        Rope::from_str(s)
    }

    #[test]
    fn detect_simple_table() {
        let rope = rope_from("| a | b |\n| c | d |\n");
        let tables = detect_tables(&rope);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.start_line, 0);
        assert_eq!(t.end_line, 2);
        assert_eq!(t.cells.len(), 2);
        assert_eq!(t.cells[0].len(), 2);
    }

    #[test]
    fn detect_table_with_separator() {
        let rope = rope_from("| Name | Age |\n|------|-----|\n| Ada  | 36  |\n");
        let tables = detect_tables(&rope);
        assert_eq!(tables.len(), 1);
        let t = &tables[0];
        assert_eq!(t.start_line, 0);
        assert_eq!(t.end_line, 3);
        assert!(t.separators.contains(&1));
        assert_eq!(t.cells[0].len(), 2); // header
        assert_eq!(t.cells[1].len(), 0); // separator
        assert_eq!(t.cells[2].len(), 2); // data row
    }

    #[test]
    fn table_at_line_finds_table() {
        let rope = rope_from("some text\n| a | b |\n| c | d |\nmore text\n");
        assert!(table_at_line(&rope, 0).is_none());
        let t = table_at_line(&rope, 1).unwrap();
        assert_eq!(t.start_line, 1);
        assert_eq!(t.end_line, 3);
        let t2 = table_at_line(&rope, 2).unwrap();
        assert_eq!(t2.start_line, 1);
    }

    #[test]
    fn cell_at_cursor_identifies_cell() {
        let rope = rope_from("| abc | def |\n| ghi | jkl |\n");
        let t = table_at_line(&rope, 0).unwrap();
        let (row, col) = cell_at_cursor(&t, 0, 3).unwrap();
        assert_eq!(row, 0);
        assert_eq!(col, 0);
        let (row, col) = cell_at_cursor(&t, 0, 8).unwrap();
        assert_eq!(row, 0);
        assert_eq!(col, 1);
    }

    #[test]
    fn detect_multiple_tables() {
        let rope = rope_from("| a |\ntext\n| b |\n| c |\n");
        let tables = detect_tables(&rope);
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].start_line, 0);
        assert_eq!(tables[0].end_line, 1);
        assert_eq!(tables[1].start_line, 2);
        assert_eq!(tables[1].end_line, 4);
    }

    #[test]
    fn separator_detection() {
        assert!(is_separator_line("|---|---|"));
        assert!(is_separator_line("| --- | --- |"));
        assert!(is_separator_line("|:---:|---:|"));
        assert!(!is_separator_line("| abc | def |"));
        assert!(!is_separator_line("not a table"));
    }

    #[test]
    fn format_aligns_columns() {
        let rope = rope_from("| a | bb |\n| ccc | d |\n");
        let t = table_at_line(&rope, 0).unwrap();
        let formatted = format_table(&rope, &t);
        // All rows should have same-width columns.
        assert_eq!(formatted.len(), 2);
        assert_eq!(
            formatted[0].trim_end(),
            formatted[1]
                .trim_end()
                .replace("ccc", "a  ")
                .replace(" d ", " bb")
        );
        // Just verify both lines have the same length.
        assert_eq!(formatted[0].len(), formatted[1].len());
    }

    #[test]
    fn single_column_table() {
        let rope = rope_from("| a |\n| b |\n");
        let tables = detect_tables(&rope);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].cells[0].len(), 1);
    }

    #[test]
    fn col_widths_computed() {
        let rope = rope_from("| short | x |\n| a | longer |\n");
        let t = table_at_line(&rope, 0).unwrap();
        assert!(t.col_widths[0] >= 5); // "short" is 5 chars + padding
        assert!(t.col_widths[1] >= 6); // "longer" is 6 chars + padding
    }

    #[test]
    fn empty_not_a_table() {
        let rope = rope_from("just text\n");
        assert!(table_at_line(&rope, 0).is_none());
        let tables = detect_tables(&rope);
        assert!(tables.is_empty());
    }

    #[test]
    fn alignment_is_idempotent() {
        // Align once, then align again — output must be identical.
        let rope = rope_from("| short | x |\n| a | longer |\n");
        let t = table_at_line(&rope, 0).unwrap();
        let formatted1 = format_table(&rope, &t);
        let text1: String = formatted1.join("");

        // Build a new rope from the first alignment and align again.
        let rope2 = Rope::from_str(&text1);
        let t2 = table_at_line(&rope2, 0).unwrap();
        let formatted2 = format_table(&rope2, &t2);
        let text2: String = formatted2.join("");

        assert_eq!(text1, text2, "Alignment must be idempotent");
    }
}
