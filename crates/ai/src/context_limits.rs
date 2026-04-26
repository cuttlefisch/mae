//! Model capabilities lookup table — context windows and tool loop limits.
//!
//! Same prefix-match pattern as `pricing.rs`. Unknown models default to
//! conservative values grounded in commercial API norms.

/// Capabilities and limits for a specific model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLimits {
    /// Maximum input tokens the model can process.
    pub context_window: u64,
    /// Maximum tool-call rounds allowed in a single turn.
    /// Grounded in provider limits (Claude: 20-50, Gemini: 50, OpenAI: 100).
    pub max_rounds: usize,
}

impl Default for ModelLimits {
    fn default() -> Self {
        ModelLimits {
            context_window: DEFAULT_CONTEXT_WINDOW,
            max_rounds: DEFAULT_MAX_ROUNDS,
        }
    }
}

/// Look up a model's limits.
/// Matches by longest prefix — `deepseek-chat-v2` still hits `deepseek-chat`.
/// Returns conservative defaults for unknown models.
pub fn lookup(model: &str) -> ModelLimits {
    let lower = model.to_ascii_lowercase();
    for (prefix, window, rounds) in TABLE {
        if lower.starts_with(prefix) {
            return ModelLimits {
                context_window: *window,
                max_rounds: *rounds,
            };
        }
    }
    ModelLimits::default()
}

/// Conservative default context window for unknown models.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;
/// Conservative default max rounds for unknown models.
pub const DEFAULT_MAX_ROUNDS: usize = 50;

/// Model limit table: (prefix, context_window, max_rounds)
/// Order matters — longer prefixes first.
const TABLE: &[(&str, u64, usize)] = &[
    // ---- Anthropic (Claude) ----
    // Anthropic enforced tool loop pauses at ~20, but the API supports
    // more if resumed. We set 50 as a reasonable "deep task" bound.
    ("claude-opus-4", 200_000, 50),
    ("claude-sonnet-4", 200_000, 50),
    ("claude-haiku-4", 200_000, 50),
    ("claude-3-5-sonnet", 200_000, 50),
    ("claude-3-5-haiku", 200_000, 50),
    ("claude-3-opus", 200_000, 50),
    // ---- OpenAI ----
    // OpenAI is primarily context-limited, but 100 is a safe upper bound
    // to prevent infinite loops / token drains.
    ("gpt-4o-mini", 128_000, 30),
    ("gpt-4o", 128_000, 100),
    ("gpt-4-turbo", 128_000, 100),
    ("gpt-4", 8_192, 50),
    ("o1-mini", 128_000, 100),
    ("o1", 200_000, 100),
    // ---- Google (Gemini) ----
    // Gemini agent loops are typically optimized for ~50 rounds.
    ("gemini-2.5-flash-lite", 1_000_000, 50),
    ("gemini-2.5-pro", 1_000_000, 50),
    ("gemini-2.5-flash", 1_000_000, 50),
    // ---- DeepSeek ----
    // DeepSeek is often used for heavy reasoning/tool-chains.
    // 50 rounds matches Claude/Gemini — 25 was too low for self-test (~35 calls).
    ("deepseek-reasoner", 64_000, 50),
    ("deepseek-chat", 64_000, 50),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_sonnet_limits() {
        let l = lookup("claude-sonnet-4-5");
        assert_eq!(l.context_window, 200_000);
        assert_eq!(l.max_rounds, 50);
    }

    #[test]
    fn deepseek_chat_limits() {
        let l = lookup("deepseek-chat");
        assert_eq!(l.context_window, 64_000);
        assert_eq!(l.max_rounds, 50);
    }

    #[test]
    fn gemini_pro_limits() {
        let l = lookup("gemini-2.5-pro");
        assert_eq!(l.context_window, 1_000_000);
        assert_eq!(l.max_rounds, 50);
    }

    #[test]
    fn gpt4o_mini_limits() {
        let l = lookup("gpt-4o-mini");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.max_rounds, 30);
    }

    #[test]
    fn unknown_defaults() {
        let l = lookup("llama3");
        assert_eq!(l.context_window, DEFAULT_CONTEXT_WINDOW);
        assert_eq!(l.max_rounds, DEFAULT_MAX_ROUNDS);
    }
}
