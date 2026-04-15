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
