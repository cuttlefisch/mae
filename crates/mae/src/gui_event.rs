//! Custom user event type for the winit event loop.
//!
//! The `MaeEvent` enum bridges the tokio background thread (AI, LSP, DAP, MCP,
//! shell tick) to the main GUI thread via `EventLoopProxy<MaeEvent>`. This
//! replaces the `pump_app_events` + `tokio::select!` pattern with winit's
//! proper `run_app` + `ApplicationHandler::user_event` flow.
//!
//! Alacritty uses the same architecture — see `alacritty::event::EventType`.

/// Events sent from the tokio background thread to the winit main thread.
#[derive(Debug)]
pub enum MaeEvent {
    /// An AI agent event (tool call, text response, streaming chunk, etc.).
    AiEvent(mae_ai::AiEvent),
    /// An LSP task event (definition result, diagnostics, etc.).
    LspEvent(mae_lsp::LspTaskEvent),
    /// A DAP task event (stopped, output, variables, etc.).
    DapEvent(mae_dap::DapTaskEvent),
    /// An MCP tool request from an external agent.
    McpToolRequest(mae_mcp::McpToolRequest),
    /// Shell terminals need a redraw (~30fps tick).
    ShellTick,
    /// MCP idle timeout check (~500ms tick).
    McpIdleTick,
    /// Periodic health check (~30s tick).
    HealthCheck,
    /// Idle tick — fired when no input received for ~100ms.
    /// Used for deferred background work (syntax reparse, swap files).
    IdleTick,
    /// A collaborative editing event from the collab background task.
    CollabEvent(crate::collab_bridge::CollabEvent),
    /// A completed KB graph-view force-directed layout computation
    /// (`graph_layout_bridge`, Part C Phase 1) — computed on a background
    /// blocking thread so opening/refreshing a dense graph never blocks the
    /// UI thread.
    GraphLayoutEvent(crate::graph_layout_bridge::GraphLayoutResult),
}
