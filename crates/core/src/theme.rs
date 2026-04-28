use std::collections::HashMap;

/// A parsed color — either RGB or a named ANSI color for low-color terminals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeColor {
    Rgb(u8, u8, u8),
    Named(NamedColor),
}

/// Named ANSI colors for terminals without true color support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    DarkGray,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    Gray,
}

/// Style combining foreground, background, and modifiers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ThemeStyle {
    pub fg: Option<ThemeColor>,
    pub bg: Option<ThemeColor>,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub underline: bool,
}

/// Internal unresolved style — stores color name strings, not resolved colors.
/// This is the source of truth; resolved colors are cached separately.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnresolvedStyle {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub underline: bool,
}

/// A complete editor theme.
///
/// Themes use a palette of named colors and a map of semantic style keys.
/// Style keys are dot-namespaced (e.g., "ui.statusline.mode.normal") and
/// fall back through the hierarchy when a specific key isn't defined.
///
/// Follows the Helix editor convention: TOML format, palette system,
/// theme inheritance via `inherits`.
///
/// Colors are stored as name strings (source of truth) and resolved against
/// the palette into a cache. The cache is rebuilt on theme load, palette
/// mutation, and style mutation — never on every `style()` call.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub palette: HashMap<String, ThemeColor>,
    unresolved: HashMap<String, UnresolvedStyle>,
    resolved_cache: HashMap<String, ThemeStyle>,
}

/// Errors that can occur during theme loading.
#[derive(Debug)]
pub enum ThemeError {
    ParseError(String),
    InheritanceError(String),
    ColorError(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::ParseError(s) => write!(f, "theme parse error: {}", s),
            ThemeError::InheritanceError(s) => write!(f, "theme inheritance error: {}", s),
            ThemeError::ColorError(s) => write!(f, "theme color error: {}", s),
        }
    }
}

/// Trait for resolving theme names to TOML content.
/// Implementations can read from filesystem or bundled strings.
pub trait ThemeResolver {
    fn resolve(&self, name: &str) -> Option<String>;
}

/// Resolver that uses only bundled (compiled-in) themes.
pub struct BundledResolver;

impl ThemeResolver for BundledResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        bundled_themes().get(name).map(|s| s.to_string())
    }
}

/// Return all bundled theme TOML strings, compiled into the binary.
pub fn bundled_themes() -> HashMap<String, &'static str> {
    let mut m = HashMap::new();
    let default_dark = include_str!("themes/default.toml");
    m.insert("default".into(), default_dark);
    // "dark-ansi" is an alias for the default ANSI-only dark theme.
    m.insert("dark-ansi".into(), default_dark);
    m.insert("light-ansi".into(), include_str!("themes/light-ansi.toml"));
    m.insert(
        "gruvbox-dark".into(),
        include_str!("themes/gruvbox-dark.toml"),
    );
    m.insert(
        "gruvbox-light".into(),
        include_str!("themes/gruvbox-light.toml"),
    );
    m.insert("dracula".into(), include_str!("themes/dracula.toml"));
    m.insert(
        "catppuccin-mocha".into(),
        include_str!("themes/catppuccin-mocha.toml"),
    );
    m.insert(
        "solarized-dark".into(),
        include_str!("themes/solarized-dark.toml"),
    );
    m.insert("one-dark".into(), include_str!("themes/one-dark.toml"));
    m
}

/// Return a sorted list of all bundled theme names.
pub fn bundled_theme_names() -> Vec<String> {
    let mut names: Vec<String> = bundled_themes().keys().cloned().collect();
    names.sort();
    names
}

/// Parse a hex color string: "#RGB", "#RRGGBB", or "#RRGGBBAA" (alpha ignored).
fn parse_hex_color(s: &str) -> Result<ThemeColor, ThemeError> {
    let s = s.trim_start_matches('#');
    match s.len() {
        3 => {
            let r = u8::from_str_radix(&s[0..1], 16)
                .map_err(|e| ThemeError::ColorError(format!("bad hex: {}", e)))?;
            let g = u8::from_str_radix(&s[1..2], 16)
                .map_err(|e| ThemeError::ColorError(format!("bad hex: {}", e)))?;
            let b = u8::from_str_radix(&s[2..3], 16)
                .map_err(|e| ThemeError::ColorError(format!("bad hex: {}", e)))?;
            Ok(ThemeColor::Rgb(r * 17, g * 17, b * 17))
        }
        6 | 8 => {
            let r = u8::from_str_radix(&s[0..2], 16)
                .map_err(|e| ThemeError::ColorError(format!("bad hex: {}", e)))?;
            let g = u8::from_str_radix(&s[2..4], 16)
                .map_err(|e| ThemeError::ColorError(format!("bad hex: {}", e)))?;
            let b = u8::from_str_radix(&s[4..6], 16)
                .map_err(|e| ThemeError::ColorError(format!("bad hex: {}", e)))?;
            Ok(ThemeColor::Rgb(r, g, b))
        }
        _ => Err(ThemeError::ColorError(format!(
            "invalid hex color length: #{}",
            s
        ))),
    }
}

