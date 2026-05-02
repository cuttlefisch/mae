//! Splash screen — shown when the scratch buffer is empty and focused.
//! Inspired by Doom Emacs's dashboard: ASCII art logo + quick-action hints.
//!
//! Shared constants and data live in `mae_core::render_common::splash`.
//! This module handles ratatui-specific rendering.

use mae_core::render_common::splash::{should_show_splash, ALL_ARTS, MAE_LOGO, QUICK_ACTIONS};
use mae_core::Editor;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::theme_convert::ts;

// Re-export for external use.
pub use mae_core::render_common::splash::splash_action_count;

pub(crate) fn render_splash_if_needed(frame: &mut Frame, area: Rect, editor: &Editor) -> bool {
    if !should_show_splash(editor) {
        return false;
    }
    render_splash(frame, area, editor);
    true
}

fn render_splash(frame: &mut Frame, area: Rect, editor: &Editor) {
    let selected = editor.splash_art.as_deref().unwrap_or("bat");
    let splash = ALL_ARTS
        .iter()
        .find(|a| a.name == selected)
        .unwrap_or(&ALL_ARTS[0]);

    let art_primary = ts(editor, "keyword");
    let art_accent = ts(editor, "string");
    let logo_style = ts(editor, "function");
    let key_style = ts(editor, "type");
    let desc_style = ts(editor, "ui.text");
    let subtitle_style = ts(editor, "comment");

    let mut lines: Vec<Line> = Vec::new();

    // Art lines with two-tone coloring.
    let art_lines: Vec<&str> = splash.art.lines().collect();
    let art_width = art_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    for (i, line) in art_lines.iter().enumerate() {
        let style = if splash.accent_lines.contains(&i) {
            art_accent
        } else {
            art_primary
        };
        lines.push(Line::styled(line.to_string(), style));
    }

    // Helper: center a block of text within art_width.
    let center_block_pad =
        |block_width: usize| -> usize { art_width.saturating_sub(block_width) / 2 };

    // MAE logo.
    let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
    let logo_width = logo_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let logo_pad = center_block_pad(logo_width);
    for line in &logo_lines {
        let padded = format!(
            "{:>pad$}{:<width$}",
            "",
            line,
            pad = logo_pad,
            width = logo_width
        );
        lines.push(Line::styled(padded, logo_style));
    }

    // Subtitle.
    let subtitle = "Modern AI Editor — ai-native lisp machine";
    let sub_pad = art_width.saturating_sub(subtitle.len()) / 2;
    lines.push(Line::styled(
        format!("{:>width$}{}", "", subtitle, width = sub_pad),
        subtitle_style,
    ));
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let ver_pad = center_block_pad(version.len());
    lines.push(Line::styled(
        format!("{:>w$}{}", "", version, w = ver_pad),
        subtitle_style,
    ));
    lines.push(Line::raw(""));

    // Quick actions.
    let qa_width = QUICK_ACTIONS
        .iter()
        .map(|(k, d, _)| format!("{:<10}{}", k, d).len())
        .max()
        .unwrap_or(0);
    let qa_pad = center_block_pad(qa_width);
    let sel_bg = ts(editor, "ui.selection")
        .bg
        .unwrap_or(ratatui::style::Color::DarkGray);
    for (i, &(key, desc, _cmd)) in QUICK_ACTIONS.iter().enumerate() {
        let is_selected = i == editor.splash_selection;
        let mut key_s = key_style;
        let mut desc_s = desc_style;
        if is_selected {
            key_s = key_s.bg(sel_bg).bold();
            desc_s = desc_s.bg(sel_bg).bold();
        }
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(qa_pad)),
            if is_selected {
                Span::styled("▸ ", key_s)
            } else {
                Span::raw("  ")
            },
            Span::styled(format!("{:<10}", key), key_s),
            Span::styled(
                format!("{:<width$}", desc, width = qa_width.saturating_sub(10)),
                desc_s,
            ),
        ]));
    }
    lines.push(Line::raw(""));

    // Recent files (up to 5).
    let recent: Vec<&std::path::Path> = editor
        .recent_files
        .list()
        .iter()
        .take(5)
        .map(|p| p.as_path())
        .collect();
    if !recent.is_empty() {
        let header = "Recent Files";
        let header_pad = center_block_pad(header.len());
        lines.push(Line::styled(
            format!("{:>w$}{}", "", header, w = header_pad),
            subtitle_style,
        ));
        for (i, path) in recent.iter().enumerate() {
            let display = path.display().to_string();
            let truncated = if display.len() > 50 {
                format!("...{}", &display[display.len() - 47..])
            } else {
                display
            };
            lines.push(Line::from(vec![
                Span::raw(" ".repeat(qa_pad)),
                Span::styled(format!("  {}  ", i + 1), key_style),
                Span::styled(truncated, desc_style),
            ]));
        }
        lines.push(Line::raw(""));
    }

    // Dismiss hint.
    let dismiss = "j/k to navigate, Enter to select, any other key to dismiss";
    let dismiss_pad = art_width.saturating_sub(dismiss.len()) / 2;
    lines.push(Line::styled(
        format!("{:>width$}{}", "", dismiss, width = dismiss_pad),
        subtitle_style,
    ));

    // Vertical + horizontal centering.
    let total_height = lines.len() as u16;
    let top_pad = area.height.saturating_sub(total_height) / 2;
    let max_width = lines.iter().map(|l| l.width()).max().unwrap_or(0) as u16;
    let left_pad = area.width.saturating_sub(max_width) / 2;
    let centered_area = Rect {
        x: area.x + left_pad,
        y: area.y + top_pad,
        width: area.width.saturating_sub(left_pad),
        height: area.height.saturating_sub(top_pad),
    };

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, centered_area);
}
