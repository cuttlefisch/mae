//! mae-ai: AI agent integration — tool-calling transport, provider adapters, session management.
//!
//! @stability: stable
//! @since: 0.3.0

pub mod claude;
pub mod connectivity;
pub mod context_limits;
pub mod executor;
pub mod gemini;
pub mod guardrail;
pub mod guidance;
pub mod ollama;
pub mod openai;
pub mod pricing;
pub mod provider;
pub mod session;
pub mod token_estimate;
mod tool_impls;
pub mod tools;
pub mod types;

pub use claude::ClaudeProvider;
pub use connectivity::ConnectivityResult;
pub use context_limits::{lookup as lookup_context_limit, ModelVerification};
pub use executor::{execute_tool, execute_tool_with_requester, DeferredKind, ExecuteResult};
pub use gemini::GeminiProvider;
pub use guardrail::{GuardrailProvider, StagePolicy, ToolStage};
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;
pub use pricing::{lookup as lookup_price, ModelPrice};
pub use provider::{
    AgentProvider, BudgetConfig, ErrorKind, ProviderConfig, ProviderError, ProviderResponse,
    StopReason, Usage,
};
pub use session::AgentSession;
pub use tool_impls::execute_audit_configuration;
pub use tools::{
    ai_specific_tools, classify_command_permission, classify_tool_tier,
    scheme_tools_to_definitions, tools_from_registry, PermissionPolicy, ToolCategory, ToolTier,
};
pub use types::*;
