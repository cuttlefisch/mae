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

    let lines = editor.dap_set_breakpoint(source.to_string(), line);
    Ok(json!({
        "source": source,
        "line": line,
        "all_lines_for_source": lines,
    })
    .to_string())
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
}
