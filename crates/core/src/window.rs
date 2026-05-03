use std::collections::HashMap;

/// Unique window identifier.
pub type WindowId = u32;

/// Saved view state for a buffer that was previously displayed in this window.
#[derive(Clone, Debug)]
pub struct BufferViewState {
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub col_offset: usize,
}

/// A window is a view onto a buffer — it owns cursor state and scroll position.
///
/// Emacs lesson: Emacs got this right from day one — point (cursor) is per-window,
/// not per-buffer. Two windows can view the same buffer at different positions.
/// Neovim's win_T does the same. We follow suit.
#[derive(Clone)]
pub struct Window {
    pub id: WindowId,
    pub buffer_idx: usize,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll_offset: usize,
    pub col_offset: usize,
    /// Multi-cursor set. Index 0 = primary (synced with cursor_row/cursor_col).
    pub cursor_set: crate::cursor::CursorSet,
    /// Per-buffer saved view states for cursor/scroll preservation on buffer switch.
    pub saved_view_states: HashMap<usize, BufferViewState>,
    /// When true, skip ensure_scroll_wrapped for this frame.
    /// Set by scroll commands (C-e/C-y/zz/zt/zb), cleared when cursor moves.
    pub scroll_locked: bool,
    /// cursor_row when scroll_locked was set. Lock clears when cursor_row
    /// diverges (a non-scroll command moved the cursor).
    pub scroll_locked_cursor: usize,
}

impl Window {
    pub fn new(id: WindowId, buffer_idx: usize) -> Self {
        Window {
            id,
            buffer_idx,
            cursor_row: 0,
            cursor_col: 0,
            scroll_offset: 0,
            col_offset: 0,
            cursor_set: crate::cursor::CursorSet::new(0, 0),
            saved_view_states: HashMap::new(),
            scroll_locked: false,
            scroll_locked_cursor: 0,
        }
    }

    /// Save current cursor + scroll state for the current buffer.
    pub fn save_view_state(&mut self) {
        self.saved_view_states.insert(
            self.buffer_idx,
            BufferViewState {
                cursor_row: self.cursor_row,
                cursor_col: self.cursor_col,
                scroll_offset: self.scroll_offset,
                col_offset: self.col_offset,
            },
        );
    }

    /// Restore saved state for a buffer, or reset to (0,0) if none.
    pub fn restore_view_state(&mut self, buf_idx: usize) {
        self.buffer_idx = buf_idx;
        if let Some(state) = self.saved_view_states.get(&buf_idx) {
            self.cursor_row = state.cursor_row;
            self.cursor_col = state.cursor_col;
            self.scroll_offset = state.scroll_offset;
            self.col_offset = state.col_offset;
        } else {
            self.cursor_row = 0;
            self.cursor_col = 0;
            self.scroll_offset = 0;
            self.col_offset = 0;
        }
    }

    // --- Movement ---
    // These methods take &Buffer to query line count/length.

