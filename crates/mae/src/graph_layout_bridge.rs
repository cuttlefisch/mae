//! Graph layout bridge — computes the KB graph view's force-directed layout
//! on a background blocking thread, mirroring `lsp_bridge`/`dap_bridge`'s
//! "background task + channel + `EventLoopProxy`" shape (the Alacritty
//! pattern — no polling; see `gui_app.rs`'s module doc). Layout computation
//! (`ForceLayout::run`) is CPU-bound synchronous work, so each request is
//! off-loaded to `tokio::task::spawn_blocking` rather than run inline on the
//! async executor's worker pool — see the KB-graph-view architecture plan's
//! "Threading" section.
//!
//! Unlike the persistent LSP/DAP adapter connections, graph layout is a
//! one-shot request/response per open/refresh/set-depth — this bridge is a
//! single `mpsc` receiver drained in `bridge_task`'s existing `select!`
//! loop (same channel-per-source shape as `ai_rx`/`lsp_rx`/`dap_rx`), not a
//! separately-spawned persistent task.

use mae_canvas::layout::{ForceLayout, LayoutConfig};
use mae_canvas::scene::SceneGraph;
use mae_core::GraphLayoutIntent;

/// A request to (re)compute a force-directed layout for the graph-view
/// buffer at `buf_idx`. Constructed from `mae_core::GraphLayoutIntent`
/// (`Editor::pending_graph_layout`) by `drain_graph_layout_intent`.
#[derive(Debug)]
pub struct GraphLayoutRequest {
    pub buf_idx: usize,
    pub scene: SceneGraph,
    pub iterations: usize,
}

impl From<GraphLayoutIntent> for GraphLayoutRequest {
    fn from(intent: GraphLayoutIntent) -> Self {
        GraphLayoutRequest {
            buf_idx: intent.buf_idx,
            scene: intent.scene,
            iterations: intent.iterations,
        }
    }
}

/// A completed layout, ready to be applied back onto the owning
/// `GraphView.scene` via `Editor::apply_graph_layout_result`.
#[derive(Debug)]
pub struct GraphLayoutResult {
    pub buf_idx: usize,
    pub scene: SceneGraph,
}

/// Drain `editor.pending_graph_layout` (set by `graph_view_ops.rs` on every
/// open/refresh/set-depth) and forward it to the background layout channel.
/// Safe to call every loop iteration — at most one request is queued at a
/// time (`Option`, not a `Vec`), matching "multiple graph views aren't
/// expected but the plumbing is correct regardless" from the architecture
/// plan (a second request simply supersedes the first if the background
/// task hasn't drained it yet, since both target the same `buf_idx` in
/// practice today).
pub(crate) fn drain_graph_layout_intent(
    editor: &mut mae_core::Editor,
    tx: &tokio::sync::mpsc::Sender<GraphLayoutRequest>,
) {
    let Some(intent) = editor.pending_graph_layout.take() else {
        return;
    };
    if tx.try_send(intent.into()).is_err() {
        tracing::warn!("graph layout request channel full or closed — request dropped");
    }
}

/// Run one queued layout request on a blocking thread and send the result
/// back via `proxy`. Called from `bridge_task`'s `select!` arm for
/// `graph_layout_rx.recv()`.
pub(crate) fn spawn_layout_computation(
    req: GraphLayoutRequest,
    proxy: winit::event_loop::EventLoopProxy<crate::gui_event::MaeEvent>,
) {
    tokio::task::spawn_blocking(move || {
        let mut scene = req.scene;
        let layout = ForceLayout::new(LayoutConfig::default());
        layout.run(&mut scene.nodes, &scene.edges, req.iterations);
        let result = GraphLayoutResult {
            buf_idx: req.buf_idx,
            scene,
        };
        let _ = proxy.send_event(crate::gui_event::MaeEvent::GraphLayoutEvent(result));
    });
}

/// Apply a completed background layout back onto the owning `GraphView`.
/// Thin wrapper so `gui_app.rs`'s `user_event` match arm reads consistently
/// with `dap_bridge::handle_dap_event`/`lsp_bridge::handle_lsp_event`.
pub(crate) fn handle_graph_layout_event(editor: &mut mae_core::Editor, result: GraphLayoutResult) {
    editor.apply_graph_layout_result(result.buf_idx, result.scene);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_converts_to_request_verbatim() {
        let intent = GraphLayoutIntent {
            buf_idx: 3,
            scene: SceneGraph::new(),
            iterations: 42,
        };
        let req: GraphLayoutRequest = intent.into();
        assert_eq!(req.buf_idx, 3);
        assert_eq!(req.iterations, 42);
    }

    #[test]
    fn drain_takes_pending_intent_and_leaves_none_behind() {
        let mut editor = mae_core::Editor::new();
        editor.pending_graph_layout = Some(GraphLayoutIntent {
            buf_idx: 0,
            scene: SceneGraph::new(),
            iterations: 10,
        });
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        drain_graph_layout_intent(&mut editor, &tx);
        assert!(editor.pending_graph_layout.is_none());
        let received = rx.try_recv().expect("request should have been sent");
        assert_eq!(received.iterations, 10);
    }

    #[test]
    fn drain_is_a_no_op_when_nothing_pending() {
        let mut editor = mae_core::Editor::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        drain_graph_layout_intent(&mut editor, &tx);
        assert!(rx.try_recv().is_err());
    }
}
