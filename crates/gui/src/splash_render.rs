//! Splash screen rendering for the GUI backend.
//!
//! Shared constants and data live in `mae_core::render_common::splash`.
//! This module handles Skia-specific rendering.
//!
//! ## Centering model (Doom-style)
//!
//! Each visual section (image, logo, subtitle, quick actions, recent files,
//! dismiss) is centered independently as a block.  Lines within a section
//! share the same `left_pad` so they are left-aligned to each other, while
//! the block as a whole is centered in `area_width`.

use mae_core::render_common::splash::{ALL_ARTS, MAE_LOGO, QUICK_ACTIONS};
use mae_core::Editor;
use unicode_width::UnicodeWidthStr;

use crate::canvas::SkiaCanvas;
use crate::theme;

pub use mae_core::render_common::splash::should_show_splash;

/// A line within a splash section.
struct SplashLine {
    text: String,
    fg: skia_safe::Color4f,
    is_selected: bool,
}

/// A group of lines that share centering — the block is centered in
/// `area_width` by its widest member, and all lines start at the same column.
struct SplashSection {
    lines: Vec<SplashLine>,
}

impl SplashSection {
    fn new() -> Self {
        Self { lines: Vec::new() }
    }

    fn push(&mut self, text: String, fg: skia_safe::Color4f, is_selected: bool) {
        self.lines.push(SplashLine {
            text,
            fg,
            is_selected,
        });
    }

    fn max_width(&self) -> usize {
        self.lines.iter().map(|l| l.text.width()).max().unwrap_or(0)
    }

    fn height(&self) -> usize {
        self.lines.len()
    }

    fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

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
    let subtitle_fg = theme::ts_fg(editor, "comment");
    let sel_bg = theme::ts_bg(editor, "ui.selection");

    let (cell_w, cell_h) = canvas.cell_size();
    let has_image = image_path.is_some();

    // --- Compute image rendering area ---
    let width_pct = editor.splash_image_width.clamp(10, 80) as f32 / 100.0;
    let area_px_w = area_width as f32 * cell_w;
    let render_box_w = area_px_w * width_pct;
    let render_box_h = area_height as f32 * cell_h * 0.35;

    let mut image_lines = 0usize;
    let mut img_actual_w = 0.0f32;
    let mut img_actual_h = 0.0f32;
    if let Some(img) = image_path {
        if let Some((nat_w, nat_h)) = canvas.image_natural_size(img) {
            let scale = (render_box_w / nat_w as f32).min(render_box_h / nat_h as f32);
            img_actual_w = nat_w as f32 * scale;
            img_actual_h = nat_h as f32 * scale;
        } else {
            // SVG: no intrinsic size, use render box directly.
            img_actual_w = render_box_w;
            img_actual_h = render_box_h;
        }
        image_lines = (img_actual_h / cell_h).ceil() as usize;
    }

    // --- Build sections ---
    let mut sections: Vec<SplashSection> = Vec::new();

    // Section 0: Image placeholder (blank lines, pixel-positioned later).
    if image_lines > 0 {
        let mut sec = SplashSection::new();
        for _ in 0..image_lines {
            sec.push(String::new(), art_fg, false);
        }
        sections.push(sec);
    }

    // Section 1: ASCII art (skip when image present).
    if !has_image {
        let art_lines_vec: Vec<&str> = art_str.lines().collect();
        if !art_lines_vec.is_empty() {
            let mut sec = SplashSection::new();
            for (i, line) in art_lines_vec.iter().enumerate() {
                let fg = if accent_lines.contains(&i) {
                    art_accent
                } else {
                    art_fg
                };
                sec.push(line.to_string(), fg, false);
            }
            sections.push(sec);
        }
    }

    // Section 2: Logo (auto-hide when image present — the image IS the logo).
    if editor.splash_show_logo && !has_image {
        let logo_lines: Vec<&str> = MAE_LOGO.lines().collect();
        if !logo_lines.is_empty() {
            let mut sec = SplashSection::new();
            for line in &logo_lines {
                sec.push(line.to_string(), logo_fg, false);
            }
            sections.push(sec);
        }
    }

    // Section 3a: Tagline (centered independently).
    {
        let mut sec = SplashSection::new();
        sec.push(
            "Modern AI Editor -- ai-native lisp machine".to_string(),
            subtitle_fg,
            false,
        );
        sections.push(sec);
    }

    // Section 3b: Version (centered independently under tagline).
    {
        let mut sec = SplashSection::new();
        sec.push(
            concat!("v", env!("CARGO_PKG_VERSION")).to_string(),
            subtitle_fg,
            false,
        );
        sec.push(String::new(), subtitle_fg, false);
        sections.push(sec);
    }

    // Section 4: Quick actions.
    {
        let mut sec = SplashSection::new();
        for (i, &(key, desc, _cmd)) in QUICK_ACTIONS.iter().enumerate() {
            let is_selected = i == editor.splash_selection;
            let prefix = if is_selected { "▸ " } else { "  " };
            sec.push(
                format!("{}{:<10}{}", prefix, key, desc),
                key_fg,
                is_selected,
            );
        }
        sec.push(String::new(), subtitle_fg, false);
        sections.push(sec);
    }

    // Section 5: Dismiss hint.
    {
        let mut sec = SplashSection::new();
        sec.push(
            "j/k navigate · Enter select".to_string(),
            subtitle_fg,
            false,
        );
        sections.push(sec);
    }

    // --- Layout: compute vertical centering ---
    let total_height: usize = sections.iter().map(|s| s.height()).sum();
    let top_pad = area_height.saturating_sub(total_height) / 2;

    // --- Render sections ---
    let mut row = area_row + top_pad;
    for section in &sections {
        if section.is_empty() {
            continue;
        }
        let section_width = section.max_width();
        let section_left = area_width.saturating_sub(section_width) / 2;

        for line in &section.lines {
            if row >= area_row + area_height {
                break;
            }
            if line.is_selected {
                // Highlight spans full section width for consistent bar.
                if let Some(bg) = sel_bg {
                    canvas.draw_rect_fill(row, section_left, section_width, 1, bg);
                }
                canvas.draw_text_bold(row, section_left, &line.text, line.fg);
            } else {
                canvas.draw_text_at(row, section_left, &line.text, line.fg);
            }
            row += 1;
        }
    }

    // --- Draw image (pixel-positioned over the placeholder lines) ---
    if let Some(img) = image_path {
        let img_x = (area_px_w - img_actual_w) / 2.0;
        let img_y = (area_row + top_pad) as f32 * cell_h
            + (image_lines as f32 * cell_h - img_actual_h) / 2.0;
        canvas.draw_image_from_cache(img, img_x, img_y, img_actual_w, img_actual_h);
    }
}

// Tests for shared splash logic live in mae_core::render_common::splash::tests.
