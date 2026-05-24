//! Popup overlays: file picker, file browser, command palette, LSP completion.

use mae_core::text_utils::{
    centered_popup_dims, display_width, format_keypress, truncate_end, truncate_start,
    which_key_column_layout, WK_BREADCRUMB_SEP, WK_DOC_MIN_WIDTH,
};
use mae_core::Editor;
use skia_safe::Color4f;

use crate::canvas::{CellRect, SkiaCanvas};
use crate::layout::FrameLayout;
use crate::theme;

/// Centered popup rect using editor-configured percentages.
pub fn centered_popup_rect_from(
    area_width: usize,
    area_height: usize,
    width_pct: usize,
    height_pct: usize,
) -> CellRect {
    let (w, h, x, y) = centered_popup_dims(area_width, area_height, width_pct, height_pct, 40, 10);
    CellRect::new(y, x, w, h)
}

/// Centered popup rect using default 70%×60% (used by tests).
#[cfg(test)]
pub fn centered_popup_rect(area_width: usize, area_height: usize) -> CellRect {
    centered_popup_rect_from(area_width, area_height, 70, 60)
}

// ---------------------------------------------------------------------------
// LSP completion popup
// ---------------------------------------------------------------------------

pub fn render_completion_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_row: usize,
    area_width: usize,
    area_height: usize,
    frame_layout: Option<&FrameLayout>,
    win_col_offset: usize,
    win_row_offset: usize,
    _win_width: usize,
    win_height: usize,
) {
    let items = &editor.completion_items;
    if items.is_empty() {
        return;
    }

    let win = editor.window_mgr.focused_window();
    // Use FrameLayout's fold/scale-aware display_row_of for cursor screen position.
    let cursor_screen_row = frame_layout
        .and_then(|fl| fl.display_row_of(win.cursor_row))
        .unwrap_or_else(|| win.cursor_row.saturating_sub(win.scroll_offset));
    let cursor_screen_col = win.cursor_col;

    let max_items = editor.completion_max_items;
    let visible_count = items.len().min(max_items);
    let popup_width = items
        .iter()
        .take(max_items)
        .map(|i| {
            let detail_len = i.detail.as_deref().map(|d| d.len() + 2).unwrap_or(0);
            i.label.len() + detail_len + 4
        })
        .max()
        .unwrap_or(20)
        .min(50);
    let popup_height = (visible_count + 2).min(area_height.saturating_sub(2)); // border top+bottom, clamped

    // Position relative to the focused window's screen rect.
    let abs_row = win_row_offset + cursor_screen_row;
    let popup_top = if cursor_screen_row + 1 + popup_height < win_height {
        abs_row + 1
    } else {
        abs_row.saturating_sub(popup_height)
    };
    let popup_top = popup_top.clamp(
        area_row,
        area_row + area_height.saturating_sub(popup_height),
    );
    let abs_col = win_col_offset + cursor_screen_col;
    let popup_left = if abs_col + popup_width <= area_width {
        abs_col
    } else {
        area_width.saturating_sub(popup_width)
    };

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

    for (i, item) in items.iter().take(max_items).enumerate() {
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

    let popup =
        centered_popup_rect_from(cols, rows, editor.popup_width_pct, editor.popup_height_pct);
    let text_fg = theme::ts_fg(editor, "ui.text");
    let selection_bg = theme::ts_bg(editor, "ui.selection");
    let selection_fg = theme::ts_fg(editor, "ui.selection");
    let prompt_fg = theme::ts_fg(editor, "ui.popup.key");
    let border_fg = theme::ts_fg(editor, "ui.window.border.active");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    // Clear and draw border with title.
    canvas.draw_rect_fill(popup.row, popup.col, popup.width, popup.height, bg);
    let match_count = picker.filtered.len();
    let total = picker.candidates.len();
    let title = format!(
        " Find File [{}] ({}/{}) ",
        picker.root_label, match_count, total
    );
    draw_border_titled(
        canvas,
        popup.row,
        popup.col,
        popup.width,
        popup.height,
        border_fg,
        &title,
    );

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
        let display = if display_width(path) > max_w {
            truncate_start(path, max_w)
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

    let popup =
        centered_popup_rect_from(cols, rows, editor.popup_width_pct, editor.popup_height_pct);
    let text_fg = theme::ts_fg(editor, "ui.text");
    let selection_fg = theme::ts_fg(editor, "ui.selection");
    let selection_bg = theme::ts_bg(editor, "ui.selection");
    let prompt_fg = theme::ts_fg(editor, "ui.popup.key");
    let dir_fg = theme::ts_fg(editor, "keyword");
    let border_fg = theme::ts_fg(editor, "ui.window.border.active");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(popup.row, popup.col, popup.width, popup.height, bg);
    let cwd_display = browser.cwd.display().to_string();
    let match_count = browser.filtered.len();
    let total = browser.entries.len();
    let title = format!(" {} ({}/{}) ", cwd_display, match_count, total);
    draw_border_titled(
        canvas,
        popup.row,
        popup.col,
        popup.width,
        popup.height,
        border_fg,
        &title,
    );

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
        if display_width(&name) > max_w {
            name = truncate_start(&name, max_w);
        }
        canvas.draw_text_at(row, inner.col, &name, fg);
    }
}

// ---------------------------------------------------------------------------
// Command palette
// ---------------------------------------------------------------------------

pub fn render_command_palette(canvas: &mut SkiaCanvas, editor: &Editor, cols: usize, rows: usize) {
    // If a mini-dialog is active, render that instead of the fuzzy palette.
    if let Some(ref dialog) = editor.mini_dialog {
        render_mini_dialog(canvas, editor, dialog, cols, rows);
        return;
    }

    let palette = match &editor.command_palette {
        Some(p) => p,
        None => return,
    };

    let popup =
        centered_popup_rect_from(cols, rows, editor.popup_width_pct, editor.popup_height_pct);
    let text_fg = theme::ts_fg(editor, "ui.text");
    let selection_fg = theme::ts_fg(editor, "ui.selection");
    let selection_bg = theme::ts_bg(editor, "ui.selection");
    let prompt_fg = theme::ts_fg(editor, "ui.popup.key");
    let doc_fg = theme::ts_fg(editor, "ui.popup.text");
    let border_fg = theme::ts_fg(editor, "ui.window.border.active");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(popup.row, popup.col, popup.width, popup.height, bg);
    let match_count = palette.filtered.len();
    let total = palette.entries.len();
    let title = format!(" {} ({}/{}) ", palette.purpose.label(), match_count, total);
    draw_border_titled(
        canvas,
        popup.row,
        popup.col,
        popup.width,
        popup.height,
        border_fg,
        &title,
    );

    let inner = popup.inner();
    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let query_fg = if palette.query_selected {
        selection_fg
    } else {
        text_fg
    };
    if palette.query_selected {
        if let Some(bg) = selection_bg {
            canvas.draw_rect_fill(inner.row, inner.col, inner.width, 1, bg);
        }
    }
    canvas.draw_text_at(inner.row, inner.col, "> ", prompt_fg);
    canvas.draw_text_at(inner.row, inner.col + 2, &palette.query, query_fg);

    // Virtual "[Create]" row for find-or-create palettes.
    let has_create = palette.has_create_from_query() && !palette.query.is_empty();
    let chrome_rows = 1 + if has_create { 1 } else { 0 };
    if has_create {
        let create_row = inner.row + 1;
        let create_fg = if palette.query_selected {
            selection_fg
        } else {
            prompt_fg
        };
        if palette.query_selected {
            if let Some(bg) = selection_bg {
                canvas.draw_rect_fill(create_row, inner.col, inner.width, 1, bg);
            }
        }
        let hint = format!("[Create] \"{}\"", palette.query);
        canvas.draw_text_at(create_row, inner.col + 1, &hint, create_fg);
    }

    let results_height = inner.height.saturating_sub(chrome_rows);
    let start = if palette.selected >= results_height {
        palette.selected - results_height + 1
    } else {
        0
    };

    // For path-heavy palettes (recent files, projects), use full width since
    // there's no doc column. Otherwise cap name at 40% to leave room for docs.
    let full_width_name = matches!(
        palette.purpose,
        mae_core::command_palette::PalettePurpose::RecentFile
            | mae_core::command_palette::PalettePurpose::SwitchProject
            | mae_core::command_palette::PalettePurpose::SwitchBuffer
            | mae_core::command_palette::PalettePurpose::SetTheme
            | mae_core::command_palette::PalettePurpose::SetSplashArt
            | mae_core::command_palette::PalettePurpose::GitBranch
            | mae_core::command_palette::PalettePurpose::ForgetProject
            | mae_core::command_palette::PalettePurpose::MiniDialog
    );
    let max_name_width = if full_width_name {
        inner.width.saturating_sub(2)
    } else {
        (inner.width * 2 / 5).max(12)
    };
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
        let is_selected = actual_idx == palette.selected && !palette.query_selected;
        let row = inner.row + chrome_rows + display_idx;

        if is_selected {
            if let Some(bg) = selection_bg {
                canvas.draw_rect_fill(row, inner.col, inner.width, 1, bg);
            }
        }

        let fg = if is_selected { selection_fg } else { text_fg };
        let dfg = if is_selected { selection_fg } else { doc_fg };

        let name_display = if display_width(&entry.name) > name_col {
            if full_width_name {
                // For paths, show the end (most distinctive part)
                truncate_start(&entry.name, name_col)
            } else {
                truncate_end(&entry.name, name_col)
            }
        } else {
            format!("{:<w$}", entry.name, w = name_col)
        };

        canvas.draw_text_at(row, inner.col + 1, &name_display, fg);

        if !full_width_name {
            let available_for_doc = inner.width.saturating_sub(name_col + 3);
            let doc_display =
                if display_width(&entry.doc) > available_for_doc && available_for_doc > 1 {
                    truncate_end(&entry.doc, available_for_doc)
                } else {
                    entry.doc.clone()
                };
            canvas.draw_text_at(row, inner.col + 1 + name_col + 2, &doc_display, dfg);
        }
    }
}

