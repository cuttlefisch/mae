//! Skia surface management and frame composition.
//!
//! Manages the Skia raster surface that backs the editor window.
//! Each frame: clear -> draw styled cells -> present via softbuffer.

use std::io;
use std::num::NonZeroU32;
use std::rc::Rc;

use mae_core::Theme;
use skia_safe::{surfaces, Color4f, Font, FontMgr, FontStyle, Paint, Surface};
use winit::window::Window;

use crate::text::StyledLine;
use crate::theme::{self, fill_paint, DEFAULT_BG};

/// Skia rendering surface, font state, and softbuffer presentation.
pub struct SkiaCanvas {
    surface: Surface,
    font: Font,
    bold_font: Font,
    icon_font: Option<Font>,
    cell_width: f32,
    cell_height: f32,
    /// Distance from cell top to the text baseline (= magnitude of ascent).
    /// Skia's `draw_str` interprets the y coordinate as the baseline, so
    /// characters with descenders (p, g, j, q, y) extend below this point.
    ascent: f32,
    width: u32,
    height: u32,
    /// softbuffer surface for blitting raster pixels to the OS window.
    sb_surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
    /// Precomputed glyph coverage for ASCII chars 0..128 in the primary font.
    /// `true` means the char has a glyph and can be batched into text runs.
    ascii_in_font: [bool; 128],
    /// Reusable pixel buffer for `end_frame()` — avoids ~4MB alloc per frame.
    pixel_buf: Vec<u8>,
}

