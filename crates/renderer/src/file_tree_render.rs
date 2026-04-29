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

    if let Some(ref ft) = buf.file_tree {
        let sel_style = ts(editor, "ui.selection");
        let dir_style = ts(editor, "keyword").add_modifier(Modifier::BOLD);
        let file_style = ts(editor, "ui.text");

        let viewport_height = inner.height as usize;
        let mut scroll = ft.scroll_offset;
        if ft.selected < scroll {
            scroll = ft.selected;
        }
        if viewport_height > 0 && ft.selected >= scroll + viewport_height {
            scroll = ft.selected.saturating_sub(viewport_height - 1);
        }

        let mut lines: Vec<Line> = Vec::new();
        for (i, entry) in ft
            .entries
            .iter()
            .skip(scroll)
            .take(viewport_height)
            .enumerate()
        {
            let global_idx = scroll + i;
            let indent = "  ".repeat(entry.depth);
            let is_expanded = entry.is_dir && ft.expanded_dirs.contains(&entry.path);
            let icon = mae_core::file_tree::icon_for_path(&entry.path, entry.is_dir, is_expanded);
            let display = format!("{}{} {}", indent, icon, entry.name);
            let git_suffix = match entry.git_status {
                Some(gs) if gs != mae_core::file_tree::FileGitStatus::Clean => {
                    format!(" [{}]", gs.marker_char())
                }
                _ => String::new(),
            };
            let display_full = format!("{}{}", display, git_suffix);
            let style = if global_idx == ft.selected {
                sel_style
            } else if let Some(gs) = entry.git_status {
                if gs != mae_core::file_tree::FileGitStatus::Clean {
                    ts(editor, gs.theme_key())
                } else if entry.is_dir {
                    dir_style
                } else {
                    file_style
                }
            } else if entry.is_dir {
                dir_style
            } else {
                file_style
            };
            lines.push(Line::from(Span::styled(display_full, style)));
        }
        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, inner);
    }
}
