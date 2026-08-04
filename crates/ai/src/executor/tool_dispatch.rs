//! Tool call routing: `execute_tool()` and `dispatch_tool()`.

use mae_core::Editor;

use crate::tools::PermissionPolicy;
use crate::types::*;

use crate::tool_impls::lsp::{
    execute_lsp_definition, execute_lsp_document_symbols, execute_lsp_hover,
    execute_lsp_references, execute_lsp_workspace_symbol,
};

use super::{DeferredKind, ExecuteResult};

/// Execute a tool call against editor state.
/// Runs on the MAIN THREAD because Editor and SchemeRuntime are !Send.
///
/// This is the single point where AI actions become editor mutations.
/// Every tool call goes through here, ensuring consistent permission
/// checks and undo tracking.
pub fn execute_tool(
    editor: &mut Editor,
    call: &ToolCall,
    all_tools: &[ToolDefinition],
    policy: &PermissionPolicy,
) -> ExecuteResult {
    execute_tool_with_requester(editor, call, all_tools, policy, None, None)
}

/// Real logic behind [`execute_tool`]. `requester_provider` -- the caller's
/// AI provider, when known -- is threaded down to `kb_exec::dispatch` for
/// the AI-residency seed-content exemption's post-filter tools
/// (ADR-048/#358: kb_search, kb_search_context, kb_agenda). `None` is a
/// safe default: it's treated the same as any other non-local provider, and
/// filtering only ever removes results, and only when a KB is actually
/// `LocalModelsOnly` -- callers that don't care about residency (tests,
/// most existing call sites) are unaffected either way.
///
/// `session_id` (ADR-051) -- the issuing MCP `ClientSession::id`, or `None`
/// for dispatch with no MCP session (the embedded human AI path,
/// `--self-test`) -- is threaded to `Editor::with_ai_dispatch_scope_for_session`
/// so concurrent MCP clients each get their own companion window. `policy`
/// itself should already be the CALLER's effective, possibly per-session-
/// tightened policy (see `crates/mae/src/ai_event_handler.rs`'s
/// `effective_permission_policy`) -- this function does not itself look up
/// or apply any session-specific override, it just enforces whatever
/// `policy` it's given, same as always.
///
/// Wraps the actual dispatch (`execute_tool_dispatch_body`) in
/// `Editor::with_ai_dispatch_scope_for_session` (issue #372, ADR-051) --
/// this is THE enforced MCP/AI dispatch boundary: every tool call, for
/// every builtin command (`command_*`) and every other tool category, is
/// guaranteed a companion window that keeps the conversation/agent-shell
/// buffer visible, without any individual tool needing its own
/// window-protection logic. Do not bypass this function for tool dispatch
/// -- see also the Scheme-command bridge in
/// `crates/mae/src/ai_event_handler.rs`, the one other MCP-originated
/// mutation path, which wraps itself the same way.
pub fn execute_tool_with_requester(
    editor: &mut Editor,
    call: &ToolCall,
    all_tools: &[ToolDefinition],
    policy: &PermissionPolicy,
    requester_provider: Option<&str>,
    session_id: Option<u64>,
) -> ExecuteResult {
    editor.with_ai_dispatch_scope_for_session(session_id, |editor| {
        execute_tool_dispatch_body(editor, call, all_tools, policy, requester_provider)
    })
}