// ---------------------------------------------------------------------------
// Which-key popup
// @ai-caution: [which-key] Mirror of TUI which_key_render.rs layout logic. Changes here
// MUST be reflected in the TUI renderer.
// ---------------------------------------------------------------------------

pub fn render_which_key_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    row_start: usize,
    height: usize,
    cols: usize,
    entries: &[mae_core::WhichKeyEntry],
    title_override: Option<&str>,
) {
    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let group_fg = theme::ts_fg(editor, "ui.popup.group");
    let key_fg = theme::ts_fg(editor, "ui.popup.key");
    let text_fg = theme::ts_fg(editor, "ui.popup.text");
    // Doc color: try ui.popup.doc, fallback to dimmed text color
    let doc_fg = {
        let style = editor.theme.style("ui.popup.doc");
        if style.fg.is_some() {
            theme::ts_fg(editor, "ui.popup.doc")
        } else {
            // Dim the text color by reducing alpha
            let mut dimmed = text_fg;
            dimmed.a *= 0.6;
            dimmed
        }
    };
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    let separator = editor
        .get_option("which-key-separator")
        .map(|(v, _)| v)
        .unwrap_or_else(|| " ".to_string());
    let max_desc: usize = editor
        .get_option("which-key-max-desc-length")
        .and_then(|(v, _)| v.parse().ok())
        .unwrap_or(40);

    canvas.draw_rect_fill(row_start, 0, cols, height, bg);
    let title = if let Some(t) = title_override {
        format!(" {} keys ", t)
    } else {
        let breadcrumb: String = editor
            .which_key_prefix
            .iter()
            .map(format_keypress)
            .collect::<Vec<_>>()
            .join(WK_BREADCRUMB_SEP);
        format!(" {} ", breadcrumb)
    };
    draw_border_titled(canvas, row_start, 0, cols, height, border_fg, &title);

    let inner_row = row_start + 1;
    let inner_col = 1_usize;
    let inner_width = cols.saturating_sub(2);
    let inner_height = height.saturating_sub(2);

    let sep_width = display_width(&separator);
    let (col_width, num_cols) = which_key_column_layout(entries, inner_width, sep_width, max_desc);

    // Total rows needed for all entries
    let total_rows = entries.len().div_ceil(num_cols);

    // Clamp scroll offset so it can't go past the last page
    let max_scroll = total_rows.saturating_sub(inner_height);
    let scroll = editor.which_key_scroll.min(max_scroll);

    let skip_entries = scroll * num_cols;
    let show_above = scroll > 0;
    let show_below = total_rows > scroll + inner_height;

    let effective_max_rows = if show_above && show_below {
        inner_height.saturating_sub(2)
    } else if show_above || show_below {
        inner_height.saturating_sub(1)
    } else {
        inner_height
    };

    let mut row = 0;

    // "above" indicator
    if show_above {
        let above_count = skip_entries;
        canvas.draw_text_at(
            inner_row,
            inner_col,
            &format!("\u{2191} +{} above", above_count),
            doc_fg,
        );
        row += 1;
    }

    let visible_entries = &entries[skip_entries..];
    let mut col = 0;
    let mut displayed = 0;

    for entry in visible_entries.iter() {
        if row >= effective_max_rows + if show_above { 1 } else { 0 } {
            break;
        }

        let key_str = format_keypress(&entry.key);
        let (kfg, lfg) = if entry.is_group {
            (group_fg, group_fg)
        } else {
            (key_fg, text_fg)
        };

        let key_w = display_width(&key_str);
        let max_label = col_width.saturating_sub(key_w + sep_width + 1);
        let label_w = display_width(&entry.label);
        let label = if label_w > max_label {
            truncate_end(&entry.label, max_label)
        } else {
            entry.label.clone()
        };
        let actual_label_w = display_width(&label);

        let x = inner_col + col * col_width;
        canvas.draw_text_at(inner_row + row, x, &key_str, kfg);
        let sep_x = x + key_w;
        canvas.draw_text_at(inner_row + row, sep_x, &separator, text_fg);
        let label_x = sep_x + sep_width;
        canvas.draw_text_at(inner_row + row, label_x, &label, lfg);

        // Doc string display for leaf entries
        if !entry.is_group {
            if let Some(ref doc) = entry.doc {
                let used = key_w + sep_width + actual_label_w;
                let remaining = col_width.saturating_sub(used + 2);
                if remaining > WK_DOC_MIN_WIDTH {
                    let trunc = truncate_end(doc, remaining);
                    let doc_x = label_x + actual_label_w + 1;
                    canvas.draw_text_at(inner_row + row, doc_x, &trunc, doc_fg);
                }
            }
        }

        col += 1;
        displayed += 1;
        if col >= num_cols {
            col = 0;
            row += 1;
        }
    }

    // "below" indicator
    if show_below {
        let below_count = entries.len() - skip_entries - displayed;
        if below_count > 0 {
            let indicator_row = inner_row + inner_height.saturating_sub(1);
            canvas.draw_text_at(
                indicator_row,
                inner_col,
                &format!("\u{2193} +{} below", below_count),
                doc_fg,
            );
        }
    }
}

