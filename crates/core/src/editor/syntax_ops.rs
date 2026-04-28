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

    /// Toggle folding for the Org heading at the cursor.
    pub fn org_cycle(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let lang = self.syntax.language_of(buf_idx);
        if lang != Some(crate::syntax::Language::Org) {
            return;
        }

        info!(buf_idx, "org cycling at cursor");

        let win = self.window_mgr.focused_window();
        let cursor_char = self.buffers[buf_idx].char_offset_at(win.cursor_row, win.cursor_col);
        let cursor_byte = self.buffers[buf_idx].rope().char_to_byte(cursor_char);

        let source: String = self.buffers[buf_idx].rope().chars().collect();
        let Some(tree) = self.syntax.tree_for(buf_idx, &source) else {
            return;
        };

        // Find the heading node at or above the cursor
        let mut node = tree
            .root_node()
            .descendant_for_byte_range(cursor_byte, cursor_byte);

        while let Some(n) = node {
            if n.kind() == "headline" {
                break;
            }
            node = n.parent();
        }

        let Some(headline) = node else {
            return;
        };

        let start_row = self.buffers[buf_idx]
            .rope()
            .byte_to_line(headline.start_byte());
        let end_row = self.buffers[buf_idx]
            .rope()
            .byte_to_line(headline.end_byte());

        if start_row >= end_row {
            return;
        }

        // Cycle logic: Unfolded -> Children -> Folded -> Unfolded
        // For now, just implement simple Fold/Unfold toggle
        let is_folded = self.buffers[buf_idx]
            .folded_ranges
            .iter()
            .any(|(s, _)| *s == start_row);

        if is_folded {
            self.buffers[buf_idx]
                .folded_ranges
                .retain(|(s, _)| *s != start_row);
            self.set_status("Unfolded");
        } else {
            self.buffers[buf_idx]
                .folded_ranges
                .push((start_row, end_row));
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

    /// Promote the Org heading at the cursor (remove one `*`). Noop if
    /// not on a heading or already a single-star heading.
    pub fn org_promote(&mut self) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Org) {
            return;
        }
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let star_count = line.chars().take_while(|&c| c == '*').count();
        if star_count <= 1 {
            return; // single star or not a heading
        }
        // Remove first star
        let start = self.buffers[buf_idx].rope().line_to_char(row);
        self.buffers[buf_idx].delete_range(start, start + 1);
        self.set_status(format!("Promoted to level {}", star_count - 1));
    }

    /// Demote the Org heading at the cursor (add one `*`). Noop if not on a heading.
    pub fn org_demote(&mut self) {
        let buf_idx = self.active_buffer_idx();
        if self.syntax.language_of(buf_idx) != Some(crate::syntax::Language::Org) {
            return;
        }
        let row = self.window_mgr.focused_window().cursor_row;
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let star_count = line.chars().take_while(|&c| c == '*').count();
        if star_count == 0 {
            return; // not a heading
        }
        let start = self.buffers[buf_idx].rope().line_to_char(row);
        self.buffers[buf_idx].insert_text_at(start, "*");
        self.set_status(format!("Demoted to level {}", star_count + 1));
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
        let line_count = self.buffers[buf_idx].line_count();
        let line: String = self.buffers[buf_idx].rope().line(row).chars().collect();
        let level = line.chars().take_while(|&c| c == '*').count();
        if level == 0 {
            return None;
        }
        let mut end = row + 1;
        while end < line_count {
            let l: String = self.buffers[buf_idx].rope().line(end).chars().collect();
            let l_level = l.chars().take_while(|&c| c == '*').count();
            if l_level > 0 && l_level <= level {
                break;
            }
            end += 1;
        }
        Some((row, end))
    }

    /// Move the subtree at the cursor down past the next sibling subtree.
    pub fn org_move_subtree_down(&mut self) {
        let row = self.window_mgr.focused_window().cursor_row;
        let Some((start, end)) = self.org_subtree_range(row) else {
            return;
        };
        let buf_idx = self.active_buffer_idx();
        let line_count = self.buffers[buf_idx].line_count();
        if end >= line_count {
            return; // already at bottom
        }
        // Find the sibling subtree below
        let sibling_start = end;
        let sibling_range = self.org_subtree_range(sibling_start);
        let sibling_end = match sibling_range {
            Some((_, se)) => se,
            None => {
                // Next line is not a heading — treat as single line
                sibling_start + 1
            }
        };

        // Extract both blocks as text
        let rope = self.buffers[buf_idx].rope();
        let our_char_start = rope.line_to_char(start);
        let our_char_end = rope.line_to_char(end);
        let sib_char_start = rope.line_to_char(sibling_start);
        let sib_char_end = if sibling_end >= line_count {
            rope.len_chars()
        } else {
            rope.line_to_char(sibling_end)
        };

        let our_text: String = rope.slice(our_char_start..our_char_end).chars().collect();
        let sib_text: String = rope.slice(sib_char_start..sib_char_end).chars().collect();

        // Replace: sibling first, then our block
        self.buffers[buf_idx].delete_range(our_char_start, sib_char_end);
        let combined = format!("{}{}", sib_text, our_text);
        self.buffers[buf_idx].insert_text_at(our_char_start, &combined);

        // Move cursor: count newlines in sibling text to find offset
        let sib_lines = sib_text.chars().filter(|&c| c == '\n').count();
        self.window_mgr.focused_window_mut().cursor_row = start + sib_lines;
        self.set_status("Moved subtree down");
    }

    /// Move the subtree at the cursor up past the previous sibling subtree.
    pub fn org_move_subtree_up(&mut self) {
        let row = self.window_mgr.focused_window().cursor_row;
        let Some((start, end)) = self.org_subtree_range(row) else {
            return;
        };
        if start == 0 {
            return; // already at top
        }
        let buf_idx = self.active_buffer_idx();

        // Find the sibling above: scan upward for a heading at same or higher level
        let line: String = self.buffers[buf_idx].rope().line(start).chars().collect();
        let level = line.chars().take_while(|&c| c == '*').count();

        let mut prev_start = start - 1;
        loop {
            let l: String = self.buffers[buf_idx]
                .rope()
                .line(prev_start)
                .chars()
                .collect();
            let l_level = l.chars().take_while(|&c| c == '*').count();
            if l_level > 0 && l_level <= level {
                break;
            }
            if prev_start == 0 {
                return; // no sibling above
            }
            prev_start -= 1;
        }

        // Extract both blocks as text
        let rope = self.buffers[buf_idx].rope();
        let prev_char_start = rope.line_to_char(prev_start);
        let prev_char_end = rope.line_to_char(start);
        let our_char_start = rope.line_to_char(start);
        let our_char_end = if end >= self.buffers[buf_idx].line_count() {
            rope.len_chars()
        } else {
            rope.line_to_char(end)
        };

        let prev_text: String = rope.slice(prev_char_start..prev_char_end).chars().collect();
        let our_text: String = rope.slice(our_char_start..our_char_end).chars().collect();

        // Replace: our block first, then previous
        self.buffers[buf_idx].delete_range(prev_char_start, our_char_end);
        let combined = format!("{}{}", our_text, prev_text);
        self.buffers[buf_idx].insert_text_at(prev_char_start, &combined);

        // Move cursor to new position
        self.window_mgr.focused_window_mut().cursor_row = prev_start;
        self.set_status("Moved subtree up");
    }

    /// Toggle fold at cursor (za). Works for both org headings and code blocks.
    pub fn toggle_fold(&mut self) {
        let buf_idx = self.active_buffer_idx();
        let cursor_row = self.window_mgr.focused_window().cursor_row;
        let source: String = self.buffers[buf_idx].rope().chars().collect();

        // For org buffers, delegate to org_cycle
        if self.syntax.language_of(buf_idx) == Some(crate::syntax::Language::Org) {
            self.org_cycle();
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

    /// Close all folds (zM). Folds all tree-sitter/org fold points.
    pub fn close_all_folds(&mut self) {
        let buf_idx = self.active_buffer_idx();
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
}
