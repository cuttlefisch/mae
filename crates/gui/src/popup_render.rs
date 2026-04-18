//! Popup overlays: file picker, file browser, command palette, LSP completion.

use mae_core::{Editor, PalettePurpose};
use skia_safe::Color4f;

use crate::canvas::{CellRect, SkiaCanvas};
use crate::theme;

/// Centered popup rect (70% x 60% of the area, clamped).
pub fn centered_popup_rect(area_width: usize, area_height: usize) -> CellRect {
    let w = (area_width * 70 / 100).max(40).min(area_width);
    let h = (area_height * 60 / 100).max(10).min(area_height);
    let x = (area_width.saturating_sub(w)) / 2;
    let y = (area_height.saturating_sub(h)) / 2;
    CellRect::new(y, x, w, h)
}

// ---------------------------------------------------------------------------
// LSP completion popup
// ---------------------------------------------------------------------------

pub fn render_completion_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_row: usize,
    _area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    let items = &editor.completion_items;
    if items.is_empty() {
        return;
    }

    let win = editor.window_mgr.focused_window();
    let cursor_screen_row = win.cursor_row.saturating_sub(win.scroll_offset);
    let cursor_screen_col = win.cursor_col;

    const MAX_ITEMS: usize = 10;
    let visible_count = items.len().min(MAX_ITEMS);
    let popup_width = items
        .iter()
        .take(MAX_ITEMS)
        .map(|i| {
            let detail_len = i.detail.as_deref().map(|d| d.len() + 2).unwrap_or(0);
            i.label.len() + detail_len + 4
        })
        .max()
        .unwrap_or(20)
        .min(50);
    let popup_height = visible_count + 2; // border top+bottom

    let popup_top = if cursor_screen_row + 1 + popup_height < area_height {
        area_row + cursor_screen_row + 1
    } else {
        area_row + cursor_screen_row.saturating_sub(popup_height)
    };
    let popup_left = cursor_screen_col.min(area_width.saturating_sub(popup_width));

    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let normal_fg = theme::ts_fg(editor, "ui.popup.text");
    let normal_bg = theme::ts_bg(editor, "ui.popup.text");
    let selected_fg = theme::ts_fg(editor, "ui.popup.key");
    let selected_bg = theme::ts_bg(editor, "ui.popup.key");

    // Clear popup area.
    let bg = normal_bg.unwrap_or(theme::DEFAULT_BG);
    canvas.draw_rect_fill(popup_top, popup_left, popup_width, popup_height, bg);

    // Border.
    draw_border(
        canvas,
        popup_top,
        popup_left,
        popup_width,
        popup_height,
        border_fg,
    );

    // Items.
    let inner_top = popup_top + 1;
    let inner_left = popup_left + 1;
    let inner_width = popup_width.saturating_sub(2);

    for (i, item) in items.iter().take(MAX_ITEMS).enumerate() {
        let is_selected = i == editor.completion_selected;
        let fg = if is_selected { selected_fg } else { normal_fg };
        let item_bg = if is_selected { selected_bg } else { normal_bg };

        if let Some(bg) = item_bg {
            canvas.draw_rect_fill(inner_top + i, inner_left, inner_width, 1, bg);
        }

        let sigil = item.kind_sigil;
        let detail_part = item
            .detail
            .as_deref()
            .map(|d| {
                let truncated: String = d.chars().take(20).collect();
                format!("  {}", truncated)
            })
            .unwrap_or_default();
        let text = format!("{} {}{}", sigil, item.label, detail_part);
        let display: String = text.chars().take(inner_width).collect();
        canvas.draw_text_at(inner_top + i, inner_left, &display, fg);
    }
}

// ---------------------------------------------------------------------------
// File picker
// ---------------------------------------------------------------------------

