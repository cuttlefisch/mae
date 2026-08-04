use mae_core::Editor;

use crate::tool_impls::{
    execute_git_branch_delete, execute_git_branch_list, execute_git_checkout, execute_git_commit,
    execute_git_diff, execute_git_log, execute_git_merge, execute_git_pull, execute_git_push,
    execute_git_stage, execute_git_stash_list, execute_git_stash_pop, execute_git_stash_push,
    execute_git_status, execute_git_unstage, execute_github_pr_create, execute_github_pr_status,
    execute_shell_list, execute_shell_read_output, execute_shell_scrollback,
    execute_shell_send_input, execute_terminal_at_file, execute_terminal_spawn,
};
use crate::types::ToolCall;

/// Dispatch shell, terminal, git, and GitHub tools.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "shell_list" => execute_shell_list(editor),
        "shell_read_output" => execute_shell_read_output(editor, &call.arguments),
        "shell_send_input" => execute_shell_send_input(editor, &call.arguments),
        "terminal_spawn" => execute_terminal_spawn(editor, &call.arguments),
        "terminal_at_file" => execute_terminal_at_file(editor, &call.arguments),
        "terminal_send" => execute_shell_send_input(editor, &call.arguments),
        "terminal_read" => execute_shell_read_output(editor, &call.arguments),
        "shell_scrollback" => execute_shell_scrollback(editor, &call.arguments),
        "shell_exec" => {
            // Apply sandbox shell filter during test mode.
            if let Some(ref sandbox_dir) = editor.test_sandbox_dir {
                if let Some(cmd) = call.arguments.get("command").and_then(|v| v.as_str()) {
                    match super::sandbox::filter_shell_command(cmd, sandbox_dir) {
                        Ok(filtered) => {
                            let mut args = call.arguments.clone();
                            args["command"] = serde_json::Value::String(filtered);
                            execute_shell_exec_sync(&args)
                        }
                        Err(e) => Err(e),
                    }
                } else {
                    execute_shell_exec_sync(&call.arguments)
                }
            } else {
                execute_shell_exec_sync(&call.arguments)
            }
        }
        "github_pr_status" => execute_github_pr_status(editor),
        "github_pr_create" => execute_github_pr_create(editor, &call.arguments),

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
        "git_stash_push" => execute_git_stash_push(editor, &call.arguments),
        "git_stash_pop" => execute_git_stash_pop(editor),
        "git_stash_list" => execute_git_stash_list(editor),
        "git_branch_list" => execute_git_branch_list(editor),
        "git_branch_delete" => execute_git_branch_delete(editor, &call.arguments),
        "git_merge" => execute_git_merge(editor, &call.arguments),

        _ => return None,
    };
    Some(result)
}

/// Synchronous shell_exec for MCP callers and other non-session paths.
///
/// The AI session handles shell_exec async (see `AgentSession::execute_shell`),
/// but MCP tool calls bypass the session and go through `execute_tool` directly.
/// This synchronous version uses `std::process::Command` so MCP agents can
/// run shell commands without the async session context.
pub(super) fn execute_shell_exec_sync(args: &serde_json::Value) -> Result<String, String> {
    let command = args.get("command").and_then(|v| v.as_str()).unwrap_or("");

    if command.is_empty() {
        return Err("Missing 'command' argument".into());
    }

    // Shared with the session's async version — see `crate::shell_policy`.
    if let Some(pattern) = crate::shell_policy::blocked_pattern(command) {
        return Err(format!(
            "Command blocked: contains dangerous pattern '{}'",
            pattern
        ));
    }

    let timeout = crate::shell_policy::timeout_from_args(args);

    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to execute: {}", e))?;

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
                    return Err(format!(
                        "Command timed out after {}",
                        crate::shell_policy::describe_timeout(timeout)
                    ));
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
        // ADR-087 / audit #594: shell output is arbitrary UTF-8; a fixed byte
        // cut can land mid-character. See `session/run_loop.rs`'s identical
        // truncation for the same reasoning.
        let stdout_str = if stdout.len() > 10_000 {
            let cut = mae_core::grapheme::floor_char_boundary(&stdout, 10_000);
            format!("{}...[truncated]", &stdout[..cut])
        } else {
            stdout.to_string()
        };
        out.push_str(&format!("stdout:\n{}\n", stdout_str));
    }
    if !stderr.is_empty() {
        let stderr_str = if stderr.len() > 5_000 {
            let cut = mae_core::grapheme::floor_char_boundary(&stderr, 5_000);
            format!("{}...[truncated]", &stderr[..cut])
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
