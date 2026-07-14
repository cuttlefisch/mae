//! Native KB graph view Scheme primitives (Part C Phase 1 of the
//! KB-graph-view architecture plan).
//!
//! Mirrors `kb_primitives.rs`'s KB collaboration lifecycle primitives
//! (`(kb-share)` etc.): each function here queues a `GraphViewIntent` onto
//! `SharedState::pending_graph_view_intents`, drained in order into the
//! matching `Editor::kb_graph_view_*` method by `apply_to_editor`
//! (`state_sync_apply.rs`). Every primitive calls the SAME `Editor` method
//! the `kb_graph_view_*` MCP tools + buffer-local keybindings call —
//! CLAUDE.md principle #3 (AI/human parity): human and AI provably drive
//! identical code paths.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::{arg_bool, arg_float, arg_int, arg_string};
use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register native KB graph view primitives.
pub(super) fn register_kb_graph_view_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // (kb-graph-view-open [id] [depth])
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-open",
        "Open the native KB graph view, centered on ID (default: whichever KB node the *KB* buffer is currently showing, else \"index\"), at DEPTH hops (default: kb_graph_default_depth option). Reuses the existing graph window if already open.",
        Arity::Variadic(0),
        move |args: &[Value]| {
            let center = if !args.is_empty() {
                Some(arg_string(args, 0, "kb-graph-view-open")?)
            } else {
                None
            };
            let depth = if args.len() > 1 {
                Some(arg_int(args, 1, "kb-graph-view-open")? as usize)
            } else {
                None
            };
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::Open { center, depth });
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-close)
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-close",
        "Close the native KB graph view, if open.",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::Close);
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-refresh)
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-refresh",
        "Refresh the native KB graph view in place (same center node/depth, freshly re-extracted data) if it's open. Never re-splits or steals focus.",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::Refresh);
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-set-depth N)
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-set-depth",
        "Change the native KB graph view's hop radius to N and refresh in place.",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let depth = arg_int(args, 0, "kb-graph-view-set-depth")? as usize;
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::SetDepth(depth));
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-navigate DIR) — DIR is "up"/"down"/"left"/"right"
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-navigate",
        "Move the native KB graph view's node selection toward DIR (\"up\"/\"down\"/\"left\"/\"right\").",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let dir_str = arg_string(args, 0, "kb-graph-view-navigate")?;
            let dir = mae_core::GraphNavDirection::parse(&dir_str).ok_or_else(|| {
                LispError::internal(format!(
                    "kb-graph-view-navigate: invalid direction '{}' (expected up|down|left|right)",
                    dir_str
                ))
            })?;
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::Navigate(dir));
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-select-current)
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-select-current",
        "Navigate the graph view's captured companion window (the last non-graph window focused) to the currently-selected node's KB buffer.",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::SelectCurrent);
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-zoom-to FACTOR) — issue #322
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-zoom-to",
        "Set the native KB graph view's zoom to an explicit level (0.1-10.0, clamped) — the AI-appropriate equivalent of the mouse wheel's pixel-focus-based zoom, which has no meaningful non-pointer input. Applies to the focused window if it's showing the graph, else the first window found showing it.",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let target = arg_float(args, 0, "kb-graph-view-zoom-to")?;
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::ZoomTo(target));
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-set-pinned ID PINNED [X] [Y]) — issue #322
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-set-pinned",
        "Pin or unpin a graph node by KB ID — the AI-appropriate equivalent of drag-to-pin, no drag gesture needed. Optionally repositions it to (X Y) in scene coordinates; omit both to leave it wherever it currently is. Reflattens every window showing the graph (shared topology, not per-window state).",
        Arity::Variadic(2),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-graph-view-set-pinned")?;
            let pinned = arg_bool(args, 1, "kb-graph-view-set-pinned")?;
            let pos = match args.len() {
                2 => None,
                4 => Some((
                    arg_float(args, 2, "kb-graph-view-set-pinned")?,
                    arg_float(args, 3, "kb-graph-view-set-pinned")?,
                )),
                n => {
                    return Err(LispError::internal(format!(
                        "kb-graph-view-set-pinned: expected 2 or 4 arguments (id pinned [x y]), got {n}"
                    )))
                }
            };
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::SetPinned { id, pinned, pos });
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-toggle-overlay)
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-toggle-overlay",
        "Toggle the native KB graph view between its normal tiled split-window pane and a full-frame modal overlay with a dimmed background, so the graph can be inspected without the tiled pane's size constraints. No-op if no graph view is open.",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            s.lock()
                .pending_graph_view_intents
                .push(mae_core::GraphViewIntent::ToggleOverlay);
            Ok(Value::Void)
        },
    );

    // (kb-graph-view-state) → #f if no graph view is open, else
    // (center depth kb-instance follow-current? selected-node hovered-node
    //  nodes edges)
    //   nodes: list of (id title kind x y pinned? selected? hovered?)
    //   edges: list of (source-id target-id boundary? label-or-#f)
    // Read-only, so — unlike every other primitive in this file — this
    // does NOT queue a GraphViewIntent; it reads the SharedState snapshot
    // `inject_graph_view_state` (`state_sync_inject_kb.rs`) populates fresh
    // before every eval, the same snapshot-not-live pattern `get-option`
    // uses for `option_values`. Kept in this file (not `kb_queries.rs`)
    // alongside its 6 driving siblings so the whole `kb-graph-view-*`
    // family stays discoverable in one place.
    let s = shared.clone();
    vm.register_fn(
        "kb-graph-view-state",
        "Structured introspection snapshot of the open native KB graph view: which node is hovered, which is selected, every node currently rendered (the ego-network), and every edge/link shown between them. Returns #f if no graph view is open. Returns (center depth kb-instance follow-current? selected-node hovered-node nodes edges) — nodes: list of (id title kind x y pinned? selected? hovered?); edges: list of (source-id target-id boundary? label-or-#f).",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            let state = s.lock();
            let Some(gv) = state.graph_view_state.as_ref() else {
                return Ok(Value::Bool(false));
            };

            let opt_string = |v: &Option<String>| {
                v.clone().map(Value::string).unwrap_or(Value::Bool(false))
            };

            let nodes = Value::list(
                gv.nodes
                    .iter()
                    .map(|n| {
                        Value::list(vec![
                            Value::string(n.id.clone()),
                            Value::string(n.title.clone()),
                            Value::string(format!("{:?}", n.kind)),
                            Value::Float(n.x),
                            Value::Float(n.y),
                            Value::Bool(n.pinned),
                            Value::Bool(n.selected),
                            Value::Bool(n.hovered),
                        ])
                    })
                    .collect::<Vec<_>>(),
            );
            let edges = Value::list(
                gv.edges
                    .iter()
                    .map(|e| {
                        Value::list(vec![
                            Value::string(e.source_id.clone()),
                            Value::string(e.target_id.clone()),
                            Value::Bool(e.boundary),
                            e.label
                                .clone()
                                .map(Value::string)
                                .unwrap_or(Value::Bool(false)),
                        ])
                    })
                    .collect::<Vec<_>>(),
            );

            Ok(Value::list(vec![
                opt_string(&gv.center_node),
                Value::Int(gv.depth as i64),
                opt_string(&gv.kb_instance),
                Value::Bool(gv.follow_current),
                opt_string(&gv.selected_node),
                opt_string(&gv.hovered_node),
                nodes,
                edges,
            ]))
        },
    );
}
