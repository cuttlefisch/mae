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
