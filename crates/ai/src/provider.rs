use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::types::*;

/// Error type for provider operations.
#[derive(Debug)]
pub struct ProviderError {
    pub message: String,
    pub retryable: bool,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ProviderError {}

/// Configuration for an agent provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub provider_type: String,
    pub api_key: Option<String>,
    pub model: String,
    pub base_url: Option<String>,
    pub max_tokens: u32,
    pub temperature: Option<f64>,
    /// HTTP request timeout in seconds. Defaults to 300 (5 min) for slow
    /// local inference (Ollama on CPU can take 60-120+ seconds per turn).
    pub timeout_secs: u64,
    /// Per-session spend guardrails (USD). Empty = unbounded.
    #[serde(default)]
    pub budget: BudgetConfig,
}

/// Per-session cost guardrails. A MAE session is a single run of the
/// editor process; both fields accumulate across every turn in that run.
///
/// # Design
/// Unknown-priced models (e.g. Ollama-hosted local models) are treated
/// as free — a zero-cost pass-through. The hard cap therefore only ever
/// fires against priced providers, which is exactly what we want: a
/// local FOSS contributor running `llama3` should never see a budget
/// rejection, while a paid-API user always gets the circuit breaker.
///
/// # Why session-level and not per-request
/// Per-request caps (`max_tokens`) already exist on `ProviderConfig` and
/// protect against single runaway responses. The *session* cap is what
/// protects against the far more common footgun: a correctly-bounded
/// loop that still rings up $5 over fifty tool-call rounds because each
/// round is cheap in isolation. Cost creeps at the session level.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Emit a one-shot warning event the first time cumulative session
    /// cost crosses this value. `None` = never warn.
    #[serde(default)]
    pub session_warn_usd: Option<f64>,
    /// Reject new requests once cumulative session cost reaches this
    /// value. `None` = unbounded. The rejection surfaces as an
    /// `AiEvent::Error` with a clear "budget exceeded" message so
    /// users can act on it.
    #[serde(default)]
    pub session_hard_cap_usd: Option<f64>,
}

/// Agent provider trait — the abstraction that makes MAE agent-agnostic.
///
/// Implementations handle:
/// - Serializing tools + messages into provider-specific JSON
/// - Making HTTP requests (or stdio for local models)
/// - Parsing responses back into canonical types
///
/// This runs on a tokio task (Send + Sync required).
///
/// Emacs lesson: design the abstraction for multiple backends from day one.
/// VSCode Copilot was tightly coupled to OpenAI and required major refactoring
/// when alternatives appeared. We support Claude + OpenAI from the start.
#[async_trait]
pub trait AgentProvider: Send + Sync {
    /// Send a conversation turn and get back a response.
    /// The response may contain tool calls that need execution.
    async fn send(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        system_prompt: &str,
    ) -> Result<ProviderResponse, ProviderError>;

    /// Provider name for display and logging.
    fn name(&self) -> &str;
}

/// Response from a provider — text, tool calls, or both.
#[derive(Debug, Clone)]
pub struct ProviderResponse {
    pub text: Option<String>,
    pub tool_calls: Vec<ToolCall>,
    pub stop_reason: StopReason,
    /// Token usage reported by the provider. `None` when the provider
    /// didn't include a `usage` field (e.g. some Ollama builds).
    pub usage: Option<Usage>,
}

/// Raw token counts reported by the provider. Used by the session
/// budget tracker together with the static pricing table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// Model finished its response naturally.
    EndTurn,
    /// Model wants tool results before continuing.
    ToolUse,
    /// Max tokens reached (truncated).
    MaxTokens,
}
