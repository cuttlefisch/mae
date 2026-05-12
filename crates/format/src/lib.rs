//! mae-format: Code formatter bridge — external commands and format-on-save.
//!
//! @stability: experimental
//! @since: 0.9.0
//!
//! Complements LSP formatting with external formatter support (prettier, black,
//! rustfmt, gofmt, etc.). Formatters are configured per-language and can run
//! via stdin piping or filename argument.

pub mod external;

use std::collections::HashMap;
use std::path::Path;

/// Result of a format operation.
#[derive(Debug, Clone)]
pub struct FormatResult {
    /// The formatted text.
    pub formatted: String,
    /// Whether the text was changed.
    pub changed: bool,
}

/// Configuration for an external formatter.
#[derive(Debug, Clone)]
pub struct ExternalFormatter {
    /// Command to run (e.g., "prettier", "black", "rustfmt").
    pub command: String,
    /// Arguments to pass (e.g., ["--stdin-filepath", "{file}"]).
    /// `{file}` is replaced with the actual file path.
    pub args: Vec<String>,
    /// If true, pipe content via stdin. If false, pass filename as argument.
    pub stdin: bool,
}

/// Format configuration: maps language identifiers to formatters.
#[derive(Debug, Clone, Default)]
pub struct FormatConfig {
    /// Language ID → formatter definition.
    pub formatters: HashMap<String, ExternalFormatter>,
}

impl FormatConfig {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a formatter for a language.
    pub fn set(&mut self, language: &str, formatter: ExternalFormatter) {
        self.formatters.insert(language.to_string(), formatter);
    }

    /// Get the formatter for a language, if configured.
    pub fn get(&self, language: &str) -> Option<&ExternalFormatter> {
        self.formatters.get(language)
    }

    /// Parse a format config from a TOML-like map.
    ///
    /// Expected structure:
    /// ```toml
    /// [format.rust]
    /// command = "rustfmt"
    /// stdin = true
    ///
    /// [format.python]
    /// command = "black"
    /// args = ["-", "-q"]
    /// stdin = true
    /// ```
    pub fn from_entries(entries: &[(String, String, Vec<String>, bool)]) -> Self {
        let mut config = Self::new();
        for (lang, command, args, stdin) in entries {
            config.set(
                lang,
                ExternalFormatter {
                    command: command.clone(),
                    args: args.clone(),
                    stdin: *stdin,
                },
            );
        }
        config
    }
}

/// Format content using an external formatter.
///
/// If the formatter uses stdin mode, pipes content through the command.
/// If not, writes to a temp file, runs the command, reads back.
pub fn format_with_external(
    formatter: &ExternalFormatter,
    content: &str,
    file_path: &Path,
) -> Result<FormatResult, String> {
    external::run_formatter(formatter, content, file_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_set_and_get() {
        let mut config = FormatConfig::new();
        config.set(
            "rust",
            ExternalFormatter {
                command: "rustfmt".into(),
                args: vec![],
                stdin: true,
            },
        );
        assert!(config.get("rust").is_some());
        assert!(config.get("python").is_none());
    }

    #[test]
    fn config_from_entries() {
        let entries = vec![(
            "python".into(),
            "black".into(),
            vec!["-".into(), "-q".into()],
            true,
        )];
        let config = FormatConfig::from_entries(&entries);
        let fmt = config.get("python").unwrap();
        assert_eq!(fmt.command, "black");
        assert!(fmt.stdin);
    }
}