impl SkiaCanvas {
    /// Create a new Skia raster surface with monospace font metrics.
    ///
    /// `font_family` overrides the default font search order. Pass `None` to
    /// use the built-in fallback chain (Nerd Font → JetBrains Mono → ...).
    pub fn new(
        width: u32,
        height: u32,
        window: Rc<Window>,
        font_family: Option<&str>,
        icon_font_family: Option<&str>,
        font_size_override: Option<f32>,
    ) -> io::Result<Self> {
        let surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or_else(|| io::Error::other("failed to create Skia surface"))?;

        // Load a monospace font. If a family is configured, try it first.
        // The default chain prefers Nerd Font variants (icon/glyph support).
        let font_mgr = FontMgr::default();

        let typeface = font_family
            .and_then(|fam| font_mgr.match_family_style(fam, FontStyle::normal()))
            .or_else(|| {
                font_mgr.match_family_style("JetBrainsMono Nerd Font Mono", FontStyle::normal())
            })
            .or_else(|| font_mgr.match_family_style("JetBrainsMono Nerd Font", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("JetBrains Mono", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Fira Code", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Cascadia Code", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("monospace", FontStyle::normal()))
            .ok_or_else(|| {
                io::Error::other(
                    "no monospace font found on the system — install JetBrains Mono, \
                     Fira Code, or Cascadia Code",
                )
            })?;

        let bold_typeface = font_family
            .and_then(|fam| font_mgr.match_family_style(fam, FontStyle::bold()))
            .or_else(|| {
                font_mgr.match_family_style("JetBrainsMono Nerd Font Mono", FontStyle::bold())
            })
            .or_else(|| font_mgr.match_family_style("JetBrainsMono Nerd Font", FontStyle::bold()))
            .or_else(|| font_mgr.match_family_style("JetBrains Mono", FontStyle::bold()))
            .or_else(|| font_mgr.match_family_style("Fira Code", FontStyle::bold()))
            .or_else(|| font_mgr.match_family_style("Cascadia Code", FontStyle::bold()))
            .or_else(|| font_mgr.match_family_style("monospace", FontStyle::bold()))
            .unwrap_or_else(|| typeface.clone());

        let icon_typeface = icon_font_family
            .and_then(|fam| font_mgr.match_family_style(fam, FontStyle::normal()))
            .or_else(|| {
                font_mgr.match_family_style("JetBrainsMono Nerd Font Mono", FontStyle::normal())
            })
            .or_else(|| font_mgr.match_family_style("JetBrainsMono Nerd Font", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Symbols Nerd Font Mono", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Symbols Nerd Font", FontStyle::normal()));

        let font_size = font_size_override.unwrap_or(14.0);
        let font = Font::from_typeface(typeface, font_size);
        let bold_font = Font::from_typeface(bold_typeface, font_size);
        let icon_font = icon_typeface.map(|tf| Font::from_typeface(tf, font_size));

        // Measure a reference character for cell dimensions.
        let (_, bounds) = font.measure_str("M", None);
        let cell_width = bounds.width().max(font_size * 0.6);
        let cell_height = font.spacing();
        // Font metrics: ascent is negative in Skia (distance above baseline).
        let (_, metrics) = font.metrics();
        let ascent = (-metrics.ascent).max(font_size * 0.8);

        let context = softbuffer::Context::new(window.clone())
            .map_err(|e| io::Error::other(e.to_string()))?;
        let mut sb_surface = softbuffer::Surface::new(&context, window)
            .map_err(|e| io::Error::other(e.to_string()))?;

        sb_surface
            .resize(
                NonZeroU32::new(width).unwrap_or(NonZeroU32::new(1).unwrap()),
                NonZeroU32::new(height).unwrap_or(NonZeroU32::new(1).unwrap()),
            )
            .map_err(|e| io::Error::other(e.to_string()))?;

        let ascii_in_font = build_ascii_table(&font);

        Ok(SkiaCanvas {
            surface,
            font,
            bold_font,
            icon_font,
            cell_width,
            cell_height,
            ascent,
            width,
            height,
            sb_surface,
            ascii_in_font,
            pixel_buf: Vec::new(),
        })
    }

    /// Recreate font objects and recalculate cell metrics for a new font size.
    /// Called when font size is changed at runtime via `:set font_size` or Scheme.
    pub fn update_font_size(&mut self, size: f32) {
        let typeface = self.font.typeface();
        let bold_typeface = self.bold_font.typeface();
        let icon_typeface = self.icon_font.as_ref().map(|f| f.typeface());

        self.font = Font::from_typeface(typeface, size);
        self.bold_font = Font::from_typeface(bold_typeface, size);
        self.icon_font = icon_typeface.map(|tf| Font::from_typeface(tf, size));

        let (_, bounds) = self.font.measure_str("M", None);
        self.cell_width = bounds.width().max(size * 0.6);
        self.cell_height = self.font.spacing();
        let (_, metrics) = self.font.metrics();
        self.ascent = (-metrics.ascent).max(size * 0.8);
        self.ascii_in_font = build_ascii_table(&self.font);
    }

    /// Return (cell_width, cell_height) in pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }

    /// Return the surface dimensions in pixels.
    #[allow(dead_code)]
    pub fn pixel_size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Resize the surface.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        if let Some(new_surface) = surfaces::raster_n32_premul((width as i32, height as i32)) {
            self.surface = new_surface;
        }
        let w = NonZeroU32::new(width).unwrap_or(NonZeroU32::new(1).unwrap());
        let h = NonZeroU32::new(height).unwrap_or(NonZeroU32::new(1).unwrap());
        let _ = self.sb_surface.resize(w, h);
    }

    /// Access the underlying Skia canvas for direct drawing.
    pub fn canvas(&mut self) -> &skia_safe::Canvas {
        self.surface.canvas()
    }

    /// Begin a new frame: clear the surface with the theme's background color.
    pub fn begin_frame(&mut self, theme: &Theme) {
        let bg_style = theme.style("ui.background");
        let bg = theme::color_or(bg_style.bg, DEFAULT_BG);
        self.surface.canvas().clear(bg);
    }

    // -----------------------------------------------------------------------
    // Cell-level drawing methods
    // -----------------------------------------------------------------------

    /// Draw a single character at (row, col) with optional bg rect.
    #[allow(dead_code)]
    pub fn draw_cell(
        &mut self,
        row: usize,
        col: usize,
        ch: char,
        fg: Color4f,
        bg: Option<Color4f>,
    ) {
        let x = col as f32 * self.cell_width;
        let y = row as f32 * self.cell_height;
        let canvas = self.surface.canvas();

        // Background rect if specified.
        if let Some(bg_color) = bg {
            let bg_paint = fill_paint(bg_color);
            canvas.draw_rect(
                skia_safe::Rect::from_xywh(x, y, self.cell_width, self.cell_height),
                &bg_paint,
            );
        }

        if ch != ' ' {
            let mut fg_paint = Paint::new(fg, None);
            fg_paint.set_anti_alias(true);
            self.draw_char(row, col, ch, fg, false, false, 1.0);
        }
    }

    /// Draw a line of individually-styled cells at the given row.
    #[allow(dead_code)]
    pub fn draw_styled_line(&mut self, row: usize, cells: &StyledLine) {
        for (col, cell) in cells.iter().enumerate() {
            // Background rect if specified.
            if let Some(bg_color) = cell.bg {
                self.draw_rect_fill(row, col, 1, 1, bg_color);
            }

            if cell.ch == ' ' && !cell.underline {
                continue;
            }

            self.draw_char(row, col, cell.ch, cell.fg, cell.bold, cell.italic, 1.0);

            if cell.underline {
                let x = col as f32 * self.cell_width;
                let y = row as f32 * self.cell_height;
                let baseline = y + self.ascent;
                let underline_y = baseline + 1.0;
                let mut paint = Paint::new(cell.fg, None);
                paint.set_style(skia_safe::PaintStyle::Stroke);
                paint.set_stroke_width(1.0);
                self.surface.canvas().draw_line(
                    (x, underline_y),
                    (x + self.cell_width, underline_y),
                    &paint,
                );
            }
        }
    }

    /// Fill a rectangular cell region with a solid color.
    pub fn draw_rect_fill(&mut self, row: usize, col: usize, w: usize, h: usize, color: Color4f) {
        let x = col as f32 * self.cell_width;
        let y = row as f32 * self.cell_height;
        let pw = w as f32 * self.cell_width;
        let ph = h as f32 * self.cell_height;
        let paint = fill_paint(color);
        self.surface
            .canvas()
            .draw_rect(skia_safe::Rect::from_xywh(x, y, pw, ph), &paint);
    }

    /// Fill a pixel-precise rectangle.
    pub fn draw_pixel_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color4f) {
        let paint = fill_paint(color);
        self.surface
            .canvas()
            .draw_rect(skia_safe::Rect::from_xywh(x, y, w, h), &paint);
    }

