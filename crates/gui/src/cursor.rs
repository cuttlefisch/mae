//! Cursor rendering for the GUI backend.
//!
//! Computes cursor position from editor state and draws mode-appropriate
//! cursor shapes using Skia. Uses `FrameLayout` as the single source of
//! truth for line positions and column scaling.
//!
//! Cursor pixel positions come from `FrameLayout`, which bakes in heading
//! scale from `syntax_spans`. No separate span parameter is needed here.

use mae_core::render_common::collab_cursor::{
    normalize_selection_range, offscreen_side, selection_col_range, OffscreenSide,
};
use mae_core::wrap::wrap_cursor_position;
use mae_core::{Editor, HighlightSpan, Mode};
use skia_safe::Color4f;

use crate::canvas::{CellRect, SkiaCanvas};
use crate::layout::FrameLayout;
use crate::theme;

/// Cursor shape varies by mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    /// Filled block (Normal/Visual mode).
    Block,
    /// Thin vertical bar (Insert mode).
    Bar,
}

/// Cursor position with optional heading scale info.
pub struct CursorPos {
    pub row: usize,
    pub col: usize,
    /// Exact pixel Y from the layout. None for Command/Search modes.
    pub pixel_y: Option<f32>,
    /// Exact pixel X for the cursor. When set, the cursor renderer uses this
    /// instead of `col * cell_width`. Computed using the font's actual glyph
    /// advance, which may differ from `scale * cell_width` due to grid-fitting.
    pub pixel_x: Option<f32>,
    /// Font scale at the cursor's line (1.0 for normal, >1.0 for org headings).
    pub scale: f32,
}

