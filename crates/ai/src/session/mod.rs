use crate::provider::*;
use crate::token_estimate;
use crate::types::*;
use std::path::PathBuf;
use tokio::sync::mpsc;

mod context_mgmt;
mod handle_prompt;
pub(crate) mod progress;
mod run_loop;

/// Degradation level for context pressure management.
/// One-way within a session: Normal → ToolsShed → Minimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DegradationLevel {
    /// All tools, full system prompt.
    Normal,
    /// Extended tools removed, core only.
    ToolsShed,
    /// System prompt shortened + tools shed.
    Minimal,
}

#[cfg(test)]
mod tests;

/// AgentSession runs the agentic loop on a spawned tokio task:
///   1. Receive user prompt via channel
///   2. Call provider with conversation history + tools
///   3. For each tool call: send to main thread, await result via oneshot
///   4. Feed tool results back to provider
///   5. Repeat until EndTurn or max rounds
///
/// The session never touches Editor directly — all mutations flow through
/// the main thread's event loop via AiEvent/ToolResult channels.
///
/// Emacs lesson: process.c conflates I/O, lifecycle, and buffering in 7k lines.
/// We separate transport (provider), protocol (types), and orchestration (session).
pub struct AgentSession {
    pub(super) provider: Box<dyn AgentProvider>,
    pub(super) tools: Vec<ToolDefinition>,
    pub(super) messages: Vec<Message>,
    pub(super) system_prompt: String,
    pub(super) event_tx: mpsc::Sender<AiEvent>,
    pub(super) command_rx: mpsc::Receiver<AiCommand>,
    pub(super) max_rounds: usize,
    /// Maximum messages to keep in conversation history.
    /// Older messages are trimmed (keeping the first user message for context).
    pub(super) max_messages: usize,
    /// Consecutive provider error count for circuit breaker.
    pub(super) consecutive_errors: usize,
    /// Cached pricing for the session's model. Resolved once in
    /// `with_budget` so every `update_cost` skips the pricing-table scan
    /// and the `to_ascii_lowercase()` allocation. `None` for unpriced
    /// models (e.g. Ollama) — the tracker treats that as free.
    pub(super) price: Option<crate::pricing::ModelPrice>,
    /// Per-session cost guardrails.
    pub(super) budget: crate::BudgetConfig,
    /// Cumulative USD cost for this session. Zero-initialized on
    /// construction; incremented after every successful round.
    pub(super) session_cost_usd: f64,
    /// Cumulative token counters, forwarded to the editor on every
    /// round so the status line can surface both "dollars" and "tokens"
    /// (the latter matters for Ollama/unpriced models).
    pub(super) session_tokens_in: u64,
    pub(super) session_tokens_out: u64,
    /// Cumulative cache read tokens (prompt cache hits).
    pub(super) session_cache_read: u64,
    /// Cumulative cache creation tokens.
    pub(super) session_cache_creation: u64,
    /// One-shot flag so `BudgetWarning` is emitted at most once per
    /// session. Users don't want a warn per round after crossing the
    /// threshold.
    pub(super) warned: bool,
    /// Model's context window size in tokens (from context_limits table).
    pub(super) context_window: u64,
    /// Cached token estimate for the system prompt (computed once).
    pub(super) system_prompt_tokens: u64,
    /// Cached token estimate for the tool definitions (computed once).
    pub(super) tools_tokens: u64,
    /// Output tokens reserved for the model's response.
    pub(super) reserved_output: u64,
    /// Whether the session initialization message has been emitted.
    pub(super) initialized: bool,
    /// Model name for display purposes.
    pub(super) model_name: String,
    /// All tools (core + extended). Partitioned at construction.
    pub(super) all_tools: Vec<ToolDefinition>,
    /// Categories that have been enabled via `request_tools`.
    pub(super) enabled_categories: std::collections::HashSet<crate::tools::ToolCategory>,
    /// Index in `self.messages` where the current transaction (User prompt) started.
    /// Used for tool stack compression.
    pub(super) transaction_start_idx: Option<usize>,
    /// Current round in the tool loop. Exposed for introspection.
    pub(super) current_round: usize,
    /// Optional name of the buffer to route output to (e.g. "*AI-Explorer*").
    /// If None, output goes to the default conversation buffer.
    pub(super) target_buffer: Option<String>,
    /// Current AI operating mode (standard/plan/auto-accept), injected per-turn.
    pub(super) current_mode: String,
    /// Current AI prompt profile, injected per-turn.
    pub(super) current_profile: String,
    /// Last executed tool calls (for diagnostics).
    pub(super) last_tool_calls: Option<Vec<ToolCall>>,
    /// History of tool call signatures (name:args) for oscillating loop detection.
    pub(super) turn_history: std::collections::VecDeque<String>,
    /// Progress checkpoint tracker for semantic stagnation detection.
    pub(super) progress: progress::ProgressTracker,
    /// Current degradation level for context pressure management.
    pub(super) degradation_level: DegradationLevel,
    /// Original system prompt (preserved for reference after truncation).
    #[allow(dead_code)]
    pub(super) original_system_prompt: String,
    /// Path to the session's auto-saved transcript log.
    pub(super) transcript_path: Option<PathBuf>,
    /// Cached string representation of transcript_path (computed once).
    pub(super) transcript_path_str: Option<String>,
}

