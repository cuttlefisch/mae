//! DAP (Debug Adapter Protocol) state extracted from Editor.
//! All fields were previously `debug_state` / `pending_dap_intents` on Editor;
//! now accessed via `editor.dap.*`.

use crate::dap_intent::DapIntent;
use crate::debug::DebugState;

/// DAP context: active debug session and pending intent queue.
pub struct DapContext {
    /// Active debug session state, if any. Both self-debug and DAP populate this.
    pub state: Option<DebugState>,
    /// Queue of pending DAP requests for the binary to drain each event-loop tick.
    /// Same pattern as `pending_lsp_requests`: core cannot call async DAP code
    /// directly; commands push intents here and `main.rs` forwards them to
    /// `run_dap_task`.
    pub pending_intents: Vec<DapIntent>,
}

impl DapContext {
    pub fn new() -> Self {
        Self {
            state: None,
            pending_intents: Vec::new(),
        }
    }
}

impl Default for DapContext {
    fn default() -> Self {
        Self::new()
    }
}