/// Compute the cursor's (row, col) screen position within a window area.
/// Uses `FrameLayout` for fold-aware Y and scale-aware X positioning.
/// Returns `None` if the cursor is outside the visible viewport.
pub fn compute_cursor_position(
    editor: &Editor,
    frame_layout: Option<&FrameLayout>,
    win_inner: CellRect,
    gutter_w: usize,
    _syntax_spans: Option<&[HighlightSpan]>,
) -> Option<CursorPos> {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    match editor.mode {
        Mode::Command => {
            let cursor_col = editor.vi.command_line
                [..editor.vi.command_cursor.min(editor.vi.command_line.len())]
                .chars()
                .count();
            Some(CursorPos {
                row: 0,
                col: 1 + cursor_col,
                pixel_y: None,
                pixel_x: None,
                scale: 1.0,
            })
        }
        Mode::Search => {
            let col = editor.search_input.len();
            Some(CursorPos {
                row: 0,
                col: 1 + col,
                pixel_y: None,
                pixel_x: None,
                scale: 1.0,
            })
        }
        _ => {
            // Normal/Insert/Visual — cursor in buffer content.
            let line_text = if win.cursor_row < buf.line_count() {
                let line = buf.rope().line(win.cursor_row);
                let s: String = line.chars().collect();
                s.trim_end_matches('\n').to_string()
            } else {
                String::new()
            };

            // When display regions are active, map cursor_col to display coordinates.
            let cursor_layout = frame_layout.and_then(|fl| fl.layout_for_row(win.cursor_row));
            let display_cursor_col =
                if let Some(dm) = cursor_layout.and_then(|ll| ll.display_map.as_ref()) {
                    mae_core::display_region::rope_col_to_display_col(win.cursor_col, dm)
                } else {
                    win.cursor_col
                };
            let display_line_text =
                if let Some(dc) = cursor_layout.and_then(|ll| ll.display_chars.as_ref()) {
                    dc.iter().collect::<String>()
                } else {
                    line_text.clone()
                };

            // Use FrameLayout for fold-aware positioning when available.
            if let Some(layout) = frame_layout {
                let text_width = if let Some(fl) = frame_layout {
                    fl.text_width
                } else {
                    win_inner.width.saturating_sub(gutter_w)
                };
                let wrap =
                    buf.local_options.word_wrap.unwrap_or(editor.word_wrap) && text_width > 0;

                // Look up the display row from the layout (fold-aware).
                let display_row = layout.display_row_of(win.cursor_row);
                let scale = layout.scale_for_row(win.cursor_row);
                let pix_y = layout.pixel_y_for_row(win.cursor_row);

                if wrap {
                    // For wrapped lines, find the wrap segment offset.
                    let show_break_w = editor.show_break.chars().count();
                    let (row_off, col) = wrap_cursor_position(
                        &display_line_text,
                        display_cursor_col,
                        text_width,
                        editor.break_indent,
                        show_break_w,
                    );

                    let base_display_row = display_row?;
                    let screen_row = base_display_row + row_off;

                    let indent_len = if editor.break_indent && row_off > 0 {
                        let chars: Vec<char> = display_line_text.chars().collect();
                        mae_core::wrap::content_indent_len(&chars)
                    } else {
                        0
                    };
                    let prefix_w = if row_off > 0 {
                        indent_len + show_break_w
                    } else {
                        0
                    };
                    let target = prefix_w + col;
                    let glyph_advance = layout.glyph_advance_for_row(win.cursor_row);
                    let scaled_col = FrameLayout::scaled_col(&display_line_text, target, scale);
                    // Compute pixel X using the font's actual glyph advance.
                    let pixel_x_abs = if scale != 1.0 {
                        let text_start_px = layout.text_col as f32 * layout.cell_width;
                        Some(
                            text_start_px
                                + FrameLayout::pixel_x_for_col(
                                    &display_line_text,
                                    target,
                                    glyph_advance,
                                ),
                        )
                    } else {
                        None
                    };

                    let actual_pixel_y = layout.pixel_y_for_display_row(screen_row);

                    // Viewport pixel clip: if scroll_pixel_offset pushes
                    // this row above the content area, hide the cursor.
                    let content_top = layout.area_row as f32 * layout.cell_height;
                    if let Some(py) = actual_pixel_y {
                        if py < content_top - 0.5 {
                            return None;
                        }
                    }

                    if screen_row < win_inner.height {
                        Some(CursorPos {
                            row: screen_row,
                            col: gutter_w + scaled_col,
                            pixel_y: actual_pixel_y,
                            pixel_x: pixel_x_abs,
                            scale,
                        })
                    } else {
                        None
                    }
                } else {
                    // No wrap: use layout's display_row directly (fold-aware).
                    let screen_row = display_row?;
                    // Compute column offset using FrameLayout::scaled_col.
                    let visible_start = win.col_offset;
                    let cursor_char_in_visible = display_cursor_col.saturating_sub(visible_start);
                    let visible_text: String = display_line_text
                        .chars()
                        .skip(visible_start)
                        .take(cursor_char_in_visible)
                        .collect();
                    let scaled_col =
                        FrameLayout::scaled_col(&visible_text, cursor_char_in_visible, scale);
                    let glyph_advance = layout.glyph_advance_for_row(win.cursor_row);
                    // Compute pixel X using the font's actual glyph advance.
                    let pixel_x_abs = if scale != 1.0 {
                        let text_start_px = layout.text_col as f32 * layout.cell_width;
                        Some(
                            text_start_px
                                + FrameLayout::pixel_x_for_col(
                                    &visible_text,
                                    cursor_char_in_visible,
                                    glyph_advance,
                                ),
                        )
                    } else {
                        None
                    };

                    if screen_row < win_inner.height {
                        Some(CursorPos {
                            row: screen_row,
                            col: gutter_w + scaled_col,
                            pixel_y: pix_y,
                            pixel_x: pixel_x_abs,
                            scale,
                        })
                    } else {
                        None
                    }
                }
            } else {
                // No layout available — fall back to simple calculation.
                let screen_row = win.cursor_row.saturating_sub(win.scroll_offset);
                let display_col =
                    mae_core::grapheme::display_width_up_to_grapheme(&line_text, win.cursor_col);
                let scroll_col =
                    mae_core::grapheme::display_width_up_to_grapheme(&line_text, win.col_offset);
                let visible_col = display_col.saturating_sub(scroll_col);

                if screen_row < win_inner.height {
                    Some(CursorPos {
                        row: screen_row,
                        col: gutter_w + visible_col,
                        pixel_y: None,
                        pixel_x: None,
                        scale: 1.0,
                    })
                } else {
                    None
                }
            }
        }
    }
}

