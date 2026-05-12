//! Model capabilities lookup table — context windows and tool loop limits.
//!
//! Same prefix-match pattern as `pricing.rs`. Unknown models default to
//! conservative values grounded in commercial API norms.

/// Prompt tier: controls how detailed the system prompt is.
/// Frontier models (Opus, Sonnet, GPT-4o) get concise prompts;
/// smaller models (DeepSeek, Haiku, Flash) get explicit guardrails.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModelTier {
    /// Large frontier models — concise prompts work well.
    Full,
    /// Smaller/cheaper models — need explicit guardrails, tool preferences,
    /// anti-looping rules, and common recipes.
    #[default]
    Compact,
}

impl ModelTier {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Compact => "compact",
        }
    }

    pub fn parse_tier(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "full" => Self::Full,
            _ => Self::Compact,
        }
    }
}

/// Capabilities and limits for a specific model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelLimits {
    /// Maximum input tokens the model can process.
    pub context_window: u64,
    /// Maximum tool-call rounds allowed in a single turn.
    /// Grounded in provider limits (Claude: 20-50, Gemini: 50, OpenAI: 100).
    pub max_rounds: usize,
    /// Prompt tier for this model.
    pub tier: ModelTier,
}

impl Default for ModelLimits {
    fn default() -> Self {
        ModelLimits {
            context_window: DEFAULT_CONTEXT_WINDOW,
            max_rounds: DEFAULT_MAX_ROUNDS,
            tier: ModelTier::Compact,
        }
    }
}

/// Look up a model's limits.
/// Matches by longest prefix — `deepseek-chat-v2` still hits `deepseek-chat`.
/// Returns conservative defaults for unknown models.
pub fn lookup(model: &str) -> ModelLimits {
    let lower = model.to_ascii_lowercase();
    for (prefix, window, rounds, t) in TABLE {
        if lower.starts_with(prefix) {
            return ModelLimits {
                context_window: *window,
                max_rounds: *rounds,
                tier: *t,
            };
        }
    }
    ModelLimits::default()
}

/// Look up the prompt tier for a model.
/// Unknown models default to `Compact` (safe: over-prompting wastes a few
/// tokens; under-prompting wastes millions).
pub fn tier(model: &str) -> ModelTier {
    lookup(model).tier
}

/// Provider family detected from model name prefix.
/// Used for provider-specific prompt hints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHint {
    Claude,
    OpenAi,
    Gemini,
    DeepSeek,
    Unknown,
}

impl ProviderHint {
    /// Detect provider from a model name string.
    pub fn from_model(model: &str) -> Self {
        let lower = model.to_ascii_lowercase();
        if lower.starts_with("claude") {
            Self::Claude
        } else if lower.starts_with("gpt") || lower.starts_with("o1") {
            Self::OpenAi
        } else if lower.starts_with("gemini") {
            Self::Gemini
        } else if lower.starts_with("deepseek") {
            Self::DeepSeek
        } else {
            Self::Unknown
        }
    }

    /// Optional provider-specific hints to append to the system prompt.
    /// Returns `None` for Claude (the primary dev target — no extra hints needed).
    pub fn prompt_hints(&self) -> Option<&'static str> {
        match self {
            Self::Gemini => Some(concat!(
                "\n<provider-hints>\n",
                "## Gemini-Specific\n",
                "- Use explicit JSON examples when calling tools with complex args.\n",
                "- tool_choice is set to 'auto' — you can call multiple tools per turn.\n",
                "- Prefer longer, more descriptive tool call arguments.\n",
                "- RAG: call kb_search_context like: {\"query\": \"buffer management\", \"limit\": 5}\n",
                "  Response is an array of {id, title, kind, excerpt, score}.\n",
                "</provider-hints>\n",
            )),
            Self::DeepSeek => Some(concat!(
                "\n<provider-hints>\n",
                "## DeepSeek-Specific\n",
                "- Follow numbered step sequences strictly.\n",
                "- If you find yourself repeating the same tool call, STOP and try a different approach.\n",
                "- State your plan before each tool call.\n",
                "- RAG workflow: 1. kb_search_context(query) 2. Read top excerpt 3. If insufficient, kb_get(id) 4. Synthesize answer.\n",
                "</provider-hints>\n",
            )),
            Self::OpenAi => Some(concat!(
                "\n<provider-hints>\n",
                "## OpenAI-Specific\n",
                "- Use kb_search_context for architecture questions — do not skip available tools.\n",
                "</provider-hints>\n",
            )),
            Self::Claude => None, // Primary target
            Self::Unknown => None,
        }
    }
}

/// Conservative default context window for unknown models.
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;
/// Conservative default max rounds for unknown models.
pub const DEFAULT_MAX_ROUNDS: usize = 50;