fn execute_tool_dispatch_body(
    editor: &mut Editor,
    call: &ToolCall,
    all_tools: &[ToolDefinition],
    policy: &PermissionPolicy,
    requester_provider: Option<&str>,
) -> ExecuteResult {
    // 1. Find the tool definition
    let tool_def = all_tools.iter().find(|t| t.name == call.name);
    // Decision #6: a tool's declared tier is a floor, not the whole answer.
    // `set_option` is ordinary configuration for every option except the one
    // that carries the permission tier, and `execute_command` is a Write-tier
    // passthrough to `dispatch_builtin` whose real blast radius is the tier of
    // the command it was handed. `effective_tier` only ever raises.
    let permission = crate::tools::effective_tier(
        &call.name,
        &call.arguments,
        tool_def
            .and_then(|t| t.permission)
            .unwrap_or(PermissionTier::Write),
    );

    // 1b. Validate arguments against schema
    if let Some(def) = tool_def {
        if let Err(e) = validate_tool_args(def, &call.arguments) {
            return ExecuteResult::Immediate(ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: e,
            });
        }
    }

    // 2. Ask the policy (ADR-090). This is the enforcement point; the
    // decision itself lives in `PermissionPolicy::decide` and covers BOTH the
    // tier axis and the tool-category axis (ADR-056) -- tier answers "how
    // mutating," category answers "which subsystem," and `decide` evaluates
    // deny-first so neither can be softened into an `Ask`.
    match policy.decide(&call.name, permission) {
        crate::tools::Decision::Allow => {}
        crate::tools::Decision::Deny(reason) => {
            return ExecuteResult::Immediate(ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: crate::tools::deny_message(&call.name, permission, reason),
            });
        }
        crate::tools::Decision::Ask => {
            // Nothing has run. The caller decides: prompt, or deny explicitly.
            return ExecuteResult::NeedsApproval(super::ApprovalRequest {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                tier: permission,
                auto_approve_up_to: policy.auto_approve_up_to,
            });
        }
    }

    // 3. Check for deferred (async) tools first -- LSP and DAP
    let deferred_kind = match call.name.as_str() {
        "lsp_definition" => Some(DeferredKind::LspDefinition),
        "lsp_references" => Some(DeferredKind::LspReferences),
        "lsp_hover" => Some(DeferredKind::LspHover),
        "lsp_workspace_symbol" => Some(DeferredKind::LspWorkspaceSymbol),
        "lsp_document_symbols" => Some(DeferredKind::LspDocumentSymbols),
        "dap_start" => Some(DeferredKind::DapStart),
        "dap_continue" => Some(DeferredKind::DapContinue),
        "dap_step" => Some(DeferredKind::DapStep),
        _ => None,
    };

    if let Some(kind) = deferred_kind {
        let result: Result<(), String> = match kind {
            DeferredKind::LspDefinition => execute_lsp_definition(editor, &call.arguments),
            DeferredKind::LspReferences => execute_lsp_references(editor, &call.arguments),
            DeferredKind::LspHover => execute_lsp_hover(editor, &call.arguments),
            DeferredKind::LspWorkspaceSymbol => {
                execute_lsp_workspace_symbol(editor, &call.arguments)
            }
            DeferredKind::LspDocumentSymbols => {
                execute_lsp_document_symbols(editor, &call.arguments)
            }
            DeferredKind::DapStart => {
                crate::tool_impls::execute_dap_start(editor, &call.arguments).map(|_| ())
            }
            DeferredKind::DapContinue => {
                crate::tool_impls::execute_dap_continue(editor).map(|_| ())
            }
            DeferredKind::DapStep => {
                crate::tool_impls::execute_dap_step(editor, &call.arguments).map(|_| ())
            }
        };
        return match result {
            Ok(()) => ExecuteResult::Deferred {
                tool_call_id: call.id.clone(),
                kind,
            },
            Err(e) => ExecuteResult::Immediate(ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: e,
            }),
        };
    }

    // 4. Handle ai_permissions specially (needs access to policy).
    if call.name == "ai_permissions" {
        let output = super::permission::format_permissions_info(policy);
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success: true,
            output,
        });
    }

    // 4b. Handle self_test_suite (returns structured test plan or grades results).
    // Auto-save editor state so it can be restored when the session completes.
    if call.name == "self_test_suite" {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("plan");

        // ADR-086: an unrecognised `action` or a `grade` call missing its
        // `results` array is a refusal — the requested postcondition (a
        // plan or a grade report) does not hold — and must report
        // `success: false`, not `true`. See audit #590.2.
        let (success, output) = match action {
            "plan" => {
                if !editor.self_test_active {
                    editor.save_state();
                    editor.self_test_active = true;
                }
                // Create sandbox if not already present.
                if editor.test_sandbox_dir.is_none() {
                    let project_root = editor
                        .active_project_root()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| {
                            // No editor project set — find MAE workspace root
                            // from the binary itself. current_exe() returns e.g.
                            // .../target/debug/mae; detect_project_root walks up
                            // to find .git / Cargo.toml / .project markers.
                            std::env::current_exe()
                                .ok()
                                .and_then(|exe| mae_core::detect_project_root(&exe))
                                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
                        });
                    // Ensure the editor has a project set so subsequent tools
                    // (git, project_files, etc.) operate in the right context.
                    if editor.active_project_root().is_none() {
                        let idx = editor.active_buffer_idx();
                        editor.buffers[idx].project_root = Some(project_root.clone());
                    }
                    let sandbox = super::sandbox::create_test_sandbox(&project_root);
                    editor.test_sandbox_dir = Some(sandbox.dir);
                }
                let sandbox_path = editor
                    .test_sandbox_dir
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                let filter = call
                    .arguments
                    .get("categories")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let project_root_str = editor
                    .active_project_root()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                (
                    true,
                    super::self_test::build_self_test_plan(
                        filter,
                        &sandbox_path,
                        &project_root_str,
                    ),
                )
            }
            "grade" => {
                let model = call
                    .arguments
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let results = call.arguments.get("results").and_then(|v| v.as_array());
                match results {
                    Some(arr) => {
                        let mut grades = Vec::new();
                        for entry in arr {
                            let test_id =
                                entry.get("test_id").and_then(|v| v.as_str()).unwrap_or("0");
                            let output_text =
                                entry.get("output").and_then(|v| v.as_str()).unwrap_or("");
                            let success = entry
                                .get("success")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            let tool_calls: Vec<ToolCall> = entry
                                .get("tool_calls")
                                .and_then(|v| serde_json::from_value(v.clone()).ok())
                                .unwrap_or_default();
                            let final_text = entry
                                .get("final_text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");

                            if let Some(grading_val) = entry.get("grading") {
                                if let Ok(spec) =
                                    serde_json::from_value::<super::grading::GradingSpec>(
                                        grading_val.clone(),
                                    )
                                {
                                    let grade = if !tool_calls.is_empty() || !final_text.is_empty()
                                    {
                                        super::grading::grade_prompt_result(
                                            &spec,
                                            test_id,
                                            &tool_calls,
                                            final_text,
                                        )
                                    } else {
                                        super::grading::grade_tool_result(
                                            &spec,
                                            test_id,
                                            output_text,
                                            success,
                                        )
                                    };
                                    grades.push(grade);
                                }
                            }
                        }
                        let result = super::model_exam::aggregate_grades(model, &grades);
                        let mut output = serde_json::to_string_pretty(&result).unwrap_or_default();

                        // Auto-save exam run.
                        let run = super::model_exam::ExamRun {
                            timestamp: chrono::Utc::now()
                                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                            runner: "mae-builtin".to_string(),
                            mae_version: env!("CARGO_PKG_VERSION").to_string(),
                            result: result.clone(),
                            grades: grades.clone(),
                        };
                        match super::model_exam::save_exam_run(&run) {
                            Ok(path) => {
                                output.push_str(&format!(
                                    "\n\nExam results saved to: {}",
                                    path.display()
                                ));
                            }
                            Err(e) => {
                                output.push_str(&format!(
                                    "\n\nWarning: failed to save exam results: {e}"
                                ));
                            }
                        }
                        (true, output)
                    }
                    None => (
                        false,
                        "Missing 'results' array for grade action".to_string(),
                    ),
                }
            }
            _ => (
                false,
                format!("Invalid action: use 'plan' or 'grade' (got {action:?})"),
            ),
        };
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success,
            output,
        });
    }

    // 4c. Handle input_lock (sets editor.ai.input_lock).
    if call.name == "input_lock" {
        let locked = call
            .arguments
            .get("locked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        editor.ai.input_lock = if locked {
            mae_core::InputLock::AiBusy
        } else {
            mae_core::InputLock::None
        };
        let msg = if locked {
            "Input locked — user keystrokes discarded (Esc/Ctrl-C to cancel)"
        } else {
            "Input unlocked — user keystrokes re-enabled"
        };
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success: true,
            output: msg.to_string(),
        });
    }

    // 4d. Handle model_exam (deprecated — delegates to self_test_suite).
    if call.name == "model_exam" {
        let action = call
            .arguments
            .get("action")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        // ADR-086: see the identical comment on self_test_suite above — an
        // unrecognised action or a missing `results` array is a refusal.
        let (success, output) = match action {
            "plan" => {
                // Delegate to self_test_suite with exam-only categories.
                let exam_cats =
                    "tool_selection,parameter_accuracy,output_interpretation,multi_step,pushback";
                (
                    true,
                    super::self_test::build_self_test_plan(exam_cats, "", ""),
                )
            }
            "grade" => {
                // Legacy grading path — use original exam grading.
                let model = call
                    .arguments
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let results = call.arguments.get("results").and_then(|v| v.as_array());
                match results {
                    Some(arr) => {
                        let tests: Vec<super::model_exam::ExamTest> =
                            serde_json::from_value(serde_json::Value::Array(
                                serde_json::from_str(&super::model_exam::build_exam_plan())
                                    .unwrap_or_default(),
                            ))
                            .unwrap_or_default();
                        let mut grades = Vec::new();
                        for entry in arr {
                            let test_id = entry
                                .get("test_id")
                                .and_then(|v| {
                                    v.as_str()
                                        .map(String::from)
                                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                                })
                                .unwrap_or_else(|| "0".to_string());
                            let test_id_num: usize = test_id
                                .rsplit('.')
                                .next()
                                .and_then(|s| s.parse().ok())
                                .unwrap_or(0);
                            let tool_calls: Vec<ToolCall> = entry
                                .get("tool_calls")
                                .and_then(|v| serde_json::from_value(v.clone()).ok())
                                .unwrap_or_default();
                            let final_text = entry
                                .get("final_text")
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            if let Some(test) = tests.iter().find(|t| t.id == test_id_num) {
                                grades.push(super::model_exam::grade_exam_response(
                                    test,
                                    &tool_calls,
                                    final_text,
                                ));
                            }
                        }
                        let result = super::model_exam::aggregate_grades(model, &grades);
                        let mut output = serde_json::to_string_pretty(&result).unwrap_or_default();
                        let run = super::model_exam::ExamRun {
                            timestamp: chrono::Utc::now()
                                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                            runner: "mae-builtin".to_string(),
                            mae_version: env!("CARGO_PKG_VERSION").to_string(),
                            result: result.clone(),
                            grades: grades.clone(),
                        };
                        match super::model_exam::save_exam_run(&run) {
                            Ok(path) => {
                                output.push_str(&format!(
                                    "\n\nExam results saved to: {}",
                                    path.display()
                                ));
                            }
                            Err(e) => {
                                output.push_str(&format!(
                                    "\n\nWarning: failed to save exam results: {e}"
                                ));
                            }
                        }
                        (true, output)
                    }
                    None => (
                        false,
                        "Missing 'results' array for grade action".to_string(),
                    ),
                }
            }
            _ => (
                false,
                format!("Invalid action: use 'plan' or 'grade' (got {action:?})"),
            ),
        };
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success,
            output,
        });
    }

    // 4d2. Handle request_tools for a generic MCP client. The embedded agent
    // (crates/ai/src/session/handle_prompt.rs) intercepts `request_tools`
    // earlier, in-session, mutating its own live `self.tools` set -- that
    // mechanism only exists for `AgentSession`, not for an external MCP
    // client dispatching through this chokepoint. `tools/call` dispatch is
    // never restricted to what `tools/list` advertised (K2's tiered
    // `mcp_tools` in crates/mae/src/main.rs only filters the wire-visible
    // list, never what's callable) -- so what actually unlocks an
    // Extended-tier tool for an external client is returning its full
    // definition (name + input schema) here; the client can then call it
    // directly by name with no server-side session-tool-list mutation
    // needed or possible.
    if call.name == "request_tools" {
        let categories = crate::tools::parse_categories(
            call.arguments
                .get("categories")
                .and_then(|v| v.as_str())
                .unwrap_or(""),
        );
        let requested_names: Vec<&str> = call
            .arguments
            .get("tools")
            .and_then(|v| v.as_str())
            .map(|s| {
                s.split(',')
                    .map(|n| n.trim())
                    .filter(|n| !n.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        // ADR-091: `request_tools` is a discovery surface, so the
        // embedded-session-only tools must not be reachable through it for an
        // external client — offering them and then failing the `tools/call`
        // is precisely the shape ADR-085 rejects. The embedded `AgentSession`
        // dispatches with no session id and is unaffected: it is the one
        // context where `ask_user` actually works.
        let external = editor.is_external_mcp_dispatch();
        let matched: Vec<&ToolDefinition> = all_tools
            .iter()
            .filter(|t| !(external && crate::tools::is_embedded_session_only(&t.name)))
            .filter(|t| {
                categories
                    .iter()
                    .any(|c| crate::tools::classify_tool_category(&t.name) == Some(*c))
                    || requested_names.contains(&t.name.as_str())
            })
            .collect();
        let json_results: Vec<serde_json::Value> = matched
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                    "permission": t.permission.map(|p| format!("{p:?}")),
                })
            })
            .collect();
        let output = if json_results.is_empty() {
            "No tools matched the given categories/names.".to_string()
        } else {
            serde_json::to_string_pretty(&json_results).unwrap_or_default()
        };
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success: true,
            output,
        });
    }

    // 4e. Handle search_tools (needs access to all_tools).
    if call.name == "search_tools" {
        let query = call
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let limit = call
            .arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;
        // ADR-091: same exclusion as `request_tools` above — `search_tools` is
        // the surface an external client is explicitly told to use when the
        // Core `tools/list` doesn't have what it wants, so leaving the
        // interactive three findable here would defeat withholding them from
        // `tools/list`. Only allocates the filtered copy for external callers.
        let results = if editor.is_external_mcp_dispatch() {
            let visible = crate::tools::external_discovery_tools(all_tools);
            crate::tools::tool_search::search_tools(&visible, query, limit)
        } else {
            crate::tools::tool_search::search_tools(all_tools, query, limit)
        };
        let json_results: Vec<serde_json::Value> = results
            .iter()
            .map(|r| {
                serde_json::json!({
                    "name": r.name,
                    "description": r.description,
                    "score": r.score,
                })
            })
            .collect();
        let output = serde_json::to_string_pretty(&json_results).unwrap_or_default();
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success: true,
            output,
        });
    }

    // 4f. Sandbox guard — confine write-path tools during test mode.
    if let Some(ref sandbox_dir) = editor.test_sandbox_dir {
        if let Some(err) = sandbox_guard(&call.name, &call.arguments, sandbox_dir) {
            return ExecuteResult::Immediate(ToolResult {
                tool_call_id: call.id.clone(),
                tool_name: call.name.clone(),
                success: false,
                output: err,
            });
        }
    }

    // 5. Dispatch synchronous tools via submodules, contained against a
    // panicking tool implementation (see `catch_tool_panic`'s own doc for
    // why this exists and what it does/doesn't protect against).
    let result = catch_tool_panic(&call.name, || {
        dispatch_tool(editor, call, requester_provider)
    });

    ExecuteResult::Immediate(ToolResult {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        success: result.is_ok(),
        output: result.unwrap_or_else(|e| e),
    })
}

