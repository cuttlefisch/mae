//! *Messages* buffer rendering.

use mae_core::{Buffer, Editor, Window};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::theme_convert::ts;

pub(crate) fn render_messages_window(
    frame: &mut Frame,
    area: Rect,
    buf: &Buffer,
    win: &Window,
    focused: bool,
    editor: &Editor,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
    };

    let entry_count = editor.message_log.len();
    let title = format!(" *Messages* ({}) ", entry_count);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let entries = editor.message_log.entries();
    let viewport_height = inner.height as usize;
    let inner_width = inner.width as usize;

    let total = entries.len();
    let start = win.scroll_offset.min(total);

    let target_style = ts(editor, "diagnostic.target");

    // When word wrapping, a single entry may span multiple visual rows.
    // Over-fetch entries and let ratatui's Paragraph + Wrap clip at the
    // widget boundary. We also accumulate visual rows so scrolling stays
    // correct — stop once we've filled the viewport.
    let wrap_enabled = buf.local_options.word_wrap.unwrap_or(editor.word_wrap) && inner_width > 0;

    let mut lines: Vec<Line> = Vec::new();
    let mut visual_rows = 0usize;
    for entry in entries.iter().skip(start) {
        if visual_rows >= viewport_height {
            break;
        }
        let level_style = match entry.level {
            mae_core::MessageLevel::Error => ts(editor, "diagnostic.error"),
            mae_core::MessageLevel::Warn => ts(editor, "diagnostic.warn"),
            mae_core::MessageLevel::Info => ts(editor, "diagnostic.info"),
            mae_core::MessageLevel::Debug => ts(editor, "diagnostic.debug"),
            mae_core::MessageLevel::Trace => ts(editor, "diagnostic.trace"),
        };

        let level_tag = match entry.level {
            mae_core::MessageLevel::Error => "ERROR",
            mae_core::MessageLevel::Warn => " WARN",
            mae_core::MessageLevel::Info => " INFO",
            mae_core::MessageLevel::Debug => "DEBUG",
            mae_core::MessageLevel::Trace => "TRACE",
        };

        // Approximate prefix width: "[ERROR] [target] "
        let prefix_len = 2 + level_tag.len() + 3 + entry.target.len() + 2;
        let line_chars = prefix_len + entry.message.len();
        let rows = if wrap_enabled && inner_width > 0 {
            line_chars.div_ceil(inner_width)
        } else {
            1
        };
        visual_rows += rows;

        lines.push(Line::from(vec![
            Span::styled(format!("[{}]", level_tag), level_style),
            Span::raw(" "),
            Span::styled(format!("[{}]", entry.target), target_style),
            Span::raw(" "),
            Span::styled(&entry.message, ts(editor, "ui.text")),
        ]));
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(no messages)",
            ts(editor, "ui.text"),
        )));
    }

    let mut paragraph = Paragraph::new(lines);
    if wrap_enabled {
        paragraph = paragraph.wrap(Wrap { trim: false });
    }
    frame.render_widget(paragraph, inner);
}
