use crate::{Mode, VisualType};

use super::Editor;

impl Editor {
    // --- Visual mode ---

    /// Enter visual mode, recording the anchor at the current cursor position.
    ///
    /// Also stamps `Cursor::anchor` on every cursor (primary + secondaries)
    /// -- a per-cursor field that already existed for exactly this purpose
    /// but was never populated before #364. The primary keeps using
    /// `vi.visual_anchor_row/col` as its authoritative anchor (unchanged,
    /// every other visual-mode method already depends on it); secondaries'
    /// `Cursor::anchor` is what lets `visual_selection_ranges()` give each
    /// one its own independent selection instead of collapsing to just the
    /// primary's.
    pub fn enter_visual_mode(&mut self, vtype: VisualType) {
        let win = self.window_mgr.focused_window_mut();
        self.vi.visual_anchor_row = win.cursor_row;
        self.vi.visual_anchor_col = win.cursor_col;
        for c in win.cursor_set.iter_mut() {
            c.anchor = Some((c.row, c.col));
        }
        self.set_mode(Mode::Visual(vtype));
    }

    /// Compute the ordered char-offset range for the current visual selection.
    /// Returns `(start, end)` where `start..end` is the selected range.
    pub fn visual_selection_range(&self) -> (usize, usize) {
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();

        match self.mode {
            Mode::Visual(VisualType::Line) => {
                let min_row = self.vi.visual_anchor_row.min(win.cursor_row);
                let max_row = self.vi.visual_anchor_row.max(win.cursor_row);
                let start = buf.rope().line_to_char(min_row);
                let end = if max_row + 1 < buf.line_count() {
                    buf.rope().line_to_char(max_row + 1)
                } else {
                    buf.rope().len_chars()
                };
                (start, end)
            }
            Mode::Visual(VisualType::Block) => {
                // For block mode, return the bounding char range covering the whole rect.
                // This is a rough approximation used for selection size; block operators
                // use block_selection_rect() directly.
                let (min_row, max_row, min_col, max_col) = self.block_selection_rect();
                let start = buf.char_offset_at(min_row, min_col);
                let end_row_col = (max_col + 1).min(
                    buf.line_text(max_row)
                        .trim_end_matches('\n')
                        .chars()
                        .count(),
                );
                let end = buf.char_offset_at(max_row, end_row_col);
                (start, end.max(start))
            }
            _ => {
                // Charwise
                let anchor =
                    buf.char_offset_at(self.vi.visual_anchor_row, self.vi.visual_anchor_col);
                let cursor = buf.char_offset_at(win.cursor_row, win.cursor_col);
                let start = anchor.min(cursor);
                let end = (anchor.max(cursor) + 1).min(buf.rope().len_chars());
                (start, end)
            }
        }
    }

    /// Compute one char-offset range PER ACTIVE CURSOR for the current
    /// visual selection (#364). Single-cursor: returns exactly
    /// `vec![self.visual_selection_range()]`, byte-for-byte identical to
    /// pre-#364 behavior. Block mode is unaffected (multi-cursor +
    /// block-visual stays primary-only -- see the #364 follow-up issue).
    ///
    /// The primary's range is `visual_selection_range()`, unchanged. Each
    /// secondary's range is computed from ITS OWN `Cursor::anchor`
    /// (stamped by `enter_visual_mode`) to its own current position --
    /// falling back to its current position as its own anchor if somehow
    /// unset (defensive; `enter_visual_mode` always sets it, but a cursor
    /// added via `mc-add-cursor-*` AFTER entering visual mode would have no
    /// anchor otherwise).
    pub fn visual_selection_ranges(&self) -> Vec<(usize, usize)> {
        let win = self.window_mgr.focused_window();
        if win.cursor_set.is_single() || matches!(self.mode, Mode::Visual(VisualType::Block)) {
            return vec![self.visual_selection_range()];
        }

        let buf = &self.buffers[self.active_buffer_idx()];
        let mut ranges = vec![self.visual_selection_range()];
        for c in win.cursor_set.secondaries() {
            let (anchor_row, anchor_col) = c.anchor.unwrap_or((c.row, c.col));
            match self.mode {
                Mode::Visual(VisualType::Line) => {
                    let min_row = anchor_row.min(c.row);
                    let max_row = anchor_row.max(c.row);
                    let start = buf.rope().line_to_char(min_row);
                    let end = if max_row + 1 < buf.line_count() {
                        buf.rope().line_to_char(max_row + 1)
                    } else {
                        buf.rope().len_chars()
                    };
                    ranges.push((start, end));
                }
                _ => {
                    let anchor_off = buf.char_offset_at(anchor_row, anchor_col);
                    let cursor_off = buf.char_offset_at(c.row, c.col);
                    let start = anchor_off.min(cursor_off);
                    let end = (anchor_off.max(cursor_off) + 1).min(buf.rope().len_chars());
                    ranges.push((start, end));
                }
            }
        }
        ranges
    }

