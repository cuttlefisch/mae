//! LSP and DAP primitives — the Scheme half of the `lsp_*`/`dap_*`/
//! `debug_state` MCP tools.
//!
//! CLAUDE.md principle #3 illustrates the AI-as-peer claim with
//! `(lsp-references ...)` and `(dap-inspect-variable ...)`. Neither existed;
//! `docs/CROSS_SURFACE_PARITY.md` recorded it as headline gap #1 ("LSP and DAP
//! have zero Scheme primitives"). This module closes it.
//!
//! # The asynchrony, and how each primitive answers it
//!
//! LSP and DAP are not synchronous in MAE. Commands and tools push an intent
//! (`Editor::lsp.pending_requests` / `Editor::dap.pending_intents`); the outer
//! binary drains it each event-loop tick and forwards it to the LSP/DAP task;
//! the answer returns later as an event delivered to *that same* event loop. A
//! Scheme primitive runs on the editor's main thread inside `eval`, so it
//! **cannot block** waiting for the answer — the loop it would block is the
//! loop that must deliver it. That is a deadlock, not a delay. Nor can this
//! crate call `Editor` directly: it holds only `Arc<Mutex<SharedState>>` (see
//! `kb_export.rs`'s header for the same constraint).
//!
//! So each primitive takes exactly the shape its backing `mae-ai`
//! implementation can honestly support — three shapes, no stubs:
//!
//! | Shape | Primitives | Why |
//! |---|---|---|
//! | **Synchronous read** | `lsp-diagnostics`, `debug-state`, `dap-inspect-variable` | The `mae-ai` implementation takes `&Editor` and answers now, so the answer is snapshotted into `SharedState` once per eval (only when the subsystem is actually live) and read straight back. |
//! | **Request + result** | `lsp-definition`, `lsp-references`, `lsp-hover`, `lsp-workspace-symbol`, `lsp-document-symbols` | The `mae-ai` implementation returns *nothing* — it queues an intent, and MCP defers the tool call until the event arrives. Scheme has no reply channel to defer, so the request returns an **id** and `(lsp-result ID)` reads the slot. See [`mae_core::scheme_async`]. |
//! | **Action + existing reader** | `dap-start`, `dap-set-breakpoint`, `dap-continue`, `dap-step-over/into/out` | The action is queued; the answer is already available through `(debug-state)`, which reads durable session state. Adding a second correlation mechanism for DAP would duplicate what `(debug-state)` already does (principle #8). |
//!
//! Payload shape: every primitive that returns structured data returns the
//! **same JSON payload the equivalent MCP tool returns**, converted by the one
//! shared `json_to_value` the `(json-decode)` primitive uses — objects become
//! alists, arrays become vectors. One data model, two surfaces, which is the
//! strongest available reading of principle #3.
//!
//! @ai-caution: [architecture-debt] The precondition checks below (`no active
//! debug session`, `no such request id`) read the per-eval snapshot so a
//! Scheme caller gets a real, catchable error instead of a status-line message
//! it cannot see. They are fast-fail validations, not authorization: the
//! authoritative checks stay in `mae_ai::execute_dap_*`, which run again
//! against the live `Editor`. Do not migrate policy into them.

use std::sync::Arc;

use parking_lot::Mutex;

use crate::ffi::{arg_int, arg_string};
use crate::lisp_error::{Arity, LispError};
use crate::permission::tier;
use crate::stdlib::json::json_to_value;
use crate::value::Value;
use crate::vm::Vm;

use super::SharedState;

/// Parse a JSON payload produced by a `mae-ai` tool implementation into the
/// Scheme value shape `(json-decode)` produces. A malformed payload is a bug
/// in the tool implementation, not user error, so it surfaces as an error
/// rather than silently degrading to a string.
fn payload_to_value(fn_name: &str, payload: &str) -> Result<Value, LispError> {
    let parsed: serde_json::Value = serde_json::from_str(payload).map_err(|e| {
        LispError::internal(format!("{fn_name}: tool returned malformed JSON: {e}"))
    })?;
    Ok(json_to_value(&parsed))
}