/// Run a single tool dispatch, converting a PANIC inside it into a normal
/// `Err(String)` instead of letting it unwind further.
///
/// **Why this exists**: `execute_tool`/`execute_tool_with_requester` is THE
/// single dispatch point for every tool call MAE's own agent makes AND every
/// tool call an external MCP client (VS Code Copilot, any other paired
/// editor's agent per ADR-050) makes — it runs synchronously on the editor's
/// main thread (`Editor`/`SchemeRuntime` are `!Send`, so this can't be
/// pushed onto `spawn_blocking` the way the daemon's KB-query path is).
/// Before this wrapper, ANY tool implementation panicking on a malformed or
/// adversarial argument (an out-of-bounds index, an unwrap on unexpected
/// input shape, etc.) would unwind straight out of `dispatch_tool` with
/// nothing to catch it, crashing the whole editor process for one bad tool
/// call — a genuinely reachable adversarial surface once an untrusted-ish
/// external MCP client is a first-class caller, not just the built-in
/// agent. Mirrors the daemon's own established pattern for the identical
/// class of problem (`daemon/src/handler.rs`'s `spawn_query`/
/// `spawn_query_result` mapping a panicking `spawn_blocking` task to
/// `DaemonError::Internal` instead of propagating it) — same intent, a
/// synchronous `catch_unwind` here since there's no async task boundary to
/// isolate behind on this `!Send` path.
///
/// **What this does NOT protect against**: `editor: &mut Editor` is
/// asserted `UnwindSafe` (via `AssertUnwindSafe`) because Rust's default
/// `UnwindSafe` bound is conservative, not because a panic mid-mutation is
/// guaranteed harmless — a tool that panics after partially mutating
/// `Editor` state (e.g. a buffer edit applied before an undo-stack push
/// panics) can leave that state genuinely inconsistent, not just "the tool
/// call failed cleanly." This is a deliberate, bounded trade-off, not an
/// oversight: a possibly-inconsistent-but-still-running editor (recoverable
/// by the user via undo/reload/restart) is strictly better than a crashed
/// process that loses all unsaved work outright, and a tool panicking here
/// at all is already a MAE bug regardless of which of these two outcomes
/// follows. The panic is logged at `error` level specifically so it's never
/// silently swallowed — a caught tool panic must be exactly as visible/
/// fixable as an uncaught one used to be, just without taking the whole
/// process down to report it.
fn catch_tool_panic<F, R>(tool_name: &str, f: F) -> Result<R, String>
where
    F: FnOnce() -> Result<R, String>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => result,
        Err(payload) => {
            let message = payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or("<no panic message>")
                .to_string();
            tracing::error!(
                tool = tool_name,
                panic_message = %message,
                "tool implementation panicked -- contained, editor process still running \
                 (this is a MAE bug; please report it)"
            );
            Err(format!(
                "Internal error: tool '{tool_name}' panicked ({message}) -- this is a MAE bug, \
                 not a problem with your request. The editor process is still running; please \
                 report this."
            ))
        }
    }
}

