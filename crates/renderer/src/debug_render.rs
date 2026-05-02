//! Debug panel renderer — styles the `*Debug*` buffer lines based on
//! `DebugView::line_map` items.
//!
//! Shared logic (title, style resolution, scroll offset) lives in
//! `mae_core::render_common::debug`. This module handles ratatui-specific rendering.

use mae_core::render_common::debug::{
    debug_line_style, debug_scroll_offset, debug_style_theme_key, debug_title, DebugLineStyle,
};
use mae_core::Editor;
use ratatui::prelude::*;

use crate::theme_convert::ts;

/// Render a debug panel window.
pub(crate) fn render_debug_window(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    _win: &mae_core::Window,
    focused: bool,
    editor: &Editor,
) {
    let title = debug_title(editor);

    let border_style = if focused {
        ts(editor, "ui.statusline")
    } else {
        ts(editor, "ui.statusline.inactive")
    };
    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let view = match buf.debug_view() {
        Some(v) => v,
        None => return,
    };

    let rope = buf.rope();
    let total_lines = rope.len_lines();
    if total_lines == 0 {
        return;
    }

    let cursor_idx = view.cursor_index;
    let visible_height = inner.height as usize;
    let scroll_offset = debug_scroll_offset(cursor_idx, visible_height);

    let active_thread_id = editor.debug_state.as_ref().map(|s| s.active_thread_id);
    let selected_frame_id = view.selected_frame_id;
    let cursor_style = ts(editor, "ui.selection");

    for row in 0..visible_height {
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
        let mut line_style = ts(editor, theme_key);
        if style_cat == DebugLineStyle::SectionHeader || style_cat == DebugLineStyle::ActiveThread {
            line_style = line_style.bold();
        }

        let final_style = if is_cursor_line {
            line_style.patch(cursor_style)
        } else {
            line_style
        };

        let display_width = inner.width as usize;
        let truncated: String = line_text.chars().take(display_width).collect();
        let span = Span::styled(truncated, final_style);

        if is_cursor_line {
            let pad_len = display_width.saturating_sub(line_text.chars().count());
            let padded = Line::from(vec![span, Span::styled(" ".repeat(pad_len), final_style)]);
            frame.render_widget(
                padded,
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            );
        } else {
            frame.render_widget(
                span,
                Rect::new(inner.x, inner.y + row as u16, inner.width, 1),
            );
        }
    }
}
