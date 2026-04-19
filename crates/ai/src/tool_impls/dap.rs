//! DAP-related AI tool implementations.
//!
//! Exposes five action-oriented tools so the AI agent can drive a
//! debug session as a peer actor: `dap_start`, `dap_set_breakpoint`,
//! `dap_continue`, `dap_step`, `dap_inspect_variable`.
//!
//! The richer read-side view (threads, frames, scopes, all variables,
//! all breakpoints) is already covered by `debug_state`, so these
//! tools stay focused on **actions** and **targeted queries**.
//!
//! Design notes:
//!
//! - All ops are idempotent where it makes sense. `dap_set_breakpoint`
//!   no-ops when the line is already set; `dap_start` fails loudly if
//!   a session is already active rather than stacking sessions.
//! - Line numbers in tool arguments and return values are 1-indexed,
//!   matching `lsp_diagnostics`, status-bar conventions, and DAP itself.
//! - Continue/step error out (rather than silently no-op) when no
//!   session is active — the AI needs clear feedback that its mental
//!   model is stale. Editor helpers remain no-op for keymap safety.

use mae_core::{Editor, StepKind};
use serde_json::{json, Value};

/// Start a DAP session against a program using a named adapter preset.
///
/// Args:
/// - `adapter` (string, required): `"lldb"`, `"debugpy"`, or `"codelldb"`.
/// - `program` (string, required): path to the binary/script to debug.
/// - `args` (array of strings, optional): program arguments.
///
/// Returns a short confirmation string. Actual adapter startup happens
/// asynchronously — the AI should call `debug_state` after a moment to
/// see the session come up.
pub fn execute_dap_start(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let adapter = args
        .get("adapter")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'adapter' argument (one of: lldb, debugpy, codelldb)")?;

    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("launch");

    if mode == "attach" {
        let pid = args
            .get("pid")
            .and_then(|v| v.as_u64())
            .ok_or("Missing 'pid' argument for attach mode")?;
        editor.dap_attach_with_adapter(adapter, pid as u32)?;
        return Ok(format!("Attaching {} to pid {}", adapter, pid));
    }

    let program = args
        .get("program")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'program' argument")?;
    let extra_args: Vec<String> = args
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    // Preconditions (unknown adapter, already-active session) live in
    // `dap_start_with_adapter` so this tool and `:debug-start` agree.
    editor.dap_start_with_adapter(adapter, program, &extra_args)?;
    Ok(format!("Starting {} session against {}", adapter, program))
}

/// Set a breakpoint at `source:line`. Idempotent — no-op if already set.
///
/// Args:
/// - `source` (string, required): source file path (matches adapter's view).
/// - `line` (integer, required): 1-indexed line number.
///
/// Returns JSON with the full line set for that source after the op.
pub fn execute_dap_set_breakpoint(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let source = args
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'source' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_i64())
        .ok_or("Missing 'line' argument (1-indexed)")?;
    if line < 1 {
        return Err("'line' must be >= 1".into());
    }
    let condition = args
        .get("condition")
        .and_then(|v| v.as_str())
        .map(String::from);
    let hit_condition = args
        .get("hit_condition")
        .and_then(|v| v.as_str())
        .map(String::from);

    let lines = if condition.is_some() || hit_condition.is_some() {
        editor.dap_set_breakpoint_conditional(
            source.to_string(),
            line,
            condition.clone(),
            hit_condition.clone(),
        )
    } else {
        editor.dap_set_breakpoint(source.to_string(), line)
    };
    let mut result = json!({
        "source": source,
        "line": line,
        "all_lines_for_source": lines,
    });
    if let Some(c) = &condition {
        result["condition"] = json!(c);
    }
    if let Some(hc) = &hit_condition {
        result["hit_condition"] = json!(hc);
    }
    Ok(result.to_string())
}

/// Resume execution on the active thread.
///
/// Errors if no session is active (helps the AI catch stale state).
pub fn execute_dap_continue(editor: &mut Editor) -> Result<String, String> {
    if editor.debug_state.is_none() {
        return Err("No active debug session".into());
    }
    editor.dap_continue();
    Ok("continue".into())
}