    /// Draw a single character at a specific (row, col) with optional bold/italic/icon/scale fallback.
    pub fn draw_char(
        &mut self,
        _row: usize,
        col: usize,
        ch: char,
        fg: Color4f,
        bold: bool,
        italic: bool,
        scale: f32,
    ) {
        let x = col as f32 * self.cell_width;
        let y = _row as f32 * self.cell_height;
        let mut paint = Paint::new(fg, None);
        paint.set_anti_alias(true);

        let text = ch.to_string();

        let mut font = if bold {
            self.bold_font.clone()
        } else {
            self.font.clone()
        };

        if scale != 1.0 {
            font.set_size(font.size() * scale);
        }

        let (_, metrics) = font.metrics();
        let baseline = y - metrics.ascent;

        if italic {
            self.surface.canvas().save();
            // Simulate italic with a slight skew.
            let mut skew_matrix = skia_safe::Matrix::new_identity();
            skew_matrix.pre_skew((-0.2, 0.0), None);
            // We need to translate so the skew happens relative to the character position.
            self.surface.canvas().translate((x, baseline));
            self.surface.canvas().concat(&skew_matrix);
            // Now draw at (0,0) because of the translation.
            self.surface.canvas().draw_str(&text, (0, 0), &font, &paint);
            self.surface.canvas().restore();
            return;
        }

        // 1. Try primary font
        if font.unichar_to_glyph(ch as i32) != 0 {
            self.surface
                .canvas()
                .draw_str(&text, (x, baseline), &font, &paint);
            return;
        }

        // 2. Try icon font fallback
        if let Some(mut icon_font) = self.icon_font.clone() {
            if scale != 1.0 {
                icon_font.set_size(icon_font.size() * scale);
            }
            if icon_font.unichar_to_glyph(ch as i32) != 0 {
                self.surface
                    .canvas()
                    .draw_str(&text, (x, baseline), &icon_font, &paint);
                return;
            }
        }

        // 3. System fallback via FontMgr
        let font_mgr = FontMgr::default();
        let family_name = self.font.typeface().family_name();
        let style = self.font.typeface().font_style();

        if let Some(fallback_tf) = font_mgr.match_family_style_character(
            family_name.as_str(),
            style,
            &[], // bcp47
            ch as i32,
        ) {
            let fallback_font = Font::from_typeface(fallback_tf, self.font.size() * scale);
            let (_, fb_metrics) = fallback_font.metrics();
            let fb_baseline = y - fb_metrics.ascent;
            self.surface
                .canvas()
                .draw_str(&text, (x, fb_baseline), &fallback_font, &paint);
        } else {
            // Last resort
            self.surface
                .canvas()
                .draw_str(&text, (x, baseline), &font, &paint);
        }
    }