pub fn render_file_picker(canvas: &mut SkiaCanvas, editor: &Editor, cols: usize, rows: usize) {
    let picker = match &editor.file_picker {
        Some(p) => p,
        None => return,
    };

    let popup = centered_popup_rect(cols, rows);
    let text_fg = theme::ts_fg(editor, "ui.text");
    let selection_bg = theme::ts_bg(editor, "ui.selection");
    let selection_fg = theme::ts_fg(editor, "ui.selection");
    let prompt_fg = theme::ts_fg(editor, "ui.popup.key");
    let border_fg = theme::ts_fg(editor, "ui.window.border.active");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    // Clear and draw border.
    canvas.draw_rect_fill(popup.row, popup.col, popup.width, popup.height, bg);
    draw_border(
        canvas,
        popup.row,
        popup.col,
        popup.width,
        popup.height,
        border_fg,
    );

    // Title.
    let match_count = picker.filtered.len();
    let total = picker.candidates.len();
    let title = format!(
        " Find File [{}] ({}/{}) ",
        picker.root_label, match_count, total
    );
    canvas.draw_text_at(popup.row, popup.col + 2, &title, border_fg);

    let inner = popup.inner();
    if inner.height < 2 || inner.width < 4 {
        return;
    }

    // Query line.
    canvas.draw_text_at(inner.row, inner.col, "> ", prompt_fg);
    let query_fg = if picker.query_selected {
        selection_fg
    } else {
        text_fg
    };
    if picker.query_selected {
        if let Some(bg) = selection_bg {
            canvas.draw_rect_fill(inner.row, inner.col + 2, picker.query.len().max(1), 1, bg);
        }
    }
    canvas.draw_text_at(inner.row, inner.col + 2, &picker.query, query_fg);

    // Results.
    let results_height = inner.height.saturating_sub(1);
    let start = if picker.selected >= results_height {
        picker.selected - results_height + 1
    } else {
        0
    };

    for (display_idx, &filtered_idx) in picker
        .filtered
        .iter()
        .skip(start)
        .take(results_height)
        .enumerate()
    {
        let path = &picker.candidates[filtered_idx];
        let actual_idx = start + display_idx;
        let is_selected = actual_idx == picker.selected && !picker.query_selected;
        let row = inner.row + 1 + display_idx;

        if is_selected {
            if let Some(bg) = selection_bg {
                canvas.draw_rect_fill(row, inner.col, inner.width, 1, bg);
            }
        }

        let fg = if is_selected { selection_fg } else { text_fg };
        let max_w = inner.width.saturating_sub(1);
        let display = if path.len() > max_w {
            format!("…{}", &path[path.len() - max_w + 1..])
        } else {
            path.clone()
        };
        canvas.draw_text_at(row, inner.col, &display, fg);
    }
}

// ---------------------------------------------------------------------------
// File browser
// ---------------------------------------------------------------------------

pub fn render_file_browser(canvas: &mut SkiaCanvas, editor: &Editor, cols: usize, rows: usize) {
    let browser = match &editor.file_browser {
        Some(b) => b,
        None => return,
    };

    let popup = centered_popup_rect(cols, rows);
    let text_fg = theme::ts_fg(editor, "ui.text");
    let selection_fg = theme::ts_fg(editor, "ui.selection");
    let selection_bg = theme::ts_bg(editor, "ui.selection");
    let prompt_fg = theme::ts_fg(editor, "ui.popup.key");
    let dir_fg = theme::ts_fg(editor, "keyword");
    let border_fg = theme::ts_fg(editor, "ui.window.border.active");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(popup.row, popup.col, popup.width, popup.height, bg);
    draw_border(
        canvas,
        popup.row,
        popup.col,
        popup.width,
        popup.height,
        border_fg,
    );

    let cwd_display = browser.cwd.display().to_string();
    let match_count = browser.filtered.len();
    let total = browser.entries.len();
    let title = format!(" {} ({}/{}) ", cwd_display, match_count, total);
    canvas.draw_text_at(popup.row, popup.col + 2, &title, border_fg);

    let inner = popup.inner();
    if inner.height < 2 || inner.width < 4 {
        return;
    }

    canvas.draw_text_at(inner.row, inner.col, "> ", prompt_fg);
    canvas.draw_text_at(inner.row, inner.col + 2, &browser.query, text_fg);

    let results_height = inner.height.saturating_sub(1);
    let start = if browser.selected >= results_height {
        browser.selected - results_height + 1
    } else {
        0
    };

    for (display_idx, &idx) in browser
        .filtered
        .iter()
        .skip(start)
        .take(results_height)
        .enumerate()
    {
        let entry = &browser.entries[idx];
        let actual_idx = start + display_idx;
        let is_selected = actual_idx == browser.selected;
        let row = inner.row + 1 + display_idx;

        if is_selected {
            if let Some(bg) = selection_bg {
                canvas.draw_rect_fill(row, inner.col, inner.width, 1, bg);
            }
        }

        let base_fg = if entry.is_dir { dir_fg } else { text_fg };
        let fg = if is_selected { selection_fg } else { base_fg };

        let mut name = entry.display();
        let max_w = inner.width.saturating_sub(1);
        if name.len() > max_w {
            name = format!("…{}", &name[name.len() - max_w + 1..]);
        }
        canvas.draw_text_at(row, inner.col, &name, fg);
    }
}

