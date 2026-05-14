//! Shared heading, fold, and narrow operations for org-mode and markdown.

use super::Editor;

impl Editor {
    /// Return the heading level (count of leading `*` or `#`) for a line,
    /// respecting the buffer language. Returns 0 if not a heading.
    pub fn heading_level(line: &str, lang: crate::syntax::Language) -> u8 {
        let prefix_char = match lang {
            crate::syntax::Language::Org => '*',
            crate::syntax::Language::Markdown => '#',
            _ => return 0,
        };
        let count = line.chars().take_while(|&c| c == prefix_char).count();
        if count == 0 {
            return 0;
        }
        // Require a space after the prefix chars (e.g. "* Heading" not "**word")
        let next = line.chars().nth(count);
        if next == Some(' ') || next.is_none() {
            count.min(255) as u8
        } else {
            0
        }
    }

    /// Generalized subtree range: find the range of lines covered by a heading
    /// subtree at `row`. Works for both org (`*`) and markdown (`#`).
    pub fn heading_subtree_range(
        &self,
        row: usize,
        lang: crate::syntax::Language,
    ) -> Option<(usize, usize)> {
        let buf_idx = self.active_buffer_idx();
        let line_count = self.buffers[buf_idx].line_count();
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let level = Self::heading_level(&line, lang);
        if level == 0 {
            return None;
        }
        let mut end = row + 1;
        while end < line_count {
            let l: String = self.buffers[buf_idx].rope().line(end).chars().collect();
            let l_level = Self::heading_level(&l, lang);
            if l_level > 0 && l_level <= level {
                break;
            }
            end += 1;
        }
        Some((row, end))
    }

    /// Find direct child headings within a range (at exactly `parent_level + 1`).
    pub fn direct_child_headings(
        &self,
        start_row: usize,
        end_row: usize,
        parent_level: u8,
        lang: crate::syntax::Language,
    ) -> Vec<usize> {
        let buf_idx = self.active_buffer_idx();
        let mut children = Vec::new();
        for row in (start_row + 1)..end_row {
            let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
            let level = Self::heading_level(&line, lang);
            if level == parent_level + 1 {
                children.push(row);
            }
        }
        children
    }

    /// Generic three-state heading cycle for org and markdown.
    pub fn heading_cycle(&mut self, lang: crate::syntax::Language) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;

        tracing::info!(buf_idx, row, "heading cycle");

        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let level = Self::heading_level(&line, lang);
        if level == 0 {
            // Not on a heading — jump to next link (Doom-style Tab behavior).
            self.text_next_link();
            return;
        }

        let Some((start, end)) = self.heading_subtree_range(row, lang) else {
            return;
        };
        if start >= end.saturating_sub(1) {
            return; // single-line heading, nothing to fold
        }

        // Determine current state from folded_ranges
        let is_folded = self.buffers[buf_idx]
            .folded_ranges
            .iter()
            .any(|(s, _)| *s == start);

        let children = self.direct_child_headings(start, end, level, lang);

        if children.is_empty() {
            // Leaf heading: two-state toggle
            if is_folded {
                self.buffers[buf_idx]
                    .folded_ranges
                    .retain(|(s, _)| *s != start);
                self.set_status("Unfolded");
            } else {
                self.buffers[buf_idx].folded_ranges.push((start, end));
                self.set_status("Folded");
            }
            return;
        }

        // Three-state cycle: SUBTREE → FOLDED → CHILDREN → SUBTREE
        let children_all_folded = children.iter().all(|&child_row| {
            self.buffers[buf_idx]
                .folded_ranges
                .iter()
                .any(|(s, _)| *s == child_row)
        });