/// Parse a named ANSI color string.
fn parse_named_ansi(s: &str) -> Option<NamedColor> {
    match s.to_lowercase().as_str() {
        "black" => Some(NamedColor::Black),
        "red" => Some(NamedColor::Red),
        "green" => Some(NamedColor::Green),
        "yellow" => Some(NamedColor::Yellow),
        "blue" => Some(NamedColor::Blue),
        "magenta" => Some(NamedColor::Magenta),
        "cyan" => Some(NamedColor::Cyan),
        "white" => Some(NamedColor::White),
        "gray" | "grey" => Some(NamedColor::Gray),
        "dark_gray" | "dark_grey" | "darkgray" | "darkgrey" => Some(NamedColor::DarkGray),
        "light_red" | "lightred" => Some(NamedColor::LightRed),
        "light_green" | "lightgreen" => Some(NamedColor::LightGreen),
        "light_yellow" | "lightyellow" => Some(NamedColor::LightYellow),
        "light_blue" | "lightblue" => Some(NamedColor::LightBlue),
        "light_magenta" | "lightmagenta" => Some(NamedColor::LightMagenta),
        "light_cyan" | "lightcyan" => Some(NamedColor::LightCyan),
        _ => None,
    }
}

impl Theme {
    /// Parse a theme from a TOML string.
    pub fn from_toml(name: &str, toml_str: &str) -> Result<Self, ThemeError> {
        let table: toml::Table = toml_str
            .parse()
            .map_err(|e| ThemeError::ParseError(format!("{}", e)))?;

        let mut palette = HashMap::new();
        if let Some(toml::Value::Table(pal)) = table.get("palette") {
            for (key, val) in pal {
                if let toml::Value::String(color_str) = val {
                    palette.insert(key.clone(), resolve_color_value(color_str, &palette)?);
                }
            }
        }

        let mut unresolved = HashMap::new();
        if let Some(toml::Value::Table(sty)) = table.get("styles") {
            for (key, val) in sty {
                unresolved.insert(key.clone(), parse_unresolved_style(val)?);
            }
        }

        let mut theme = Theme {
            name: name.to_string(),
            palette,
            unresolved,
            resolved_cache: HashMap::new(),
        };
        theme.rebuild_cache();
        Ok(theme)
    }

    /// Load a theme with inheritance support.
    /// If the theme TOML has `inherits = "parent"`, loads the parent first
    /// and merges child styles on top.
    pub fn load(name: &str, resolver: &dyn ThemeResolver) -> Result<Self, ThemeError> {
        let toml_str = resolver
            .resolve(name)
            .ok_or_else(|| ThemeError::ParseError(format!("theme '{}' not found", name)))?;

        let table: toml::Table = toml_str
            .parse()
            .map_err(|e| ThemeError::ParseError(format!("{}", e)))?;

        // Check for inheritance
        let parent = if let Some(toml::Value::String(parent_name)) = table.get("inherits") {
            if parent_name == name {
                return Err(ThemeError::InheritanceError(format!(
                    "theme '{}' inherits from itself",
                    name
                )));
            }
            Some(Theme::load(parent_name, resolver)?)
        } else {
            None
        };

        let mut theme = Theme::from_toml(name, &toml_str)?;

        // Merge: parent palette/unresolved are the base, child overrides.
        // Then rebuild cache against the merged palette so inherited styles
        // pick up the child's palette colors.
        if let Some(parent) = parent {
            let mut merged_palette = parent.palette;
            for (k, v) in theme.palette {
                merged_palette.insert(k, v);
            }
            let mut merged_unresolved = parent.unresolved;
            for (k, v) in theme.unresolved {
                merged_unresolved.insert(k, v);
            }
            theme.palette = merged_palette;
            theme.unresolved = merged_unresolved;
            theme.rebuild_cache();
        }

        Ok(theme)
    }

