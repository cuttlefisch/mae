//! Debug panel renderer — styles the `*Debug*` buffer lines based on
//! `DebugView::line_map` items.
//!
//! The buffer rope is already populated with text by `debug_populate_buffer`.
//! This renderer applies semantic styling: section headers are bold, active
//! threads/frames are highlighted, variable names are colored, and the
//! cursor line gets a selection background.

use mae_core::{DebugLineItem, Editor};
use ratatui::prelude::*;

use crate::theme_convert::ts;

/// Render a debug panel window. Reads the buffer's rope and styles each
/// line according to the `line_map` semantic items.
pub(crate) fn render_debug_window(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    _win: &mae_core::Window,
    focused: bool,
    editor: &Editor,
) {
    // Border
    let title = match &editor.debug_state {
        Some(state) => match &state.target {
            mae_core::debug::DebugTarget::Dap {
                adapter_name,
                program,
            } => {
                let short = program.rsplit('/').next().unwrap_or(program);
                format!(" *Debug* [{}: {}] ", adapter_name, short)
            }
            mae_core::debug::DebugTarget::SelfDebug => " *Debug* [self] ".to_string(),
        },
        None => " *Debug* ".to_string(),
    };

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

    let view = match buf.debug_view.as_ref() {
        Some(v) => v,
        None => return,
    };

    // Collect rope lines.
    let rope = buf.rope();
    let total_lines = rope.len_lines();
    if total_lines == 0 {
        return;
    }

    // Compute scroll offset to keep cursor visible.
    let cursor_idx = view.cursor_index;
    let visible_height = inner.height as usize;
    let scroll_offset = if cursor_idx >= visible_height {
        cursor_idx - visible_height + 1
    } else {
        0
    };

    let default_style = ts(editor, "ui.text");
    let section_style = ts(editor, "ui.text").bold();
    let active_thread_style = ts(editor, "markup.heading").bold();
    let thread_style = ts(editor, "ui.text");
    let active_frame_style = ts(editor, "markup.heading");
    let frame_style = ts(editor, "ui.text");
    let var_name_style = ts(editor, "variable");

    let cursor_style = ts(editor, "ui.selection");
    let output_style = ts(editor, "comment");

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

        // Determine style based on line_map.
        let item = view.line_map.get(line_idx);
        let is_cursor_line = focused && line_idx == cursor_idx;

        let line_style = match item {
            Some(DebugLineItem::SectionHeader(_)) => section_style,
            Some(DebugLineItem::Thread(tid)) => {
                if let Some(state) = &editor.debug_state {
                    if *tid == state.active_thread_id {
                        active_thread_style
                    } else {
                        thread_style
                    }
                } else {
                    thread_style
                }
            }
            Some(DebugLineItem::Frame(fid)) => {
                let is_selected = view.selected_frame_id == Some(*fid);
                if is_selected {
                    active_frame_style
                } else {
                    frame_style
                }
            }
            Some(DebugLineItem::Variable { .. }) => var_name_style,
            Some(DebugLineItem::OutputLine(_)) => output_style,
            Some(DebugLineItem::Blank) | None => default_style,
        };

        let final_style = if is_cursor_line {
            line_style.patch(cursor_style)
        } else {
            line_style
        };

        // Truncate to fit width.
        let display_width = inner.width as usize;
        let truncated: String = line_text.chars().take(display_width).collect();
        let span = Span::styled(truncated, final_style);

        // Pad with background if cursor line.
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