/// Determine cursor shape for the current mode.
pub fn cursor_shape(editor: &Editor) -> CursorShape {
    match editor.mode {
        Mode::Insert | Mode::ConversationInput => CursorShape::Bar,
        _ => CursorShape::Block,
    }
}

/// Render the cursor onto the canvas at a pixel Y position.
/// `pixel_y` is the exact pixel Y from the FrameLayout. `pixel_x` is the exact
/// pixel X position. `scale` is the font scale at the cursor line.
pub fn render_cursor(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    pixel_y: f32,
    pixel_x: f32,
    scale: f32,
) {
    let cursor_style = editor.theme.style("ui.cursor");
    let cursor_bg = theme::color_or(cursor_style.bg, Color4f::new(0.9, 0.9, 0.9, 1.0));

    let (cw, ch) = canvas.cell_size();
    let shape = cursor_shape(editor);

    let scaled_ch = ch * scale;
    let cursor_cw = cw * scale;

    match shape {
        CursorShape::Block => {
            canvas.draw_pixel_rect(pixel_x, pixel_y, cursor_cw, scaled_ch, cursor_bg);
            let cursor_fg = theme::color_or(cursor_style.fg, Color4f::new(0.1, 0.1, 0.1, 1.0));
            let ch_under = match editor.mode {
                Mode::Command => {
                    let chars: Vec<char> = editor.vi.command_line.chars().collect();
                    chars.get(editor.vi.command_cursor).copied()
                }
                Mode::Search => None,
                _ => {
                    let win = editor.window_mgr.focused_window();
                    let buf = &editor.buffers[win.buffer_idx];
                    if win.cursor_row < buf.line_count() {
                        let line = buf.rope().line(win.cursor_row);
                        let line_str: String = line.chars().collect();
                        let chars: Vec<char> = line_str.trim_end_matches('\n').chars().collect();
                        chars.get(win.cursor_col).copied()
                    } else {
                        None
                    }
                }
            };
            if let Some(c) = ch_under {
                let bold = scale > 1.0;
                canvas.draw_char_at_pixel(pixel_x, pixel_y, c, cursor_fg, bold, scale);
            }
        }
        CursorShape::Bar => {
            canvas.draw_pixel_rect(pixel_x, pixel_y, 2.0, scaled_ch, cursor_bg);
        }
    }
}

/// Render thin vertical bars for all secondary cursors in the focused window.
/// `inner_row`/`inner_col` are the top-left cell coordinates of the buffer content area.
pub fn render_secondary_cursors(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    frame_layout: Option<&FrameLayout>,
    inner_row: usize,
    inner_col: usize,
    inner_height: usize,
    gutter_w: usize,
) {
    let win = editor.window_mgr.focused_window();
    if win.cursor_set.is_single() {
        return;
    }

    let sec_style = editor.theme.style("ui.cursor.secondary");
    let sec_color = theme::color_or(sec_style.bg, Color4f::new(0.6, 0.6, 0.9, 0.8));

    let (cw, ch) = canvas.cell_size();
    let buf = &editor.buffers[win.buffer_idx];

    for cursor in win.cursor_set.secondaries() {
        // Compute screen position for this secondary cursor.
        let screen_row = if let Some(layout) = frame_layout {
            match layout.display_row_of(cursor.row) {
                Some(r) => r,
                None => continue, // off-screen or folded
            }
        } else {
            let r = cursor.row.saturating_sub(win.scroll_offset);
            if r >= inner_height {
                continue;
            }
            r
        };

        if screen_row >= inner_height {
            continue;
        }

        let line_text = if cursor.row < buf.line_count() {
            let line = buf.rope().line(cursor.row);
            let s: String = line.chars().collect();
            s.trim_end_matches('\n').to_string()
        } else {
            String::new()
        };

        let visible_start = win.col_offset;
        let display_col = mae_core::grapheme::display_width_up_to_grapheme(&line_text, cursor.col)
            .saturating_sub(mae_core::grapheme::display_width_up_to_grapheme(
                &line_text,
                visible_start,
            ));

        let scale = frame_layout
            .map(|fl| fl.scale_for_row(cursor.row))
            .unwrap_or(1.0);

        let pixel_y = if let Some(layout) = frame_layout {
            layout
                .pixel_y_for_row(cursor.row)
                .unwrap_or((inner_row + screen_row) as f32 * ch)
        } else {
            (inner_row + screen_row) as f32 * ch
        };

        let pixel_x = (inner_col + gutter_w + display_col) as f32 * cw;
        let scaled_ch = ch * scale;

        // Draw thin vertical bar (2px wide).
        canvas.draw_pixel_rect(pixel_x, pixel_y, 2.0, scaled_ch, sec_color);
    }
}

