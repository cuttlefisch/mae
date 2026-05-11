//! Table navigation and editing operations (M5-M6).

use crate::table;

use super::Editor;

impl Editor {
    /// Move cursor to the next cell in the current table (Tab).
    /// Follows Emacs `org-table-next-field`: save column INDEX before alignment,
    /// then navigate using post-alignment boundaries.
    pub fn table_next_cell(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let col = self.window_mgr.focused_window().cursor_col;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };

        // Save column INDEX (not char offset) before alignment — Emacs pattern.
        let (trow, tcol) =
            table::cell_at_cursor(&t, row, col).unwrap_or((row.saturating_sub(t.start_line), 0));

        // Align (mutates buffer).
        self.table_align_impl(&t);

        // Re-parse after alignment.
        let rope = self.buffers[buf_idx].rope().clone();
        let Some(t) = table::table_at_line(&rope, row) else {
            return;
        };

        let num_cols = t.col_widths.len();
        let num_rows = t.cells.len();

        // Compute next cell index.
        let mut nr = trow;
        let mut nc = tcol + 1;
        loop {
            if nc >= num_cols {
                nc = 0;
                nr += 1;
            }
            if nr >= num_rows {
                // At last row — insert new row.
                // If trailing hline, insert before it; nr points to where data row lands.
                let last_row_idx = t.cells.len().saturating_sub(1);
                let insert_nr = if t.separators.contains(&last_row_idx) {
                    last_row_idx
                } else {
                    nr
                };
                self.table_insert_row_at(&t);
                // Re-parse and move to first cell of new row.
                let rope = self.buffers[buf_idx].rope().clone();
                if let Some(t2) = table::table_at_line(&rope, row) {
                    let new_row = t.start_line + insert_nr;
                    if new_row < t2.end_line
                        && insert_nr < t2.cells.len()
                        && !t2.cells[insert_nr].is_empty()
                    {
                        let (s, _) = t2.cells[insert_nr][0];
                        let win = self.window_mgr.focused_window_mut();
                        win.cursor_row = new_row;
                        win.cursor_col = s + 1;
                    }
                }
                return;
            }
            if !t.separators.contains(&nr) {
                break;
            }
            nr += 1;
            nc = 0;
        }

