//! Popup overlays: file picker, file browser, command palette, LSP completion,
//! hover popup, code action menu.

use mae_core::text_utils::centered_popup_dims;
use mae_core::Editor;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::theme_convert::ts;

/// Centered popup rect using the shared layout computation and editor options.
fn centered_popup_rect(area: Rect, editor: &Editor) -> Rect {
    let (w, h, x, y) = centered_popup_dims(
        area.width as usize,
        area.height as usize,
        editor.popup_width_pct,
        editor.popup_height_pct,
        40,
        10,
    );
    Rect::new(area.x + x as u16, area.y + y as u16, w as u16, h as u16)
}

// ---------------------------------------------------------------------------
// LSP completion popup
// ---------------------------------------------------------------------------

pub(crate) fn render_completion_popup(frame: &mut Frame, editor_area: Rect, editor: &Editor) {
    let items = &editor.lsp.completion_items;
    if items.is_empty() {
        return;
    }

    let win = editor.window_mgr.focused_window();
    let scroll_row = win.scroll_offset;
    let cursor_screen_row = win.cursor_row.saturating_sub(scroll_row) as u16;
    let cursor_screen_col = win.cursor_col as u16;

    let max_items = editor.completion_max_items;
    let visible_count = items.len().min(max_items) as u16;
    let popup_width = items
        .iter()
        .take(max_items)
        .map(|i| {
            let detail_len = i.detail.as_deref().map(|d| d.len() + 2).unwrap_or(0);
            i.label.len() + detail_len + 4
        })
        .max()
        .unwrap_or(20)
        .min(50) as u16;
    let popup_height = (visible_count + 2).min(editor_area.height.saturating_sub(2));

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
        .take(max_items)
        .enumerate()
        .map(|(i, item)| {
            let style = if i == editor.lsp.completion_selected {
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

    let popup_area = centered_popup_rect(area, editor);

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

    let popup_area = centered_popup_rect(area, editor);

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
    // If a mini-dialog is active, render that instead of the fuzzy palette.
    if let Some(ref dialog) = editor.mini_dialog {
        render_mini_dialog(frame, area, editor, dialog);
        return;
    }

    let palette = match &editor.command_palette {
        Some(p) => p,
        None => return,
    };

    let popup_area = centered_popup_rect(area, editor);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border.active");
    let match_count = palette.filtered.len();
    let total = palette.entries.len();

    let title = format!(" {} ({}/{}) ", palette.purpose.label(), match_count, total);

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

    let query_style = if palette.query_selected {
        selection_style
    } else {
        text_style
    };
    let query_line = Line::from(vec![
        Span::styled("> ", prompt_style),
        Span::styled(&palette.query, query_style),
    ]);

    // Reserve a row for the "[Create]" hint when applicable.
    let has_create = palette.has_create_from_query() && !palette.query.is_empty();
    let chrome_rows = 1 + if has_create { 1 } else { 0 }; // query line + optional create hint
    let results_height = (inner.height as usize).saturating_sub(chrome_rows);
    let mut lines = vec![query_line];

    // Virtual "[Create]" row for find-or-create palettes.
    if has_create {
        let create_style = if palette.query_selected {
            selection_style
        } else {
            prompt_style
        };
        let hint = format!(" [Create] \"{}\"", palette.query);
        lines.push(Line::from(Span::styled(hint, create_style)));
    }

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
        (inner.width as usize).saturating_sub(2)
    } else {
        (inner.width as usize * 2 / 5).max(12)
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
        let row_style = if is_selected {
            selection_style
        } else {
            text_style
        };
        let doc_row_style = if is_selected {
            selection_style
        } else {
            doc_style
        };

        let name_display = if entry.name.len() > name_col {
            if full_width_name {
                // For paths, show the end (most distinctive part)
                let skip = entry.name.len() - name_col + 1;
                format!("…{}", &entry.name[skip..])
            } else {
                format!("{:<w$}", &entry.name[..name_col], w = name_col)
            }
        } else {
            format!("{:<w$}", entry.name, w = name_col)
        };

        if full_width_name {
            lines.push(Line::from(Span::styled(
                format!(" {}", name_display),
                row_style,
            )));
        } else {
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
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);

    frame.set_cursor_position(Position::new(
        inner.x + 2 + palette.query.len() as u16,
        inner.y,
    ));
}

// ---------------------------------------------------------------------------
// Hover popup
// ---------------------------------------------------------------------------

pub(crate) fn render_hover_popup(frame: &mut Frame, editor_area: Rect, editor: &Editor) {
    let popup = match &editor.lsp.hover_popup {
        Some(p) => p,
        None => return,
    };

    let lines = mae_core::render_common::hover::compute_hover_lines(&popup.contents, 76);
    if lines.is_empty() {
        return;
    }

    let win = editor.window_mgr.focused_window();
    let cursor_screen_row = win.cursor_row.saturating_sub(win.scroll_offset) as u16;

    let max_visible = editor.hover_max_lines;
    let visible_count = lines.len().min(max_visible) as u16;
    let popup_width = lines
        .iter()
        .take(max_visible)
        .map(|l| l.len())
        .max()
        .unwrap_or(20)
        .min(76) as u16
        + 2;
    let popup_height = (visible_count + 2).min(editor_area.height.saturating_sub(2));

    // Position below cursor with a 1-line gap so the trigger line stays visible.
    let popup_top = if cursor_screen_row + 2 + popup_height < editor_area.height {
        editor_area.y + cursor_screen_row + 2
    } else if cursor_screen_row > popup_height {
        editor_area.y + cursor_screen_row.saturating_sub(popup_height + 1)
    } else {
        editor_area.y + cursor_screen_row.saturating_sub(popup_height)
    };
    let popup_left = (editor_area.x + win.cursor_col as u16)
        .min(editor_area.x + editor_area.width.saturating_sub(popup_width));

    let popup_area = Rect {
        x: popup_left,
        y: popup_top,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border");
    let text_style = ts(editor, "ui.popup.text");

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" Hover ")
        .style(text_style);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let scroll = popup.scroll_offset;
    let content_lines: Vec<Line> = lines
        .iter()
        .skip(scroll)
        .take(max_visible)
        .map(|l| Line::styled(l.as_str(), text_style))
        .collect();

    let para = Paragraph::new(content_lines);
    frame.render_widget(para, inner);
}

// ---------------------------------------------------------------------------
// KB-link hover preview popup (KB-graph-view plan, Part D)
// ---------------------------------------------------------------------------

/// Mirrors `render_hover_popup` above, with ONE deliberate correction: this
/// positions off `popup.anchor_row`/`anchor_col` (where the preview was
/// requested), not the live cursor. `render_hover_popup` above uses
/// `win.cursor_row`/`cursor_col` directly — a known inconsistency with its
/// GUI counterpart (which already anchors correctly) — and that bug is not
/// propagated here: if the cursor has moved since the popup was requested
/// (e.g. scrolled while the popup remains from a moment ago), the anchor
/// still reflects where the link actually was, matching the GUI's behavior.
pub(crate) fn render_kb_preview_popup(frame: &mut Frame, editor_area: Rect, editor: &Editor) {
    let popup = match editor.kb_preview_popup() {
        Some(p) => p,
        None => return,
    };

    let lines = mae_core::render_common::hover::compute_hover_lines(&popup.contents, 76);
    if lines.is_empty() {
        return;
    }

    let win = editor.window_mgr.focused_window();
    // Anchor position (not the live cursor — see doc comment above).
    let anchor_screen_row = popup.anchor_row.saturating_sub(win.scroll_offset) as u16;

    let max_visible = editor.kb_preview_max_lines;
    let visible_count = lines.len().min(max_visible) as u16;
    let popup_width = lines
        .iter()
        .take(max_visible)
        .map(|l| l.len())
        .max()
        .unwrap_or(20)
        .min(76) as u16
        + 2;
    let popup_height = (visible_count + 2).min(editor_area.height.saturating_sub(2));

    let popup_top = if anchor_screen_row + 2 + popup_height < editor_area.height {
        editor_area.y + anchor_screen_row + 2
    } else if anchor_screen_row > popup_height {
        editor_area.y + anchor_screen_row.saturating_sub(popup_height + 1)
    } else {
        editor_area.y + anchor_screen_row.saturating_sub(popup_height)
    };
    let popup_left = (editor_area.x + popup.anchor_col as u16)
        .min(editor_area.x + editor_area.width.saturating_sub(popup_width));

    let popup_area = Rect {
        x: popup_left,
        y: popup_top,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border");
    let text_style = ts(editor, "ui.popup.text");

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" KB Preview ")
        .style(text_style);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let scroll = popup.scroll_offset;
    let content_lines: Vec<Line> = lines
        .iter()
        .skip(scroll)
        .take(max_visible)
        .map(|l| Line::styled(l.as_str(), text_style))
        .collect();

    let para = Paragraph::new(content_lines);
    frame.render_widget(para, inner);
}

// ---------------------------------------------------------------------------
// Code action popup
// ---------------------------------------------------------------------------

pub(crate) fn render_code_action_popup(frame: &mut Frame, editor_area: Rect, editor: &Editor) {
    let menu = match &editor.lsp.code_action_menu {
        Some(m) => m,
        None => return,
    };
    if menu.items.is_empty() {
        return;
    }

    let win = editor.window_mgr.focused_window();
    let cursor_screen_row = win.cursor_row.saturating_sub(win.scroll_offset) as u16;

    let max_items = editor.code_action_max_items;
    let visible_count = menu.items.len().min(max_items) as u16;
    let popup_width = menu
        .items
        .iter()
        .take(max_items)
        .map(|item| item.title.len() + 4)
        .max()
        .unwrap_or(20)
        .min(60) as u16
        + 2;
    let popup_height = visible_count + 2;

    let popup_top = if cursor_screen_row + 2 + popup_height < editor_area.height {
        editor_area.y + cursor_screen_row + 1
    } else {
        editor_area.y + cursor_screen_row.saturating_sub(popup_height)
    };
    let popup_left = (editor_area.x + win.cursor_col as u16)
        .min(editor_area.x + editor_area.width.saturating_sub(popup_width));

    let popup_area = Rect {
        x: popup_left,
        y: popup_top,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border");
    let normal_style = ts(editor, "ui.popup.text");
    let selected_style = ts(editor, "ui.popup.key");

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(" Code Actions ")
        .style(normal_style);

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let content_lines: Vec<Line> = menu
        .items
        .iter()
        .take(max_items)
        .enumerate()
        .map(|(i, item)| {
            let style = if i == menu.selected {
                selected_style
            } else {
                normal_style
            };
            let icon = match item.kind.as_deref() {
                Some(k) if k.contains("quickfix") => "* ",
                Some(k) if k.contains("refactor") => "~ ",
                Some(k) if k.contains("source") => "+ ",
                _ => "- ",
            };
            Line::styled(format!("{}{}", icon, item.title), style)
        })
        .collect();

    let para = Paragraph::new(content_lines);
    frame.render_widget(para, inner);
}

// ---------------------------------------------------------------------------
// Mini-dialog renderer (edit-link, rename, etc.)
// ---------------------------------------------------------------------------

fn render_mini_dialog(
    frame: &mut Frame,
    area: Rect,
    editor: &Editor,
    dialog: &mae_core::command_palette::MiniDialogState,
) {
    // Content-adaptive geometry (B-23): the box grows to fit its title/body/fields
    // and wraps long content (e.g. the host-key fingerprint) instead of clipping.
    // Shared with the GUI via `render_common::dialog` so the two can't diverge.
    use mae_core::render_common::dialog::{mini_dialog_layout, DialogLine};
    let layout = mini_dialog_layout(dialog, area.width as usize, area.height as usize);
    let popup_area = Rect::new(
        area.x + layout.col as u16,
        area.y + layout.row as u16,
        layout.width as u16,
        layout.height as u16,
    );

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border.active");
    let title = format!(" {} ", dialog.title());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(title);
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let text_style = ts(editor, "ui.text");
    let prompt_style = ts(editor, "ui.popup.key");
    let selected_style = ts(editor, "ui.selection");

    for (i, line) in layout.lines.iter().enumerate() {
        if i as u16 >= inner.height {
            break;
        }
        let row_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let (text, style) = match line {
            DialogLine::Text(t) => (t.clone(), text_style),
            DialogLine::Hint(h) => (h.clone(), prompt_style),
            DialogLine::Field {
                label,
                value,
                placeholder,
                active,
                ..
            } => {
                let display_value = if value.is_empty() { placeholder } else { value };
                let cursor = if *active { "│" } else { "" };
                let style = if *active { selected_style } else { text_style };
                (format!("{label}: {display_value}{cursor}"), style)
            }
        };
        frame.render_widget(Paragraph::new(Line::styled(text, style)), row_area);
    }
}

/// Render signature help popup (TUI).
pub(crate) fn render_signature_help_popup(frame: &mut Frame, area: Rect, editor: &Editor) {
    let state = match &editor.lsp.signature_help {
        Some(s) => s,
        None => return,
    };
    if state.signatures.is_empty() {
        return;
    }

    let sig = &state.signatures[state.active_signature.min(state.signatures.len() - 1)];
    let width = (sig.label.len() as u16 + 4).min(area.width).max(20);
    let height = if sig.documentation.is_some() { 4 } else { 3 };

    let win = editor.window_mgr.focused_window();
    let anchor_row = state.anchor_line.saturating_sub(win.scroll_offset) as u16;

    let top = if anchor_row > height {
        area.y + anchor_row.saturating_sub(height)
    } else {
        area.y + anchor_row + 1
    };
    let top = top.min(area.y + area.height.saturating_sub(height));
    let left = area.x + (state.anchor_col as u16).min(area.width.saturating_sub(width));

    let popup_area = Rect::new(left, top, width, height);
    let block = Block::default().title(" Signature ").borders(Borders::ALL);
    let inner = block.inner(popup_area);
    frame.render_widget(Clear, popup_area);
    frame.render_widget(block, popup_area);

    if inner.height > 0 {
        let label_area = Rect::new(inner.x, inner.y, inner.width, 1);
        let display: String = sig.label.chars().take(inner.width as usize).collect();
        frame.render_widget(Paragraph::new(display), label_area);
    }
    if let Some(doc) = &sig.documentation {
        if inner.height > 1 {
            let doc_area = Rect::new(inner.x, inner.y + 1, inner.width, 1);
            let doc_line: String = doc
                .lines()
                .next()
                .unwrap_or("")
                .chars()
                .take(inner.width as usize)
                .collect();
            frame.render_widget(Paragraph::new(doc_line), doc_area);
        }
    }
}

/// Render peek definition popup (TUI).
pub(crate) fn render_peek_definition_popup(frame: &mut Frame, area: Rect, editor: &Editor) {
    let state = match &editor.lsp.peek_state {
        Some(s) => s,
        None => return,
    };
    if state.context_lines.is_empty() {
        return;
    }

    let max_width = state
        .context_lines
        .iter()
        .map(|l| l.len())
        .max()
        .unwrap_or(40);
    let width = ((max_width + 4) as u16).min(area.width).max(40);
    let height = ((state.context_lines.len() + 2) as u16).min(area.height.saturating_sub(2));

    let win = editor.window_mgr.focused_window();
    let cursor_row = win.cursor_row.saturating_sub(win.scroll_offset) as u16;

    let top = if cursor_row + 2 + height < area.height {
        area.y + cursor_row + 1
    } else if cursor_row > height {
        area.y + cursor_row.saturating_sub(height)
    } else {
        area.y
    };
    let top = top.min(area.y + area.height.saturating_sub(height));

    let short_path = state
        .file_path
        .rsplit('/')
        .next()
        .unwrap_or(&state.file_path);
    let title = format!(" {}:{} ", short_path, state.line + 1);

    let popup_area = Rect::new(area.x, top, width, height);
    let block = Block::default().title(title).borders(Borders::ALL);
    let inner = block.inner(popup_area);
    frame.render_widget(Clear, popup_area);
    frame.render_widget(block, popup_area);

    let visible = inner.height as usize;
    let scroll = state.scroll_offset;
    for (i, line) in state
        .context_lines
        .iter()
        .skip(scroll)
        .take(visible)
        .enumerate()
    {
        let row_area = Rect::new(inner.x, inner.y + i as u16, inner.width, 1);
        let display: String = line.chars().take(inner.width as usize).collect();
        let style = if scroll + i == state.highlight_line {
            Style::default().bg(Color::DarkGray)
        } else {
            Style::default()
        };
        frame.render_widget(Paragraph::new(Line::styled(display, style)), row_area);
    }
}

// ---------------------------------------------------------------------------
// Symbol outline popup
// ---------------------------------------------------------------------------

pub(crate) fn render_symbol_outline_popup(frame: &mut Frame, editor_area: Rect, editor: &Editor) {
    let state = match &editor.lsp.symbol_outline {
        Some(s) => s,
        None => return,
    };
    if state.entries.is_empty() {
        return;
    }

    let max_items = editor.symbol_outline_max_items;
    let filtered_count = state.filtered_indices.len();
    let visible_count = filtered_count.min(max_items) as u16;
    // +2 for border, +1 for filter line
    let popup_height = visible_count + 3;
    let popup_width = (editor_area.width * 3 / 4).clamp(30, 60);
    let popup_left = editor_area.x + (editor_area.width.saturating_sub(popup_width)) / 2;
    let popup_top = editor_area.y + (editor_area.height.saturating_sub(popup_height)) / 2;

    let popup_area = Rect {
        x: popup_left,
        y: popup_top,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let border_style = ts(editor, "ui.window.border");
    let normal_style = ts(editor, "ui.popup.text");
    let selected_style = ts(editor, "ui.popup.key");

    let title = format!(" Outline [{}/{}] ", filtered_count, state.entries.len());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    // Filter line
    if inner.height > 0 {
        let filter_text = if state.filter.is_empty() {
            "Type to filter...".to_string()
        } else {
            state.filter.clone()
        };
        let filter_style = if state.filter.is_empty() {
            Style::default().fg(Color::DarkGray)
        } else {
            normal_style
        };
        let filter_area = Rect::new(inner.x, inner.y, inner.width, 1);
        frame.render_widget(
            Paragraph::new(Line::styled(filter_text, filter_style)),
            filter_area,
        );
    }

    // Entries
    let entries_start = inner.y + 1;
    let entries_height = inner.height.saturating_sub(1) as usize;
    // Scroll if selected is beyond visible window
    let scroll = if state.selected >= entries_height {
        state.selected - entries_height + 1
    } else {
        0
    };

    for (i, &idx) in state
        .filtered_indices
        .iter()
        .skip(scroll)
        .take(entries_height)
        .enumerate()
    {
        let entry = &state.entries[idx];
        let row_area = Rect::new(inner.x, entries_start + i as u16, inner.width, 1);
        let indent = "  ".repeat(entry.depth);
        let line_num = format!("{:>4}", entry.line + 1);
        let display = format!(
            "{} {}{} {} {}",
            entry.kind_icon,
            indent,
            entry.name,
            line_num,
            entry.detail.as_deref().unwrap_or("")
        );
        let display: String = display.chars().take(inner.width as usize).collect();
        let style = if scroll + i == state.selected {
            selected_style
        } else {
            normal_style
        };
        frame.render_widget(Paragraph::new(Line::styled(display, style)), row_area);
    }
}