/// Render remote collaborative cursors and labels within the visible viewport.
///
/// Draws a 2px-wide colored bar at each remote user's cursor position,
/// plus a username label above the cursor. Labels auto-hide after 3s.
/// Only draws cursors within the current viewport bounds.
pub fn render_remote_cursors(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    frame_layout: Option<&FrameLayout>,
    inner_row: usize,
    inner_col: usize,
    inner_height: usize,
    gutter_w: usize,
) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let doc_id = match &buf.collab_doc_id {
        Some(id) => id.as_str(),
        None => return,
    };

    let remote_users = editor.collab.remote_users.users_for_doc(doc_id);
    if remote_users.is_empty() {
        return;
    }

    let (cw, ch) = canvas.cell_size();
    let now = std::time::Instant::now();

    for user in &remote_users {
        // Compute screen row from frame layout (fold-aware).
        let screen_row = if let Some(layout) = frame_layout {
            match layout.display_row_of(user.cursor_row) {
                Some(r) => r,
                None => continue, // off-screen or folded
            }
        } else {
            let r = user.cursor_row.saturating_sub(win.scroll_offset);
            if r >= inner_height {
                continue;
            }
            r
        };

        if screen_row >= inner_height {
            continue;
        }

        // Get cursor color from theme.
        let color_key =
            mae_core::render_common::collab_colors::collab_cursor_style_key(user.color_index);
        let cursor_style = editor.theme.style(&color_key);
        let cursor_color = theme::color_or(cursor_style.fg, Color4f::new(0.8, 0.8, 0.8, 1.0));

        // Compute pixel position.
        let visible_col = user.cursor_col.saturating_sub(win.col_offset);
        let pixel_y = if let Some(layout) = frame_layout {
            layout
                .pixel_y_for_row(user.cursor_row)
                .unwrap_or((inner_row + screen_row) as f32 * ch)
        } else {
            (inner_row + screen_row) as f32 * ch
        };
        let pixel_x = (inner_col + gutter_w + visible_col) as f32 * cw;

        // Draw 2px thin bar (visually distinct from primary block cursor).
        canvas.draw_pixel_rect(pixel_x, pixel_y, 2.0, ch, cursor_color);

        // Draw username label above cursor (auto-hide after 3s of no movement).
        let elapsed = now.duration_since(user.last_seen).as_secs();
        if elapsed < 3 {
            let label_style = editor.theme.style("ui.collab.label");
            let label_color = theme::color_or(
                label_style.fg.or(cursor_style.fg),
                Color4f::new(1.0, 1.0, 1.0, 1.0),
            );
            let label_bg = cursor_color;

            // Draw label background + text above cursor.
            let label = &user.user_name;
            let label_width = label.len() as f32 * cw * 0.75; // slightly smaller font
            let label_height = ch * 0.8;
            let label_y = pixel_y - label_height - 2.0;

            if label_y >= 0.0 {
                canvas.draw_pixel_rect(pixel_x, label_y, label_width, label_height, label_bg);
                // Draw each character of the label.
                let mut char_x = pixel_x;
                for c in label.chars() {
                    canvas.draw_char_at_pixel(char_x, label_y, c, label_color, true, 0.75);
                    char_x += cw * 0.75;
                }
            }
        }
    }
}

