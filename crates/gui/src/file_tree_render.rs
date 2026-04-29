//! File tree sidebar rendering for the GUI backend.

use mae_core::{Editor, Window};

use crate::canvas::SkiaCanvas;
use crate::draw_window_border;
use crate::theme;

pub fn render_file_tree_window(
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
    let border_fg = if focused {
        theme::ts_fg(editor, "ui.window.border.active")
    } else {
        theme::ts_fg(editor, "ui.window.border")
    };
    draw_window_border(
        canvas,
        area_row,
        area_col,
        area_width,
        area_height,
        border_fg,
        " File Tree ",
    );

    let inner_row = area_row + 1;
    let inner_col = area_col + 1;
    let inner_width = area_width.saturating_sub(2);
    let inner_height = area_height.saturating_sub(2);

    if let Some(ref ft) = buf.file_tree {
        let sel_bg = theme::ts_bg(editor, "ui.selection").unwrap_or(theme::DEFAULT_BG);
        let dir_fg = theme::ts_fg(editor, "keyword");
        let file_fg = theme::ts_fg(editor, "ui.text");

        // Compute effective scroll offset to keep selected visible.
        let mut scroll = ft.scroll_offset;
        if ft.selected < scroll {
            scroll = ft.selected;
        }
        if inner_height > 0 && ft.selected >= scroll + inner_height {
            scroll = ft.selected.saturating_sub(inner_height - 1);
        }

        for (i, entry) in ft
            .entries
            .iter()
            .skip(scroll)
            .take(inner_height)
            .enumerate()
        {
            let row = inner_row + i;
            let global_idx = scroll + i;

            // Selection highlight.
            if global_idx == ft.selected {
                canvas.draw_rect_fill(row, inner_col, inner_width, 1, sel_bg);
            }

            // Build display line: indent + icon + name.
            let indent = "  ".repeat(entry.depth);
            let is_expanded = entry.is_dir && ft.expanded_dirs.contains(&entry.path);
            let icon = mae_core::file_tree::icon_for_path(&entry.path, entry.is_dir, is_expanded);
            let display = format!("{}{} {}", indent, icon, entry.name);

            let fg = if let Some(gs) = entry.git_status {
                if gs != mae_core::file_tree::FileGitStatus::Clean {
                    theme::ts_fg(editor, gs.theme_key())
                } else if entry.is_dir {
                    dir_fg
                } else {
                    file_fg
                }
            } else if entry.is_dir {
                dir_fg
            } else {
                file_fg
            };
            let suffix = match entry.git_status {
                Some(gs) if gs != mae_core::file_tree::FileGitStatus::Clean => {
                    format!(" [{}]", gs.marker_char())
                }
                _ => String::new(),
            };
            let full = format!("{}{}", display, suffix);
            let truncated: String = full.chars().take(inner_width).collect();
            canvas.draw_text_at(row, inner_col, &truncated, fg);
        }
    }
}
