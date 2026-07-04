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

/// End-to-end verification status for a model with MAE's tool-calling flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelVerification {
    /// End-to-end tested with self-test suite and production use.
    Verified,
    /// Basic testing done, may have edge cases.
    Testing,
    /// Model entry exists but not tested with MAE. May have issues.
    Untested,
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
    /// Whether this model has been tested with MAE.
    pub verification: ModelVerification,
}

impl ModelLimits {
    /// Memory budget: 2% of context_window, clamped [256, 4096] tokens.
    pub fn memory_budget_tokens(&self) -> u64 {
        (self.context_window / 50).clamp(256, 4096)
    }

    /// Approximate character budget (tokens × 4).
    pub fn memory_budget_chars(&self) -> usize {
        (self.memory_budget_tokens() * 4) as usize
    }
}

impl Default for ModelLimits {
    fn default() -> Self {
        ModelLimits {
            context_window: DEFAULT_CONTEXT_WINDOW,
            max_rounds: DEFAULT_MAX_ROUNDS,
            tier: ModelTier::Compact,
            verification: ModelVerification::Untested,
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
                verification: verification_status(prefix),
            };
        }
    }
    ModelLimits::default()
}

/// Determine verification status based on model prefix.
fn verification_status(prefix: &str) -> ModelVerification {
    match prefix {
        // End-to-end tested with self-test suite and production use.
        "claude-opus-4" | "claude-opus-4-6" | "claude-opus-4-7" | "claude-opus-4-8"
        | "claude-sonnet-4" | "claude-sonnet-4-6" | "deepseek-chat" | "deepseek-reasoner" => {
            ModelVerification::Verified
        }
        // Basic testing done, may have edge cases.
        "claude-fable-5"
        | "gemini-2.5-pro"
        | "gemini-2.5-flash"
        | "gemini-2.5-flash-lite"
        | "gemini-2.0-flash"
        | "gpt-4o"
        | "gpt-4o-mini"
        | "claude-3-5-sonnet"
        | "claude-3-5-haiku" => ModelVerification::Testing,
        // Everything else: model entry exists but not tested.
        _ => ModelVerification::Untested,
    }
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
    Qwen,
    Mistral,
    /// Local models via Ollama/etc (Llama, Phi, Command-R).
    Local,
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
        } else if lower.starts_with("qwen") {
            Self::Qwen
        } else if lower.starts_with("mistral") || lower.starts_with("codestral") {
            Self::Mistral
        } else if lower.starts_with("llama")
            || lower.starts_with("phi")
            || lower.starts_with("command-r")
        {
            Self::Local
        } else {
            Self::Unknown
        }
    }

    /// Default API endpoint URL for this provider (for connectivity checks).
    pub fn default_endpoint(&self) -> Option<&'static str> {
        match self {
            Self::Claude => Some("https://api.anthropic.com"),
            Self::OpenAi => Some("https://api.openai.com"),
            Self::Gemini => Some("https://generativelanguage.googleapis.com"),
            Self::DeepSeek => Some("https://api.deepseek.com"),
            Self::Qwen => Some("https://dashscope.aliyuncs.com"),
            Self::Mistral => Some("https://api.mistral.ai"),
            Self::Local | Self::Unknown => None,
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
            Self::Qwen => Some(concat!(
                "\n<provider-hints>\n",
                "## Qwen-Specific\n",
                "- Qwen3 supports native tool calling via the OpenAI-compatible API.\n",
                "- Prefer single tool calls per turn — parallel calling is supported but less reliable in smaller variants.\n",
                "- State your plan before each tool call.\n",
                "- Use explicit JSON for complex arguments.\n",
                "</provider-hints>\n",
            )),
            Self::Mistral => Some(concat!(
                "\n<provider-hints>\n",
                "## Mistral-Specific\n",
                "- Mistral models support native function calling.\n",
                "- Use explicit JSON for tool arguments.\n",
                "- Prefer project_search when LSP is slow.\n",
                "- Single tool calls per turn for reliability.\n",
                "</provider-hints>\n",
            )),
            Self::Local => Some(concat!(
                "\n<provider-hints>\n",
                "## Local Model Hints\n",
                "- Prefer small, targeted tool calls — large payloads may be slow.\n",
                "- Stop and summarize if you find yourself looping.\n",
                "- Response times may be slow on local hardware — that is normal.\n",
                "- State your plan before each tool call.\n",
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
    // Longer prefixes first: 4.6+ Opus and Sonnet 4.6 ship a 1M context
    // window; older 4.0/4.1/4.5 revisions stay at 200K.
    ("claude-fable-5", 1_000_000, 50, ModelTier::Full),
    ("claude-opus-4-8", 1_000_000, 50, ModelTier::Full),
    ("claude-opus-4-7", 1_000_000, 50, ModelTier::Full),
    ("claude-opus-4-6", 1_000_000, 50, ModelTier::Full),
    ("claude-opus-4", 200_000, 50, ModelTier::Full),
    ("claude-sonnet-4-6", 1_000_000, 50, ModelTier::Full),
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
    ("gemini-2.0-flash", 1_000_000, 50, ModelTier::Compact),
    // ---- DeepSeek ----
    // DeepSeek is often used for heavy reasoning/tool-chains.
    // 50 rounds matches Claude/Gemini — 25 was too low for self-test (~35 calls).
    ("deepseek-reasoner", 64_000, 50, ModelTier::Compact),
    ("deepseek-chat", 64_000, 50, ModelTier::Compact),
    // ---- Qwen (Alibaba) ----
    // Qwen3 supports parallel tool calling natively.
    // Context windows match published specs (qwen3 technical report).
    ("qwen3-235b", 128_000, 50, ModelTier::Full),
    ("qwen3-30b", 128_000, 50, ModelTier::Compact),
    ("qwen3", 128_000, 50, ModelTier::Compact),
    ("qwen2.5-coder", 128_000, 50, ModelTier::Compact),
    ("qwen2.5", 128_000, 50, ModelTier::Compact),
    // ---- Meta (Llama) ----
    ("llama4", 128_000, 50, ModelTier::Full),
    ("llama3.3", 128_000, 50, ModelTier::Full),
    ("llama3.1", 128_000, 50, ModelTier::Compact),
    ("llama3", 8_192, 30, ModelTier::Compact),
    // ---- Mistral ----
    ("mistral-large", 128_000, 50, ModelTier::Full),
    ("mistral-small", 32_000, 30, ModelTier::Compact),
    ("codestral", 32_000, 50, ModelTier::Compact),
    ("mistral", 32_000, 30, ModelTier::Compact),
    // ---- Microsoft ----
    ("phi-4", 16_384, 30, ModelTier::Compact),
    // ---- Cohere ----
    ("command-r-plus", 128_000, 50, ModelTier::Full),
    ("command-r", 128_000, 30, ModelTier::Compact),
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
    fn latest_claude_models_have_1m_context() {
        // 4.6+ Opus and Sonnet 4.6 ship a 1M window; a dated revision must
        // still hit the family prefix.
        assert_eq!(lookup("claude-opus-4-8").context_window, 1_000_000);
        assert_eq!(lookup("claude-opus-4-7").context_window, 1_000_000);
        assert_eq!(lookup("claude-opus-4-6").context_window, 1_000_000);
        assert_eq!(
            lookup("claude-sonnet-4-6-20251114").context_window,
            1_000_000
        );
        assert_eq!(lookup("claude-fable-5").context_window, 1_000_000);
        // Older Opus/Sonnet revisions keep the 200K window.
        assert_eq!(lookup("claude-opus-4-5").context_window, 200_000);
        assert_eq!(lookup("claude-sonnet-4-5").context_window, 200_000);
    }

    #[test]
    fn gemini_2_0_flash_has_limits() {
        // Previously fell through to the 128K DEFAULT (~8x under-report).
        let l = lookup("gemini-2.0-flash");
        assert_eq!(l.context_window, 1_000_000);
        assert_eq!(l.tier, ModelTier::Compact);
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
        let l = lookup("some-random-model");
        assert_eq!(l.context_window, DEFAULT_CONTEXT_WINDOW);
        assert_eq!(l.max_rounds, DEFAULT_MAX_ROUNDS);
        assert_eq!(l.tier, ModelTier::Compact);
        assert_eq!(l.verification, ModelVerification::Untested);
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
        assert_eq!(ProviderHint::from_model("qwen3-235b"), ProviderHint::Qwen);
        assert_eq!(
            ProviderHint::from_model("qwen2.5-coder"),
            ProviderHint::Qwen
        );
        assert_eq!(
            ProviderHint::from_model("mistral-large"),
            ProviderHint::Mistral
        );
        assert_eq!(ProviderHint::from_model("codestral"), ProviderHint::Mistral);
        assert_eq!(ProviderHint::from_model("llama3"), ProviderHint::Local);
        assert_eq!(ProviderHint::from_model("llama4"), ProviderHint::Local);
        assert_eq!(ProviderHint::from_model("phi-4"), ProviderHint::Local);
        assert_eq!(
            ProviderHint::from_model("command-r-plus"),
            ProviderHint::Local
        );
        assert_eq!(
            ProviderHint::from_model("unknown-model"),
            ProviderHint::Unknown
        );
    }

    #[test]
    fn provider_hints_only_for_non_claude() {
        assert!(ProviderHint::Claude.prompt_hints().is_none());
        assert!(ProviderHint::OpenAi.prompt_hints().is_some());
        assert!(ProviderHint::Gemini.prompt_hints().is_some());
        assert!(ProviderHint::DeepSeek.prompt_hints().is_some());
        assert!(ProviderHint::Qwen.prompt_hints().is_some());
        assert!(ProviderHint::Mistral.prompt_hints().is_some());
        assert!(ProviderHint::Local.prompt_hints().is_some());
        assert!(ProviderHint::Unknown.prompt_hints().is_none());
    }

    #[test]
    fn gemini_hints_contain_provider_tag() {
        let hints = ProviderHint::Gemini.prompt_hints().unwrap();
        assert!(hints.contains("<provider-hints>"));
        assert!(hints.contains("Gemini"));
    }

    #[test]
    fn qwen_hints_contain_provider_tag() {
        let hints = ProviderHint::Qwen.prompt_hints().unwrap();
        assert!(hints.contains("<provider-hints>"));
        assert!(hints.contains("Qwen"));
    }

    #[test]
    fn mistral_hints_contain_provider_tag() {
        let hints = ProviderHint::Mistral.prompt_hints().unwrap();
        assert!(hints.contains("<provider-hints>"));
        assert!(hints.contains("Mistral"));
    }

    #[test]
    fn local_hints_contain_provider_tag() {
        let hints = ProviderHint::Local.prompt_hints().unwrap();
        assert!(hints.contains("<provider-hints>"));
        assert!(hints.contains("Local"));
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

    // --- Memory budget tests ---

    #[test]
    fn memory_budget_min_clamp() {
        let l = lookup("llama3-8b"); // 8K context
        assert_eq!(l.memory_budget_tokens(), 256); // 8192/50 = 163 → clamped to 256
    }

    #[test]
    fn memory_budget_max_clamp() {
        let l = lookup("gemini-2.5-pro"); // 1M context
        assert_eq!(l.memory_budget_tokens(), 4096); // 1_000_000/50 = 20_000 → clamped to 4096
    }

    #[test]
    fn memory_budget_normal() {
        let l = lookup("claude-sonnet-4-5"); // 200K context
        assert_eq!(l.memory_budget_tokens(), 4000); // 200_000/50 = 4000
    }

    #[test]
    fn memory_budget_chars() {
        let l = lookup("claude-sonnet-4-5");
        assert_eq!(l.memory_budget_chars(), 16000); // 4000 * 4
    }

    // --- Provider endpoint tests ---

    #[test]
    fn provider_default_endpoints() {
        assert_eq!(
            ProviderHint::Claude.default_endpoint(),
            Some("https://api.anthropic.com")
        );
        assert_eq!(
            ProviderHint::OpenAi.default_endpoint(),
            Some("https://api.openai.com")
        );
        assert_eq!(
            ProviderHint::Gemini.default_endpoint(),
            Some("https://generativelanguage.googleapis.com")
        );
        assert_eq!(
            ProviderHint::DeepSeek.default_endpoint(),
            Some("https://api.deepseek.com")
        );
        assert_eq!(
            ProviderHint::Qwen.default_endpoint(),
            Some("https://dashscope.aliyuncs.com")
        );
        assert_eq!(
            ProviderHint::Mistral.default_endpoint(),
            Some("https://api.mistral.ai")
        );
        assert!(ProviderHint::Local.default_endpoint().is_none());
        assert!(ProviderHint::Unknown.default_endpoint().is_none());
    }

    // --- New model prefix tests ---

    #[test]
    fn qwen3_235b_limits() {
        let l = lookup("qwen3-235b");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.tier, ModelTier::Full);
    }

    #[test]
    fn qwen3_compact_limits() {
        let l = lookup("qwen3-30b");
        assert_eq!(l.tier, ModelTier::Compact);
    }

    #[test]
    fn qwen25_coder_limits() {
        let l = lookup("qwen2.5-coder:7b");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.tier, ModelTier::Compact);
    }

    #[test]
    fn llama4_limits() {
        let l = lookup("llama4-scout");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.tier, ModelTier::Full);
    }

    #[test]
    fn llama33_limits() {
        let l = lookup("llama3.3-70b");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.tier, ModelTier::Full);
    }

    #[test]
    fn llama3_limits() {
        let l = lookup("llama3-8b");
        assert_eq!(l.context_window, 8_192);
        assert_eq!(l.max_rounds, 30);
    }

    #[test]
    fn mistral_large_limits() {
        let l = lookup("mistral-large-latest");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.tier, ModelTier::Full);
    }

    #[test]
    fn codestral_limits() {
        let l = lookup("codestral-latest");
        assert_eq!(l.context_window, 32_000);
        assert_eq!(l.tier, ModelTier::Compact);
    }

    #[test]
    fn phi4_limits() {
        let l = lookup("phi-4");
        assert_eq!(l.context_window, 16_384);
        assert_eq!(l.max_rounds, 30);
    }

    #[test]
    fn command_r_plus_limits() {
        let l = lookup("command-r-plus");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.tier, ModelTier::Full);
    }

    #[test]
    fn command_r_limits() {
        let l = lookup("command-r");
        assert_eq!(l.context_window, 128_000);
        assert_eq!(l.max_rounds, 30);
    }

    // --- Verification status tests ---

    #[test]
    fn verified_models() {
        assert_eq!(
            lookup("claude-opus-4-6").verification,
            ModelVerification::Verified
        );
        assert_eq!(
            lookup("claude-sonnet-4-5").verification,
            ModelVerification::Verified
        );
        assert_eq!(
            lookup("deepseek-chat").verification,
            ModelVerification::Verified
        );
        assert_eq!(
            lookup("deepseek-reasoner").verification,
            ModelVerification::Verified
        );
    }

    #[test]
    fn testing_models() {
        assert_eq!(
            lookup("gemini-2.5-pro").verification,
            ModelVerification::Testing
        );
        assert_eq!(lookup("gpt-4o").verification, ModelVerification::Testing);
        assert_eq!(
            lookup("claude-3-5-sonnet").verification,
            ModelVerification::Testing
        );
    }

    #[test]
    fn untested_models() {
        assert_eq!(
            lookup("qwen3-235b").verification,
            ModelVerification::Untested
        );
        assert_eq!(lookup("llama4").verification, ModelVerification::Untested);
        assert_eq!(
            lookup("mistral-large").verification,
            ModelVerification::Untested
        );
        assert_eq!(lookup("phi-4").verification, ModelVerification::Untested);
        assert_eq!(
            lookup("command-r-plus").verification,
            ModelVerification::Untested
        );
    }

    #[test]
    fn unknown_model_untested() {
        assert_eq!(
            lookup("totally-unknown-model").verification,
            ModelVerification::Untested
        );
    }
}