// format_keypress is now shared via mae_core::text_utils::format_keypress

// ---------------------------------------------------------------------------
// Hover popup
// ---------------------------------------------------------------------------

pub fn render_hover_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_row: usize,
    area_width: usize,
    area_height: usize,
    frame_layout: Option<&FrameLayout>,
    win_col_offset: usize,
    win_row_offset: usize,
    _win_width: usize,
    win_height: usize,
) {
    let popup = match &editor.hover_popup {
        Some(p) => p,
        None => return,
    };

    let win = editor.window_mgr.focused_window();
    // Use the saved anchor position, not the live cursor, so the popup
    // stays where the hover was requested.
    let anchor_screen_row = frame_layout
        .and_then(|fl| fl.display_row_of(popup.anchor_row))
        .unwrap_or_else(|| popup.anchor_row.saturating_sub(win.scroll_offset));

    // Dynamic max width: up to full screen width minus 4 cols margin, at least 40 cols.
    // This matches VS Code / Emacs lsp-ui-doc behavior — wide enough for type sigs.
    // Popup may overflow the focused window bounds (intentional — content visibility
    // takes priority over window containment).
    let max_popup_cols = area_width.saturating_sub(4).max(40);
    // Wrap width for content: leave 2 cols for border.
    let wrap_width = max_popup_cols.saturating_sub(2);

    let lines = mae_core::render_common::hover::compute_hover_lines(&popup.contents, wrap_width);
    if lines.is_empty() {
        return;
    }

    let max_visible = editor.hover_max_lines;
    let visible_count = lines.len().min(max_visible);
    // Size popup to content, capped at available space (not a fixed 78).
    let popup_width = lines
        .iter()
        .take(visible_count)
        .map(|l| l.len())
        .max()
        .unwrap_or(20)
        .min(max_popup_cols)
        + 2; // border
    let popup_height = (visible_count + 2).min(area_height.saturating_sub(2)); // border top+bottom, clamped

    // Position below the anchor, offset by the focused window's screen position.
    let abs_anchor_row = win_row_offset + anchor_screen_row;
    let popup_top = if anchor_screen_row + 2 + popup_height < win_height {
        abs_anchor_row + 2
    } else if anchor_screen_row > popup_height {
        abs_anchor_row.saturating_sub(popup_height + 1)
    } else {
        abs_anchor_row.saturating_sub(popup_height)
    };
    // Clamp top to visible area.
    let popup_top = popup_top.clamp(
        area_row,
        area_row + area_height.saturating_sub(popup_height),
    );

    // Horizontal: position at anchor col within the window, clamped to screen.
    let abs_anchor_col = win_col_offset + popup.anchor_col;
    let popup_left = if abs_anchor_col + popup_width <= area_width {
        abs_anchor_col
    } else {
        // Shift left to fit, but don't go past column 0.
        area_width.saturating_sub(popup_width)
    };

    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let text_fg = theme::ts_fg(editor, "ui.popup.text");
    // Use ui.popup bg → ui.popup.text bg → ui.background bg → DEFAULT_BG.
    let bg = theme::ts_bg(editor, "ui.popup")
        .or_else(|| theme::ts_bg(editor, "ui.popup.text"))
        .or_else(|| theme::ts_bg(editor, "ui.background"))
        .unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(popup_top, popup_left, popup_width, popup_height, bg);
    draw_border_titled(
        canvas,
        popup_top,
        popup_left,
        popup_width,
        popup_height,
        border_fg,
        " Hover ",
    );

    let inner_top = popup_top + 1;
    let inner_left = popup_left + 1;
    let inner_width = popup_width.saturating_sub(2);

    let scroll = popup.scroll_offset;
    for (i, line) in lines.iter().skip(scroll).take(visible_count).enumerate() {
        let display: String = line.chars().take(inner_width).collect();
        canvas.draw_text_at(inner_top + i, inner_left, &display, text_fg);
    }

    // Scroll indicator (clear border chars beneath it first to avoid strikethrough artifact).
    if lines.len() > max_visible {
        let indicator = format!("[{}/{}]", scroll + visible_count, lines.len());
        let x = popup_left + popup_width.saturating_sub(indicator.len() + 1);
        canvas.draw_rect_fill(popup_top + popup_height - 1, x, indicator.len(), 1, bg);
        canvas.draw_text_at(popup_top + popup_height - 1, x, &indicator, border_fg);
    }
}

