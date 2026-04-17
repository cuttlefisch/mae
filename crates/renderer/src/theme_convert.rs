//! Theme → ratatui conversion helpers.
//!
//! These are the lowest-level building blocks used by every other
//! renderer submodule. `ts()` is the main entry point: look up a
//! theme key and convert to a ratatui `Style`.

use mae_core::{Editor, NamedColor, ThemeColor, ThemeStyle};
use ratatui::prelude::*;

pub(crate) fn to_ratatui_color(tc: ThemeColor) -> Color {
    match tc {
        ThemeColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        ThemeColor::Named(n) => match n {
            NamedColor::Black => Color::Black,
            NamedColor::Red => Color::Red,
            NamedColor::Green => Color::Green,
            NamedColor::Yellow => Color::Yellow,
            NamedColor::Blue => Color::Blue,
            NamedColor::Magenta => Color::Magenta,
            NamedColor::Cyan => Color::Cyan,
            NamedColor::White => Color::White,
            NamedColor::DarkGray => Color::DarkGray,
            NamedColor::LightRed => Color::LightRed,
            NamedColor::LightGreen => Color::LightGreen,
            NamedColor::LightYellow => Color::LightYellow,
            NamedColor::LightBlue => Color::LightBlue,
            NamedColor::LightMagenta => Color::LightMagenta,
            NamedColor::LightCyan => Color::LightCyan,
            NamedColor::Gray => Color::Gray,
        },
    }
}

pub(crate) fn to_ratatui_style(ts: &ThemeStyle) -> Style {
    let mut style = Style::default();
    if let Some(fg) = ts.fg {
        style = style.fg(to_ratatui_color(fg));
    }
    if let Some(bg) = ts.bg {
        style = style.bg(to_ratatui_color(bg));
    }
    if ts.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if ts.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if ts.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    if ts.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    style
}

/// Shorthand: look up a theme key and convert to ratatui Style.
pub(crate) fn ts(editor: &Editor, key: &str) -> Style {
    to_ratatui_style(&editor.theme.style(key))
}
