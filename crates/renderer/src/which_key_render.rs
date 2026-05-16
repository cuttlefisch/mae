//! Which-key popup rendering (TUI). Dynamic column layout, doc display, themed separator.
// @ai-caution: [which-key] Column width, doc truncation, and separator rendering must stay
// in sync between TUI and GUI renderers (gui/src/popup_render.rs).
// @ai-caution: [which-key] All string truncation MUST use text_utils::truncate_end() —
// never raw &s[..n] which panics on multi-byte chars. All position calculations MUST use
// text_utils::display_width() not .len() which counts bytes.

use mae_core::text_utils::{
    display_width, format_keypress, truncate_end, which_key_column_layout, WK_BREADCRUMB_SEP,
    WK_DOC_MIN_WIDTH,
};
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

    let separator = editor
        .get_option("which-key-separator")
        .map(|(v, _)| v)
        .unwrap_or_else(|| " ".to_string());
    let max_desc: usize = editor
        .get_option("which-key-max-desc-length")
        .and_then(|(v, _)| v.parse().ok())
        .unwrap_or(40);

    let sep_width = display_width(&separator);
    let (col_width, num_cols) =
        which_key_column_layout(entries, inner.width as usize, sep_width, max_desc);
    let col_width_u16 = col_width as u16;
    let max_rows = inner.height as usize;

    // Total rows needed for all entries
    let total_rows = entries.len().div_ceil(num_cols);

    // Clamp scroll offset so it can't go past the last page
    let max_scroll = total_rows.saturating_sub(max_rows);
    let scroll = editor.which_key_scroll.min(max_scroll);

    // Compute entry skip and visible range
    let skip_entries = scroll * num_cols;
    let show_above = scroll > 0;
    let show_below = total_rows > scroll + max_rows;

    let mut lines: Vec<Line> = Vec::new();

    // "above" indicator on first row
    if show_above {
        let above_count = skip_entries;
        lines.push(Line::from(Span::styled(
            format!("\u{2191} +{} above", above_count),
            doc_style,
        )));
    }

    let effective_max_rows = if show_above && show_below {
        max_rows.saturating_sub(2)
    } else if show_above || show_below {
        max_rows.saturating_sub(1)
    } else {
        max_rows
    };

    let visible_entries = &entries[skip_entries..];
    let mut current_spans: Vec<Span> = Vec::new();
    let mut col = 0;
    let mut displayed = 0;

    for entry in visible_entries.iter() {
        if lines.len() >= effective_max_rows + if show_above { 1 } else { 0 } {
            break;
        }

        let key_str = format_keypress(&entry.key);
        let (ks, ls) = if entry.is_group {
            (group_style, group_style)
        } else {
            (key_style, text_style)
        };

        let key_w = display_width(&key_str);
        let max_label = (col_width_u16 as usize).saturating_sub(key_w + sep_width + 1);
        let label_w = display_width(&entry.label);
        let label = if label_w > max_label {
            truncate_end(&entry.label, max_label)
        } else {
            entry.label.clone()
        };
        let actual_label_w = display_width(&label);

        let entry_width = col_width_u16 as usize;
        let used = key_w + sep_width + actual_label_w;

        current_spans.push(Span::styled(key_str, ks));
        current_spans.push(Span::styled(separator.clone(), sep_style));
        current_spans.push(Span::styled(label, ls));

        // Doc string display for leaf entries
        let mut doc_width = 0;
        if !entry.is_group {
            if let Some(ref doc) = entry.doc {
                let remaining = entry_width.saturating_sub(used + 2);
                if remaining > WK_DOC_MIN_WIDTH {
                    let trunc = truncate_end(doc, remaining);
                    let span_text = format!(" {}", trunc);
                    doc_width = display_width(&span_text);
                    current_spans.push(Span::styled(span_text, doc_style));
                }
            }
        }

        // Pad to fill column (accounting for doc span width)
        let total_used = used + doc_width;
        let padding = entry_width.saturating_sub(total_used);
        current_spans.push(Span::raw(" ".repeat(padding)));

        col += 1;
        displayed += 1;
        if col >= num_cols {
            lines.push(Line::from(std::mem::take(&mut current_spans)));
            col = 0;
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    // "below" indicator
    if show_below {
        let below_count = entries.len() - skip_entries - displayed;
        if below_count > 0 {
            lines.push(Line::from(Span::styled(
                format!("\u{2193} +{} below", below_count),
                doc_style,
            )));
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
}
