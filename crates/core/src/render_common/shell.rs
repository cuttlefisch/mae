//! Shared shell color resolution logic.
//!
//! Both GUI and TUI shell renderers resolve NamedColor → theme palette
//! using the same candidate list. This module extracts that mapping.

/// ANSI named color index for theme palette resolution.
///
/// This is a backend-agnostic representation of the 16+4 ANSI named colors.
/// Both GUI and TUI shell renderers map their backend-specific `NamedColor`
/// to this enum, then use `palette_candidates()` for theme resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnsiName {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    Foreground,
    DimForeground,
    Background,
}

/// Return theme palette key candidates for a named ANSI color.
///
/// Each named color maps to a prioritized list of palette keys.
/// The caller tries each key against the theme palette in order.
pub fn palette_candidates(name: AnsiName) -> &'static [&'static str] {
    match name {
        AnsiName::Black => &["black", "bg0", "base", "crust"],
        AnsiName::Red => &["red", "maroon"],
        AnsiName::Green => &["green"],
        AnsiName::Yellow => &["yellow", "peach", "orange"],
        AnsiName::Blue => &["blue", "sapphire"],
        AnsiName::Magenta => &["magenta", "purple", "pink", "mauve"],
        AnsiName::Cyan => &["cyan", "aqua", "teal", "sky"],
        AnsiName::White => &["white", "fg0", "fg1", "text", "fg"],
        AnsiName::BrightBlack => &["bright_black", "bg3", "overlay0", "comment"],
        AnsiName::BrightRed => &["bright_red", "red"],
        AnsiName::BrightGreen => &["bright_green", "green"],
        AnsiName::BrightYellow => &["bright_yellow", "yellow"],
        AnsiName::BrightBlue => &["bright_blue", "blue", "lavender"],
        AnsiName::BrightMagenta => &["bright_magenta", "bright_purple", "purple", "pink", "mauve"],
        AnsiName::BrightCyan => &["bright_cyan", "bright_aqua", "aqua", "teal", "sky"],
        AnsiName::BrightWhite => &["bright_white", "fg0", "text", "fg"],
        AnsiName::Foreground => &["fg", "fg1", "fg0", "text", "foreground"],
        AnsiName::DimForeground => &["fg", "fg2", "fg3", "subtext0"],
        AnsiName::Background => &["bg", "bg0", "base", "base03", "background"],
    }
}

/// Whether the named color should fall back to `ui.background` style
/// if no palette key matched.
pub fn should_fallback_to_ui_background(name: AnsiName) -> bool {
    matches!(name, AnsiName::Background | AnsiName::Black)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_color_basic() {
        use AnsiName::*;
        for name in [Black, Red, Green, Yellow, Blue, Magenta, Cyan, White] {
            assert!(
                !palette_candidates(name).is_empty(),
                "basic color {:?} should have candidates",
                name
            );
        }
    }

    #[test]
    fn ansi_color_bright() {
        use AnsiName::*;
        for name in [
            BrightBlack,
            BrightRed,
            BrightGreen,
            BrightYellow,
            BrightBlue,
            BrightMagenta,
            BrightCyan,
            BrightWhite,
        ] {
            assert!(
                !palette_candidates(name).is_empty(),
                "bright color {:?} should have candidates",
                name
            );
        }
    }

    #[test]
    fn background_fallback() {
        assert!(should_fallback_to_ui_background(AnsiName::Background));
        assert!(should_fallback_to_ui_background(AnsiName::Black));
        assert!(!should_fallback_to_ui_background(AnsiName::Red));
    }
}
