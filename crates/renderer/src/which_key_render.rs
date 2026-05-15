//! Which-key popup rendering (TUI). Dynamic column layout, doc display, themed separator.
// @ai-caution: [which-key] Column width, doc truncation, and separator rendering must stay
// in sync between TUI and GUI renderers (gui/src/popup_render.rs).

use mae_core::{Editor, Key};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme_convert::ts;

pub(crate) fn format_keypress(kp: &mae_core::KeyPress) -> String {
    let mut s = String::new();
    if kp.ctrl {
        s.push_str("C-");
    }
    if kp.alt {
        s.push_str("M-");
    }
    match &kp.key {
        Key::Char(' ') => s.push_str("SPC"),
        Key::Char(c) => s.push(*c),
        Key::Escape => s.push_str("Esc"),
        Key::Enter => s.push_str("Enter"),
        Key::Tab => s.push_str("Tab"),
        Key::Backspace => s.push_str("BS"),
        Key::Up => s.push_str("Up"),
        Key::Down => s.push_str("Down"),
        Key::Left => s.push_str("Left"),
        Key::Right => s.push_str("Right"),
        Key::F(n) => {
            s.push_str(&format!("F{}", n));
        }
        _ => s.push('?'),
    }
    s
}

pub(crate) fn render_which_key_popup(
    frame: &mut Frame,
    area: Rect,
    editor: &Editor,
    entries: &[mae_core::WhichKeyEntry],
    title_override: Option<&str>,
) {
    let title = if let Some(t) = title_override {
        format!(" {} keys ", t)
    } else {
        let breadcrumb: String = editor
            .which_key_prefix
            .iter()
            .map(format_keypress)
            .collect::<Vec<_>>()
            .join(" > ");
        format!(" {} ", breadcrumb)
    };

    let popup_border = ts(editor, "ui.window.border");
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(popup_border)
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let group_style = ts(editor, "ui.popup.group");
    let key_style = ts(editor, "ui.popup.key");
    let text_style = ts(editor, "ui.popup.text");
    let sep_style =
        ts(editor, "ui.popup.separator").patch(Style::default().add_modifier(Modifier::DIM));
    let doc_style = ts(editor, "ui.popup.doc").patch(Style::default().add_modifier(Modifier::DIM));

    let separator = editor
        .get_option("which-key-separator")
        .map(|(v, _)| v)
        .unwrap_or_else(|| " ".to_string());
    let max_desc: usize = editor
        .get_option("which-key-max-desc-length")
        .and_then(|(v, _)| v.parse().ok())
        .unwrap_or(40);

    // Dynamic column width based on content
    let max_entry_w = entries
        .iter()
        .map(|e| format_keypress(&e.key).len() + separator.len() + e.label.len().min(max_desc))
        .max()
        .unwrap_or(20);
    let col_width = (max_entry_w + 2).clamp(25, 60) as u16;
    let num_cols = (inner.width / col_width).max(1) as usize;
    let max_rows = inner.height as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut current_spans: Vec<Span> = Vec::new();
    let mut col = 0;
    let mut displayed = 0;

    for (i, entry) in entries.iter().enumerate() {
        let key_str = format_keypress(&entry.key);
        let (ks, ls) = if entry.is_group {
            (group_style, group_style)
        } else {
            (key_style, text_style)
        };

        let max_label = (col_width as usize).saturating_sub(key_str.len() + separator.len() + 1);
        let label = if entry.label.len() > max_label {
            format!("{}..", &entry.label[..max_label.saturating_sub(2)])
        } else {
            entry.label.clone()
        };

        let entry_width = col_width as usize;
        let used = key_str.len() + separator.len() + label.len();

        current_spans.push(Span::styled(key_str, ks));
        current_spans.push(Span::styled(separator.clone(), sep_style));
        current_spans.push(Span::styled(label, ls));

        // Doc string display for leaf entries
        if !entry.is_group {
            if let Some(ref doc) = entry.doc {
                let remaining = entry_width.saturating_sub(used + 2);
                if remaining > 8 {
                    let trunc = if doc.len() > remaining {
                        format!("{}..", &doc[..remaining.saturating_sub(2)])
                    } else {
                        doc.clone()
                    };
                    current_spans.push(Span::styled(format!(" {}", trunc), doc_style));
                }
            }
        }

        // Pad to fill column
        let padding = entry_width.saturating_sub(used);
        current_spans.push(Span::raw(" ".repeat(padding)));

        col += 1;
        displayed += 1;
        if col >= num_cols {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            col = 0;
        }

        // Overflow indicator
        if lines.len() >= max_rows && i + 1 < entries.len() {
            let remaining_count = entries.len() - displayed;
            if remaining_count > 0 {
                // Replace the last line with an overflow indicator
                if let Some(last) = lines.last_mut() {
                    *last = Line::from(Span::styled(
                        format!("… +{} more", remaining_count),
                        doc_style,
                    ));
                }
            }
            break;
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