// ---------------------------------------------------------------------------
// Command palette
// ---------------------------------------------------------------------------

pub fn render_command_palette(canvas: &mut SkiaCanvas, editor: &Editor, cols: usize, rows: usize) {
    let palette = match &editor.command_palette {
        Some(p) => p,
        None => return,
    };

    let popup = centered_popup_rect(cols, rows);
    let text_fg = theme::ts_fg(editor, "ui.text");
    let selection_fg = theme::ts_fg(editor, "ui.selection");
    let selection_bg = theme::ts_bg(editor, "ui.selection");
    let prompt_fg = theme::ts_fg(editor, "ui.popup.key");
    let doc_fg = theme::ts_fg(editor, "ui.popup.text");
    let border_fg = theme::ts_fg(editor, "ui.window.border.active");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(popup.row, popup.col, popup.width, popup.height, bg);
    draw_border(
        canvas,
        popup.row,
        popup.col,
        popup.width,
        popup.height,
        border_fg,
    );

    let match_count = palette.filtered.len();
    let total = palette.entries.len();
    let title = match palette.purpose {
        PalettePurpose::Execute => format!(" Commands ({}/{}) ", match_count, total),
        PalettePurpose::Describe => format!(" Describe Command ({}/{}) ", match_count, total),
        PalettePurpose::SetTheme => format!(" Themes ({}/{}) ", match_count, total),
        PalettePurpose::HelpSearch => format!(" Help Topics ({}/{}) ", match_count, total),
        PalettePurpose::SwitchBuffer => format!(" Buffers ({}/{}) ", match_count, total),
        PalettePurpose::SetSplashArt => format!(" Splash Art ({}/{}) ", match_count, total),
        PalettePurpose::RecentFile => format!(" Recent Files ({}/{}) ", match_count, total),
        PalettePurpose::SwitchProject => format!(" Projects ({}/{}) ", match_count, total),
    };
    canvas.draw_text_at(popup.row, popup.col + 2, &title, border_fg);

    let inner = popup.inner();
    if inner.height < 2 || inner.width < 4 {
        return;
    }

    canvas.draw_text_at(inner.row, inner.col, "> ", prompt_fg);
    canvas.draw_text_at(inner.row, inner.col + 2, &palette.query, text_fg);

    let results_height = inner.height.saturating_sub(1);
    let start = if palette.selected >= results_height {
        palette.selected - results_height + 1
    } else {
        0
    };

    let max_name_width = (inner.width * 2 / 5).max(12);
    let name_col = palette
        .filtered
        .iter()
        .skip(start)
        .take(results_height)
        .map(|&i| palette.entries[i].name.len())
        .max()
        .unwrap_or(0)
        .min(max_name_width);

    for (display_idx, &entry_idx) in palette
        .filtered
        .iter()
        .skip(start)
        .take(results_height)
        .enumerate()
    {
        let entry = &palette.entries[entry_idx];
        let actual_idx = start + display_idx;
        let is_selected = actual_idx == palette.selected;
        let row = inner.row + 1 + display_idx;

        if is_selected {
            if let Some(bg) = selection_bg {
                canvas.draw_rect_fill(row, inner.col, inner.width, 1, bg);
            }
        }

        let fg = if is_selected { selection_fg } else { text_fg };
        let dfg = if is_selected { selection_fg } else { doc_fg };

        let name_display = if entry.name.len() > name_col {
            entry.name[..name_col].to_string()
        } else {
            format!("{:<w$}", entry.name, w = name_col)
        };

        let available_for_doc = inner.width.saturating_sub(name_col + 3);
        let doc_display = if entry.doc.len() > available_for_doc && available_for_doc > 1 {
            let mut s = entry.doc[..available_for_doc.saturating_sub(1)].to_string();
            s.push('…');
            s
        } else {
            entry.doc.clone()
        };

        canvas.draw_text_at(row, inner.col + 1, &name_display, fg);
        canvas.draw_text_at(row, inner.col + 1 + name_col + 2, &doc_display, dfg);
    }
}

// ---------------------------------------------------------------------------
// Which-key popup
// ---------------------------------------------------------------------------

