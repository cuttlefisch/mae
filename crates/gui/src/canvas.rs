//! Skia surface management and frame composition.
//!
//! Manages the Skia raster surface that backs the editor window.
//! Each frame: clear → draw text lines → draw status → present via softbuffer.

use std::io;
use std::num::NonZeroU32;
use std::rc::Rc;

use mae_core::{NamedColor, Theme, ThemeColor};
use skia_safe::{surfaces, Color4f, Font, FontMgr, FontStyle, Paint, Surface};
use winit::window::Window;

/// Skia rendering surface, font state, and softbuffer presentation.
pub struct SkiaCanvas {
    surface: Surface,
    font: Font,
    cell_width: f32,
    cell_height: f32,
    width: u32,
    height: u32,
    /// softbuffer surface for blitting raster pixels to the OS window.
    sb_surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
}

impl SkiaCanvas {
    /// Create a new Skia raster surface with monospace font metrics.
    pub fn new(width: u32, height: u32, window: Rc<Window>) -> io::Result<Self> {
        let surface = surfaces::raster_n32_premul((width as i32, height as i32))
            .ok_or_else(|| io::Error::other("failed to create Skia surface"))?;

        // Load a monospace font. Try system fonts, fall back to default.
        let font_mgr = FontMgr::default();
        let typeface = font_mgr
            .match_family_style("JetBrains Mono", FontStyle::normal())
            .or_else(|| font_mgr.match_family_style("Fira Code", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("Cascadia Code", FontStyle::normal()))
            .or_else(|| font_mgr.match_family_style("monospace", FontStyle::normal()))
            .expect("no monospace font found on the system");

        let font_size = 14.0;
        let font = Font::from_typeface(typeface, font_size);

        // Measure a reference character for cell dimensions.
        let (_, bounds) = font.measure_str("M", None);
        let cell_width = bounds.width().max(font_size * 0.6);
        let cell_height = font.spacing();

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

        Ok(SkiaCanvas {
            surface,
            font,
            cell_width,
            cell_height,
            width,
            height,
            sb_surface,
        })
    }

    /// Return (cell_width, cell_height) in pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
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

    /// Begin a new frame: clear the surface with the theme's background color.
    pub fn begin_frame(&mut self, theme: &Theme) {
        let bg_style = theme.style("ui.background");
        let bg = bg_style
            .bg
            .map(|c| theme_color_to_skia(&c))
            .unwrap_or_else(|| Color4f::new(0.1, 0.1, 0.1, 1.0));
        self.surface.canvas().clear(bg);
    }

    /// Draw a single line of text at the given visual row.
    pub fn draw_text_line(&mut self, row: usize, text: &str, theme: &Theme) {
        let fg_style = theme.style("ui.text");
        let fg = fg_style
            .fg
            .map(|c| theme_color_to_skia(&c))
            .unwrap_or_else(|| Color4f::new(0.9, 0.9, 0.9, 1.0));
        let mut paint = Paint::new(fg, None);
        paint.set_anti_alias(true);

        let x = 0.0;
        let y = (row as f32 + 1.0) * self.cell_height; // baseline

        let canvas = self.surface.canvas();
        canvas.draw_str(text, (x, y), &self.font, &paint);
    }

    /// Draw the status line at the given visual row.
    pub fn draw_status_line(&mut self, row: usize, text: &str, theme: &Theme) {
        let status_style = theme.style("ui.statusline");
        let status_bg = status_style
            .bg
            .map(|c| theme_color_to_skia(&c))
            .unwrap_or_else(|| Color4f::new(0.2, 0.2, 0.2, 1.0));
        let status_fg = status_style
            .fg
            .map(|c| theme_color_to_skia(&c))
            .unwrap_or_else(|| Color4f::new(0.9, 0.9, 0.9, 1.0));

        let y = row as f32 * self.cell_height;
        let canvas = self.surface.canvas();

        // Background rectangle.
        let mut bg_paint = Paint::new(status_bg, None);
        bg_paint.set_style(skia_safe::PaintStyle::Fill);
        canvas.draw_rect(
            skia_safe::Rect::from_xywh(0.0, y, self.width as f32, self.cell_height),
            &bg_paint,
        );

        // Status text.
        let mut fg_paint = Paint::new(status_fg, None);
        fg_paint.set_anti_alias(true);
        canvas.draw_str(text, (0.0, y + self.cell_height), &self.font, &fg_paint);
    }

    /// End the frame: blit the Skia raster pixels to the OS window via softbuffer.
    pub fn end_frame(&mut self) {
        // Read Skia pixel data (premultiplied BGRA on little-endian).
        let image_info = self.surface.image_info();
        let row_bytes = image_info.min_row_bytes();
        let total_bytes = row_bytes * self.height as usize;
        let mut pixels = vec![0u8; total_bytes];
        self.surface
            .read_pixels(&image_info, &mut pixels, row_bytes, (0, 0));

        // softbuffer wants u32 pixels in 0x00RRGGBB format.
        let Ok(mut buffer) = self.sb_surface.buffer_mut() else {
            return;
        };

        let pixel_count = (self.width * self.height) as usize;
        for i in 0..pixel_count.min(buffer.len()) {
            let offset = i * 4;
            if offset + 3 >= pixels.len() {
                break;
            }
            // Skia raster_n32_premul on little-endian is BGRA byte order.
            let b = pixels[offset] as u32;
            let g = pixels[offset + 1] as u32;
            let r = pixels[offset + 2] as u32;
            // softbuffer format: 0x00RRGGBB
            buffer[i] = (r << 16) | (g << 8) | b;
        }

        let _ = buffer.present();
    }
}

/// Convert a mae_core ThemeColor to a Skia Color4f.
fn theme_color_to_skia(color: &ThemeColor) -> Color4f {
    match color {
        ThemeColor::Rgb(r, g, b) => {
            Color4f::new(*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0, 1.0)
        }
        ThemeColor::Named(named) => {
            let (r, g, b) = named_color_to_rgb(named);
            Color4f::new(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0)
        }
    }
}

/// Map ANSI named colors to approximate RGB values (xterm-256 standard).
fn named_color_to_rgb(c: &NamedColor) -> (u8, u8, u8) {
    match c {
        NamedColor::Black => (0, 0, 0),
        NamedColor::Red => (205, 0, 0),
        NamedColor::Green => (0, 205, 0),
        NamedColor::Yellow => (205, 205, 0),
        NamedColor::Blue => (0, 0, 238),
        NamedColor::Magenta => (205, 0, 205),
        NamedColor::Cyan => (0, 205, 205),
        NamedColor::White => (229, 229, 229),
        NamedColor::DarkGray => (127, 127, 127),
        NamedColor::LightRed => (255, 0, 0),
        NamedColor::LightGreen => (0, 255, 0),
        NamedColor::LightYellow => (255, 255, 0),
        NamedColor::LightBlue => (92, 92, 255),
        NamedColor::LightMagenta => (255, 0, 255),
        NamedColor::LightCyan => (0, 255, 255),
        NamedColor::Gray => (192, 192, 192),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_color_rgb_conversion() {
        let color = ThemeColor::Rgb(255, 128, 0);
        let skia = theme_color_to_skia(&color);
        assert!((skia.r - 1.0).abs() < 0.01);
        assert!((skia.g - 0.502).abs() < 0.01);
        assert!((skia.b - 0.0).abs() < 0.01);
    }

    #[test]
    fn named_color_black() {
        let color = ThemeColor::Named(NamedColor::Black);
        let skia = theme_color_to_skia(&color);
        assert!((skia.r - 0.0).abs() < 0.01);
        assert!((skia.g - 0.0).abs() < 0.01);
        assert!((skia.b - 0.0).abs() < 0.01);
    }

    #[test]
    fn named_color_white() {
        let color = ThemeColor::Named(NamedColor::White);
        let skia = theme_color_to_skia(&color);
        assert!(skia.r > 0.8);
        assert!(skia.g > 0.8);
        assert!(skia.b > 0.8);
    }
}