// ---------------------------------------------------------------------------
// Code action popup
// ---------------------------------------------------------------------------

pub fn render_code_action_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_row: usize,
    area_width: usize,
    area_height: usize,
    frame_layout: Option<&FrameLayout>,
    win_col_offset: usize,
    win_row_offset: usize,
    _win_width: usize,
    win_height: usize,
) {
    let menu = match &editor.code_action_menu {
        Some(m) => m,
        None => return,
    };

    let win = editor.window_mgr.focused_window();
    let cursor_screen_row = frame_layout
        .and_then(|fl| fl.display_row_of(win.cursor_row))
        .unwrap_or_else(|| win.cursor_row.saturating_sub(win.scroll_offset));

    if menu.items.is_empty() {
        return;
    }

    const MAX_ITEMS: usize = 12;
    let visible_count = menu.items.len().min(MAX_ITEMS);
    let popup_width = menu
        .items
        .iter()
        .take(MAX_ITEMS)
        .map(|item| {
            let kind_w = item.kind.as_deref().map(|k| k.len() + 3).unwrap_or(2);
            kind_w + item.title.len()
        })
        .max()
        .unwrap_or(20)
        .min(60)
        + 4; // padding + border
    let popup_height = visible_count + 2;

    // Position relative to the focused window's screen rect.
    let abs_row = win_row_offset + cursor_screen_row;
    let popup_top = if cursor_screen_row + 2 + popup_height < win_height {
        abs_row + 1
    } else {
        abs_row.saturating_sub(popup_height)
    };
    let popup_top = popup_top.clamp(
        area_row,
        area_row + area_height.saturating_sub(popup_height),
    );
    let abs_col = win_col_offset + win.cursor_col;
    let popup_left = if abs_col + popup_width <= area_width {
        abs_col
    } else {
        area_width.saturating_sub(popup_width)
    };

    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let normal_fg = theme::ts_fg(editor, "ui.popup.text");
    let normal_bg = theme::ts_bg(editor, "ui.popup")
        .or_else(|| theme::ts_bg(editor, "ui.popup.text"))
        .or_else(|| theme::ts_bg(editor, "ui.background"));
    let selected_fg = theme::ts_fg(editor, "ui.popup.key");
    let selected_bg = theme::ts_bg(editor, "ui.popup.key");

    let bg = normal_bg.unwrap_or(theme::DEFAULT_BG);
    canvas.draw_rect_fill(popup_top, popup_left, popup_width, popup_height, bg);
    draw_border_titled(
        canvas,
        popup_top,
        popup_left,
        popup_width,
        popup_height,
        border_fg,
        " Code Actions ",
    );

    let inner_top = popup_top + 1;
    let inner_left = popup_left + 1;
    let inner_width = popup_width.saturating_sub(2);

    for (i, item) in menu.items.iter().take(MAX_ITEMS).enumerate() {
        let is_selected = i == menu.selected;
        let fg = if is_selected { selected_fg } else { normal_fg };
        let item_bg = if is_selected { selected_bg } else { normal_bg };

        if let Some(bg) = item_bg {
            canvas.draw_rect_fill(inner_top + i, inner_left, inner_width, 1, bg);
        }

        let icon = match item.kind.as_deref() {
            Some(k) if k.contains("quickfix") => "💡",
            Some(k) if k.contains("refactor") => "🔧",
            Some(k) if k.contains("source") => "📦",
            _ => "•",
        };
        let text = format!("{} {}", icon, item.title);
        let display: String = text.chars().take(inner_width).collect();
        canvas.draw_text_at(inner_top + i, inner_left, &display, fg);
    }
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
    draw_border_titled(canvas, row, col, width, height, color, "");
}