    /// Draw text at a specific (row, col) cell position with given fg color.
    /// ASCII-only strings use a single `draw_text_run`; mixed/CJK falls back per-char.
    pub fn draw_text_at(&mut self, row: usize, col: usize, text: &str, fg: Color4f) {
        if text.is_ascii() {
            self.draw_text_run(row, col, text, fg, false, false, 1.0);
            return;
        }
        use unicode_width::UnicodeWidthChar;
        let mut c = col;
        for ch in text.chars() {
            self.draw_char(row, c, ch, fg, false, false, 1.0);
            c += ch.width().unwrap_or(1);
        }
    }

    /// Draw text at a specific (row, col) with bold font.
    /// ASCII-only strings use a single `draw_text_run`; mixed/CJK falls back per-char.
    pub fn draw_text_bold(&mut self, row: usize, col: usize, text: &str, fg: Color4f) {
        if text.is_ascii() {
            self.draw_text_run(row, col, text, fg, true, false, 1.0);
            return;
        }
        use unicode_width::UnicodeWidthChar;
        let mut c = col;
        for ch in text.chars() {
            self.draw_char(row, c, ch, fg, true, false, 1.0);
            c += ch.width().unwrap_or(1);
        }
    }

    /// Draw a horizontal line across a full row (cell-based).
    #[allow(dead_code)]
    pub fn draw_hline(&mut self, row: usize, col_start: usize, col_end: usize, color: Color4f) {
        let y = row as f32 * self.cell_height + self.cell_height / 2.0;
        let x1 = col_start as f32 * self.cell_width;
        let x2 = col_end as f32 * self.cell_width;
        let mut paint = Paint::new(color, None);
        paint.set_stroke_width(1.0);
        paint.set_style(skia_safe::PaintStyle::Stroke);
        self.surface.canvas().draw_line((x1, y), (x2, y), &paint);
    }

    /// Draw a vertical line across rows (cell-based).
    pub fn draw_vline(&mut self, col: usize, row_start: usize, row_end: usize, color: Color4f) {
        let x = col as f32 * self.cell_width;
        let y1 = row_start as f32 * self.cell_height;
        let y2 = row_end as f32 * self.cell_height;
        let mut paint = Paint::new(color, None);
        paint.set_stroke_width(1.0);
        paint.set_style(skia_safe::PaintStyle::Stroke);
        self.surface.canvas().draw_line((x, y1), (x, y2), &paint);
    }

