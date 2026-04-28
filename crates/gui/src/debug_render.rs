//! Debug panel rendering for the GUI backend.
//!
//! Shared logic (title, style resolution, scroll offset) lives in
//! `mae_core::render_common::debug`. This module handles Skia-specific rendering.

use mae_core::render_common::debug::{
    debug_line_style, debug_scroll_offset, debug_style_theme_key, debug_title, DebugLineStyle,
};
use mae_core::{Editor, Window};

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
use crate::theme;

/// Render a debug panel window.
pub fn render_debug_window(
    canvas: &mut SkiaCanvas,
    buf: &mae_core::Buffer,
    _win: &Window,
    focused: bool,
    editor: &Editor,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    let title = debug_title(editor);

    let border_fg = if focused {
        theme::ts_fg(editor, "ui.statusline")
    } else {
        theme::ts_fg(editor, "ui.statusline.inactive")
    };

    draw_window_border(
        canvas,
        area_row,
        area_col,
        area_width,
        area_height,
        border_fg,
        &title,
    );

    let inner_row = area_row + 1;
    let inner_col = area_col + 1;
    let inner_width = area_width.saturating_sub(2);
    let inner_height = area_height.saturating_sub(2);

    if inner_width == 0 || inner_height == 0 {
        return;
    }

    let view = match buf.debug_view.as_ref() {
        Some(v) => v,
        None => return,
    };

    let rope = buf.rope();
    let total_lines = rope.len_lines();
    if total_lines == 0 {
        return;
    }

    let cursor_idx = view.cursor_index;
    let scroll_offset = debug_scroll_offset(cursor_idx, inner_height);

    let active_thread_id = editor.debug_state.as_ref().map(|s| s.active_thread_id);
    let selected_frame_id = view.selected_frame_id;
    let cursor_bg = theme::ts_bg(editor, "ui.selection");

    for row in 0..inner_height {
        let line_idx = scroll_offset + row;
        if line_idx >= total_lines {
            break;
        }

        let line_text = {
            let start = rope.line_to_char(line_idx);
            let end = if line_idx + 1 < total_lines {
                rope.line_to_char(line_idx + 1)
            } else {
                rope.len_chars()
            };
            let s: String = rope.slice(start..end).chars().collect();
            s.trim_end_matches('\n').to_string()
        };

        let item = view.line_map.get(line_idx);
        let is_cursor_line = focused && line_idx == cursor_idx;
        let style_cat = debug_line_style(item, active_thread_id, selected_frame_id);
        let theme_key = debug_style_theme_key(style_cat);
        let fg = theme::ts_fg(editor, theme_key);
        let is_bold = style_cat == DebugLineStyle::SectionHeader;

        let screen_row = inner_row + row;

        if let (true, Some(bg)) = (is_cursor_line, cursor_bg) {
            canvas.draw_rect_fill(screen_row, inner_col, inner_width, 1, bg);
        }

        let display: String = line_text.chars().take(inner_width).collect();
        if is_bold {
            canvas.draw_text_bold(screen_row, inner_col, &display, fg);
        } else {
            canvas.draw_text_at(screen_row, inner_col, &display, fg);
        }
    }
}
