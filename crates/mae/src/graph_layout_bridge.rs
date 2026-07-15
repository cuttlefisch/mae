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
use mae_core::{GraphLayoutIntent, GraphLayoutMode};

/// A request to (re)compute a force-directed layout for the graph-view
/// buffer at `buf_idx`. Constructed from `mae_core::GraphLayoutIntent`
/// (`Editor::pending_graph_layout`) by `drain_graph_layout_intent`. `mode`
/// carries `GraphLayoutIntent`'s `OneShot`/`Tick` distinction through
/// unchanged — see `GraphLayoutMode`'s doc comment.
#[derive(Debug)]
pub struct GraphLayoutRequest {
    pub buf_idx: usize,
    pub scene: SceneGraph,
    pub mode: GraphLayoutMode,
    pub layout_config: LayoutConfig,
}

impl From<GraphLayoutIntent> for GraphLayoutRequest {
    fn from(intent: GraphLayoutIntent) -> Self {
        GraphLayoutRequest {
            buf_idx: intent.buf_idx,
            scene: intent.scene,
            mode: intent.mode,
            layout_config: intent.layout_config,
        }
    }
}

/// A completed layout, ready to be applied back onto the owning
/// `GraphView.scene` via `Editor::apply_graph_layout_result`.
///
/// `max_displacement` is `None` for a completed `OneShot` `run()` pass
/// (Phase 1/2, unchanged) and `Some(signal)` — `ForceLayout::step`'s
/// settlement signal — for a completed Phase 3 animation `Tick`.
#[derive(Debug)]
pub struct GraphLayoutResult {
    pub buf_idx: usize,
    pub scene: SceneGraph,
    pub max_displacement: Option<f32>,
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
/// `graph_layout_rx.recv()`. Both `GraphLayoutMode` variants reuse this same
/// spawn/channel/proxy plumbing — Phase 3 animation ticking is NOT a second
/// background mechanism, just a different (single-`step`) computation
/// dispatched from the same request type.
pub(crate) fn spawn_layout_computation(
    req: GraphLayoutRequest,
    proxy: winit::event_loop::EventLoopProxy<crate::gui_event::MaeEvent>,
) {
    tokio::task::spawn_blocking(move || {
        let mut scene = req.scene;
        let layout = ForceLayout::new(req.layout_config);
        let max_displacement = match req.mode {
            GraphLayoutMode::OneShot { iterations } => {
                layout.run(&mut scene.nodes, &scene.edges, iterations);
                None
            }
            GraphLayoutMode::Tick { temperature } => {
                Some(layout.step(&mut scene.nodes, &scene.edges, temperature))
            }
        };
        let result = GraphLayoutResult {
            buf_idx: req.buf_idx,
            scene,
            max_displacement,
        };
        let _ = proxy.send_event(crate::gui_event::MaeEvent::GraphLayoutEvent(result));
    });
}

/// Apply a completed background layout back onto the owning `GraphView`.
/// Thin wrapper so `gui_app.rs`'s `user_event` match arm reads consistently
/// with `dap_bridge::handle_dap_event`/`lsp_bridge::handle_lsp_event`.
pub(crate) fn handle_graph_layout_event(editor: &mut mae_core::Editor, result: GraphLayoutResult) {
    editor.apply_graph_layout_result(result.buf_idx, result.scene, result.max_displacement);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_converts_to_request_verbatim() {
        let intent = GraphLayoutIntent {
            buf_idx: 3,
            scene: SceneGraph::new(),
            mode: GraphLayoutMode::OneShot { iterations: 42 },
            layout_config: LayoutConfig::default(),
        };
        let req: GraphLayoutRequest = intent.into();
        assert_eq!(req.buf_idx, 3);
        assert!(matches!(
            req.mode,
            GraphLayoutMode::OneShot { iterations: 42 }
        ));
    }

    #[test]
    fn intent_converts_tick_mode_verbatim() {
        let intent = GraphLayoutIntent {
            buf_idx: 7,
            scene: SceneGraph::new(),
            mode: GraphLayoutMode::Tick { temperature: 12.5 },
            layout_config: LayoutConfig::default(),
        };
        let req: GraphLayoutRequest = intent.into();
        assert_eq!(req.buf_idx, 7);
        match req.mode {
            GraphLayoutMode::Tick { temperature } => assert_eq!(temperature, 12.5),
            GraphLayoutMode::OneShot { .. } => panic!("expected Tick mode"),
        }
    }

    #[test]
    fn intent_converts_layout_config_verbatim() {
        // Regression guard for the tick-requeue trap fixed in
        // `graph_view_ops.rs::apply_graph_layout_result`: a non-default
        // `layout_config` (e.g. kind clustering enabled) must survive the
        // Intent -> Request conversion unchanged, not silently reset.
        let kind_affinity = mae_canvas::layout::KindAffinityConfig {
            same_kind_repulsion: 0.7,
            cross_kind_repulsion: 1.0,
            same_kind_attraction: 1.3,
            cross_kind_attraction: 1.0,
        };
        let intent = GraphLayoutIntent {
            buf_idx: 1,
            scene: SceneGraph::new(),
            mode: GraphLayoutMode::OneShot { iterations: 1 },
            layout_config: LayoutConfig {
                kind_affinity: Some(kind_affinity),
                ..LayoutConfig::default()
            },
        };
        let req: GraphLayoutRequest = intent.into();
        assert_eq!(req.layout_config.kind_affinity, Some(kind_affinity));
    }

    #[test]
    fn drain_takes_pending_intent_and_leaves_none_behind() {
        let mut editor = mae_core::Editor::new();
        editor.pending_graph_layout = Some(GraphLayoutIntent {
            buf_idx: 0,
            scene: SceneGraph::new(),
            mode: GraphLayoutMode::OneShot { iterations: 10 },
            layout_config: LayoutConfig::default(),
        });
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        drain_graph_layout_intent(&mut editor, &tx);
        assert!(editor.pending_graph_layout.is_none());
        let received = rx.try_recv().expect("request should have been sent");
        assert!(matches!(
            received.mode,
            GraphLayoutMode::OneShot { iterations: 10 }
        ));
    }

    #[test]
    fn drain_is_a_no_op_when_nothing_pending() {
        let mut editor = mae_core::Editor::new();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        drain_graph_layout_intent(&mut editor, &tx);
        assert!(rx.try_recv().is_err());
    }
}
