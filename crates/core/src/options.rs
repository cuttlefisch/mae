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
    /// Valid values for enum-like options (tab completion). Empty = any value.
    pub valid_values: &'static [&'static str],
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
                    valid_values: &[],
                },
                OptionDef {
                    name: "relative_line_numbers",
                    aliases: &["relative-line-numbers"],
                    doc: "Use relative line numbering (distance from cursor)",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.relative_line_numbers"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "word_wrap",
                    aliases: &["word-wrap"],
                    doc: "Soft-wrap long lines at the window edge",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.word_wrap"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "break_indent",
                    aliases: &["break-indent"],
                    doc: "Indent wrapped continuation lines to match the original indentation",
                    kind: OptionKind::Bool,
                    default_value: "true",
                    config_key: Some("editor.break_indent"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "show_break",
                    aliases: &["show-break"],
                    doc: "Character prefix for wrapped continuation lines (e.g. \"↪ \")",
                    kind: OptionKind::String,
                    default_value: "↪ ",
                    config_key: Some("editor.show_break"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "org_hide_emphasis_markers",
                    aliases: &["org-hide-emphasis-markers"],
                    doc: "Hide *bold* and /italic/ markers in Org-mode",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.org_hide_emphasis_markers"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "show_fps",
                    aliases: &["show-fps"],
                    doc: "Show FPS/frame-timing overlay in the status bar",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.show_fps"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "font_size",
                    aliases: &["font-size"],
                    doc: "GUI font size in points (6.0–72.0). Takes effect immediately.",
                    kind: OptionKind::Float,
                    default_value: "14.0",
                    config_key: Some("editor.font_size"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "font_family",
                    aliases: &["font-family"],
                    doc: "Primary GUI monospace font family",
                    kind: OptionKind::String,
                    default_value: "",
                    config_key: Some("editor.font_family"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "icon_font_family",
                    aliases: &["icon-font-family"],
                    doc: "Secondary GUI font family for icons and symbols (fallback)",
                    kind: OptionKind::String,
                    default_value: "",
                    config_key: Some("editor.icon_font_family"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "theme",
                    aliases: &[],
                    doc: "Color theme name (use `:theme <name>` or `SPC t t` to cycle)",
                    kind: OptionKind::Theme,
                    default_value: "default",
                    config_key: Some("editor.theme"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "splash_art",
                    aliases: &["splash-art"],
                    doc: "ASCII art variant for the splash screen",
                    kind: OptionKind::String,
                    default_value: "bat",
                    config_key: None,
                    valid_values: &[],
                },
                OptionDef {
                    name: "debug_mode",
                    aliases: &["debug-mode"],
                    doc:
                        "Show RSS/CPU/frame-time in the status bar (Emacs --debug-init equivalent)",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.debug_mode"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "clipboard",
                    aliases: &["clipboard-mode"],
                    doc: "Clipboard integration: \"unnamedplus\" (paste from system clipboard), \"unnamed\" (yank syncs out, paste reads internal), \"internal\" (no clipboard sync)",
                    kind: OptionKind::String,
                    default_value: "unnamed",
                    config_key: Some("editor.clipboard"),
                    valid_values: &["unnamedplus", "unnamed", "internal"],
                },
                OptionDef {
                    name: "ai_tier",
                    aliases: &["ai-tier"],
                    doc: "Current AI permission tier (ReadOnly, Write, Shell, Privileged)",
                    kind: OptionKind::String,
                    default_value: "ReadOnly",
                    config_key: Some("ai.auto_approve_tier"),
                    valid_values: &["ReadOnly", "Write", "Shell", "Privileged"],
                },
                OptionDef {
                    name: "ai_editor",
                    aliases: &["ai-editor"],
                    doc: "Command to launch for AI agent shell sessions (e.g. claude, aider)",
                    kind: OptionKind::String,
                    default_value: "claude",
                    config_key: Some("ai.editor"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "ai_provider",
                    aliases: &["ai-provider"],
                    doc: "AI API provider: claude, openai, gemini, ollama, deepseek",
                    kind: OptionKind::String,
                    default_value: "",
                    config_key: Some("ai.provider"),
                    valid_values: &["claude", "openai", "gemini", "ollama", "deepseek"],
                },
                OptionDef {
                    name: "ai_model",
                    aliases: &["ai-model"],
                    doc: "AI model identifier (empty = provider default)",
                    kind: OptionKind::String,
                    default_value: "",
                    config_key: Some("ai.model"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "ai_api_key_command",
                    aliases: &["ai-api-key-command"],
                    doc: "Shell command whose stdout is the API key (e.g. \"pass show deepseek/api-key\")",
                    kind: OptionKind::String,
                    default_value: "",
                    config_key: Some("ai.api_key_command"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "ai_base_url",
                    aliases: &["ai-base-url"],
                    doc: "Base URL override for the AI API endpoint",
                    kind: OptionKind::String,
                    default_value: "",
                    config_key: Some("ai.base_url"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "ai_mode",
                    aliases: &["ai-mode"],
                    doc: "AI operating mode: \"standard\" (manual approval), \"plan\" (drafting only), \"auto-accept\" (hands-free execution for small tasks)",
                    kind: OptionKind::String,
                    default_value: "standard",
                    config_key: Some("ai.mode"),
                    valid_values: &["standard", "plan", "auto-accept"],
                },
                OptionDef {
                    name: "ai_profile",
                    aliases: &["ai-profile"],
                    doc: "Active AI prompt profile: \"pair-programmer\", \"explorer\", \"planner\", \"reviewer\"",
                    kind: OptionKind::String,
                    default_value: "pair-programmer",
                    config_key: Some("ai.profile"),
                    valid_values: &["pair-programmer", "explorer", "planner", "reviewer"],
                },
                OptionDef {
                    name: "restore_session",
                    aliases: &["restore-session"],
                    doc: "Automatically restore the previous session on startup (per-project)",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.restore_session"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "insert_ctrl_d",
                    aliases: &["insert-ctrl-d"],
                    doc: "Insert-mode C-d behavior: \"dedent\" (vim, default) or \"delete-forward\" (Emacs)",
                    kind: OptionKind::String,
                    default_value: "dedent",
                    config_key: Some("editor.insert_ctrl_d"),
                    valid_values: &["dedent", "delete-forward"],
                },
                OptionDef {
                    name: "autosave_interval",
                    aliases: &["autosave-interval"],
                    doc: "Auto-save interval in seconds (0 = disabled). Saves all modified file-backed buffers.",
                    kind: OptionKind::String,
                    default_value: "0",
                    config_key: Some("editor.autosave_interval"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "ignorecase",
                    aliases: &[],
                    doc: "Case-insensitive search (like vim :set ignorecase)",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.ignorecase"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "heading_scale",
                    aliases: &["heading-scale"],
                    doc: "Scale heading font size in org/markdown buffers (GUI only)",
                    kind: OptionKind::Bool,
                    default_value: "true",
                    config_key: Some("editor.heading_scale"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "smartcase",
                    aliases: &[],
                    doc: "When ignorecase is on and pattern contains uppercase, search case-sensitively",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.smartcase"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "scrollbar",
                    aliases: &[],
                    doc: "Show vertical scrollbar in the GUI",
                    kind: OptionKind::Bool,
                    default_value: "true",
                    config_key: Some("editor.scrollbar"),
                    valid_values: &[],
                },
                OptionDef {
                    name: "nyan_mode",
                    aliases: &["nyan-mode"],
                    doc: "Show nyan cat progress indicator in the status bar",
                    kind: OptionKind::Bool,
                    default_value: "false",
                    config_key: Some("editor.nyan_mode"),
                    valid_values: &[],
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

    /// Return all canonical option names (for tab completion).
    pub fn all_names(&self) -> Vec<String> {
        self.options.iter().map(|o| o.name.to_string()).collect()
    }

    /// Check if an option exists by canonical name or alias.
    pub fn has_option(&self, name: &str) -> bool {
        self.find(name).is_some()
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