/// Draw a box border with an optional title embedded in the top edge.
/// Title is rendered as part of the border string to prevent
/// strikethrough artifacts from dashes overlapping title glyphs in Skia.
fn draw_border_titled(
    canvas: &mut SkiaCanvas,
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    color: Color4f,
    title: &str,
) {
    if width < 2 || height < 2 {
        return;
    }
    let inner_w = width.saturating_sub(2);
    let title_len = title.chars().count();
    let top = if !title.is_empty() && title_len < inner_w {
        let pad = inner_w - title_len;
        format!("┌{}{}┐", title, "─".repeat(pad))
    } else {
        format!("┌{}┐", "─".repeat(inner_w))
    };
    canvas.draw_text_at(row, col, &top, color);

    // Side borders.
    for r in 1..height.saturating_sub(1) {
        canvas.draw_text_at(row + r, col, "│", color);
        canvas.draw_text_at(row + r, col + width - 1, "│", color);
    }

    // Bottom border.
    let bottom = format!("└{}┘", "─".repeat(inner_w));
    canvas.draw_text_at(row + height - 1, col, &bottom, color);
}

// ---------------------------------------------------------------------------
// Mini-dialog renderer (edit-link, rename, etc.)
// ---------------------------------------------------------------------------

fn render_mini_dialog(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    dialog: &mae_core::command_palette::MiniDialogState,
    cols: usize,
    rows: usize,
) {
    // Smaller dialog box than the full palette.
    let dialog_width = 50.min(cols.saturating_sub(4));
    let dialog_height = (4 + dialog.fields.len()).min(rows.saturating_sub(2));
    let col = cols.saturating_sub(dialog_width) / 2;
    let row = rows.saturating_sub(dialog_height) / 2;

    let text_fg = theme::ts_fg(editor, "ui.text");
    let prompt_fg = theme::ts_fg(editor, "ui.popup.key");
    let selection_bg = theme::ts_bg(editor, "ui.selection");
    let border_fg = theme::ts_fg(editor, "ui.window.border.active");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(row, col, dialog_width, dialog_height, bg);
    let title = format!(" {} ", dialog.title());
    draw_border_titled(
        canvas,
        row,
        col,
        dialog_width,
        dialog_height,
        border_fg,
        &title,
    );

    let inner_col = col + 2;
    let inner_width = dialog_width.saturating_sub(4);

    for (i, field) in dialog.fields.iter().enumerate() {
        let field_row = row + 1 + i;
        let is_active = i == dialog.active_field;

        if is_active {
            if let Some(bg) = selection_bg {
                canvas.draw_rect_fill(field_row, col + 1, dialog_width - 2, 1, bg);
            }
        }

        let label = format!("{}: ", field.label);
        canvas.draw_text_at(field_row, inner_col, &label, prompt_fg);

        let value_col = inner_col + label.len();
        let max_value_len = inner_width.saturating_sub(label.len());
        let display_value = if field.value.is_empty() {
            &field.placeholder
        } else {
            &field.value
        };
        let fg = if field.value.is_empty() {
            // Dim placeholder
            theme::ts_fg(editor, "ui.popup.text")
        } else {
            text_fg
        };
        let truncated: String = display_value.chars().take(max_value_len).collect();
        canvas.draw_text_at(field_row, value_col, &truncated, fg);

        // Draw cursor for active field
        if is_active && !field.value.is_empty() {
            let cursor_col = value_col + field.value.len().min(max_value_len);
            canvas.draw_text_at(field_row, cursor_col, "│", text_fg);
        } else if is_active {
            canvas.draw_text_at(field_row, value_col, "│", text_fg);
        }
    }

    // Footer hint
    let footer_row = row + 1 + dialog.fields.len();
    if footer_row < row + dialog_height - 1 {
        let hint = "Tab: next  Enter: apply  Esc: cancel";
        let hint_col = inner_col;
        canvas.draw_text_at(footer_row, hint_col, hint, prompt_fg);
    }
}