    // -----------------------------------------------------------------------
    // Legacy draw methods (kept for compatibility during transition)
    // -----------------------------------------------------------------------

    /// Draw a single line of text at the given visual row.
    #[allow(dead_code)]
    pub fn draw_text_line(&mut self, row: usize, text: &str, theme: &Theme) {
        let fg_style = theme.style("ui.text");
        let fg = theme::color_or(fg_style.fg, theme::DEFAULT_FG);
        self.draw_text_at(row, 0, text, fg);
    }

    /// Draw the status line at the given visual row.
    #[allow(dead_code)]
    pub fn draw_status_line(&mut self, row: usize, text: &str, theme: &Theme) {
        let status_style = theme.style("ui.statusline");
        let status_bg = theme::color_or(status_style.bg, theme::STATUS_BG);
        let status_fg = theme::color_or(status_style.fg, theme::DEFAULT_FG);

        let cols = (self.width as f32 / self.cell_width) as usize;
        self.draw_rect_fill(row, 0, cols, 1, status_bg);
        self.draw_text_at(row, 0, text, status_fg);
    }

    /// Return the precomputed ASCII glyph coverage table.
    pub fn ascii_in_font(&self) -> &[bool; 128] {
        &self.ascii_in_font
    }

    /// Draw an entire text run in a single Skia `draw_str` call.
    ///
    /// The caller guarantees all chars in `text` are covered by the primary
    /// (or bold) font — no per-glyph fallback. This eliminates per-char
    /// `String` alloc, `Font::clone`, `font.metrics()`, and `unichar_to_glyph`.
    pub fn draw_text_run(
        &mut self,
        row: usize,
        col: usize,
        text: &str,
        fg: Color4f,
        bold: bool,
        italic: bool,
        scale: f32,
    ) {
        if text.is_empty() {
            return;
        }
        let x = col as f32 * self.cell_width;
        let y = row as f32 * self.cell_height;
        let font = if bold { &self.bold_font } else { &self.font };
        let mut paint = Paint::new(fg, None);
        paint.set_anti_alias(true);

        if scale != 1.0 {
            let mut scaled = font.clone();
            scaled.set_size(font.size() * scale);
            let (_, m) = scaled.metrics();
            let baseline = y - m.ascent;
            if italic {
                self.surface.canvas().save();
                let mut skew = skia_safe::Matrix::new_identity();
                skew.pre_skew((-0.2, 0.0), None);
                self.surface.canvas().translate((x, baseline));
                self.surface.canvas().concat(&skew);
                self.surface
                    .canvas()
                    .draw_str(text, (0, 0), &scaled, &paint);
                self.surface.canvas().restore();
            } else {
                self.surface
                    .canvas()
                    .draw_str(text, (x, baseline), &scaled, &paint);
            }
            return;
        }

        let baseline = y + self.ascent;
        if italic {
            self.surface.canvas().save();
            let mut skew = skia_safe::Matrix::new_identity();
            skew.pre_skew((-0.2, 0.0), None);
            self.surface.canvas().translate((x, baseline));
            self.surface.canvas().concat(&skew);
            self.surface.canvas().draw_str(text, (0, 0), font, &paint);
            self.surface.canvas().restore();
        } else {
            self.surface
                .canvas()
                .draw_str(text, (x, baseline), font, &paint);
        }
    }

    /// Draw a horizontal underline spanning `count` cells.
    pub fn draw_underline_span(&mut self, row: usize, col: usize, count: usize, color: Color4f) {
        let x = col as f32 * self.cell_width;
        let y = row as f32 * self.cell_height;
        let underline_y = y + self.ascent + 1.0;
        let width = count as f32 * self.cell_width;
        let mut paint = Paint::new(color, None);
        paint.set_style(skia_safe::PaintStyle::Stroke);
        paint.set_stroke_width(1.0);
        self.surface
            .canvas()
            .draw_line((x, underline_y), (x + width, underline_y), &paint);
    }