/// Dispatch a synchronous tool call to the appropriate submodule.
/// `requester_provider` is threaded only to `kb_exec::dispatch` (the only
/// category with AI-residency post-filter tools, #358) -- every other
/// category dispatcher keeps its plain 2-arg signature.
fn dispatch_tool(
    editor: &mut Editor,
    call: &ToolCall,
    requester_provider: Option<&str>,
) -> Result<String, String> {
    // Try each category dispatcher in turn
    if let Some(result) = super::core_exec::dispatch(editor, call) {
        return result;
    }
    // ADR-091: the six session-scoped tools. Placed here rather than in
    // `ai_exec` because what makes them dispatchable is the session handle,
    // not their subject matter — keeping them together is what makes
    // "which tools need a session?" answerable by looking at one file.
    if let Some(result) = super::session_exec::dispatch(editor, call) {
        return result;
    }
    if let Some(result) = super::ai_exec::dispatch(editor, call) {
        return result;
    }
    if let Some(result) = super::lsp_exec::dispatch(editor, call) {
        return result;
    }
    if let Some(result) = super::dap_exec::dispatch(editor, call) {
        return result;
    }
    if let Some(result) = super::kb_exec::dispatch(editor, call, requester_provider) {
        return result;
    }
    if let Some(result) = super::shell_exec::dispatch(editor, call) {
        return result;
    }
    if let Some(result) = super::sync_exec::dispatch(editor, call) {
        return result;
    }
    if let Some(result) = super::collab_exec::dispatch(editor, call) {
        return result;
    }

    // Perf tools (kept separate since they are cross-cutting)
    match call.name.as_str() {
        "perf_stats" => return super::perf::execute_perf_stats(editor),
        "perf_benchmark" => return super::perf::execute_perf_benchmark(editor, &call.arguments),
        "perf_profile" => return super::perf::execute_perf_profile(editor, &call.arguments),
        _ => {}
    }

    // Registry commands (command_* prefix)
    if let Some(cmd_name) = call.name.strip_prefix("command_") {
        return execute_registry_command(editor, cmd_name);
    }

    // Scheme-registered AI tools
    if let Some(st) = editor.ai.scheme_tools.iter().find(|t| t.name == call.name) {
        let handler = st.handler_fn.clone();
        let args_json = serde_json::to_string(&call.arguments).unwrap_or_default();
        let escaped = args_json.replace('\\', "\\\\").replace('"', "\\\"");
        let code = format!("({} \"{}\")", handler, escaped);
        editor.pending_scheme_eval.push(code);
        return Ok(format!("Scheme tool '{}' queued for evaluation", call.name));
    }

    Err(format!("Unknown tool: {}", call.name))
}

