use mae_core::Editor;

use crate::tools::PermissionPolicy;
use crate::types::*;

use crate::tool_impls::lsp::{
    execute_lsp_definition, execute_lsp_document_symbols, execute_lsp_hover,
    execute_lsp_references, execute_lsp_workspace_symbol,
};
use crate::tool_impls::{
    execute_ai_load, execute_ai_save, execute_buffer_read, execute_buffer_write,
    execute_close_buffer, execute_command_list, execute_create_file, execute_create_plan,
    execute_cursor_info, execute_dap_continue, execute_dap_disconnect, execute_dap_evaluate,
    execute_dap_expand_variable, execute_dap_inspect_variable, execute_dap_list_variables,
    execute_dap_output, execute_dap_remove_breakpoint, execute_dap_select_frame,
    execute_dap_select_thread, execute_dap_set_breakpoint, execute_dap_start, execute_dap_step,
    execute_debug_state, execute_editor_state, execute_event_recording, execute_file_read,
    execute_get_option, execute_git_checkout, execute_git_commit, execute_git_diff,
    execute_git_log, execute_git_pull, execute_git_push, execute_git_stage, execute_git_status,
    execute_git_unstage, execute_github_pr_create, execute_github_pr_status, execute_help_open,
    execute_introspect, execute_kb_get, execute_kb_graph, execute_kb_links_from,
    execute_kb_links_to, execute_kb_list, execute_kb_search, execute_list_buffers,
    execute_lsp_diagnostics, execute_mouse_event, execute_open_file, execute_org_cycle,
    execute_org_open_link, execute_org_todo_cycle, execute_project_files, execute_project_info,
    execute_project_search, execute_rename_file, execute_render_inspect, execute_save_memory,
    execute_set_option, execute_shell_list, execute_shell_read_output, execute_shell_scrollback,
    execute_shell_send_input, execute_switch_buffer, execute_switch_project, execute_syntax_tree,
    execute_terminal_spawn, execute_theme_inspect, execute_trigger_hook, execute_update_plan,
    execute_visual_buffer_add_circle, execute_visual_buffer_add_line,
    execute_visual_buffer_add_rect, execute_visual_buffer_add_text, execute_visual_buffer_clear,
    execute_window_layout,
};

/// What kind of deferred LSP tool call is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeferredKind {
    LspDefinition,
    LspReferences,
    LspHover,
    LspWorkspaceSymbol,
    LspDocumentSymbols,
}