// ---------------------------------------------------------------------------
// Signature help popup
// ---------------------------------------------------------------------------

pub fn render_signature_help_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_width: usize,
    area_height: usize,
    frame_layout: Option<&FrameLayout>,
    win_col_offset: usize,
    win_row_offset: usize,
    win_height: usize,
) {
    let state = match &editor.signature_help {
        Some(s) => s,
        None => return,
    };
    if state.signatures.is_empty() {
        return;
    }

    let sig = &state.signatures[state.active_signature.min(state.signatures.len() - 1)];
    let label = &sig.label;

    // Compute popup dimensions based on signature label.
    let popup_width = (label.len() + 4).min(area_width.saturating_sub(2)).max(20);
    let has_doc = sig.documentation.is_some();
    let popup_height = if has_doc { 4 } else { 3 };

    let win = editor.window_mgr.focused_window();
    let anchor_screen_row = frame_layout
        .and_then(|fl| fl.display_row_of(state.anchor_line))
        .unwrap_or_else(|| state.anchor_line.saturating_sub(win.scroll_offset));

    // Position above the cursor (signature help goes above, unlike completion below).
    let abs_row = win_row_offset + anchor_screen_row;
    let popup_top = if abs_row > popup_height {
        abs_row.saturating_sub(popup_height)
    } else if anchor_screen_row + 2 + popup_height < win_height {
        abs_row + 1
    } else {
        0
    };
    let popup_top = popup_top.clamp(0, area_height.saturating_sub(popup_height));

    let abs_col = win_col_offset + state.anchor_col;
    let popup_left = if abs_col + popup_width <= area_width {
        abs_col
    } else {
        area_width.saturating_sub(popup_width)
    };

    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let text_fg = theme::ts_fg(editor, "ui.popup.text");
    let highlight_fg = theme::ts_fg(editor, "ui.popup.key");
    let bg = theme::ts_bg(editor, "ui.popup")
        .or_else(|| theme::ts_bg(editor, "ui.background"))
        .unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(popup_top, popup_left, popup_width, popup_height, bg);
    draw_border_titled(
        canvas,
        popup_top,
        popup_left,
        popup_width,
        popup_height,
        border_fg,
        " Signature ",
    );

    // Draw signature label with active parameter highlighted.
    let inner_top = popup_top + 1;
    let inner_left = popup_left + 1;
    let inner_width = popup_width.saturating_sub(2);

    let active_param = state.active_parameter;
    if active_param < sig.parameters.len() {
        let (ps, pe) = sig.parameters[active_param];
        // Draw in three segments: before, highlighted param, after.
        let before: String = label.chars().take(ps).take(inner_width).collect();
        let param: String = label[ps..pe]
            .chars()
            .take(inner_width.saturating_sub(before.len()))
            .collect();
        let after: String = label[pe..]
            .chars()
            .take(inner_width.saturating_sub(before.len() + param.len()))
            .collect();
        canvas.draw_text_at(inner_top, inner_left, &before, text_fg);
        canvas.draw_text_at(inner_top, inner_left + before.len(), &param, highlight_fg);
        canvas.draw_text_at(
            inner_top,
            inner_left + before.len() + param.len(),
            &after,
            text_fg,
        );
    } else {
        let display: String = label.chars().take(inner_width).collect();
        canvas.draw_text_at(inner_top, inner_left, &display, text_fg);
    }

    // Optional documentation on second line.
    if let Some(doc) = &sig.documentation {
        let doc_line: String = doc
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(inner_width)
            .collect();
        canvas.draw_text_at(inner_top + 1, inner_left, &doc_line, text_fg);
    }
}