    /// Rebuild the resolved style cache from unresolved styles + palette.
    fn rebuild_cache(&mut self) {
        self.resolved_cache.clear();
        for (key, us) in &self.unresolved {
            self.resolved_cache.insert(
                key.clone(),
                ThemeStyle {
                    fg: us
                        .fg
                        .as_ref()
                        .and_then(|s| resolve_color_value_ok(s, &self.palette)),
                    bg: us
                        .bg
                        .as_ref()
                        .and_then(|s| resolve_color_value_ok(s, &self.palette)),
                    bold: us.bold,
                    italic: us.italic,
                    dim: us.dim,
                    underline: us.underline,
                },
            );
        }
    }

    /// Look up a style by semantic key.
    /// Falls back through dot-notation hierarchy:
    /// "ui.statusline.mode.normal" → "ui.statusline.mode" → "ui.statusline" → "ui.text" → default
    pub fn style(&self, key: &str) -> ThemeStyle {
        // Fast path: exact match (avoids String allocation — hot path, ~250+ calls/frame)
        if let Some(style) = self.resolved_cache.get(key) {
            return style.clone();
        }
        // Slow path: walk dot-notation hierarchy
        let mut lookup = key.to_string();
        while let Some(pos) = lookup.rfind('.') {
            lookup.truncate(pos);
            if let Some(style) = self.resolved_cache.get(&lookup) {
                return style.clone();
            }
        }
        // Final fallback: "ui.text" for ui keys, default otherwise
        if key.starts_with("ui.") {
            if let Some(style) = self.resolved_cache.get("ui.text") {
                return style.clone();
            }
        }
        ThemeStyle::default()
    }

    /// Convert the theme palette to 16 standard ANSI color RGB values.
    /// Used to configure shell terminal emulators with theme-aware colors.
    /// Returns: [Black, Red, Green, Yellow, Blue, Magenta, Cyan, White,
    ///           BrightBlack, BrightRed, BrightGreen, BrightYellow,
    ///           BrightBlue, BrightMagenta, BrightCyan, BrightWhite]
    /// Also returns (fg, bg) as separate tuples.
    #[allow(clippy::type_complexity)]
    pub fn to_ansi_colors(&self) -> ([(u8, u8, u8); 16], (u8, u8, u8), (u8, u8, u8)) {
        let colors = [
            resolve_style_or_palette(self, &["black", "bg0", "base", "crust"], (0, 0, 0)),
            resolve_style_or_palette(self, &["red", "maroon"], (204, 36, 29)),
            resolve_style_or_palette(self, &["green"], (152, 151, 26)),
            resolve_style_or_palette(self, &["yellow", "peach", "orange"], (215, 153, 33)),
            resolve_style_or_palette(self, &["blue", "sapphire"], (69, 133, 136)),
            resolve_style_or_palette(
                self,
                &["magenta", "purple", "pink", "mauve"],
                (177, 98, 134),
            ),
            resolve_style_or_palette(self, &["cyan", "aqua", "teal", "sky"], (104, 157, 106)),
            resolve_style_or_palette(
                self,
                &["white", "fg0", "fg1", "text", "fg"],
                (235, 219, 178),
            ),
            // Bright variants
            resolve_style_or_palette(
                self,
                &["bright_black", "bg3", "overlay0", "comment"],
                (146, 131, 116),
            ),
            resolve_style_or_palette(self, &["bright_red"], (251, 73, 52)),
            resolve_style_or_palette(self, &["bright_green"], (184, 187, 38)),
            resolve_style_or_palette(self, &["bright_yellow"], (250, 189, 47)),
            resolve_style_or_palette(self, &["bright_blue"], (131, 165, 152)),
            resolve_style_or_palette(self, &["bright_purple", "bright_magenta"], (211, 134, 155)),
            resolve_style_or_palette(self, &["bright_cyan", "bright_aqua"], (142, 192, 124)),
            resolve_style_or_palette(self, &["bright_white", "fg0"], (253, 244, 193)),
        ];

        // FG from ui.text, BG from ui.background
        let fg = self
            .style("ui.text")
            .fg
            .map(|c| match c {
                ThemeColor::Rgb(r, g, b) => (r, g, b),
                ThemeColor::Named(n) => named_to_rgb(n),
            })
            .unwrap_or((235, 219, 178));

        let bg = self
            .style("ui.background")
            .bg
            .map(|c| match c {
                ThemeColor::Rgb(r, g, b) => (r, g, b),
                ThemeColor::Named(n) => named_to_rgb(n),
            })
            .unwrap_or((0, 0, 0));

        (colors, fg, bg)
    }

