//! Splash screen — shown when the scratch buffer is empty and focused.
//! Inspired by Doom Emacs's dashboard: ASCII art logo + quick-action hints.
//!
//! Shared constants and data live in `mae_core::render_common::splash`.
//! This module handles ratatui-specific rendering.

use mae_core::render_common::splash::{
    resolve_active_splash_art, should_show_splash, MAE_LOGO, QUICK_ACTIONS,
};
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
    // ADR-087 Rule 3: splash art may be a module-registered non-ASCII
    // banner; measure it under the user's width policy.
    let policy = editor.width_policy();
    // Art lookup (custom vs. built-in) is shared with the GUI backend —
    // see `resolve_active_splash_art`'s doc comment for why the rest of the
    // layout isn't (the two backends' centering models have genuinely
    // diverged).
    let (art_str, accent_lines, image_path) = resolve_active_splash_art(editor);

    let art_primary = ts(editor, "keyword");
    let art_accent = ts(editor, "string");
    let logo_style = ts(editor, "function");
    let key_style = ts(editor, "type");
    let desc_style = ts(editor, "ui.text");
    let subtitle_style = ts(editor, "comment");

    let mut lines: Vec<Line> = Vec::new();

    // Art lines with two-tone coloring (TUI always uses ASCII, no images).
    let has_image = image_path.is_some();
    let art_lines: Vec<&str> = art_str.lines().collect();
    // ADR-087: measure in display columns, not bytes — a custom art
    // registered via a module isn't guaranteed to be ASCII-only.
    let art_width = art_lines
        .iter()
        .map(|l| mae_core::grapheme::display_width_with(l, policy))
        .max()
        .unwrap_or(0);
    // When art_width is 0 (image-only art, TUI can't render images),
    // use the dismiss hint width as fallback so text centers properly.
    let art_width = if art_width > 0 { art_width } else { 58 };
    for (i, line) in art_lines.iter().enumerate() {
        let style = if accent_lines.contains(&i) {
            art_accent
        } else {
            art_primary
        };
        lines.push(Line::styled(line.to_string(), style));
    }

    // Helper: center a block of text within art_width.
    let center_block_pad =
        |block_width: usize| -> usize { art_width.saturating_sub(block_width) / 2 };

    // MAE logo (auto-hide when image art is selected — the image IS the logo).
    if editor.splash_show_logo && !has_image {
        let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
        let logo_width = logo_lines
            .iter()
            .map(|l| mae_core::grapheme::display_width_with(l, policy))
            .max()
            .unwrap_or(0);
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
    }

    // Subtitle.
    // ADR-087: the em dash "—" is 3 bytes / 1 display column — byte length
    // (a prior version of this line) overcounts it by 2, mis-centering the
    // subtitle relative to the art block by a couple of columns. The GUI
    // backend's equivalent (`SplashSection::max_width`) already measured in
    // display columns; this brings the TUI copy in line with it.
    let subtitle = "Modern AI Editor — ai-native lisp machine";
    let sub_pad =
        art_width.saturating_sub(mae_core::grapheme::display_width_with(subtitle, policy)) / 2;
    lines.push(Line::styled(
        format!("{:>width$}{}", "", subtitle, width = sub_pad),
        subtitle_style,
    ));
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let ver_pad = center_block_pad(mae_core::grapheme::display_width_with(version, policy));
    lines.push(Line::styled(
        format!("{:>w$}{}", "", version, w = ver_pad),
        subtitle_style,
    ));
    lines.push(Line::raw(""));

    // Quick actions.
    let qa_width = QUICK_ACTIONS
        .iter()
        .map(|(k, d, _)| mae_core::grapheme::display_width_with(&format!("{:<10}{}", k, d), policy))
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

    // Dismiss hint.
    // ADR-087: "·" (middle dot) is 2 bytes / 1 display column — same
    // byte-vs-column fix as the subtitle above.
    let dismiss = "j/k navigate · Enter select";
    let dismiss_pad =
        art_width.saturating_sub(mae_core::grapheme::display_width_with(dismiss, policy)) / 2;
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