    pub fn move_up(&mut self, buf: &crate::buffer::Buffer) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            // Skip folded lines
            while self.cursor_row > 0 && buf.is_line_folded(self.cursor_row) {
                self.cursor_row -= 1;
            }
            self.clamp_cursor(buf);
        }
    }

    pub fn move_down(&mut self, buf: &crate::buffer::Buffer) {
        let max = buf.display_line_count();
        if self.cursor_row + 1 < max {
            self.cursor_row += 1;
            // Skip folded lines
            while self.cursor_row + 1 < max && buf.is_line_folded(self.cursor_row) {
                self.cursor_row += 1;
            }
            self.clamp_cursor(buf);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        }
        self.sync_primary();
    }

    pub fn move_right(&mut self, buf: &crate::buffer::Buffer) {
        if self.cursor_col < buf.line_len(self.cursor_row) {
            self.cursor_col += 1;
        }
    }

    pub fn move_to_line_start(&mut self) {
        self.cursor_col = 0;
        self.sync_primary();
    }

    pub fn move_to_line_end(&mut self, buf: &crate::buffer::Buffer) {
        self.cursor_col = buf.line_len(self.cursor_row);
        self.sync_primary();
    }

    pub fn move_to_first_line(&mut self, buf: &crate::buffer::Buffer) {
        self.cursor_row = 0;
        self.clamp_cursor(buf);
    }

    pub fn move_to_last_line(&mut self, buf: &crate::buffer::Buffer) {
        let count = buf.display_line_count();
        if count > 0 {
            self.cursor_row = count - 1;
        }
        self.clamp_cursor(buf);
    }

    // --- Word motions ---

    /// Helper: convert a char offset back to (row, col) and set cursor.
    fn set_cursor_from_offset(&mut self, buf: &crate::buffer::Buffer, char_pos: usize) {
        let rope = buf.rope();
        if rope.len_chars() == 0 {
            self.cursor_row = 0;
            self.cursor_col = 0;
            self.sync_primary();
            return;
        }
        let pos = char_pos.min(rope.len_chars().saturating_sub(1));
        self.cursor_row = rope.char_to_line(pos);
        let line_start = rope.line_to_char(self.cursor_row);
        self.cursor_col = pos - line_start;
        self.sync_primary();
    }

    pub fn move_word_forward(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::word_start_forward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    pub fn move_word_backward(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::word_start_backward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    pub fn move_word_end(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::word_end_forward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    pub fn move_big_word_forward(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::big_word_start_forward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    pub fn move_big_word_backward(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::big_word_start_backward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    pub fn move_big_word_end(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::big_word_end_forward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    pub fn move_word_end_backward(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::word_end_backward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    pub fn move_big_word_end_backward(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        let new_pos = crate::word::big_word_end_backward(buf.rope(), offset);
        self.set_cursor_from_offset(buf, new_pos);
    }

    /// vi `^` — move to first non-blank column on the current line.
    pub fn move_to_first_non_blank(&mut self, buf: &crate::buffer::Buffer) {
        self.cursor_col = crate::word::first_non_blank_col(buf.rope(), self.cursor_row);
    }

    pub fn move_matching_bracket(&mut self, buf: &crate::buffer::Buffer) {
        let offset = buf.char_offset_at(self.cursor_row, self.cursor_col);
        if let Some(new_pos) = crate::word::matching_bracket(buf.rope(), offset) {
            self.set_cursor_from_offset(buf, new_pos);
        }
    }

    pub fn move_paragraph_forward(&mut self, buf: &crate::buffer::Buffer) {
        let new_line = crate::word::paragraph_forward(buf.rope(), self.cursor_row);
        self.cursor_row = new_line;
        self.cursor_col = 0;
        self.clamp_cursor(buf);
    }

    pub fn move_paragraph_backward(&mut self, buf: &crate::buffer::Buffer) {
        let new_line = crate::word::paragraph_backward(buf.rope(), self.cursor_row);
        self.cursor_row = new_line;
        self.cursor_col = 0;
        self.clamp_cursor(buf);
    }

    pub fn move_find_char(&mut self, buf: &crate::buffer::Buffer, ch: char) {
        if let Some(col) =
            crate::word::find_char_forward(buf.rope(), self.cursor_row, self.cursor_col, ch)
        {
            self.cursor_col = col;
        }
    }

    pub fn move_find_char_back(&mut self, buf: &crate::buffer::Buffer, ch: char) {
        if let Some(col) =
            crate::word::find_char_backward(buf.rope(), self.cursor_row, self.cursor_col, ch)
        {
            self.cursor_col = col;
        }
    }

    pub fn move_till_char(&mut self, buf: &crate::buffer::Buffer, ch: char) {
        if let Some(col) =
            crate::word::find_char_forward_till(buf.rope(), self.cursor_row, self.cursor_col, ch)
        {
            self.cursor_col = col;
        }
    }

    pub fn move_till_char_back(&mut self, buf: &crate::buffer::Buffer, ch: char) {
        if let Some(col) =
            crate::word::find_char_backward_till(buf.rope(), self.cursor_row, self.cursor_col, ch)
        {
            self.cursor_col = col;
        }
    }

    // --- Cursor sync ---

    /// Sync the `CursorSet` primary cursor with `cursor_row`/`cursor_col`.
    ///
    /// `cursor_row`/`cursor_col` are the authoritative cursor position used by
    /// all 50+ movement/edit functions. The `CursorSet` primary must mirror
    /// them so multi-cursor rendering and queries see the correct position.
    pub fn sync_primary(&mut self) {
        let p = self.cursor_set.primary_mut();
        p.row = self.cursor_row;
        p.col = self.cursor_col;
    }

    // --- Cursor clamping ---

    /// Ensure cursor is within valid bounds after any structural change.
    /// Respects narrowed range if set. Also syncs cursor_set primary.
    pub fn clamp_cursor(&mut self, buf: &crate::buffer::Buffer) {
        let line_count = buf.line_count();
        if line_count == 0 {
            self.cursor_row = 0;
            self.cursor_col = 0;
            self.sync_primary();
            return;
        }
        // Respect narrowed range
        if let Some((ns, ne)) = buf.narrowed_range {
            if self.cursor_row < ns {
                self.cursor_row = ns;
            }
            let max_narrow = ne.saturating_sub(1);
            if self.cursor_row > max_narrow {
                self.cursor_row = max_narrow;
            }
        }
        let max_row = line_count.saturating_sub(1);
        if self.cursor_row > max_row {
            self.cursor_row = max_row;
        }
        let line_len = buf.line_len(self.cursor_row);
        if self.cursor_col > line_len {
            self.cursor_col = line_len;
        }
        self.sync_primary();
    }

    // --- Scroll commands ---

    /// Scroll half a page up (Ctrl-U). Moves cursor up by half viewport.
    pub fn scroll_half_up(&mut self, viewport_height: usize) {
        let half = viewport_height / 2;
        self.cursor_row = self.cursor_row.saturating_sub(half);
        self.scroll_offset = self.scroll_offset.saturating_sub(half);
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll half a page down (Ctrl-D). Moves cursor down by half viewport.
    pub fn scroll_half_down(&mut self, buf: &crate::buffer::Buffer, viewport_height: usize) {
        let half = viewport_height / 2;
        let max_row = buf.display_line_count().saturating_sub(1);
        self.cursor_row = (self.cursor_row + half).min(max_row);
        self.scroll_offset = (self.scroll_offset + half).min(max_row);
        self.clamp_cursor(buf);
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll a full page up (Ctrl-B).
    pub fn scroll_page_up(&mut self, viewport_height: usize) {
        let page = viewport_height.saturating_sub(2); // keep 2 lines overlap like vim
        self.cursor_row = self.cursor_row.saturating_sub(page);
        self.scroll_offset = self.scroll_offset.saturating_sub(page);
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll a full page down (Ctrl-F).
    pub fn scroll_page_down(&mut self, buf: &crate::buffer::Buffer, viewport_height: usize) {
        let page = viewport_height.saturating_sub(2);
        let max_row = buf.display_line_count().saturating_sub(1);
        self.cursor_row = (self.cursor_row + page).min(max_row);
        self.scroll_offset = (self.scroll_offset + page).min(max_row);
        self.clamp_cursor(buf);
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll so the cursor line is centered on screen (zz).
    pub fn scroll_center(&mut self, viewport_height: usize) {
        let half = viewport_height / 2;
        self.scroll_offset = self.cursor_row.saturating_sub(half);
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll so the cursor line is at the top of the screen (zt).
    pub fn scroll_cursor_top(&mut self) {
        self.scroll_offset = self.cursor_row;
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll so the cursor line is at the bottom of the screen (zb).
    pub fn scroll_cursor_bottom(&mut self, viewport_height: usize) {
        if viewport_height > 0 {
            self.scroll_offset = self.cursor_row.saturating_sub(viewport_height - 1);
        }
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll up one line (C-y). Cursor stays on screen.
    /// Skips over folded lines so each scroll step moves to the next visible line.
    pub fn scroll_up_line(&mut self, buf: &crate::buffer::Buffer, viewport_height: usize) {
        self.scroll_up_line_wrapped(buf, viewport_height, |_| 1);
        self.scroll_locked = true;
    }

    /// Scroll up one line (C-y) with wrap-aware cursor clamping.
    ///
    /// `line_visual_rows` returns how many display rows a buffer line occupies.
    /// Folded (invisible) lines return 0 and are skipped.
    /// The cursor is clamped to the last buffer line whose visual rows fit
    /// within the viewport, preventing `ensure_scroll_wrapped` from undoing
    /// the scroll on the next frame.
    pub fn scroll_up_line_wrapped<F>(
        &mut self,
        buf: &crate::buffer::Buffer,
        viewport_height: usize,
        line_visual_rows: F,
    ) where
        F: Fn(usize) -> usize,
    {
        // Move to previous visible line (skip folds).
        self.scroll_offset = buf.prev_visible_line(self.scroll_offset);
        if viewport_height == 0 {
            return;
        }
        // Walk forward from scroll_offset, counting visual rows, to find
        // the last buffer row that fits entirely in the viewport.
        let max_row = buf.display_line_count().saturating_sub(1);
        let mut visual = 0;
        let mut bottom = self.scroll_offset;
        let mut line = self.scroll_offset;
        while line <= max_row {
            let rows = line_visual_rows(line);
            if rows > 0 {
                if visual + rows > viewport_height {
                    break;
                }
                visual += rows;
                bottom = line;
            }
            line = buf.next_visible_line(line);
            if line <= bottom {
                break; // safety: prevent infinite loop
            }
        }
        if self.cursor_row > bottom {
            self.cursor_row = bottom;
            self.clamp_cursor(buf);
        }
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    /// Scroll down one line (C-e). Cursor stays on screen.
    /// Skips over folded lines so each scroll step moves to the next visible line.
    pub fn scroll_down_line(&mut self, buf: &crate::buffer::Buffer, viewport_height: usize) {
        let max_row = buf.display_line_count().saturating_sub(1);
        // Move to next visible line (skip folds).
        let next = buf.next_visible_line(self.scroll_offset);
        self.scroll_offset = next.min(max_row);
        // If cursor scrolled above the viewport, push it down to the top visible line.
        if self.cursor_row < self.scroll_offset {
            self.cursor_row = self.scroll_offset;
            self.clamp_cursor(buf);
        }
        let _ = viewport_height; // used by scroll_up_line for symmetry
        self.scroll_locked = true;
        self.scroll_locked_cursor = self.cursor_row;
    }

    // --- Screen-relative cursor ---

    /// Move cursor to top visible line (H).
    pub fn move_to_screen_top(&mut self) {
        self.cursor_row = self.scroll_offset;
        self.cursor_col = 0;
        self.sync_primary();
    }

    /// Move cursor to middle visible line (M).
    pub fn move_to_screen_middle(&mut self, buf: &crate::buffer::Buffer, viewport_height: usize) {
        let max_row = buf.display_line_count().saturating_sub(1);
        self.cursor_row = (self.scroll_offset + viewport_height / 2).min(max_row);
        self.cursor_col = 0;
        self.clamp_cursor(buf);
    }

    /// Move cursor to bottom visible line (L).
    pub fn move_to_screen_bottom(&mut self, buf: &crate::buffer::Buffer, viewport_height: usize) {
        let max_row = buf.display_line_count().saturating_sub(1);
        self.cursor_row = (self.scroll_offset + viewport_height.saturating_sub(1)).min(max_row);
        self.cursor_col = 0;
        self.clamp_cursor(buf);
    }

    // --- Scrolling ---

    /// Adjust scroll_offset so the cursor stays visible within the viewport,
    /// keeping at least `scrolloff` lines of context above/below the cursor.
    pub fn ensure_scroll(&mut self, viewport_height: usize) {
        self.ensure_scroll_with_margin(viewport_height, 0);
    }

    /// Like `ensure_scroll` but respects a scroll margin (vim `scrolloff`).
    pub fn ensure_scroll_with_margin(&mut self, viewport_height: usize, scrolloff: usize) {
        if viewport_height == 0 {
            return;
        }
        let margin = scrolloff.min(viewport_height / 2);
        if self.cursor_row < self.scroll_offset + margin {
            self.scroll_offset = self.cursor_row.saturating_sub(margin);
        }
        if self.cursor_row + margin >= self.scroll_offset + viewport_height {
            self.scroll_offset = (self.cursor_row + margin + 1).saturating_sub(viewport_height);
        }
    }

    /// Word-wrap-aware scroll adjustment. Counts visual rows consumed by
    /// wrapped lines between `scroll_offset` and `cursor_row`, and adjusts
    /// `scroll_offset` so the cursor line fits in the viewport.
    ///
    /// `line_visual_rows` returns how many visual rows a given buffer line
    /// occupies (>= 1). For non-wrapped buffers, always returns 1.
    ///
    /// O(viewport_height) — walks backward from the cursor line instead of
    /// incrementing scroll_offset forward from its old position.
    pub fn ensure_scroll_wrapped<F>(&mut self, viewport_height: usize, line_visual_rows: F)
    where
        F: Fn(usize) -> usize,
    {
        self.ensure_scroll_wrapped_with_margin(viewport_height, 0, usize::MAX, line_visual_rows);
    }

    /// Like `ensure_scroll_wrapped` but respects a scroll margin (vim `scrolloff`).
    /// The margin is applied as extra visual rows required above/below the cursor.
    ///
    /// `line_count` is the total number of buffer lines — used to reduce the
    /// bottom margin when the cursor is near the buffer end (vim behavior:
    /// scrolloff doesn't apply at buffer extremes).
    pub fn ensure_scroll_wrapped_with_margin<F>(
        &mut self,
        viewport_height: usize,
        scrolloff: usize,
        line_count: usize,
        line_visual_rows: F,
    ) where
        F: Fn(usize) -> usize,
    {
        if viewport_height == 0 {
            return;
        }
        let old_offset = self.scroll_offset;
        let margin = scrolloff.min(viewport_height / 2);

        // Adaptive margins: reduce at buffer extremes (vim behavior).
        let top_margin = margin.min(self.cursor_row);
        let lines_below = line_count.saturating_sub(self.cursor_row + 1);
        let bottom_margin = margin.min(lines_below);

        // Cursor above viewport (with margin) — scroll up.
        // Count visual rows for margin lines above the cursor.
        let mut margin_above_lines = 0;
        let mut margin_above_rows = 0;
        {
            let mut line = self.cursor_row;
            while margin_above_rows < top_margin && line > 0 {
                line -= 1;
                margin_above_rows += line_visual_rows(line);
                margin_above_lines += 1;
            }
        }
        let top_target = self.cursor_row.saturating_sub(margin_above_lines);
        if top_target < self.scroll_offset {
            self.scroll_offset = top_target;
            tracing::trace!(
                cursor = self.cursor_row,
                old = old_offset,
                new = self.scroll_offset,
                top_margin,
                bottom_margin,
                "ensure_scroll: top adjust"
            );
            return;
        }

        // Fast check: is cursor already visible with margin below?
        let mut visual = 0;
        let mut cursor_visible = false;
        for line in self.scroll_offset..=self.cursor_row {
            let rows = line_visual_rows(line);
            if line == self.cursor_row {
                if visual + rows + bottom_margin <= viewport_height {
                    cursor_visible = true;
                }
                break;
            }
            visual += rows;
            if visual >= viewport_height {
                break;
            }
        }
        if cursor_visible {
            return;
        }

        // Cursor not visible — walk backward from cursor_row to find the
        // right scroll_offset, reserving margin rows below the cursor.
        // Cap cursor_rows at viewport_height so a single large line (e.g. image)
        // doesn't make budget go to zero and force scroll_offset == cursor_row.
        let cursor_rows = line_visual_rows(self.cursor_row).min(viewport_height);
        let mut budget = viewport_height
            .saturating_sub(cursor_rows)
            .saturating_sub(bottom_margin);
        let mut new_offset = self.cursor_row;
        while new_offset > 0 {
            let prev_rows = line_visual_rows(new_offset - 1);
            if prev_rows > budget {
                break;
            }
            budget -= prev_rows;
            new_offset -= 1;
        }
        self.scroll_offset = new_offset;
        if self.scroll_offset != old_offset {
            tracing::trace!(
                cursor = self.cursor_row,
                old = old_offset,
                new = self.scroll_offset,
                cursor_rows,
                bottom_margin,
                viewport_height,
                "ensure_scroll: bottom adjust"
            );
        }
    }

    /// Adjust horizontal scroll so the cursor column stays visible.
    /// `viewport_width` is the number of text columns available (after gutter).
    pub fn ensure_scroll_horizontal(&mut self, viewport_width: usize) {
        if viewport_width == 0 {
            return;
        }
        if self.cursor_col < self.col_offset {
            self.col_offset = self.cursor_col;
        }
        if self.cursor_col >= self.col_offset + viewport_width {
            self.col_offset = self.cursor_col - viewport_width + 1;
        }
    }
}

/// Direction for splits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal, // top / bottom
    Vertical,   // left / right
}

/// Direction for focus movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// A node in the binary window layout tree.
///
/// Emacs uses the same model: a frame's window tree is a binary tree of
/// horizontal and vertical splits, with leaves being actual windows.
#[derive(Debug, Clone)]
pub enum LayoutNode {
    Leaf(WindowId),
    Split {
        direction: SplitDirection,
        /// Proportion allocated to `first` child (0.0..1.0).
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

/// Minimum window dimensions to prevent unusable splits.
pub const MIN_WINDOW_HEIGHT: u16 = 3;
pub const MIN_WINDOW_WIDTH: u16 = 20;

/// Simple rectangle for layout computation (avoids depending on ratatui in core).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    /// Test if a point (col, row) is inside this rectangle (exclusive upper bound).
    pub fn contains(&self, col: u16, row: u16) -> bool {
        col >= self.x && col < self.x + self.width && row >= self.y && row < self.y + self.height
    }
}

/// Manages the window tree, focus, and window-buffer associations.
///
/// Vim's model: closing a window does NOT delete the buffer. The editor always
/// has at least one window. This is proven across 30+ years of Vim.
pub struct WindowManager {
    windows: HashMap<WindowId, Window>,
    pub layout: LayoutNode,
    focused: WindowId,
    next_id: WindowId,
}

impl WindowManager {
    /// Create a new window manager with a single window viewing `buffer_idx`.
    pub fn new(buffer_idx: usize) -> Self {
        let id = 0;
        let window = Window::new(id, buffer_idx);
        let mut windows = HashMap::new();
        windows.insert(id, window);

        WindowManager {
            windows,
            layout: LayoutNode::Leaf(id),
            focused: id,
            next_id: 1,
        }
    }

    /// Get the focused window.
    pub fn focused_window(&self) -> &Window {
        self.windows
            .get(&self.focused)
            .expect("focused window must exist")
    }

    /// Get the focused window mutably.
    pub fn focused_window_mut(&mut self) -> &mut Window {
        self.windows
            .get_mut(&self.focused)
            .expect("focused window must exist")
    }

    /// Get the focused window ID.
    pub fn focused_id(&self) -> WindowId {
        self.focused
    }

    /// Set the focused window by ID. No-op if the window doesn't exist.
    pub fn set_focused(&mut self, id: WindowId) {
        if self.windows.contains_key(&id) {
            self.focused = id;
        }
    }

    /// Get a window by ID.
    pub fn window(&self, id: WindowId) -> Option<&Window> {
        self.windows.get(&id)
    }

    /// Get a mutable window by ID.
    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    /// How many windows are open.
    pub fn window_count(&self) -> usize {
        self.windows.len()
    }

    /// Cycle focus to the next window (by ID order, wrapping around).
    pub fn focus_next(&mut self) {
        if self.windows.len() <= 1 {
            return;
        }
        let mut ids: Vec<WindowId> = self.windows.keys().copied().collect();
        ids.sort();
        let pos = ids.iter().position(|&id| id == self.focused).unwrap_or(0);
        let next = (pos + 1) % ids.len();
        self.focused = ids[next];
    }

    /// Iterate over all windows.
    pub fn iter_windows(&self) -> impl Iterator<Item = &Window> {
        self.windows.values()
    }

    /// Iterate over all windows mutably.
    pub fn iter_windows_mut(&mut self) -> impl Iterator<Item = &mut Window> {
        self.windows.values_mut()
    }

    /// Split the focused window in the given direction.
    /// The new window views `buffer_idx`. Returns the new window's ID.
    /// Fails if the resulting windows would be too small.
    pub fn split(
        &mut self,
        direction: SplitDirection,
        buffer_idx: usize,
        available: Rect,
    ) -> Result<WindowId, String> {
        // Check minimum size
        let rects = self.layout_rects(available);
        let focused_rect = rects
            .iter()
            .find(|(id, _)| *id == self.focused)
            .map(|(_, r)| *r)
            .unwrap_or(available);

        match direction {
            SplitDirection::Vertical => {
                let half = focused_rect.width / 2;
                if half < MIN_WINDOW_WIDTH {
                    return Err(format!(
                        "Cannot split: width {} < minimum {} per pane",
                        half, MIN_WINDOW_WIDTH
                    ));
                }
            }
            SplitDirection::Horizontal => {
                let half = focused_rect.height / 2;
                if half < MIN_WINDOW_HEIGHT {
                    return Err(format!(
                        "Cannot split: height {} < minimum {} per pane",
                        half, MIN_WINDOW_HEIGHT
                    ));
                }
            }
        }

        let new_id = self.next_id;
        self.next_id += 1;

        let new_window = Window::new(new_id, buffer_idx);
        self.windows.insert(new_id, new_window);

        // Replace the focused leaf with a split
        let old_focused = self.focused;
        self.replace_leaf(
            old_focused,
            LayoutNode::Split {
                direction,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(old_focused)),
                second: Box::new(LayoutNode::Leaf(new_id)),
            },
        );

        Ok(new_id)
    }

    /// Split the focused window with a custom ratio (0.0..1.0).
    /// `ratio` is the proportion allocated to the first (original) child.
    pub fn split_with_ratio(
        &mut self,
        direction: SplitDirection,
        buffer_idx: usize,
        available: Rect,
        ratio: f32,
    ) -> Result<WindowId, String> {
        let rects = self.layout_rects(available);
        let focused_rect = rects
            .iter()
            .find(|(id, _)| *id == self.focused)
            .map(|(_, r)| *r)
            .unwrap_or(available);

        match direction {
            SplitDirection::Vertical => {
                let smaller = (focused_rect.width as f32 * ratio.min(1.0 - ratio)) as u16;
                if smaller < MIN_WINDOW_WIDTH {
                    return Err(format!(
                        "Cannot split: pane width {} < minimum {}",
                        smaller, MIN_WINDOW_WIDTH
                    ));
                }
            }
            SplitDirection::Horizontal => {
                let smaller = (focused_rect.height as f32 * ratio.min(1.0 - ratio)) as u16;
                if smaller < MIN_WINDOW_HEIGHT {
                    return Err(format!(
                        "Cannot split: pane height {} < minimum {}",
                        smaller, MIN_WINDOW_HEIGHT
                    ));
                }
            }
        }

        let new_id = self.next_id;
        self.next_id += 1;

        let new_window = Window::new(new_id, buffer_idx);
        self.windows.insert(new_id, new_window);

        let old_focused = self.focused;
        self.replace_leaf(
            old_focused,
            LayoutNode::Split {
                direction,
                ratio: ratio.clamp(0.1, 0.9),
                first: Box::new(LayoutNode::Leaf(old_focused)),
                second: Box::new(LayoutNode::Leaf(new_id)),
            },
        );

        Ok(new_id)
    }

    /// Close a window. Cannot close the last window.
    /// Returns the buffer_idx of the closed window (now hidden).
    pub fn close(&mut self, id: WindowId) -> Option<usize> {
        if self.windows.len() <= 1 {
            return None; // Can't close last window
        }

        let buffer_idx = self.windows.get(&id)?.buffer_idx;

        // Remove from layout tree (promote sibling)
        self.remove_leaf(id);
        self.windows.remove(&id);

        // If we closed the focused window, focus the first remaining window
        if self.focused == id {
            self.focused = self.first_leaf_id(&self.layout);
        }

        Some(buffer_idx)
    }

    /// Move focus in the given direction based on spatial layout.
    pub fn focus_direction(&mut self, dir: Direction, total: Rect) {
        let rects = self.layout_rects(total);
        let focused_rect = match rects.iter().find(|(id, _)| *id == self.focused) {
            Some((_, r)) => *r,
            None => return,
        };

        // Find center of focused window
        let fx = focused_rect.x as i32 + focused_rect.width as i32 / 2;
        let fy = focused_rect.y as i32 + focused_rect.height as i32 / 2;

        let mut best: Option<(WindowId, i32)> = None;

        for &(id, rect) in &rects {
            if id == self.focused {
                continue;
            }
            let cx = rect.x as i32 + rect.width as i32 / 2;
            let cy = rect.y as i32 + rect.height as i32 / 2;

            let is_valid_direction = match dir {
                Direction::Left => cx < fx,
                Direction::Right => cx > fx,
                Direction::Up => cy < fy,
                Direction::Down => cy > fy,
            };

            if is_valid_direction {
                let dist = (cx - fx).abs() + (cy - fy).abs();
                if best.is_none() || dist < best.unwrap().1 {
                    best = Some((id, dist));
                }
            }
        }

        if let Some((id, _)) = best {
            self.focused = id;
        }
    }

    /// Compute screen rectangles for each window by recursively dividing the total area.
    pub fn layout_rects(&self, total: Rect) -> Vec<(WindowId, Rect)> {
        let mut result = Vec::new();
        Self::compute_rects(&self.layout, total, &mut result);
        result
    }

    /// Find the window at the given cell coordinates, or `None` if outside all windows.
    pub fn window_at_cell(&self, col: u16, row: u16, total: Rect) -> Option<WindowId> {
        self.layout_rects(total)
            .iter()
            .find(|(_, rect)| rect.contains(col, row))
            .map(|(id, _)| *id)
    }

    fn compute_rects(node: &LayoutNode, area: Rect, out: &mut Vec<(WindowId, Rect)>) {
        match node {
            LayoutNode::Leaf(id) => {
                out.push((*id, area));
            }
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (first_area, second_area) = match direction {
                    SplitDirection::Vertical => {
                        let first_width = (area.width as f32 * ratio) as u16;
                        let second_width = area.width - first_width;
                        (
                            Rect {
                                x: area.x,
                                y: area.y,
                                width: first_width,
                                height: area.height,
                            },
                            Rect {
                                x: area.x + first_width,
                                y: area.y,
                                width: second_width,
                                height: area.height,
                            },
                        )
                    }
                    SplitDirection::Horizontal => {
                        let first_height = (area.height as f32 * ratio) as u16;
                        let second_height = area.height - first_height;
                        (
                            Rect {
                                x: area.x,
                                y: area.y,
                                width: area.width,
                                height: first_height,
                            },
                            Rect {
                                x: area.x,
                                y: area.y + first_height,
                                width: area.width,
                                height: second_height,
                            },
                        )
                    }
                };
                Self::compute_rects(first, first_area, out);
                Self::compute_rects(second, second_area, out);
            }
        }
    }

    /// Replace a leaf node with a new node in the layout tree.
    fn replace_leaf(&mut self, target_id: WindowId, replacement: LayoutNode) {
        self.layout = Self::replace_leaf_recursive(
            std::mem::replace(&mut self.layout, LayoutNode::Leaf(0)),
            target_id,
            replacement,
        );
    }

    fn replace_leaf_recursive(
        node: LayoutNode,
        target_id: WindowId,
        replacement: LayoutNode,
    ) -> LayoutNode {
        match node {
            LayoutNode::Leaf(id) if id == target_id => replacement,
            LayoutNode::Leaf(_) => node,
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                if Self::contains_leaf(&first, target_id) {
                    LayoutNode::Split {
                        direction,
                        ratio,
                        first: Box::new(Self::replace_leaf_recursive(
                            *first,
                            target_id,
                            replacement,
                        )),
                        second,
                    }
                } else {
                    LayoutNode::Split {
                        direction,
                        ratio,
                        first,
                        second: Box::new(Self::replace_leaf_recursive(
                            *second,
                            target_id,
                            replacement,
                        )),
                    }
                }
            }
        }
    }

    fn contains_leaf(node: &LayoutNode, target_id: WindowId) -> bool {
        match node {
            LayoutNode::Leaf(id) => *id == target_id,
            LayoutNode::Split { first, second, .. } => {
                Self::contains_leaf(first, target_id) || Self::contains_leaf(second, target_id)
            }
        }
    }

    /// Remove a leaf from the layout tree, promoting its sibling.
    fn remove_leaf(&mut self, target_id: WindowId) {
        self.layout = Self::remove_leaf_recursive(
            std::mem::replace(&mut self.layout, LayoutNode::Leaf(0)),
            target_id,
        );
    }

    fn remove_leaf_recursive(node: LayoutNode, target_id: WindowId) -> LayoutNode {
        match node {
            LayoutNode::Leaf(_) => node, // Can't remove a root leaf
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                // Check if one of the direct children is the target
                if matches!(&*first, LayoutNode::Leaf(id) if *id == target_id) {
                    return *second; // Promote sibling
                }
                if matches!(&*second, LayoutNode::Leaf(id) if *id == target_id) {
                    return *first; // Promote sibling
                }
                // Recurse into children
                LayoutNode::Split {
                    direction,
                    ratio,
                    first: Box::new(Self::remove_leaf_recursive(*first, target_id)),
                    second: Box::new(Self::remove_leaf_recursive(*second, target_id)),
                }
            }
        }
    }

    /// Get the first leaf window ID in the layout tree (leftmost/topmost).
    fn first_leaf_id(&self, node: &LayoutNode) -> WindowId {
        match node {
            LayoutNode::Leaf(id) => *id,
            LayoutNode::Split { first, .. } => self.first_leaf_id(first),
        }
    }

    // --- Resize / Balance / Maximize / Move ---

    /// Adjust the split ratio of the split containing the focused window.
    /// `delta` is ±0.05 typically. Direction: `Left`/`Up` adjust the first
    /// child, `Right`/`Down` adjust the second.
    pub fn adjust_ratio(&mut self, direction: Direction, delta: f32) {
        Self::adjust_ratio_recursive(&mut self.layout, self.focused, direction, delta);
    }

    fn adjust_ratio_recursive(
        node: &mut LayoutNode,
        target: WindowId,
        direction: Direction,
        delta: f32,
    ) -> bool {
        match node {
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split {
                direction: split_dir,
                ratio,
                first,
                second,
            } => {
                let in_first = Self::contains_leaf(first, target);
                let in_second = Self::contains_leaf(second, target);
                if !in_first && !in_second {
                    return false;
                }
                // Only adjust if the split orientation matches the direction.
                let matching = matches!(
                    (split_dir, direction),
                    (SplitDirection::Vertical, Direction::Left | Direction::Right)
                        | (SplitDirection::Horizontal, Direction::Up | Direction::Down)
                );
                if matching && (in_first || in_second) {
                    // "grow" = increase focused side. If focused is in first, grow means more ratio.
                    let grow = matches!(direction, Direction::Right | Direction::Down);
                    if in_first {
                        *ratio = (*ratio + if grow { delta } else { -delta }).clamp(0.1, 0.9);
                    } else {
                        *ratio = (*ratio + if grow { -delta } else { delta }).clamp(0.1, 0.9);
                    }
                    return true;
                }
                // Recurse into the subtree that contains the target.
                if in_first {
                    Self::adjust_ratio_recursive(first, target, direction, delta)
                } else {
                    Self::adjust_ratio_recursive(second, target, direction, delta)
                }
            }
        }
    }

    /// Set all split ratios to 0.5 recursively.
    pub fn balance(&mut self) {
        Self::balance_recursive(&mut self.layout);
    }

    fn balance_recursive(node: &mut LayoutNode) {
        if let LayoutNode::Split {
            ratio,
            first,
            second,
            ..
        } = node
        {
            *ratio = 0.5;
            Self::balance_recursive(first);
            Self::balance_recursive(second);
        }
    }

    /// Toggle maximize: save layout and replace with single focused leaf,
    /// or restore the saved layout.
    pub fn maximize_toggle(
        &mut self,
        saved_layout: &mut Option<(HashMap<WindowId, Window>, LayoutNode, WindowId, WindowId)>,
    ) {
        if let Some((windows, layout, focused, next_id)) = saved_layout.take() {
            // Restore.
            self.windows = windows;
            self.layout = layout;
            self.focused = focused;
            self.next_id = next_id;
        } else {
            // Save and maximize.
            *saved_layout = Some(self.snapshot());
            // Keep only the focused window.
            let focused = self.focused;
            self.layout = LayoutNode::Leaf(focused);
            self.windows.retain(|&id, _| id == focused);
        }
    }

    /// Move the focused window in the given direction by swapping it with its
    /// neighbor. Returns true if a swap occurred.
    pub fn move_window(&mut self, dir: Direction, total: Rect) -> bool {
        let rects = self.layout_rects(total);
        let focused_rect = match rects.iter().find(|(id, _)| *id == self.focused) {
            Some((_, r)) => *r,
            None => return false,
        };
        let fx = focused_rect.x as i32 + focused_rect.width as i32 / 2;
        let fy = focused_rect.y as i32 + focused_rect.height as i32 / 2;

        let mut best: Option<(WindowId, i32)> = None;
        for &(id, rect) in &rects {
            if id == self.focused {
                continue;
            }
            let cx = rect.x as i32 + rect.width as i32 / 2;
            let cy = rect.y as i32 + rect.height as i32 / 2;
            let is_valid = match dir {
                Direction::Left => cx < fx,
                Direction::Right => cx > fx,
                Direction::Up => cy < fy,
                Direction::Down => cy > fy,
            };
            if is_valid {
                let dist = (cx - fx).abs() + (cy - fy).abs();
                if best.is_none() || dist < best.unwrap().1 {
                    best = Some((id, dist));
                }
            }
        }

        if let Some((neighbor_id, _)) = best {
            // Swap the two window IDs in the layout tree.
            Self::swap_leaves(&mut self.layout, self.focused, neighbor_id);
            true
        } else {
            false
        }
    }

    fn swap_leaves(node: &mut LayoutNode, a: WindowId, b: WindowId) {
        match node {
            LayoutNode::Leaf(id) => {
                if *id == a {
                    *id = b;
                } else if *id == b {
                    *id = a;
                }
            }
            LayoutNode::Split { first, second, .. } => {
                Self::swap_leaves(first, a, b);
                Self::swap_leaves(second, a, b);
            }
        }
    }

    /// Take a snapshot of the window manager state (layout, windows, focus).
    pub fn snapshot(&self) -> (HashMap<WindowId, Window>, LayoutNode, WindowId, WindowId) {
        (
            self.windows.clone(),
            self.layout.clone(),
            self.focused,
            self.next_id,
        )
    }

    /// Restore window manager state from a snapshot.
    pub fn restore(
        &mut self,
        windows: HashMap<WindowId, Window>,
        layout: LayoutNode,
        focused: WindowId,
        next_id: WindowId,
    ) {
        self.windows = windows;
        self.layout = layout;
        self.focused = focused;
        self.next_id = next_id;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_area() -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        }
    }

    #[test]
    fn new_manager_has_single_window() {
        let wm = WindowManager::new(0);
        assert_eq!(wm.window_count(), 1);
        assert_eq!(wm.focused_window().buffer_idx, 0);
        assert_eq!(wm.focused_window().cursor_row, 0);
    }

    #[test]
    fn split_vertical_creates_two_windows() {
        let mut wm = WindowManager::new(0);
        let result = wm.split(SplitDirection::Vertical, 1, default_area());
        assert!(result.is_ok());
        assert_eq!(wm.window_count(), 2);
        // Original window still focused
        assert_eq!(wm.focused_window().buffer_idx, 0);
        // New window has buffer_idx 1
        let new_id = result.unwrap();
        assert_eq!(wm.window(new_id).unwrap().buffer_idx, 1);
    }

    #[test]
    fn split_horizontal_creates_two_windows() {
        let mut wm = WindowManager::new(0);
        let result = wm.split(SplitDirection::Horizontal, 1, default_area());
        assert!(result.is_ok());
        assert_eq!(wm.window_count(), 2);
    }

    #[test]
    fn close_last_window_is_noop() {
        let mut wm = WindowManager::new(0);
        let result = wm.close(0);
        assert!(result.is_none());
        assert_eq!(wm.window_count(), 1);
    }

    #[test]
    fn close_window_promotes_sibling() {
        let mut wm = WindowManager::new(0);
        let new_id = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        assert_eq!(wm.window_count(), 2);

        let closed_buf = wm.close(new_id);
        assert_eq!(closed_buf, Some(1));
        assert_eq!(wm.window_count(), 1);
        // Layout should be back to a single leaf
        assert!(matches!(wm.layout, LayoutNode::Leaf(0)));
    }

    #[test]
    fn focus_direction_left_right() {
        let mut wm = WindowManager::new(0);
        let new_id = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();

        // Focus starts on left (window 0)
        assert_eq!(wm.focused_id(), 0);

        // Move right → should focus the new window
        wm.focus_direction(Direction::Right, default_area());
        assert_eq!(wm.focused_id(), new_id);

        // Move left → back to original
        wm.focus_direction(Direction::Left, default_area());
        assert_eq!(wm.focused_id(), 0);
    }

    #[test]
    fn focus_direction_up_down() {
        let mut wm = WindowManager::new(0);
        let new_id = wm
            .split(SplitDirection::Horizontal, 1, default_area())
            .unwrap();

        assert_eq!(wm.focused_id(), 0);

        wm.focus_direction(Direction::Down, default_area());
        assert_eq!(wm.focused_id(), new_id);

        wm.focus_direction(Direction::Up, default_area());
        assert_eq!(wm.focused_id(), 0);
    }

    #[test]
    fn focus_at_boundary_is_noop() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();

        // Already at leftmost, moving left should do nothing
        wm.focus_direction(Direction::Left, default_area());
        assert_eq!(wm.focused_id(), 0);

        // Moving up should also do nothing (horizontal split doesn't exist)
        wm.focus_direction(Direction::Up, default_area());
        assert_eq!(wm.focused_id(), 0);
    }

    #[test]
    fn focus_next_cycles_windows() {
        let mut wm = WindowManager::new(0);
        let id1 = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        assert_eq!(wm.focused_id(), 0);

        wm.focus_next();
        assert_eq!(wm.focused_id(), id1);

        // Wraps around
        wm.focus_next();
        assert_eq!(wm.focused_id(), 0);
    }

    #[test]
    fn focus_next_single_window_noop() {
        let mut wm = WindowManager::new(0);
        wm.focus_next();
        assert_eq!(wm.focused_id(), 0);
    }

    #[test]
    fn layout_rects_single_window_fills_area() {
        let wm = WindowManager::new(0);
        let area = default_area();
        let rects = wm.layout_rects(area);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0], (0, area));
    }

    #[test]
    fn layout_rects_vertical_split() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        let rects = wm.layout_rects(default_area());
        assert_eq!(rects.len(), 2);

        let (_, left) = rects[0];
        let (_, right) = rects[1];

        // Left gets 50% width
        assert_eq!(left.x, 0);
        assert_eq!(left.width, 60); // 120 * 0.5
                                    // Right gets the other 50%
        assert_eq!(right.x, 60);
        assert_eq!(right.width, 60);
        // Both full height
        assert_eq!(left.height, 40);
        assert_eq!(right.height, 40);
    }

    #[test]
    fn layout_rects_horizontal_split() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Horizontal, 1, default_area())
            .unwrap();
        let rects = wm.layout_rects(default_area());
        assert_eq!(rects.len(), 2);

        let (_, top) = rects[0];
        let (_, bottom) = rects[1];

        assert_eq!(top.y, 0);
        assert_eq!(top.height, 20); // 40 * 0.5
        assert_eq!(bottom.y, 20);
        assert_eq!(bottom.height, 20);
        assert_eq!(top.width, 120);
        assert_eq!(bottom.width, 120);
    }

    #[test]
    fn layout_rects_nested_splits() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        // Focus left window, split horizontally
        let _ = wm
            .split(SplitDirection::Horizontal, 2, default_area())
            .unwrap();
        let rects = wm.layout_rects(default_area());
        assert_eq!(rects.len(), 3);
    }

    #[test]
    fn split_refuses_when_too_small() {
        let mut wm = WindowManager::new(0);
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 30,
            height: 4,
        };
        // Vertical split: 30/2=15 < MIN_WINDOW_WIDTH=20
        let result = wm.split(SplitDirection::Vertical, 1, tiny);
        assert!(result.is_err());
        assert_eq!(wm.window_count(), 1);

        // Horizontal split: 4/2=2 < MIN_WINDOW_HEIGHT=3
        let result = wm.split(SplitDirection::Horizontal, 1, tiny);
        assert!(result.is_err());
        assert_eq!(wm.window_count(), 1);
    }

    // --- Scroll and screen-relative cursor tests ---

    fn make_buffer(lines: usize) -> crate::buffer::Buffer {
        let text: String = (0..lines).map(|i| format!("line {}\n", i)).collect();
        let mut buf = crate::buffer::Buffer::new();
        buf.insert_text_at(0, &text);
        buf
    }

    #[test]
    fn scroll_half_down_moves_cursor_and_scroll() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 0;
        win.scroll_offset = 0;
        win.scroll_half_down(&buf, 20);
        assert_eq!(win.cursor_row, 10);
        assert_eq!(win.scroll_offset, 10);
    }

    #[test]
    fn scroll_half_up_moves_cursor_and_scroll() {
        let _buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 40;
        win.scroll_half_up(20);
        assert_eq!(win.cursor_row, 40);
        assert_eq!(win.scroll_offset, 30);
    }

    #[test]
    fn scroll_page_down_moves_full_page() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 0;
        win.scroll_offset = 0;
        // page = 20 - 2 = 18
        win.scroll_page_down(&buf, 20);
        assert_eq!(win.cursor_row, 18);
        assert_eq!(win.scroll_offset, 18);
    }

    #[test]
    fn scroll_page_up_moves_full_page() {
        let _buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 40;
        // page = 20 - 2 = 18
        win.scroll_page_up(20);
        assert_eq!(win.cursor_row, 32);
        assert_eq!(win.scroll_offset, 22);
    }

    #[test]
    fn scroll_center_positions_cursor_mid_screen() {
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_center(20);
        // half = 10, so scroll_offset = 50 - 10 = 40
        assert_eq!(win.scroll_offset, 40);
    }

    #[test]
    fn scroll_top_positions_cursor_at_top() {
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 30;
        win.scroll_cursor_top();
        assert_eq!(win.scroll_offset, 50);
    }

    #[test]
    fn scroll_bottom_positions_cursor_at_bottom() {
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 50;
        win.scroll_cursor_bottom(20);
        // scroll_offset = 50 - 19 = 31
        assert_eq!(win.scroll_offset, 31);
    }

    #[test]
    fn move_screen_top_goes_to_first_visible() {
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 40;
        win.move_to_screen_top();
        assert_eq!(win.cursor_row, 40);
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn move_screen_middle_goes_to_mid_visible() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 40;
        win.move_to_screen_middle(&buf, 20);
        // middle = 40 + 10 = 50
        assert_eq!(win.cursor_row, 50);
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn move_screen_bottom_goes_to_last_visible() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 40;
        win.scroll_offset = 40;
        win.move_to_screen_bottom(&buf, 20);
        // bottom = 40 + 19 = 59
        assert_eq!(win.cursor_row, 59);
        assert_eq!(win.cursor_col, 0);
    }

    #[test]
    fn scroll_half_down_clamps_at_end() {
        let buf = make_buffer(10);
        let mut win = Window::new(0, 0);
        win.cursor_row = 8;
        win.scroll_offset = 5;
        win.scroll_half_down(&buf, 20);
        // max_row = 10 (10 lines + trailing newline = 11 lines, so max = 10)
        // cursor_row = min(8+10, 10) = 10
        assert!(win.cursor_row <= buf.display_line_count().saturating_sub(1));
    }

    #[test]
    fn scroll_half_up_clamps_at_start() {
        let _buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 3;
        win.scroll_offset = 2;
        win.scroll_half_up(20);
        assert_eq!(win.cursor_row, 0);
        assert_eq!(win.scroll_offset, 0);
    }

    #[test]
    fn scroll_down_line_clamps_cursor_to_viewport() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 0;
        win.scroll_offset = 0;
        // Scroll viewport down 5 lines — cursor at row 0 is now above viewport.
        for _ in 0..5 {
            win.scroll_down_line(&buf, 20);
        }
        assert_eq!(win.scroll_offset, 5);
        // Cursor must have been pushed to the top of the viewport.
        assert_eq!(win.cursor_row, 5);
    }

    #[test]
    fn scroll_up_line_clamps_cursor_to_viewport() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 40;
        // Scroll viewport up 15 lines — cursor at row 50 would go below viewport.
        for _ in 0..15 {
            win.scroll_up_line(&buf, 20);
        }
        assert_eq!(win.scroll_offset, 25);
        // Cursor must have been pulled to bottom visible line (25 + 19 = 44).
        assert_eq!(win.cursor_row, 44);
    }

    #[test]
    fn scroll_down_line_continues_past_cursor() {
        // Regression: C-e used to stop when cursor hit viewport bottom.
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 10;
        win.scroll_offset = 0;
        // Scroll 30 lines — well past the cursor's original position.
        for _ in 0..30 {
            win.scroll_down_line(&buf, 20);
        }
        assert_eq!(win.scroll_offset, 30);
        // Cursor should be at top of viewport.
        assert_eq!(win.cursor_row, 30);
    }

    #[test]
    fn scroll_up_line_wrapped_clamps_to_visual_bottom() {
        // Simulate: 100-line buffer where line 44 wraps to 3 visual rows.
        // With viewport_height=20, naive bottom = scroll_offset + 19 = 44,
        // but visually only fits through line 42 because line 44 takes 3 rows.
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 50;
        win.scroll_offset = 40;
        // Scroll up 15 with a closure that reports line 44 as 3 visual rows.
        for _ in 0..15 {
            win.scroll_up_line_wrapped(&buf, 20, |line| if line == 44 { 3 } else { 1 });
        }
        assert_eq!(win.scroll_offset, 25);
        // With line 44 being 3 rows, the viewport from 25 fits:
        // lines 25..43 = 19 lines * 1 row = 19 rows, then line 44 = 3 rows
        // = 22 rows > 20. So bottom should be 43, not 44.
        assert!(
            win.cursor_row <= 43,
            "cursor={} should be <= 43",
            win.cursor_row
        );
    }

    // --- WindowManager tests ---

    #[test]
    fn close_focused_refocuses() {
        let mut wm = WindowManager::new(0);
        let new_id = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        // Focus the new window
        wm.focus_direction(Direction::Right, default_area());
        assert_eq!(wm.focused_id(), new_id);

        // Close focused window
        wm.close(new_id);
        // Should refocus to remaining window
        assert_eq!(wm.focused_id(), 0);
        assert_eq!(wm.window_count(), 1);
    }

    // --- Horizontal scroll tests ---

    #[test]
    fn col_offset_starts_at_zero() {
        let win = Window::new(0, 0);
        assert_eq!(win.col_offset, 0);
    }

    #[test]
    fn ensure_scroll_horizontal_no_shift_when_visible() {
        let mut win = Window::new(0, 0);
        win.cursor_col = 5;
        win.ensure_scroll_horizontal(80);
        assert_eq!(win.col_offset, 0);
    }

    #[test]
    fn ensure_scroll_horizontal_shifts_right() {
        let mut win = Window::new(0, 0);
        win.cursor_col = 90;
        win.ensure_scroll_horizontal(80);
        // cursor_col (90) >= col_offset (0) + viewport_width (80), so shift
        assert_eq!(win.col_offset, 11); // 90 - 80 + 1
    }

    #[test]
    fn ensure_scroll_horizontal_shifts_left() {
        let mut win = Window::new(0, 0);
        win.col_offset = 20;
        win.cursor_col = 10;
        win.ensure_scroll_horizontal(80);
        // cursor_col (10) < col_offset (20), so shift left
        assert_eq!(win.col_offset, 10);
    }

    #[test]
    fn ensure_scroll_horizontal_zero_width_noop() {
        let mut win = Window::new(0, 0);
        win.cursor_col = 50;
        win.ensure_scroll_horizontal(0);
        assert_eq!(win.col_offset, 0);
    }

    #[test]
    fn ensure_scroll_horizontal_cursor_at_edge() {
        let mut win = Window::new(0, 0);
        win.cursor_col = 79;
        win.ensure_scroll_horizontal(80);
        // Exactly at last visible column — no shift needed
        assert_eq!(win.col_offset, 0);
    }

    #[test]
    fn ensure_scroll_horizontal_cursor_one_past_edge() {
        let mut win = Window::new(0, 0);
        win.cursor_col = 80;
        win.ensure_scroll_horizontal(80);
        // One past edge — needs shift
        assert_eq!(win.col_offset, 1);
    }

    #[test]
    fn ensure_scroll_wrapped_heading_scale_keeps_cursor_visible() {
        // Regression: scaled headings take >1 visual row but ensure_scroll
        // counted them as 1, allowing cursor to go below viewport.
        let mut win = Window::new(0, 0);
        win.scroll_offset = 0;
        win.cursor_row = 9;

        // Viewport = 10 cell rows.
        // Lines 0, 3, 6 are headings taking 2 visual rows each.
        // Visual budget: lines 0(2) + 1(1) + 2(1) + 3(2) + 4(1) + 5(1) + 6(2) = 10 rows.
        // So only lines 0-6 fit. Line 9 should push scroll forward.
        win.ensure_scroll_wrapped(10, |line| {
            if line == 0 || line == 3 || line == 6 {
                2 // heading: ceil(1.5)
            } else {
                1
            }
        });

        // Cursor at row 9 should NOT be at scroll_offset 0 (that only fits 7 lines = 10 rows).
        assert!(
            win.scroll_offset > 0,
            "scroll must advance when headings consume extra visual rows, got offset={}",
            win.scroll_offset
        );

        // Verify cursor is actually within viewport by walking forward.
        let mut visual = 0;
        for line in win.scroll_offset..=win.cursor_row {
            let rows = if line == 0 || line == 3 || line == 6 {
                2
            } else {
                1
            };
            visual += rows;
        }
        assert!(
            visual <= 10,
            "cursor row {} should be within viewport (visual rows used: {}, viewport: 10)",
            win.cursor_row,
            visual
        );
    }

    // --- Resize / Balance / Maximize / Move tests ---

    #[test]
    fn adjust_ratio_grows_and_shrinks() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        // Initial ratio is 0.5
        assert!(
            matches!(&wm.layout, LayoutNode::Split { ratio, .. } if (*ratio - 0.5).abs() < 0.01)
        );
        wm.adjust_ratio(Direction::Right, 0.05);
        if let LayoutNode::Split { ratio, .. } = &wm.layout {
            assert!((*ratio - 0.55).abs() < 0.01, "ratio should grow: {}", ratio);
        }
        wm.adjust_ratio(Direction::Left, 0.05);
        if let LayoutNode::Split { ratio, .. } = &wm.layout {
            assert!(
                (*ratio - 0.5).abs() < 0.01,
                "ratio should shrink back: {}",
                ratio
            );
        }
    }

    #[test]
    fn adjust_ratio_clamps() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        // Shrink many times — should clamp at 0.1
        for _ in 0..100 {
            wm.adjust_ratio(Direction::Left, 0.05);
        }
        if let LayoutNode::Split { ratio, .. } = &wm.layout {
            assert!(*ratio >= 0.1, "ratio should clamp at 0.1: {}", ratio);
        }
    }

    #[test]
    fn balance_resets_all_ratios() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        wm.adjust_ratio(Direction::Right, 0.2);
        wm.balance();
        if let LayoutNode::Split { ratio, .. } = &wm.layout {
            assert!((*ratio - 0.5).abs() < 0.01);
        }
    }

    #[test]
    fn maximize_and_restore() {
        let mut wm = WindowManager::new(0);
        let new_id = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        assert_eq!(wm.window_count(), 2);

        let mut saved = None;
        // Maximize — saves layout, leaves only focused window.
        wm.maximize_toggle(&mut saved);
        assert!(saved.is_some(), "layout should be saved");
        assert_eq!(wm.window_count(), 1);
        assert!(matches!(wm.layout, LayoutNode::Leaf(id) if id == 0));

        // Restore.
        wm.maximize_toggle(&mut saved);
        assert!(saved.is_none(), "saved should be consumed");
        assert_eq!(wm.window_count(), 2);
        assert!(wm.window(new_id).is_some());
    }

    #[test]
    fn move_window_swaps_positions() {
        let mut wm = WindowManager::new(0);
        let new_id = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        // Window 0 is left (first), new_id is right (second).
        assert_eq!(wm.focused_id(), 0);

        wm.move_window(Direction::Right, default_area());
        // After swap, window 0 should be on the right side of the layout.
        // The layout tree's first child should now be new_id, second = 0.
        if let LayoutNode::Split { first, second, .. } = &wm.layout {
            assert!(matches!(**first, LayoutNode::Leaf(id) if id == new_id));
            assert!(matches!(**second, LayoutNode::Leaf(id) if id == 0));
        } else {
            panic!("expected split layout");
        }
    }

    #[test]
    fn move_window_at_boundary_is_noop() {
        let mut wm = WindowManager::new(0);
        let _ = wm
            .split(SplitDirection::Vertical, 1, default_area())
            .unwrap();
        // Window 0 is already at leftmost — moving left should be noop.
        let moved = wm.move_window(Direction::Left, default_area());
        assert!(!moved);
    }

    #[test]
    fn single_window_resize_noop() {
        let mut wm = WindowManager::new(0);
        // No split → adjust_ratio should be harmless noop.
        wm.adjust_ratio(Direction::Right, 0.05);
        assert!(matches!(wm.layout, LayoutNode::Leaf(0)));
    }

    #[test]
    fn single_window_maximize_noop() {
        let mut wm = WindowManager::new(0);
        let mut saved = None;
        wm.maximize_toggle(&mut saved);
        // Single window — save still works but it's already maximized.
        assert_eq!(wm.window_count(), 1);
    }

    #[test]
    fn cursor_move_down_skips_folded_lines() {
        let mut win = Window::new(0, 0);
        let mut buf = crate::buffer::Buffer::new();
        buf.insert_text_at(0, "line0\nline1\nline2\nline3\nline4\n");
        // Fold lines 1-3 (visible: 0, 1(start), 4)
        buf.folded_ranges.push((1, 4));
        win.cursor_row = 1;
        win.move_down(&buf);
        // Should skip lines 2, 3 and land on 4
        assert_eq!(win.cursor_row, 4);
    }

    #[test]
    fn cursor_move_up_skips_folded_lines() {
        let mut win = Window::new(0, 0);
        let mut buf = crate::buffer::Buffer::new();
        buf.insert_text_at(0, "line0\nline1\nline2\nline3\nline4\n");
        buf.folded_ranges.push((1, 4));
        win.cursor_row = 4;
        win.move_up(&buf);
        // Should skip lines 3, 2 and land on 1
        assert_eq!(win.cursor_row, 1);
    }

    // --- Scrolloff (ensure_scroll_with_margin) tests ---

    #[test]
    fn scrolloff_keeps_margin_above_cursor() {
        let mut win = Window::new(0, 0);
        win.scroll_offset = 0;
        win.cursor_row = 3;
        // scrolloff=5, viewport=20: margin = min(5, 10) = 5
        // cursor_row (3) < scroll_offset (0) + margin (5) → adjust
        win.ensure_scroll_with_margin(20, 5);
        // scroll_offset should be max(0, 3-5) = 0 (can't go negative)
        assert_eq!(win.scroll_offset, 0);

        // Now cursor at row 10, scroll_offset at 8
        win.cursor_row = 10;
        win.scroll_offset = 8;
        // cursor_row (10) < scroll_offset (8) + margin (5) = 13? yes
        win.ensure_scroll_with_margin(20, 5);
        // scroll_offset = max(0, 10 - 5) = 5
        assert_eq!(win.scroll_offset, 5);
    }

    #[test]
    fn scrolloff_keeps_margin_below_cursor() {
        let mut win = Window::new(0, 0);
        win.scroll_offset = 0;
        win.cursor_row = 17;
        // scrolloff=5, viewport=20: margin=5
        // cursor_row (17) + margin (5) = 22 >= scroll_offset (0) + viewport (20) = 20
        win.ensure_scroll_with_margin(20, 5);
        // scroll_offset = (17 + 5 + 1) - 20 = 3
        assert_eq!(win.scroll_offset, 3);
    }

    #[test]
    fn scrolloff_capped_at_half_viewport() {
        let mut win = Window::new(0, 0);
        win.scroll_offset = 0;
        win.cursor_row = 3;
        // scrolloff=15, viewport=10: margin = min(15, 5) = 5
        win.ensure_scroll_with_margin(10, 15);
        // cursor_row (3) < 0 + 5 → adjust: scroll_offset = max(0, 3-5) = 0
        assert_eq!(win.scroll_offset, 0);

        // Cursor at bottom area
        win.cursor_row = 7;
        win.scroll_offset = 0;
        win.ensure_scroll_with_margin(10, 15);
        // 7 + 5 = 12 >= 0 + 10 → scroll_offset = (7+5+1) - 10 = 3
        assert_eq!(win.scroll_offset, 3);
    }

    #[test]
    fn scrolloff_zero_behaves_like_no_margin() {
        let mut win = Window::new(0, 0);
        win.scroll_offset = 0;
        win.cursor_row = 19;
        // scrolloff=0, viewport=20: margin=0
        // 19 + 0 = 19 < 0 + 20 = 20 → no adjustment
        win.ensure_scroll_with_margin(20, 0);
        assert_eq!(win.scroll_offset, 0);

        // Just past edge
        win.cursor_row = 20;
        win.ensure_scroll_with_margin(20, 0);
        // 20 + 0 = 20 >= 0 + 20 → scroll_offset = (20+0+1) - 20 = 1
        assert_eq!(win.scroll_offset, 1);
    }

    #[test]
    fn scrolloff_wrapped_keeps_margin() {
        let mut win = Window::new(0, 0);
        win.scroll_offset = 0;
        win.cursor_row = 8;
        // All lines 1 visual row, scrolloff=3, viewport=10
        // Margin below: cursor_row(8) visual row 1 + margin 3 = needs 4 rows below
        // 8 visible lines (0..7) = 8 rows + cursor(1) + margin(3) = 12 > 10
        win.ensure_scroll_wrapped_with_margin(10, 3, 100, |_| 1);
        // Should scroll forward so cursor has 3 lines of margin below
        assert!(
            win.scroll_offset > 0,
            "should have scrolled: offset={}",
            win.scroll_offset
        );
        // Verify: budget = 10 - 1(cursor) - 3(margin) = 6 rows above cursor
        // so scroll_offset = 8 - 6 = 2
        assert_eq!(win.scroll_offset, 2);
    }

    #[test]
    fn ensure_scroll_wrapped_large_line_no_thrash() {
        // A line with 50 visual rows (e.g. image) shouldn't force scroll_offset == cursor_row
        let mut win = Window::new(0, 0);
        win.cursor_row = 5;
        win.scroll_offset = 0;
        win.ensure_scroll_wrapped_with_margin(30, 0, 100, |line| if line == 5 { 50 } else { 1 });
        // scroll_offset should be at most cursor_row (not forced to cursor_row)
        assert!(
            win.scroll_offset <= 5,
            "scroll_offset should be <= 5, got {}",
            win.scroll_offset
        );
    }

    #[test]
    fn scroll_down_line_sets_scroll_locked() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.scroll_down_line(&buf, 20);
        assert!(win.scroll_locked);
    }

    #[test]
    fn scroll_up_line_sets_scroll_locked() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.scroll_offset = 10;
        win.cursor_row = 10;
        win.scroll_up_line(&buf, 20);
        assert!(win.scroll_locked);
    }

    #[test]
    fn ce_cy_roundtrip_with_scrolloff() {
        // C-e 5 times then C-y 5 times should return to original scroll position.
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 15;
        win.scroll_offset = 0;
        for _ in 0..5 {
            win.scroll_down_line(&buf, 30);
        }
        assert_eq!(win.scroll_offset, 5);
        for _ in 0..5 {
            win.scroll_up_line(&buf, 30);
        }
        assert_eq!(win.scroll_offset, 0);
    }

    #[test]
    fn ensure_scroll_bottom_margin_reduces_at_buffer_end() {
        // Cursor at last line of 50-line buffer, viewport=30, scrolloff=5
        let mut win = Window::new(0, 0);
        win.cursor_row = 49;
        win.scroll_offset = 20;
        win.ensure_scroll_wrapped_with_margin(30, 5, 50, |_| 1);
        // With lines_below=0, bottom_margin=0. Cursor at 49, budget=30-1-0=29.
        // scroll_offset = 49-29 = 20. Should stay at 20.
        assert_eq!(win.scroll_offset, 20);
    }

    #[test]
    fn ensure_scroll_top_margin_reduces_at_buffer_start() {
        // Cursor at line 2 of 100-line buffer, viewport=30, scrolloff=5
        let mut win = Window::new(0, 0);
        win.cursor_row = 2;
        win.scroll_offset = 10;
        win.ensure_scroll_wrapped_with_margin(30, 5, 100, |_| 1);
        // top_margin = min(5, 2) = 2. top_target = 2-2 = 0.
        // 0 < 10 → scroll_offset = 0.
        assert_eq!(win.scroll_offset, 0);
    }

    #[test]
    fn scroll_half_up_sets_scroll_locked() {
        let mut win = Window::new(0, 0);
        win.cursor_row = 20;
        win.scroll_offset = 10;
        win.scroll_half_up(20);
        assert!(win.scroll_locked);
    }

    #[test]
    fn clamp_cursor_allows_phantom_but_nav_uses_display() {
        // clamp_cursor uses line_count() (allows phantom line for insert mode),
        // but navigation call sites use display_line_count() to prevent cursor
        // from reaching the invisible phantom line during normal movement.
        let mut buf = crate::buffer::Buffer::new();
        buf.insert_text_at(0, "a\nb\n");
        assert_eq!(buf.line_count(), 3); // includes phantom
        assert_eq!(buf.display_line_count(), 2); // excludes phantom

        // clamp_cursor allows phantom line (needed for insert mode after Enter at EOF)
        let mut win = Window::new(0, 0);
        win.cursor_row = 2; // phantom line
        win.clamp_cursor(&buf);
        assert_eq!(win.cursor_row, 2); // allowed — insert mode needs this

        // But navigation should use display_line_count for bounds
        let nav_max = buf.display_line_count().saturating_sub(1);
        assert_eq!(nav_max, 1);
    }

    #[test]
    fn scroll_locked_persists_until_cursor_moves() {
        let buf = make_buffer(100);
        let mut win = Window::new(0, 0);
        win.cursor_row = 15;
        win.scroll_offset = 0;

        // C-e sets scroll_locked and records cursor position
        win.scroll_down_line(&buf, 30);
        assert!(win.scroll_locked);
        assert_eq!(win.scroll_locked_cursor, win.cursor_row);
        let locked_cursor = win.scroll_locked_cursor;

        // Simulate frame: cursor hasn't moved → stays locked
        assert_eq!(win.cursor_row, locked_cursor);

        // If cursor moves (j command), lock should be detected as stale
        win.cursor_row += 1;
        assert_ne!(win.cursor_row, win.scroll_locked_cursor);
    }

    #[test]
    fn scroll_locked_cursor_set_by_all_scroll_commands() {
        let buf = make_buffer(100);

        // scroll_half_up
        let mut win = Window::new(0, 0);
        win.cursor_row = 20;
        win.scroll_offset = 10;
        win.scroll_half_up(30);
        assert!(win.scroll_locked);
        assert_eq!(win.scroll_locked_cursor, win.cursor_row);

        // scroll_half_down
        win.scroll_locked = false;
        win.scroll_half_down(&buf, 30);
        assert!(win.scroll_locked);
        assert_eq!(win.scroll_locked_cursor, win.cursor_row);

        // scroll_center
        win.scroll_locked = false;
        win.scroll_center(30);
        assert!(win.scroll_locked);
        assert_eq!(win.scroll_locked_cursor, win.cursor_row);

        // scroll_cursor_top
        win.scroll_locked = false;
        win.scroll_cursor_top();
        assert!(win.scroll_locked);
        assert_eq!(win.scroll_locked_cursor, win.cursor_row);

        // scroll_cursor_bottom
        win.scroll_locked = false;
        win.scroll_cursor_bottom(30);
        assert!(win.scroll_locked);
        assert_eq!(win.scroll_locked_cursor, win.cursor_row);
    }
}