/// One optional Scheme argument coerced to a string; `#f` and an omitted
/// argument are the same thing.
fn opt_arg_string(args: &[Value], i: usize, fn_name: &str) -> Result<Option<String>, LispError> {
    match args.get(i) {
        None | Some(Value::Bool(false)) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.to_string())),
        Some(Value::Symbol(s)) => Ok(Some(s.name().to_string())),
        Some(other) => Err(LispError::type_error(
            "string or #f",
            format!("{fn_name} got {other:?}"),
        )),
    }
}

/// Build the `{buffer_name?, line?, character?}` argument object the deferred
/// LSP tool implementations accept, from `(… [BUFFER-NAME] [LINE] [COL])`.
///
/// LINE/COL are 1-indexed, matching the MCP tools, `:diagnostics` output, and
/// the status bar — not 0-indexed like the internal cursor.
fn lsp_position_args(args: &[Value], fn_name: &str) -> Result<serde_json::Value, LispError> {
    let mut obj = serde_json::Map::new();
    if let Some(name) = opt_arg_string(args, 0, fn_name)? {
        obj.insert("buffer_name".into(), serde_json::json!(name));
    }
    if args.len() > 1 && !matches!(args[1], Value::Bool(false)) {
        let line = arg_int(args, 1, fn_name)?;
        if line < 1 {
            return Err(LispError::internal(format!(
                "{fn_name}: LINE is 1-indexed and must be >= 1"
            )));
        }
        obj.insert("line".into(), serde_json::json!(line));
    }
    if args.len() > 2 && !matches!(args[2], Value::Bool(false)) {
        let col = arg_int(args, 2, fn_name)?;
        if col < 1 {
            return Err(LispError::internal(format!(
                "{fn_name}: COL is 1-indexed and must be >= 1"
            )));
        }
        obj.insert("character".into(), serde_json::json!(col));
    }
    Ok(serde_json::Value::Object(obj))
}

/// Queue an async request against `tool` and hand Scheme back its id.
fn queue_request(
    shared: &Arc<Mutex<SharedState>>,
    tool: &str,
    args: serde_json::Value,
) -> Result<Value, LispError> {
    let mut state = shared.lock();
    state.next_async_request_id += 1;
    let id = state.next_async_request_id;
    state
        .pending_async_requests
        .push((id, tool.to_string(), args));
    Ok(Value::Int(id as i64))
}

/// Queue a DAP action whose outcome is read back through `(debug-state)`.
///
/// `requires_session` mirrors `execute_dap_continue`/`execute_dap_step`'s own
/// precondition so the caller gets a catchable Scheme error rather than a
/// status-line message on the next tick.
fn queue_dap_action(
    shared: &Arc<Mutex<SharedState>>,
    fn_name: &str,
    tool: &str,
    args: serde_json::Value,
    requires_session: bool,
) -> Result<Value, LispError> {
    let mut state = shared.lock();
    if requires_session && state.dap_debug_state.is_none() {
        return Err(LispError::internal(format!(
            "{fn_name}: No active debug session. Call dap-start first."
        )));
    }
    state.next_async_request_id += 1;
    let id = state.next_async_request_id;
    state
        .pending_async_requests
        .push((id, tool.to_string(), args));
    Ok(Value::Bool(true))
}

/// Register the LSP + DAP primitives.
pub(super) fn register_lsp_dap_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    register_lsp_fns(vm, shared);
    register_dap_fns(vm, shared);
}

