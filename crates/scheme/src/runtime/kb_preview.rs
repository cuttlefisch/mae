//! KB-link hover preview Scheme primitives (KB-graph-view architecture
//! plan, Part D).
//!
//! Mirrors `kb_graph_view.rs`'s pattern exactly: each function here queues
//! a `KbPreviewIntent` onto `SharedState::pending_kb_preview_intents`,
//! drained in order into the matching `Editor::kb_preview_*` method by
//! `apply_to_editor` (`state_sync_apply.rs`). Every primitive calls the
//! SAME `Editor` method the `kb_preview_show`/`kb_preview_dismiss` MCP
//! tools and buffer-local keybindings call — CLAUDE.md principle #3
//! (AI/human parity): human and AI provably drive identical code paths.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::arg_string;
use crate::lisp_error::Arity;
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register KB-link hover preview primitives.
pub(super) fn register_kb_preview_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // (kb-preview-show ID)
    let s = shared.clone();
    vm.register_fn(
        "kb-preview-show",
        "Show the KB-link hover preview popup for ID, anchored at the current cursor position. Scoped to KB-view-mode buffers; ID does not need to be the target of a link under the cursor.",
        Arity::Fixed(1),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-preview-show")?;
            s.lock()
                .pending_kb_preview_intents
                .push(mae_core::KbPreviewIntent::Show(id));
            Ok(Value::Void)
        },
    );

    // (kb-preview-dismiss)
    let s = shared.clone();
    vm.register_fn(
        "kb-preview-dismiss",
        "Dismiss the KB-link hover preview popup, if showing.",
        Arity::Fixed(0),
        move |_args: &[Value]| {
            s.lock()
                .pending_kb_preview_intents
                .push(mae_core::KbPreviewIntent::Dismiss);
            Ok(Value::Void)
        },
    );
}