    /// Delete the visual selection(s), storing the combined text in the
    /// default register. Multi-cursor (#364): deletes each cursor's own
    /// range (`visual_selection_ranges()`), not just the primary's.
    pub fn visual_delete(&mut self) {
        let mut ranges = self.visual_selection_ranges();
        if ranges.iter().all(|(s, e)| s >= e) {
            self.set_mode(Mode::Normal);
            return;
        }
        let idx = self.active_buffer_idx();

        // Capture register text in ascending (top-to-bottom) order BEFORE
        // any mutation invalidates later ranges' offsets.
        let mut ascending = ranges.clone();
        ascending.sort_by_key(|(s, _)| *s);
        let combined: String = ascending
            .iter()
            .filter(|(s, e)| s < e)
            .map(|(s, e)| self.buffers[idx].text_range(*s, *e))
            .collect();
        self.save_delete(combined);

        // Mutate in descending order so an earlier (higher-offset) deletion
        // never shifts a later (lower-offset) range's start/end out from
        // under it -- mirrors `replay_at_secondaries`'s established pattern
        // (`multicursor.rs`).
        ranges.sort_by_key(|b| std::cmp::Reverse(b.0));
        let mut new_positions: Vec<(usize, usize)> = Vec::with_capacity(ranges.len());
        for (start, end) in &ranges {
            if start >= end {
                continue;
            }
            self.buffers[idx].delete_range(*start, *end);
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line((*start).min(rope.len_chars().saturating_sub(1)));
            let line_start = rope.line_to_char(new_row);
            new_positions.push((new_row, start.saturating_sub(line_start)));
        }
        // Each position was clamped against the rope state immediately
        // after ITS OWN deletion -- but earlier (higher-offset) entries can
        // still be stale once LATER deletions shrink the buffer further
        // (e.g. deleting every cursor's line leaves several positions
        // computed against intermediate, now-too-large rope sizes). Re-clamp
        // every position against the FINAL buffer state before dedup.
        let final_line_count = self.buffers[idx].line_count();
        for (row, col) in &mut new_positions {
            *row = (*row).min(final_line_count.saturating_sub(1));
            *col = (*col).min(self.buffers[idx].line_len(*row));
        }
        new_positions.sort();
        // Collapsed ranges (e.g. deleting the entire buffer) can leave
        // multiple cursors pointing at the identical resulting position --
        // dedup so cursor_set doesn't end up with redundant secondaries
        // stacked on top of the primary.
        new_positions.dedup();

        // Rebuild cursor_set: the topmost surviving position becomes
        // primary, the rest become secondaries -- lets a following
        // `visual_change`'s insert-mode typing replicate at all of them via
        // the existing multi-cursor insert-replay mechanism.
        let win = self.window_mgr.focused_window_mut();
        win.cursor_set.clear_secondaries();
        if let Some(&(row, col)) = new_positions.first() {
            win.cursor_row = row;
            win.cursor_col = col;
        }
        win.clamp_cursor(&self.buffers[idx]);
        win.sync_primary();
        for &(row, col) in &new_positions[1..] {
            win.cursor_set.add(row, col);
        }
        self.set_mode(Mode::Normal);
    }