    /// Compute the relative luminance of the `ui.background` bg color.
    /// Returns a value in [0.0, 1.0] where 0 = black, 1 = white.
    /// Uses the sRGB luminance formula: 0.2126*R + 0.7152*G + 0.0722*B.
    pub fn background_luminance(&self) -> f64 {
        let (_, _, bg) = self.to_ansi_colors();
        (0.2126 * bg.0 as f64 + 0.7152 * bg.1 as f64 + 0.0722 * bg.2 as f64) / 255.0
    }

    /// Whether this theme is considered "dark" (background luminance < 0.5).
    pub fn is_dark(&self) -> bool {
        self.background_luminance() < 0.5
    }

    /// Resolve a ThemeColor to concrete RGB values.
    pub fn resolve_to_rgb(color: &ThemeColor) -> (u8, u8, u8) {
        match color {
            ThemeColor::Rgb(r, g, b) => (*r, *g, *b),
            ThemeColor::Named(n) => named_to_rgb(*n),
        }
    }

    /// List all available style keys in this theme (sorted).
    pub fn style_keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.resolved_cache.keys().map(|s| s.as_str()).collect();
        keys.sort();
        keys
    }

    /// Mutate a palette color and rebuild the resolved cache.
    /// For Scheme runtime palette mutation.
    pub fn set_palette_color(&mut self, name: &str, color: ThemeColor) {
        self.palette.insert(name.to_string(), color);
        self.rebuild_cache();
    }

    /// Set or update an unresolved style and rebuild the resolved cache.
    /// For Scheme runtime style mutation.
    pub fn set_style(&mut self, key: &str, style: UnresolvedStyle) {
        self.unresolved.insert(key.to_string(), style);
        self.rebuild_cache();
    }
}

/// Resolve a color string: could be a hex color, palette reference, or ANSI name.
/// Palette entries take priority over ANSI names — a palette entry named "red"
/// overrides the ANSI color "red".
fn resolve_color_value(
    s: &str,
    palette: &HashMap<String, ThemeColor>,
) -> Result<ThemeColor, ThemeError> {
    if s.starts_with('#') {
        parse_hex_color(s)
    } else if let Some(color) = palette.get(s) {
        Ok(*color)
    } else if let Some(named) = parse_named_ansi(s) {
        Ok(ThemeColor::Named(named))
    } else {
        Err(ThemeError::ColorError(format!(
            "unknown color '{}' (not hex, palette entry, or ANSI name)",
            s
        )))
    }
}

/// Like resolve_color_value but returns Option instead of Result (for cache building).
fn resolve_color_value_ok(s: &str, palette: &HashMap<String, ThemeColor>) -> Option<ThemeColor> {
    resolve_color_value(s, palette).ok()
}

/// Parse a TOML value into an UnresolvedStyle (color name strings, not resolved).
fn parse_unresolved_style(val: &toml::Value) -> Result<UnresolvedStyle, ThemeError> {
    match val {
        toml::Value::String(color_str) => Ok(UnresolvedStyle {
            fg: Some(color_str.clone()),
            ..Default::default()
        }),
        toml::Value::Table(tbl) => {
            let fg = tbl.get("fg").and_then(|v| v.as_str()).map(String::from);
            let bg = tbl.get("bg").and_then(|v| v.as_str()).map(String::from);
            let bold = tbl.get("bold").and_then(|v| v.as_bool()).unwrap_or(false);
            let italic = tbl.get("italic").and_then(|v| v.as_bool()).unwrap_or(false);
            let dim = tbl.get("dim").and_then(|v| v.as_bool()).unwrap_or(false);
            let underline = tbl
                .get("underline")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(UnresolvedStyle {
                fg,
                bg,
                bold,
                italic,
                dim,
                underline,
            })
        }
        _ => Err(ThemeError::ParseError(
            "style value must be a string or table".into(),
        )),
    }
}