/// Model limit table: (prefix, context_window, max_rounds, tier)
/// Order matters — longer prefixes first.
const TABLE: &[(&str, u64, usize, ModelTier)] = &[
    // ---- Anthropic (Claude) ----
    // Anthropic enforced tool loop pauses at ~20, but the API supports
    // more if resumed. We set 50 as a reasonable "deep task" bound.
    ("claude-opus-4", 200_000, 50, ModelTier::Full),
    ("claude-sonnet-4", 200_000, 50, ModelTier::Full),
    ("claude-haiku-4", 200_000, 50, ModelTier::Compact),
    ("claude-3-5-sonnet", 200_000, 50, ModelTier::Full),
    ("claude-3-5-haiku", 200_000, 50, ModelTier::Compact),
    ("claude-3-opus", 200_000, 50, ModelTier::Full),
    // ---- OpenAI ----
    // OpenAI is primarily context-limited, but 100 is a safe upper bound
    // to prevent infinite loops / token drains.
    ("gpt-4o-mini", 128_000, 30, ModelTier::Compact),
    ("gpt-4o", 128_000, 100, ModelTier::Full),
    ("gpt-4-turbo", 128_000, 100, ModelTier::Full),
    ("gpt-4", 8_192, 50, ModelTier::Full),
    ("o1-mini", 128_000, 100, ModelTier::Compact),
    ("o1", 200_000, 100, ModelTier::Full),
    // ---- Google (Gemini) ----
    // Gemini agent loops are typically optimized for ~50 rounds.
    ("gemini-2.5-flash-lite", 1_000_000, 50, ModelTier::Compact),
    ("gemini-2.5-pro", 1_000_000, 50, ModelTier::Full),
    ("gemini-2.5-flash", 1_000_000, 50, ModelTier::Compact),
    // ---- DeepSeek ----
    // DeepSeek is often used for heavy reasoning/tool-chains.
    // 50 rounds matches Claude/Gemini — 25 was too low for self-test (~35 calls).
    ("deepseek-reasoner", 64_000, 50, ModelTier::Compact),
    ("deepseek-chat", 64_000, 50, ModelTier::Compact),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_sonnet_limits() {
        let l = lookup("claude-sonnet-4-5");
        assert_eq!(l.context_window, 200_000);
        assert_eq!(l.max_rounds, 50);
        assert_eq!(l.tier, ModelTier::Full);
    }

    #[test]
    fn deepseek_chat_limits() {
        let l = lookup("deepseek-chat");
        assert_eq!(l.context_window, 64_000);
        assert_eq!(l.max_rounds, 50);
        assert_eq!(l.tier, ModelTier::Compact);
    }

    #[test]
    fn gemini_pro_limits() {
        let l = lookup("gemini-2.5-pro");
        assert_eq!(l.context_window, 1_000_000);
        assert_eq!(l.max_rounds, 50);
        assert_eq!(l.tier, ModelTier::Full);
    }

    #[test]
    fn gpt4o_mini_limits() {
        let l = lookup("gpt-4o-mini");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.max_rounds, 30);
        assert_eq!(l.tier, ModelTier::Compact);
    }

    #[test]
    fn unknown_defaults() {
        let l = lookup("llama3");
        assert_eq!(l.context_window, DEFAULT_CONTEXT_WINDOW);
        assert_eq!(l.max_rounds, DEFAULT_MAX_ROUNDS);
        assert_eq!(l.tier, ModelTier::Compact);
    }

    #[test]
    fn tier_classification() {
        assert_eq!(tier("claude-opus-4-6"), ModelTier::Full);
        assert_eq!(tier("claude-sonnet-4-5"), ModelTier::Full);
        assert_eq!(tier("claude-haiku-4-5"), ModelTier::Compact);
        assert_eq!(tier("deepseek-chat"), ModelTier::Compact);
        assert_eq!(tier("gpt-4o"), ModelTier::Full);
        assert_eq!(tier("gpt-4o-mini"), ModelTier::Compact);
        assert_eq!(tier("gemini-2.5-pro-latest"), ModelTier::Full);
        assert_eq!(tier("gemini-2.5-flash"), ModelTier::Compact);
        assert_eq!(tier("unknown-model"), ModelTier::Compact);
    }

    #[test]
    fn provider_hint_detection() {
        assert_eq!(
            ProviderHint::from_model("claude-opus-4-6"),
            ProviderHint::Claude
        );
        assert_eq!(ProviderHint::from_model("gpt-4o"), ProviderHint::OpenAi);
        assert_eq!(ProviderHint::from_model("o1-mini"), ProviderHint::OpenAi);
        assert_eq!(
            ProviderHint::from_model("gemini-2.5-pro"),
            ProviderHint::Gemini
        );
        assert_eq!(
            ProviderHint::from_model("deepseek-chat"),
            ProviderHint::DeepSeek
        );
        assert_eq!(ProviderHint::from_model("llama3"), ProviderHint::Unknown);
    }

    #[test]
    fn provider_hints_only_for_non_claude() {
        assert!(ProviderHint::Claude.prompt_hints().is_none());
        assert!(ProviderHint::OpenAi.prompt_hints().is_some());
        assert!(ProviderHint::Gemini.prompt_hints().is_some());
        assert!(ProviderHint::DeepSeek.prompt_hints().is_some());
        assert!(ProviderHint::Unknown.prompt_hints().is_none());
    }

    #[test]
    fn gemini_hints_contain_provider_tag() {
        let hints = ProviderHint::Gemini.prompt_hints().unwrap();
        assert!(hints.contains("<provider-hints>"));
        assert!(hints.contains("Gemini"));
    }

    #[test]
    fn model_tier_parse_tier_round_trip() {
        assert_eq!(ModelTier::parse_tier("full"), ModelTier::Full);
        assert_eq!(ModelTier::parse_tier("Full"), ModelTier::Full);
        assert_eq!(ModelTier::parse_tier("FULL"), ModelTier::Full);
        assert_eq!(ModelTier::parse_tier("compact"), ModelTier::Compact);
        assert_eq!(ModelTier::parse_tier("bogus"), ModelTier::Compact);
        assert_eq!(ModelTier::Full.as_str(), "full");
        assert_eq!(ModelTier::Compact.as_str(), "compact");
    }
}