    /// Yank the visual selection(s) into the default register without
    /// deleting. Multi-cursor (#364): the register holds each cursor's own
    /// range concatenated in top-to-bottom order; cursor positions are left
    /// untouched (nothing was deleted, so there's no "start of what
    /// changed" to move to for more than one range).
    pub fn visual_yank(&mut self) {
        let idx = self.active_buffer_idx();

        // Conversation buffer: yank from flat_text instead of rope
        if self.buffers[idx].conversation().is_some() {
            if let Some(text) = self.conversation_visual_yank_text() {
                self.save_yank(text);
            }
            self.set_mode(Mode::Normal);
            return;
        }

        let mut ranges = self.visual_selection_ranges();
        if ranges.iter().all(|(s, e)| s >= e) {
            self.set_mode(Mode::Normal);
            return;
        }
        ranges.sort_by_key(|(s, _)| *s); // ascending, top-to-bottom
        let combined: String = ranges
            .iter()
            .filter(|(s, e)| s < e)
            .map(|(s, e)| self.buffers[idx].text_range(*s, *e))
            .collect();
        self.save_yank(combined);

        if ranges.len() == 1 {
            // Preserve pre-#364 single-cursor behavior exactly: move cursor
            // to selection start.
            let (start, _) = ranges[0];
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line(start);
            let line_start = rope.line_to_char(new_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = start - line_start;
        }
        self.set_mode(Mode::Normal);
    }

    /// Extract the selected text from a conversation buffer's flat_text.
    fn conversation_visual_yank_text(&self) -> Option<String> {
        let idx = self.active_buffer_idx();
        let conv = self.buffers[idx].conversation()?;
        let flat = conv.flat_text();
        let lines: Vec<&str> = flat.lines().collect();
        let win = self.window_mgr.focused_window();
        let min_row = self.vi.visual_anchor_row.min(win.cursor_row);
        let max_row = self.vi.visual_anchor_row.max(win.cursor_row);

        match self.mode {
            Mode::Visual(VisualType::Line) => {
                let selected: Vec<&str> = lines
                    .iter()
                    .skip(min_row)
                    .take(max_row - min_row + 1)
                    .copied()
                    .collect();
                Some(selected.join("\n") + "\n")
            }
            _ => {
                // Charwise selection
                let mut result = String::new();
                for (row, line) in lines
                    .iter()
                    .enumerate()
                    .take(max_row.min(lines.len().saturating_sub(1)) + 1)
                    .skip(min_row)
                {
                    if row == min_row && row == max_row {
                        let start_col = self.vi.visual_anchor_col.min(win.cursor_col);
                        let end_col =
                            (self.vi.visual_anchor_col.max(win.cursor_col) + 1).min(line.len());
                        if start_col < line.len() {
                            result.push_str(&line[start_col..end_col.min(line.len())]);
                        }
                    } else if row == min_row {
                        let start_col = if self.vi.visual_anchor_row < win.cursor_row {
                            self.vi.visual_anchor_col
                        } else {
                            win.cursor_col
                        };
                        if start_col < line.len() {
                            result.push_str(&line[start_col..]);
                        }
                        result.push('\n');
                    } else if row == max_row {
                        let end_col = if self.vi.visual_anchor_row > win.cursor_row {
                            self.vi.visual_anchor_col + 1
                        } else {
                            win.cursor_col + 1
                        };
                        result.push_str(&line[..end_col.min(line.len())]);
                    } else {
                        result.push_str(line);
                        result.push('\n');
                    }
                }
                if result.is_empty() {
                    None
                } else {
                    Some(result)
                }
            }
        }
    }

    /// Change the visual selection: delete it and enter insert mode.
    pub fn visual_change(&mut self) {
        self.visual_delete();
        self.set_mode(Mode::Insert);
    }

    /// Save the current visual state for `gv` (reselect-visual).
    pub fn save_visual_state(&mut self) {
        let win = self.window_mgr.focused_window();
        if let Mode::Visual(vtype) = self.mode {
            self.vi.last_visual = Some((
                self.vi.visual_anchor_row,
                self.vi.visual_anchor_col,
                win.cursor_row,
                win.cursor_col,
                vtype,
            ));
        }
    }

    /// Swap cursor and anchor in visual mode (o key).
    pub fn visual_swap_ends(&mut self) {
        let win = self.window_mgr.focused_window_mut();
        let (ar, ac) = (self.vi.visual_anchor_row, self.vi.visual_anchor_col);
        self.vi.visual_anchor_row = win.cursor_row;
        self.vi.visual_anchor_col = win.cursor_col;
        win.cursor_row = ar;
        win.cursor_col = ac;
    }

    /// Indent all lines in the visual selection by 4 spaces.
    pub fn visual_indent(&mut self) {
        self.save_visual_state();
        let win = self.window_mgr.focused_window();
        let min_row = self.vi.visual_anchor_row.min(win.cursor_row);
        let max_row = self.vi.visual_anchor_row.max(win.cursor_row);
        let idx = self.active_buffer_idx();
        for row in min_row..=max_row {
            let line_start = self.buffers[idx].rope().line_to_char(row);
            self.buffers[idx].insert_text_at(line_start, "    ");
        }
        self.set_mode(Mode::Normal);
    }

    /// Dedent all lines in the visual selection by up to 4 spaces.
    pub fn visual_dedent(&mut self) {
        self.save_visual_state();
        let win = self.window_mgr.focused_window();
        let min_row = self.vi.visual_anchor_row.min(win.cursor_row);
        let max_row = self.vi.visual_anchor_row.max(win.cursor_row);
        let idx = self.active_buffer_idx();
        // Process in reverse so char offsets stay valid.
        for row in (min_row..=max_row).rev() {
            let line_start = self.buffers[idx].rope().line_to_char(row);
            let line_text = self.buffers[idx].line_text(row);
            let spaces: usize = line_text.chars().take(4).take_while(|c| *c == ' ').count();
            if spaces > 0 {
                self.buffers[idx].delete_range(line_start, line_start + spaces);
            }
        }
        self.set_mode(Mode::Normal);
    }

    /// Join all lines in the visual selection.
    pub fn visual_join(&mut self) {
        self.save_visual_state();
        let win = self.window_mgr.focused_window();
        let min_row = self.vi.visual_anchor_row.min(win.cursor_row);
        let max_row = self.vi.visual_anchor_row.max(win.cursor_row);
        let join_count = max_row - min_row;
        // Position cursor at min_row for joining.
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = min_row;
        for _ in 0..join_count {
            self.join_line();
        }
        self.set_mode(Mode::Normal);
    }

    /// Replace visual selection with register contents without clobbering the register.
    pub fn visual_paste(&mut self) {
        self.save_visual_state();
        // Read paste text before the delete so we don't lose it.
        let paste = self.paste_text();
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.set_mode(Mode::Normal);
            return;
        }
        let idx = self.active_buffer_idx();
        // Delete the selection (save to black-hole by using active_register = '_').
        self.vi.active_register = Some('_');
        let text = self.buffers[idx].text_range(start, end);
        self.buffers[idx].delete_range(start, end);
        self.save_delete(text);
        // Insert paste text at the deletion point.
        if let Some(ref paste_text) = paste {
            self.buffers[idx].insert_text_at(start, paste_text);
            let end_pos = start + paste_text.chars().count().saturating_sub(1);
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line(end_pos.min(rope.len_chars().saturating_sub(1)));
            let line_start = rope.line_to_char(new_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = end_pos.saturating_sub(line_start);
        } else {
            // No paste text — just position cursor at start.
            let rope = self.buffers[idx].rope();
            let new_row = rope.char_to_line(start.min(rope.len_chars().saturating_sub(1)));
            let line_start = rope.line_to_char(new_row);
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = start.saturating_sub(line_start);
        }
        self.set_mode(Mode::Normal);
    }

    /// Uppercase the visual selection(s) text. Multi-cursor (#364): each
    /// cursor's own range is transformed. Cursor position(s) are left as-is
    /// (matches pre-#364 single-cursor behavior, which never repositioned
    /// the cursor for this operator either).
    pub fn visual_uppercase(&mut self) {
        self.save_visual_state();
        let mut ranges = self.visual_selection_ranges();
        if ranges.iter().all(|(s, e)| s >= e) {
            self.set_mode(Mode::Normal);
            return;
        }
        // Descending order avoids offset drift: `delete_range` +
        // `insert_text_at` isn't guaranteed length-preserving for all
        // Unicode case folding (e.g. German `ß` -> `SS`), so this is a real
        // risk across multiple ranges, not just for `visual_delete`.
        ranges.sort_by_key(|b| std::cmp::Reverse(b.0));
        let idx = self.active_buffer_idx();
        for (start, end) in ranges {
            if start >= end {
                continue;
            }
            let text = self.buffers[idx].text_range(start, end);
            let upper = text.to_uppercase();
            self.buffers[idx].delete_range(start, end);
            self.buffers[idx].insert_text_at(start, &upper);
        }
        self.set_mode(Mode::Normal);
    }

    /// Compute visual selection size: (lines, chars).
    pub fn visual_selection_size(&self) -> (usize, usize) {
        let win = self.window_mgr.focused_window();
        let min_row = self.vi.visual_anchor_row.min(win.cursor_row);
        let max_row = self.vi.visual_anchor_row.max(win.cursor_row);
        let lines = max_row - min_row + 1;
        let (start, end) = self.visual_selection_range();
        let chars = end.saturating_sub(start);
        (lines, chars)
    }

    /// Compute the rectangular region for block visual mode:
    /// (min_row, max_row, min_col, max_col).
    pub fn block_selection_rect(&self) -> (usize, usize, usize, usize) {
        let win = self.window_mgr.focused_window();
        let min_row = self.vi.visual_anchor_row.min(win.cursor_row);
        let max_row = self.vi.visual_anchor_row.max(win.cursor_row);
        let min_col = self.vi.visual_anchor_col.min(win.cursor_col);
        let max_col = self.vi.visual_anchor_col.max(win.cursor_col);
        (min_row, max_row, min_col, max_col)
    }

    /// Delete the rectangular block selection (column range on each row).
    pub fn block_visual_delete(&mut self) {
        self.save_visual_state();
        let (min_row, max_row, min_col, max_col) = self.block_selection_rect();
        let idx = self.active_buffer_idx();
        let mut yanked = String::new();

        self.buffers[idx].begin_undo_group();
        // Process in reverse for stable offsets.
        for row in (min_row..=max_row).rev() {
            let line_start = self.buffers[idx].rope().line_to_char(row);
            let line_text = self.buffers[idx].line_text(row);
            let line_chars: Vec<char> = line_text.trim_end_matches('\n').chars().collect();
            let start = min_col.min(line_chars.len());
            let end = (max_col + 1).min(line_chars.len());
            if start < end {
                let deleted: String = line_chars[start..end].iter().collect();
                yanked = format!("{}\n{}", deleted, yanked);
                self.buffers[idx].delete_range(line_start + start, line_start + end);
            }
        }
        self.buffers[idx].end_undo_group();
        if yanked.ends_with('\n') {
            yanked.pop();
        }
        self.save_delete(yanked);

        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = min_row;
        win.cursor_col = min_col;
        win.clamp_cursor(&self.buffers[idx]);
        self.set_mode(Mode::Normal);
    }

    /// Yank the rectangular block selection without deleting.
    pub fn block_visual_yank(&mut self) {
        self.save_visual_state();
        let (min_row, max_row, min_col, max_col) = self.block_selection_rect();
        let idx = self.active_buffer_idx();
        let mut yanked = String::new();

        for row in min_row..=max_row {
            let line_text = self.buffers[idx].line_text(row);
            let line_chars: Vec<char> = line_text.trim_end_matches('\n').chars().collect();
            let start = min_col.min(line_chars.len());
            let end = (max_col + 1).min(line_chars.len());
            if start < end {
                let selected: String = line_chars[start..end].iter().collect();
                yanked.push_str(&selected);
            }
            if row < max_row {
                yanked.push('\n');
            }
        }
        self.save_yank(yanked);

        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = min_row;
        win.cursor_col = min_col;
        self.set_mode(Mode::Normal);
    }

    /// Insert text at the left edge of the block on all rows.
    /// Called from `I` in block visual: enters insert mode on the first row at
    /// min_col, and when insert exits, the typed text is replicated to all rows.
    /// For simplicity, we capture the text now from the pending insert register
    /// or a given string. This initial implementation prompts with a status
    /// message; the full "type then replicate" UX requires insert-mode exit hooks.
    pub fn block_visual_insert(&mut self, text: &str) {
        if text.is_empty() {
            self.set_mode(Mode::Normal);
            return;
        }
        let (min_row, max_row, min_col, _max_col) = self.block_selection_rect();
        let idx = self.active_buffer_idx();
        self.buffers[idx].begin_undo_group();
        // Insert in reverse so offsets stay stable.
        for row in (min_row..=max_row).rev() {
            let line_start = self.buffers[idx].rope().line_to_char(row);
            let line_len = self.buffers[idx]
                .line_text(row)
                .trim_end_matches('\n')
                .chars()
                .count();
            let col = min_col.min(line_len);
            self.buffers[idx].insert_text_at(line_start + col, text);
        }
        self.buffers[idx].end_undo_group();
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = min_row;
        win.cursor_col = min_col;
        win.clamp_cursor(&self.buffers[idx]);
        self.set_mode(Mode::Normal);
    }

    /// Change the block selection: delete it then enter insert mode.
    pub fn block_visual_change(&mut self) {
        self.block_visual_delete();
        self.set_mode(Mode::Insert);
    }

    /// Lowercase the visual selection(s) text. See `visual_uppercase`'s doc
    /// comment (#364) — same multi-cursor treatment.
    pub fn visual_lowercase(&mut self) {
        self.save_visual_state();
        let mut ranges = self.visual_selection_ranges();
        if ranges.iter().all(|(s, e)| s >= e) {
            self.set_mode(Mode::Normal);
            return;
        }
        ranges.sort_by_key(|b| std::cmp::Reverse(b.0));
        let idx = self.active_buffer_idx();
        for (start, end) in ranges {
            if start >= end {
                continue;
            }
            let text = self.buffers[idx].text_range(start, end);
            let lower = text.to_lowercase();
            self.buffers[idx].delete_range(start, end);
            self.buffers[idx].insert_text_at(start, &lower);
        }
        self.set_mode(Mode::Normal);
    }
}
