//! Shared message-level prefix logic for the *Messages* buffer.
//!
//! Both GUI and TUI renderers need the same level tag → theme key mapping.
//! This module extracts that duplicated logic into a single source of truth.

use crate::MessageLevel;

/// Pre-computed prefix for a message log entry.
pub struct MessagePrefix {
    /// Display tag, padded to 5 chars: `"ERROR"`, `" WARN"`, `" INFO"`, `"DEBUG"`, `"TRACE"`.
    pub tag: &'static str,
    /// Theme key for the level color: `"diagnostic.error"`, etc.
    pub theme_key: &'static str,
}

/// Return the display tag and theme key for a message level.
pub fn message_prefix(level: MessageLevel) -> MessagePrefix {
    match level {
        MessageLevel::Error => MessagePrefix {
            tag: "ERROR",
            theme_key: "diagnostic.error",
        },
        MessageLevel::Warn => MessagePrefix {
            tag: " WARN",
            theme_key: "diagnostic.warn",
        },
        MessageLevel::Info => MessagePrefix {
            tag: " INFO",
            theme_key: "diagnostic.info",
        },
        MessageLevel::Debug => MessagePrefix {
            tag: "DEBUG",
            theme_key: "diagnostic.debug",
        },
        MessageLevel::Trace => MessagePrefix {
            tag: "TRACE",
            theme_key: "diagnostic.trace",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_prefix_levels() {
        let cases = [
            (MessageLevel::Error, "ERROR", "diagnostic.error"),
            (MessageLevel::Warn, " WARN", "diagnostic.warn"),
            (MessageLevel::Info, " INFO", "diagnostic.info"),
            (MessageLevel::Debug, "DEBUG", "diagnostic.debug"),
            (MessageLevel::Trace, "TRACE", "diagnostic.trace"),
        ];
        for (level, expected_tag, expected_key) in cases {
            let p = message_prefix(level);
            assert_eq!(p.tag, expected_tag);
            assert_eq!(p.theme_key, expected_key);
            assert_eq!(p.tag.len(), 5, "all tags must be 5 chars for alignment");
        }
    }
}
