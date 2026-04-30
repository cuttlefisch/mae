//! Tree-sitter driven structural editing operations.
//!
//! Three commands:
//! - `syntax_select_node` — enter charwise Visual mode with the selection
//!   set to the smallest named node covering the cursor.
//! - `syntax_expand_selection` — grow the current Visual selection to the
//!   parent node. Records each prior range on a stack.
//! - `syntax_contract_selection` — pop the stack and restore the previous
//!   Visual selection.
//!
//! Selections are stored as `(anchor_row, anchor_col)` + window cursor,
//! so byte ranges from tree-sitter are converted through char offsets.

use crate::{Mode, VisualType};

use super::Editor;
use tracing::info;

impl Editor {
    /// Enter charwise Visual mode selecting the tree-sitter node at the cursor.
    /// Returns `true` if a node was found and selected.
    pub fn syntax_select_node(&mut self) -> bool {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx).is_none() {
            self.set_status("No language attached to this buffer");
            return false;
        }
        let win = self.window_mgr.focused_window();
        let cursor_char = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        let cursor_byte = self.buffers[buf_idx].rope().char_to_byte(cursor_char);

        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let Some(tree) = self.syntax.tree_for(buf_idx, &source) else {
            self.set_status("Failed to parse buffer");
            return false;
        };

        // Try to grab a node at exactly the cursor byte; fall back to a
        // zero-width range so cursors on whitespace still resolve.
        let node = tree
            .root_node()
            .named_descendant_for_byte_range(cursor_byte, cursor_byte.saturating_add(1))
            .or_else(|| {
                tree.root_node()
                    .named_descendant_for_byte_range(cursor_byte, cursor_byte)
            });
        let Some(node) = node else {
            self.set_status("No node at cursor");
            return false;
        };
        // Extract owned data before releasing the borrow on self.syntax.
        let start_byte = node.start_byte();
        let end_byte = node.end_byte();
        let kind = node.kind().to_string();

