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

    if let Some(ft) = buf.file_tree() {
        let sel_bg = theme::ts_bg(editor, "ui.selection").unwrap_or(theme::DEFAULT_BG);
        let dir_fg = theme::ts_fg(editor, "keyword");
        let file_fg = theme::ts_fg(editor, "ui.text");

        let (lines, _scroll) =
            mae_core::render_common::file_tree::format_file_tree_lines(ft, inner_height);

        for (i, line) in lines.iter().enumerate() {
            let row = inner_row + i;

            if line.is_selected {
                canvas.draw_rect_fill(row, inner_col, inner_width, 1, sel_bg);
            }

            let fg = if let Some(theme_key) = line.git_theme_key {
                theme::ts_fg(editor, theme_key)
            } else if line.is_dir {
                dir_fg
            } else {
                file_fg
            };
            let truncated: String = line.display.chars().take(inner_width).collect();
            canvas.draw_text_at(row, inner_col, &truncated, fg);
        }
    }
}
