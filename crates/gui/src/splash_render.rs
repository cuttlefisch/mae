//! Splash screen rendering for the GUI backend.
//!
//! Shared constants and data live in `mae_core::render_common::splash`.
//! This module handles Skia-specific rendering.

use mae_core::render_common::splash::{ALL_ARTS, MAE_LOGO, QUICK_ACTIONS};
use mae_core::render_common::status::truncate_path;
use mae_core::Editor;

use crate::canvas::SkiaCanvas;
use crate::theme;

pub use mae_core::render_common::splash::should_show_splash;

/// Render the splash screen centered in the available area.
pub fn render_splash(
    canvas: &mut SkiaCanvas,
    editor: &Editor,
    area_row: usize,
    _area_col: usize,
    area_width: usize,
    area_height: usize,
) {
    let selected = editor.splash_art.as_deref().unwrap_or("bat");

    // Check for custom art with image path (GUI-only feature).
    let custom = editor
        .custom_splash_arts
        .iter()
        .find(|a| a.name == selected);
    let image_path = custom.and_then(|c| c.image_path.as_ref());

    // Resolve art text: custom or built-in.
    let (art_str, accent_lines): (&str, &[usize]) = if let Some(c) = custom {
        (c.art.as_str(), &c.accent_lines)
    } else {
        let splash = ALL_ARTS
            .iter()
            .find(|a| a.name == selected)
            .unwrap_or(&ALL_ARTS[0]);
        (splash.art, splash.accent_lines)
    };

    let art_fg = theme::ts_fg(editor, "keyword");
    let art_accent = theme::ts_fg(editor, "string");
    let logo_fg = theme::ts_fg(editor, "function");
    let key_fg = theme::ts_fg(editor, "type");
    let _desc_fg = theme::ts_fg(editor, "ui.text");
    let subtitle_fg = theme::ts_fg(editor, "comment");

    // If this custom art has an image, try to render it centered.
    // We track how many "virtual lines" it occupies so the rest of the
    // splash layout stays consistent. Image rendering happens at the end
    // once we know centering offsets.
    let mut image_lines = 0usize;
    let (cell_w, cell_h) = canvas.cell_size();
    if image_path.is_some() {
        // Reserve vertical space — scale to ~35% of area height.
        let max_img_h = (area_height as f32 * cell_h) * 0.35;
        image_lines = (max_img_h / cell_h).ceil() as usize;
        let _ = cell_w; // used below during image draw
    }

    // Collect all lines: (text, fg_color, is_selected).
    let mut lines: Vec<(String, skia_safe::Color4f, bool)> = Vec::new();

    // Reserve blank lines for image space.
    for _ in 0..image_lines {
        lines.push((String::new(), art_fg, false));
    }

    // Art (ASCII fallback — skip if we have an image).
    let art_lines_vec: Vec<&str> = art_str.lines().collect();
    let art_width = art_lines_vec.iter().map(|l| l.len()).max().unwrap_or(0);
    if image_path.is_none() {
        for (i, line) in art_lines_vec.iter().enumerate() {
            let fg = if accent_lines.contains(&i) {
                art_accent
            } else {
                art_fg
            };
            lines.push((line.to_string(), fg, false));
        }
    }

    // Logo.
    let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
    let logo_width = logo_lines.iter().map(|l| l.len()).max().unwrap_or(0);
    let logo_pad = art_width.saturating_sub(logo_width) / 2;
    for line in &logo_lines {
        let padded = format!("{:>pad$}{}", "", line, pad = logo_pad);
        lines.push((padded, logo_fg, false));
    }

    // Subtitle.
    let subtitle = "Modern AI Editor -- ai-native lisp machine";
    let sub_pad = art_width.saturating_sub(subtitle.len()) / 2;
    lines.push((
        format!("{:>w$}{}", "", subtitle, w = sub_pad),
        subtitle_fg,
        false,
    ));
    let version = concat!("v", env!("CARGO_PKG_VERSION"));
    let ver_pad = art_width.saturating_sub(version.len()) / 2;
    lines.push((
        format!("{:>w$}{}", "", version, w = ver_pad),
        subtitle_fg,
        false,
    ));
    lines.push((String::new(), subtitle_fg, false));

    // Quick actions — with selection highlight.
    let qa_width = QUICK_ACTIONS
        .iter()
        .map(|(k, d, _)| format!("{:<10}{}", k, d).len())
        .max()
        .unwrap_or(0);
    let qa_pad = art_width.saturating_sub(qa_width + 2) / 2;
    let sel_bg = theme::ts_bg(editor, "ui.selection");
    for (i, &(key, desc, _cmd)) in QUICK_ACTIONS.iter().enumerate() {
        let is_selected = i == editor.splash_selection;
        let prefix = if is_selected { "▸ " } else { "  " };
        let text = format!("{:>pad$}{}{:<10}{}", "", prefix, key, desc, pad = qa_pad);
        lines.push((text, key_fg, is_selected));
    }
    lines.push((String::new(), subtitle_fg, false));

    // Recent files (up to 5).
    let recent: Vec<&str> = editor
        .recent_files
        .list()
        .iter()
        .take(5)
        .map(|p| p.to_str().unwrap_or("?"))
        .collect();
    if !recent.is_empty() {
        let header = "Recent Files";
        let header_pad = art_width.saturating_sub(header.len()) / 2;
        lines.push((
            format!("{:>w$}{}", "", header, w = header_pad),
            subtitle_fg,
            false,
        ));
        for (i, path) in recent.iter().enumerate() {
            let label = format!("  {}  {}", i + 1, truncate_path(path, 50));
            let label_pad = art_width.saturating_sub(label.len()) / 2;
            lines.push((format!("{:>w$}{}", "", label, w = label_pad), key_fg, false));
        }
        lines.push((String::new(), subtitle_fg, false));
    }

    // Dismiss hint.
    let dismiss = "j/k to navigate, Enter to select, any other key to dismiss";
    let dismiss_pad = art_width.saturating_sub(dismiss.len()) / 2;
    lines.push((
        format!("{:>w$}{}", "", dismiss, w = dismiss_pad),
        subtitle_fg,
        false,
    ));

    // Centering.
    let total_height = lines.len();
    let top_pad = area_height.saturating_sub(total_height) / 2;
    let max_width = lines.iter().map(|(l, _, _)| l.len()).max().unwrap_or(0);
    let left_pad = area_width.saturating_sub(max_width) / 2;

    for (i, (text, fg, selected)) in lines.iter().enumerate() {
        let row = area_row + top_pad + i;
        if row >= area_row + area_height {
            break;
        }
        if *selected {
            if let Some(bg) = sel_bg {
                canvas.draw_rect_fill(row, left_pad, text.len(), 1, bg);
            }
            canvas.draw_text_bold(row, left_pad, text, *fg);
        } else {
            canvas.draw_text_at(row, left_pad, text, *fg);
        }
    }

    // Draw image splash if available (over the reserved blank lines).
    if let Some(img) = image_path {
        let max_img_w = area_width as f32 * cell_w * 0.4;
        let max_img_h = image_lines as f32 * cell_h;
        // Center horizontally, place at the reserved top region.
        let img_x = (area_width as f32 * cell_w - max_img_w) / 2.0;
        let img_y = (area_row + top_pad) as f32 * cell_h;
        canvas.draw_image_from_cache(img, img_x, img_y, max_img_w, max_img_h);
    }
}

// Tests for shared splash logic live in mae_core::render_common::splash::tests.
