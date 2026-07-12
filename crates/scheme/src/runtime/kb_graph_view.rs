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

use crate::ffi::{arg_int, arg_string};
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
}
