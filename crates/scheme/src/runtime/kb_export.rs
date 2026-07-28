//! `(kb-export-subgraph-html ...)` — the Scheme-callable counterpart to the
//! `kb_export_subgraph_html` MCP tool, following the same pattern
//! `kb_graph_view.rs` already establishes for `kb-graph-view-*`: this
//! primitive queues a request rather than calling `Editor` directly (this
//! crate never holds a live `&Editor`, only `Arc<Mutex<SharedState>>`), and
//! `state_sync_apply.rs::apply_kb_mutations` drains it into the SAME
//! `mae_ai::execute_kb_export_subgraph_html(editor, args)` call the MCP tool
//! and the `:kb-export-html` colon-command already share — CLAUDE.md
//! principle #3 (AI/human parity): human (this primitive, wired to a real
//! command + keybinding by `modules/kb-subgraph-export/`) and AI (the MCP
//! tool) provably drive identical code paths, not two implementations that
//! can drift apart.
//!
//! See `/home/hayden/src/bilingual-kb-export/kb/adrs/0002-mae-module-not-
//! scheme-reimplementation.org` for why the actual export logic stays
//! compiled Rust and only this thin wiring is Scheme.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::{arg_int, arg_string};
use crate::lisp_error::Arity;
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Register the KB subgraph HTML export primitive.
pub(super) fn register_kb_export_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // (kb-export-subgraph-html ID PATH [DEPTH] [TRANSLATIONS] [TITLE])
    let s = shared.clone();
    vm.register_fn(
        "kb-export-subgraph-html",
        "Export a KB subgraph rooted at ID to a standalone, bilingual (EN/ES) interactive HTML \
         file at PATH — chord-diagram nav, language toggle, theme toggle, browser-history \
         navigation. Optional DEPTH (BFS hop radius, default 2, clamped to 4). Optional \
         TRANSLATIONS: path to a `{id: {title_es, body_es}}` JSON overlay file (omit for an \
         English-only export). Optional TITLE: page <title>/<h1> text (default derived from the \
         seed node's own title). Queues the request; applied on the next editor tick, with the \
         result (success message, or error) shown via the status line — the same underlying \
         export the kb_export_subgraph_html MCP tool and :kb-export-html colon-command use.",
        Arity::Variadic(2),
        move |args: &[Value]| {
            let id = arg_string(args, 0, "kb-export-subgraph-html")?;
            let path = arg_string(args, 1, "kb-export-subgraph-html")?;
            let mut json_args = serde_json::json!({ "id": id, "path": path });
            if args.len() > 2 {
                let depth = arg_int(args, 2, "kb-export-subgraph-html")?;
                json_args["depth"] = serde_json::json!(depth);
            }
            if args.len() > 3 {
                let translations = arg_string(args, 3, "kb-export-subgraph-html")?;
                json_args["translations"] = serde_json::json!(translations);
            }
            if args.len() > 4 {
                let title = arg_string(args, 4, "kb-export-subgraph-html")?;
                json_args["title"] = serde_json::json!(title);
            }
            s.lock().pending_kb_export_requests.push(json_args);
            Ok(Value::Void)
        },
    );
}
