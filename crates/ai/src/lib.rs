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
pub mod shell_policy;
pub mod token_estimate;
mod tool_impls;
pub mod tools;
pub mod types;
pub mod web;

pub use claude::ClaudeProvider;
pub use connectivity::ConnectivityResult;
pub use context_limits::{lookup as lookup_context_limit, ModelVerification};
pub use executor::{
    execute_tool, execute_tool_with_requester, ApprovalRequest, DeferredKind, ExecuteResult,
};
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
pub use tool_impls::{
    execute_audit_configuration, execute_kb_export_guidance, execute_kb_export_subgraph_html,
};
// The LSP/DAP tool implementations, re-exported at the crate root so
// `mae-scheme`'s `lsp-*`/`dap-*` primitives drive the SAME code path the
// equivalent MCP tools do rather than re-implementing an LSP/DAP read path a
// second time (CLAUDE.md principles #3 + #15; same precedent as
// `execute_kb_export_subgraph_html` above).
pub use tool_impls::{
    execute_dap_continue, execute_dap_inspect_variable, execute_dap_set_breakpoint,
    execute_dap_start, execute_dap_step, execute_debug_state, execute_lsp_definition,
    execute_lsp_diagnostics, execute_lsp_document_symbols, execute_lsp_hover,
    execute_lsp_references, execute_lsp_workspace_symbol,
};
pub use tools::{
    ai_specific_tools, annotations_for_tier, ask_denied_message, ask_message,
    classify_command_permission, classify_tool_tier, deny_message, external_discovery_tools,
    is_embedded_session_only, parse_categories, request_tools_definition,
    scheme_tools_to_definitions, tools_from_registry, Decision, DenyReason, HardCeiling,
    HardCeilingSource, PermissionPolicy, ToolCategory, ToolTier, EMBEDDED_SESSION_ONLY_TOOLS,
    SESSION_SCOPED_DISPATCHABLE_TOOLS,
};
pub use types::*;
