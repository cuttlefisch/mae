//! Render/viewport computation: visual-row accounting, redraw-level
//! tracking, and cursor positioning extracted from `Editor`'s main impl
//! block. Split out of `mod.rs` (ADR none needed) — pure code motion, same
//! pattern as `kb_ops.rs`.

use crate::window::{Rect, WindowId};

use super::Editor;

impl Editor {
    /// Mark that only the cursor moved — reuse cached syntax spans.
    pub fn mark_cursor_moved(&mut self) {
        self.redraw_level = self
            .redraw_level
            .max(crate::redraw::RedrawLevel::CursorOnly);
    }

    /// Mark that the viewport scrolled.
    pub fn mark_scrolled(&mut self) {
        self.redraw_level = self.redraw_level.max(crate::redraw::RedrawLevel::Scroll);
    }

    /// Mark specific lines as dirty, merging with any existing dirty range.
    pub fn mark_lines_dirty(&mut self, start: usize, end: usize) {
        self.redraw_level = self
            .redraw_level
            .max(crate::redraw::RedrawLevel::PartialLines);
        self.dirty_line_range = Some(match self.dirty_line_range {
            Some((old_start, old_end)) => (old_start.min(start), old_end.max(end)),
            None => (start, end),
        });
    }

    /// Mark that a full redraw is needed (theme, resize, mode change, etc.).
    pub fn mark_full_redraw(&mut self) {
        self.redraw_level = crate::redraw::RedrawLevel::Full;
    }

    /// Reset redraw level after rendering. Called by the event loop after `render()`.
    pub fn clear_redraw(&mut self) {
        self.redraw_level = crate::redraw::RedrawLevel::None;
        self.dirty_line_range = None;
    }

    /// Per-window viewport height from cached layout. Falls back to global.
    pub fn window_viewport_height(&self, win_id: WindowId) -> usize {
        if self.last_layout_area.width > 0 && self.last_layout_area.height > 0 {
            let rects = self.window_mgr.layout_rects(self.last_layout_area);
            for (id, rect) in &rects {
                if *id == win_id && rect.height >= 3 {
                    return (rect.height as usize).saturating_sub(2); // status + border
                }
            }
        }
        self.viewport_height // fallback (startup, tests, zero-area)
    }

    /// Focused window's viewport height (convenience).
    pub fn focused_viewport_height(&self) -> usize {
        self.window_viewport_height(self.window_mgr.focused_id())
    }