        self.syntax_selection_stack.clear();
        self.set_visual_from_byte_range(start_byte, end_byte);
        self.set_status(format!("Selected: {}", kind));
        true
    }

    /// Expand the current Visual selection to the parent tree-sitter node.
    /// When not already in Visual mode, behaves like `syntax_select_node`.
    pub fn syntax_expand_selection(&mut self) -> bool {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx).is_none() {
            self.set_status("No language attached to this buffer");
            return false;
        }

        if !matches!(self.mode, Mode::Visual(_)) {
            return self.syntax_select_node();
        }

        let current_range = self.visual_selection_range();
        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let Some(tree) = self.syntax.tree_for(buf_idx, &source) else {
            self.set_status("Failed to parse buffer");
            return false;
        };

        let rope = self.buffers[buf_idx].rope();
        let byte_start = rope.char_to_byte(current_range.0);
        let byte_end = rope.char_to_byte(current_range.1);

        // Smallest named node that covers the current selection.
        let Some(mut node) = tree
            .root_node()
            .named_descendant_for_byte_range(byte_start, byte_end)
        else {
            return false;
        };

        // If the node matches the selection exactly, walk to the parent.
        if node.start_byte() == byte_start && node.end_byte() == byte_end {
            match node.parent() {
                Some(p) => node = p,
                None => {
                    self.set_status("Already at root node");
                    return false;
                }
            }
        }

        let new_start = node.start_byte();
        let new_end = node.end_byte();
        let kind = node.kind().to_string();

        self.syntax_selection_stack.push(current_range);
        self.set_visual_from_byte_range(new_start, new_end);
        self.set_status(format!("Expanded: {}", kind));
        true
    }

    /// Pop the syntax-selection stack and restore the previous Visual range.
    pub fn syntax_contract_selection(&mut self) -> bool {
        let Some((start, end)) = self.syntax_selection_stack.pop() else {
            self.set_status("No prior selection");
            return false;
        };
        let buf_idx = self.active_buffer_idx();
        let rope = self.buffers[buf_idx].rope();
        let byte_start = rope.char_to_byte(start);
        let byte_end = rope.char_to_byte(end);
        self.set_visual_from_byte_range(byte_start, byte_end);
        self.set_status("Contracted selection");
        true
    }

    /// Set Visual mode with a charwise selection covering the given byte range.
    /// Selection is inclusive of the cursor character (matching the existing
    /// `visual_selection_range` convention: `[anchor, cursor+1)`).
    fn set_visual_from_byte_range(&mut self, byte_start: usize, byte_end: usize) {
        let buf_idx = self.active_buffer_idx();
        let rope = self.buffers[buf_idx].rope();
        let char_start = rope.byte_to_char(byte_start);
        // Inclusive-end cursor position; byte_end is tree-sitter exclusive.
        let char_end_excl = rope.byte_to_char(byte_end).min(rope.len_chars());
        let char_cursor = char_end_excl.saturating_sub(1).max(char_start);

        let anchor_row = rope.char_to_line(char_start);
        let anchor_col = char_start - rope.line_to_char(anchor_row);
        let cursor_row = rope.char_to_line(char_cursor);
        let cursor_col = char_cursor - rope.line_to_char(cursor_row);

        self.visual_anchor_row = anchor_row;
        self.visual_anchor_col = anchor_col;
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = cursor_row;
        win.cursor_col = cursor_col;
        self.set_mode(Mode::Visual(VisualType::Char));
    }

    /// Return the full S-expression of the buffer's parsed tree.
    pub fn syntax_tree_sexp(&mut self) -> Option<String> {
        let buf_idx = self.active_buffer_idx();
        self.syntax.language_of(buf_idx)?;
        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let tree = self.syntax.tree_for(buf_idx, &source)?;
        Some(tree.root_node().to_sexp())
    }

    /// Return the smallest named node kind at the cursor, if any.
    pub fn syntax_node_kind_at_cursor(&mut self) -> Option<String> {
        let buf_idx = self.active_buffer_idx();
        self.syntax.language_of(buf_idx)?;
        let win = self.window_mgr.focused_window();
        let cursor_char = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        let cursor_byte = self.buffers[buf_idx].rope().char_to_byte(cursor_char);
        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let tree = self.syntax.tree_for(buf_idx, &source)?;
        tree.root_node()
            .named_descendant_for_byte_range(cursor_byte, cursor_byte.saturating_add(1))
            .or_else(|| {
                tree.root_node()
                    .named_descendant_for_byte_range(cursor_byte, cursor_byte)
            })
            .map(|n| n.kind().to_string())
    }

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

    /// Three-state org heading cycle (TAB).
    ///
    /// Cycle: SUBTREE (all visible) → FOLDED (heading only) → CHILDREN
    /// (body + child headings visible, child bodies folded) → SUBTREE.
    /// Leaf headings (no children) cycle: SUBTREE ↔ FOLDED.
    pub fn org_cycle(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let lang = self.syntax.language_of(buf_idx);
        if lang != Some(crate::syntax::Language::Org) {
            return;
        }
        self.heading_cycle(crate::syntax::Language::Org);
    }

    /// Generic three-state heading cycle for org and markdown.
    pub fn heading_cycle(&mut self, lang: crate::syntax::Language) {
        let buf_idx = self.active_buffer_idx();
        let row = self.window_mgr.focused_window().cursor_row;

        info!(buf_idx, row, "heading cycle");

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

    /// Cycle TODO state for the Org heading at the cursor.
    pub fn org_todo_cycle(&mut self, forward: bool) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Org) {
            return;
        }

        info!(buf_idx, forward, "org todo cycle");
        // Trivial string replacement logic for now
        let row = self.window_mgr.focused_window().cursor_row;
        let line = self.buffers[buf_idx].rope().line(row);
        let line_str: String = line.chars().collect();

        let (new_line, status) = if line_str.contains("TODO ") {
            if forward {
                (line_str.replace("TODO ", "DONE "), "DONE")
            } else {
                (line_str.replace("TODO ", ""), "None")
            }
        } else if line_str.contains("DONE ") {
            if forward {
                (line_str.replace("DONE ", ""), "None")
            } else {
                (line_str.replace("DONE ", "TODO "), "TODO")
            }
        } else {
            // Find the start of the heading text (after stars)
            let mut stars_found = false;
            let mut stars_end = 0;
            for (i, ch) in line_str.chars().enumerate() {
                if ch == '*' {
                    stars_found = true;
                } else if stars_found && ch == ' ' {
                    stars_end = i + 1;
                    break;
                } else if stars_found {
                    // unexpected char after stars
                    break;
                } else {
                    break;
                }
            }

            if stars_found && stars_end > 0 {
                let mut next = line_str.clone();
                if forward {
                    next.insert_str(stars_end, "TODO ");
                    (next, "TODO")
                } else {
                    next.insert_str(stars_end, "DONE ");
                    (next, "DONE")
                }
            } else {
                (line_str.clone(), "Not a heading")
            }
        };

        if new_line != line_str {
            let start = self.buffers[buf_idx].rope().line_to_char(row);
            let end = start + line.len_chars();
            self.buffers[buf_idx].delete_range(start, end);
            self.buffers[buf_idx].insert_text_at(start, &new_line);
            self.set_status(status);
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

    /// Promote Org heading (thin wrapper).
    pub fn org_promote(&mut self) {
        self.heading_promote(crate::syntax::Language::Org);
    }

    /// Demote Org heading (thin wrapper).
    pub fn org_demote(&mut self) {
        self.heading_demote(crate::syntax::Language::Org);
    }

    /// Calculate the range of lines covered by the subtree rooted at the
    /// heading on `row`. Returns `(start_row, end_row_exclusive)` where
    /// `start_row` is the heading itself and `end_row_exclusive` is the
    /// first line of the next sibling (same or higher level) or EOF.
    pub fn org_subtree_range(&self, row: usize) -> Option<(usize, usize)> {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Org) {
            return None;
        }
        self.heading_subtree_range(row, crate::syntax::Language::Org)
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

    /// Move org subtree down (thin wrapper).
    pub fn org_move_subtree_down(&mut self) {
        self.heading_move_subtree_down(crate::syntax::Language::Org);
    }

    /// Move org subtree up (thin wrapper).
    pub fn org_move_subtree_up(&mut self) {
        self.heading_move_subtree_up(crate::syntax::Language::Org);
    }

    // --- Markdown structural editing ---

    /// Three-state heading cycle for markdown buffers (TAB).
    pub fn md_cycle(&mut self) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Markdown) {
            return;
        }
        self.heading_cycle(crate::syntax::Language::Markdown);
    }

    /// Promote markdown heading (thin wrapper).
    pub fn md_promote(&mut self) {
        self.heading_promote(crate::syntax::Language::Markdown);
    }

    /// Demote markdown heading (thin wrapper).
    pub fn md_demote(&mut self) {
        self.heading_demote(crate::syntax::Language::Markdown);
    }

    /// Move markdown subtree down (thin wrapper).
    pub fn md_move_subtree_down(&mut self) {
        self.heading_move_subtree_down(crate::syntax::Language::Markdown);
    }

    /// Move markdown subtree up (thin wrapper).
    pub fn md_move_subtree_up(&mut self) {
        self.heading_move_subtree_up(crate::syntax::Language::Markdown);
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

    /// Open the Org link at the cursor.
    pub fn org_open_link(&mut self) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Org) {
            return;
        }

        info!(buf_idx, "org open link at cursor");

        let win = self.window_mgr.focused_window();
        let cursor_char = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        let cursor_byte = self.buffers[buf_idx].rope().char_to_byte(cursor_char);

        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let Some(tree) = self.syntax.tree_for(buf_idx, &source) else {
            return;
        };

        let mut node = tree
            .root_node()
            .descendant_for_byte_range(cursor_byte, cursor_byte);

        // Org links often have nested nodes, walk up to find the link node.
        while let Some(n) = node {
            if n.kind() == "link" {
                break;
            }
            node = n.parent();
        }

        let Some(link) = node else {
            return;
        };

        // Extract target from [[target][label]] or [[target]]
        let link_text = &source[link.start_byte()..link.end_byte()];
        if let Some(target) = link_text
            .strip_prefix("[[")
            .and_then(|s| s.split(']').next())
        {
            let target = target.split('|').next().unwrap_or(target).trim();
            if target.starts_with("http") {
                // Open external link
                let _ = std::process::Command::new("xdg-open").arg(target).spawn();
                self.set_status(format!("Opening {}", target));
            } else {
                // Jump to internal heading
                self.set_status(format!("Jumping to heading: {}", target));
                // TODO: implement actual search/jump logic
            }
        }
    }

    /// Global heading fold cycle (Doom Emacs S-TAB pattern).
    ///
    /// Three states: SHOW ALL (0) → OVERVIEW (1) → CONTENTS (2) → SHOW ALL.
    /// - SHOW ALL: clear all heading folds
    /// - OVERVIEW: fold every heading (all levels)
    /// - CONTENTS: show level 1 + 2 headings, fold level 3+
    pub fn heading_global_cycle(&mut self, lang: crate::syntax::Language) {
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

    #[test]
    fn org_demote_adds_star() {
        let mut ed = org_editor("* Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_demote();
        assert_eq!(ed.buffers[0].text(), "** Heading\nBody\n");
        assert!(ed.status_msg.contains("level 2"));
    }

    #[test]
    fn org_promote_removes_star() {
        let mut ed = org_editor("** Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_promote();
        assert_eq!(ed.buffers[0].text(), "* Heading\nBody\n");
        assert!(ed.status_msg.contains("level 1"));
    }

    #[test]
    fn org_promote_single_star_noop() {
        let mut ed = org_editor("* Heading\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_promote();
        assert_eq!(ed.buffers[0].text(), "* Heading\n");
    }

    #[test]
    fn dedent_line_dispatches_org_promote() {
        let mut ed = org_editor("** Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.dispatch_builtin("dedent-line");
        assert_eq!(ed.buffers[0].text(), "* Heading\nBody\n");
    }

    #[test]
    fn indent_line_dispatches_org_demote() {
        let mut ed = org_editor("* Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.dispatch_builtin("indent-line");
        assert_eq!(ed.buffers[0].text(), "** Heading\nBody\n");
    }

    #[test]
    fn org_demote_non_heading_noop() {
        let mut ed = org_editor("Just text\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_demote();
        assert_eq!(ed.buffers[0].text(), "Just text\n");
    }

    #[test]
    fn org_subtree_range_single() {
        let ed = org_editor("* H1\nBody\n* H2\n");
        let range = ed.org_subtree_range(0);
        assert_eq!(range, Some((0, 2)));
    }

    #[test]
    fn org_subtree_range_nested() {
        let ed = org_editor("* H1\n** Sub\nBody\n* H2\n");
        let range = ed.org_subtree_range(0);
        assert_eq!(range, Some((0, 3)));
        let range = ed.org_subtree_range(1);
        assert_eq!(range, Some((1, 3)));
    }

    #[test]
    fn org_move_subtree_down() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_move_subtree_down();
        assert_eq!(ed.buffers[0].text(), "* H2\nBody2\n* H1\nBody1\n");
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 2);
    }

    #[test]
    fn org_move_subtree_up() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 2;
        ed.org_move_subtree_up();
        assert_eq!(ed.buffers[0].text(), "* H2\nBody2\n* H1\nBody1\n");
        assert_eq!(ed.window_mgr.focused_window().cursor_row, 0);
    }

    #[test]
    fn org_move_at_boundary_noop() {
        let mut ed = org_editor("* H1\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_move_subtree_down();
        assert_eq!(ed.buffers[0].text(), "* H1\nBody\n");
        ed.org_move_subtree_up();
        assert_eq!(ed.buffers[0].text(), "* H1\nBody\n");
    }

    // --- Three-state org heading cycle tests ---

    #[test]
    fn org_cycle_subtree_to_folded() {
        // TAB on unfolded heading folds entire subtree
        let mut ed = org_editor("* H1\nBody\n** Sub\nSub body\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_cycle();
        assert!(
            ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0),
            "Expected fold at row 0"
        );
        assert!(ed.status_msg.contains("Folded"));
    }

    #[test]
    fn org_cycle_folded_to_children() {
        // TAB on folded heading shows children but folds their bodies
        let mut ed = org_editor("* H1\nBody\n** Sub\nSub body\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        // First TAB: SUBTREE → FOLDED
        ed.org_cycle();
        assert!(ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0));
        // Second TAB: FOLDED → CHILDREN
        ed.org_cycle();
        assert!(
            !ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0),
            "Heading 0 should not be folded in CHILDREN state"
        );
        // Child heading at row 2 should be folded
        assert!(
            ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 2),
            "Child heading at row 2 should be folded"
        );
        assert!(ed.status_msg.contains("Children"));
    }

    #[test]
    fn org_cycle_children_to_subtree() {
        // TAB on children-visible heading unfolds all
        let mut ed = org_editor("* H1\nBody\n** Sub\nSub body\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_cycle(); // SUBTREE → FOLDED
        ed.org_cycle(); // FOLDED → CHILDREN
        ed.org_cycle(); // CHILDREN → SUBTREE
        assert!(
            ed.buffers[0].folded_ranges.is_empty(),
            "All folds should be cleared in SUBTREE state"
        );
        assert!(ed.status_msg.contains("Subtree"));
    }

    #[test]
    fn org_cycle_full_round_trip() {
        // 3 TABs return to original state (SUBTREE)
        let mut ed = org_editor("* H1\nBody\n** Sub\nSub body\n* H2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        assert!(ed.buffers[0].folded_ranges.is_empty());
        ed.org_cycle(); // → FOLDED
        ed.org_cycle(); // → CHILDREN
        ed.org_cycle(); // → SUBTREE
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn org_cycle_leaf_heading_two_state() {
        // Heading with no children only toggles fold/unfold
        let mut ed = org_editor("* H1\nBody line 1\nBody line 2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_cycle(); // → FOLDED
        assert!(!ed.buffers[0].folded_ranges.is_empty());
        ed.org_cycle(); // → UNFOLDED
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn org_cycle_nested_children() {
        // Grandchildren stay folded in CHILDREN state
        let mut ed = org_editor("* H1\n** Sub1\n*** Deep\nDeep body\n** Sub2\nSub2 body\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.org_cycle(); // → FOLDED
        ed.org_cycle(); // → CHILDREN
                        // ** Sub1 (row 1) should be folded (has content below)
        assert!(
            ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 1),
            "Sub1 should be folded in CHILDREN state"
        );
        // ** Sub2 (row 4) should be folded
        assert!(
            ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 4),
            "Sub2 should be folded in CHILDREN state"
        );
    }

    // --- Fold-aware structural editing tests (Item 5) ---

    #[test]
    fn org_move_subtree_down_clears_folds() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        // Fold H1
        ed.buffers[0].folded_ranges.push((0, 2));
        ed.org_move_subtree_down();
        // Folds in affected range should be cleared
        assert!(
            ed.buffers[0].folded_ranges.is_empty(),
            "Folds should be cleared after move: {:?}",
            ed.buffers[0].folded_ranges
        );
    }

    #[test]
    fn org_move_subtree_up_clears_folds() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 2;
        // Fold H2
        ed.buffers[0].folded_ranges.push((2, 4));
        ed.org_move_subtree_up();
        assert!(
            ed.buffers[0].folded_ranges.is_empty(),
            "Folds should be cleared after move up"
        );
    }

    #[test]
    fn org_promote_preserves_folds() {
        let mut ed = org_editor("** Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.buffers[0].folded_ranges.push((0, 2));
        ed.org_promote();
        assert_eq!(
            ed.buffers[0].folded_ranges.len(),
            1,
            "Promote should preserve folds"
        );
    }

    #[test]
    fn org_demote_preserves_folds() {
        let mut ed = org_editor("* Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.buffers[0].folded_ranges.push((0, 2));
        ed.org_demote();
        assert_eq!(
            ed.buffers[0].folded_ranges.len(),
            1,
            "Demote should preserve folds"
        );
    }

    // --- heading_level helper tests ---

    #[test]
    fn heading_level_org() {
        assert_eq!(Editor::heading_level("* H1", Language::Org), 1);
        assert_eq!(Editor::heading_level("** H2", Language::Org), 2);
        assert_eq!(Editor::heading_level("*** H3", Language::Org), 3);
        assert_eq!(Editor::heading_level("Not a heading", Language::Org), 0);
        assert_eq!(Editor::heading_level("**nospace", Language::Org), 0);
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

    #[test]
    fn heading_scale_option_toggle() {
        let mut ed = Editor::new();
        assert!(ed.heading_scale); // default on
        assert!(ed.set_option("heading_scale", "false").is_ok());
        assert!(!ed.heading_scale);
        assert!(ed.set_option("heading-scale", "true").is_ok());
        assert!(ed.heading_scale);
    }

    // --- Markdown structural editing tests ---

    fn md_editor(text: &str) -> Editor {
        let mut ed = Editor::new();
        ed.buffers[0].insert_text_at(0, text);
        ed.syntax.set_language(0, Language::Markdown);
        ed
    }

    #[test]
    fn heading_level_markdown() {
        assert_eq!(Editor::heading_level("# H1", Language::Markdown), 1);
        assert_eq!(Editor::heading_level("## H2", Language::Markdown), 2);
        assert_eq!(Editor::heading_level("### H3", Language::Markdown), 3);
        assert_eq!(
            Editor::heading_level("Not a heading", Language::Markdown),
            0
        );
        assert_eq!(Editor::heading_level("##nospace", Language::Markdown), 0);
    }

    #[test]
    fn md_promote_removes_hash() {
        let mut ed = md_editor("## Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.md_promote();
        assert_eq!(ed.buffers[0].text(), "# Heading\nBody\n");
    }

    #[test]
    fn md_demote_adds_hash() {
        let mut ed = md_editor("# Heading\nBody\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.md_demote();
        assert_eq!(ed.buffers[0].text(), "## Heading\nBody\n");
    }

    #[test]
    fn md_subtree_range() {
        let ed = md_editor("# H1\nBody\n## Sub\nSub body\n# H2\n");
        let range = ed.heading_subtree_range(0, Language::Markdown);
        assert_eq!(range, Some((0, 4)));
        let range = ed.heading_subtree_range(2, Language::Markdown);
        assert_eq!(range, Some((2, 4)));
    }

    #[test]
    fn md_cycle_three_state() {
        let mut ed = md_editor("# H1\nBody\n## Sub\nSub body\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        // SUBTREE → FOLDED
        ed.md_cycle();
        assert!(ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0));
        // FOLDED → CHILDREN
        ed.md_cycle();
        assert!(!ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 0));
        assert!(ed.buffers[0].folded_ranges.iter().any(|(s, _)| *s == 2));
        // CHILDREN → SUBTREE
        ed.md_cycle();
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn md_move_subtree_down() {
        let mut ed = md_editor("# H1\nBody1\n# H2\nBody2\n");
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.md_move_subtree_down();
        assert_eq!(ed.buffers[0].text(), "# H2\nBody2\n# H1\nBody1\n");
    }

    // --- zM/zR for org and markdown headings ---

    #[test]
    fn org_close_all_folds() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.close_all_folds();
        assert!(!ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn org_open_all_folds_clears() {
        let mut ed = org_editor("* H1\nBody1\n* H2\nBody2\n");
        ed.close_all_folds();
        ed.open_all_folds();
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn md_close_all_folds() {
        let mut ed = md_editor("# H1\nBody1\n## H2\nBody2\n");
        ed.close_all_folds();
        assert!(!ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn md_open_all_folds() {
        let mut ed = md_editor("# H1\nBody1\n## H2\nBody2\n");
        ed.close_all_folds();
        ed.open_all_folds();
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    fn rust_editor(text: &str) -> Editor {
        let mut ed = Editor::new();
        ed.buffers[0].insert_text_at(0, text);
        ed.syntax.set_language(0, Language::Rust);
        ed
    }

    #[test]
    fn toggle_fold_on_rust_function() {
        let code = "fn main() {\n    println!(\"hello\");\n    let x = 1;\n}\n";
        let mut ed = rust_editor(code);
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.toggle_fold();
        // After toggling, there should be a fold range starting at line 0
        assert!(
            !ed.buffers[0].folded_ranges.is_empty(),
            "Expected fold range"
        );
        // Toggle again to unfold
        ed.toggle_fold();
        assert!(
            ed.buffers[0].folded_ranges.is_empty(),
            "Expected no folds after second toggle"
        );
    }

    #[test]
    fn close_all_folds_rust() {
        let code = "fn foo() {\n    1\n}\nfn bar() {\n    2\n}\n";
        let mut ed = rust_editor(code);
        ed.close_all_folds();
        assert!(
            !ed.buffers[0].folded_ranges.is_empty(),
            "Expected at least one fold"
        );
    }

    #[test]
    fn open_all_folds() {
        let code = "fn foo() {\n    1\n}\n";
        let mut ed = rust_editor(code);
        ed.close_all_folds();
        assert!(!ed.buffers[0].folded_ranges.is_empty());
        ed.open_all_folds();
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn toggle_fold_dispatch() {
        let code = "fn main() {\n    println!(\"hello\");\n}\n";
        let mut ed = rust_editor(code);
        ed.window_mgr.focused_window_mut().cursor_row = 0;
        ed.dispatch_builtin("toggle-fold");
        assert!(!ed.buffers[0].folded_ranges.is_empty());
        ed.dispatch_builtin("toggle-fold");
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    #[test]
    fn close_open_all_folds_dispatch() {
        let code = "fn foo() {\n    1\n}\nfn bar() {\n    2\n}\n";
        let mut ed = rust_editor(code);
        ed.dispatch_builtin("close-all-folds");
        assert!(!ed.buffers[0].folded_ranges.is_empty());
        ed.dispatch_builtin("open-all-folds");
        assert!(ed.buffers[0].folded_ranges.is_empty());
    }

    // --- Global fold cycle tests ---

    fn org_editor_with_headings() -> Editor {
        let text = "* H1\nbody1\n** H2a\nbody2a\n*** H3\nbody3\n** H2b\nbody2b\n* H1b\nbody1b\n";
        let mut ed = Editor::new();
        let idx = ed.active_buffer_idx();
        ed.buffers[idx].insert_text_at(0, text);
        ed.syntax.set_language(idx, Language::Org);
        ed
    }

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
}
