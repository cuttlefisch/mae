//! Cursor rendering for the GUI backend.
//!
//! Computes cursor position from editor state and draws mode-appropriate
//! cursor shapes using Skia. Uses `FrameLayout` as the single source of
//! truth for line positions and column scaling.

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
            let cursor_col = editor.command_line
                [..editor.command_cursor.min(editor.command_line.len())]
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

            // Use FrameLayout for fold-aware positioning when available.
            if let Some(layout) = frame_layout {
                let text_width = if let Some(fl) = frame_layout {
                    fl.text_width
                } else {
                    win_inner.width.saturating_sub(gutter_w)
                };
                let wrap = editor.word_wrap && text_width > 0;

                // Look up the display row from the layout (fold-aware).
                let display_row = layout.display_row_of(win.cursor_row);
                let scale = layout.scale_for_row(win.cursor_row);
                let pix_y = layout.pixel_y_for_row(win.cursor_row);

                if wrap {
                    // For wrapped lines, find the wrap segment offset.
                    let show_break_w = editor.show_break.chars().count();
                    let (row_off, col) = wrap_cursor_position(
                        &line_text,
                        win.cursor_col,
                        text_width,
                        editor.break_indent,
                        show_break_w,
                    );

                    let base_display_row = display_row?;
                    let screen_row = base_display_row + row_off;

                    let indent_len = if editor.break_indent && row_off > 0 {
                        let chars: Vec<char> = line_text.chars().collect();
                        mae_core::wrap::leading_indent_len(&chars)
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
                    let scaled_col = FrameLayout::scaled_col(&line_text, target, scale);
                    // Compute pixel X using the font's actual glyph advance.
                    let pixel_x_abs = if scale != 1.0 {
                        let text_start_px = layout.text_col as f32 * layout.cell_width;
                        Some(
                            text_start_px
                                + FrameLayout::pixel_x_for_col(&line_text, target, glyph_advance),
                        )
                    } else {
                        None
                    };

                    let actual_pixel_y = layout.pixel_y_for_display_row(screen_row);

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
                    let cursor_char_in_visible = win.cursor_col.saturating_sub(visible_start);
                    let visible_text: String = line_text
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
                    let chars: Vec<char> = editor.command_line.chars().collect();
                    chars.get(editor.command_cursor).copied()
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
    fn compute_cursor_command_mode() {
        let editor = Editor {
            mode: Mode::Command,
            command_line: "w".to_string(),
            command_cursor: 1,
            ..Default::default()
        };
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
        editor.dispatch_builtin("ai-prompt");
        assert_eq!(editor.mode, Mode::ConversationInput);

        let pair = editor.conversation_pair.as_ref().unwrap().clone();

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
