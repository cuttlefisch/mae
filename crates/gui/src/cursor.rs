//! Cursor rendering for the GUI backend.
//!
//! Computes cursor position from editor state and draws mode-appropriate
//! cursor shapes using Skia.

use mae_core::wrap::{wrap_cursor_position, wrap_line_display_rows};
use mae_core::{grapheme, Editor, Mode};
use skia_safe::Color4f;

use crate::canvas::{CellRect, SkiaCanvas};
use crate::theme;

/// Cursor shape varies by mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorShape {
    /// Filled block (Normal/Visual mode).
    Block,
    /// Thin vertical bar (Insert mode).
    Bar,
}

/// Compute the cursor's (row, col) screen position within a window area.
/// Returns `None` if the cursor is outside the visible viewport.
pub fn compute_cursor_position(
    editor: &Editor,
    win_inner: CellRect,
    gutter_w: usize,
) -> Option<(usize, usize)> {
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
            Some((0, 1 + cursor_col)) // relative to command area
        }
        Mode::Search => {
            let col = editor.search_input.len();
            Some((0, 1 + col))
        }
        Mode::ConversationInput => {
            if let Some(ref conv) = buf.conversation {
                if conv.scroll == 0 {
                    let cursor_byte = conv.input_cursor.min(conv.input_line.len());
                    let cursor_col = conv.input_line[..cursor_byte].chars().count();
                    let input_y = win_inner.height.saturating_sub(1);
                    return Some((input_y, 2 + cursor_col));
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
                    Some((screen_row, gutter_w + prefix_w + col))
                } else {
                    None
                }
            } else {
                let display_col =
                    grapheme::display_width_up_to_grapheme(&line_text, win.cursor_col);
                let screen_row = win.cursor_row.saturating_sub(win.scroll_offset);
                let scroll_col = grapheme::display_width_up_to_grapheme(&line_text, win.col_offset);
                let screen_col = gutter_w + display_col.saturating_sub(scroll_col);

                if screen_row < win_inner.height {
                    Some((screen_row, screen_col))
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

/// Render the cursor onto the canvas at an absolute (row, col) position.
pub fn render_cursor(canvas: &mut SkiaCanvas, editor: &Editor, abs_row: usize, abs_col: usize) {
    let cursor_style = editor.theme.style("ui.cursor");
    let cursor_bg = theme::color_or(cursor_style.bg, Color4f::new(0.9, 0.9, 0.9, 1.0));

    let (cw, ch) = canvas.cell_size();
    let shape = cursor_shape(editor);

    match shape {
        CursorShape::Block => {
            // Filled block.
            canvas.draw_pixel_rect(abs_col as f32 * cw, abs_row as f32 * ch, cw, ch, cursor_bg);
            // Draw the character under the cursor with inverted color.
            let win = editor.window_mgr.focused_window();
            let buf = &editor.buffers[win.buffer_idx];
            if win.cursor_row < buf.line_count() {
                let line = buf.rope().line(win.cursor_row);
                let line_str: String = line.chars().collect();
                let chars: Vec<char> = line_str.trim_end_matches('\n').chars().collect();
                if let Some(&ch_under) = chars.get(win.cursor_col) {
                    let cursor_fg =
                        theme::color_or(cursor_style.fg, Color4f::new(0.1, 0.1, 0.1, 1.0));
                    canvas.draw_text_at(abs_row, abs_col, &ch_under.to_string(), cursor_fg);
                }
            }
        }
        CursorShape::Bar => {
            // Thin vertical bar (2px wide) at the left edge of the cell.
            canvas.draw_pixel_rect(abs_col as f32 * cw, abs_row as f32 * ch, 2.0, ch, cursor_bg);
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
        let pos = compute_cursor_position(&editor, inner, 3);
        assert!(pos.is_some());
        let (row, col) = pos.unwrap();
        assert_eq!(row, 0);
        assert_eq!(col, 3); // gutter_w
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
        let pos = compute_cursor_position(&editor, inner, 3);
        assert!(pos.is_some());
        let (_, col) = pos.unwrap();
        assert_eq!(col, 2); // ':' + 'w'
    }
}