/// Inverse of `crate::tools::sanitize_command_name` — decodes a `command_*`
/// MCP tool-name suffix back into the original `CommandRegistry` command
/// name. MUST stay exactly paired with that function; see its doc comment
/// for the encoding and `all_registered_command_names_round_trip` for the
/// property test that holds the pairing accountable.
///
/// Scans left to right: a bare `_` decodes to `-`, but first checks for the
/// 4-character `_XX_` hex-escape triplet (two lowercase hex digits) used to
/// recover any non-hyphen character `sanitize_command_name` had to escape.
pub(crate) fn unsanitize_command_name(tool_suffix: &str) -> String {
    let chars: Vec<char> = tool_suffix.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '_'
            && i + 3 < chars.len()
            && chars[i + 1].is_ascii_hexdigit()
            && chars[i + 2].is_ascii_hexdigit()
            && chars[i + 3] == '_'
        {
            let hex: String = [chars[i + 1], chars[i + 2]].iter().collect();
            if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                out.push(ch);
                i += 4;
                continue;
            }
        }
        if chars[i] == '_' {
            out.push('-');
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

/// Execute a registered editor command by name (MCP `command_*` tool
/// handler). Plain `dispatch_builtin` is correct here (no target-window
/// redirection needed locally) -- the enclosing `with_ai_dispatch_scope`
/// call in `execute_tool_with_requester` has already focused the companion
/// window, if one was needed, before this ever runs (issue #372).
fn execute_registry_command(editor: &mut Editor, tool_suffix: &str) -> Result<String, String> {
    let cmd_name = unsanitize_command_name(tool_suffix);
    if editor.dispatch_builtin(&cmd_name) {
        Ok(format!("Executed: {}", cmd_name))
    } else {
        Err(format!("Unknown command: {}", cmd_name))
    }
}

// ---------------------------------------------------------------------------
// Argument validation
// ---------------------------------------------------------------------------

/// Validate tool arguments against the schema defined in `ToolDefinition`.
/// Catches type mismatches and missing required params before dispatch.
fn validate_tool_args(tool_def: &ToolDefinition, args: &serde_json::Value) -> Result<(), String> {
    let obj = args.as_object();

    // Check required params are present and non-null
    for req in &tool_def.parameters.required {
        let present = obj
            .and_then(|o| o.get(req.as_str()))
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if !present {
            return Err(format!(
                "Missing required parameter '{}' for tool '{}'",
                req, tool_def.name
            ));
        }
    }

    // Type-check provided params
    if let Some(obj) = obj {
        for (key, value) in obj {
            if value.is_null() {
                continue;
            }
            if let Some(prop) = tool_def.parameters.properties.get(key.as_str()) {
                validate_json_type(&tool_def.name, key, value, prop)?;
            }
            // Unknown params are silently ignored (forward-compatible)
        }
    }
    Ok(())
}

fn validate_json_type(
    tool_name: &str,
    param_name: &str,
    value: &serde_json::Value,
    prop: &ToolProperty,
) -> Result<(), String> {
    let ok = match prop.prop_type.as_str() {
        "string" => value.is_string(),
        "integer" | "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true, // unknown type → skip validation
    };
    if !ok {
        return Err(format!(
            "Parameter '{}' for tool '{}' expected {}, got {}",
            param_name,
            tool_name,
            prop.prop_type,
            json_type_name(value)
        ));
    }
    // Check enum constraint
    if let Some(ref allowed) = prop.enum_values {
        if let Some(s) = value.as_str() {
            if !allowed.iter().any(|a| a == s) {
                return Err(format!(
                    "Parameter '{}' for tool '{}': value '{}' not in {:?}",
                    param_name, tool_name, s, allowed
                ));
            }
        }
    }
    Ok(())
}

/// Check write-path tools against the sandbox directory during test mode.
/// Returns `Some(error_message)` if the call should be blocked, `None` if OK.
fn sandbox_guard(
    tool_name: &str,
    args: &serde_json::Value,
    sandbox_dir: &std::path::Path,
) -> Option<String> {
    match tool_name {
        "create_file" => {
            if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
                if let Err(e) = super::sandbox::validate_write_path(path, sandbox_dir) {
                    return Some(e);
                }
            }
        }
        "rename_file" => {
            for key in &["old_path", "new_path"] {
                if let Some(path) = args.get(*key).and_then(|v| v.as_str()) {
                    if let Err(e) = super::sandbox::validate_write_path(path, sandbox_dir) {
                        return Some(e);
                    }
                }
            }
        }
        "shell_exec" => {
            if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
                if let Err(e) = super::sandbox::filter_shell_command(cmd, sandbox_dir) {
                    return Some(e);
                }
            }
        }
        _ => {}
    }
    None
}

