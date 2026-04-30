//! Popup overlays: file picker, file browser, command palette, LSP completion.

use mae_core::{Editor, PalettePurpose};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme_convert::ts;

/// Centered popup rect (70% × 60% of the area, clamped).
fn centered_popup_rect(area: Rect) -> Rect {
    let w = (area.width * 70 / 100).max(40).min(area.width);
    let h = (area.height * 60 / 100).max(10).min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

// ---------------------------------------------------------------------------
// LSP completion popup
// ---------------------------------------------------------------------------

pub(crate) fn render_completion_popup(frame: &mut Frame, editor_area: Rect, editor: &Editor) {
    let items = &editor.completion_items;
    if items.is_empty() {
        return;
    }

    let win = editor.window_mgr.focused_window();
    let scroll_row = win.scroll_offset;
    let cursor_screen_row = win.cursor_row.saturating_sub(scroll_row) as u16;
    let cursor_screen_col = win.cursor_col as u16;

    const MAX_ITEMS: usize = 10;
    let visible_count = items.len().min(MAX_ITEMS) as u16;
    let popup_width = items
        .iter()
        .take(MAX_ITEMS)
        .map(|i| {
            let detail_len = i.detail.as_deref().map(|d| d.len() + 2).unwrap_or(0);
            i.label.len() + detail_len + 4
        })
        .max()
        .unwrap_or(20)
        .min(50) as u16;
    let popup_height = visible_count + 2;

    let popup_top = if cursor_screen_row + 1 + popup_height < editor_area.height {
        editor_area.y + cursor_screen_row + 1
    } else {
        editor_area.y + cursor_screen_row.saturating_sub(popup_height)
    };
    let popup_left = (editor_area.x + cursor_screen_col)
        .min(editor_area.x + editor_area.width.saturating_sub(popup_width));

    let popup_area = Rect {
        x: popup_left,
        y: popup_top,
        width: popup_width,
        height: popup_height,
    };

    let border_style = ts(editor, "ui.window.border");
    let normal_style = ts(editor, "ui.popup.text");
    let selected_style = ts(editor, "ui.popup.key");

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(normal_style);
    frame.render_widget(block, popup_area);

    let inner = Rect {
        x: popup_area.x + 1,
        y: popup_area.y + 1,
        width: popup_area.width.saturating_sub(2),
        height: popup_area.height.saturating_sub(2),
    };

    let lines: Vec<Line> = items
        .iter()
        .take(MAX_ITEMS)
        .enumerate()
        .map(|(i, item)| {
            let style = if i == editor.completion_selected {
                selected_style
            } else {
                normal_style
            };
            let sigil = item.kind_sigil;
            let label = &item.label;
            let detail_part = item
                .detail
                .as_deref()
                .map(|d| {
                    let truncated: String = d.chars().take(20).collect();
                    format!("  {}", truncated)
                })
                .unwrap_or_default();
            let text = format!("{} {}{}", sigil, label, detail_part);
            let max_chars = inner.width as usize;
            let display: String = text.chars().take(max_chars).collect();
            Line::styled(display, style)
        })
        .collect();

    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

// ---------------------------------------------------------------------------
// File picker popup
// ---------------------------------------------------------------------------

pub(crate) fn render_file_picker(frame: &mut Frame, area: Rect, editor: &Editor) {
    let picker = match &editor.file_picker {
        Some(p) => p,
        None => return,
    };

    let popup_area = centered_popup_rect(area);

    let clear = ratatui::widgets::Clear;
    frame.render_widget(clear, popup_area);

    let border_style = ts(editor, "ui.window.border.active");
    let match_count = picker.filtered.len();
    let total = picker.candidates.len();
    let title = format!(
        " Find File [{}] ({}/{}) ",
        picker.root_label, match_count, total
    );

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let text_style = ts(editor, "ui.text");
    let selection_style = ts(editor, "ui.selection");
    let prompt_style = ts(editor, "ui.popup.key");

    let query_text_style = if picker.query_selected {
        selection_style
    } else {
        text_style
    };
    let query_line = Line::from(vec![
        Span::styled("> ", prompt_style),
        Span::styled(&picker.query, query_text_style),
    ]);

    let results_height = (inner.height - 1) as usize;

    let mut lines = vec![query_line];

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
        let style = if actual_idx == picker.selected && !picker.query_selected {
            selection_style
        } else {
            text_style
        };

        let max_w = inner.width as usize - 1;
        let display = if path.len() > max_w {
            format!("…{}", &path[path.len() - max_w + 1..])
        } else {
            path.clone()
        };

        lines.push(Line::from(Span::styled(display, style)));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);

    frame.set_cursor_position(Position::new(
        inner.x + 2 + picker.query.len() as u16,
        inner.y,
    ));
}

// ---------------------------------------------------------------------------
// File browser popup (SPC f d)
// ---------------------------------------------------------------------------

pub(crate) fn render_file_browser(frame: &mut Frame, area: Rect, editor: &Editor) {
    let browser = match &editor.file_browser {
        Some(b) => b,
        None => return,
    };

    let popup_area = centered_popup_rect(area);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border.active");
    let match_count = browser.filtered.len();
    let total = browser.entries.len();
    let cwd_display = browser.cwd.display().to_string();
    let title = format!(" {} ({}/{}) ", cwd_display, match_count, total);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let text_style = ts(editor, "ui.text");
    let selection_style = ts(editor, "ui.selection");
    let prompt_style = ts(editor, "ui.popup.key");
    let dir_style = ts(editor, "keyword");

    let query_line = Line::from(vec![
        Span::styled("> ", prompt_style),
        Span::styled(&browser.query, text_style),
    ]);

    let results_height = (inner.height - 1) as usize;
    let mut lines = vec![query_line];

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
        let base_style = if entry.is_dir { dir_style } else { text_style };
        let style = if actual_idx == browser.selected {
            selection_style
        } else {
            base_style
        };

        let mut name = entry.display();
        let max_w = inner.width as usize - 1;
        if name.len() > max_w {
            name = format!("…{}", &name[name.len() - max_w + 1..]);
        }
        lines.push(Line::from(Span::styled(name, style)));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);

    frame.set_cursor_position(Position::new(
        inner.x + 2 + browser.query.len() as u16,
        inner.y,
    ));
}

