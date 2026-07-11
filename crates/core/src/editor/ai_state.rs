//! AI session state extracted from Editor.
//! All fields were previously `ai_*` on Editor; now accessed via `editor.ai.*`.
//! User-facing option names (e.g. "ai_provider") are unchanged — only Rust
//! field access changes.

use crate::window::WindowId;
use crate::SchemeToolDef;

use super::ConversationPair;

/// Input lock scope — controls what keyboard input is allowed during AI/MCP operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputLock {
    /// No lock — all input accepted normally.
    None,
    /// AI session active — block editor commands but allow shell input and navigation.
    AiBusy,
    /// MCP tool executing — block editor commands but allow shell input and navigation.
    McpBusy,
}

/// Network connectivity check result (lightweight copy for display/reporting).
#[derive(Debug, Clone)]
pub struct AiNetworkCheck {
    pub endpoint: String,
    pub reachable: bool,
    pub http_status: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// AI session state: provider config, token counters, streaming flags,
/// conversation pair, permission tier, and target context.
#[derive(Debug)]
pub struct AiState {
    /// Running AI session spend in USD.
    pub session_cost_usd: f64,
    /// Cumulative prompt tokens this session.
    pub session_tokens_in: u64,
    /// Cumulative completion tokens this session.
    pub session_tokens_out: u64,
    /// Cumulative cache read tokens.
    pub cache_read_tokens: u64,
    /// Cumulative cache creation tokens.
    pub cache_creation_tokens: u64,
    /// Model's context window size in tokens.
    pub context_window: u64,
    /// Estimated tokens currently used in context.
    pub context_used_tokens: u64,
    /// Timestamp of the last successful AI API call.
    pub last_api_success: Option<std::time::Instant>,
    /// Last AI API error message.
    pub last_api_error: Option<String>,
    /// Latency of the last AI API call in milliseconds.
    pub last_api_latency_ms: Option<u64>,
    /// Total number of AI API calls this session.
    pub api_call_count: u64,
    /// Last network connectivity check result.
    pub last_network_check: Option<AiNetworkCheck>,
    /// Throttle for AI output scroll during streaming.
    pub last_output_scroll: Option<std::time::Instant>,
    /// Dedicated window for AI file operations.
    pub work_window_id: Option<WindowId>,
    /// AI editor/agent command (e.g. "claude", "aider").
    pub editor_name: String,
    /// Whether `open-ai-agent`'s shell wraps `editor_name` through the
    /// user's login+interactive shell (sourcing `.bashrc`/`.zshrc` etc.) so
    /// it inherits the same environment a normal terminal would — auth
    /// tokens, PATH shims. Disable if a slow/side-effecting shell rc delays
    /// agent launch.
    pub agent_login_shell: bool,
    /// AI provider name: "claude", "openai", "gemini", "ollama", "deepseek".
    pub provider: String,
    /// AI model identifier. Empty = use provider default.
    pub model: String,
    /// Shell command whose stdout is the API key.
    pub api_key_command: String,
    /// Base URL override for the AI API.
    pub base_url: String,
    /// AI operating mode (standard, auto-accept, plan).
    pub mode: String,
    /// Reasoning/thinking mode override for supported providers:
    /// "true", "false", "high", "medium", "low". Empty = provider default.
    pub thinking: String,
    /// Active prompt profile name.
    pub profile: String,
    /// True while the AI session is actively streaming.
    pub streaming: bool,
    /// Set to true when the user requests AI cancellation.
    pub cancel_requested: bool,
    /// Current round in the AI tool loop.
    pub current_round: usize,
    /// Current transaction start index in history.
    pub transaction_start_idx: Option<usize>,
    /// AI's target buffer context.
    pub target_buffer_idx: Option<usize>,
    /// AI's target window context.
    pub target_window_id: Option<WindowId>,
    /// Current AI permission tier label.
    pub permission_tier: String,
    /// Whether an AI provider was successfully configured at startup.
    pub configured: bool,
    /// Linked output+input buffer pair for split-view conversation UI.
    pub conversation_pair: Option<ConversationPair>,
    /// Controls what keyboard input is allowed during AI/MCP operations.
    pub input_lock: InputLock,
    /// Pending agent setup request.
    pub pending_agent_setup: Option<String>,
    /// Last time the Escape key was pressed (for double-esc detection).
    pub last_esc_time: Option<std::time::Instant>,
    /// Scheme-registered AI tools.
    pub scheme_tools: Vec<SchemeToolDef>,
}

impl AiState {
    pub fn new() -> Self {
        Self {
            session_cost_usd: 0.0,
            session_tokens_in: 0,
            session_tokens_out: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            context_window: 0,
            context_used_tokens: 0,
            last_api_success: None,
            last_api_error: None,
            last_api_latency_ms: None,
            api_call_count: 0,
            last_network_check: None,
            last_output_scroll: None,
            work_window_id: None,
            editor_name: "mae-agent".to_string(),
            agent_login_shell: true,
            provider: String::new(),
            model: String::new(),
            api_key_command: String::new(),
            base_url: String::new(),
            mode: "standard".to_string(),
            thinking: String::new(),
            profile: "pair-programmer".to_string(),
            streaming: false,
            cancel_requested: false,
            current_round: 0,
            transaction_start_idx: None,
            target_buffer_idx: None,
            target_window_id: None,
            permission_tier: "ReadOnly".to_string(),
            configured: false,
            conversation_pair: None,
            input_lock: InputLock::None,
            pending_agent_setup: None,
            last_esc_time: None,
            scheme_tools: Vec::new(),
        }
    }
}

impl Default for AiState {
    fn default() -> Self {
        Self::new()
    }
}
