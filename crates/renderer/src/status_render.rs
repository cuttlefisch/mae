//! Status bar and command line rendering (terminal backend).
//!
//! Segment building, truncation, and formatting logic lives in
//! `mae_core::render_common::status`.  This module handles ratatui drawing.

use mae_core::render_common::status::{
    build_status_segments, command_line_text, layout_status_segments, mode_label, mode_theme_key,
};
use mae_core::Editor;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

use crate::theme_convert::ts;

pub(crate) fn render_status_bar(frame: &mut Frame, area: Rect, editor: &Editor) {
    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    let mode_str = mode_label(editor);
    let mode_style = ts(editor, mode_theme_key(editor));

    let sl_style = if editor.bell_active() {
        let base = ts(editor, "ui.statusline");
        Style::default()
            .fg(base.bg.unwrap_or(Color::Black))
            .bg(base.fg.unwrap_or(Color::White))
    } else {
        ts(editor, "ui.statusline")
    };

    let mode_len = UnicodeWidthStr::width(mode_str.as_str());
    let avail = (area.width as usize).saturating_sub(mode_len);

    // Build and lay out segments using shared logic.
    // TUI doesn't pass frame_ms (no FPS display in terminal mode).
    let mut segments = build_status_segments(editor, None);
    let layout = layout_status_segments(&mut segments, avail, &buf.name, buf.modified);

    let right_w = UnicodeWidthStr::width(layout.right_text.as_str());
    let remaining = avail
        .saturating_sub(UnicodeWidthStr::width(layout.left_text.as_str()))
        .saturating_sub(right_w);

    // Build right-side spans, applying styled spans for colored badges.
    let right_spans = if layout.right_styled_spans.is_empty() {
        vec![Span::styled(layout.right_text, sl_style)]
    } else {
        let mut spans = Vec::new();
        let mut byte_pos = 0;
        for styled in &layout.right_styled_spans {
            if styled.byte_offset > byte_pos {
                spans.push(Span::styled(
                    layout.right_text[byte_pos..styled.byte_offset].to_string(),
                    sl_style,
                ));
            }
            let span_text = layout.right_text
                [styled.byte_offset..styled.byte_offset + styled.byte_len]
                .to_string();
            spans.push(Span::styled(span_text, ts(editor, styled.style_key)));
            byte_pos = styled.byte_offset + styled.byte_len;
        }
        if byte_pos < layout.right_text.len() {
            spans.push(Span::styled(
                layout.right_text[byte_pos..].to_string(),
                sl_style,
            ));
        }
        spans
    };

    let mut line_spans = vec![
        Span::styled(&mode_str, mode_style),
        Span::styled(layout.left_text, sl_style),
        Span::styled(" ".repeat(remaining), sl_style),
    ];
    line_spans.extend(right_spans);
    let status_line = Line::from(line_spans);

    let paragraph = Paragraph::new(status_line);
    frame.render_widget(paragraph, area);
}

pub(crate) fn render_command_line(frame: &mut Frame, area: Rect, editor: &Editor) {
    let text = command_line_text(editor);
    let style = ts(editor, "ui.commandline");
    let paragraph = Paragraph::new(Span::styled(text, style));
    frame.render_widget(paragraph, area);
}

// Tests for shared status logic live in mae_core::render_common::status::tests.