fn register_lsp_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    /// The three cursor-positional deferred LSP primitives, which differ only
    /// in which MCP tool they dispatch to. A macro (not a table + loop) so
    /// each `register_fn` call site still carries a **literal** name: the
    /// `every_registered_scheme_fn_has_a_scheme_api_doc` guard in
    /// `crates/core/src/kb_seed/scheme_api.rs` scans source text, and a name
    /// passed as a variable would be invisible to it — the primitive would
    /// silently escape the doc-parity ratchet. Same reason
    /// `kb_primitives.rs::register_collab_command_prim!` is shaped this way.
    macro_rules! lsp_positional_prim {
        ($name:expr, $tool:expr, $doc:expr) => {{
            let s = shared.clone();
            vm.register_fn(
                $name,
                $doc,
                Arity::Variadic(0),
                tier::READ,
                move |args: &[Value]| {
                    let json_args = lsp_position_args(args, $name)?;
                    queue_request(&s, $tool, json_args)
                },
            );
        }};
    }

    lsp_positional_prim!(
        "lsp-definition",
        "lsp_definition",
        "Request textDocument/definition at BUFFER-NAME:LINE:COL (all optional — default the \
         active buffer at the cursor; LINE/COL are 1-indexed). LSP is asynchronous: this QUEUES \
         the request and returns a request id immediately. Read the answer with (lsp-result ID), \
         which reports 'pending until the language server replies — a Scheme primitive cannot \
         block for it without deadlocking the event loop that delivers it. The resolved value is \
         the same payload the lsp_definition MCP tool returns."
    );
    lsp_positional_prim!(
        "lsp-references",
        "lsp_references",
        "Request textDocument/references at BUFFER-NAME:LINE:COL (all optional — default the \
         active buffer at the cursor; LINE/COL are 1-indexed). Queues the request and returns a \
         request id; read the answer with (lsp-result ID). The resolved value is the same payload \
         the lsp_references MCP tool returns."
    );
    lsp_positional_prim!(
        "lsp-hover",
        "lsp_hover",
        "Request textDocument/hover at BUFFER-NAME:LINE:COL (all optional — default the active \
         buffer at the cursor; LINE/COL are 1-indexed). Queues the request and returns a request \
         id; read the answer with (lsp-result ID). The resolved value is the same payload the \
         lsp_hover MCP tool returns."
    );

    // (lsp-workspace-symbol QUERY LANGUAGE-ID) → request id
    let s = shared.clone();
    vm.register_fn(
        "lsp-workspace-symbol",
        "Request workspace/symbol for QUERY from the language server for LANGUAGE-ID (e.g. \
         \"rust\", \"python\") — both required, mirroring the lsp_workspace_symbol MCP tool, \
         because a workspace symbol search is not scoped to a buffer and so cannot infer its \
         server. Queues the request and returns a request id; read the answer with (lsp-result \
         ID).",
        Arity::Fixed(2),
        tier::READ,
        move |args: &[Value]| {
            let query = arg_string(args, 0, "lsp-workspace-symbol")?;
            let language_id = arg_string(args, 1, "lsp-workspace-symbol")?;
            queue_request(
                &s,
                "lsp_workspace_symbol",
                serde_json::json!({ "query": query, "language_id": language_id }),
            )
        },
    );

    // (lsp-document-symbols [BUFFER-NAME]) → request id
    let s = shared.clone();
    vm.register_fn(
        "lsp-document-symbols",
        "Request textDocument/documentSymbol for BUFFER-NAME (optional — default the active \
         buffer). Queues the request and returns a request id; read the answer with (lsp-result \
         ID). The resolved value is the same payload the lsp_document_symbols MCP tool returns.",
        Arity::Variadic(0),
        tier::READ,
        move |args: &[Value]| {
            let mut obj = serde_json::Map::new();
            if let Some(name) = opt_arg_string(args, 0, "lsp-document-symbols")? {
                obj.insert("buffer_name".into(), serde_json::json!(name));
            }
            queue_request(&s, "lsp_document_symbols", serde_json::Value::Object(obj))
        },
    );

    // (lsp-result ID) → 'pending | payload
    let s = shared.clone();
    vm.register_fn(
        "lsp-result",
        "Read the result of an LSP request previously queued by (lsp-definition), \
         (lsp-references), (lsp-hover), (lsp-workspace-symbol) or (lsp-document-symbols). Returns \
         the symbol 'pending while the language server has not replied yet — poll across editor \
         ticks (a hook, a test step, or the REPL), since a single eval never returns to the event \
         loop that delivers the reply. Signals an error for an unknown ID (including one evicted \
         after its result went unread) and for a request the server failed.",
        Arity::Fixed(1),
        tier::READ,
        move |args: &[Value]| {
            let id = arg_int(args, 0, "lsp-result")?;
            let state = s.lock();
            let slot = state
                .async_results
                .iter()
                .find(|(slot_id, _, _)| *slot_id as i64 == id);
            match slot {
                // Deliberately distinct from "pending": an unknown id means the
                // caller is asking about a request that never existed (or whose
                // result it left unread until eviction), and answering 'pending
                // would make that look like a slow server forever.
                None => Err(LispError::internal(format!(
                    "lsp-result: unknown request id {id}"
                ))),
                Some((_, _, None)) => Ok(Value::symbol("pending")),
                Some((_, _, Some(Err(msg)))) => {
                    Err(LispError::internal(format!("lsp-result: {msg}")))
                }
                Some((_, _, Some(Ok(payload)))) => payload_to_value("lsp-result", payload),
            }
        },
    );

    // (lsp-diagnostics [SCOPE]) → payload
    let s = shared.clone();
    vm.register_fn(
        "lsp-diagnostics",
        "Current LSP diagnostics as structured data — the same payload the lsp_diagnostics MCP \
         tool returns: an alist with \"scope\", \"counts\" (error/warning/info/hint/total) and \
         \"files\". SCOPE is \"buffer\" (the active buffer, default) or \"all\". Unlike the other \
         lsp-* primitives this answers synchronously: diagnostics are pushed by the server and \
         already held in editor state, so there is nothing to wait for.",
        Arity::Variadic(0),
        tier::READ,
        move |args: &[Value]| {
            let scope = opt_arg_string(args, 0, "lsp-diagnostics")?
                .unwrap_or_else(|| "buffer".to_string());
            if scope != "buffer" && scope != "all" {
                return Err(LispError::internal(format!(
                    "lsp-diagnostics: unknown SCOPE {scope:?} (expected \"buffer\" or \"all\")"
                )));
            }
            let state = s.lock();
            // The per-eval snapshot is taken once with scope="all", so a
            // running language server costs one `execute_lsp_diagnostics` call
            // per eval rather than one per scope. "buffer" narrows that
            // payload here against the active buffer's own path, which the
            // snapshot records alongside it.
            const EMPTY: &str = r#"{"scope":"none","counts":{"error":0,"warning":0,"info":0,"hint":0,"total":0},"files":[]}"#;
            let json = state.lsp_diagnostics_json.as_deref().unwrap_or(EMPTY);
            let mut parsed: serde_json::Value = serde_json::from_str(json).map_err(|e| {
                LispError::internal(format!("lsp-diagnostics: malformed snapshot: {e}"))
            })?;
            if scope == "buffer" {
                filter_diagnostics_to_path(&mut parsed, state.diagnostics_buffer_path.as_deref());
            }
            Ok(json_to_value(&parsed))
        },
    );
}

