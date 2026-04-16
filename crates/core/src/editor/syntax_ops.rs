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
        self.mode = Mode::Visual(VisualType::Char);
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
}
