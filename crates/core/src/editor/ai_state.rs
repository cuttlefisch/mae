//! AI session state extracted from Editor.
//! All fields were previously `ai_*` on Editor; now accessed via `editor.ai.*`.
//! User-facing option names (e.g. "ai_provider") are unchanged — only Rust
//! field access changes.

use crate::driven_window::DrivenWindow;
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

/// Maximum number of distinct MCP sessions' companion-window state
/// (`AiState::mcp_sessions`) tracked at once (ADR-051, extended by ADR-091
/// to bound the per-session agent state stored in the same record). This is a
/// coarse size bound, not an LRU: once exceeded, an arbitrary entry is
/// evicted to make room. Eviction is always safe -- `DrivenWindow::get_valid`
/// treats a missing/stale entry the same as "no window yet" and simply
/// re-creates one on that session's next dispatch, so an evicted-but-still-
/// connected session at worst gets one extra window instead of reusing its
/// old one. Without this cap, a long-running headless instance (ADR-055)
/// serving many short-lived reconnecting clients (e.g. repeated VS Code
/// sessions over days/weeks) would grow this map without bound, since
/// `ClientSession::id` is monotonically increasing per server lifetime and
/// nothing here observes session disconnects. 256 matches
/// `collab.max_connections`'s default (ADR-054) -- not load-bearing, just a
/// consistent order-of-magnitude default for "how many sessions could
/// plausibly be live/recently-live at once."
pub const MAX_TRACKED_MCP_SESSION_WINDOWS: usize = 256;

/// Per-MCP-session companion-window state (ADR-051), keyed by
/// `shared::mcp::session::ClientSession::id`. Mirrors the single process-wide
/// `work_window`/`target_window_id` pair on `AiState` below, but scoped to
/// one connected MCP client so concurrent clients (a human's own tooling
/// plus e.g. VS Code Copilot) never observe or steal each other's companion
/// window.
#[derive(Debug, Clone, Copy, Default)]
pub struct McpSessionWindowState {
    pub work_window: DrivenWindow,
    pub target_window_id: Option<WindowId>,
}

/// Session-scoped agent state (ADR-091): the state the six session-scoped AI
/// tools — `ai_set_mode`, `ai_set_profile`, `ai_set_budget`, `log_activity`,
/// `read_transcript` (and `web_fetch`, which needs none of it) — read and
/// mutate.
///
/// Before ADR-091 these fields existed only as `AgentSession`'s own private
/// fields (`self.current_mode`, `self.budget`, `self.transcript_path`, …) on
/// the embedded agent's tokio task, which `dispatch_tool` structurally cannot
/// see. That is *why* the tools were advertised over MCP and yet fell through
/// to `Unknown tool`. Lifting them here — reachable from the dispatch path,
/// resolved per MCP session — is what makes them genuinely dispatchable.
///
/// Two instances exist per resolution (see `Editor::agent_session`):
/// - one per connected MCP session, in [`McpSessionState`], so two external
///   agents never observe or clobber each other's mode/budget/activity, and
/// - one process-wide, on [`AiState::agent_session`], used when no MCP
///   session is in scope. That one is the *embedded* agent's, so the
///   mode/profile accessors additionally write through to the editor's
///   `ai_mode`/`ai_profile` options — the same effect
///   `AiEvent::UpdateMode`/`UpdateProfile` already produce for the embedded
///   session. An MCP call must not be a weaker version of the same tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentSessionState {
    /// Operating mode: standard / plan / auto-accept. Empty until set, in
    /// which case readers fall back to the editor-global `ai.mode`.
    pub mode: Option<String>,
    /// Prompt profile (pair-programmer / explorer / …). Empty until set.
    pub profile: Option<String>,
    /// Soft budget warning threshold, USD. `None` = no warning.
    pub budget_warn_usd: Option<f64>,
    /// Hard budget cap, USD. `None` = uncapped.
    pub budget_cap_usd: Option<f64>,
    /// Path to this session's transcript log, when one is being written.
    pub transcript_path: Option<std::path::PathBuf>,
    /// Reasoning steps the agent has narrated via `log_activity`, most recent
    /// last. Bounded by [`MAX_SESSION_ACTIVITY_ENTRIES`] — an agent that
    /// narrates every round would otherwise grow this without bound on a
    /// long-lived headless instance (ADR-055).
    pub activity: Vec<String>,
}

/// Upper bound on [`AgentSessionState::activity`]. Oldest entries are dropped
/// first. 200 is display-shaped, not protocol-shaped: it is far more than any
/// human reads back and small enough that 256 tracked sessions at their cap
/// stay well under a megabyte of narration.
pub const MAX_SESSION_ACTIVITY_ENTRIES: usize = 200;

impl AgentSessionState {
    /// Append a narrated reasoning step, dropping the oldest when at capacity.
    pub fn push_activity(&mut self, entry: impl Into<String>) {
        if self.activity.len() >= MAX_SESSION_ACTIVITY_ENTRIES {
            self.activity.remove(0);
        }
        self.activity.push(entry.into());
    }
}

