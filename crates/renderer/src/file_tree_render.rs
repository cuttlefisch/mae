//! File tree sidebar rendering for the TUI backend.

use mae_core::{Editor, Window};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme_convert::ts;

pub(crate) fn render_file_tree_window(
    frame: &mut Frame,
    area: Rect,
    buf: &mae_core::Buffer,
    _win: &Window,
    focused: bool,
    editor: &Editor,
) {
    let border_style = if focused {
        ts(editor, "ui.window.border.active")
    } else {
        ts(editor, "ui.window.border")
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" File Tree ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if let Some(ft) = buf.file_tree() {
        let sel_style = ts(editor, "ui.selection");
        let dir_style = ts(editor, "keyword").add_modifier(Modifier::BOLD);
        let file_style = ts(editor, "ui.text");

        let viewport_height = inner.height as usize;
        let (lines_data, _scroll) =
            mae_core::render_common::file_tree::format_file_tree_lines(ft, viewport_height);

        let mut lines: Vec<Line> = Vec::new();
        for line in &lines_data {
            let style = if line.is_selected {
                sel_style
            } else if let Some(theme_key) = line.git_theme_key {
                ts(editor, theme_key)
            } else if line.is_dir {
                dir_style
            } else {
                file_style
            };
            lines.push(Line::from(Span::styled(&*line.display, style)));
        }
        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
    }
}