fn resolve_style_or_palette(
    theme: &Theme,
    candidates: &[&str],
    fallback: (u8, u8, u8),
) -> (u8, u8, u8) {
    for name in candidates {
        if let Some(color) = theme.palette.get(*name) {
            return match color {
                ThemeColor::Rgb(r, g, b) => (*r, *g, *b),
                ThemeColor::Named(named) => named_to_rgb(*named),
            };
        }
    }
    fallback
}

fn named_to_rgb(named: NamedColor) -> (u8, u8, u8) {
    match named {
        NamedColor::Black => (0, 0, 0),
        NamedColor::Red => (204, 36, 29),
        NamedColor::Green => (152, 151, 26),
        NamedColor::Yellow => (215, 153, 33),
        NamedColor::Blue => (69, 133, 136),
        NamedColor::Magenta => (177, 98, 134),
        NamedColor::Cyan => (104, 157, 106),
        NamedColor::White => (235, 219, 178),
        NamedColor::DarkGray => (146, 131, 116),
        NamedColor::LightRed => (251, 73, 52),
        NamedColor::LightGreen => (184, 187, 38),
        NamedColor::LightYellow => (250, 189, 47),
        NamedColor::LightBlue => (131, 165, 152),
        NamedColor::LightMagenta => (211, 134, 155),
        NamedColor::LightCyan => (142, 192, 124),
        NamedColor::Gray => (168, 153, 132),
    }
}