/// Everything MAE tracks for one connected MCP session, keyed by
/// `shared::mcp::session::ClientSession::id`.
///
/// One record per session rather than one map per concern: the eviction bound
/// ([`MAX_TRACKED_MCP_SESSION_WINDOWS`]), the lazy-population rule, and the
/// lifetime are all identical, so splitting them would be two things to keep
/// in sync (principle #8).
#[derive(Debug, Clone, Default)]
pub struct McpSessionState {
    /// ADR-051's companion-window isolation.
    pub windows: McpSessionWindowState,
    /// ADR-091's session-scoped agent state.
    pub agent: AgentSessionState,
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
    /// Dedicated window this AI/MCP actor is driving — reused across a
    /// sequence of agent-triggered display calls (open_file, KB node
    /// display, etc.) regardless of the displayed content's `BufferKind`.
    /// See `crate::driven_window::DrivenWindow` for the shared primitive.
    /// Since issue #372, this is also established proactively (not just
    /// reused) by `Editor::ensure_ai_dispatch_target`/`with_ai_dispatch_scope`
    /// — the enforced default for MCP/AI dispatch, so a companion window
    /// exists before a command runs, not only after a call site that
    /// happens to know how to ask for one.
    pub work_window: DrivenWindow,
    /// Per-MCP-session companion-window state (ADR-051), keyed by MCP
    /// `ClientSession::id`. Populated lazily, on that session's first
    /// dispatch through `Editor::with_ai_dispatch_scope_for_session`. The
    /// `work_window`/`target_window_id` fields above remain the fallback
    /// used when dispatching with no session id (the interactive human's own
    /// embedded AI path, `--self-test`, and any other caller that predates
    /// per-session dispatch) -- their single-session behavior is completely
    /// unaffected by this map. See `MAX_TRACKED_MCP_SESSION_WINDOWS` for the
    /// growth bound.
    pub mcp_sessions: std::collections::HashMap<u64, McpSessionState>,
    /// The MCP session id in scope for the dispatch currently running, or
    /// `None` when the running dispatch has no MCP session (the embedded
    /// human AI path, `--self-test`, a human keybinding).
    ///
    /// Maintained by `Editor::with_ai_dispatch_scope_for_session` the same way
    /// `ai_dispatch_depth` is — saved before the body, restored after — so a
    /// tool implementation reached from that scope can resolve *whose* session
    /// state it is acting on without every dispatcher in the chain threading a
    /// parameter it does not otherwise care about (ADR-091).
    ///
    /// @ai-caution: [dispatch] This is identity/routing, not authority. It
    /// answers "which session is calling", never "may it do this" — the tier
    /// and category gates in `execute_tool_dispatch_body` answer that, and
    /// nothing here may be used to widen them.
    pub dispatch_session_id: Option<u64>,
    /// Process-wide agent session state — the record used when
    /// `dispatch_session_id` is `None`. See [`AgentSessionState`].
    pub agent_session: AgentSessionState,
    /// Nesting depth of the current AI-originated dispatch, or 0 when the
    /// running operation originated from a human.
    ///
    /// This is the minimum viable form of ADR-088's carried authority: rather
    /// than asking "what tier is this session?" (ambient), effects can ask
    /// "did a human ask for this?" — the question the confused-deputy problem
    /// actually turns on. Maintained by
    /// `Editor::with_ai_dispatch_scope_for_session`, which already wraps every
    /// MCP-originated dispatch, and read via `Editor::is_ai_originated_dispatch`.
    ///
    /// A depth counter rather than a bool because dispatch nests (a tool that
    /// runs a command that dispatches another); a bool would be cleared by the
    /// inner scope's exit while the outer one is still running.
    ///
    /// @ai-caution: [security] Load-bearing for ADR-089 D4 — it is what lets
    /// `save_buffer_*` refuse to write MAE's own config for an agent while
    /// leaving the human's `:w` and `:set-save` untouched. Anything that sets
    /// this to 0 inside an AI dispatch re-opens that path.
    pub ai_dispatch_depth: u32,
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
    /// ADR-061 Phase E: KB enrichment embedding provider (only "ollama" is
    /// supported today).
    pub embedding_provider: String,
    /// ADR-061 Phase E: embedding model name.
    pub embedding_model: String,
    /// ADR-061 Phase E: base URL override for the embedding provider's API.
    pub embedding_base_url: String,
    /// ADR-061 Phase E: shell command whose stdout is the embedding
    /// provider's API key. Empty = none.
    pub embedding_api_key_command: String,
    /// ADR-061 Phase E: ADR-031 cache-key third component. Must match the
    /// daemon's own `[enrichment] chunk_version` to share cache entries with
    /// its background sweep.
    pub embedding_chunk_version: i64,
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
            work_window: DrivenWindow::none(),
            mcp_sessions: std::collections::HashMap::new(),
            dispatch_session_id: None,
            agent_session: AgentSessionState::default(),
            ai_dispatch_depth: 0,
            editor_name: "mae-agent".to_string(),
            agent_login_shell: true,
            provider: String::new(),
            model: String::new(),
            api_key_command: String::new(),
            base_url: String::new(),
            mode: "standard".to_string(),
            thinking: String::new(),
            embedding_provider: "ollama".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            embedding_base_url: String::new(),
            embedding_api_key_command: String::new(),
            embedding_chunk_version: 1,
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
