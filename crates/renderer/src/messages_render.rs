//! *Messages* buffer rendering.

use mae_core::{Editor, Window};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme_convert::ts;

pub(crate) fn render_messages_window(
    frame: &mut Frame,
    area: Rect,
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

    let total = entries.len();
    let start = if win.scroll_offset > 0 {
        win.scroll_offset.min(total)
    } else {
        total.saturating_sub(viewport_height)
    };

    let target_style = ts(editor, "diagnostic.target");

    let mut lines: Vec<Line> = Vec::new();
    for entry in entries.iter().skip(start).take(viewport_height) {
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

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