impl AgentSession {
    pub fn new(
        provider: Box<dyn AgentProvider>,
        tools: Vec<ToolDefinition>,
        system_prompt: String,
        event_tx: mpsc::Sender<AiEvent>,
        command_rx: mpsc::Receiver<AiCommand>,
    ) -> Self {
        // ... (partition tools omitted for brevity)
        let mut core_tools: Vec<ToolDefinition> = tools
            .iter()
            .filter(|t| crate::tools::classify_tool_tier(&t.name) == crate::tools::ToolTier::Core)
            .cloned()
            .collect();
        core_tools.push(crate::tools::request_tools_definition());

        let system_prompt_tokens = token_estimate::estimate_tokens(&system_prompt);
        let tools_tokens = token_estimate::estimate_tools_tokens(&core_tools);

        // Setup transcript logging — XDG-compliant path:
        //   $XDG_DATA_HOME/mae/transcripts/ (default: ~/.local/share/mae/transcripts/)
        // Falls back to $HOME/.local/share/mae/transcripts/ if XDG is unset.
        let transcript_path = {
            let base = std::env::var("XDG_DATA_HOME")
                .map(PathBuf::from)
                .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".local/share")))
                .ok();
            if let Some(mut p) = base {
                p.push("mae");
                p.push("transcripts");
                let _ = std::fs::create_dir_all(&p);
                let filename = format!("{}.json", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"));
                p.push(filename);
                Some(p)
            } else {
                None
            }
        };

        let original_system_prompt = system_prompt.clone();
        AgentSession {
            provider,
            all_tools: tools,
            tools: core_tools,
            messages: Vec::new(),
            system_prompt,
            event_tx,
            command_rx,
            max_rounds: 250,
            max_messages: 2000,
            consecutive_errors: 0,
            price: None,
            budget: crate::BudgetConfig::default(),
            session_cost_usd: 0.0,
            session_tokens_in: 0,
            session_tokens_out: 0,
            session_cache_read: 0,
            session_cache_creation: 0,
            warned: false,
            context_window: crate::context_limits::DEFAULT_CONTEXT_WINDOW,
            system_prompt_tokens,
            tools_tokens,
            reserved_output: 4096,
            initialized: false,
            model_name: String::new(),
            enabled_categories: std::collections::HashSet::new(),
            transaction_start_idx: None,
            current_round: 0,
            target_buffer: None,
            current_mode: "standard".into(),
            current_profile: "pair-programmer".into(),
            last_tool_calls: None,
            turn_history: std::collections::VecDeque::with_capacity(6),
            progress: progress::ProgressTracker::new(10, false),
            degradation_level: DegradationLevel::Normal,
            original_system_prompt,
            transcript_path_str: transcript_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            transcript_path,
        }
    }

    pub fn with_target_buffer(mut self, name: String) -> Self {
        self.target_buffer = Some(name);
        self
    }

    /// Configure for self-test mode: wider checkpoint interval, higher stagnation tolerance.
    pub fn with_self_test_mode(mut self) -> Self {
        self.progress = progress::ProgressTracker::new(15, true);
        self
    }

    /// Configure model + budget for this session. Called once by the
    /// editor bootstrap after the session is constructed but before
    /// it starts running. Separated from `new` so tests can exercise
    /// the session without a real `ProviderConfig`.
    ///
    /// The model name is resolved to a `ModelPrice` immediately and
    /// cached — the pricing table doesn't change at runtime, so every
    /// subsequent round can skip the prefix-scan + lowercase alloc.
    pub fn with_budget(mut self, model: impl AsRef<str>, budget: crate::BudgetConfig) -> Self {
        let model_str = model.as_ref();
        self.price = crate::pricing::lookup(model_str);
        let limits = crate::context_limits::lookup(model_str);
        self.context_window = limits.context_window;
        self.max_rounds = limits.max_rounds;
        self.model_name = model_str.to_string();
        self.budget = budget;
        self
    }
}