/// Load the default theme (ANSI-only, works on all terminals).
pub fn default_theme() -> Theme {
    Theme::load("default", &BundledResolver).expect("bundled default theme must parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_colors() {
        assert_eq!(
            parse_hex_color("#cc241d").unwrap(),
            ThemeColor::Rgb(204, 36, 29)
        );
        assert_eq!(
            parse_hex_color("#fff").unwrap(),
            ThemeColor::Rgb(255, 255, 255)
        );
        assert_eq!(
            parse_hex_color("#282828").unwrap(),
            ThemeColor::Rgb(40, 40, 40)
        );
        // 8-char hex (alpha ignored)
        assert_eq!(
            parse_hex_color("#cc241dff").unwrap(),
            ThemeColor::Rgb(204, 36, 29)
        );
    }

    #[test]
    fn parse_minimal_theme() {
        let toml = r##"
[palette]
red = "#cc241d"
bg = "#282828"

[styles]
"ui.text" = { fg = "red" }
"ui.background" = { bg = "bg" }
"##;
        let theme = Theme::from_toml("test", toml).unwrap();
        assert_eq!(theme.name, "test");
        assert_eq!(theme.palette["red"], ThemeColor::Rgb(204, 36, 29));
        assert_eq!(
            theme.style("ui.text").fg,
            Some(ThemeColor::Rgb(204, 36, 29))
        );
        assert_eq!(
            theme.style("ui.background").bg,
            Some(ThemeColor::Rgb(40, 40, 40))
        );
    }

    #[test]
    fn style_key_fallback() {
        let toml = r#"
[styles]
"ui.statusline" = { fg = "white", bg = "black" }
"#;
        let theme = Theme::from_toml("test", toml).unwrap();
        // Specific key falls back to parent
        let style = theme.style("ui.statusline.mode.normal");
        assert_eq!(style.fg, Some(ThemeColor::Named(NamedColor::White)));
        assert_eq!(style.bg, Some(ThemeColor::Named(NamedColor::Black)));
    }

    #[test]
    fn style_string_shorthand() {
        let toml = r#"
[styles]
"keyword" = "red"
"#;
        let theme = Theme::from_toml("test", toml).unwrap();
        assert_eq!(
            theme.style("keyword").fg,
            Some(ThemeColor::Named(NamedColor::Red))
        );
    }

    #[test]
    fn theme_inheritance() {
        let parent_toml = r##"
[palette]
red = "#ff0000"
blue = "#0000ff"

[styles]
"ui.text" = { fg = "red" }
"ui.gutter" = { fg = "blue" }
"##;
        let child_toml = r##"
inherits = "parent"

[palette]
red = "#cc0000"

[styles]
"ui.text" = { fg = "red" }
"##;

        struct TestResolver {
            parent: String,
            child: String,
        }
        impl ThemeResolver for TestResolver {
            fn resolve(&self, name: &str) -> Option<String> {
                match name {
                    "parent" => Some(self.parent.clone()),
                    "child" => Some(self.child.clone()),
                    _ => None,
                }
            }
        }

        let resolver = TestResolver {
            parent: parent_toml.to_string(),
            child: child_toml.to_string(),
        };

        let theme = Theme::load("child", &resolver).unwrap();
        // Child overrides red in palette
        assert_eq!(theme.palette["red"], ThemeColor::Rgb(204, 0, 0));
        // Child overrides ui.text
        assert_eq!(theme.style("ui.text").fg, Some(ThemeColor::Rgb(204, 0, 0)));
        // Parent's ui.gutter is inherited — but blue is NOT overridden,
        // so it stays as the parent's blue.
        assert_eq!(
            theme.style("ui.gutter").fg,
            Some(ThemeColor::Rgb(0, 0, 255))
        );
    }

    #[test]
    fn inherited_styles_re_resolve_against_child_palette() {
        // Parent defines "markup.heading" with fg = "yellow" but no palette
        // entry for yellow — resolves to Named(Yellow).
        // Child overrides yellow in palette → inherited style should pick up
        // the child's yellow, not the ANSI fallback.
        let parent_toml = r#"
[palette]

[styles]
"markup.heading" = { fg = "yellow", bold = true }
"#;
        let child_toml = r##"
inherits = "parent"

[palette]
yellow = "#e5c07b"

[styles]
"##;

        struct Resolver {
            parent: String,
            child: String,
        }
        impl ThemeResolver for Resolver {
            fn resolve(&self, name: &str) -> Option<String> {
                match name {
                    "parent" => Some(self.parent.clone()),
                    "child" => Some(self.child.clone()),
                    _ => None,
                }
            }
        }

        let resolver = Resolver {
            parent: parent_toml.to_string(),
            child: child_toml.to_string(),
        };

        let theme = Theme::load("child", &resolver).unwrap();
        let heading = theme.style("markup.heading");
        // Should be the child's palette yellow, not Named(Yellow).
        assert_eq!(heading.fg, Some(ThemeColor::Rgb(229, 192, 123)));
        assert!(heading.bold);
    }

    #[test]
    fn inherited_style_uses_child_palette_rgb() {
        // Parent defines markup.heading with fg = "yellow" AND has a palette
        // entry for yellow (RGB). Child overrides yellow with different RGB.
        // The old code baked parent's RGB at parse time — this test verifies
        // the child's palette yellow is used instead.
        let parent_toml = r##"
[palette]
yellow = "#d79921"

[styles]
"markup.heading" = { fg = "yellow", bold = true }
"##;
        let child_toml = r##"
inherits = "parent"

[palette]
yellow = "#e5c07b"

[styles]
"##;

        struct Resolver {
            parent: String,
            child: String,
        }
        impl ThemeResolver for Resolver {
            fn resolve(&self, name: &str) -> Option<String> {
                match name {
                    "parent" => Some(self.parent.clone()),
                    "child" => Some(self.child.clone()),
                    _ => None,
                }
            }
        }

        let resolver = Resolver {
            parent: parent_toml.to_string(),
            child: child_toml.to_string(),
        };

        let theme = Theme::load("child", &resolver).unwrap();
        let heading = theme.style("markup.heading");
        // Must be child's yellow (#e5c07b), NOT parent's (#d79921)
        assert_eq!(heading.fg, Some(ThemeColor::Rgb(0xe5, 0xc0, 0x7b)));
        assert!(heading.bold);
    }

    #[test]
    fn markup_heading_differs_across_themes() {
        let resolver = BundledResolver;
        let default = Theme::load("default", &resolver).unwrap();
        let one_dark = Theme::load("one-dark", &resolver).unwrap();
        let default_heading = default.style("markup.heading");
        let one_dark_heading = one_dark.style("markup.heading");
        // Both should have a heading fg, but with different colors
        assert!(default_heading.fg.is_some());
        assert!(one_dark_heading.fg.is_some());
        assert_ne!(
            default_heading.fg, one_dark_heading.fg,
            "markup.heading fg should differ between default and one-dark themes"
        );
    }

    #[test]
    fn palette_mutation_rebuilds_cache() {
        let toml = r##"
[palette]
red = "#ff0000"

[styles]
"keyword" = { fg = "red" }
"##;
        let mut theme = Theme::from_toml("test", toml).unwrap();
        assert_eq!(theme.style("keyword").fg, Some(ThemeColor::Rgb(255, 0, 0)));

        // Mutate palette
        theme.set_palette_color("red", ThemeColor::Rgb(0, 255, 0));
        assert_eq!(
            theme.style("keyword").fg,
            Some(ThemeColor::Rgb(0, 255, 0)),
            "style should reflect new palette color after mutation"
        );
    }

    #[test]
    fn set_style_rebuilds_cache() {
        let toml = r#"
[styles]
"keyword" = { fg = "red" }
"#;
        let mut theme = Theme::from_toml("test", toml).unwrap();
        assert_eq!(
            theme.style("keyword").fg,
            Some(ThemeColor::Named(NamedColor::Red))
        );

        // Set a new style
        theme.set_style(
            "keyword",
            UnresolvedStyle {
                fg: Some("blue".into()),
                bold: true,
                ..Default::default()
            },
        );
        let s = theme.style("keyword");
        assert_eq!(s.fg, Some(ThemeColor::Named(NamedColor::Blue)));
        assert!(s.bold);
    }

    #[test]
    fn all_bundled_themes_define_cursorline() {
        let resolver = BundledResolver;
        for name in bundled_theme_names() {
            let theme = Theme::load(&name, &resolver).unwrap();
            let style = theme.style("ui.cursorline");
            assert!(
                style.bg.is_some(),
                "bundled theme '{}' must define ui.cursorline with a bg color",
                name,
            );
        }
    }

    #[test]
    fn bundled_themes_all_parse() {
        let resolver = BundledResolver;
        for name in bundled_theme_names() {
            let result = Theme::load(&name, &resolver);
            assert!(
                result.is_ok(),
                "bundled theme '{}' failed to parse: {:?}",
                name,
                result.err()
            );
        }
    }

    #[test]
    fn default_theme_loads() {
        let theme = default_theme();
        assert_eq!(theme.name, "default");
        // Should have basic styles defined
        let text = theme.style("ui.text");
        assert!(text.fg.is_some(), "default theme must define ui.text fg");
    }

    #[test]
    fn style_modifiers() {
        let toml = r#"
[styles]
"keyword" = { fg = "red", bold = true, italic = true }
"#;
        let theme = Theme::from_toml("test", toml).unwrap();
        let style = theme.style("keyword");
        assert!(style.bold);
        assert!(style.italic);
        assert!(!style.dim);
        assert!(!style.underline);
    }

    #[test]
    fn background_luminance_dark_theme() {
        let resolver = BundledResolver;
        let theme = Theme::load("gruvbox-dark", &resolver).unwrap();
        assert!(theme.is_dark(), "gruvbox-dark should be dark");
        assert!(
            theme.background_luminance() < 0.3,
            "dark theme bg luminance should be low"
        );
    }

    #[test]
    fn background_luminance_light_theme() {
        let resolver = BundledResolver;
        let theme = Theme::load("gruvbox-light", &resolver).unwrap();
        assert!(!theme.is_dark(), "gruvbox-light should not be dark");
        assert!(
            theme.background_luminance() > 0.5,
            "light theme bg luminance should be high"
        );
    }

    #[test]
    fn catppuccin_mocha_ui_background_is_base() {
        // Regression: ui.background must resolve to the theme's palette "base"
        // color, not a fallback.
        let resolver = BundledResolver;
        let theme = Theme::load("catppuccin-mocha", &resolver).unwrap();
        let style = theme.style("ui.background");
        assert_eq!(
            style.bg,
            Some(ThemeColor::Rgb(0x1e, 0x1e, 0x2e)),
            "catppuccin-mocha ui.background should be #1e1e2e"
        );
    }

    #[test]
    fn to_ansi_colors_bg_fallback_is_black() {
        // Regression: default bg fallback should be (0,0,0), not (40,40,40).
        let toml = r#"
[styles]
"ui.text" = { fg = "white" }
"#;
        let theme = Theme::from_toml("minimal", toml).unwrap();
        let (_, _, bg) = theme.to_ansi_colors();
        assert_eq!(bg, (0, 0, 0), "bg fallback should be black");
    }

    #[test]
    fn resolve_to_rgb_named_colors() {
        assert_eq!(
            Theme::resolve_to_rgb(&ThemeColor::Rgb(10, 20, 30)),
            (10, 20, 30)
        );
        assert_eq!(
            Theme::resolve_to_rgb(&ThemeColor::Named(NamedColor::Red)),
            (204, 36, 29)
        );
    }
}