/// Step on the active thread.
///
/// Args:
/// - `direction` (string, required): `"over"`, `"in"`, or `"out"`.
pub fn execute_dap_step(editor: &mut Editor, args: &Value) -> Result<String, String> {
    if editor.debug_state.is_none() {
        return Err("No active debug session".into());
    }
    let direction = args
        .get("direction")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'direction' argument (one of: over, in, out)")?;
    let kind = StepKind::parse(direction).ok_or_else(|| {
        format!(
            "Unknown step direction: {} (expected: over, in, out)",
            direction
        )
    })?;
    editor.dap_step(kind);
    Ok(format!("step {}", kind.as_str()))
}

/// Look up a variable by name across all scopes of the active stop.
///
/// Args:
/// - `name` (string, required): variable name to find.
/// - `scope` (string, optional): restrict search to a specific scope name
///   (e.g. `"Locals"`, `"Globals"`). Default: search all scopes.
///
/// Returns JSON with `name`, `value`, `type`, `scope`, `variables_reference`
/// (non-zero means expandable — use `debug_state` to follow children).
/// Errors if no match is found.
pub fn execute_dap_inspect_variable(editor: &Editor, args: &Value) -> Result<String, String> {
    let state = editor
        .debug_state
        .as_ref()
        .ok_or("No active debug session")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' argument")?;
    let scope_filter = args.get("scope").and_then(|v| v.as_str());

    match state.find_variable(name, scope_filter) {
        Some((scope, var)) => Ok(json!({
            "name": var.name,
            "value": var.value,
            "type": var.var_type,
            "scope": scope.name,
            "variables_reference": var.variables_reference,
        })
        .to_string()),
        None => Err(match scope_filter {
            Some(s) => format!("Variable '{}' not found in scope '{}'", name, s),
            None => format!("Variable '{}' not found in any scope", name),
        }),
    }
}

/// Remove a breakpoint at `source:line`.
///
/// Args:
/// - `source` (string, required): source file path.
/// - `line` (integer, required): 1-indexed line number.
///
/// Returns JSON with the remaining line set for that source.
pub fn execute_dap_remove_breakpoint(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let source = args
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'source' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_i64())
        .ok_or("Missing 'line' argument (1-indexed)")?;
    if line < 1 {
        return Err("'line' must be >= 1".into());
    }

    let lines = editor.dap_remove_breakpoint(source.to_string(), line);
    Ok(json!({
        "source": source,
        "removed_line": line,
        "remaining_lines": lines,
    })
    .to_string())
}