/// Render remote users' selections (semi-transparent colored fills).
///
/// Draws selection spans with user's color at 20% opacity, BEFORE local
/// selection so remote selections appear underneath.
pub fn render_remote_selections(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    frame_layout: Option<&FrameLayout>,
    inner_row: usize,
    inner_col: usize,
    inner_height: usize,
    gutter_w: usize,
) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let doc_id = match &buf.collab_doc_id {
        Some(id) => id.as_str(),
        None => return,
    };

    let remote_users = editor.collab.remote_users.users_for_doc(doc_id);
    let (cw, ch) = canvas.cell_size();

    for user in &remote_users {
        let (start_row, start_col, end_row, end_col) = match user.selection {
            Some(sel) => sel,
            None => continue,
        };

        // Normalize selection direction.
        let ((sr, sc), (er, ec)) =
            normalize_selection_range((start_row, start_col), (end_row, end_col));

        let color_key =
            mae_core::render_common::collab_colors::collab_cursor_style_key(user.color_index);
        let cursor_style = editor.theme.style(&color_key);
        let base_color = theme::color_or(cursor_style.fg, Color4f::new(0.8, 0.8, 0.8, 1.0));
        // 20% opacity selection.
        let sel_color = Color4f::new(base_color.r, base_color.g, base_color.b, 0.2);

        for row in sr..=er {
            let screen_row = if let Some(layout) = frame_layout {
                match layout.display_row_of(row) {
                    Some(r) => r,
                    None => continue,
                }
            } else {
                let r = row.saturating_sub(win.scroll_offset);
                if r >= inner_height {
                    continue;
                }
                r
            };

            if screen_row >= inner_height {
                continue;
            }

            let (vis_start, vis_end) =
                selection_col_range(row, sr, sc, er, ec, buf.line_len(row), win.col_offset);
            let width = vis_end.saturating_sub(vis_start);

            if width == 0 {
                continue;
            }

            let pixel_y = if let Some(layout) = frame_layout {
                layout
                    .pixel_y_for_row(row)
                    .unwrap_or((inner_row + screen_row) as f32 * ch)
            } else {
                (inner_row + screen_row) as f32 * ch
            };
            let pixel_x = (inner_col + gutter_w + vis_start) as f32 * cw;

            canvas.draw_pixel_rect(pixel_x, pixel_y, width as f32 * cw, ch, sel_color);
        }
    }
}