        // Place cursor using POST-alignment cell boundaries.
        // For right/center aligned cells, land on content not padding.
        if nr < t.cells.len() && nc < t.cells[nr].len() {
            let (s, e) = t.cells[nr][nc];
            let line_str: String = self.buffers[buf_idx]
                .rope()
                .line(t.start_line + nr)
                .chars()
                .collect();
            let cell_content = line_str.get(s..e).unwrap_or("");
            let content_offset = cell_content.find(|c: char| c != ' ').unwrap_or(1);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = t.start_line + nr;
            win.cursor_col = s + content_offset;
        }
    }

    /// Move cursor to the previous cell in the current table (S-Tab).
    /// Follows Emacs pattern: save column INDEX before alignment.
    pub fn table_prev_cell(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let col = self.window_mgr.focused_window().cursor_col;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };

        // Save column INDEX before alignment.
        let (trow, tcol) =
            table::cell_at_cursor(&t, row, col).unwrap_or((row.saturating_sub(t.start_line), 0));

        self.table_align_impl(&t);
        let rope = self.buffers[buf_idx].rope().clone();
        let Some(t) = table::table_at_line(&rope, row) else {
            return;
        };

        let num_cols = t.col_widths.len();
        let mut nr = trow;
        let mut nc = tcol;

        loop {
            if nc == 0 {
                if nr == 0 {
                    return; // Already at first cell.
                }
                nr -= 1;
                nc = num_cols;
            }
            nc -= 1;
            if !t.separators.contains(&nr) {
                break;
            }
            if nr == 0 {
                return;
            }
            nr -= 1;
            nc = num_cols;
        }

        if nr < t.cells.len() && nc < t.cells[nr].len() {
            let (s, e) = t.cells[nr][nc];
            let line_str: String = self.buffers[buf_idx]
                .rope()
                .line(t.start_line + nr)
                .chars()
                .collect();
            let cell_content = line_str.get(s..e).unwrap_or("");
            let content_offset = cell_content.find(|c: char| c != ' ').unwrap_or(1);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = t.start_line + nr;
            win.cursor_col = s + content_offset;
        }
    }

    /// Align all columns in the table under the cursor.
    pub fn table_align(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };
        self.table_align_impl(&t);
        self.set_status("Table aligned");
    }

    fn table_align_impl(&mut self, t: &table::TableMap) {
        let buf_idx = self.active_buffer_idx();
        let rope = self.buffers[buf_idx].rope().clone();
        let formatted = table::format_table(&rope, t);

        self.buffers[buf_idx].begin_undo_group();
        // Replace all table lines.
        let start_char = self.buffers[buf_idx].rope().line_to_char(t.start_line);
        let end_char = if t.end_line < self.buffers[buf_idx].rope().len_lines() {
            self.buffers[buf_idx].rope().line_to_char(t.end_line)
        } else {
            self.buffers[buf_idx].rope().len_chars()
        };
        self.buffers[buf_idx].delete_range(start_char, end_char);
        let new_text: String = formatted.join("");
        self.buffers[buf_idx].insert_text_at(start_char, &new_text);
        self.buffers[buf_idx].end_undo_group();
    }

    /// Insert a new empty row below the cursor in the table.
    pub fn table_insert_row(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };
        self.table_insert_row_at(&t);
    }

    fn table_insert_row_at(&mut self, t: &table::TableMap) {
        let buf_idx = self.active_buffer_idx();
        let max_cols = t.col_widths.len();
        let mut new_row = String::from("|");
        for ci in 0..max_cols {
            let w = t.col_widths[ci].max(1);
            new_row.push(' ');
            for _ in 0..w {
                new_row.push(' ');
            }
            new_row.push_str(" |");
        }
        new_row.push('\n');

        // Insert before a trailing hline (Emacs pattern), or after the table.
        let last_row_idx = t.cells.len().saturating_sub(1);
        let insert_line = if t.separators.contains(&last_row_idx) {
            t.start_line + last_row_idx // before trailing hline
        } else {
            t.end_line // after table
        };
        let insert_pos = if insert_line < self.buffers[buf_idx].rope().len_lines() {
            self.buffers[buf_idx].rope().line_to_char(insert_line)
        } else {
            self.buffers[buf_idx].rope().len_chars()
        };
        self.buffers[buf_idx].begin_undo_group();
        self.buffers[buf_idx].insert_text_at(insert_pos, &new_row);
        self.buffers[buf_idx].end_undo_group();
    }

    /// Delete the current row from the table.
    pub fn table_delete_row(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };

        if t.end_line - t.start_line <= 1 {
            self.set_status("Cannot delete last table row");
            return;
        }

        let start_char = self.buffers[buf_idx].rope().line_to_char(row);
        let end_char = if row + 1 < self.buffers[buf_idx].rope().len_lines() {
            self.buffers[buf_idx].rope().line_to_char(row + 1)
        } else {
            self.buffers[buf_idx].rope().len_chars()
        };

        self.buffers[buf_idx].begin_undo_group();
        self.buffers[buf_idx].delete_range(start_char, end_char);
        self.buffers[buf_idx].end_undo_group();

        // Adjust cursor if needed.
        let win = self.window_mgr.focused_window_mut();
        let line_count = self.buffers[buf_idx].line_count();
        if win.cursor_row >= line_count {
            win.cursor_row = line_count.saturating_sub(1);
        }
        self.set_status("Row deleted");
    }

    /// Insert a new column after the current cell.
    pub fn table_insert_column(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let col = self.window_mgr.focused_window().cursor_col;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };

        let target_col = table::cell_at_cursor(&t, row, col)
            .map(|(_, c)| c + 1)
            .unwrap_or(t.col_widths.len());

        self.buffers[buf_idx].begin_undo_group();
        // Process lines from bottom to top to preserve char offsets.
        for ri in (0..t.cells.len()).rev() {
            let line_idx = t.start_line + ri;
            let line_str: String = self.buffers[buf_idx]
                .rope()
                .line(line_idx)
                .chars()
                .collect();
            // Find the insertion point: after the target_col-th pipe.
            let insert_text = "     |";
            let pipe_positions: Vec<usize> = line_str
                .char_indices()
                .filter(|(_, c)| *c == '|')
                .map(|(i, _)| i)
                .collect();
            if target_col < pipe_positions.len() {
                let insert_byte = pipe_positions[target_col];
                let line_start = self.buffers[buf_idx].rope().line_to_char(line_idx);
                self.buffers[buf_idx].insert_text_at(line_start + insert_byte, insert_text);
            }
        }
        self.buffers[buf_idx].end_undo_group();

        // Re-align.
        let rope = self.buffers[buf_idx].rope().clone();
        if let Some(t) = table::table_at_line(&rope, row) {
            self.table_align_impl(&t);
        }
        self.set_status("Column inserted");
    }

    /// Delete the column at the cursor.
    pub fn table_delete_column(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let col = self.window_mgr.focused_window().cursor_col;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };

        let Some((_, target_col)) = table::cell_at_cursor(&t, row, col) else {
            self.set_status("Not in a cell");
            return;
        };

        if t.col_widths.len() <= 1 {
            self.set_status("Cannot delete last column");
            return;
        }

        self.buffers[buf_idx].begin_undo_group();
        for ri in (0..t.cells.len()).rev() {
            let line_idx = t.start_line + ri;
            let line_str: String = self.buffers[buf_idx]
                .rope()
                .line(line_idx)
                .chars()
                .collect();
            let pipe_positions: Vec<usize> = line_str
                .char_indices()
                .filter(|(_, c)| *c == '|')
                .map(|(i, _)| i)
                .collect();
            if target_col + 1 < pipe_positions.len() {
                let start_byte = pipe_positions[target_col];
                let end_byte = pipe_positions[target_col + 1];
                let line_start = self.buffers[buf_idx].rope().line_to_char(line_idx);
                self.buffers[buf_idx].delete_range(line_start + start_byte, line_start + end_byte);
            }
        }
        self.buffers[buf_idx].end_undo_group();

        let rope = self.buffers[buf_idx].rope().clone();
        if let Some(t) = table::table_at_line(&rope, row) {
            self.table_align_impl(&t);
        }
        self.set_status("Column deleted");
    }

    /// Swap current row with the row above.
    pub fn table_move_row_up(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };

        let trow = row - t.start_line;
        if trow == 0 {
            self.set_status("Already at first row");
            return;
        }

        // Find prev non-separator row.
        let mut target = trow - 1;
        while target > 0 && t.separators.contains(&target) {
            target -= 1;
        }
        if t.separators.contains(&target) {
            return;
        }

        self.swap_lines(row, t.start_line + target);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = t.start_line + target;
    }

    /// Swap current row with the row below.
    pub fn table_move_row_down(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let rope = self.buffers[buf_idx].rope().clone();

        let Some(t) = table::table_at_line(&rope, row) else {
            self.set_status("Not in a table");
            return;
        };

        let trow = row - t.start_line;
        let num_rows = t.cells.len();
        if trow + 1 >= num_rows {
            self.set_status("Already at last row");
            return;
        }

        let mut target = trow + 1;
        while target < num_rows && t.separators.contains(&target) {
            target += 1;
        }
        if target >= num_rows {
            return;
        }

        self.swap_lines(row, t.start_line + target);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = t.start_line + target;
    }

    /// Swap two lines in the active buffer.
    fn swap_lines(&mut self, a: usize, b: usize) {
        let buf_idx = self.active_buffer_idx();
        let line_a: String = self.buffers[buf_idx].rope().line(a).chars().collect();
        let line_b: String = self.buffers[buf_idx].rope().line(b).chars().collect();

        let (first, second) = if a < b { (a, b) } else { (b, a) };

        self.buffers[buf_idx].begin_undo_group();
        // Replace second line first (to preserve offsets).
        let start2 = self.buffers[buf_idx].rope().line_to_char(second);
        let end2 = start2 + self.buffers[buf_idx].rope().line(second).len_chars();
        let first_line_text = if first == a { &line_a } else { &line_b };
        self.buffers[buf_idx].delete_range(start2, end2);
        self.buffers[buf_idx].insert_text_at(start2, first_line_text);

        let start1 = self.buffers[buf_idx].rope().line_to_char(first);
        let end1 = start1 + self.buffers[buf_idx].rope().line(first).len_chars();
        let second_line_text = if first == a { &line_b } else { &line_a };
        self.buffers[buf_idx].delete_range(start1, end1);
        self.buffers[buf_idx].insert_text_at(start1, second_line_text);

        self.buffers[buf_idx].end_undo_group();
    }
}