/// Narrow an `lsp_diagnostics` payload to a single file path, recomputing the
/// counts so they describe what is actually returned.
///
/// Kept here rather than in `mae-ai` because it operates on the *snapshot*:
/// `execute_lsp_diagnostics` already does scope filtering when it has a live
/// `&Editor`, and this crate never does. Taking the snapshot once with
/// `scope="all"` and narrowing here avoids recomputing the whole payload per
/// scope on every eval.
fn filter_diagnostics_to_path(payload: &mut serde_json::Value, path: Option<&str>) {
    let keep: Vec<serde_json::Value> = match path {
        None => Vec::new(),
        Some(p) => payload
            .get("files")
            .and_then(|f| f.as_array())
            .map(|files| {
                files
                    .iter()
                    .filter(|f| f.get("path").and_then(|v| v.as_str()) == Some(p))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
    };
    let (mut error, mut warning, mut info, mut hint) = (0u64, 0u64, 0u64, 0u64);
    for file in &keep {
        for d in file
            .get("diagnostics")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
        {
            match d.get("severity").and_then(|v| v.as_str()) {
                Some("error") => error += 1,
                Some("warning") => warning += 1,
                Some("info") => info += 1,
                Some("hint") => hint += 1,
                _ => {}
            }
        }
    }
    payload["scope"] = serde_json::json!(if keep.is_empty() { "none" } else { "buffer" });
    payload["counts"] = serde_json::json!({
        "error": error,
        "warning": warning,
        "info": info,
        "hint": hint,
        "total": error + warning + info + hint,
    });
    payload["files"] = serde_json::Value::Array(keep);
}

fn register_dap_fns(vm: &mut Vm, shared: &Arc<Mutex<SharedState>>) {
    // (dap-start ADAPTER PROGRAM [ARGS] [STOP-ON-ENTRY]) → #t
    let s = shared.clone();
    vm.register_fn(
        "dap-start",
        "Launch a debug session: ADAPTER is \"lldb\", \"debugpy\" or \"codelldb\"; PROGRAM is the \
         binary/script to debug; optional ARGS is a list of program arguments; optional \
         STOP-ON-ENTRY stops at the first line. Queues the launch (the adapter starts \
         asynchronously) and returns #t — poll (debug-state) on a later tick to see the session \
         come up. Backed by Editor::dap_start_with_adapter_opts via the same \
         mae-ai implementation the dap_start MCP tool uses.",
        Arity::Variadic(2),
        tier::SHELL,
        move |args: &[Value]| {
            let adapter = arg_string(args, 0, "dap-start")?;
            let program = arg_string(args, 1, "dap-start")?;
            let mut json_args = serde_json::json!({
                "adapter": adapter,
                "program": program,
                "mode": "launch",
            });
            if args.len() > 2 && !matches!(args[2], Value::Bool(false)) {
                let items = args[2].to_list().ok_or_else(|| {
                    LispError::type_error("list of strings", format!("dap-start got {:?}", args[2]))
                })?;
                let mut argv = Vec::with_capacity(items.len());
                for item in &items {
                    match item {
                        Value::String(v) => argv.push(v.to_string()),
                        Value::Symbol(v) => argv.push(v.name().to_string()),
                        other => {
                            return Err(LispError::type_error(
                                "string",
                                format!("dap-start ARGS got {other:?}"),
                            ))
                        }
                    }
                }
                json_args["args"] = serde_json::json!(argv);
            }
            if args.len() > 3 {
                json_args["stop_on_entry"] = serde_json::json!(args[3].is_true());
            }
            queue_dap_action(&s, "dap-start", "dap_start", json_args, false)
        },
    );

    // (dap-set-breakpoint SOURCE LINE [CONDITION]) → #t
    let s = shared.clone();
    vm.register_fn(
        "dap-set-breakpoint",
        "Set a breakpoint at SOURCE:LINE (LINE is 1-indexed). Optional CONDITION is a \
         adapter-evaluated expression that must hold for the breakpoint to fire. Idempotent — \
         setting an already-set line is a no-op. Returns #t once queued; read the resulting \
         breakpoint set back from (debug-state)'s \"breakpoints\" field. Backed by the same \
         mae-ai implementation the dap_set_breakpoint MCP tool uses.",
        Arity::Variadic(2),
        tier::WRITE,
        move |args: &[Value]| {
            let source = arg_string(args, 0, "dap-set-breakpoint")?;
            let line = arg_int(args, 1, "dap-set-breakpoint")?;
            if line < 1 {
                return Err(LispError::internal(
                    "dap-set-breakpoint: LINE is 1-indexed and must be >= 1".to_string(),
                ));
            }
            let mut json_args = serde_json::json!({ "source": source, "line": line });
            if let Some(cond) = opt_arg_string(args, 2, "dap-set-breakpoint")? {
                json_args["condition"] = serde_json::json!(cond);
            }
            queue_dap_action(
                &s,
                "dap-set-breakpoint",
                "dap_set_breakpoint",
                json_args,
                false,
            )
        },
    );

    // (dap-continue) / (dap-step-over) / (dap-step-into) / (dap-step-out) → #t
    //
    // A macro rather than a table + loop for the same reason as
    // `lsp_positional_prim!` above: each `register_fn` call site must carry a
    // literal name or the doc-parity guard's source scan cannot see it.
    // DIRECTION is `""` for continue and the DAP step direction otherwise.
    macro_rules! dap_step_prim {
        ($name:expr, $direction:expr, $doc:expr) => {{
            let s = shared.clone();
            vm.register_fn(
                $name,
                $doc,
                Arity::Fixed(0),
                tier::WRITE,
                move |_args: &[Value]| {
                    let (tool, json_args) = if $direction.is_empty() {
                        ("dap_continue", serde_json::json!({}))
                    } else {
                        ("dap_step", serde_json::json!({ "direction": $direction }))
                    };
                    queue_dap_action(&s, $name, tool, json_args, true)
                },
            );
        }};
    }

    dap_step_prim!(
        "dap-continue",
        "",
        "Resume execution on the active thread. Signals an error if no debug session is active. \
         Returns #t once queued; poll (debug-state) on a later tick for the next stop — the \
         debuggee runs asynchronously, so a Scheme primitive cannot return the stop location \
         without blocking the event loop that reports it."
    );
    dap_step_prim!(
        "dap-step-over",
        "over",
        "Step over the current line on the active thread. Signals an error if no debug session is \
         active. Returns #t once queued; poll (debug-state) for the new stop."
    );
    dap_step_prim!(
        "dap-step-into",
        "in",
        "Step into the call on the current line. Signals an error if no debug session is active. \
         Returns #t once queued; poll (debug-state) for the new stop."
    );
    dap_step_prim!(
        "dap-step-out",
        "out",
        "Step out of the current frame. Signals an error if no debug session is active. Returns \
         #t once queued; poll (debug-state) for the new stop."
    );

    // (dap-inspect-variable NAME [SCOPE]) → payload
    let s = shared.clone();
    vm.register_fn(
        "dap-inspect-variable",
        "Look up debug variable NAME across the active stop's scopes, optionally restricted to \
         SCOPE (e.g. \"Locals\", \"Globals\"). Returns an alist with \"name\", \"value\", \
         \"type\", \"scope\" and \"variables_reference\" — the same payload the \
         dap_inspect_variable MCP tool returns. Signals an error if no debug session is active or \
         if no variable matches. Answers synchronously: the stop's variables are already held in \
         editor state.",
        Arity::Variadic(1),
        tier::READ,
        move |args: &[Value]| {
            let name = arg_string(args, 0, "dap-inspect-variable")?;
            let scope_filter = opt_arg_string(args, 1, "dap-inspect-variable")?;
            let state = s.lock();
            let dbg = state.dap_debug_state.as_ref().ok_or_else(|| {
                LispError::internal(
                    "dap-inspect-variable: No active debug session. Call dap-start first."
                        .to_string(),
                )
            })?;
            // `find_variable` is the same mae-core method
            // `mae_ai::execute_dap_inspect_variable` calls — the lookup rule
            // lives in one place.
            match dbg.find_variable(&name, scope_filter.as_deref()) {
                Some((scope, var)) => {
                    let payload = serde_json::json!({
                        "name": var.name,
                        "value": var.value,
                        "type": var.var_type,
                        "scope": scope.name,
                        "variables_reference": var.variables_reference,
                    });
                    Ok(json_to_value(&payload))
                }
                None => Err(LispError::internal(match scope_filter {
                    Some(sc) => {
                        format!("dap-inspect-variable: Variable '{name}' not found in scope '{sc}'")
                    }
                    None => {
                        format!("dap-inspect-variable: Variable '{name}' not found in any scope")
                    }
                })),
            }
        },
    );

    // (debug-state) → payload | #f
    let s = shared.clone();
    vm.register_fn(
        "debug-state",
        "Structured snapshot of the active debug session — the same payload the debug_state MCP \
         tool returns: an alist with \"threads\", \"frames\", \"breakpoints\" and \"variables\" \
         grouped by scope. Returns #f when no session is active. This is the reader for every \
         dap-* action primitive: dap-start / dap-continue / dap-step-* return once the action is \
         queued, and the resulting session state shows up here on a later editor tick.",
        Arity::Fixed(0),
        tier::READ,
        move |_args: &[Value]| {
            let state = s.lock();
            match &state.dap_state_json {
                None => Ok(Value::Bool(false)),
                Some(json) => payload_to_value("debug-state", json),
            }
        },
    );
}
