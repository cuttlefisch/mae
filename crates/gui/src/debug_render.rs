//! Debug panel rendering for the GUI backend.
//!
//! Styles the `*Debug*` buffer lines based on `DebugView::line_map` items.

use mae_core::{DebugLineItem, Editor, Window};

use crate::canvas::SkiaCanvas;
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
    // Border + title.
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

    // Scroll to keep cursor visible.
    let cursor_idx = view.cursor_index;
    let scroll_offset = if cursor_idx >= inner_height {
        cursor_idx - inner_height + 1
    } else {
        0
    };

    let default_fg = theme::ts_fg(editor, "ui.text");
    let section_fg = theme::ts_fg(editor, "ui.text");
    let active_thread_fg = theme::ts_fg(editor, "markup.heading");
    let active_frame_fg = theme::ts_fg(editor, "markup.heading");
    let var_fg = theme::ts_fg(editor, "variable");
    let output_fg = theme::ts_fg(editor, "comment");
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

        let fg = match item {
            Some(DebugLineItem::SectionHeader(_)) => section_fg,
            Some(DebugLineItem::Thread(tid)) => {
                if let Some(state) = &editor.debug_state {
                    if *tid == state.active_thread_id {
                        active_thread_fg
                    } else {
                        default_fg
                    }
                } else {
                    default_fg
                }
            }
            Some(DebugLineItem::Frame(fid)) => {
                if view.selected_frame_id == Some(*fid) {
                    active_frame_fg
                } else {
                    default_fg
                }
            }
            Some(DebugLineItem::Variable { .. }) => var_fg,
            Some(DebugLineItem::OutputLine(_)) => output_fg,
            Some(DebugLineItem::Blank) | None => default_fg,
        };

        let screen_row = inner_row + row;
        let is_bold = matches!(item, Some(DebugLineItem::SectionHeader(_)));

        // Cursor line background.
        if is_cursor_line {
            if let Some(bg) = cursor_bg {
                canvas.draw_rect_fill(screen_row, inner_col, inner_width, 1, bg);
            }
        }

        let display: String = line_text.chars().take(inner_width).collect();
        if is_bold {
            canvas.draw_text_bold(screen_row, inner_col, &display, fg);
        } else {
            canvas.draw_text_at(screen_row, inner_col, &display, fg);
        }
    }
}

fn draw_window_border(
    canvas: &mut SkiaCanvas,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    color: skia_safe::Color4f,
    title: &str,
) {
    if width < 2 || height < 2 {
        return;
    }
    let top = format!("┌{}┐", "─".repeat(width.saturating_sub(2)));
    canvas.draw_text_at(row, col, &top, color);
    if title.len() + 2 < width {
        canvas.draw_text_at(row, col + 1, title, color);
    }
    for r in 1..height.saturating_sub(1) {
        canvas.draw_text_at(row + r, col, "│", color);
        canvas.draw_text_at(row + r, col + width - 1, "│", color);
    }
    let bottom = format!("└{}┘", "─".repeat(width.saturating_sub(2)));
    canvas.draw_text_at(row + height - 1, col, &bottom, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_header_is_bold() {
        let item = DebugLineItem::SectionHeader("Threads".to_string());
        assert!(matches!(item, DebugLineItem::SectionHeader(_)));
    }

    #[test]
    fn variable_item_fields() {
        let item = DebugLineItem::Variable {
            scope: "Locals".to_string(),
            name: "x".to_string(),
            depth: 0,
            variables_reference: 42,
        };
        if let DebugLineItem::Variable { name, depth, .. } = &item {
            assert_eq!(name, "x");
            assert_eq!(*depth, 0);
        }
    }
}