    /// Draw a horizontal line exactly under a cell (for underlining).
    #[allow(dead_code)]
    pub fn draw_hline_exact(&mut self, row: usize, col: usize, color: Color4f) {
        let x = col as f32 * self.cell_width;
        let y = row as f32 * self.cell_height;
        let baseline = y + self.ascent;
        let underline_y = baseline + 1.0;
        let mut paint = Paint::new(color, None);
        paint.set_style(skia_safe::PaintStyle::Stroke);
        paint.set_stroke_width(1.0);
        self.surface.canvas().draw_line(
            (x, underline_y),
            (x + self.cell_width, underline_y),
            &paint,
        );
    }

    /// End the frame: blit the Skia raster pixels to the OS window via softbuffer.
    pub fn end_frame(&mut self) {
        // Read Skia pixel data (premultiplied BGRA on little-endian).
        let image_info = self.surface.image_info();
        let row_bytes = image_info.min_row_bytes();
        let total_bytes = row_bytes * self.height as usize;
        // Reuse pixel buffer across frames — avoids ~4MB allocation per frame.
        self.pixel_buf.resize(total_bytes, 0);
        self.surface
            .read_pixels(&image_info, &mut self.pixel_buf, row_bytes, (0, 0));

        // softbuffer wants u32 pixels in 0x00RRGGBB format.
        let Ok(mut buffer) = self.sb_surface.buffer_mut() else {
            return;
        };

        let pixel_count = (self.width * self.height) as usize;
        // Process as u32 slice: on little-endian, BGRA bytes → u32 = 0xAARRGGBB.
        // Masking off alpha gives 0x00RRGGBB — exactly what softbuffer wants.
        let u32_count = self.pixel_buf.len() / 4;
        let pixels_u32 =
            unsafe { std::slice::from_raw_parts(self.pixel_buf.as_ptr() as *const u32, u32_count) };
        let count = pixel_count.min(buffer.len()).min(u32_count);
        for i in 0..count {
            buffer[i] = pixels_u32[i] & 0x00FF_FFFF;
        }

        let _ = buffer.present();
    }
}

/// Cell-based rectangle for layout computation.
#[derive(Debug, Clone, Copy)]
pub struct CellRect {
    pub row: usize,
    pub col: usize,
    pub width: usize,
    pub height: usize,
}

impl CellRect {
    pub fn new(row: usize, col: usize, width: usize, height: usize) -> Self {
        Self {
            row,
            col,
            width,
            height,
        }
    }

    /// Inner rect with 1-cell border removed.
    pub fn inner(&self) -> Self {
        Self {
            row: self.row + 1,
            col: self.col + 1,
            width: self.width.saturating_sub(2),
            height: self.height.saturating_sub(2),
        }
    }
}

/// Build a lookup table of which ASCII codepoints have glyphs in `font`.
fn build_ascii_table(font: &Font) -> [bool; 128] {
    let mut table = [false; 128];
    for i in 0..128u8 {
        table[i as usize] = font.unichar_to_glyph(i as i32) != 0;
    }
    // Space always counts as "in font" for run batching purposes.
    table[b' ' as usize] = true;
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_rect_inner() {
        let r = CellRect::new(0, 0, 80, 24);
        let inner = r.inner();
        assert_eq!(inner.row, 1);
        assert_eq!(inner.col, 1);
        assert_eq!(inner.width, 78);
        assert_eq!(inner.height, 22);
    }

    #[test]
    fn cell_rect_inner_small() {
        let r = CellRect::new(5, 10, 2, 2);
        let inner = r.inner();
        assert_eq!(inner.width, 0);
        assert_eq!(inner.height, 0);
    }

    #[test]
    fn styled_cell_draw_basic() {
        // Just verify StyledCell/StyledLine types work with canvas API.
        let cell = crate::text::StyledCell::new('X', Color4f::new(1.0, 1.0, 1.0, 1.0));
        assert_eq!(cell.ch, 'X');
    }
}
