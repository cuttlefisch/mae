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
    // 1. Find the tool definition
    let tool_def = all_tools.iter().find(|t| t.name == call.name);
    let permission = tool_def
        .and_then(|t| t.permission)
        .unwrap_or(PermissionTier::Write);

    // 2. Check permission
    if !policy.is_allowed(permission) {
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success: false,
            output: format!(
                "Permission denied: {} requires {:?} tier",
                call.name, permission
            ),
        });
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

    // 4b. Handle self_test_suite (returns structured test plan).
    // Auto-save editor state so it can be restored when the session completes.
    if call.name == "self_test_suite" {
        if !editor.self_test_active {
            editor.save_state();
            editor.self_test_active = true;
        }
        let filter = call
            .arguments
            .get("categories")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let output = super::self_test::build_self_test_plan(filter);
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success: true,
            output,
        });
    }

    // 4c. Handle input_lock (sets editor.input_lock).
    if call.name == "input_lock" {
        let locked = call
            .arguments
            .get("locked")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        editor.input_lock = if locked {
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

    // 5. Dispatch synchronous tools via submodules
    let result = dispatch_tool(editor, call);

    ExecuteResult::Immediate(ToolResult {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        success: result.is_ok(),
        output: result.unwrap_or_else(|e| e),
    })
}

/// Dispatch a synchronous tool call to the appropriate submodule.
fn dispatch_tool(editor: &mut Editor, call: &ToolCall) -> Result<String, String> {
    // Try each category dispatcher in turn
    if let Some(result) = super::core_exec::dispatch(editor, call) {
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
    if let Some(result) = super::kb_exec::dispatch(editor, call) {
        return result;
    }
    if let Some(result) = super::shell_exec::dispatch(editor, call) {
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

    Err(format!("Unknown tool: {}", call.name))
}

fn execute_registry_command(editor: &mut Editor, tool_suffix: &str) -> Result<String, String> {
    let cmd_name = tool_suffix.replace('_', "-");
    if editor.dispatch_builtin(&cmd_name) {
        Ok(format!("Executed: {}", cmd_name))
    } else {
        Err(format!("Unknown command: {}", cmd_name))
    }
}
