//! Unified text layout — single source of truth for line positions.
//!
//! `compute_layout()` runs once per frame per window, producing a `FrameLayout`
//! that both the renderer and cursor code consume. This eliminates the dual
//! coordinate system (pixel-Y accumulation vs independent cursor computation)
//! that caused cursor misalignment, fold annotation overlap, and fold-unaware
//! cursor positioning.
//!
//! **Invariant**: The `syntax_spans` passed to `compute_layout()` MUST be the
//! same slice passed to `render_buffer_content()` and used for cursor
//! positioning. Layout computes heading scale from `markup.heading` spans —
//! if the renderer sees different spans, line heights will not match.

use mae_core::wrap::{char_width, find_wrap_break, leading_indent_len};
use mae_core::{Buffer, Editor, HighlightSpan, Window};

use crate::buffer_render;
use crate::gutter;

/// Layout for one visible display row.
///
/// A single buffer line may produce multiple `LineLayout` entries if word-wrap
/// is active (one per wrap segment). The first segment has `is_wrap_continuation
/// = false`; subsequent segments have it set to `true`.
#[derive(Debug, Clone)]
pub struct LineLayout {
    /// Buffer line index.
    pub buf_row: usize,
    /// Exact pixel Y position of this display row.
    pub pixel_y: f32,
    /// Pixel height of this display row (scale * cell_height).
    pub line_height: f32,
    /// Heading scale factor (1.0 for normal lines).
    pub scale: f32,
    /// Actual per-glyph advance width in pixels for this line's font size.
    /// For scale=1.0 this equals cell_width; for scaled headings it's the
    /// font engine's grid-fitted advance at `base_size * scale`.
    pub glyph_advance: f32,
    /// True for 2nd+ segments of a wrapped line.
    pub is_wrap_continuation: bool,
    /// True if this line is the start of a folded range.
    pub is_fold_start: bool,
    /// Number of lines hidden by the fold (0 if not a fold start).
    pub folded_line_count: usize,
    /// Character index where this segment starts (0 for first/only segment).
    pub char_start: usize,
    /// Character count in this segment.
    pub char_count: usize,
}

/// Complete layout for one window's visible content area.
#[allow(dead_code)]
pub struct FrameLayout {
    /// One entry per visible display row (including wrap continuations).
    pub lines: Vec<LineLayout>,
    /// Gutter width in columns.
    pub gutter_width: usize,
    /// First text column (area_col + gutter_width).
    pub text_col: usize,
    /// Text area width in columns (area_width - gutter_width - scrollbar).
    pub text_width: usize,
    /// Total area width in columns.
    pub area_width: usize,
    /// Base cell width in pixels.
    pub cell_width: f32,
    /// Base cell height in pixels.
    pub cell_height: f32,
    /// Area row offset (for absolute screen positioning).
    pub area_row: usize,
    /// Area col offset.
    pub area_col: usize,
    /// Pixel Y limit (area_row + area_height) * cell_height.
    pub pixel_y_limit: f32,
    /// Scrollbar column (absolute), or None if scrollbar disabled.
    pub scrollbar_col: Option<usize>,
    /// Total buffer line count (for scrollbar thumb computation).
    pub total_lines: usize,
    /// Scroll offset (for scrollbar thumb position).
    pub scroll_offset: usize,
}

#[allow(dead_code)]
impl FrameLayout {
    /// Find the first LineLayout entry for a buffer row, if visible.
    pub fn layout_for_row(&self, buf_row: usize) -> Option<&LineLayout> {
        self.lines
            .iter()
            .find(|l| l.buf_row == buf_row && !l.is_wrap_continuation)
    }

    /// Display row index (0-based from top of viewport) for a buffer row.
    /// Returns the index of the first segment for that row.
    pub fn display_row_of(&self, buf_row: usize) -> Option<usize> {
        self.lines
            .iter()
            .position(|l| l.buf_row == buf_row && !l.is_wrap_continuation)
    }

    /// Pixel Y for a buffer row. Returns None if not visible.
    pub fn pixel_y_for_row(&self, buf_row: usize) -> Option<f32> {
        self.layout_for_row(buf_row).map(|l| l.pixel_y)
    }

    /// Scale factor for a buffer row. Returns 1.0 if not visible.
    pub fn scale_for_row(&self, buf_row: usize) -> f32 {
        self.layout_for_row(buf_row).map(|l| l.scale).unwrap_or(1.0)
    }