    /// Single source of truth for how many visual cell-rows a buffer line occupies.
    ///
    /// Accounts for folds (0 rows), word wrap (>= 1 rows), and heading scale
    /// (ceil of scale factor). All scroll paths — `ensure_scroll_wrapped`,
    /// `scroll_up_line_wrapped`, mouse scroll bottom computation — must use this
    /// instead of computing visual rows independently, to prevent the scroll guard
    /// from fighting with scroll commands.
    pub fn line_visual_rows(&self, buf_idx: usize, line: usize) -> usize {
        let buf = &self.buffers[buf_idx];
        // Folded lines are invisible.
        if buf.is_line_folded(line) {
            return 0;
        }
        if line >= buf.rope().len_lines() {
            return 1;
        }

        // Check visual rows cache for text rows.
        let text_rows = if let Some(ref cache) = buf.visual_rows_cache {
            if cache.generation == buf.generation
                && cache.display_regions_gen == buf.display_regions_gen
                && cache.text_width == self.text_area_width
                && cache.break_indent == self.break_indent
                && cache.show_break_width == self.show_break.chars().count()
                && cache.heading_scale == self.heading_scale
                && line >= cache.line_start
                && line < cache.line_start + cache.rows.len()
            {
                let v = cache.rows[line - cache.line_start] as usize;
                if v > 0 {
                    Some(v)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let text_rows = text_rows.unwrap_or_else(|| self.line_text_visual_rows(buf_idx, line));

        // Account for inline image display height. Resolved via
        // `inline_images_for` (global + buffer-local override), not a
        // hardcoded `false` fallback that silently ignored the
        // `inline_images` global/`:set`/`:setl` entirely.
        let image_rows = if self.inline_images_for(buf_idx) {
            self.image_extra_rows(buf, line)
        } else {
            0
        };

        text_rows + image_rows
    }

    /// Compute the text-only visual rows for a line, applying display regions
    /// (link concealment) before wrapping — matching what `compute_layout()` does.
    fn line_text_visual_rows(&self, buf_idx: usize, line: usize) -> usize {
        let buf = &self.buffers[buf_idx];
        let rope = buf.rope();
        if line >= rope.len_lines() {
            return 1;
        }

        let line_slice = rope.line(line);
        let line_char_count = line_slice.len_chars();
        let line_byte_count = line_slice.len_bytes();
        // Content length excluding trailing newline.
        let content_len = if line_char_count > 0 && line_slice.char(line_char_count - 1) == '\n' {
            line_char_count - 1
        } else {
            line_char_count
        };

        // Fast path: no wrap, no heading scale, no display regions — skip char collection.
        // byte_len == char_count implies all single-byte (ASCII) chars, so
        // content_len == display_width.
        let is_ascii = line_byte_count == line_char_count;
        let has_display_regions = !buf.display_regions.is_empty();

        if !self.word_wrap_for(buf_idx) || self.text_area_width == 0 {
            // Still need heading rows for non-wrapped headings.
            if !self.heading_scale {
                return 1;
            }
            // Only collect chars for heading detection.
            let rope_chars: Vec<char> = line_slice
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();
            let heading_level = crate::heading::heading_level_from_chars(&rope_chars);
            if heading_level == 0 {
                return 1;
            }
            return crate::heading::heading_scale_for_level_with(
                heading_level,
                self.heading_scale_h1,
                self.heading_scale_h2,
                self.heading_scale_h3,
            )
            .ceil() as usize;
        }

        // Fast path: short ASCII line, no heading scale, no display regions →
        // guaranteed to fit in one row, skip all allocation.
        if is_ascii
            && content_len <= self.text_area_width
            && !self.heading_scale
            && !has_display_regions
        {
            return 1;
        }

        let rope_chars: Vec<char> = line_slice
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();

        let heading_level = crate::heading::heading_level_from_chars(&rope_chars);
        let heading_scale_factor = if self.heading_scale && heading_level > 0 {
            crate::heading::heading_scale_for_level_with(
                heading_level,
                self.heading_scale_h1,
                self.heading_scale_h2,
                self.heading_scale_h3,
            )
        } else {
            1.0
        };
        let heading_rows = heading_scale_factor.ceil() as usize;

        // Apply display regions (link concealment) to match compute_layout() behavior.
        let effective_regions = crate::display_region::regions_with_cursor_reveal(
            &buf.display_regions,
            buf.display_reveal_cursor,
        );

        let line_byte_start = rope.line_to_byte(line);
        let next_line_byte = if line + 1 < rope.len_lines() {
            rope.line_to_byte(line + 1)
        } else {
            rope.len_bytes()
        };

        // Check if any display regions overlap this line (binary search).
        let start_idx = effective_regions.partition_point(|r| r.byte_end <= line_byte_start);
        let has_regions = effective_regions
            .get(start_idx)
            .is_some_and(|r| r.byte_start < next_line_byte);

        let chars_for_wrap = if has_regions {
            let (display_chars, _) = crate::display_region::apply_display_regions_to_line(
                &rope_chars,
                line_byte_start,
                next_line_byte,
                &effective_regions,
            );
            display_chars
        } else {
            rope_chars
        };

        // For headings with scale > 1, reduce wrap width to match GUI layout.
        // compute_layout() does: (text_area_width / scale).floor()
        let wrap_width = if heading_scale_factor > 1.0 {
            (self.text_area_width as f32 / heading_scale_factor).floor() as usize
        } else {
            self.text_area_width
        };

        let text: String = chars_for_wrap.iter().collect();
        let sb_w = self.show_break.chars().count();
        let wrap_rows =
            crate::wrap::wrap_line_display_rows(&text, wrap_width, self.break_indent, sb_w);

        // Heading wrap correctness: first wrap segment gets heading scale,
        // continuation rows are normal height. Total cell rows =
        // heading_rows (ceil of scale) + (wrap_count - 1) continuation rows.
        (wrap_rows - 1) + heading_rows
    }

    /// Pre-compute visual row counts for a contiguous line range and store in the
    /// buffer's cache. Subsequent `line_visual_rows()` calls hit the cache.
    /// Pre-compute visual row counts for a contiguous needed range and store
    /// in the buffer's cache.
    ///
    /// **Fix A**: The cache is checked against the `needed_start..needed_end`
    /// range, but on miss it computes a wider padded range to absorb future
    /// scroll shifts without re-computation.
    pub fn populate_visual_rows_cache(
        &mut self,
        buf_idx: usize,
        needed_start: usize,
        needed_end: usize,
    ) {
        let buf = &self.buffers[buf_idx];
        let gen = buf.generation;
        let dr_gen = buf.display_regions_gen;
        let text_width = self.text_area_width;
        let break_indent = self.break_indent;
        let sb_w = self.show_break.chars().count();
        let hs = self.heading_scale;

        // Check if existing cache covers the NEEDED range (not padded).
        if let Some(ref cache) = buf.visual_rows_cache {
            if cache.generation == gen
                && cache.display_regions_gen == dr_gen
                && cache.text_width == text_width
                && cache.break_indent == break_indent
                && cache.show_break_width == sb_w
                && cache.heading_scale == hs
                && cache.line_start <= needed_start
                && cache.line_start + cache.rows.len() >= needed_end
            {
                self.perf_stats.visual_rows_cache_hits += 1;
                return;
            }
        }
        self.perf_stats.visual_rows_cache_misses += 1;

        // Miss — compute with padding to absorb future scroll shifts.
        let total = buf.display_line_count();
        let pad = self.focused_viewport_height();
        let compute_start = needed_start.saturating_sub(pad);
        let compute_end = (needed_end + pad).min(total);

        let mut rows = Vec::with_capacity(compute_end.saturating_sub(compute_start));
        for line in compute_start..compute_end {
            let r = self.line_text_visual_rows(buf_idx, line);
            rows.push(r.min(255) as u8);
        }

        self.buffers[buf_idx].visual_rows_cache = Some(crate::buffer::VisualRowsCache {
            generation: gen,
            display_regions_gen: dr_gen,
            text_width,
            break_indent,
            show_break_width: sb_w,
            heading_scale: hs,
            line_start: compute_start,
            rows,
        });
    }

    /// Estimate extra visual rows consumed by an inline image on this line.
    /// Uses the same sizing logic as GUI layout (MAX_H=400, aspect-ratio fit).
    fn image_extra_rows(&self, buf: &crate::buffer::Buffer, line: usize) -> usize {
        let rope = buf.rope();
        if line >= rope.len_lines() {
            return 0;
        }
        let line_byte_start = rope.line_to_byte(line);
        let line_byte_end = if line + 1 < rope.len_lines() {
            rope.line_to_byte(line + 1)
        } else {
            rope.len_bytes()
        };
        for region in &buf.display_regions {
            if region.byte_start >= line_byte_end {
                break;
            }
            if region.byte_end <= line_byte_start {
                continue;
            }
            if let Some(ref img) = region.image {
                // Mirror GUI layout sizing: MAX_H=400, text_area_width for max_w.
                // Use actual cell dimensions pushed by the GUI (or 8.0/16.0 defaults).
                let text_area_px = (self.text_area_width as f32) * self.gui_cell_width;
                let max_w = if let Some(w) = img.width {
                    (w as f32).min(text_area_px)
                } else {
                    text_area_px
                };
                const MAX_H: f32 = 400.0;
                // Use cached dimensions from ImageAttrs (populated at region creation time).
                let (img_w, img_h) = if img.natural_width > 0 && img.natural_height > 0 {
                    (img.natural_width as f32, img.natural_height as f32)
                } else {
                    (max_w, max_w)
                };
                let display_h = if img_w > 0.0 && img_h > 0.0 {
                    let h = max_w / (img_w / img_h);
                    h.min(MAX_H)
                } else {
                    max_w.min(MAX_H)
                };
                let cell_h = self.gui_cell_height;
                return (display_h / cell_h).ceil() as usize;
            }
        }
        0
    }

    /// Calculate the actual inner height (text rows) for the focused window.
    /// This accounts for the window manager layout AND window borders.
    pub fn focused_window_viewport_height(&self, total_area: Rect) -> usize {
        let rects = self.window_mgr.layout_rects(total_area);
        let focused_id = self.window_mgr.focused_id();
        if let Some((_, rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
            // Every window currently has a top and bottom border (2 rows total).
            (rect.height as usize).saturating_sub(2)
        } else {
            (total_area.height as usize).saturating_sub(2)
        }
    }

    /// Handle a mouse click at the given cell coordinates.
    ///
    /// Left-click places the cursor, adjusting for gutter width and scroll offset.
    /// Middle-click pastes from the default register. Right-click is reserved for
    /// future context menu support.
    /// Set cursor position directly from buffer (row, col) coordinates.
    /// Used by the GUI mouse handler when FrameLayout-based pixel positioning
    /// is available (bypasses scroll/gutter arithmetic).
    pub fn set_cursor_position(&mut self, buf_row: usize, char_col: usize) {
        let win = self.window_mgr.focused_window();
        let buf = &self.buffers[win.buffer_idx];
        let max_row = buf.display_line_count().saturating_sub(1);
        let target_row = buf_row.min(max_row);
        let line_len = buf.line_len(target_row);
        let target_col = char_col.min(if line_len > 0 { line_len - 1 } else { 0 });
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = target_row;
        win.cursor_col = target_col;
    }
}
