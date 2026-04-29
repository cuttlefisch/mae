//! Cursor positioning logic.

use mae_core::{grapheme, Editor, Mode};
use ratatui::prelude::*;

use mae_core::wrap::{wrap_cursor_position, wrap_line_display_rows};

use mae_core::render_common::gutter::gutter_width;

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
        let gutter_w = if focused_buf.kind == mae_core::BufferKind::Conversation {
            0
        } else if editor.show_line_numbers {
            gutter_width(focused_buf.display_line_count())
        } else {
            2
        };

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
            // ConversationInput: cursor is in the *ai-input* Text buffer.
            let line_text = if focused_win.cursor_row < focused_buf.line_count() {
                let line = focused_buf.rope().line(focused_win.cursor_row);
                let s: String = line.chars().collect();
                s.trim_end_matches('\n').to_string()
            } else {
                String::new()
            };
            let display_col =
                grapheme::display_width_up_to_grapheme(&line_text, focused_win.cursor_col);
            let cursor_x = inner.x + gutter_w as u16 + display_col as u16;
            let cursor_y = inner.y
                + focused_win
                    .cursor_row
                    .saturating_sub(focused_win.scroll_offset) as u16;
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        } else {
            let line_text = if focused_win.cursor_row < focused_buf.line_count() {
                let line = focused_buf.rope().line(focused_win.cursor_row);
                let s: String = line.chars().collect();
                s.trim_end_matches('\n').to_string()
            } else {
                String::new()
            };
            let display_col =
                grapheme::display_width_up_to_grapheme(&line_text, focused_win.cursor_col);

            let text_width = inner.width.saturating_sub(gutter_w as u16) as usize;
            let wrap = editor.word_wrap && text_width > 0;

            let show_break_w = editor.show_break.chars().count();

            if wrap {
                // Count display rows consumed by lines before the cursor line.
                let mut screen_row: u16 = 0;
                for ln in focused_win.scroll_offset..focused_win.cursor_row {
                    if ln < focused_buf.line_count() {
                        let line = focused_buf.rope().line(ln);
                        let lt: String = line.chars().collect();
                        let rows = wrap_line_display_rows(
                            lt.trim_end_matches('\n'),
                            text_width,
                            editor.break_indent,
                            show_break_w,
                        );
                        screen_row += rows as u16;
                    } else {
                        screen_row += 1;
                    }
                }
                // Add wrapped row/col offset within the cursor's own line.
                let (wrap_row, wrap_col) = wrap_cursor_position(
                    &line_text,
                    focused_win.cursor_col,
                    text_width,
                    editor.break_indent,
                    show_break_w,
                );
                screen_row += wrap_row as u16;
                // Continuation lines have indent+showbreak prefix.
                let col_prefix = if wrap_row > 0 {
                    let chars: Vec<char> = line_text.chars().collect();
                    let indent = if editor.break_indent {
                        chars
                            .iter()
                            .take_while(|c| **c == ' ' || **c == '\t')
                            .count()
                    } else {
                        0
                    };
                    indent + show_break_w
                } else {
                    0
                };
                let screen_col = gutter_w as u16 + col_prefix as u16 + wrap_col as u16;
                if screen_row < inner.height {
                    frame.set_cursor_position(Position::new(
                        inner.x + screen_col,
                        inner.y + screen_row,
                    ));
                }
            } else {
                let screen_row = focused_win
                    .cursor_row
                    .saturating_sub(focused_win.scroll_offset)
                    as u16;
                let scroll_col =
                    grapheme::display_width_up_to_grapheme(&line_text, focused_win.col_offset);
                let screen_col = gutter_w as u16 + (display_col.saturating_sub(scroll_col)) as u16;
                if screen_row < inner.height {
                    frame.set_cursor_position(Position::new(
                        inner.x + screen_col,
                        inner.y + screen_row,
                    ));
                }
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
