//! Editor option registry — single source of truth for all configurable options.
//!
//! Every option has a canonical name (underscore-separated, used by `:set`),
//! optional aliases (hyphen-separated, used by Scheme's `set-option!`),
//! documentation, type, default value, and an optional config.toml key path.
//!
//! The registry is queried by:
//! - `:set` ex-command handler
//! - `set-option!` Scheme function (via `Editor::set_option`)
//! - `execute_set_option` AI tool (via `Editor::set_option`)
//! - `describe-option` command
//! - KB auto-generation (`install_option_nodes`)

/// The kind of value an option accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionKind {
    Bool,
    String,
    Float,
    Theme,
}

impl std::fmt::Display for OptionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptionKind::Bool => write!(f, "boolean"),
            OptionKind::String => write!(f, "string"),
            OptionKind::Float => write!(f, "float"),
            OptionKind::Theme => write!(f, "theme name"),
        }
    }
}

/// Metadata for a single editor option.
pub struct OptionDef {
    /// Canonical name: `"line_numbers"` (underscore-separated).
    pub name: &'static str,
    /// Alternative names (e.g. Scheme hyphenated form): `["line-numbers"]`.
    pub aliases: &'static [&'static str],
    /// Human-readable documentation.
    pub doc: &'static str,
    /// Value type.
    pub kind: OptionKind,
    /// Default value as a string.
    pub default_value: &'static str,
    /// TOML path in config.toml, if persistable. e.g. `"editor.line_numbers"`.
    pub config_key: Option<&'static str>,
}

/// Registry of all known editor options.
pub struct OptionRegistry {
    options: Vec<OptionDef>,
}

impl Default for OptionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl OptionRegistry {
    pub fn new() -> Self {
        OptionRegistry {
            options: vec![
                OptionDef {
                    name: "line_numbers",
                    aliases: &["line-numbers", "show-line-numbers"],
                    doc: "Show line numbers in the gutter",
                    kind: OptionKind::Bool,
                    default_value: "true",
                    config_key: Some("editor.line_numbers"),
                },
                OptionDef {
                    name: "relative_line_numbers",
                    aliases: &["relative-line-numbers"],
                    doc: "Use relative line numbering (distance from cursor)",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.relative_line_numbers"),
                },
                OptionDef {
                    name: "word_wrap",
                    aliases: &["word-wrap"],
                    doc: "Soft-wrap long lines at the window edge",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.word_wrap"),
                },
                OptionDef {
                    name: "break_indent",
                    aliases: &["break-indent"],
                    doc: "Indent wrapped continuation lines to match the original indentation",
                    kind: OptionKind::Bool,
                    default_value: "true",
                    config_key: Some("editor.break_indent"),
                },
                OptionDef {
                    name: "show_break",
                    aliases: &["show-break"],
                    doc: "Character prefix for wrapped continuation lines (e.g. \"↪ \")",
                    kind: OptionKind::String,
                    default_value: "↪ ",
                    config_key: Some("editor.show_break"),
                },
                OptionDef {
                    name: "show_fps",
                    aliases: &["show-fps"],
                    doc: "Show FPS/frame-timing overlay in the status bar",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.show_fps"),
                },
                OptionDef {
                    name: "font_size",
                    aliases: &["font-size"],
                    doc: "GUI font size in points (6.0–72.0). Takes effect immediately.",
                    kind: OptionKind::Float,
                    default_value: "14.0",
                    config_key: Some("editor.font_size"),
                },
                OptionDef {
                    name: "theme",
                    aliases: &[],
                    doc: "Color theme name (use `:theme <name>` or `SPC t t` to cycle)",
                    kind: OptionKind::Theme,
                    default_value: "default",
                    config_key: Some("editor.theme"),
                },
                OptionDef {
                    name: "splash_art",
                    aliases: &["splash-art"],
                    doc: "ASCII art variant for the splash screen",
                    kind: OptionKind::String,
                    default_value: "bat",
                    config_key: None,
                },
                OptionDef {
                    name: "debug_mode",
                    aliases: &["debug-mode"],
                    doc:
                        "Show RSS/CPU/frame-time in the status bar (Emacs --debug-init equivalent)",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.debug_mode"),
                },
                OptionDef {
                    name: "clipboard",
                    aliases: &["clipboard-mode"],
                    doc: "Clipboard integration: \"unnamedplus\" (paste from system clipboard), \"unnamed\" (yank syncs out, paste reads internal), \"internal\" (no clipboard sync)",
                    kind: OptionKind::String,
                    default_value: "unnamed",
                    config_key: Some("editor.clipboard"),
                },
            ],
        }
    }

    /// Find an option by canonical name or alias.
    pub fn find(&self, name: &str) -> Option<&OptionDef> {
        self.options
            .iter()
            .find(|o| o.name == name || o.aliases.contains(&name))
    }

    /// List all registered options.
    pub fn list(&self) -> &[OptionDef] {
        &self.options
    }
}

/// Parse a string as a boolean value. Accepts: true, #t, 1, yes, on → true; everything else → false.
pub fn parse_option_bool(s: &str) -> Result<bool, String> {
    match s {
        "true" | "#t" | "1" | "yes" | "on" => Ok(true),
        "false" | "#f" | "0" | "no" | "off" => Ok(false),
        _ => Err(format!("Invalid boolean: '{}' (expected true/false)", s)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_finds_by_canonical_name() {
        let reg = OptionRegistry::new();
        assert!(reg.find("line_numbers").is_some());
        assert!(reg.find("word_wrap").is_some());
        assert!(reg.find("theme").is_some());
    }

    #[test]
    fn registry_finds_by_alias() {
        let reg = OptionRegistry::new();
        let opt = reg.find("line-numbers").unwrap();
        assert_eq!(opt.name, "line_numbers");

        let opt = reg.find("show-line-numbers").unwrap();
        assert_eq!(opt.name, "line_numbers");
    }

    #[test]
    fn registry_returns_none_for_unknown() {
        let reg = OptionRegistry::new();
        assert!(reg.find("nonexistent").is_none());
    }

    #[test]
    fn registry_lists_all_options() {
        let reg = OptionRegistry::new();
        assert!(reg.list().len() >= 8);
    }

    #[test]
    fn parse_bool_variants() {
        assert_eq!(parse_option_bool("true"), Ok(true));
        assert_eq!(parse_option_bool("#t"), Ok(true));
        assert_eq!(parse_option_bool("1"), Ok(true));
        assert_eq!(parse_option_bool("false"), Ok(false));
        assert_eq!(parse_option_bool("0"), Ok(false));
        assert!(parse_option_bool("maybe").is_err());
    }
}