/// Render off-screen indicators (▲/▼ arrows) for remote users whose cursors
/// are above or below the current viewport. Arrows are drawn at the top/bottom
/// edge of the gutter area, stacked horizontally, using each user's color.
pub fn render_remote_offscreen_indicators(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    frame_layout: Option<&FrameLayout>,
    inner_row: usize,
    inner_col: usize,
    inner_height: usize,
) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let doc_id = match &buf.collab_doc_id {
        Some(id) => id.as_str(),
        None => return,
    };

    let remote_users = editor.collab.remote_users.users_for_doc(doc_id);
    if remote_users.is_empty() {
        return;
    }

    let (cw, ch) = canvas.cell_size();
    let mut above: Vec<Color4f> = Vec::new();
    let mut below: Vec<Color4f> = Vec::new();

    for user in &remote_users {
        let fallback_side = offscreen_side(user.cursor_row, win.scroll_offset, inner_height);
        let is_above = if let Some(layout) = frame_layout {
            layout.display_row_of(user.cursor_row).is_none() && user.cursor_row < win.scroll_offset
        } else {
            fallback_side == Some(OffscreenSide::Above)
        };

        let is_below = if let Some(layout) = frame_layout {
            layout.display_row_of(user.cursor_row).is_none() && user.cursor_row >= win.scroll_offset
        } else {
            fallback_side == Some(OffscreenSide::Below)
        };

        let color_key =
            mae_core::render_common::collab_colors::collab_cursor_style_key(user.color_index);
        let cursor_style = editor.theme.style(&color_key);
        let color = theme::color_or(cursor_style.fg, Color4f::new(0.8, 0.8, 0.8, 1.0));

        if is_above {
            above.push(color);
        } else if is_below {
            below.push(color);
        }
    }

    // Draw ▲ at top-left of gutter, stacked horizontally.
    let top_y = inner_row as f32 * ch;
    for (i, &color) in above.iter().enumerate() {
        let x = (inner_col + i) as f32 * cw;
        canvas.draw_char_at_pixel(x, top_y, '▲', color, true, 1.0);
    }

    // Draw ▼ at bottom-left of gutter.
    let bottom_y = (inner_row + inner_height.saturating_sub(1)) as f32 * ch;
    for (i, &color) in below.iter().enumerate() {
        let x = (inner_col + i) as f32 * cw;
        canvas.draw_char_at_pixel(x, bottom_y, '▼', color, true, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout;
    use mae_core::wrap::char_width;

    #[test]
    fn cursor_shape_normal_is_block() {
        let editor = Editor::default();
        assert_eq!(cursor_shape(&editor), CursorShape::Block);
    }

    #[test]
    fn cursor_shape_insert_is_bar() {
        let editor = Editor {
            mode: Mode::Insert,
            ..Default::default()
        };
        assert_eq!(cursor_shape(&editor), CursorShape::Bar);
    }

    #[test]
    fn cursor_shape_visual_is_block() {
        let editor = Editor {
            mode: Mode::Visual(mae_core::VisualType::Char),
            ..Default::default()
        };
        assert_eq!(cursor_shape(&editor), CursorShape::Block);
    }

    #[test]
    fn compute_cursor_normal_mode() {
        let editor = Editor::default();
        let inner = CellRect::new(1, 1, 78, 22);
        let pos = compute_cursor_position(&editor, None, inner, 3, None);
        assert!(pos.is_some());
        let p = pos.unwrap();
        assert_eq!(p.row, 0);
        assert_eq!(p.col, 3); // gutter_w
    }

    #[test]
    #[allow(clippy::field_reassign_with_default)]
    fn compute_cursor_command_mode() {
        let mut editor = Editor::default();
        editor.mode = Mode::Command;
        editor.vi.command_line = "w".to_string();
        editor.vi.command_cursor = 1;
        let inner = CellRect::new(1, 1, 78, 22);
        let pos = compute_cursor_position(&editor, None, inner, 3, None);
        assert!(pos.is_some());
        let p = pos.unwrap();
        assert_eq!(p.col, 2); // ':' + 'w'
    }

    #[test]
    fn compute_cursor_no_extra_rows_without_layout() {
        // Without layout, cursor falls back to simple row calculation.
        let mut editor = Editor::default();
        let text: String = (0..10).map(|i| format!("line {}\n", i)).collect();
        editor.buffers[0].insert_text_at(0, &text);
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 5;
        win.cursor_col = 0;
        win.scroll_offset = 0;
        let inner = CellRect::new(0, 0, 80, 24);
        let pos = compute_cursor_position(&editor, None, inner, 3, None);
        assert!(pos.is_some());
        let p = pos.unwrap();
        assert_eq!(p.row, 5);
        assert_eq!(p.scale, 1.0);
    }

    #[test]
    fn compute_cursor_scale_default_is_one() {
        let editor = Editor::default();
        let inner = CellRect::new(0, 0, 80, 24);
        let pos = compute_cursor_position(&editor, None, inner, 3, None);
        assert!(pos.is_some());
        assert_eq!(pos.unwrap().scale, 1.0);
    }

    #[test]
    fn cursor_fold_aware_via_layout() {
        // With a fold, the layout skips folded lines. Cursor on a line after
        // the fold should have a display_row that accounts for the skip.
        let mut editor = Editor::new();
        editor.show_line_numbers = true;
        let idx = editor.active_buffer_idx();
        editor.buffers[idx].insert_text_at(0, "a\nb\nc\nd\ne\nf\ng\nh\n");
        // Fold lines 2-5 (line 1 is fold start)
        editor.buffers[idx].folded_ranges.push((1, 5));
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 6;
        win.cursor_col = 0;
        win.scroll_offset = 0;

        let buf = &editor.buffers[idx];
        let win = editor.window_mgr.focused_window();
        let fl = layout::compute_layout(&editor, buf, win, 0, 0, 80, 20, 16.0, 8.0, None, None);

        let inner = CellRect::new(0, 0, 80, 20);
        let gutter_w = fl.gutter_width;
        let pos = compute_cursor_position(&editor, Some(&fl), inner, gutter_w, None);
        assert!(pos.is_some());
        let p = pos.unwrap();
        // Visible lines: 0, 1(fold), 5, 6 → cursor on line 6 is display row 3
        assert_eq!(p.row, 3);
    }

    #[test]
    fn scaled_col_via_layout() {
        // Verify FrameLayout::scaled_col produces correct results.
        let line = "** Heading text";
        let scale = 1.3;
        let col5 = FrameLayout::scaled_col(line, 5, scale);
        let mut expected = 0.0f32;
        for ch in line.chars().take(5) {
            expected += char_width(ch) as f32 * scale;
        }
        assert_eq!(col5, expected.round() as usize);
    }

    #[test]
    fn scaled_col_at_zero_is_zero() {
        assert_eq!(FrameLayout::scaled_col("* Heading", 0, 1.5), 0);
    }

    #[test]
    fn cursor_conversation_input_follows_text() {
        // Simulate opening conversation buffer and typing.
        let mut editor = Editor::new();
        editor.ai_chat_enabled = true;
        editor.dispatch_builtin("ai-prompt");
        assert_eq!(editor.mode, Mode::ConversationInput);

        let pair = editor.ai.conversation_pair.as_ref().unwrap().clone();

        // Type "hi" into the input buffer.
        {
            let buf = &mut editor.buffers[pair.input_buffer_idx];
            let win = editor.window_mgr.focused_window_mut();
            buf.insert_char(win, 'h');
            buf.insert_char(win, 'i');
        }

        // Verify cursor advanced.
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_col, 2);
        assert_eq!(win.buffer_idx, pair.input_buffer_idx);

        // Compute layout for the input window.
        let buf = &editor.buffers[pair.input_buffer_idx];
        let fl = layout::compute_layout(&editor, buf, win, 0, 0, 80, 6, 16.0, 8.0, None, None);

        let gutter_w = fl.gutter_width;
        let inner = CellRect::new(0, 0, 80, 6);
        let pos = compute_cursor_position(&editor, Some(&fl), inner, gutter_w, None);
        assert!(
            pos.is_some(),
            "cursor position should be Some for input buffer"
        );
        let p = pos.unwrap();
        assert_eq!(p.row, 0);
        // Cursor col should be gutter + 2 chars of "hi".
        assert_eq!(p.col, gutter_w + 2);
        assert_eq!(p.scale, 1.0);
    }

    /// Regression: cursor pixel_x must use the same formula as text rendering
    /// pixel_offsets. Both use `char_width(ch) * glyph_advance` accumulated
    /// without intermediate rounding. This ensures cursor tracks text exactly
    /// on multi-run scaled lines (org headings with tags).
    #[test]
    fn cursor_and_text_pixel_x_agree() {
        // Simulate org heading: "* Section :tag:" with glyph_advance=13
        let line = "* Section :tag:";
        let glyph_advance = 13.0_f32;

        // Cursor uses pixel_x_for_col:
        for target_col in 0..line.len() {
            let cursor_px = FrameLayout::pixel_x_for_col(line, target_col, glyph_advance);

            // Text rendering uses the same accumulation in pixel_offsets:
            let text_px: f32 = line
                .chars()
                .take(target_col)
                .map(|ch| char_width(ch) as f32 * glyph_advance)
                .sum();

            assert_eq!(
                cursor_px, text_px,
                "col={}: cursor_px={} != text_px={}",
                target_col, cursor_px, text_px,
            );
        }
    }
}