// ---------------------------------------------------------------------------
// Peek definition popup
// ---------------------------------------------------------------------------

pub fn render_peek_definition_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_width: usize,
    area_height: usize,
    frame_layout: Option<&FrameLayout>,
    win_col_offset: usize,
    win_row_offset: usize,
    win_height: usize,
) {
    let state = match &editor.peek_state {
        Some(s) => s,
        None => return,
    };

    let content_lines = &state.context_lines;
    if content_lines.is_empty() {
        return;
    }

    let max_width = content_lines.iter().map(|l| l.len()).max().unwrap_or(40);
    let popup_width = (max_width + 4).min(area_width.saturating_sub(4)).max(40);
    let popup_height = (content_lines.len() + 3).min(area_height.saturating_sub(2));

    let win = editor.window_mgr.focused_window();
    let cursor_screen_row = frame_layout
        .and_then(|fl| fl.display_row_of(win.cursor_row))
        .unwrap_or_else(|| win.cursor_row.saturating_sub(win.scroll_offset));

    let abs_row = win_row_offset + cursor_screen_row;
    let popup_top = if cursor_screen_row + 2 + popup_height < win_height {
        abs_row + 1
    } else {
        abs_row.saturating_sub(popup_height)
    };
    let popup_top = popup_top.clamp(0, area_height.saturating_sub(popup_height));

    let popup_left = win_col_offset;

    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let text_fg = theme::ts_fg(editor, "ui.popup.text");
    let highlight_bg = theme::ts_bg(editor, "ui.popup.key").unwrap_or(theme::DEFAULT_BG);
    let bg = theme::ts_bg(editor, "ui.popup")
        .or_else(|| theme::ts_bg(editor, "ui.background"))
        .unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(popup_top, popup_left, popup_width, popup_height, bg);

    // Title with file path.
    let short_path = state
        .file_path
        .rsplit('/')
        .next()
        .unwrap_or(&state.file_path);
    let title = format!(" {}:{} ", short_path, state.line + 1);
    draw_border_titled(
        canvas,
        popup_top,
        popup_left,
        popup_width,
        popup_height,
        border_fg,
        &title,
    );

    let inner_top = popup_top + 1;
    let inner_left = popup_left + 1;
    let inner_width = popup_width.saturating_sub(2);

    let scroll = state.scroll_offset;
    let visible = popup_height.saturating_sub(2);
    for (i, line) in content_lines.iter().skip(scroll).take(visible).enumerate() {
        let display: String = line.chars().take(inner_width).collect();
        let row_idx = scroll + i;
        if row_idx == state.highlight_line {
            // Highlight the definition line.
            canvas.draw_rect_fill(inner_top + i, inner_left, inner_width, 1, highlight_bg);
        }
        canvas.draw_text_at(inner_top + i, inner_left, &display, text_fg);
    }

    // Footer: Enter to jump, Esc to close.
    let footer = "Enter:jump  Esc:close";
    let fx = popup_left + popup_width.saturating_sub(footer.len() + 1);
    canvas.draw_text_at(popup_top + popup_height - 1, fx, footer, border_fg);
}

// ---------------------------------------------------------------------------
// Symbol outline popup (SPC c o)
// ---------------------------------------------------------------------------

