//! Cursor rendering for the GUI backend.
//!
//! Computes cursor position from editor state and draws mode-appropriate
//! cursor shapes using Skia.

use mae_core::wrap::{char_width, wrap_cursor_position, wrap_line_display_rows};
use mae_core::{grapheme, Editor, HighlightSpan, Mode};
use skia_safe::Color4f;

use crate::buffer_render;
use crate::canvas::{CellRect, SkiaCanvas};
use crate::theme;

/// Accumulate per-char scaled column offset up to `target_col` chars.
/// This matches `draw_styled_at`'s `col_offsets` logic exactly, avoiding
/// the rounding divergence that `(col * scale).round()` produces.
fn accumulated_scaled_col(line: &str, target_col: usize, scale: f32) -> usize {
    let mut acc = 0.0f32;
    for (i, ch) in line.chars().enumerate() {
        if i >= target_col {
            break;
        }
        acc += char_width(ch) as f32 * scale;
    }
    acc.round() as usize
}

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
    /// Font scale at the cursor's line (1.0 for normal, >1.0 for org headings).
    pub scale: f32,
}

/// Compute the cursor's (row, col) screen position within a window area.
/// Returns `None` if the cursor is outside the visible viewport.
pub fn compute_cursor_position(
    editor: &Editor,
    win_inner: CellRect,
    gutter_w: usize,
    syntax_spans: Option<&[HighlightSpan]>,
) -> Option<CursorPos> {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    match editor.mode {
        Mode::Command => {
            let cursor_col = editor.command_line
                [..editor.command_cursor.min(editor.command_line.len())]
                .chars()
                .count();
            // Command line cursor is handled separately in render_command_line.
            // Return None here; it's drawn on the command row.
            Some(CursorPos {
                row: 0,
                col: 1 + cursor_col,
                scale: 1.0,
            })
        }
        Mode::Search => {
            let col = editor.search_input.len();
            Some(CursorPos {
                row: 0,
                col: 1 + col,
                scale: 1.0,
            })
        }
        Mode::ConversationInput => {
            if let Some(ref conv) = buf.conversation {
                if conv.scroll == 0 {
                    let cursor_byte = conv.input_cursor.min(conv.input_line.len());
                    let cursor_col = conv.input_line[..cursor_byte].chars().count();
                    let input_y = win_inner.height.saturating_sub(1);
                    return Some(CursorPos {
                        row: input_y,
                        col: 2 + cursor_col,
                        scale: 1.0,
                    });
                }
            }
            None
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

            let text_width = win_inner.width.saturating_sub(gutter_w);
            let wrap = editor.word_wrap && text_width > 0;

            if wrap {
                let show_break_w = editor.show_break.chars().count();
                // Count display rows consumed by lines before the cursor line.
                let mut screen_row = 0;
                for ln in win.scroll_offset..win.cursor_row {
                    if ln < buf.line_count() {
                        let line = buf.rope().line(ln);
                        let lt: String = line.chars().collect();
                        screen_row += wrap_line_display_rows(
                            lt.trim_end_matches('\n'),
                            text_width,
                            editor.break_indent,
                            show_break_w,
                        );
                    }
                }

                // Row/col within the wrapped cursor line.
                let (row_off, col) = wrap_cursor_position(
                    &line_text,
                    win.cursor_col,
                    text_width,
                    editor.break_indent,
                    show_break_w,
                );
                screen_row += row_off;

                let line_scale = if editor.heading_scale {
                    buffer_render::line_heading_scale(buf, syntax_spans, win.cursor_row)
                } else {
                    1.0
                };
                if screen_row < win_inner.height {
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
                    // Scale column offset to match heading text positioning.
                    // Accumulate per-char widths (same as draw_styled_at col_offsets)
                    // to avoid rounding divergence from `(col * scale).round()`.
                    let scaled_col = if line_scale != 1.0 {
                        accumulated_scaled_col(&line_text, prefix_w + col, line_scale)
                    } else {
                        prefix_w + col
                    };
                    Some(CursorPos {
                        row: screen_row,
                        col: gutter_w + scaled_col,
                        scale: line_scale,
                    })
                } else {
                    None
                }
            } else {
                let display_col =
                    grapheme::display_width_up_to_grapheme(&line_text, win.cursor_col);
                let screen_row = win.cursor_row.saturating_sub(win.scroll_offset);
                let scroll_col = grapheme::display_width_up_to_grapheme(&line_text, win.col_offset);
                let line_scale = if editor.heading_scale {
                    buffer_render::line_heading_scale(buf, syntax_spans, win.cursor_row)
                } else {
                    1.0
                };
                // Scale column offset using per-char accumulation (matches draw_styled_at).
                let visible_col = display_col.saturating_sub(scroll_col);
                let scaled_col = if line_scale != 1.0 {
                    // Accumulate from the scroll-visible portion of the line
                    let visible_start = win.col_offset;
                    let visible_end = visible_start + win.cursor_col;
                    let visible_text: String = line_text
                        .chars()
                        .skip(visible_start)
                        .take(visible_end - visible_start)
                        .collect();
                    accumulated_scaled_col(&visible_text, visible_col, line_scale)
                } else {
                    visible_col
                };
                let screen_col = gutter_w + scaled_col;

                if screen_row < win_inner.height {
                    Some(CursorPos {
                        row: screen_row,
                        col: screen_col,
                        scale: line_scale,
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
/// `pixel_y` is the exact pixel Y from the PixelYMap. `abs_col` is cell-based.
/// `scale` is the font scale at the cursor line (1.0 for normal, >1.0 for headings).
pub fn render_cursor(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    pixel_y: f32,
    abs_col: usize,
    scale: f32,
) {
    let cursor_style = editor.theme.style("ui.cursor");
    let cursor_bg = theme::color_or(cursor_style.bg, Color4f::new(0.9, 0.9, 0.9, 1.0));

    let (cw, ch) = canvas.cell_size();
    let shape = cursor_shape(editor);

    // Cursor x uses unscaled cell width (same as canvas _at_y methods).
    // Height is scaled to match the heading line height.
    let scaled_ch = ch * scale;
    let pixel_x = abs_col as f32 * cw;
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
                // Draw character under cursor with the same style (bold/italic)
                // as the underlying text to avoid misalignment on headings.
                let bold = scale > 1.0; // org headings are bold
                canvas.draw_char_at_y(pixel_y, abs_col, c, cursor_fg, bold, false, scale);
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
        let pos = compute_cursor_position(&editor, inner, 3, None);
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
        let pos = compute_cursor_position(&editor, inner, 3, None);
        assert!(pos.is_some());
        let p = pos.unwrap();
        assert_eq!(p.col, 2); // ':' + 'w'
    }

    #[test]
    fn compute_cursor_no_extra_rows_without_spans() {
        // Without syntax spans, no heading scaling — cursor row should be
        // a simple offset from scroll_offset, with no heading_extra_rows added.
        let mut editor = Editor::default();
        let text: String = (0..10).map(|i| format!("line {}\n", i)).collect();
        editor.buffers[0].insert_text_at(0, &text);
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 5;
        win.cursor_col = 0;
        win.scroll_offset = 0;
        let inner = CellRect::new(0, 0, 80, 24);
        let pos = compute_cursor_position(&editor, inner, 3, None);
        assert!(pos.is_some());
        let p = pos.unwrap();
        // Row should be exactly cursor_row - scroll_offset = 5.
        assert_eq!(p.row, 5);
        assert_eq!(p.scale, 1.0);
    }

    #[test]
    fn compute_cursor_scale_default_is_one() {
        // Without syntax spans, scale should always be 1.0.
        let editor = Editor::default();
        let inner = CellRect::new(0, 0, 80, 24);
        let pos = compute_cursor_position(&editor, inner, 3, None);
        assert!(pos.is_some());
        assert_eq!(pos.unwrap().scale, 1.0);
    }

    #[test]
    fn accumulated_scaled_col_matches_draw_styled_at() {
        // Verify that accumulated_scaled_col produces the same offsets
        // as draw_styled_at's col_offsets logic for scaled headings.
        let line = "** Heading text";
        let scale = 1.3;
        // Each ASCII char has width 1, so accumulated offset at col N
        // should be round(sum of 1.0*scale for each char up to N).
        let col5 = accumulated_scaled_col(line, 5, scale);
        // Manual: 5 * 1.3 = 6.5, round = 7 (but accumulated:
        // 1.3->1, 2.6->3, 3.9->4, 5.2->5, 6.5->7)
        // The per-char accumulation rounds the running total, not each step.
        let mut expected = 0.0f32;
        for ch in line.chars().take(5) {
            expected += char_width(ch) as f32 * scale;
        }
        assert_eq!(col5, expected.round() as usize);
    }

    #[test]
    fn accumulated_scaled_col_at_zero_is_zero() {
        assert_eq!(accumulated_scaled_col("* Heading", 0, 1.5), 0);
    }
}
