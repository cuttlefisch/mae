//! Which-key popup rendering.

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

    let col_width = 30_u16;
    let num_cols = (inner.width / col_width).max(1) as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut current_spans: Vec<Span> = Vec::new();
    let mut col = 0;

    for entry in entries {
        let key_str = format_keypress(&entry.key);
        let (ks, ls) = if entry.is_group {
            (group_style, group_style)
        } else {
            (key_style, text_style)
        };

        let max_label = (col_width as usize).saturating_sub(key_str.len() + 2);
        let label = if entry.label.len() > max_label {
            format!("{}..", &entry.label[..max_label.saturating_sub(2)])
        } else {
            entry.label.clone()
        };

        let entry_width = col_width as usize;
        let padding = entry_width.saturating_sub(key_str.len() + 1 + label.len());

        current_spans.push(Span::styled(key_str, ks));
        current_spans.push(Span::raw(" "));
        current_spans.push(Span::styled(label, ls));
        current_spans.push(Span::raw(" ".repeat(padding)));

        col += 1;
        if col >= num_cols {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            col = 0;
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
