//! Which-key popup rendering (TUI). Draws the grid computed by
//! `mae_core::render_common::which_key::compute_which_key_layout` — column
//! sizing, doc truncation, and scroll clamping all live there and are
//! shared with the GUI renderer (`gui/src/popup_render.rs`); this file only
//! turns the computed cells into ratatui `Span`/`Line`.

use mae_core::render_common::which_key::compute_which_key_layout;
use mae_core::text_utils::{format_keypress, WK_BREADCRUMB_SEP};
use mae_core::Editor;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::theme_convert::ts;

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
            .join(WK_BREADCRUMB_SEP);
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

    let separator = editor.which_key_separator.clone();
    let max_desc: usize = editor.which_key_max_desc_length;
    let sep_width = mae_core::grapheme::display_width_with(&separator, editor.width_policy());

    let layout = compute_which_key_layout(
        entries,
        inner.width as usize,
        inner.height as usize,
        sep_width,
        max_desc,
        editor.which_key_scroll,
        editor.width_policy(),
    );

    let mut lines: Vec<Line> = Vec::new();

    if let Some(above_count) = layout.above_count {
        lines.push(Line::from(Span::styled(
            format!("\u{2191} +{} above", above_count),
            doc_style,
        )));
    }

    let mut current_spans: Vec<Span> = Vec::new();
    let mut current_row = 0usize;
    for cell in &layout.cells {
        if cell.row != current_row {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            current_row = cell.row;
        }

        let (ks, ls) = if cell.is_group {
            (group_style, group_style)
        } else {
            (key_style, text_style)
        };

        current_spans.push(Span::styled(cell.key_text.clone(), ks));
        current_spans.push(Span::styled(separator.clone(), sep_style));
        current_spans.push(Span::styled(cell.label_text.clone(), ls));
        if let Some(ref doc) = cell.doc_text {
            current_spans.push(Span::styled(format!(" {}", doc), doc_style));
        }
        current_spans.push(Span::raw(" ".repeat(cell.trailing_padding)));
    }
    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    if let Some(below_count) = layout.below_count {
        lines.push(Line::from(Span::styled(
            format!("\u{2193} +{} below", below_count),
            doc_style,
        )));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