// ---------------------------------------------------------------------------
// Command palette popup (SPC SPC)
// ---------------------------------------------------------------------------

pub(crate) fn render_command_palette(frame: &mut Frame, area: Rect, editor: &Editor) {
    let palette = match &editor.command_palette {
        Some(p) => p,
        None => return,
    };

    let popup_area = centered_popup_rect(area);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border.active");
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
        PalettePurpose::AiMode => format!(" AI Operating Mode ({}/{}) ", match_count, total),
        PalettePurpose::AiProfile => format!(" AI Prompt Profile ({}/{}) ", match_count, total),
        PalettePurpose::GitBranch => format!(" Git Branch ({}/{}) ", match_count, total),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if inner.height < 2 || inner.width < 4 {
        return;
    }

    let text_style = ts(editor, "ui.text");
    let selection_style = ts(editor, "ui.selection");
    let prompt_style = ts(editor, "ui.popup.key");
    let doc_style = ts(editor, "ui.popup.text");

    let query_line = Line::from(vec![
        Span::styled("> ", prompt_style),
        Span::styled(&palette.query, text_style),
    ]);

    let results_height = (inner.height - 1) as usize;
    let mut lines = vec![query_line];

    let start = if palette.selected >= results_height {
        palette.selected - results_height + 1
    } else {
        0
    };

    // Adaptive name column: fit visible entries, but cap at 40% of inner
    // width so docs always get the majority of space.
    let max_name_width = (inner.width as usize * 2 / 5).max(12);
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
        let row_style = if actual_idx == palette.selected {
            selection_style
        } else {
            text_style
        };
        let doc_row_style = if actual_idx == palette.selected {
            selection_style
        } else {
            doc_style
        };

        let name_display = if entry.name.len() > name_col {
            format!("{:<w$}", &entry.name[..name_col], w = name_col)
        } else {
            format!("{:<w$}", entry.name, w = name_col)
        };

        let available_for_doc = (inner.width as usize).saturating_sub(name_col + 3);
        let doc_display = if entry.doc.len() > available_for_doc && available_for_doc > 1 {
            let mut s = entry.doc[..available_for_doc.saturating_sub(1)].to_string();
            s.push('…');
            s
        } else {
            entry.doc.clone()
        };

        lines.push(Line::from(vec![
            Span::styled(format!(" {}  ", name_display), row_style),
            Span::styled(doc_display, doc_row_style),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);

    frame.set_cursor_position(Position::new(
        inner.x + 2 + palette.query.len() as u16,
        inner.y,
    ));
}