        if is_folded {
            // FOLDED → CHILDREN: unfold this heading, fold each direct child
            self.buffers[buf_idx]
                .folded_ranges
                .retain(|(s, _)| *s != start);
            for &child_row in &children {
                // Add fold for each child if it has content to fold
                if let Some((cs, ce)) = self.heading_subtree_range(child_row, lang) {
                    if ce > cs + 1 {
                        // Only add if not already folded
                        if !self.buffers[buf_idx]
                            .folded_ranges
                            .iter()
                            .any(|(s, _)| *s == cs)
                        {
                            self.buffers[buf_idx].folded_ranges.push((cs, ce));
                        }
                    }
                }
            }
            self.set_status("Children");
        } else if children_all_folded {
            // CHILDREN → SUBTREE: unfold all children in range
            self.buffers[buf_idx]
                .folded_ranges
                .retain(|(s, _)| *s < start || *s >= end);
            // Also clear any deeper nested folds within this subtree
            self.set_status("Subtree (all visible)");
        } else {
            // SUBTREE → FOLDED: fold entire subtree
            // First clear any existing folds within this subtree range
            self.buffers[buf_idx]
                .folded_ranges
                .retain(|(s, _)| *s < start || *s >= end);
            self.buffers[buf_idx].folded_ranges.push((start, end));
            self.set_status("Folded");
        }
    }

    /// Promote a heading (remove one prefix char). Works for org (`*`) and markdown (`#`).
    pub fn heading_promote(&mut self, lang: crate::syntax::Language) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(lang) {
            return;
        }
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let level = Self::heading_level(&line, lang);
        if level <= 1 {
            return;
        }
        let start = self.buffers[buf_idx].rope().line_to_char(row);
        self.buffers[buf_idx].delete_range(start, start + 1);
        self.set_status(format!("Promoted to level {}", level - 1));
    }

    /// Demote a heading (add one prefix char). Works for org (`*`) and markdown (`#`).
    pub fn heading_demote(&mut self, lang: crate::syntax::Language) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(lang) {
            return;
        }
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let level = Self::heading_level(&line, lang);
        if level == 0 {
            return;
        }
        let prefix_char = match lang {
            crate::syntax::Language::Org => "*",
            crate::syntax::Language::Markdown => "#",
            _ => return,
        };
        let start = self.buffers[buf_idx].rope().line_to_char(row);
        self.buffers[buf_idx].insert_text_at(start, prefix_char);
        self.set_status(format!("Demoted to level {}", level + 1));
    }

    /// Move a heading subtree down past the next sibling. Works for org and markdown.
    pub fn heading_move_subtree_down(&mut self, lang: crate::syntax::Language) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(lang) {
            return;
        }
        let row = self.window_mgr.focused_window().cursor_row;
        let Some((start, end)) = self.heading_subtree_range(row, lang) else {
            return;
        };
        let line_count = self.buffers[buf_idx].line_count();
        if end >= line_count {
            return;
        }
        let sibling_start = end;
        let sibling_end = match self.heading_subtree_range(sibling_start, lang) {
            Some((_, se)) => se,
            None => sibling_start + 1,
        };

        let rope = self.buffers[buf_idx].rope();
        let our_char_start = rope.line_to_char(start);
        let our_char_end = rope.line_to_char(end);
        let sib_char_end = if sibling_end >= line_count {
            rope.len_chars()
        } else {
            rope.line_to_char(sibling_end)
        };

        let our_text: String = rope.slice(our_char_start..our_char_end).chars().collect();
        let sib_text: String = rope
            .slice(rope.line_to_char(sibling_start)..sib_char_end)
            .chars()
            .collect();

        self.buffers[buf_idx].delete_range(our_char_start, sib_char_end);
        let combined = format!("{}{}", sib_text, our_text);
        self.buffers[buf_idx].insert_text_at(our_char_start, &combined);

        self.buffers[buf_idx]
            .folded_ranges
            .retain(|(s, _)| *s < start || *s >= sibling_end);

        let sib_lines = sib_text.chars().filter(|&c| c == '\n').count();
        self.window_mgr.focused_window_mut().cursor_row = start + sib_lines;
        self.set_status("Moved subtree down");
    }

    /// Move a heading subtree up past the previous sibling. Works for org and markdown.
    pub fn heading_move_subtree_up(&mut self, lang: crate::syntax::Language) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(lang) {
            return;
        }
        let row = self.window_mgr.focused_window().cursor_row;
        let Some((start, end)) = self.heading_subtree_range(row, lang) else {
            return;
        };
        if start == 0 {
            return;
        }

        let line: String = self.buffers[buf_idx].rope().line(start).chars().collect();
        let level = Self::heading_level(&line, lang);

        let mut prev_start = start - 1;
        loop {
            let l: String = self.buffers[buf_idx]
                .rope()
                .line(prev_start)
                .chars()
                .collect();
            let l_level = Self::heading_level(&l, lang);
            if l_level > 0 && l_level <= level {
                break;
            }
            if prev_start == 0 {
                return;
            }
            prev_start -= 1;
        }

        let rope = self.buffers[buf_idx].rope();
        let prev_char_start = rope.line_to_char(prev_start);
        let our_char_start = rope.line_to_char(start);
        let our_char_end = if end >= self.buffers[buf_idx].line_count() {
            rope.len_chars()
        } else {
            rope.line_to_char(end)
        };

        let prev_text: String = rope
            .slice(prev_char_start..our_char_start)
            .chars()
            .collect();
        let our_text: String = rope.slice(our_char_start..our_char_end).chars().collect();

        self.buffers[buf_idx].delete_range(prev_char_start, our_char_end);
        let combined = format!("{}{}", our_text, prev_text);
        self.buffers[buf_idx].insert_text_at(prev_char_start, &combined);

        self.buffers[buf_idx]
            .folded_ranges
            .retain(|(s, _)| *s < prev_start || *s >= end);

        self.window_mgr.focused_window_mut().cursor_row = prev_start;
        self.set_status("Moved subtree up");
    }

    // --- Narrow/Widen ---

    /// Narrow buffer to the subtree at the cursor (org or markdown).
    pub fn narrow_to_subtree(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let lang = self.syntax.language_of(buf_idx);
        let heading_lang = match lang {
            Some(crate::syntax::Language::Org) => crate::syntax::Language::Org,
            Some(crate::syntax::Language::Markdown) => crate::syntax::Language::Markdown,
            _ => {
                self.set_status("Narrow: not an org/markdown buffer");
                return;
            }
        };
        let row = self.window_mgr.focused_window().cursor_row;
        let Some((start, end)) = self.heading_subtree_range(row, heading_lang) else {
            self.set_status("Narrow: not on a heading");
            return;
        };
        self.buffers[buf_idx].narrow_to(start, end);
        // Clamp cursor to narrow range
        let win = self.window_mgr.focused_window_mut();
        if win.cursor_row < start {
            win.cursor_row = start;
            win.cursor_col = 0;
        } else if win.cursor_row >= end {
            win.cursor_row = end.saturating_sub(1);
            win.cursor_col = 0;
        }
        if win.scroll_offset < start {
            win.scroll_offset = start;
        }
        self.set_status("[Narrowed]");
    }

    /// Remove narrowing, restoring the full buffer view.
    pub fn widen(&mut self) {
        let buf_idx = self.active_buffer_idx();
        self.buffers[buf_idx].widen();
        self.set_status("Widened");
    }

    /// Toggle fold at cursor (za). Works for org/markdown headings and code blocks.
    pub fn toggle_fold(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let source: String = self.buffers[buf_idx].rope().chars().collect();

        // For org buffers, delegate to heading cycle
        if self.syntax.language_of(buf_idx) == Some(crate::syntax::Language::Org) {
            self.org_cycle();
            return;
        }
        // For markdown buffers, delegate to heading cycle
        if self.syntax.language_of(buf_idx) == Some(crate::syntax::Language::Markdown) {
            self.md_cycle();
            return;
        }

        // For code buffers, compute fold ranges from tree-sitter
        let fold_ranges = self.syntax.compute_fold_ranges(buf_idx, &source);
        if fold_ranges.is_empty() {
            self.set_status("No foldable regions found");
            return;
        }

        self.buffers[buf_idx].toggle_fold_at(cursor_row, &fold_ranges);
        let is_now_folded = self.buffers[buf_idx].folded_ranges.iter().any(|(s, _)| {
            fold_ranges
                .iter()
                .any(|(fs, _)| *fs == *s && cursor_row >= *s)
        });
        self.set_status(if is_now_folded { "Folded" } else { "Unfolded" });
    }

    /// Compute heading-based fold ranges for org/markdown buffers.
    /// Scans lines for heading prefixes and computes subtree ranges.
    pub fn compute_heading_fold_ranges(
        &self,
        lang: crate::syntax::Language,
    ) -> Vec<(usize, usize)> {
        let buf_idx = self.active_buffer_idx();
        let line_count = self.buffers[buf_idx].line_count();
        let mut ranges = Vec::new();
        let mut i = 0;
        while i < line_count {
            let line: String = self.buffers[buf_idx].rope().line(i).chars().collect();
            let level = Self::heading_level(&line, lang);
            if level > 0 {
                if let Some((s, e)) = self.heading_subtree_range(i, lang) {
                    if e > s + 1 {
                        ranges.push((s, e));
                    }
                }
            }
            i += 1;
        }
        ranges
    }

    /// Close all folds (zM). Folds all tree-sitter/org/markdown fold points.
    pub fn close_all_folds(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let lang = self.syntax.language_of(buf_idx);

        // For org/markdown, use heading-based fold ranges
        if lang == Some(crate::syntax::Language::Org)
            || lang == Some(crate::syntax::Language::Markdown)
        {
            let heading_lang = lang.unwrap();
            let fold_ranges = self.compute_heading_fold_ranges(heading_lang);
            if fold_ranges.is_empty() {
                self.set_status("No foldable headings found");
                return;
            }
            self.buffers[buf_idx].fold_all(&fold_ranges);
            self.set_status(format!("Folded {} headings", fold_ranges.len()));
            return;
        }

        // For code buffers, use tree-sitter fold ranges
        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let fold_ranges = self.syntax.compute_fold_ranges(buf_idx, &source);
        if fold_ranges.is_empty() {
            self.set_status("No foldable regions found");
            return;
        }
        self.buffers[buf_idx].fold_all(&fold_ranges);
        self.set_status(format!("Folded {} regions", fold_ranges.len()));
    }

    /// Open all folds (zR).
    pub fn open_all_folds(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let count = self.buffers[buf_idx].folded_ranges.len();
        self.buffers[buf_idx].unfold_all();
        self.set_status(format!("Unfolded {} regions", count));
    }

    /// Insert a heading at the same level below the current subtree (M-Enter).
    ///
    /// - On a heading line: insert new heading at same level after subtree.
    /// - Not on a heading: insert level-1 heading below current line.
    /// - Enters insert mode with cursor after the heading prefix.
    pub fn insert_heading(&mut self, lang: crate::syntax::Language) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let level = Self::heading_level(&line, lang);

        let (insert_row, insert_level) = if level > 0 {
            let end = self
                .heading_subtree_range(row, lang)
                .map(|(_, e)| e)
                .unwrap_or(row + 1);
            (end, level)
        } else {
            (row + 1, 1)
        };

        let prefix_char = match lang {
            crate::syntax::Language::Org => '*',
            crate::syntax::Language::Markdown => '#',
            _ => return,
        };
        let prefix: String = std::iter::repeat_n(prefix_char, insert_level as usize)
            .chain(std::iter::once(' '))
            .collect();

        // Build the text to insert: newline + heading prefix
        let insert_text = format!("\n{}", prefix);
        let char_offset = self.buffers[buf_idx].rope().line_to_char(insert_row);
        // If inserting at end-of-file, put the newline before the prefix.
        // If inserting between lines, insert at start of insert_row.
        if insert_row >= self.buffers[buf_idx].line_count() {
            // At EOF: append after last char
            let len = self.buffers[buf_idx].rope().len_chars();
            self.buffers[buf_idx].insert_text_at(len, &insert_text);
            len + insert_text.chars().count()
        } else {
            // Insert a new line before insert_row
            let text = format!("{}\n", prefix);
            self.buffers[buf_idx].insert_text_at(char_offset, &text);
            char_offset + text.chars().count() - 1 // before the newline
        };

        // Move cursor to end of prefix and enter insert mode.
        let new_row = insert_row;
        let new_col = prefix.chars().count();
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = new_col;
        self.mode = crate::Mode::Insert;
        self.set_status(format!("Inserted level-{} heading", insert_level));
    }

    /// Global heading fold cycle (Doom Emacs S-TAB pattern).
    ///
    /// Three states: SHOW ALL (0) → OVERVIEW (1) → CONTENTS (2) → SHOW ALL.
    /// - SHOW ALL: clear all heading folds
    /// - OVERVIEW: fold every heading (all levels)
    /// - CONTENTS: show level 1 + 2 headings, fold level 3+
    pub fn heading_global_cycle(&mut self, lang: crate::syntax::Language) {
        // S-Tab on a table line → prev cell navigation instead of global fold.
        let buf_idx = self.active_buffer_idx();
        if self.buffers[buf_idx].rope().len_lines() > self.large_file_lines {
            self.set_status("Global fold cycle disabled for large files");
            return;
        }
        let row = self.window_mgr.focused_window().cursor_row;
        if crate::table::table_at_line(self.buffers[buf_idx].rope(), row).is_some() {
            self.table_prev_cell();
            return;
        }
        let buf_idx = self.active_buffer_idx();
        let state = self.buffers[buf_idx].global_fold_state;
        let next = (state + 1) % 3;
        self.buffers[buf_idx].global_fold_state = next;

        // Collect all headings with their ranges.
        let line_count = self.buffers[buf_idx].line_count();
        let mut headings: Vec<(usize, u8, usize)> = Vec::new(); // (row, level, end)
        for row in 0..line_count {
            let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
            let level = Self::heading_level(&line, lang);
            if level > 0 {
                // Find subtree end
                let mut end = row + 1;
                while end < line_count {
                    let next_line: String =
                        self.buffers[buf_idx].rope().line(end).chars().collect();
                    let next_level = Self::heading_level(&next_line, lang);
                    if next_level > 0 && next_level <= level {
                        break;
                    }
                    end += 1;
                }
                if end > row + 1 {
                    headings.push((row, level, end));
                }
            }
        }

        // Clear all existing heading folds first.
        self.buffers[buf_idx].folded_ranges.clear();

        match next {
            0 => {
                // SHOW ALL — already cleared above
                self.set_status("SHOW ALL");
            }
            1 => {
                // OVERVIEW — fold every heading
                for &(row, _, end) in &headings {
                    self.buffers[buf_idx].folded_ranges.push((row, end));
                }
                self.set_status("OVERVIEW");
            }
            2 => {
                // CONTENTS — fold only level 3+ headings
                for &(row, level, end) in &headings {
                    if level >= 3 {
                        self.buffers[buf_idx].folded_ranges.push((row, end));
                    }
                }
                self.set_status("CONTENTS");
            }
            _ => unreachable!(),
        }
    }

    /// Jump cursor to the next display-region link. Wraps around buffer end.
    pub fn text_next_link(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let regions = &self.buffers[buf_idx].display_regions;
        if regions.is_empty() {
            return;
        }
        let win = self.window_mgr.focused_window();
        let char_offset = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        let cursor_byte = self.buffers[buf_idx].rope().char_to_byte(char_offset);
        if let Some((byte_start, _)) = crate::display_region::next_link_region(regions, cursor_byte)
        {
            let char_pos = self.buffers[buf_idx].rope().byte_to_char(byte_start);
            let row = self.buffers[buf_idx].rope().char_to_line(char_pos);
            let line_start = self.buffers[buf_idx].rope().line_to_char(row);
            let col = char_pos - line_start;
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = row;
            win.cursor_col = col;
        }
    }

    /// Jump cursor to the previous display-region link. Wraps around buffer start.
    pub fn text_prev_link(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let regions = &self.buffers[buf_idx].display_regions;
        if regions.is_empty() {
            return;
        }
        let win = self.window_mgr.focused_window();
        let char_offset = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        let cursor_byte = self.buffers[buf_idx].rope().char_to_byte(char_offset);
        if let Some((byte_start, _)) = crate::display_region::prev_link_region(regions, cursor_byte)
        {
            let char_pos = self.buffers[buf_idx].rope().byte_to_char(byte_start);
            let row = self.buffers[buf_idx].rope().char_to_line(char_pos);
            let line_start = self.buffers[buf_idx].rope().line_to_char(row);
            let col = char_pos - line_start;
            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = row;
            win.cursor_col = col;
        }
    }

    /// Smart newline in insert mode: continues list markers.
    /// Returns `true` if a smart continuation was inserted, `false` otherwise.
    pub fn insert_smart_newline(&mut self) -> bool {
        use regex::Regex;
        use std::sync::OnceLock;

        static UNORDERED: OnceLock<Regex> = OnceLock::new();
        static ORDERED: OnceLock<Regex> = OnceLock::new();
        static CHECKBOX: OnceLock<Regex> = OnceLock::new();

        let unordered = UNORDERED.get_or_init(|| Regex::new(r"^(\s*)([-+*]) (.*)$").unwrap());
        let ordered = ORDERED.get_or_init(|| Regex::new(r"^(\s*)(\d+)([.)]) (.*)$").unwrap());
        let checkbox =
            CHECKBOX.get_or_init(|| Regex::new(r"^(\s*)([-+*]) \[[ xX\-]\] (.*)$").unwrap());

        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let line_trimmed = line.trim_end_matches('\n');

        // Try checkbox first (more specific).
        if let Some(cap) = checkbox.captures(line_trimmed) {
            let indent = cap.get(1).unwrap().as_str();
            let marker = cap.get(2).unwrap().as_str();
            let body = cap.get(3).unwrap().as_str();
            if body.is_empty() {
                // Empty checkbox item → end list (delete marker, plain newline)
                return self.smart_newline_end_list(row, &line);
            }
            let insert = format!("\n{}{} [ ] ", indent, marker);
            self.insert_at_cursor(&insert);
            return true;
        }

        // Try unordered list.
        if let Some(cap) = unordered.captures(line_trimmed) {
            let indent = cap.get(1).unwrap().as_str();
            let marker = cap.get(2).unwrap().as_str();
            let body = cap.get(3).unwrap().as_str();
            if body.is_empty() {
                return self.smart_newline_end_list(row, &line);
            }
            let insert = format!("\n{}{} ", indent, marker);
            self.insert_at_cursor(&insert);
            return true;
        }

        // Try ordered list.
        if let Some(cap) = ordered.captures(line_trimmed) {
            let indent = cap.get(1).unwrap().as_str();
            let num: usize = cap.get(2).unwrap().as_str().parse().unwrap_or(1);
            let sep = cap.get(3).unwrap().as_str();
            let body = cap.get(4).unwrap().as_str();
            if body.is_empty() {
                return self.smart_newline_end_list(row, &line);
            }
            let insert = format!("\n{}{}{} ", indent, num + 1, sep);
            self.insert_at_cursor(&insert);
            return true;
        }

        false
    }

    /// Helper: end a list by replacing the current marker-only line with a plain newline.
    fn smart_newline_end_list(&mut self, row: usize, line: &str) -> bool {
        let buf_idx = self.active_buffer_idx();
        let start = self.buffers[buf_idx].rope().line_to_char(row);
        let end = start + self.buffers[buf_idx].rope().line(row).len_chars();
        let has_nl = line.ends_with('\n');
        self.buffers[buf_idx].begin_undo_group();
        self.buffers[buf_idx].delete_range(start, end);
        let replacement = if has_nl { "\n" } else { "" };
        self.buffers[buf_idx].insert_text_at(start, replacement);
        self.buffers[buf_idx].end_undo_group();
        // Place cursor at start of the new blank line
        let win = self.window_mgr.focused_window_mut();
        win.cursor_col = 0;
        true
    }

    /// Insert text at the current cursor position (insert mode helper).
    pub fn insert_at_cursor(&mut self, text: &str) {
        let buf_idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let pos = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        self.buffers[buf_idx].insert_text_at(pos, text);
        // Move cursor to end of inserted text
        let new_pos = pos + text.chars().count();
        let new_row = self.buffers[buf_idx].rope().char_to_line(new_pos);
        let line_start = self.buffers[buf_idx].rope().line_to_char(new_row);
        let new_col = new_pos - line_start;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = new_col;
    }

    /// Smart enter: context-aware action in org/markdown buffers.
    /// 1. Checkbox line → toggle checkbox
    /// 2. TODO heading → cycle TODO state
    /// 3. Link → follow link
    /// 4. Otherwise → delegate to open-link-at-cursor (which handles display regions too)
    pub fn smart_enter(&mut self) {
        use regex::Regex;
        use std::sync::OnceLock;

        let buf_idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let row = win.cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();

        // 1. Checkbox toggle
        static CHECKBOX_RE: OnceLock<Regex> = OnceLock::new();
        let checkbox_re = CHECKBOX_RE
            .get_or_init(|| Regex::new(r"^(\s*(?:[-+*]|\d+[.)]) )\[([ xX\-])\]").unwrap());
        if checkbox_re.is_match(&line) {
            self.toggle_checkbox_at_cursor();
            return;
        }

        // 2. TODO heading cycle
        static TODO_HEADING: OnceLock<Regex> = OnceLock::new();
        let todo_heading = TODO_HEADING.get_or_init(|| {
            Regex::new(r"^(?:\*+|#+) +(?:TODO|DONE|NEXT|WAIT|CANCELLED|DEFERRED)\b").unwrap()
        });
        if todo_heading.is_match(&line) {
            self.org_todo_cycle();
            return;
        }

        // 3. Fall through to open-link-at-cursor (handles display regions, link_spans, etc.)
        self.dispatch_builtin("open-link-at-cursor");
    }

    /// Toggle checkbox at cursor: `[ ]` ↔ `[x]`, `[-]` → `[ ]`.
    pub fn toggle_checkbox_at_cursor(&mut self) {
        use regex::Regex;
        use std::sync::OnceLock;

        static CHECKBOX_RE: OnceLock<Regex> = OnceLock::new();
        let checkbox_re = CHECKBOX_RE
            .get_or_init(|| Regex::new(r"^(\s*(?:[-+*]|\d+[.)]) )\[([ xX\-])\](.*)$").unwrap());

        let buf_idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let row = win.cursor_row;
        let line: String = self.buffers[buf_idx]
            .rope()
            .line(row)
            .chars()
            .collect::<String>()
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();

        let Some(caps) = checkbox_re.captures(&line) else {
            return;
        };
        let prefix = caps.get(1).unwrap().as_str();
        let state = caps.get(2).unwrap().as_str();
        let rest = caps.get(3).unwrap().as_str();

        let new_state = match state {
            " " => "x",
            _ => " ", // x, X, - all toggle to unchecked
        };

        let new_line = format!("{}[{}]{}\n", prefix, new_state, rest);

        // Replace the line — all edits (checkbox + cookie updates) in one undo group
        self.buffers[buf_idx].begin_undo_group();

        let line_start = self.buffers[buf_idx].rope().line_to_char(row);
        let line_end = if row + 1 < self.buffers[buf_idx].rope().len_lines() {
            self.buffers[buf_idx].rope().line_to_char(row + 1)
        } else {
            self.buffers[buf_idx].rope().len_chars()
        };
        self.buffers[buf_idx].delete_range(line_start, line_end);
        self.buffers[buf_idx].insert_text_at(line_start, &new_line);

        self.set_status(format!(
            "Checkbox {}",
            if new_state == "x" {
                "checked"
            } else {
                "unchecked"
            }
        ));

        // Update parent statistics cookies (adds edits to the open undo group)
        self.update_statistics_cookies(row);

        self.buffers[buf_idx].end_undo_group();
    }

    /// Walk upward from `changed_row` to find parent headings/list items with
    /// statistics cookies (`[n/m]` or `[n%]`) and update them.
    pub fn update_statistics_cookies(&mut self, changed_row: usize) {
        use regex::Regex;
        use std::sync::OnceLock;

        static CHECKBOX_RE: OnceLock<Regex> = OnceLock::new();
        let checkbox_re =
            CHECKBOX_RE.get_or_init(|| Regex::new(r"^\s*(?:[-+*]|\d+[.)]) \[([ xX\-])\]").unwrap());
        static HEADING_RE: OnceLock<Regex> = OnceLock::new();
        let heading_re = HEADING_RE.get_or_init(|| Regex::new(r"^(\*+|#+)\s").unwrap());
        static COOKIE_FRAC: OnceLock<Regex> = OnceLock::new();
        let cookie_frac = COOKIE_FRAC.get_or_init(|| Regex::new(r"\[\d*/\d*\]").unwrap());
        static COOKIE_PCT: OnceLock<Regex> = OnceLock::new();
        let cookie_pct = COOKIE_PCT.get_or_init(|| Regex::new(r"\[\d*%\]").unwrap());

        let buf_idx = self.active_buffer_idx();

        // Find parent: walk upward looking for a parent heading (fewer stars),
        // a lower-indent list item, or any line with a statistics cookie.
        let changed_line: String = self.buffers[buf_idx]
            .rope()
            .line(changed_row)
            .chars()
            .collect();
        let changed_indent = changed_line.len() - changed_line.trim_start().len();
        let changed_heading_level = heading_re
            .captures(&changed_line)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().len());

        let mut parent_row = None;
        let scan_limit = changed_row.saturating_sub(1000);
        for r in (scan_limit..changed_row).rev() {
            let l: String = self.buffers[buf_idx].rope().line(r).chars().collect();
            if heading_re.is_match(&l) {
                let this_level = heading_re
                    .captures(&l)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().len())
                    .unwrap_or(0);
                // Only accept a heading with fewer stars as parent
                if let Some(changed_level) = changed_heading_level {
                    if this_level < changed_level {
                        parent_row = Some(r);
                        break;
                    }
                    // Same or deeper level heading — skip, keep looking
                } else {
                    // Changed line is not a heading (e.g. list item under heading)
                    parent_row = Some(r);
                    break;
                }
            }
            let indent = l.len() - l.trim_start().len();
            if indent < changed_indent
                && l.trim()
                    .starts_with(|c: char| c == '-' || c == '+' || c == '*' || c.is_ascii_digit())
            {
                parent_row = Some(r);
                break;
            }
            // Plain-text line with a cookie (e.g. "Parent task [0/3]:")
            if indent <= changed_indent && (cookie_frac.is_match(&l) || cookie_pct.is_match(&l)) {
                parent_row = Some(r);
                break;
            }
        }

        let Some(parent) = parent_row else {
            return;
        };

        let parent_line: String = self.buffers[buf_idx]
            .rope()
            .line(parent)
            .chars()
            .collect::<String>()
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string();

        let has_frac = cookie_frac.is_match(&parent_line);
        let has_pct = cookie_pct.is_match(&parent_line);

        if !has_frac && !has_pct {
            return;
        }

        // Count children checkboxes
        let line_count = self.buffers[buf_idx].rope().len_lines();
        let parent_indent = parent_line.len() - parent_line.trim_start().len();
        let is_heading = heading_re.is_match(&parent_line);

        let mut total = 0usize;
        let mut checked = 0usize;

        for r in (parent + 1)..line_count {
            let l: String = self.buffers[buf_idx].rope().line(r).chars().collect();
            let trimmed = l.trim_start();
            let indent = l.len() - trimmed.len();

            // Stop at same-level or higher heading
            if is_heading && heading_re.is_match(&l) {
                let parent_level = heading_re
                    .captures(&parent_line)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().len())
                    .unwrap_or(0);
                let this_level = heading_re
                    .captures(&l)
                    .and_then(|c| c.get(1))
                    .map(|m| m.as_str().len())
                    .unwrap_or(0);
                if this_level <= parent_level {
                    break;
                }
            }
            // Stop at same or lower indent for list items
            if !is_heading && indent <= parent_indent && !l.trim().is_empty() {
                break;
            }

            if let Some(caps) = checkbox_re.captures(&l) {
                total += 1;
                let state = caps.get(1).unwrap().as_str();
                if state == "x" || state == "X" {
                    checked += 1;
                }
            } else if is_heading && heading_re.is_match(&l) {
                // Count child headings with TODO/DONE keywords
                if trimmed.contains("TODO ") || trimmed.contains("DONE ") {
                    total += 1;
                    if trimmed.contains("DONE ") {
                        checked += 1;
                    }
                }
            }
        }

        // Build new line with updated cookie
        let mut new_line = parent_line.clone();
        if has_frac {
            new_line = cookie_frac
                .replace(&new_line, format!("[{}/{}]", checked, total).as_str())
                .to_string();
        }
        if has_pct {
            let pct = (checked * 100).checked_div(total).unwrap_or(0);
            new_line = cookie_pct
                .replace(&new_line, format!("[{}%]", pct).as_str())
                .to_string();
        }

        if new_line != parent_line {
            let new_line = format!("{}\n", new_line);
            let line_start = self.buffers[buf_idx].rope().line_to_char(parent);
            let line_end = if parent + 1 < self.buffers[buf_idx].rope().len_lines() {
                self.buffers[buf_idx].rope().line_to_char(parent + 1)
            } else {
                self.buffers[buf_idx].rope().len_chars()
            };
            self.buffers[buf_idx].delete_range(line_start, line_end);
            self.buffers[buf_idx].insert_text_at(line_start, &new_line);

            // Recurse upward (depth limited by call stack, max ~10 heading levels)
            self.update_statistics_cookies(parent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::Language;

    fn org_editor(text: &str) -> Editor {
        let mut ed = Editor::new();
        ed.buffers[0].insert_text_at(0, text);
        ed.syntax.set_language(0, Language::Org);
        ed
    }

    fn org_editor_with_headings() -> Editor {
        let text = "* H1\nbody1\n** H2a\nbody2a\n*** H3\nbody3\n** H2b\nbody2b\n* H1b\nbody1b\n";
        let mut ed = Editor::new();
        let idx = ed.active_buffer_idx();
        ed.buffers[idx].insert_text_at(0, text);
        ed.syntax.set_language(idx, Language::Org);
        ed
    }

    // --- Narrow/widen tests ---

    #[test]
    fn narrow_to_subtree_hides_outer_lines() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.narrow_to_subtree();
        let range = ed.buffers[0].narrowed_range;
        assert_eq!(range, Some((0, 2)));
        // Lines outside range are not visible
        assert!(ed.buffers[0].is_line_visible(0));
        assert!(ed.buffers[0].is_line_visible(1));
        assert!(!ed.buffers[0].is_line_visible(2));
        assert!(!ed.buffers[0].is_line_visible(3));
    }

    #[test]
    fn widen_restores_full_buffer() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.narrow_to_subtree();
        assert!(ed.buffers[0].narrowed_range.is_some());
        ed.widen();
        assert!(ed.buffers[0].narrowed_range.is_none());
        assert!(ed.buffers[0].is_line_visible(3));
    }

    #[test]
    fn narrow_clamps_cursor() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 3;
        // Narrow to H1 subtree (rows 0-1), cursor at row 3 should clamp
        ed.buffers[0].narrow_to(0, 2);
        let win = ed.window_mgr.focused_window_mut();
        win.clamp_cursor(&ed.buffers[0]);
        assert!(win.cursor_row <= 1);
    }

    #[test]
    fn narrow_status_indicator() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.narrow_to_subtree();
        assert!(ed.status_msg.contains("Narrowed"));
    }

    // --- Global fold cycle tests ---

    #[test]
    fn global_cycle_to_overview() {
        let mut ed = org_editor_with_headings();
        // State 0 → 1 (OVERVIEW): all headings folded
        ed.heading_global_cycle(Language::Org);
        assert_eq!(ed.buffers[0].global_fold_state, 1);
        assert!(!ed.buffers[0].folded_ranges.is_empty());
        // Every heading with a body should be folded
        assert!(ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0)); // H1
        assert!(ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 8)); // H1b
    }

    #[test]
    fn global_cycle_to_contents() {
        let mut ed = org_editor_with_headings();
        // Cycle twice: 0 → 1 → 2 (CONTENTS)
        ed.heading_global_cycle(Language::Org);
        ed.heading_global_cycle(Language::Org);
        assert_eq!(ed.buffers[0].global_fold_state, 2);
        // Level 3+ headings should be folded
        let has_l3_fold = ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 4);
        assert!(has_l3_fold, "Level 3 heading should be folded");
        // Level 1/2 headings should NOT be folded
        let has_l1_fold = ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0);
        assert!(
            !has_l1_fold,
            "Level 1 heading should not be folded in CONTENTS"
        );
    }

    #[test]
    fn global_cycle_to_show_all() {
        let mut ed = org_editor_with_headings();
        // Cycle three times: 0 → 1 → 2 → 0 (SHOW ALL)
        ed.heading_global_cycle(Language::Org);
        ed.heading_global_cycle(Language::Org);
        ed.heading_global_cycle(Language::Org);
        assert_eq!(ed.buffers[0].global_fold_state, 0);
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn global_cycle_round_trip() {
        let mut ed = org_editor_with_headings();
        // Full cycle: 0 → 1 → 2 → 0 → 1
        for _ in 0..3 {
            ed.heading_global_cycle(Language::Org);
        }
        assert_eq!(ed.buffers[0].global_fold_state, 0);
        ed.heading_global_cycle(Language::Org);
        assert_eq!(ed.buffers[0].global_fold_state, 1);
    }

    // --- Checkbox and statistics cookie tests ---

    #[test]
    fn toggle_checkbox_checks() {
        let mut ed = org_editor("- [ ] task\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.toggle_checkbox_at_cursor();
        assert!(ed.buffers[0].text().contains("[x]"));
    }

    #[test]
    fn toggle_checkbox_unchecks() {
        let mut ed = org_editor("- [x] task\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.toggle_checkbox_at_cursor();
        assert!(ed.buffers[0].text().contains("[ ]"));
    }

    #[test]
    fn statistics_cookie_fraction_updates() {
        let mut ed = org_editor("* Parent [0/2]\n- [ ] a\n- [ ] b\n");
        ed.window_mgr.focused_window_mut().cursor_row = 1;
        ed.toggle_checkbox_at_cursor();
        assert!(ed.buffers[0].text().contains("[1/2]"));
    }

    #[test]
    fn statistics_cookie_percent_updates() {
        let mut ed = org_editor("* Parent [0%]\n- [ ] a\n- [ ] b\n");
        ed.window_mgr.focused_window_mut().cursor_row = 1;
        ed.toggle_checkbox_at_cursor();
        assert!(ed.buffers[0].text().contains("[50%]"));
    }
}