/// List all variables in the current frame's scopes.
///
/// Returns JSON with scope → variable list mapping. Includes expanded
/// children from the debug panel's `DebugView.child_variables` so the AI
/// can see results of prior `dap_expand_variable` calls.
pub fn execute_dap_list_variables(editor: &Editor) -> Result<String, String> {
    let state = editor
        .debug_state
        .as_ref()
        .ok_or("No active debug session")?;

    // Grab child_variables from the debug view (if the panel is open).
    let child_vars = editor
        .buffers
        .iter()
        .find(|b| b.kind == mae_core::buffer::BufferKind::Debug)
        .and_then(|b| b.debug_view.as_ref())
        .map(|v| &v.child_variables);

    let mut scopes_out = serde_json::Map::new();
    for scope in &state.scopes {
        let vars = state
            .variables
            .get(&scope.name)
            .map(|vars| {
                vars.iter()
                    .map(|v| render_variable_json(v, child_vars))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        scopes_out.insert(scope.name.clone(), json!(vars));
    }
    serde_json::to_string_pretty(&scopes_out).map_err(|e| e.to_string())
}

/// Render a variable to JSON, recursing into expanded children.
fn render_variable_json(
    v: &mae_core::debug::Variable,
    child_vars: Option<&std::collections::HashMap<i64, Vec<mae_core::debug::Variable>>>,
) -> Value {
    let mut obj = json!({
        "name": v.name,
        "value": v.value,
        "type": v.var_type,
        "variables_reference": v.variables_reference,
    });
    if v.variables_reference > 0 {
        if let Some(children_map) = child_vars {
            if let Some(children) = children_map.get(&v.variables_reference) {
                let child_json: Vec<Value> = children
                    .iter()
                    .map(|c| render_variable_json(c, child_vars))
                    .collect();
                obj["children"] = json!(child_json);
            }
        }
    }
    obj
}

/// Get children of a nested variable by its `variables_reference`.
///
/// Args:
/// - `variables_reference` (integer, required): the parent's reference.
/// - `scope` (string, required): scope name for the request.
///
/// Queues a DAP request and returns immediately. The AI should call
/// `debug_state` or `dap_list_variables` after a moment to see results.
pub fn execute_dap_expand_variable(editor: &mut Editor, args: &Value) -> Result<String, String> {
    if editor.debug_state.is_none() {
        return Err("No active debug session".into());
    }
    let var_ref = args
        .get("variables_reference")
        .and_then(|v| v.as_i64())
        .ok_or("Missing 'variables_reference' argument")?;
    let scope = args
        .get("scope")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'scope' argument")?;

    editor.dap_request_variables(scope.to_string(), var_ref);
    Ok(format!(
        "Requested children for variables_reference={} in scope '{}'",
        var_ref, scope
    ))
}

/// Switch to a different stack frame.
///
/// Args:
/// - `frame_id` (integer, required): the frame id to select.
///
/// Queues a scopes request for the new frame.
pub fn execute_dap_select_frame(editor: &mut Editor, args: &Value) -> Result<String, String> {
    if editor.debug_state.is_none() {
        return Err("No active debug session".into());
    }
    let frame_id = args
        .get("frame_id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing 'frame_id' argument")?;

    // Verify the frame exists.
    let frame_exists = editor
        .debug_state
        .as_ref()
        .map(|s| s.stack_frames.iter().any(|f| f.id == frame_id))
        .unwrap_or(false);
    if !frame_exists {
        return Err(format!("No frame with id {}", frame_id));
    }

    // Update selected_frame_id in the debug view so the panel tracks the AI's selection.
    if let Some(buf) = editor
        .buffers
        .iter_mut()
        .find(|b| b.kind == mae_core::buffer::BufferKind::Debug)
    {
        if let Some(view) = buf.debug_view.as_mut() {
            view.selected_frame_id = Some(frame_id);
        }
    }

    editor.dap_request_scopes(frame_id);
    Ok(format!("Selected frame {} and requested scopes", frame_id))
}

/// Switch the active thread.
///
/// Args:
/// - `thread_id` (integer, required): the thread id to select.
pub fn execute_dap_select_thread(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let state = editor
        .debug_state
        .as_mut()
        .ok_or("No active debug session")?;
    let thread_id = args
        .get("thread_id")
        .and_then(|v| v.as_i64())
        .ok_or("Missing 'thread_id' argument")?;

    if !state.set_active_thread(thread_id) {
        return Err(format!("No thread with id {}", thread_id));
    }

    // Refresh to get stack trace for the new thread.
    editor.dap_refresh();
    Ok(format!("Switched to thread {}", thread_id))
}

/// Read recent debug output log.
///
/// Args:
/// - `lines` (integer, optional): number of recent lines to return (default 50).
pub fn execute_dap_output(editor: &Editor, args: &Value) -> Result<String, String> {
    let state = editor
        .debug_state
        .as_ref()
        .ok_or("No active debug session")?;

    let max_lines = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

    let total = state.output_log.len();
    let start = total.saturating_sub(max_lines);
    let lines: Vec<&str> = state.output_log[start..]
        .iter()
        .map(|s| s.as_str())
        .collect();

    Ok(json!({
        "total_lines": total,
        "returned_lines": lines.len(),
        "output": lines,
    })
    .to_string())
}

/// Evaluate an expression in the debuggee's context.
///
/// Args:
/// - `expression` (string, required): expression to evaluate.
/// - `frame_id` (integer, optional): stack frame for evaluation context.
/// - `context` (string, optional): `"watch"`, `"repl"`, or `"hover"`.
///
/// This is a deferred tool — the result arrives asynchronously via
/// `DapTaskEvent::EvaluateResult`. The AI should call `debug_state`
/// or `dap_output` after a moment to see the result.
pub fn execute_dap_evaluate(editor: &mut Editor, args: &Value) -> Result<String, String> {
    if editor.debug_state.is_none() {
        return Err("No active debug session".into());
    }
    let expression = args
        .get("expression")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'expression' argument")?;
    if expression.is_empty() {
        return Err("'expression' must not be empty".into());
    }
    let frame_id = args.get("frame_id").and_then(|v| v.as_i64());
    let context = args.get("context").and_then(|v| v.as_str());

    editor.dap_evaluate(expression, frame_id, context);
    Ok(format!("Evaluating: {}", expression))
}

/// Disconnect from the debug adapter.
///
/// Args:
/// - `terminate_debuggee` (boolean, optional): if true, also terminate
///   the debugged process. Default: false (detach only).
pub fn execute_dap_disconnect(editor: &mut Editor, args: &Value) -> Result<String, String> {
    if editor.debug_state.is_none() {
        return Err("No active debug session".into());
    }
    let terminate = args
        .get("terminate_debuggee")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    editor.dap_disconnect(terminate);
    Ok(format!("Disconnecting (terminate_debuggee={})", terminate))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::{DebugState, DebugTarget, Scope, Variable};

    fn ed_with_dap_session() -> Editor {
        let mut ed = Editor::new();
        ed.debug_state = Some(DebugState::new(DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "/bin/ls".into(),
        }));
        ed.debug_state.as_mut().unwrap().active_thread_id = 1;
        ed
    }

    #[test]
    fn dap_start_requires_adapter_and_program() {
        let mut ed = Editor::new();
        let err = execute_dap_start(&mut ed, &json!({})).unwrap_err();
        assert!(err.contains("adapter"));
        let err = execute_dap_start(&mut ed, &json!({"adapter": "lldb"})).unwrap_err();
        assert!(err.contains("program"));
    }

    #[test]
    fn dap_start_queues_intent() {
        let mut ed = Editor::new();
        let out =
            execute_dap_start(&mut ed, &json!({"adapter": "lldb", "program": "/bin/ls"})).unwrap();
        assert!(out.contains("Starting lldb"));
        assert_eq!(ed.pending_dap_intents.len(), 1);
        assert!(ed.debug_state.is_some());
    }

    #[test]
    fn dap_start_with_program_args() {
        let mut ed = Editor::new();
        let out = execute_dap_start(
            &mut ed,
            &json!({
                "adapter": "lldb",
                "program": "/bin/ls",
                "args": ["--help", "-la"],
            }),
        )
        .unwrap();
        assert!(out.contains("Starting"));
        assert_eq!(ed.pending_dap_intents.len(), 1);
    }

    #[test]
    fn dap_start_rejects_concurrent_session() {
        // Guard lives in `dap_start_with_adapter`, so the error surfaces
        // through `Result<(), String>` to the tool layer unchanged.
        let mut ed = ed_with_dap_session();
        let err = execute_dap_start(&mut ed, &json!({"adapter": "lldb", "program": "/bin/ls"}))
            .unwrap_err();
        assert!(err.contains("already active"));
    }

    #[test]
    fn dap_start_unknown_adapter_errors() {
        let mut ed = Editor::new();
        let err = execute_dap_start(&mut ed, &json!({"adapter": "bogus", "program": "/bin/ls"}))
            .unwrap_err();
        assert!(err.contains("Unknown adapter"));
    }

    #[test]
    fn dap_set_breakpoint_returns_line_set() {
        let mut ed = Editor::new();
        let out =
            execute_dap_set_breakpoint(&mut ed, &json!({"source": "/a.rs", "line": 10})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["source"], "/a.rs");
        assert_eq!(v["line"], 10);
        assert_eq!(v["all_lines_for_source"], json!([10]));
    }

    #[test]
    fn dap_set_breakpoint_is_idempotent() {
        let mut ed = Editor::new();
        execute_dap_set_breakpoint(&mut ed, &json!({"source": "/a.rs", "line": 10})).unwrap();
        let out =
            execute_dap_set_breakpoint(&mut ed, &json!({"source": "/a.rs", "line": 10})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        // Still one entry — not duplicated.
        assert_eq!(v["all_lines_for_source"], json!([10]));
    }

    #[test]
    fn dap_set_breakpoint_rejects_missing_args() {
        let mut ed = Editor::new();
        let err = execute_dap_set_breakpoint(&mut ed, &json!({"line": 1})).unwrap_err();
        assert!(err.contains("source"));
        let err = execute_dap_set_breakpoint(&mut ed, &json!({"source": "/a.rs"})).unwrap_err();
        assert!(err.contains("line"));
    }

    #[test]
    fn dap_set_breakpoint_rejects_zero_line() {
        let mut ed = Editor::new();
        let err = execute_dap_set_breakpoint(&mut ed, &json!({"source": "/a.rs", "line": 0}))
            .unwrap_err();
        assert!(err.contains(">= 1"));
    }

    #[test]
    fn dap_continue_without_session_errors() {
        let mut ed = Editor::new();
        let err = execute_dap_continue(&mut ed).unwrap_err();
        assert!(err.contains("No active"));
    }

    #[test]
    fn dap_continue_queues_intent() {
        let mut ed = ed_with_dap_session();
        execute_dap_continue(&mut ed).unwrap();
        assert_eq!(ed.pending_dap_intents.len(), 1);
    }

    #[test]
    fn dap_step_requires_direction() {
        let mut ed = ed_with_dap_session();
        let err = execute_dap_step(&mut ed, &json!({})).unwrap_err();
        assert!(err.contains("direction"));
    }

    #[test]
    fn dap_step_unknown_direction_errors() {
        let mut ed = ed_with_dap_session();
        let err = execute_dap_step(&mut ed, &json!({"direction": "sideways"})).unwrap_err();
        assert!(err.contains("Unknown step"));
    }

    #[test]
    fn dap_step_all_directions_queue_intent() {
        for dir in ["over", "in", "out"] {
            let mut ed = ed_with_dap_session();
            execute_dap_step(&mut ed, &json!({"direction": dir})).unwrap();
            assert_eq!(ed.pending_dap_intents.len(), 1, "direction {}", dir);
        }
    }

    #[test]
    fn dap_step_without_session_errors() {
        let mut ed = Editor::new();
        let err = execute_dap_step(&mut ed, &json!({"direction": "over"})).unwrap_err();
        assert!(err.contains("No active"));
    }

    #[test]
    fn dap_inspect_variable_finds_match() {
        let mut ed = ed_with_dap_session();
        let state = ed.debug_state.as_mut().unwrap();
        state.scopes.push(Scope {
            name: "Locals".into(),
            variables_reference: 1,
            expensive: false,
        });
        state.variables.insert(
            "Locals".into(),
            vec![Variable {
                name: "x".into(),
                value: "42".into(),
                var_type: Some("i32".into()),
                variables_reference: 0,
            }],
        );
        let out = execute_dap_inspect_variable(&ed, &json!({"name": "x"})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["name"], "x");
        assert_eq!(v["value"], "42");
        assert_eq!(v["type"], "i32");
        assert_eq!(v["scope"], "Locals");
    }

    #[test]
    fn dap_inspect_variable_scope_filter() {
        let mut ed = ed_with_dap_session();
        let state = ed.debug_state.as_mut().unwrap();
        state.scopes.push(Scope {
            name: "Locals".into(),
            variables_reference: 1,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "Globals".into(),
            variables_reference: 2,
            expensive: false,
        });
        state.variables.insert(
            "Locals".into(),
            vec![Variable {
                name: "x".into(),
                value: "1".into(),
                var_type: None,
                variables_reference: 0,
            }],
        );
        state.variables.insert(
            "Globals".into(),
            vec![Variable {
                name: "x".into(),
                value: "999".into(),
                var_type: None,
                variables_reference: 0,
            }],
        );

        let out =
            execute_dap_inspect_variable(&ed, &json!({"name": "x", "scope": "Globals"})).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["value"], "999");
        assert_eq!(v["scope"], "Globals");
    }

    #[test]
    fn dap_inspect_variable_not_found_errors() {
        let ed = ed_with_dap_session();
        let err = execute_dap_inspect_variable(&ed, &json!({"name": "ghost"})).unwrap_err();
        assert!(err.contains("not found"));
    }

    #[test]
    fn dap_inspect_variable_without_session_errors() {
        let ed = Editor::new();
        let err = execute_dap_inspect_variable(&ed, &json!({"name": "x"})).unwrap_err();
        assert!(err.contains("No active"));
    }

    #[test]
    fn dap_inspect_variable_requires_name() {
        let ed = ed_with_dap_session();
        let err = execute_dap_inspect_variable(&ed, &json!({})).unwrap_err();
        assert!(err.contains("name"));
    }

    // ---- Tier 4: evaluate, disconnect, attach, conditional breakpoints ----

    #[test]
    fn dap_evaluate_requires_expression() {
        let mut ed = ed_with_dap_session();
        let err = execute_dap_evaluate(&mut ed, &json!({})).unwrap_err();
        assert!(err.contains("expression"));
    }

    #[test]
    fn dap_evaluate_rejects_empty_expression() {
        let mut ed = ed_with_dap_session();
        let err = execute_dap_evaluate(&mut ed, &json!({"expression": ""})).unwrap_err();
        assert!(err.contains("empty"));
    }

    #[test]
    fn dap_evaluate_without_session_errors() {
        let mut ed = Editor::new();
        let err = execute_dap_evaluate(&mut ed, &json!({"expression": "x"})).unwrap_err();
        assert!(err.contains("No active"));
    }

    #[test]
    fn dap_evaluate_queues_intent() {
        let mut ed = ed_with_dap_session();
        let out = execute_dap_evaluate(
            &mut ed,
            &json!({"expression": "1+2", "frame_id": 100, "context": "repl"}),
        )
        .unwrap();
        assert!(out.contains("Evaluating"));
        assert_eq!(ed.pending_dap_intents.len(), 1);
    }

    #[test]
    fn dap_disconnect_without_session_errors() {
        let mut ed = Editor::new();
        let err = execute_dap_disconnect(&mut ed, &json!({})).unwrap_err();
        assert!(err.contains("No active"));
    }

    #[test]
    fn dap_disconnect_clears_session() {
        let mut ed = ed_with_dap_session();
        let out = execute_dap_disconnect(&mut ed, &json!({"terminate_debuggee": true})).unwrap();
        assert!(out.contains("Disconnecting"));
        assert!(ed.debug_state.is_none());
    }

    #[test]
    fn dap_disconnect_defaults_no_terminate() {
        let mut ed = ed_with_dap_session();
        let out = execute_dap_disconnect(&mut ed, &json!({})).unwrap();
        assert!(out.contains("terminate_debuggee=false"));
    }

    #[test]
    fn dap_start_attach_mode() {
        let mut ed = Editor::new();
        let out = execute_dap_start(
            &mut ed,
            &json!({"adapter": "lldb", "mode": "attach", "pid": 12345}),
        )
        .unwrap();
        assert!(out.contains("Attaching"));
        assert!(out.contains("12345"));
        assert_eq!(ed.pending_dap_intents.len(), 1);
        assert!(matches!(
            ed.pending_dap_intents[0],
            mae_core::DapIntent::StartSession { attach: true, .. }
        ));
    }

    #[test]
    fn dap_start_attach_requires_pid() {
        let mut ed = Editor::new();
        let err =
            execute_dap_start(&mut ed, &json!({"adapter": "lldb", "mode": "attach"})).unwrap_err();
        assert!(err.contains("pid"));
    }

    #[test]
    fn dap_set_breakpoint_with_condition() {
        let mut ed = Editor::new();
        let out = execute_dap_set_breakpoint(
            &mut ed,
            &json!({"source": "/a.rs", "line": 10, "condition": "x > 5"}),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["condition"], "x > 5");
        // Verify it's stored in state.
        let state = ed.debug_state.as_ref().unwrap();
        let bp = &state.breakpoints["/a.rs"][0];
        assert_eq!(bp.condition.as_deref(), Some("x > 5"));
    }

    #[test]
    fn dap_set_breakpoint_with_hit_condition() {
        let mut ed = Editor::new();
        let out = execute_dap_set_breakpoint(
            &mut ed,
            &json!({"source": "/a.rs", "line": 10, "hit_condition": ">= 5"}),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["hit_condition"], ">= 5");
    }
}