    /// Actual glyph advance for a buffer row. Returns cell_width if not visible.
    pub fn glyph_advance_for_row(&self, buf_row: usize) -> f32 {
        self.layout_for_row(buf_row)
            .map(|l| l.glyph_advance)
            .unwrap_or(self.cell_width)
    }

    /// Pixel Y for a display row index. Used by cursor rendering.
    pub fn pixel_y_for_display_row(&self, display_row: usize) -> Option<f32> {
        self.lines.get(display_row).map(|l| l.pixel_y)
    }

    /// Reverse lookup: given a display row index, return the buffer row.
    pub fn buf_row_for_display_row(&self, display_row: usize) -> Option<usize> {
        self.lines.get(display_row).map(|l| l.buf_row)
    }

    /// Compute scaled column offset for chars `0..target_col` at `scale`.
    ///
    /// This is THE single implementation of column offset computation.
    /// Both the renderer's `draw_styled_at` col_offsets and the cursor
    /// positioning call this function, eliminating divergence.
    pub fn scaled_col(line_text: &str, target_col: usize, scale: f32) -> usize {
        if scale == 1.0 {
            line_text.chars().take(target_col).map(char_width).sum()
        } else {
            let mut acc = 0.0f32;
            for (i, ch) in line_text.chars().enumerate() {
                if i >= target_col {
                    break;
                }
                acc += char_width(ch) as f32 * scale;
            }
            acc.round() as usize
        }
    }

    /// Exact fractional scaled column offset (no rounding).
    /// Returns value in cell-width units (multiply by cell_width for pixels).
    /// Used by cursor rendering to avoid column-grid quantization on scaled lines.
    pub fn scaled_col_precise(line_text: &str, target_col: usize, scale: f32) -> f32 {
        if scale == 1.0 {
            line_text
                .chars()
                .take(target_col)
                .map(|ch| char_width(ch) as f32)
                .sum()
        } else {
            line_text
                .chars()
                .take(target_col)
                .map(|ch| char_width(ch) as f32 * scale)
                .sum()
        }
    }

    /// Compute the exact pixel X offset for `target_col` chars using the
    /// font's actual glyph advance (not `scale * cell_width`).
    ///
    /// This is the CORRECT way to compute cursor position for scaled lines.
    /// Font engines grid-fit advances to integer pixels at each size, so
    /// `char_width * scale * cell_width` diverges from Skia's actual layout.
    pub fn pixel_x_for_col(line_text: &str, target_col: usize, glyph_advance: f32) -> f32 {
        line_text
            .chars()
            .take(target_col)
            .map(|ch| char_width(ch) as f32 * glyph_advance)
            .sum()
    }

    /// Convert pixel coordinates to a (buffer_row, char_col) position.
    ///
    /// Used by the GUI mouse handler to resolve clicks on scaled/folded lines
    /// without falling back to grid-based `pixel / cell_size` math.
    /// Returns `None` if the click is outside the text area.
    pub fn pixel_to_buffer_position(&self, pixel_x: f32, pixel_y: f32) -> Option<(usize, usize)> {
        // Find which display row the click falls into.
        let line = self
            .lines
            .iter()
            .find(|l| pixel_y >= l.pixel_y && pixel_y < l.pixel_y + l.line_height)?;

        // Compute the text-area pixel offset.
        let text_pixel_x = pixel_x - (self.text_col as f32 * self.cell_width);
        if text_pixel_x < 0.0 {
            return Some((line.buf_row, 0));
        }

        // Compute char column from pixel offset using glyph advance.
        // Monospace + scaled headings: uniform advance per char.
        let chars_from_start = (text_pixel_x / line.glyph_advance).floor() as usize;
        let char_col = line.char_start + chars_from_start.min(line.char_count);

        Some((line.buf_row, char_col))
    }

    /// Return all display row indices for a buffer row (including wrap continuations).
    pub fn display_rows_for(&self, buf_row: usize) -> Vec<usize> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.buf_row == buf_row)
            .map(|(i, _)| i)
            .collect()
    }
}

