//! Cursor positioning logic.

use mae_core::{grapheme, Editor, Mode};
use ratatui::prelude::*;

use mae_core::wrap::{wrap_cursor_position, wrap_line_display_rows};

use mae_core::render_common::gutter::gutter_width;

/// Compute and set the terminal cursor position for the current mode.
pub(crate) fn set_cursor(frame: &mut Frame, editor: &Editor, window_area: Rect, cmd_area: Rect) {
    let focused_win = editor.window_mgr.focused_window();
    let focused_buf = &editor.buffers[focused_win.buffer_idx];
    // ADR-087 Rule 3: the user's width options must reach every width
    // computation, not just the status bar.
    let policy = editor.width_policy();

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
        let gutter_w = if !mae_core::BufferMode::has_gutter(&focused_buf.kind) {
            0
        } else if editor.show_line_numbers {
            gutter_width(focused_buf.display_line_count())
        } else {
            2
        };

        if editor.mode == Mode::Command {
            let cursor_col = editor.vi.command_line
                [..editor.vi.command_cursor.min(editor.vi.command_line.len())]
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
            // ADR-087 Rule 1: cursor_col is a BYTE column; converting it to a
            // screen column is `display_width_of_prefix_with`, not
            // `display_width_up_to_grapheme` (which wants a grapheme index --
            // one of the four domains this field used to be read as).
            let display_col =
                grapheme::display_width_of_prefix_with(&line_text, focused_win.cursor_col, policy);
            let cursor_x = inner.x + gutter_w as u16 + display_col as u16;
            let cursor_y = inner.y
                + focused_win
                    .cursor_row
                    .saturating_sub(focused_win.scroll_offset) as u16;
            frame.set_cursor_position(Position::new(cursor_x, cursor_y));
        } else {
            // #353: measure against the DISPLAY text (active display
            // regions substituted in — tab expansion, link concealment),
            // not the raw rope line, and remap cursor_col (a rope column)
            // to its DISPLAY column via the same helper buffer_render.rs's
            // drawing path is built on — otherwise cursor placement drifts
            // out of sync with what's actually drawn whenever the line has
            // a tab (or, pre-existing gap this also fixes as a side
            // effect, a concealed link) before the cursor.
            let (line_text, display_col) = if focused_win.cursor_row < focused_buf.line_count() {
                focused_buf.display_text_and_col(focused_win.cursor_row, focused_win.cursor_col)
            } else {
                (String::new(), focused_win.cursor_col)
            };

            let text_width = inner.width.saturating_sub(gutter_w as u16) as usize;
            let wrap = focused_buf
                .local_options
                .word_wrap
                .unwrap_or(editor.word_wrap)
                && text_width > 0;

            let show_break_w = editor.show_break.chars().count();

            if wrap {
                // Count display rows consumed by lines before the cursor line.
                let mut screen_row: u16 = 0;
                for ln in focused_win.scroll_offset..focused_win.cursor_row {
                    if ln < focused_buf.line_count() {
                        let (lt, _) = focused_buf.display_text_and_col(ln, 0);
                        let rows = wrap_line_display_rows(
                            &lt,
                            text_width,
                            editor.break_indent,
                            show_break_w,
                            policy,
                        );
                        screen_row += rows as u16;
                    } else {
                        screen_row += 1;
                    }
                }
                // Add wrapped row/col offset within the cursor's own line.
                let (wrap_row, wrap_col) = wrap_cursor_position(
                    &line_text,
                    display_col,
                    text_width,
                    editor.break_indent,
                    show_break_w,
                    policy,
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
                // col_offset is a ROPE column (set from cursor_col during
                // horizontal-scroll clamping, window.rs) -- remap it to a
                // DISPLAY column the same way cursor_col was above, so both
                // sides of the subtraction below are in the same unit.
                let (_, scroll_display_col) = focused_buf
                    .display_text_and_col(focused_win.cursor_row, focused_win.col_offset);
                // Both operands must be SCREEN columns. `display_col` and
                // `scroll_display_col` are byte offsets into the display text
                // (ADR-087 Rule 4), so each gets its own explicit conversion
                // -- subtracting a byte offset from a width was the shape of
                // the bug this rule exists to close.
                let cursor_screen_col =
                    grapheme::display_width_of_prefix_with(&line_text, display_col, policy);
                let scroll_col =
                    grapheme::display_width_of_prefix_with(&line_text, scroll_display_col, policy);
                let screen_col =
                    gutter_w as u16 + (cursor_screen_col.saturating_sub(scroll_col)) as u16;
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
