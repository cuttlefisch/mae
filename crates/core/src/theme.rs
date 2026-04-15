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

/// A complete editor theme.
///
/// Themes use a palette of named colors and a map of semantic style keys.
/// Style keys are dot-namespaced (e.g., "ui.statusline.mode.normal") and
/// fall back through the hierarchy when a specific key isn't defined.
///
/// Follows the Helix editor convention: TOML format, palette system,
/// theme inheritance via `inherits`.
#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub palette: HashMap<String, ThemeColor>,
    pub styles: HashMap<String, ThemeStyle>,
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
    m.insert("default".into(), include_str!("themes/default.toml"));
    m.insert("gruvbox-dark".into(), include_str!("themes/gruvbox-dark.toml"));
    m.insert("gruvbox-light".into(), include_str!("themes/gruvbox-light.toml"));
    m.insert("dracula".into(), include_str!("themes/dracula.toml"));
    m.insert("catppuccin-mocha".into(), include_str!("themes/catppuccin-mocha.toml"));
    m.insert("solarized-dark".into(), include_str!("themes/solarized-dark.toml"));
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

        let mut styles = HashMap::new();
        if let Some(toml::Value::Table(sty)) = table.get("styles") {
            for (key, val) in sty {
                styles.insert(key.clone(), parse_style_value(val, &palette)?);
            }
        }

        Ok(Theme {
            name: name.to_string(),
            palette,
            styles,
        })
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

        // Merge: parent palette/styles are the base, child overrides
        if let Some(parent) = parent {
            let mut merged_palette = parent.palette;
            for (k, v) in theme.palette {
                merged_palette.insert(k, v);
            }
            let mut merged_styles = parent.styles;
            for (k, v) in theme.styles {
                merged_styles.insert(k, v);
            }
            theme.palette = merged_palette;
            theme.styles = merged_styles;
        }

        Ok(theme)
    }

    /// Look up a style by semantic key.
    /// Falls back through dot-notation hierarchy:
    /// "ui.statusline.mode.normal" → "ui.statusline.mode" → "ui.statusline" → "ui.text" → default
    pub fn style(&self, key: &str) -> ThemeStyle {
        let mut lookup = key.to_string();
        loop {
            if let Some(style) = self.styles.get(&lookup) {
                return style.clone();
            }
            // Strip last component
            if let Some(pos) = lookup.rfind('.') {
                lookup.truncate(pos);
            } else {
                break;
            }
        }
        // Final fallback: "ui.text" for ui keys, default otherwise
        if key.starts_with("ui.") {
            if let Some(style) = self.styles.get("ui.text") {
                return style.clone();
            }
        }
        ThemeStyle::default()
    }

    /// List all available style keys in this theme (sorted).
    pub fn style_keys(&self) -> Vec<&str> {
        let mut keys: Vec<&str> = self.styles.keys().map(|s| s.as_str()).collect();
        keys.sort();
        keys
    }
}

/// Resolve a color string: could be a hex color, ANSI name, or palette reference.
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

/// Parse a TOML value into a ThemeStyle.
/// Supports:
/// - String: just fg color → `{ fg = "color" }`
/// - Table: `{ fg = "color", bg = "color", bold = true, ... }`
fn parse_style_value(
    val: &toml::Value,
    palette: &HashMap<String, ThemeColor>,
) -> Result<ThemeStyle, ThemeError> {
    match val {
        toml::Value::String(color_str) => {
            let fg = resolve_color_value(color_str, palette)?;
            Ok(ThemeStyle {
                fg: Some(fg),
                ..Default::default()
            })
        }
        toml::Value::Table(tbl) => {
            let fg = if let Some(toml::Value::String(s)) = tbl.get("fg") {
                Some(resolve_color_value(s, palette)?)
            } else {
                None
            };
            let bg = if let Some(toml::Value::String(s)) = tbl.get("bg") {
                Some(resolve_color_value(s, palette)?)
            } else {
                None
            };
            let bold = tbl
                .get("bold")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let italic = tbl
                .get("italic")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let dim = tbl
                .get("dim")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let underline = tbl
                .get("underline")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            Ok(ThemeStyle {
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

/// Load the default theme (ANSI-only, works on all terminals).
pub fn default_theme() -> Theme {
    Theme::load("default", &BundledResolver).expect("bundled default theme must parse")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hex_colors() {
        assert_eq!(parse_hex_color("#cc241d").unwrap(), ThemeColor::Rgb(204, 36, 29));
        assert_eq!(parse_hex_color("#fff").unwrap(), ThemeColor::Rgb(255, 255, 255));
        assert_eq!(parse_hex_color("#282828").unwrap(), ThemeColor::Rgb(40, 40, 40));
        // 8-char hex (alpha ignored)
        assert_eq!(parse_hex_color("#cc241dff").unwrap(), ThemeColor::Rgb(204, 36, 29));
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
        assert_eq!(
            theme.style("ui.text").fg,
            Some(ThemeColor::Rgb(204, 0, 0))
        );
        // Parent's ui.gutter is inherited
        assert_eq!(
            theme.style("ui.gutter").fg,
            Some(ThemeColor::Rgb(0, 0, 255))
        );
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
}
