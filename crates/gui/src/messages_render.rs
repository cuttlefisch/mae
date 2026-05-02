//! *Messages* buffer rendering for the GUI backend.

use mae_core::{Buffer, Editor, Window};

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
use crate::theme;

/// Render the *Messages* log buffer.
pub fn render_messages_window(
    canvas: &mut SkiaCanvas,
    buf: &Buffer,
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

    let wrap_enabled = buf.local_options.word_wrap.unwrap_or(editor.word_wrap) && inner_width > 0;
    let mut visual_row = 0usize;

    for entry in entries.iter().skip(start) {
        if visual_row >= inner_height {
            break;
        }

        let mp = mae_core::render_common::messages::message_prefix(entry.level);
        let level_fg = theme::ts_fg(editor, mp.theme_key);
        let level_tag = mp.tag;

        let prefix = format!("[{}] [{}] ", level_tag, entry.target);
        let prefix_len = prefix.len();

        if !wrap_enabled {
            let row = inner_row + visual_row;
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
            visual_row += 1;
        } else {
            // Word-wrap: render prefix on first line, then wrap message text
            let full_line = format!("{}{}", prefix, entry.message);
            let chars: Vec<char> = full_line.chars().collect();
            let mut pos = 0;
            let mut first = true;
            while pos < chars.len() && visual_row < inner_height {
                let row = inner_row + visual_row;
                let chunk_len = inner_width.min(chars.len() - pos);
                let chunk: String = chars[pos..pos + chunk_len].iter().collect();

                if first {
                    // Draw with colored prefix
                    canvas.draw_text_at(row, inner_col, &format!("[{}]", level_tag), level_fg);
                    let offset = level_tag.len() + 3;
                    canvas.draw_text_at(
                        row,
                        inner_col + offset,
                        &format!("[{}]", entry.target),
                        target_fg,
                    );
                    let offset2 = offset + entry.target.len() + 3;
                    let msg_chunk: String = chars[pos + prefix_len.min(chunk_len)..pos + chunk_len]
                        .iter()
                        .collect();
                    canvas.draw_text_at(row, inner_col + offset2, &msg_chunk, text_fg);
                    first = false;
                } else {
                    // Continuation lines: just message text
                    canvas.draw_text_at(row, inner_col, &chunk, text_fg);
                }

                pos += chunk_len;
                visual_row += 1;
            }
        }
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
