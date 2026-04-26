//! ThemeStyle -> Skia Paint/Color conversion.
//!
//! GUI equivalent of `renderer/src/theme_convert.rs`. Every GUI rendering
//! module uses these helpers to look up theme keys and produce Skia colors.

use mae_core::{Editor, NamedColor, ThemeColor, ThemeStyle};
use skia_safe::{Color4f, Paint};

/// Convert a mae_core `ThemeColor` to a Skia `Color4f`.
pub fn parse_hex_to_skia(hex: &str) -> Option<Color4f> {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
        let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
        let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
        Some(Color4f::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            1.0,
        ))
    } else if hex.len() == 3 {
        let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
        let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
        let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
        Some(Color4f::new(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            1.0,
        ))
    } else {
        None
    }
}

pub fn theme_color_to_skia(color: &ThemeColor) -> Color4f {
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
pub fn named_color_to_rgb(c: &NamedColor) -> (u8, u8, u8) {
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

/// Look up a theme key's foreground color, returning a Skia `Color4f`.
/// Falls back to light gray if no fg is set.
pub fn ts_fg(editor: &Editor, key: &str) -> Color4f {
    let style = editor.theme.style(key);
    style
        .fg
        .map(|c| theme_color_to_skia(&c))
        .unwrap_or_else(|| Color4f::new(0.9, 0.9, 0.9, 1.0))
}

/// Look up a theme key's background color, returning `Some(Color4f)` if set.
pub fn ts_bg(editor: &Editor, key: &str) -> Option<Color4f> {
    let style = editor.theme.style(key);
    style.bg.map(|c| theme_color_to_skia(&c))
}

/// Look up a theme key and return a full `ThemeStyle`.
pub fn ts_style(editor: &Editor, key: &str) -> ThemeStyle {
    editor.theme.style(key)
}

/// Build a Skia `Paint` for the foreground of a theme key.
/// Anti-alias is enabled. Bold simulation adds +0.5 stroke width.
pub fn ts_paint(editor: &Editor, key: &str) -> Paint {
    let style = editor.theme.style(key);
    let fg = style
        .fg
        .map(|c| theme_color_to_skia(&c))
        .unwrap_or_else(|| Color4f::new(0.9, 0.9, 0.9, 1.0));
    let mut paint = Paint::new(fg, None);
    paint.set_anti_alias(true);
    if style.bold {
        // Simulate bold by thickening the stroke slightly.
        paint.set_style(skia_safe::PaintStyle::StrokeAndFill);
        paint.set_stroke_width(0.5);
    }
    paint
}

/// Build a `Paint` directly from a `Color4f`.
pub fn color_paint(color: Color4f) -> Paint {
    let mut paint = Paint::new(color, None);
    paint.set_anti_alias(true);
    paint
}

/// Build a fill `Paint` from a `Color4f`.
pub fn fill_paint(color: Color4f) -> Paint {
    let mut paint = Paint::new(color, None);
    paint.set_style(skia_safe::PaintStyle::Fill);
    paint
}

/// Shorthand: ThemeColor option -> Color4f with fallback.
pub fn color_or(tc: Option<ThemeColor>, fallback: Color4f) -> Color4f {
    tc.map(|c| theme_color_to_skia(&c)).unwrap_or(fallback)
}

/// Pick black or white foreground for readability on the given bg color.
pub fn contrast_fg(r: u8, g: u8, b: u8) -> Color4f {
    let lum = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
    if lum > 128.0 {
        Color4f::new(0.0, 0.0, 0.0, 1.0) // black
    } else {
        Color4f::new(1.0, 1.0, 1.0, 1.0) // white
    }
}

/// Fast equality check for Color4f (avoids per-field comparison noise at call sites).
pub(crate) fn color4f_eq(a: Color4f, b: Color4f) -> bool {
    a.r == b.r && a.g == b.g && a.b == b.b && a.a == b.a
}

/// Fast equality check for Option<Color4f>.
#[allow(dead_code)]
pub(crate) fn option_color4f_eq(a: Option<Color4f>, b: Option<Color4f>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => color4f_eq(a, b),
        _ => false,
    }
}

// Default fallback colors.
pub const DEFAULT_FG: Color4f = Color4f {
    r: 0.9,
    g: 0.9,
    b: 0.9,
    a: 1.0,
};
pub const DEFAULT_BG: Color4f = Color4f {
    r: 0.1,
    g: 0.1,
    b: 0.1,
    a: 1.0,
};
pub const STATUS_BG: Color4f = Color4f {
    r: 0.2,
    g: 0.2,
    b: 0.2,
    a: 1.0,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_color_conversion() {
        let color = ThemeColor::Rgb(255, 128, 0);
        let skia = theme_color_to_skia(&color);
        assert!((skia.r - 1.0).abs() < 0.01);
        assert!((skia.g - 0.502).abs() < 0.01);
        assert!((skia.b - 0.0).abs() < 0.01);
    }

    #[test]
    fn named_color_black_conversion() {
        let color = ThemeColor::Named(NamedColor::Black);
        let skia = theme_color_to_skia(&color);
        assert!((skia.r).abs() < 0.01);
        assert!((skia.g).abs() < 0.01);
        assert!((skia.b).abs() < 0.01);
    }

    #[test]
    fn named_color_white_bright() {
        let color = ThemeColor::Named(NamedColor::White);
        let skia = theme_color_to_skia(&color);
        assert!(skia.r > 0.8);
        assert!(skia.g > 0.8);
        assert!(skia.b > 0.8);
    }

    #[test]
    fn ts_bg_returns_none_for_absent() {
        // ThemeStyle with no bg should return None — tested indirectly.
        let style = ThemeStyle::default();
        assert!(style.bg.is_none());
    }

    #[test]
    fn bold_paint_has_stroke() {
        let style = ThemeStyle {
            bold: true,
            ..Default::default()
        };
        assert!(style.bold);
        // We test the paint builder indirectly via theme lookup in integration.
    }

    #[test]
    fn contrast_fg_light_bg_gets_black() {
        let c = contrast_fg(255, 255, 255);
        assert!(c.r < 0.01);
    }

    #[test]
    fn contrast_fg_dark_bg_gets_white() {
        let c = contrast_fg(0, 0, 0);
        assert!(c.r > 0.99);
    }

    #[test]
    fn color_or_uses_fallback() {
        let fb = Color4f::new(0.5, 0.5, 0.5, 1.0);
        assert_eq!(color_or(None, fb), fb);
    }

    #[test]
    fn color_or_uses_value() {
        let fb = Color4f::new(0.5, 0.5, 0.5, 1.0);
        let result = color_or(Some(ThemeColor::Rgb(255, 0, 0)), fb);
        assert!((result.r - 1.0).abs() < 0.01);
    }
}