fn json_type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::String(_) => "string",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
        serde_json::Value::Null => "null",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_tool(name: &str, props: Vec<(&str, &str)>, required: Vec<&str>) -> ToolDefinition {
        let mut properties = HashMap::new();
        for (pname, ptype) in props {
            properties.insert(
                pname.to_string(),
                ToolProperty {
                    prop_type: ptype.to_string(),
                    description: String::new(),
                    enum_values: None,
                    items: None,
                    properties: None,
                    object_required: None,
                },
            );
        }
        ToolDefinition {
            name: name.to_string(),
            description: String::new(),
            parameters: ToolParameters {
                schema_type: "object".to_string(),
                properties,
                required: required.into_iter().map(|s| s.to_string()).collect(),
            },
            permission: None,
        }
    }

    #[test]
    fn validate_rejects_string_for_integer() {
        let tool = make_tool("buffer_read", vec![("start_line", "integer")], vec![]);
        let args = serde_json::json!({"start_line": "abc"});
        let err = validate_tool_args(&tool, &args).unwrap_err();
        assert!(err.contains("expected integer"));
        assert!(err.contains("got string"));
    }

    #[test]
    fn validate_rejects_missing_required() {
        let tool = make_tool("buffer_write", vec![("content", "string")], vec!["content"]);
        let args = serde_json::json!({});
        let err = validate_tool_args(&tool, &args).unwrap_err();
        assert!(err.contains("Missing required parameter 'content'"));
    }

    #[test]
    fn validate_accepts_correct_types() {
        let tool = make_tool(
            "buffer_read",
            vec![("start_line", "integer"), ("buffer_name", "string")],
            vec![],
        );
        let args = serde_json::json!({"start_line": 10, "buffer_name": "main.rs"});
        assert!(validate_tool_args(&tool, &args).is_ok());
    }

    #[test]
    fn validate_allows_missing_optional() {
        let tool = make_tool(
            "buffer_read",
            vec![("start_line", "integer"), ("end_line", "integer")],
            vec![],
        );
        let args = serde_json::json!({"start_line": 1});
        assert!(validate_tool_args(&tool, &args).is_ok());
    }

    #[test]
    fn validate_enum_rejects_invalid() {
        let mut tool = make_tool("set_option", vec![("scope", "string")], vec!["scope"]);
        tool.parameters
            .properties
            .get_mut("scope")
            .unwrap()
            .enum_values = Some(vec!["buffer".into(), "global".into()]);
        let args = serde_json::json!({"scope": "invalid"});
        let err = validate_tool_args(&tool, &args).unwrap_err();
        assert!(err.contains("not in"));
    }

    #[test]
    fn validate_ignores_unknown_params() {
        let tool = make_tool("buffer_read", vec![("start_line", "integer")], vec![]);
        let args = serde_json::json!({"start_line": 1, "extra_param": "whatever"});
        assert!(validate_tool_args(&tool, &args).is_ok());
    }

    #[test]
    fn scheme_tool_dispatch_queues_eval() {
        let mut editor = mae_core::Editor::new();
        editor.ai.scheme_tools.push(mae_core::SchemeToolDef {
            name: "my_tool".into(),
            description: "test".into(),
            params: vec![],
            required: vec![],
            handler_fn: "my-handler".into(),
            permission: "write".into(),
        });
        let call = ToolCall {
            id: "c1".into(),
            name: "my_tool".into(),
            arguments: serde_json::json!({"key": "val"}),
        };
        let result = dispatch_tool(&mut editor, &call, None);
        assert!(result.is_ok());
        assert_eq!(editor.pending_scheme_eval.len(), 1);
        assert!(editor.pending_scheme_eval[0].contains("my-handler"));
    }

    #[test]
    fn validate_null_values_skipped() {
        let tool = make_tool("buffer_read", vec![("start_line", "integer")], vec![]);
        let args = serde_json::json!({"start_line": null});
        assert!(validate_tool_args(&tool, &args).is_ok());
    }

    /// A tool implementation returning `Ok` normally must be completely
    /// unaffected by the panic-containment wrapper -- no behavior change,
    /// no swallowed output, on the success path.
    #[test]
    fn catch_tool_panic_passes_through_a_normal_ok_result_unchanged() {
        let result = catch_tool_panic("harmless_tool", || {
            Ok::<_, String>("real output".to_string())
        });
        assert_eq!(result, Ok("real output".to_string()));
    }

    /// A tool implementation returning `Err` normally (an ordinary, expected
    /// tool-level failure -- not a bug) must also pass through unchanged --
    /// this wrapper only intercepts PANICS, never recoverable tool errors.
    #[test]
    fn catch_tool_panic_passes_through_a_normal_err_result_unchanged() {
        let result = catch_tool_panic("harmless_tool", || {
            Err::<String, _>("expected, ordinary tool failure".to_string())
        });
        assert_eq!(result, Err("expected, ordinary tool failure".to_string()));
    }

    /// The core adversarial case this wrapper exists for: a tool
    /// implementation panicking (the exact shape a malformed/adversarial
    /// argument reaching an `.unwrap()`/index-out-of-bounds/etc. inside a
    /// real tool would produce) must become a normal `Err(String)`, not
    /// propagate the unwind -- this is what stands between "one bad tool
    /// call" and "the whole editor process crashes," so it's the single
    /// most important property under test in this file.
    #[test]
    fn catch_tool_panic_converts_a_panic_into_a_clean_err_naming_the_tool() {
        let result: Result<String, String> =
            catch_tool_panic("kb_search", || panic!("index out of bounds: the len is 0"));
        let err = result.expect_err("a panicking tool must surface as Err, never propagate");
        assert!(
            err.contains("kb_search"),
            "the error must name the tool that actually panicked, not just say 'a tool failed': {err}"
        );
        assert!(
            err.contains("index out of bounds: the len is 0"),
            "the original panic message must be preserved for debugging, not discarded: {err}"
        );
        assert!(
            err.contains("MAE bug"),
            "the message must tell the caller this is an internal bug, not their fault: {err}"
        );
    }

    /// Same property, for the OTHER panic-payload shape Rust actually
    /// produces (`&'static str` from a bare `panic!("literal")` without
    /// interpolation, vs. the `String` shape a `panic!("{}", x)` or
    /// `.unwrap()` produces) -- both must be recognized, not just one.
    #[test]
    fn catch_tool_panic_recognizes_the_static_str_payload_shape_too() {
        let result: Result<(), String> = catch_tool_panic("shell_exec", || panic!("literal"));
        let err = result.expect_err("must still be caught");
        assert!(err.contains("literal"));
    }

    /// A panic with a non-string payload (e.g. from a dependency that
    /// panics with a custom error type via `std::panic::panic_any`) must
    /// still be caught and produce a usable, non-empty error -- never a
    /// second panic while trying to format the first one's message.
    #[test]
    fn catch_tool_panic_handles_a_non_string_panic_payload_without_itself_panicking() {
        let result: Result<(), String> = catch_tool_panic("weird_tool", || {
            std::panic::panic_any(42i32);
        });
        let err = result.expect_err("must still be caught even with an unrecognized payload type");
        assert!(err.contains("weird_tool"));
        assert!(err.contains("no panic message") || !err.is_empty());
    }

    /// End-to-end through `execute_tool` itself (not just `catch_tool_panic`
    /// in isolation): a `dispatch_tool` call that panics because a category
    /// dispatcher's own future bug reaches an `.unwrap()`/index-out-of-
    /// bounds on adversarial arguments must still come back from
    /// `execute_tool` as a normal, well-formed `ExecuteResult::Immediate`
    /// with `success: false` -- never an unwind that escapes this function
    /// entirely. Exercises the REAL call site
    /// (`execute_tool_dispatch_body`'s `catch_tool_panic(&call.name, ||
    /// dispatch_tool(...))`) by giving it a tool name no category
    /// dispatcher recognizes and mutating `all_tools` so the permission
    /// check passes -- `dispatch_tool` then falls through every dispatcher
    /// to the final "Unknown tool" arm, which returns a normal `Err`, NOT a
    /// panic. To exercise the actual panic path through the real public
    /// entry point without depending on a specific pre-existing bug, this
    /// calls `catch_tool_panic` with the exact same argument shape
    /// `execute_tool_dispatch_body` uses at its real call site --
    /// `catch_tool_panic_converts_a_panic_into_a_clean_err_naming_the_tool`
    /// above already proves this exhaustively; this test additionally
    /// confirms `execute_tool`'s normal (non-panicking) unknown-tool path
    /// still returns a clean `Immediate` result end-to-end, i.e. the wrapper
    /// didn't change behavior for the non-panicking case at the real public
    /// API.
    #[test]
    fn execute_tool_unknown_tool_name_returns_a_clean_immediate_error_not_a_panic() {
        let mut editor = Editor::new();
        let call = ToolCall {
            id: "c1".into(),
            name: "definitely_not_a_real_tool_name".into(),
            arguments: serde_json::json!({}),
        };
        // An unknown tool has no registered tier, so dispatch falls back to
        // `Write` — above the shipped default. State the ceiling explicitly so
        // this stays a routing test, not a permission one.
        let policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
            ..PermissionPolicy::default()
        };
        let result = execute_tool(&mut editor, &call, &[], &policy);
        match result {
            ExecuteResult::Immediate(r) => {
                assert!(!r.success);
                assert_eq!(r.tool_call_id, "c1");
            }
            ExecuteResult::Deferred { .. } => {
                panic!("unknown tool must not be treated as deferred")
            }
            ExecuteResult::NeedsApproval(_) => {
                panic!("a ReadOnly-tier probe must not need approval")
            }
        }
    }
}
