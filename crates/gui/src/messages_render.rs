//! *Messages* buffer rendering for the GUI backend.

use mae_core::{Editor, Window};

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
use crate::theme;

/// Render the *Messages* log buffer.
pub fn render_messages_window(
    canvas: &mut SkiaCanvas,
    win: &Window,
    focused: bool,
    editor: &Editor,
    area_row: usize,
    area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    let border_fg = if focused {
        theme::ts_fg(editor, "ui.window.border.active")
    } else {
        theme::ts_fg(editor, "ui.window.border")
    };

    let entry_count = editor.message_log.len();
    let title = format!(" *Messages* ({}) ", entry_count);
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

    let entries = editor.message_log.entries();
    let total = entries.len();
    let start = win.scroll_offset.min(total);

    let target_fg = theme::ts_fg(editor, "diagnostic.target");
    let text_fg = theme::ts_fg(editor, "ui.text");

    for (i, entry) in entries.iter().skip(start).take(inner_height).enumerate() {
        let level_fg = match entry.level {
            mae_core::MessageLevel::Error => theme::ts_fg(editor, "diagnostic.error"),
            mae_core::MessageLevel::Warn => theme::ts_fg(editor, "diagnostic.warn"),
            mae_core::MessageLevel::Info => theme::ts_fg(editor, "diagnostic.info"),
            mae_core::MessageLevel::Debug => theme::ts_fg(editor, "diagnostic.debug"),
            mae_core::MessageLevel::Trace => theme::ts_fg(editor, "diagnostic.trace"),
        };

        let level_tag = match entry.level {
            mae_core::MessageLevel::Error => "ERROR",
            mae_core::MessageLevel::Warn => " WARN",
            mae_core::MessageLevel::Info => " INFO",
            mae_core::MessageLevel::Debug => "DEBUG",
            mae_core::MessageLevel::Trace => "TRACE",
        };

        let row = inner_row + i;
        let col = inner_col;

        canvas.draw_text_at(row, col, &format!("[{}]", level_tag), level_fg);
        let offset = level_tag.len() + 3;
        canvas.draw_text_at(row, col + offset, &format!("[{}]", entry.target), target_fg);
        let offset2 = offset + entry.target.len() + 3;
        let remaining: String = entry
            .message
            .chars()
            .take(inner_width.saturating_sub(offset2))
            .collect();
        canvas.draw_text_at(row, col + offset2, &remaining, text_fg);
    }

    if entries.is_empty() {
        canvas.draw_text_at(inner_row, inner_col, "(no messages)", text_fg);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn level_tags_are_padded() {
        // All level tags should be 5 chars for alignment.
        for tag in ["ERROR", " WARN", " INFO", "DEBUG", "TRACE"] {
            assert_eq!(tag.len(), 5);
        }
    }
}
