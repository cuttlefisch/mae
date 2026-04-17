//! Cursor positioning logic.

use mae_core::{grapheme, Editor, Mode};
use ratatui::prelude::*;

use crate::buffer_render::gutter_width;

/// Compute and set the terminal cursor position for the current mode.
pub(crate) fn set_cursor(frame: &mut Frame, editor: &Editor, window_area: Rect, cmd_area: Rect) {
    let focused_win = editor.window_mgr.focused_window();
    let focused_buf = &editor.buffers[focused_win.buffer_idx];

    let wa = mae_core::WinRect {
        x: window_area.x,
        y: window_area.y,
        width: window_area.width,
        height: window_area.height,
    };
    let rects = editor.window_mgr.layout_rects(wa);
    let focused_id = editor.window_mgr.focused_id();

    if let Some((_, win_rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
        let rr = Rect::new(win_rect.x, win_rect.y, win_rect.width, win_rect.height);
        let inner = inner_rect(rr);
        let gutter_w = gutter_width(focused_buf.line_count());

        if editor.mode == Mode::Command {
            let cursor_col = editor.command_line
                [..editor.command_cursor.min(editor.command_line.len())]
                .chars()
                .count() as u16;
            frame.set_cursor_position(Position::new(cmd_area.x + 1 + cursor_col, cmd_area.y));
        } else if editor.mode == Mode::Search {
            frame.set_cursor_position(Position::new(
                cmd_area.x + 1 + editor.search_input.len() as u16,
                cmd_area.y,
            ));
        } else if editor.mode == Mode::ConversationInput {
            if let Some(ref conv) = focused_buf.conversation {
                if conv.scroll == 0 {
                    let cursor_byte = conv.input_cursor.min(conv.input_line.len());
                    let cursor_col = conv.input_line[..cursor_byte].chars().count() as u16;
                    let input_x = inner.x + 2 + cursor_col;
                    let input_y = inner.y + inner.height.saturating_sub(1);
                    frame.set_cursor_position(Position::new(input_x, input_y));
                }
            }
        } else {
            let screen_row = focused_win
                .cursor_row
                .saturating_sub(focused_win.scroll_offset) as u16;
            let line_text = if focused_win.cursor_row < focused_buf.line_count() {
                let line = focused_buf.rope().line(focused_win.cursor_row);
                let s: String = line.chars().collect();
                s.trim_end_matches('\n').to_string()
            } else {
                String::new()
            };
            let display_col =
                grapheme::display_width_up_to_grapheme(&line_text, focused_win.cursor_col);
            let scroll_col =
                grapheme::display_width_up_to_grapheme(&line_text, focused_win.col_offset);
            let screen_col = gutter_w as u16 + (display_col.saturating_sub(scroll_col)) as u16;
            if screen_row < inner.height {
                frame
                    .set_cursor_position(Position::new(inner.x + screen_col, inner.y + screen_row));
            }
        }
    }
}

pub(crate) fn inner_rect(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}
