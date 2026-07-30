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
use crate::lisp_error::{Arity, LispError};
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// A single Scheme-list element expected to be a string OR a symbol --
/// mirrors `ffi::arg_string`'s own String/Symbol tolerance (bare-atom ids
/// like `foo-bar-123` read as symbols, not strings, in ordinary Scheme
/// source), applied to one already-extracted list element rather than an
/// `args` slice + index.
fn value_as_id_str(v: &Value, fn_name: &str) -> Result<String, LispError> {
    match v {
        Value::String(s) => Ok(s.to_string()),
        Value::Symbol(s) => Ok(s.name().to_string()),
        other => Err(LispError::type_error(
            "string",
            format!("{fn_name} got {other:?}"),
        )),
    }
}

/// Register the KB subgraph HTML export primitive.
pub(super) fn register_kb_export_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // (kb-export-subgraph-html ID PATH [DEPTH] [TRANSLATIONS] [TITLE] [NODE-CAP]
    //   [GUIDANCE-IDS] [CHORD-CONFIG])
    let s = shared.clone();
    vm.register_fn(
        "kb-export-subgraph-html",
        "Export a KB subgraph rooted at ID to a standalone, bilingual (EN/ES) interactive HTML \
         file at PATH — chord-diagram nav, language toggle, theme toggle, browser-history \
         navigation. Optional DEPTH (BFS hop radius, default/ceiling from the \
         kb-export-default-depth/kb-export-max-depth options). Optional TRANSLATIONS: path to a \
         `{id: {title_es, body_es}}` JSON overlay file (omit for an English-only export). \
         Optional TITLE: page <title>/<h1> text (default derived from the seed node's own \
         title). Optional NODE-CAP (reachable-set safety net, default/ceiling from \
         kb-export-default-node-cap/kb-export-max-node-cap). Optional GUIDANCE-IDS: a list of \
         node ids (strings or symbols) always included regardless of BFS reachability, e.g. \
         '(\"style-guide\" \"provenance-note\"), rendered in a distinct colophon section. \
         Optional CHORD-CONFIG: an alist of chord-diagram layout/timing overrides for this call \
         only, e.g. '((\"hover-growth-factor\" . 2.0) (\"wedge-gap-radians\" . 0.05)) — keys may \
         be strings or symbols (hyphens normalized to underscores), values must be numbers; \
         accepted keys match `chord_config`'s JSON keys on the kb_export_subgraph_html MCP tool. \
         Any field a call doesn't set falls back to its kb-export-* option (persistently \
         set-option!-able), which itself falls back to the tool's original hardcoded default. \
         Queues the request; applied on the next editor tick, with the result (success message, \
         or error) shown via the status line — the same underlying export the \
         kb_export_subgraph_html MCP tool and :kb-export-html colon-command use.",
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
            if args.len() > 5 {
                let node_cap = arg_int(args, 5, "kb-export-subgraph-html")?;
                json_args["node_cap"] = serde_json::json!(node_cap);
            }
            if args.len() > 6 {
                let ids = args[6].to_list().ok_or_else(|| {
                    LispError::type_error(
                        "list",
                        format!("kb-export-subgraph-html got {:?}", args[6]),
                    )
                })?;
                let mut guidance_ids = Vec::with_capacity(ids.len());
                for v in &ids {
                    guidance_ids.push(value_as_id_str(v, "kb-export-subgraph-html")?);
                }
                json_args["guidance_ids"] = serde_json::json!(guidance_ids);
            }
            if args.len() > 7 {
                let pairs = args[7].to_list().ok_or_else(|| {
                    LispError::type_error(
                        "list",
                        format!("kb-export-subgraph-html got {:?}", args[7]),
                    )
                })?;
                let mut chord_config = serde_json::Map::with_capacity(pairs.len());
                for pair in &pairs {
                    let key = pair.car()?;
                    let val = pair.cdr()?;
                    let key_str = match &key {
                        Value::String(k) => k.replace('-', "_"),
                        Value::Symbol(k) => k.name().replace('-', "_"),
                        other => {
                            return Err(LispError::type_error(
                                "string or symbol",
                                format!("kb-export-subgraph-html got {other:?}"),
                            ))
                        }
                    };
                    let val_num = val.as_float()?;
                    chord_config.insert(key_str, serde_json::json!(val_num));
                }
                json_args["chord_config"] = serde_json::Value::Object(chord_config);
            }
            s.lock().pending_kb_export_requests.push(json_args);
            Ok(Value::Void)
        },
    );
}
