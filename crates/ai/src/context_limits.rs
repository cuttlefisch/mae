//! Model context window lookup table — maximum tokens per model.
//!
//! Same prefix-match pattern as `pricing.rs`. Unknown models default to
//! 128K tokens (a safe floor for most commercial APIs).

/// Look up a model's context window size in tokens.
/// Matches by longest prefix — `deepseek-chat-v2` still hits `deepseek-chat`.
/// Returns a conservative 128K default for unknown models.
pub fn lookup(model: &str) -> u64 {
    let lower = model.to_ascii_lowercase();
    for (prefix, limit) in TABLE {
        if lower.starts_with(prefix) {
            return *limit;
        }
    }
    DEFAULT_CONTEXT_WINDOW
}

/// Conservative default for unknown models.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

/// Context window sizes. Order matters — longer prefixes first.
const TABLE: &[(&str, u64)] = &[
    // ---- Anthropic ----
    ("claude-opus-4", 200_000),
    ("claude-sonnet-4", 200_000),
    ("claude-haiku-4", 200_000),
    ("claude-3-5-sonnet", 200_000),
    ("claude-3-5-haiku", 200_000),
    ("claude-3-opus", 200_000),
    // ---- OpenAI ----
    ("gpt-4o-mini", 128_000),
    ("gpt-4o", 128_000),
    ("gpt-4-turbo", 128_000),
    ("gpt-4", 8_192),
    ("o1-mini", 128_000),
    ("o1", 200_000),
    // ---- Gemini ----
    ("gemini-3.1-pro", 1_000_000),
    ("gemini-3.1-flash", 1_000_000),
    ("gemini-3.0-flash", 1_000_000),
    ("gemini-2.5-pro", 1_000_000),
    ("gemini-2.5-flash", 1_000_000),
    // ---- DeepSeek ----
    ("deepseek-reasoner", 131_072),
    ("deepseek-chat", 131_072),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_sonnet_200k() {
        assert_eq!(lookup("claude-sonnet-4-5"), 200_000);
    }

    #[test]
    fn claude_opus_200k() {
        assert_eq!(lookup("claude-opus-4-6"), 200_000);
    }

    #[test]
    fn deepseek_chat_131k() {
        assert_eq!(lookup("deepseek-chat"), 131_072);
    }

    #[test]
    fn deepseek_reasoner_131k() {
        assert_eq!(lookup("deepseek-reasoner"), 131_072);
    }

    #[test]
    fn gemini_1m() {
        assert_eq!(lookup("gemini-2.5-pro"), 1_000_000);
        assert_eq!(lookup("gemini-2.5-flash"), 1_000_000);
    }

    #[test]
    fn gpt4o_mini_128k() {
        assert_eq!(lookup("gpt-4o-mini"), 128_000);
    }

    #[test]
    fn gpt4_legacy_8k() {
        assert_eq!(lookup("gpt-4"), 8_192);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(lookup("Claude-Opus-4-6"), 200_000);
        assert_eq!(lookup("GPT-4O-MINI"), 128_000);
    }

    #[test]
    fn unknown_defaults_to_128k() {
        assert_eq!(lookup("llama3"), DEFAULT_CONTEXT_WINDOW);
        assert_eq!(lookup("qwen2.5-coder:7b"), DEFAULT_CONTEXT_WINDOW);
    }

    #[test]
    fn dated_revision_matches_prefix() {
        assert_eq!(lookup("claude-haiku-4-5-20251001"), 200_000);
    }
}