pub fn render_which_key_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    row_start: usize,
    height: usize,
    cols: usize,
    entries: &[mae_core::WhichKeyEntry],
) {
    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let group_fg = theme::ts_fg(editor, "ui.popup.group");
    let key_fg = theme::ts_fg(editor, "ui.popup.key");
    let text_fg = theme::ts_fg(editor, "ui.popup.text");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(row_start, 0, cols, height, bg);
    draw_border(canvas, row_start, 0, cols, height, border_fg);

    // Breadcrumb title.
    let breadcrumb: String = editor
        .which_key_prefix
        .iter()
        .map(format_keypress)
        .collect::<Vec<_>>()
        .join(" > ");
    canvas.draw_text_at(row_start, 2, &format!(" {} ", breadcrumb), border_fg);

    let inner_row = row_start + 1;
    let inner_col = 1_usize;
    let inner_width = cols.saturating_sub(2);
    let inner_height = height.saturating_sub(2);

    let col_width = 25_usize;
    let num_cols = (inner_width / col_width).max(1);

    let mut row = 0;
    let mut col = 0;

    for entry in entries {
        if row >= inner_height {
            break;
        }

        let key_str = format_keypress(&entry.key);
        let (kfg, lfg) = if entry.is_group {
            (group_fg, group_fg)
        } else {
            (key_fg, text_fg)
        };

        let max_label = col_width.saturating_sub(key_str.len() + 2);
        let label = if entry.label.len() > max_label {
            format!("{}..", &entry.label[..max_label.saturating_sub(2)])
        } else {
            entry.label.clone()
        };

        let x = inner_col + col * col_width;
        canvas.draw_text_at(inner_row + row, x, &key_str, kfg);
        canvas.draw_text_at(inner_row + row, x + key_str.len() + 1, &label, lfg);

        col += 1;
        if col >= num_cols {
            col = 0;
            row += 1;
        }
    }
}

fn format_keypress(kp: &mae_core::KeyPress) -> String {
    let mut s = String::new();
    if kp.ctrl {
        s.push_str("C-");
    }
    if kp.alt {
        s.push_str("M-");
    }
    match &kp.key {
        mae_core::Key::Char(' ') => s.push_str("SPC"),
        mae_core::Key::Char(c) => s.push(*c),
        mae_core::Key::Escape => s.push_str("Esc"),
        mae_core::Key::Enter => s.push_str("Enter"),
        mae_core::Key::Tab => s.push_str("Tab"),
        mae_core::Key::Backspace => s.push_str("BS"),
        mae_core::Key::Up => s.push_str("Up"),
        mae_core::Key::Down => s.push_str("Down"),
        mae_core::Key::Left => s.push_str("Left"),
        mae_core::Key::Right => s.push_str("Right"),
        mae_core::Key::F(n) => {
            s.push_str(&format!("F{}", n));
        }
        _ => s.push('?'),
    }
    s
}

// ---------------------------------------------------------------------------
// Border drawing helper
// ---------------------------------------------------------------------------

fn draw_border(
    canvas: &mut SkiaCanvas,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    color: Color4f,
) {
    if width < 2 || height < 2 {
        return;
    }
    // Top border.
    let top = format!("┌{}┐", "─".repeat(width.saturating_sub(2)));
    canvas.draw_text_at(row, col, &top, color);

    // Side borders.
    for r in 1..height.saturating_sub(1) {
        canvas.draw_text_at(row + r, col, "│", color);
        canvas.draw_text_at(row + r, col + width - 1, "│", color);
    }

    // Bottom border.
    let bottom = format!("└{}┘", "─".repeat(width.saturating_sub(2)));
    canvas.draw_text_at(row + height - 1, col, &bottom, color);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centered_popup_rect_dimensions() {
        let r = centered_popup_rect(100, 50);
        assert_eq!(r.width, 70);
        assert_eq!(r.height, 30);
        assert_eq!(r.col, 15); // (100-70)/2
        assert_eq!(r.row, 10); // (50-30)/2
    }

    #[test]
    fn centered_popup_rect_small_terminal() {
        let r = centered_popup_rect(30, 8);
        assert!(r.width >= 30); // clamped to min 40 -> clamped to area
        assert!(r.height >= 8);
    }

    #[test]
    fn centered_popup_rect_clamped_to_area() {
        let r = centered_popup_rect(35, 8);
        assert!(r.width <= 35);
        assert!(r.height <= 8);
    }

    #[test]
    fn format_keypress_space() {
        let kp = mae_core::KeyPress {
            key: mae_core::Key::Char(' '),
            ctrl: false,
            alt: false,
        };
        assert_eq!(format_keypress(&kp), "SPC");
    }

    #[test]
    fn format_keypress_ctrl_c() {
        let kp = mae_core::KeyPress {
            key: mae_core::Key::Char('c'),
            ctrl: true,
            alt: false,
        };
        assert_eq!(format_keypress(&kp), "C-c");
    }
}