pub fn render_symbol_outline_popup(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_width: usize,
    area_height: usize,
) {
    let state = match &editor.symbol_outline {
        Some(s) => s,
        None => return,
    };

    let rect = centered_popup_rect_from(area_width, area_height, 60, 60);

    let border_fg = theme::ts_fg(editor, "ui.window.border");
    let text_fg = theme::ts_fg(editor, "ui.popup.text");
    let dim_fg = theme::ts_fg(editor, "comment");
    let selected_fg = theme::ts_fg(editor, "ui.popup.key");
    let selected_bg = theme::ts_bg(editor, "ui.popup.key");
    let bg = theme::ts_bg(editor, "ui.popup")
        .or_else(|| theme::ts_bg(editor, "ui.background"))
        .unwrap_or(theme::DEFAULT_BG);

    canvas.draw_rect_fill(rect.row, rect.col, rect.width, rect.height, bg);

    let title = if state.filter.is_empty() {
        " Symbol Outline ".to_string()
    } else {
        format!(" Outline: {} ", state.filter)
    };
    draw_border_titled(
        canvas,
        rect.row,
        rect.col,
        rect.width,
        rect.height,
        border_fg,
        &title,
    );

    let inner_top = rect.row + 1;
    let inner_left = rect.col + 1;
    let inner_width = rect.width.saturating_sub(2);
    let visible = rect.height.saturating_sub(2);

    let indices = &state.filtered_indices;
    if indices.is_empty() {
        canvas.draw_text_at(inner_top, inner_left, "(no symbols)", dim_fg);
        return;
    }

    // Scroll so selected is visible.
    let scroll = if state.selected >= visible {
        state.selected + 1 - visible
    } else {
        0
    };

    for (vi, &idx) in indices.iter().skip(scroll).take(visible).enumerate() {
        let entry = &state.entries[idx];
        let is_selected = scroll + vi == state.selected;

        if is_selected {
            if let Some(sbg) = selected_bg {
                canvas.draw_rect_fill(inner_top + vi, inner_left, inner_width, 1, sbg);
            }
        }

        let indent: String = "  ".repeat(entry.depth.min(4));
        let line_num = format!(":{}", entry.line + 1);
        let available = inner_width.saturating_sub(indent.len() + 2 + line_num.len());
        let name: String = entry.name.chars().take(available).collect();
        let text = format!("{}{} {}{}", indent, entry.kind_icon, name, line_num);

        let fg = if is_selected { selected_fg } else { text_fg };
        canvas.draw_text_at(inner_top + vi, inner_left, &text, fg);
    }

    // Footer.
    let footer = format!(
        "[{}/{}] Enter:jump  Esc:close",
        state.selected + 1,
        indices.len()
    );
    let fx = rect.col + rect.width.saturating_sub(footer.len() + 1);
    canvas.draw_text_at(rect.row + rect.height - 1, fx, &footer, border_fg);
}

// ---------------------------------------------------------------------------
// Blame gutter overlay
// ---------------------------------------------------------------------------

pub fn render_blame_gutter(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    win_row_offset: usize,
    win_col_offset: usize,
    win_height: usize,
    visible_start_line: usize,
) {
    let overlay = match &editor.blame_overlay {
        Some(o) if o.buffer_idx == editor.active_buffer_idx() => o,
        _ => return,
    };

    let blame_fg = theme::ts_fg(editor, "comment");
    let bg = theme::ts_bg(editor, "ui.background").unwrap_or(theme::DEFAULT_BG);

    // Render blame annotations in the right margin.
    let gutter_width = 30;

    for row in 0..win_height {
        let line = visible_start_line + row;
        if let Some(entry) = overlay.entries.iter().find(|e| e.final_line == line) {
            let age = format_relative_time(entry.timestamp);
            let author: String = entry.author.chars().take(10).collect();
            let text = format!("{} {} {}", entry.commit_hash, author, age);
            let display: String = text.chars().take(gutter_width).collect();
            // Draw at the right side of the window.
            let col = win_col_offset.saturating_sub(gutter_width + 1);
            canvas.draw_rect_fill(win_row_offset + row, col, gutter_width, 1, bg);
            canvas.draw_text_at(win_row_offset + row, col, &display, blame_fg);
        }
    }
}

fn format_relative_time(timestamp: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let diff = now - timestamp;
    if diff < 0 {
        return "future".to_string();
    }
    let diff = diff as u64;
    if diff < 60 {
        format!("{}s", diff)
    } else if diff < 3600 {
        format!("{}m", diff / 60)
    } else if diff < 86400 {
        format!("{}h", diff / 3600)
    } else if diff < 2592000 {
        format!("{}d", diff / 86400)
    } else if diff < 31536000 {
        format!("{}mo", diff / 2592000)
    } else {
        format!("{}y", diff / 31536000)
    }
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
            shift: false,
        };
        assert_eq!(format_keypress(&kp), "SPC");
    }

    #[test]
    fn format_keypress_ctrl_c() {
        let kp = mae_core::KeyPress {
            key: mae_core::Key::Char('c'),
            ctrl: true,
            alt: false,
            shift: false,
        };
        assert_eq!(format_keypress(&kp), "C-c");
    }
}