/// Result of executing a tool call — either immediately available or
/// deferred until an async response (e.g. from the LSP task) arrives.
#[derive(Debug)]
pub enum ExecuteResult {
    /// Tool completed synchronously.
    Immediate(ToolResult),
    /// Tool queued an async request (e.g. LSP). The caller must hold the
    /// reply channel open and complete it when the matching event arrives.
    Deferred {
        tool_call_id: String,
        kind: DeferredKind,
    },
}

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

    // 3. Check for deferred (async) tools first
    let deferred_kind = match call.name.as_str() {
        "lsp_definition" => Some(DeferredKind::LspDefinition),
        "lsp_references" => Some(DeferredKind::LspReferences),
        "lsp_hover" => Some(DeferredKind::LspHover),
        "lsp_workspace_symbol" => Some(DeferredKind::LspWorkspaceSymbol),
        "lsp_document_symbols" => Some(DeferredKind::LspDocumentSymbols),
        _ => None,
    };

    if let Some(kind) = deferred_kind {
        let result = match kind {
            DeferredKind::LspDefinition => execute_lsp_definition(editor, &call.arguments),
            DeferredKind::LspReferences => execute_lsp_references(editor, &call.arguments),
            DeferredKind::LspHover => execute_lsp_hover(editor, &call.arguments),
            DeferredKind::LspWorkspaceSymbol => {
                execute_lsp_workspace_symbol(editor, &call.arguments)
            }
            DeferredKind::LspDocumentSymbols => {
                execute_lsp_document_symbols(editor, &call.arguments)
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
        let output = format_permissions_info(policy);
        return ExecuteResult::Immediate(ToolResult {
            tool_call_id: call.id.clone(),
            tool_name: call.name.clone(),
            success: true,
            output,
        });
    }

    // 4b. Handle self_test_suite (returns structured test plan).
    if call.name == "self_test_suite" {
        let filter = call
            .arguments
            .get("categories")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let output = build_self_test_plan(filter);
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

    // 5. Dispatch synchronous tools
    let ai_tool_names = [
        "buffer_read",
        "buffer_write",
        "cursor_info",
        "file_read",
        "list_buffers",
        "editor_state",
        "window_layout",
        "command_list",
        "debug_state",
        "shell_exec",
        "open_file",
        "switch_buffer",
        "close_buffer",
        "create_file",
        "project_files",
        "project_info",
        "project_search",
        "switch_project",
        "lsp_diagnostics",
        "syntax_tree",
        "dap_start",
        "dap_set_breakpoint",
        "dap_continue",
        "dap_step",
        "dap_inspect_variable",
        "dap_remove_breakpoint",
        "dap_list_variables",
        "dap_expand_variable",
        "dap_select_frame",
        "dap_select_thread",
        "dap_output",
        "dap_evaluate",
        "dap_disconnect",
        "kb_get",
        "kb_search",
        "kb_list",
        "kb_links_from",
        "kb_links_to",
        "kb_graph",
        "help_open",
        "shell_list",
        "shell_read_output",
        "shell_send_input",
        "ai_permissions",
        "self_test_suite",
        "input_lock",
        "get_option",
        "set_option",
        "ai_save",
        "ai_load",
        "save_memory",
        "create_plan",
        "update_plan",
        "rename_file",
        "perf_stats",
        "perf_benchmark",
        "theme_inspect",
        "shell_scrollback",
        "terminal_spawn",
        "terminal_send",
        "terminal_read",
        // Note: session-level tools (ai_set_mode, ai_set_profile, ai_set_budget,
        // log_activity, request_tools) are intercepted in session.rs before reaching
        // the executor and are NOT listed here.
        "github_pr_status",
        "github_pr_create",
        "mouse_event",
        "render_inspect",
        "introspect",
    ];
    let result = if ai_tool_names.contains(&call.name.as_str()) {
        execute_ai_tool(editor, call)
    } else if let Some(cmd_name) = call.name.strip_prefix("command_") {
        execute_registry_command(editor, cmd_name)
    } else {
        execute_ai_tool(editor, call)
    };

    ExecuteResult::Immediate(ToolResult {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        success: result.is_ok(),
        output: result.unwrap_or_else(|e| e),
    })
}

fn execute_registry_command(editor: &mut Editor, tool_suffix: &str) -> Result<String, String> {
    let cmd_name = tool_suffix.replace('_', "-");
    if editor.dispatch_builtin(&cmd_name) {
        Ok(format!("Executed: {}", cmd_name))
    } else {
        Err(format!("Unknown command: {}", cmd_name))
    }
}

fn execute_ai_tool(editor: &mut Editor, call: &ToolCall) -> Result<String, String> {
    match call.name.as_str() {
        "buffer_read" => execute_buffer_read(editor, &call.arguments),
        "buffer_write" => execute_buffer_write(editor, &call.arguments),
        "cursor_info" => execute_cursor_info(editor),
        "file_read" => execute_file_read(&call.arguments),
        "list_buffers" => execute_list_buffers(editor),
        "editor_state" => execute_editor_state(editor),
        "window_layout" => execute_window_layout(editor),
        "command_list" => execute_command_list(editor),
        "debug_state" => execute_debug_state(editor),
        "open_file" => execute_open_file(editor, &call.arguments),
        "switch_buffer" => execute_switch_buffer(editor, &call.arguments),
        "close_buffer" => execute_close_buffer(editor, &call.arguments),
        "create_file" => execute_create_file(editor, &call.arguments),
        "project_files" => execute_project_files(&call.arguments),
        "project_info" => execute_project_info(editor),
        "project_search" => execute_project_search(&call.arguments),
        "switch_project" => execute_switch_project(editor, &call.arguments),
        "lsp_diagnostics" => execute_lsp_diagnostics(editor, &call.arguments),
        "syntax_tree" => execute_syntax_tree(editor, &call.arguments),
        "dap_start" => execute_dap_start(editor, &call.arguments),
        "dap_set_breakpoint" => execute_dap_set_breakpoint(editor, &call.arguments),
        "dap_continue" => execute_dap_continue(editor),
        "dap_step" => execute_dap_step(editor, &call.arguments),
        "dap_inspect_variable" => execute_dap_inspect_variable(editor, &call.arguments),
        "dap_remove_breakpoint" => execute_dap_remove_breakpoint(editor, &call.arguments),
        "dap_list_variables" => execute_dap_list_variables(editor),
        "dap_expand_variable" => execute_dap_expand_variable(editor, &call.arguments),
        "dap_select_frame" => execute_dap_select_frame(editor, &call.arguments),
        "dap_select_thread" => execute_dap_select_thread(editor, &call.arguments),
        "dap_output" => execute_dap_output(editor, &call.arguments),
        "dap_evaluate" => execute_dap_evaluate(editor, &call.arguments),
        "dap_disconnect" => execute_dap_disconnect(editor, &call.arguments),
        "kb_get" => execute_kb_get(editor, &call.arguments),
        "kb_search" => execute_kb_search(editor, &call.arguments),
        "kb_list" => execute_kb_list(editor, &call.arguments),
        "kb_links_from" => execute_kb_links_from(editor, &call.arguments),
        "kb_links_to" => execute_kb_links_to(editor, &call.arguments),
        "kb_graph" => execute_kb_graph(editor, &call.arguments),
        "help_open" => execute_help_open(editor, &call.arguments),
        "get_option" => execute_get_option(editor, &call.arguments),
        "set_option" => execute_set_option(editor, &call.arguments),
        "shell_list" => execute_shell_list(editor),
        "shell_read_output" => execute_shell_read_output(editor, &call.arguments),
        "shell_send_input" => execute_shell_send_input(editor, &call.arguments),
        "terminal_spawn" => execute_terminal_spawn(editor, &call.arguments),
        "terminal_send" => execute_shell_send_input(editor, &call.arguments),
        "terminal_read" => execute_shell_read_output(editor, &call.arguments),
        "github_pr_status" => execute_github_pr_status(editor),
        "github_pr_create" => execute_github_pr_create(editor, &call.arguments),
        "shell_exec" => execute_shell_exec_sync(&call.arguments),
        "ai_save" => execute_ai_save(editor, &call.arguments),
        "ai_load" => execute_ai_load(editor, &call.arguments),
        "save_memory" => execute_save_memory(&call.arguments),
        "create_plan" => execute_create_plan(&call.arguments),
        "update_plan" => execute_update_plan(&call.arguments),
        "rename_file" => execute_rename_file(editor, &call.arguments),
        "perf_stats" => execute_perf_stats(editor),
        "perf_benchmark" => execute_perf_benchmark(editor, &call.arguments),
        "theme_inspect" => execute_theme_inspect(editor, &call.arguments),
        "shell_scrollback" => execute_shell_scrollback(editor, &call.arguments),
        "mouse_event" => execute_mouse_event(editor, &call.arguments),
        "render_inspect" => execute_render_inspect(editor, &call.arguments),
        "introspect" => execute_introspect(editor, &call.arguments),
        "trigger_hook" => execute_trigger_hook(editor, &call.arguments),
        "visual_buffer_add_rect" => execute_visual_buffer_add_rect(editor, &call.arguments),
        "visual_buffer_add_line" => execute_visual_buffer_add_line(editor, &call.arguments),
        "visual_buffer_add_circle" => execute_visual_buffer_add_circle(editor, &call.arguments),
        "visual_buffer_add_text" => execute_visual_buffer_add_text(editor, &call.arguments),
        "visual_buffer_clear" => execute_visual_buffer_clear(editor),
        "org_cycle" => execute_org_cycle(editor),
        "org_todo_cycle" => execute_org_todo_cycle(editor, &call.arguments),
        "org_open_link" => execute_org_open_link(editor),
        "event_recording" => execute_event_recording(editor, &call.arguments),

        // --- Git operations ---
        "git_status" => execute_git_status(editor),
        "git_diff" => execute_git_diff(editor, &call.arguments),
        "git_log" => execute_git_log(editor, &call.arguments),
        "git_stage" => execute_git_stage(editor, &call.arguments),
        "git_unstage" => execute_git_unstage(editor, &call.arguments),
        "git_commit" => execute_git_commit(editor, &call.arguments),
        "git_push" => execute_git_push(editor, &call.arguments),
        "git_pull" => execute_git_pull(editor, &call.arguments),
        "git_checkout" => execute_git_checkout(editor, &call.arguments),

        _ => Err(format!("Unknown tool: {}", call.name)),
    }
}

/// Synchronous shell_exec for MCP callers and other non-session paths.
///
/// The AI session handles shell_exec async (see `AgentSession::execute_shell`),
/// but MCP tool calls bypass the session and go through `execute_tool` directly.
/// This synchronous version uses `std::process::Command` so MCP agents can
/// run shell commands without the async session context.
fn execute_shell_exec_sync(args: &serde_json::Value) -> Result<String, String> {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");

    if command.is_empty() {
        return Err("Missing 'command' argument".into());
    }

    // Same blocklist as session's async version.
    let blocked_patterns = ["rm -rf /", "rm -fr /", "mkfs.", "dd if=", ":(){", ">(){ :"];
    for pattern in &blocked_patterns {
        if command.contains(pattern) {
            return Err(format!(
                "Command blocked: contains dangerous pattern '{}'",
                pattern
            ));
        }
    }

    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(30)
        .min(120);

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to execute: {}", e))?;

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let start = std::time::Instant::now();
    let output = loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                break child
                    .wait_with_output()
                    .map_err(|e| format!("Wait failed: {}", e))?
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    return Err(format!("Command timed out after {}s", timeout_secs));
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(format!("Wait failed: {}", e)),
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let status = output.status.code().unwrap_or(-1);

    let mut out = format!("exit_code: {}\n", status);
    if !stdout.is_empty() {
        let stdout_str = if stdout.len() > 10_000 {
            format!("{}...[truncated]", &stdout[..10_000])
        } else {
            stdout.to_string()
        };
        out.push_str(&format!("stdout:\n{}\n", stdout_str));
    }
    if !stderr.is_empty() {
        let stderr_str = if stderr.len() > 5_000 {
            format!("{}...[truncated]", &stderr[..5_000])
        } else {
            stderr.to_string()
        };
        out.push_str(&format!("stderr:\n{}\n", stderr_str));
    }

    if output.status.success() {
        Ok(out)
    } else {
        Err(out)
    }
}

fn format_permissions_info(policy: &PermissionPolicy) -> String {
    let tier_name = match policy.auto_approve_up_to {
        PermissionTier::ReadOnly => "readonly",
        PermissionTier::Write => "standard",
        PermissionTier::Shell => "trusted",
        PermissionTier::Privileged => "full",
    };

    format!(
        "Current auto-approve tier: {tier_name}\n\n\
         Permission tiers (lowest to highest):\n\
         - readonly: Read buffer contents, cursor state, file listings, project search\n\
         - standard: Modify buffers, edit files, save, undo/redo\n\
         - trusted: Execute shell commands (default)\n\
         - full: Quit editor, modify config, privileged operations\n\n\
         Tools at or below the '{tier_name}' tier run without prompting.\n\
         Configure via MAE_AI_PERMISSIONS env var or [ai] auto_approve_tier in config.toml.\n\
         Agent tool approval (MCP) is separate — see [agents] auto_approve_tools in config.toml."
    )
}

fn execute_perf_stats(editor: &mut Editor) -> Result<String, String> {
    editor.perf_stats.sample_process_stats();
    let buffer_count = editor.buffers.len();
    let total_lines: usize = editor.buffers.iter().map(|b| b.line_count()).sum();
    let stats = serde_json::json!({
        "rss_bytes": editor.perf_stats.rss_bytes,
        "cpu_percent": editor.perf_stats.cpu_percent,
        "frame_time_us": editor.perf_stats.frame_time_us,
        "avg_frame_time_us": editor.perf_stats.avg_frame_time_us,
        "buffer_count": buffer_count,
        "total_lines": total_lines,
        "debug_mode": editor.debug_mode,
    });
    Ok(serde_json::to_string_pretty(&stats).unwrap())
}

fn execute_perf_benchmark(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let benchmark = args
        .get("benchmark")
        .and_then(|v| v.as_str())
        .unwrap_or("buffer_insert");
    let size = args.get("size").and_then(|v| v.as_u64()).unwrap_or(1000) as usize;

    let (duration_us, ops_per_sec) = match benchmark {
        "buffer_insert" => {
            let mut buf = mae_core::Buffer::new();
            let start = std::time::Instant::now();
            let mut win = mae_core::WindowManager::new(0);
            for i in 0..size {
                let line = format!("line {} — benchmark test content\n", i);
                for ch in line.chars() {
                    buf.insert_char(win.focused_window_mut(), ch);
                }
            }
            let elapsed = start.elapsed().as_micros() as u64;
            let ops = if elapsed > 0 {
                (size as f64 / (elapsed as f64 / 1_000_000.0)) as u64
            } else {
                0
            };
            (elapsed, ops)
        }
        "buffer_delete" => {
            // Set up a buffer with `size` lines, then measure deletion.
            let mut buf = mae_core::Buffer::new();
            let mut win = mae_core::WindowManager::new(0);
            for i in 0..size {
                let line = format!("line {} — content to delete\n", i);
                for ch in line.chars() {
                    buf.insert_char(win.focused_window_mut(), ch);
                }
            }
            let start = std::time::Instant::now();
            for _ in 0..size {
                if buf.line_count() > 1 {
                    win.focused_window_mut().cursor_row = 0;
                    win.focused_window_mut().cursor_col = 0;
                    buf.delete_line(win.focused_window_mut());
                }
            }
            let elapsed = start.elapsed().as_micros() as u64;
            let ops = if elapsed > 0 {
                (size as f64 / (elapsed as f64 / 1_000_000.0)) as u64
            } else {
                0
            };
            (elapsed, ops)
        }
        "syntax_parse" => {
            // Generate synthetic Rust source and parse it.
            let mut source = String::new();
            for i in 0..size {
                source.push_str(&format!("fn func_{}(x: i32) -> i32 {{ x + {} }}\n", i, i));
            }
            let start = std::time::Instant::now();
            let mut syntax_map = mae_core::syntax::SyntaxMap::new();
            syntax_map.set_language(0, mae_core::syntax::Language::Rust);
            let _ = syntax_map.spans_for(0, &source);
            let elapsed = start.elapsed().as_micros() as u64;
            let ops = if elapsed > 0 {
                (size as f64 / (elapsed as f64 / 1_000_000.0)) as u64
            } else {
                0
            };
            (elapsed, ops)
        }
        _ => return Err(format!("Unknown benchmark type: {}", benchmark)),
    };

    let _ = editor; // satisfy borrow checker
    let result = serde_json::json!({
        "benchmark": benchmark,
        "size": size,
        "duration_us": duration_us,
        "ops_per_sec": ops_per_sec,
    });
    Ok(serde_json::to_string_pretty(&result).unwrap())
}

/// Build a structured JSON test plan for the self-test suite.
///
/// Returns a JSON object that any MCP-connected agent can parse and execute
/// mechanically — no prose interpretation required.
fn build_self_test_plan(filter: &str) -> String {
    let filters: Vec<&str> = if filter.is_empty() {
        vec![]
    } else {
        filter.split(',').map(|s| s.trim()).collect()
    };
    let include = |name: &str| filters.is_empty() || filters.contains(&name);

    let mut categories = Vec::new();

    if include("introspection") {
        categories.push(serde_json::json!({
            "name": "introspection",
            "conditional": false,
            "tests": [
                {
                    "tool": "cursor_info",
                    "args": {},
                    "assert": "Returns JSON with cursor_row, mode, line_count fields"
                },
                {
                    "tool": "editor_state",
                    "args": {},
                    "assert": "Returns JSON with mode, theme, buffer_count >= 1"
                },
                {
                    "tool": "list_buffers",
                    "args": {},
                    "assert": "Returns at least 1 buffer"
                },
                {
                    "tool": "window_layout",
                    "args": {},
                    "assert": "Returns JSON with at least 1 window"
                },
                {
                    "tool": "command_list",
                    "args": {},
                    "assert": "Returns >= 30 commands; must include: save, quit, help, terminal, agent-list, agent-setup, self-test, lsp-goto-definition, debug-start, ai-prompt"
                },
                {
                    "tool": "ai_permissions",
                    "args": {},
                    "assert": "Returns text with current auto-approve tier"
                }
            ],
            "command_palette_check": {
                "description": "After running command_list, verify these commands exist",
                "required_commands": [
                    "save", "quit", "force-quit", "undo", "redo",
                    "help", "help-follow-link", "help-search",
                    "ai-prompt", "ai-cancel",
                    "terminal", "send-to-shell",
                    "agent-list", "agent-setup", "self-test",
                    "lsp-goto-definition", "lsp-find-references", "lsp-hover", "lsp-show-diagnostics",
                    "debug-start", "debug-stop", "debug-continue", "debug-toggle-breakpoint",
                    "debug-panel"
                ]
            }
        }));
    }

    if include("editing") {
        categories.push(serde_json::json!({
            "name": "editing",
            "conditional": false,
            "tests": [
                {
                    "tool": "create_file",
                    "args": {"path": "/tmp/mae-self-test-editing.txt", "content": "hello world"},
                    "assert": "Success, file created"
                },
                {
                    "tool": "open_file",
                    "args": {"path": "/tmp/mae-self-test-editing.txt"},
                    "assert": "Buffer opened"
                },
                {
                    "tool": "buffer_read",
                    "args": {"start_line": 1, "end_line": 1},
                    "assert": "Contains 'hello world'"
                },
                {
                    "tool": "buffer_write",
                    "args": {"start_line": 1, "end_line": 1, "content": "hello MAE"},
                    "assert": "Success"
                },
                {
                    "tool": "buffer_read",
                    "args": {"start_line": 1, "end_line": 1},
                    "assert": "Contains 'hello MAE' (verifies write)"
                },
                {
                    "tool": "list_buffers",
                    "args": {},
                    "assert": "Test file buffer appears in list"
                },
                {
                    "tool": "switch_buffer",
                    "args": {"name": "*AI*"},
                    "assert": "Success"
                },
                {
                    "tool": "close_buffer",
                    "args": {"name": "mae-self-test-editing.txt", "force": true},
                    "assert": "Success (force=true closes even if modified)"
                }
            ]
        }));
    }

    if include("help") {
        categories.push(serde_json::json!({
            "name": "help",
            "conditional": false,
            "tests": [
                {
                    "tool": "kb_search",
                    "args": {"query": "buffer"},
                    "assert": "Returns at least 1 result"
                },
                {
                    "tool": "kb_list",
                    "args": {"prefix": "concept:"},
                    "assert": "Returns at least 5 concept nodes"
                },
                {
                    "tool": "kb_get",
                    "args": {"id": "concept:buffer"},
                    "assert": "Returns node with title, body, links"
                },
                {
                    "tool": "kb_links_from",
                    "args": {"id": "concept:buffer"},
                    "assert": "Returns at least 1 outgoing link"
                },
                {
                    "tool": "kb_links_to",
                    "args": {"id": "concept:buffer"},
                    "assert": "Returns at least 1 incoming link (from index)"
                },
                {
                    "tool": "kb_graph",
                    "args": {"id": "concept:buffer", "depth": 1},
                    "assert": "Returns nodes and edges arrays"
                },
                {
                    "tool": "help_open",
                    "args": {"id": "index"},
                    "assert": "Opens *Help* buffer for the user"
                },
                {
                    "tool": "switch_buffer",
                    "args": {"name": "*AI*"},
                    "assert": "Switch back to *AI* after help tests (important: subsequent tests need a non-Help buffer active)"
                }
            ],
            "help_navigation_e2e": {
                "description": "Connected walkthrough: kb_search 'buffer' -> kb_get first result -> verify [[...]] links in body -> kb_graph on that node (>= 2 nodes) -> help_open index -> switch_buffer back to *AI*",
                "report_as": "help_navigation_e2e"
            }
        }));
    }

    if include("project") {
        categories.push(serde_json::json!({
            "name": "project",
            "conditional": true,
            "precondition": "Call project_info first. If it fails or returns no root, SKIP this entire category.",
            "tests": [
                {
                    "tool": "project_info",
                    "args": {},
                    "assert": "Returns JSON with root field"
                },
                {
                    "tool": "project_files",
                    "args": {"pattern": "*.rs"},
                    "assert": "Returns at least 1 file"
                },
                {
                    "tool": "project_search",
                    "args": {"pattern": "fn main", "max_results": 5},
                    "assert": "Returns at least 1 match"
                }
            ]
        }));
    }

    if include("lsp") {
        categories.push(serde_json::json!({
            "name": "lsp",
            "conditional": true,
            "precondition": "First call project_info. If no root, SKIP. Then call open_file with path 'crates/mae/src/main.rs' (relative to project root) to trigger LSP didOpen. Wait a moment, then call lsp_diagnostics with scope 'all'. If it returns an error about no LSP server, SKIP this entire category.",
            "tests": [
                {
                    "tool": "open_file",
                    "args": {"path": "crates/mae/src/main.rs"},
                    "assert": "Buffer opened (triggers LSP didOpen)"
                },
                {
                    "tool": "lsp_diagnostics",
                    "args": {"scope": "all"},
                    "assert": "Returns JSON (may be empty diagnostics)"
                },
                {
                    "tool": "lsp_document_symbols",
                    "args": {},
                    "assert": "Returns symbol list or error"
                }
            ],
            "cleanup": [
                "Close the main.rs buffer with close_buffer (name: 'main.rs')",
                "Switch back to *AI* buffer"
            ]
        }));
    }

    if include("performance") {
        categories.push(serde_json::json!({
            "name": "performance",
            "conditional": false,
            "tests": [
                {
                    "tool": "perf_stats",
                    "args": {},
                    "assert": "Returns JSON with rss_bytes field"
                },
                {
                    "tool": "perf_benchmark",
                    "args": {"benchmark": "buffer_insert", "size": 10000},
                    "assert": "duration_us < 50000"
                },
                {
                    "tool": "perf_benchmark",
                    "args": {"benchmark": "syntax_parse", "size": 1000},
                    "assert": "duration_us < 100000"
                }
            ]
        }));
    }

    if include("tool_callstack") {
        categories.push(serde_json::json!({
            "name": "tool_callstack",
            "conditional": false,
            "tests": [
                {
                    "tool": "introspect",
                    "args": {"section": "ai"},
                    "assert": "Returns JSON with current_round and transaction_start_idx"
                },
                {
                    "tool": "cursor_info",
                    "args": {},
                    "assert": "Verify round increments after this tool call (AI should check introspect again)"
                }
            ],
            "e2e_compression_check": {
                "description": "After this turn ends, the next turn should see a smaller message history due to compression",
                "procedure": "1. Run self-test. 2. Finish turn. 3. Start new turn. 4. Check introspect conversation_entries."
            }
        }));
    }

    if include("dap") {
        categories.push(serde_json::json!({
            "name": "dap",
            "conditional": true,
            "precondition": "Call debug_state first. If it returns 'No active debug session', run dap_start with adapter='lldb' and program='/bin/ls' (or skip if lldb unavailable). If dap_start fails, SKIP this entire category.",
            "tests": [
                {
                    "tool": "debug_state",
                    "args": {},
                    "assert": "Returns JSON with 'target' and 'active_thread_id' fields (or 'No active debug session' before start)"
                },
                {
                    "tool": "dap_set_breakpoint",
                    "args": {"source": "/tmp/mae-self-test-dap.rs", "line": 1},
                    "assert": "Returns JSON with 'all_lines_for_source' containing [1]"
                },
                {
                    "tool": "dap_list_variables",
                    "args": {},
                    "assert": "Returns JSON object (may be empty scopes if not stopped at breakpoint)"
                },
                {
                    "tool": "dap_output",
                    "args": {"lines": 10},
                    "assert": "Returns JSON with 'total_lines', 'returned_lines', 'output' fields"
                },
                {
                    "tool": "dap_remove_breakpoint",
                    "args": {"source": "/tmp/mae-self-test-dap.rs", "line": 1},
                    "assert": "Returns JSON with 'remaining_lines' as empty array"
                },
                {
                    "tool": "dap_continue",
                    "args": {},
                    "assert": "Returns 'continue' or error 'not stopped' (both acceptable)"
                }
            ],
            "cleanup": [
                "Call command_ debug-stop to tear down the session",
                "Switch back to *AI* buffer"
            ]
        }));
    }

    if include("git") {
        categories.push(serde_json::json!({
            "name": "git",
            "conditional": true,
            "precondition": "Call git_status first. If it fails or returns error, SKIP this entire category.",
            "tests": [
                {
                    "tool": "git_status",
                    "args": {},
                    "assert": "Returns JSON with branch, staged, unstaged, untracked arrays"
                },
                {
                    "tool": "git_log",
                    "args": {"limit": 3},
                    "assert": "Returns at least 1 log entry (if in a valid repo)"
                },
                {
                    "tool": "git_diff",
                    "args": {},
                    "assert": "Returns a diff string (may be empty)"
                },
                {
                    "tool": "github_pr_status",
                    "args": {},
                    "assert": "Executes successfully (even if no PR exists, it should return a structured response from the gh cli)"
                }
            ]
        }));
    }

    if include("hooks") {
        categories.push(serde_json::json!({
            "name": "hooks",
            "conditional": false,
            "tests": [
                {
                    "tool": "trigger_hook",
                    "args": {"hook_name": "buffer-open"},
                    "assert": "Hook 'buffer-open' triggered"
                },
                {
                    "tool": "trigger_hook",
                    "args": {"hook_name": "app-start"},
                    "assert": "Hook 'app-start' triggered"
                }
            ]
        }));
    }

    let plan = serde_json::json!({
        "version": 1,
        "description": "MAE self-test plan. Call each tool with the given args, check the assertion, report [PASS]/[FAIL]/[SKIP] per test.",
        "output_format": "=== MAE Self-Test Report ===\nCategory: <name>\n  [PASS] <tool> -- <what was verified>\n  [FAIL] <tool> -- expected <X>, got <Y>\n  [SKIP] <tool> -- <reason>\n\nSummary: N passed, N failed, N skipped",
        "cleanup": [
            "Close any test buffers opened (close_buffer with name 'mae-self-test-editing.txt')",
            "Delete test files via shell_exec: rm -f /tmp/mae-self-test-editing.txt",
            "Switch back to *AI* buffer (switch_buffer) so the user sees the report",
            "Do NOT quit the editor",
            "Do NOT close the *AI* or *Help* buffers"
        ],
        "categories": categories
    });

    serde_json::to_string_pretty(&plan).unwrap_or_else(|_| "{}".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{ai_specific_tools, tools_from_registry};
    fn make_editor_with_text(text: &str) -> Editor {
        let mut editor = Editor::new();
        for ch in text.chars() {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, ch);
        }
        editor
    }

    fn all_tools() -> Vec<ToolDefinition> {
        let mut tools = tools_from_registry(&mae_core::CommandRegistry::with_builtins());
        tools.extend(ai_specific_tools(&mae_core::OptionRegistry::new()));
        tools
    }

    fn unwrap_immediate(result: ExecuteResult) -> ToolResult {
        match result {
            ExecuteResult::Immediate(r) => r,
            ExecuteResult::Deferred { .. } => ToolResult {
                tool_call_id: "deferred".into(),
                tool_name: "deferred".into(),
                success: true,
                output: "deferred".into(),
            },
        }
    }

    fn make_call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: "test_call".into(),
            name: name.into(),
            arguments: args,
        }
    }

    #[test]
    fn buffer_read_full() {
        let mut editor = make_editor_with_text("hello\nworld\n");
        let call = make_call("buffer_read", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(result.output.contains("hello"));
        assert!(result.output.contains("world"));
    }

    #[test]
    fn buffer_read_range() {
        let mut editor = make_editor_with_text("aaa\nbbb\nccc\n");
        let call = make_call(
            "buffer_read",
            serde_json::json!({"start_line": 2, "end_line": 2}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(result.output.contains("bbb"));
        assert!(!result.output.contains("aaa"));
        assert!(!result.output.contains("ccc"));
    }

    #[test]
    fn buffer_read_empty() {
        let mut editor = Editor::new();
        let call = make_call("buffer_read", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
    }

    #[test]
    fn cursor_info_returns_json() {
        let mut editor = make_editor_with_text("hello");
        let call = make_call("cursor_info", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let info: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert!(info["cursor_row"].is_number());
        assert!(info["line_count"].is_number());
    }

    #[test]
    fn registry_command_move_down() {
        let mut editor = make_editor_with_text("line1\nline2\n");
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.window_mgr.focused_window_mut().cursor_col = 0;
        let call = make_call("command_move_down", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
    }

    #[test]
    fn registry_command_unknown() {
        let mut editor = Editor::new();
        let call = make_call("command_nonexistent", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(!result.success);
        assert!(result.output.contains("Unknown command"));
    }

    #[test]
    fn permission_denied_for_privileged() {
        let mut editor = Editor::new();
        let call = make_call("command_quit", serde_json::json!({}));
        let policy = PermissionPolicy::default(); // allows up to Shell
        let result = unwrap_immediate(execute_tool(&mut editor, &call, &all_tools(), &policy));
        assert!(!result.success);
        assert!(result.output.contains("Permission denied"));
    }

    #[test]
    fn unknown_tool_returns_error() {
        let mut editor = Editor::new();
        let call = make_call("totally_fake_tool", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(!result.success);
        assert!(result.output.contains("Unknown tool"));
    }

    #[test]
    fn list_buffers_returns_metadata() {
        let mut editor = Editor::new();
        let call = make_call("list_buffers", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let buffers: Vec<serde_json::Value> = serde_json::from_str(&result.output).unwrap();
        assert_eq!(buffers.len(), 1);
        assert_eq!(buffers[0]["name"], "[scratch]");
        assert_eq!(buffers[0]["active"], true);
    }

    #[test]
    fn buffer_write_insert() {
        let mut editor = make_editor_with_text("line1\nline2\n");
        let call = make_call(
            "buffer_write",
            serde_json::json!({"start_line": 1, "content": "new\n"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let text = editor.active_buffer().text();
        assert!(text.starts_with("new\n"));
    }

    #[test]
    fn buffer_write_replace() {
        let mut editor = make_editor_with_text("aaa\nbbb\nccc\n");
        let call = make_call(
            "buffer_write",
            serde_json::json!({"start_line": 2, "end_line": 2, "content": "XXX\n"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let text = editor.active_buffer().text();
        assert!(text.contains("XXX"));
        assert!(!text.contains("bbb"));
    }

    #[test]
    fn editor_state_returns_valid_json() {
        let mut editor = Editor::new();
        let call = make_call("editor_state", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let info: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert!(info["buffer_count"].is_number());
        assert!(info["window_count"].is_number());
        assert_eq!(info["active_buffer"], "[scratch]");
        assert_eq!(info["debug_session_active"], false);
    }

    #[test]
    fn window_layout_returns_valid_json() {
        let mut editor = Editor::new();
        let call = make_call("window_layout", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let windows: Vec<serde_json::Value> = serde_json::from_str(&result.output).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0]["buffer_name"], "[scratch]");
    }

    #[test]
    fn command_list_includes_expected_commands() {
        let mut editor = Editor::new();
        let call = make_call("command_list", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "command_list failed: {}", result.output);
        let commands: Vec<serde_json::Value> = serde_json::from_str(&result.output).unwrap();
        let names: Vec<&str> = commands
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"save"));
        assert!(names.contains(&"move-up"));
        assert!(names.contains(&"undo"));
    }

    #[test]
    fn debug_state_no_session() {
        let mut editor = Editor::new();
        let call = make_call("debug_state", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert_eq!(result.output, "No active debug session");
    }

    #[test]
    fn debug_state_with_self_debug() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("debug-self");
        let call = make_call("debug_state", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let info: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(info["target"], "SelfDebug");
        assert!(info["threads"].is_array());
        assert!(info["stack_frames"].is_array());
    }

    #[test]
    fn file_read_temp_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("mae_test_file_read.txt");
        std::fs::write(&path, "hello\nworld\n").unwrap();

        let mut editor = Editor::new();
        let call = make_call(
            "file_read",
            serde_json::json!({"path": path.to_str().unwrap()}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(result.output.contains("hello"));
        assert!(result.output.contains("world"));

        std::fs::remove_file(&path).ok();
    }

    // --- Phase 3f M1: Multi-buffer AI tools ---

    #[test]
    fn open_file_creates_buffer() {
        let dir = std::env::temp_dir();
        let path = dir.join("mae_test_open_file.txt");
        std::fs::write(&path, "line1\nline2\n").unwrap();

        let mut editor = Editor::new();
        let call = make_call(
            "open_file",
            serde_json::json!({"path": path.to_str().unwrap()}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "open_file failed: {}", result.output);
        assert_eq!(editor.buffers.len(), 2);
        let target_idx = editor.ai_target_buffer_idx.expect("should have AI target");
        assert!(editor.buffers[target_idx].text().contains("line1"));

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn open_file_deduplicates() {
        let dir = std::env::temp_dir();
        let path = dir.join("mae_test_open_dedup.txt");
        std::fs::write(&path, "content\n").unwrap();

        let mut editor = Editor::new();
        // Open twice
        let call = make_call(
            "open_file",
            serde_json::json!({"path": path.to_str().unwrap()}),
        );
        unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(result.output.contains("already open"));
        assert_eq!(editor.buffers.len(), 2); // scratch + the file, not 3

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn switch_buffer_by_name() {
        let mut editor = Editor::new();
        let mut b = mae_core::Buffer::new();
        b.name = "second".into();
        editor.buffers.push(b);

        let call = make_call("switch_buffer", serde_json::json!({"name": "second"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let target_idx = editor.ai_target_buffer_idx.expect("should have AI target");
        assert_eq!(editor.buffers[target_idx].name, "second");
    }

    #[test]
    fn switch_buffer_nonexistent() {
        let mut editor = Editor::new();
        let call = make_call("switch_buffer", serde_json::json!({"name": "nope"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(!result.success);
        assert!(result.output.contains("No buffer named"));
    }

    #[test]
    fn close_buffer_by_name() {
        let mut editor = Editor::new();
        let mut b = mae_core::Buffer::new();
        b.name = "tobeclosed".into();
        editor.buffers.push(b);
        assert_eq!(editor.buffers.len(), 2);

        let call = make_call("close_buffer", serde_json::json!({"name": "tobeclosed"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "close_buffer failed: {}", result.output);
        assert_eq!(editor.buffers.len(), 1);
    }

    #[test]
    fn close_buffer_modified_fails() {
        let mut editor = Editor::new();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'x');

        let call = make_call("close_buffer", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(!result.success);
        assert!(result.output.contains("unsaved"));
    }

    #[test]
    fn close_buffer_modified_with_force() {
        let mut editor = Editor::new();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'x');
        assert!(editor.buffers[0].modified);

        // With force=true, close should succeed even though buffer is modified
        let call = make_call("close_buffer", serde_json::json!({"force": true}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(
            result.success,
            "close_buffer with force failed: {}",
            result.output
        );
    }

    #[test]
    fn self_test_suite_lsp_has_open_file() {
        let mut editor = Editor::new();
        let call = make_call("self_test_suite", serde_json::json!({"categories": "lsp"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let plan: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let cats = plan["categories"].as_array().unwrap();
        let lsp_cat = &cats[0];
        let tests = lsp_cat["tests"].as_array().unwrap();
        // First test should be open_file to trigger LSP didOpen
        assert_eq!(tests[0]["tool"], "open_file");
    }

    #[test]
    fn ai_save_load_rename_tools_exist() {
        let tools = ai_specific_tools(&mae_core::OptionRegistry::new());
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(
            names.contains(&"ai_save"),
            "ai_save should be in ai_specific_tools(&mae_core::OptionRegistry::new())"
        );
        assert!(
            names.contains(&"ai_load"),
            "ai_load should be in ai_specific_tools(&mae_core::OptionRegistry::new())"
        );
        assert!(
            names.contains(&"rename_file"),
            "rename_file should be in ai_specific_tools(&mae_core::OptionRegistry::new())"
        );
    }

    #[test]
    fn buffer_read_by_name() {
        let mut editor = Editor::new();
        let mut b = mae_core::Buffer::new();
        b.name = "other".into();
        editor.buffers.push(b);
        // Insert text into the "other" buffer
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[1].insert_char(win, 'X');

        let call = make_call("buffer_read", serde_json::json!({"buffer_name": "other"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(result.output.contains("X"));
    }

    #[test]
    fn buffer_write_by_name() {
        let mut editor = Editor::new();
        let mut b = mae_core::Buffer::new();
        b.name = "target".into();
        editor.buffers.push(b);

        let call = make_call(
            "buffer_write",
            serde_json::json!({"buffer_name": "target", "start_line": 1, "content": "hello\n"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(editor.buffers[1].text().contains("hello"));
        // Active buffer (scratch) should be unchanged
        assert!(!editor.buffers[0].text().contains("hello"));
    }

    #[test]
    fn create_file_and_open() {
        let dir = std::env::temp_dir();
        let path = dir.join("mae_test_create_file.txt");
        // Clean up first
        std::fs::remove_file(&path).ok();

        let mut editor = Editor::new();
        let call = make_call(
            "create_file",
            serde_json::json!({"path": path.to_str().unwrap(), "content": "new file\n"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "create_file failed: {}", result.output);
        assert_eq!(editor.buffers.len(), 2);
        let target_idx = editor.ai_target_buffer_idx.expect("should have AI target");
        assert!(editor.buffers[target_idx].text().contains("new file"));
        // File should exist on disk
        assert!(path.exists());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_files_returns_results() {
        // We're in a git repo, so this should work
        let mut editor = Editor::new();
        let call = make_call("project_files", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "project_files failed: {}", result.output);
        assert!(result.output.contains("Cargo.toml"));
    }

    #[test]
    fn project_files_with_pattern() {
        let mut editor = Editor::new();
        let call = make_call("project_files", serde_json::json!({"pattern": "*.toml"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(result.output.contains("Cargo.toml"));
        // Should not contain .rs files
        assert!(!result.output.contains(".rs"));
    }

    #[test]
    fn project_search_finds_pattern() {
        let mut editor = Editor::new();
        let call = make_call(
            "project_search",
            serde_json::json!({"pattern": "mae-core", "glob": "*.toml"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "project_search failed: {}", result.output);
        assert!(result.output.contains("mae-core"));
    }

    #[test]
    fn project_search_with_max_results() {
        let mut editor = Editor::new();
        let call = make_call(
            "project_search",
            serde_json::json!({"pattern": "fn", "max_results": 3}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        // Should have at most 3 result lines (not counting truncation message)
        let non_truncation_lines: Vec<&str> = result
            .output
            .lines()
            .filter(|l| !l.starts_with("..."))
            .collect();
        assert!(non_truncation_lines.len() <= 3);
    }

    #[test]
    fn find_buffer_by_name_helper() {
        let mut editor = Editor::new();
        assert_eq!(editor.find_buffer_by_name("[scratch]"), Some(0));
        assert_eq!(editor.find_buffer_by_name("nonexistent"), None);

        let mut b = mae_core::Buffer::new();
        b.name = "test".into();
        editor.buffers.push(b);
        assert_eq!(editor.find_buffer_by_name("test"), Some(1));
    }

    #[test]
    fn lsp_diagnostics_tool_returns_structured_json() {
        use mae_core::{Buffer, Diagnostic, DiagnosticSeverity};
        use std::path::PathBuf;
        let mut b = Buffer::new();
        b.set_file_path(PathBuf::from("/tmp/a.rs"));
        let mut editor = Editor::with_buffer(b);
        editor.diagnostics.set(
            "file:///tmp/a.rs".into(),
            vec![Diagnostic {
                line: 2,
                col_start: 4,
                col_end: 7,
                end_line: 2,
                severity: DiagnosticSeverity::Error,
                message: "bad".into(),
                source: Some("rustc".into()),
                code: Some("E0001".into()),
            }],
        );
        let call = make_call("lsp_diagnostics", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "lsp_diagnostics failed: {}", result.output);
        let v: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(v["counts"]["error"], 1);
        assert_eq!(v["files"][0]["diagnostics"][0]["line"], 3);
        assert_eq!(v["files"][0]["diagnostics"][0]["code"], "E0001");
    }

    #[test]
    fn syntax_tree_tool_returns_sexp() {
        use mae_core::Buffer;
        use std::path::PathBuf;
        let mut b = Buffer::new();
        b.set_file_path(PathBuf::from("/tmp/x.rs"));
        let mut editor = Editor::with_buffer(b);
        // Populate buffer with some Rust code.
        for ch in "fn main() {}".chars() {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, ch);
        }
        editor.syntax.invalidate(0);

        let call = make_call("syntax_tree", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success, "syntax_tree failed: {}", result.output);
        let v: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(v["language"], "rust");
        assert!(v["sexp"].as_str().unwrap().contains("function_item"));
    }

    #[test]
    fn switch_to_buffer_sets_alternate() {
        let mut editor = Editor::new();
        let mut b = mae_core::Buffer::new();
        b.name = "other".into();
        editor.buffers.push(b);

        editor.switch_to_buffer(1);
        assert_eq!(editor.active_buffer_idx(), 1);
        assert_eq!(editor.alternate_buffer_idx, Some(0));
    }

    // --- Phase 4c M4: AI DAP tools ---

    /// Policy that allows Privileged tier — needed for `dap_start` since
    /// it launches arbitrary programs under a debug adapter.
    fn privileged_policy() -> PermissionPolicy {
        PermissionPolicy {
            auto_approve_up_to: PermissionTier::Privileged,
        }
    }

    #[test]
    fn dap_start_tool_queues_intent() {
        let mut editor = Editor::new();
        let call = make_call(
            "dap_start",
            serde_json::json!({"adapter": "lldb", "program": "/bin/ls"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &privileged_policy(),
        ));
        assert!(result.success, "dap_start failed: {}", result.output);
        assert_eq!(editor.pending_dap_intents.len(), 1);
        assert!(editor.debug_state.is_some());
    }

    #[test]
    fn dap_start_tool_is_allowed_at_shell_tier() {
        let mut editor = Editor::new();
        let call = make_call(
            "dap_start",
            serde_json::json!({"adapter": "lldb", "program": "/bin/ls"}),
        );
        // Default policy allows up to Shell — should be allowed.
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
    }

    #[test]
    fn dap_set_breakpoint_tool_returns_json() {
        let mut editor = Editor::new();
        let call = make_call(
            "dap_set_breakpoint",
            serde_json::json!({"source": "/a.rs", "line": 42}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(
            result.success,
            "dap_set_breakpoint failed: {}",
            result.output
        );
        let v: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(v["source"], "/a.rs");
        assert_eq!(v["line"], 42);
    }

    #[test]
    fn dap_continue_tool_errors_without_session() {
        let mut editor = Editor::new();
        let call = make_call("dap_continue", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(!result.success);
        assert!(result.output.contains("No active"));
    }

    #[test]
    fn dap_step_tool_rejects_unknown_direction() {
        let mut editor = Editor::new();
        editor.debug_state = Some(mae_core::DebugState::new(mae_core::DebugTarget::Dap {
            adapter_name: "lldb".into(),
            program: "/bin/ls".into(),
        }));
        let call = make_call("dap_step", serde_json::json!({"direction": "sideways"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(!result.success);
        assert!(result.output.contains("Unknown step"));
    }

    #[test]
    fn dap_inspect_variable_tool_errors_without_session() {
        let mut editor = Editor::new();
        let call = make_call("dap_inspect_variable", serde_json::json!({"name": "x"}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(!result.success);
        assert!(result.output.contains("No active"));
    }

    // --- Phase 4a M5: Deferred LSP AI tools ---

    #[test]
    fn lsp_definition_returns_deferred() {
        let mut b = mae_core::Buffer::new();
        b.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
        let mut editor = Editor::with_buffer(b);
        let call = make_call("lsp_definition", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        match result {
            ExecuteResult::Deferred { kind, .. } => {
                assert_eq!(kind, DeferredKind::LspDefinition);
            }
            ExecuteResult::Immediate(r) => panic!("expected Deferred, got Immediate: {}", r.output),
        }
        assert_eq!(editor.pending_lsp_requests.len(), 1);
    }

    #[test]
    fn lsp_references_returns_deferred() {
        let mut b = mae_core::Buffer::new();
        b.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
        let mut editor = Editor::with_buffer(b);
        let call = make_call("lsp_references", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(matches!(
            result,
            ExecuteResult::Deferred {
                kind: DeferredKind::LspReferences,
                ..
            }
        ));
    }

    #[test]
    fn lsp_hover_returns_deferred() {
        let mut b = mae_core::Buffer::new();
        b.set_file_path(std::path::PathBuf::from("/tmp/test.rs"));
        let mut editor = Editor::with_buffer(b);
        let call = make_call("lsp_hover", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(matches!(
            result,
            ExecuteResult::Deferred {
                kind: DeferredKind::LspHover,
                ..
            }
        ));
    }

    #[test]
    fn lsp_definition_returns_immediate_error_for_scratch() {
        let mut editor = Editor::new();
        let call = make_call("lsp_definition", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        let result = match result {
            ExecuteResult::Immediate(r) => r,
            ExecuteResult::Deferred { .. } => panic!("expected Immediate error for scratch buffer"),
        };
        assert!(!result.success);
        assert!(result.output.contains("no file path"));
    }

    #[test]
    fn ai_permissions_tool_returns_tier_info() {
        let mut editor = Editor::new();
        let call = make_call("ai_permissions", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert!(result.output.contains("trusted"));
        assert!(result.output.contains("Permission tiers"));
    }

    #[test]
    fn ai_permissions_tool_reflects_policy() {
        let mut editor = Editor::new();
        let call = make_call("ai_permissions", serde_json::json!({}));
        let policy = PermissionPolicy {
            auto_approve_up_to: PermissionTier::ReadOnly,
        };
        let result = unwrap_immediate(execute_tool(&mut editor, &call, &all_tools(), &policy));
        assert!(result.success);
        assert!(result.output.contains("readonly"));
    }

    #[test]
    fn ai_permissions_tool_exists_in_definitions() {
        let tools = ai_specific_tools(&mae_core::OptionRegistry::new());
        assert!(
            tools.iter().any(|t| t.name == "ai_permissions"),
            "ai_permissions should be in ai_specific_tools(&mae_core::OptionRegistry::new())"
        );
    }

    #[test]
    fn self_test_suite_returns_all_categories() {
        let mut editor = Editor::new();
        let call = make_call("self_test_suite", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let plan: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(plan["version"], 1);
        let cats = plan["categories"].as_array().unwrap();
        let names: Vec<&str> = cats.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"introspection"));
        assert!(names.contains(&"editing"));
        assert!(names.contains(&"help"));
        assert!(names.contains(&"project"));
        assert!(names.contains(&"lsp"));
    }

    #[test]
    fn self_test_suite_filters_categories() {
        let mut editor = Editor::new();
        let call = make_call(
            "self_test_suite",
            serde_json::json!({"categories": "editing,help"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let plan: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let cats = plan["categories"].as_array().unwrap();
        assert_eq!(cats.len(), 2);
        let names: Vec<&str> = cats.iter().map(|c| c["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"editing"));
        assert!(names.contains(&"help"));
        assert!(!names.contains(&"introspection"));
    }

    #[test]
    fn self_test_suite_exists_in_definitions() {
        let tools = ai_specific_tools(&mae_core::OptionRegistry::new());
        assert!(
            tools.iter().any(|t| t.name == "self_test_suite"),
            "self_test_suite should be in ai_specific_tools(&mae_core::OptionRegistry::new())"
        );
    }

    #[test]
    fn self_test_plan_has_performance_category() {
        let mut editor = Editor::new();
        let call = make_call(
            "self_test_suite",
            serde_json::json!({"categories": "performance"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let plan: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        let categories = plan["categories"].as_array().unwrap();
        assert!(categories.iter().any(|c| c["name"] == "performance"));
    }

    #[test]
    fn perf_stats_returns_valid_json() {
        let mut editor = Editor::new();
        let call = make_call("perf_stats", serde_json::json!({}));
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let stats: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert!(stats["rss_bytes"].is_number());
        assert!(stats["buffer_count"].is_number());
        assert!(stats["total_lines"].is_number());
    }

    #[test]
    fn perf_benchmark_buffer_insert_measures_time() {
        let mut editor = Editor::new();
        let call = make_call(
            "perf_benchmark",
            serde_json::json!({"benchmark": "buffer_insert", "size": 100}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        let bench: serde_json::Value = serde_json::from_str(&result.output).unwrap();
        assert_eq!(bench["benchmark"], "buffer_insert");
        assert_eq!(bench["size"], 100);
        assert!(bench["duration_us"].as_u64().unwrap() > 0);
        assert!(bench["ops_per_sec"].as_u64().unwrap() > 0);
    }

    #[test]
    fn trigger_hook_queues_hooks() {
        let mut editor = Editor::new();
        // Register a dummy function so fire_hook actually queues something
        editor.hooks.add("buffer-open", "my-fn");

        let call = make_call(
            "trigger_hook",
            serde_json::json!({"hook_name": "buffer-open"}),
        );
        let result = unwrap_immediate(execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        ));
        assert!(result.success);
        assert_eq!(editor.pending_hook_evals.len(), 1);
        assert_eq!(editor.pending_hook_evals[0].0, "buffer-open");
    }
}