/// Compute the layout for a window's visible content area.
///
/// This extracts the layout-computing logic from `render_buffer_content()`:
/// scroll offset iteration, fold skipping, heading scale lookup, pixel Y
/// accumulation, and visible char range. No drawing is done here.
///
/// `glyph_advance_fn` maps a heading scale to the font's actual per-glyph
/// advance width at that scale. Font engines grid-fit advances to integer
/// pixels, so `cell_width * scale` is incorrect. Pass `None` in tests to
/// use the linear approximation (for logic-only tests that don't care about
/// pixel accuracy).
pub fn compute_layout(
    editor: &Editor,
    buf: &Buffer,
    win: &Window,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
    cell_height: f32,
    cell_width: f32,
    syntax_spans: Option<&[HighlightSpan]>,
    glyph_advance_fn: Option<&dyn Fn(f32) -> f32>,
) -> FrameLayout {
    let total_lines = buf.display_line_count();
    let gutter_w = if buf.kind == mae_core::BufferKind::Conversation {
        0
    } else if editor.show_line_numbers {
        gutter::gutter_width(buf.display_line_count())
    } else {
        2
    };
    let scrollbar_enabled = editor.scrollbar;
    let scrollbar_w = if scrollbar_enabled { 1 } else { 0 };
    let text_col = area_col + gutter_w;
    let text_width = area_width
        .saturating_sub(gutter_w)
        .saturating_sub(scrollbar_w);
    let scrollbar_col = if scrollbar_enabled {
        Some(text_col + text_width)
    } else {
        None
    };

    let wrap = buf.local_options.word_wrap.unwrap_or(editor.word_wrap) && text_width > 0;
    let show_break_width = if wrap {
        unicode_width::UnicodeWidthStr::width(editor.show_break.as_str())
    } else {
        0
    };

    let mut pixel_y = area_row as f32 * cell_height;
    let pixel_y_limit = (area_row + area_height) as f32 * cell_height;

    let mut lines: Vec<LineLayout> = Vec::with_capacity(area_height + 1);
    let mut line_idx = win.scroll_offset;

    // Narrow range: clamp to visible lines.
    let narrow = buf.narrowed_range;
    if let Some((ns, _)) = narrow {
        if line_idx < ns {
            line_idx = ns;
        }
    }

    while pixel_y < pixel_y_limit && line_idx < total_lines {
        // Skip lines outside narrowed range.
        if let Some((_, ne)) = narrow {
            if line_idx >= ne {
                break;
            }
        }

        // Skip folded lines.
        let mut is_folded = false;
        for (start, end) in &buf.folded_ranges {
            if line_idx > *start && line_idx < *end {
                is_folded = true;
                break;
            }
        }
        if is_folded {
            line_idx += 1;
            continue;
        }

        // Compute heading scale.
        let org_heading_scale = if editor.heading_scale {
            buffer_render::line_heading_scale(buf, syntax_spans, line_idx)
        } else {
            1.0
        };

        // Check if this line starts a fold.
        let fold_info = buf
            .folded_ranges
            .iter()
            .find(|(s, _)| *s == line_idx)
            .map(|(_, end)| end - line_idx - 1)
            .unwrap_or(0);
        let is_fold_start = fold_info > 0;

        let line_text = buf.rope().line(line_idx);
        let full_count = line_text
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .count();

        if wrap {
            let full_chars: Vec<char> = line_text
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();
            let indent_len = if editor.break_indent {
                leading_indent_len(&full_chars)
            } else {
                0
            };
            let cont_prefix_w = indent_len + show_break_width;
            let cont_text_w = if text_width > cont_prefix_w {
                text_width - cont_prefix_w
            } else {
                text_width
            };

            let mut pos = 0;
            let mut is_first = true;
            loop {
                if pixel_y >= pixel_y_limit {
                    break;
                }
                let seg_scale = if is_first { org_heading_scale } else { 1.0 };
                let seg_height = seg_scale * cell_height;

                // Don't emit lines whose bottom overflows the viewport.
                if pixel_y + seg_height > pixel_y_limit {
                    break;
                }

                let base_avail = if is_first { text_width } else { cont_text_w };
                let avail = if seg_scale > 1.0 {
                    (base_avail as f32 / seg_scale).floor() as usize
                } else {
                    base_avail
                };
                let end = find_wrap_break(&full_chars, pos, avail);

                let seg_glyph_advance = if seg_scale != 1.0 {
                    glyph_advance_fn
                        .map(|f| f(seg_scale))
                        .unwrap_or(cell_width * seg_scale)
                } else {
                    cell_width
                };
                lines.push(LineLayout {
                    buf_row: line_idx,
                    pixel_y,
                    line_height: seg_height,
                    scale: seg_scale,
                    glyph_advance: seg_glyph_advance,
                    is_wrap_continuation: !is_first,
                    is_fold_start: is_first && is_fold_start,
                    folded_line_count: if is_first { fold_info } else { 0 },
                    char_start: pos,
                    char_count: end - pos,
                });

                pixel_y += seg_height;
                is_first = false;
                pos = end;
                if pos >= full_count {
                    break;
                }
            }
        } else {
            // No wrap — single entry per line.
            let line_height = org_heading_scale * cell_height;

            // Don't emit lines whose bottom overflows the viewport.
            if pixel_y + line_height > pixel_y_limit {
                break;
            }

            let col_offset = win.col_offset;
            let visible_start = col_offset.min(full_count);

            // Compute visible char count (matching renderer's effective_width logic).
            let effective_width = if org_heading_scale > 1.0 {
                (text_width as f32 / org_heading_scale).floor() as usize
            } else {
                text_width
            };
            let full_chars: Vec<char> = line_text
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();
            let mut vis_width = 0;
            let mut visible_end = visible_start;
            for &ch in &full_chars[visible_start..] {
                let w = char_width(ch);
                if vis_width + w > effective_width {
                    break;
                }
                vis_width += w;
                visible_end += 1;
            }

            let line_glyph_advance = if org_heading_scale != 1.0 {
                glyph_advance_fn
                    .map(|f| f(org_heading_scale))
                    .unwrap_or(cell_width * org_heading_scale)
            } else {
                cell_width
            };
            lines.push(LineLayout {
                buf_row: line_idx,
                pixel_y,
                line_height,
                scale: org_heading_scale,
                glyph_advance: line_glyph_advance,
                is_wrap_continuation: false,
                is_fold_start,
                folded_line_count: fold_info,
                char_start: visible_start,
                char_count: visible_end - visible_start,
            });

            pixel_y += line_height;
        }

        line_idx += 1;
    }

    FrameLayout {
        lines,
        gutter_width: gutter_w,
        text_col,
        text_width,
        area_width,
        cell_width,
        cell_height,
        area_row,
        area_col,
        pixel_y_limit,
        scrollbar_col,
        total_lines: buf.display_line_count(),
        scroll_offset: win.scroll_offset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::Editor;

    fn make_editor(text: &str) -> Editor {
        let mut e = Editor::new();
        e.show_line_numbers = true;
        e.heading_scale = true;
        let idx = e.active_buffer_idx();
        e.buffers[idx].insert_text_at(0, text);
        e
    }

    #[test]
    fn layout_basic_no_folds() {
        // 5 lines + trailing newline = display_line_count() == 5 (phantom excluded)
        let e = make_editor("line 1\nline 2\nline 3\nline 4\nline 5\n");
        let idx = e.active_buffer_idx();
        let buf = &e.buffers[idx];
        let win = e.window_mgr.focused_window();
        let layout = compute_layout(&e, buf, win, 0, 0, 80, 20, 16.0, 8.0, None, None);
        assert_eq!(layout.lines.len(), 5);
        for (i, ll) in layout.lines.iter().enumerate() {
            assert_eq!(ll.buf_row, i);
            assert!(!ll.is_wrap_continuation);
            assert_eq!(ll.scale, 1.0);
            assert_eq!(ll.line_height, 16.0);
            assert_eq!(ll.pixel_y, i as f32 * 16.0);
        }
    }

    #[test]
    fn layout_with_fold() {
        let mut e = make_editor("a\nb\nc\nd\ne\nf\ng\nh\ni\nj\n");
        let idx = e.active_buffer_idx();
        // Fold lines 2-7 (line 1 is fold start, lines 2-6 hidden, line 7 is fold end)
        e.buffers[idx].folded_ranges.push((1, 7));
        let buf = &e.buffers[idx];
        let win = e.window_mgr.focused_window();
        let layout = compute_layout(&e, buf, win, 0, 0, 80, 20, 16.0, 8.0, None, None);
        // Visible: line 0, line 1 (fold start), line 7, 8, 9 (phantom excluded)
        assert_eq!(layout.lines.len(), 5);
        assert_eq!(layout.lines[0].buf_row, 0);
        assert_eq!(layout.lines[1].buf_row, 1);
        assert!(layout.lines[1].is_fold_start);
        assert_eq!(layout.lines[1].folded_line_count, 5);
        assert_eq!(layout.lines[2].buf_row, 7);
        assert_eq!(layout.lines[3].buf_row, 8);
        assert_eq!(layout.lines[4].buf_row, 9);
        // Pixel Y should be contiguous (no gaps for folded lines)
        for i in 0..layout.lines.len() {
            assert_eq!(layout.lines[i].pixel_y, i as f32 * 16.0);
        }
    }

    #[test]
    fn layout_display_row_of() {
        let mut e = make_editor("a\nb\nc\nd\ne\nf\ng\nh\n");
        let idx = e.active_buffer_idx();
        e.buffers[idx].folded_ranges.push((1, 5));
        let buf = &e.buffers[idx];
        let win = e.window_mgr.focused_window();
        let layout = compute_layout(&e, buf, win, 0, 0, 80, 20, 16.0, 8.0, None, None);
        // Visible: 0, 1(fold), 5, 6, 7
        assert_eq!(layout.display_row_of(0), Some(0));
        assert_eq!(layout.display_row_of(1), Some(1));
        assert_eq!(layout.display_row_of(2), None); // folded
        assert_eq!(layout.display_row_of(5), Some(2));
        assert_eq!(layout.display_row_of(6), Some(3));
    }

    #[test]
    fn layout_buf_row_for_display_row() {
        let mut e = make_editor("a\nb\nc\nd\ne\nf\ng\nh\n");
        let idx = e.active_buffer_idx();
        e.buffers[idx].folded_ranges.push((1, 5));
        let buf = &e.buffers[idx];
        let win = e.window_mgr.focused_window();
        let layout = compute_layout(&e, buf, win, 0, 0, 80, 20, 16.0, 8.0, None, None);
        assert_eq!(layout.buf_row_for_display_row(0), Some(0));
        assert_eq!(layout.buf_row_for_display_row(1), Some(1));
        assert_eq!(layout.buf_row_for_display_row(2), Some(5));
    }

    #[test]
    fn scaled_col_ascii() {
        // "## Hello" at scale 1.3, cursor at char 5
        let result = FrameLayout::scaled_col("## Hello", 5, 1.3);
        // Accumulated: 1.3 + 1.3 + 1.3 + 1.3 + 1.3 = 6.5 → round = 7
        assert_eq!(result, 7);
    }

    #[test]
    fn scaled_col_identity() {
        let result = FrameLayout::scaled_col("hello world", 5, 1.0);
        assert_eq!(result, 5);
    }

    #[test]
    fn scaled_col_empty() {
        assert_eq!(FrameLayout::scaled_col("", 0, 1.3), 0);
        assert_eq!(FrameLayout::scaled_col("abc", 0, 1.3), 0);
    }

    #[test]
    fn scaled_col_precise_no_rounding() {
        // "## Hello" at scale 1.3, cursor at char 5:
        // 5 * 1.3 = 6.5 exactly (no rounding).
        let result = FrameLayout::scaled_col_precise("## Hello", 5, 1.3);
        assert!((result - 6.5).abs() < 1e-6);
    }

    #[test]
    fn scaled_col_precise_identity() {
        let result = FrameLayout::scaled_col_precise("hello", 5, 1.0);
        assert!((result - 5.0).abs() < 1e-6);
    }

    #[test]
    fn scaled_col_precise_zero() {
        assert_eq!(FrameLayout::scaled_col_precise("abc", 0, 1.5), 0.0);
    }

    #[test]
    fn layout_no_overflow_scaled_heading() {
        // With a scaled heading at the bottom, the layout should not emit
        // a line whose bottom exceeds the pixel limit.
        let mut e = make_editor("line 1\nline 2\nline 3\nline 4\nline 5\n");
        e.heading_scale = true;
        let idx = e.active_buffer_idx();
        let buf = &e.buffers[idx];
        let win = e.window_mgr.focused_window();
        // Use a tight area: 3 rows * 16px = 48px limit.
        let layout = compute_layout(&e, buf, win, 0, 0, 80, 3, 16.0, 8.0, None, None);
        for ll in &layout.lines {
            assert!(
                ll.pixel_y + ll.line_height <= 48.0,
                "line at pixel_y={} height={} overflows limit 48.0",
                ll.pixel_y,
                ll.line_height
            );
        }
    }
}
