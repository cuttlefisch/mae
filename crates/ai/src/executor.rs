use std::path::{Path, PathBuf};

use mae_core::Editor;

use crate::tools::PermissionPolicy;
use crate::types::*;

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
) -> ToolResult {
    // 1. Find the tool definition
    let tool_def = all_tools.iter().find(|t| t.name == call.name);
    let permission = tool_def
        .and_then(|t| t.permission)
        .unwrap_or(PermissionTier::Write);

    // 2. Check permission
    if !policy.is_allowed(permission) {
        return ToolResult {
            tool_call_id: call.id.clone(),
            success: false,
            output: format!(
                "Permission denied: {} requires {:?} tier",
                call.name, permission
            ),
        };
    }

    // 3. Dispatch — check AI-specific tools first (some have command_ prefix collision)
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
        "project_search",
    ];
    let result = if ai_tool_names.contains(&call.name.as_str()) {
        execute_ai_tool(editor, call)
    } else if let Some(cmd_name) = call.name.strip_prefix("command_") {
        execute_registry_command(editor, cmd_name)
    } else {
        execute_ai_tool(editor, call)
    };

    ToolResult {
        tool_call_id: call.id.clone(),
        success: result.is_ok(),
        output: result.unwrap_or_else(|e| e),
    }
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
        "project_search" => execute_project_search(&call.arguments),
        // shell_exec is handled async in the session, not here
        _ => Err(format!("Unknown tool: {}", call.name)),
    }
}

/// Resolve a buffer reference: if `buffer_name` is provided, find that buffer;
/// otherwise return the active buffer index.
fn resolve_buffer_idx(editor: &Editor, args: &serde_json::Value) -> Result<usize, String> {
    if let Some(name) = args.get("buffer_name").and_then(|v| v.as_str()) {
        editor
            .find_buffer_by_name(name)
            .ok_or_else(|| format!("No buffer named '{}'", name))
    } else {
        Ok(editor.active_buffer_idx())
    }
}

fn execute_buffer_read(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let buf_idx = resolve_buffer_idx(editor, args)?;
    let buf = &editor.buffers[buf_idx];
    let total_lines = buf.line_count();
    let start = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .map(|n| (n as usize).saturating_sub(1)) // 1-indexed → 0-indexed
        .unwrap_or(0);
    let end = args
        .get("end_line")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
        .unwrap_or(total_lines);

    let mut output = String::new();
    for i in start..end.min(total_lines) {
        let line = buf.rope().line(i);
        output.push_str(&format!("{:>4} | {}", i + 1, line));
    }
    Ok(output)
}

fn execute_buffer_write(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let start_line = args
        .get("start_line")
        .and_then(|v| v.as_u64())
        .ok_or("Missing 'start_line' argument")? as usize;
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'content' argument")?;

    let buf_idx = resolve_buffer_idx(editor, args)?;
    let buf = &mut editor.buffers[buf_idx];
    let total_lines = buf.line_count();

    // Convert 1-indexed to 0-indexed
    let start_idx = (start_line.saturating_sub(1)).min(total_lines);

    let end_line = args.get("end_line").and_then(|v| v.as_u64());

    if let Some(end) = end_line {
        // Replace range: delete lines [start, end], then insert content
        let end_idx = (end as usize).min(total_lines);
        let char_start = buf.rope().line_to_char(start_idx);
        let char_end = if end_idx >= total_lines {
            buf.rope().len_chars()
        } else {
            buf.rope().line_to_char(end_idx)
        };
        buf.delete_range(char_start, char_end);
        if !content.is_empty() {
            buf.insert_text_at(char_start, content);
        }
        Ok(format!(
            "Replaced lines {}-{} ({} chars written)",
            start_line,
            end,
            content.len()
        ))
    } else {
        // Insert before start_line
        let char_pos = if start_idx >= total_lines {
            buf.rope().len_chars()
        } else {
            buf.rope().line_to_char(start_idx)
        };
        buf.insert_text_at(char_pos, content);
        Ok(format!(
            "Inserted at line {} ({} chars)",
            start_line,
            content.len()
        ))
    }
}

fn execute_cursor_info(editor: &Editor) -> Result<String, String> {
    let buf = editor.active_buffer();
    let win = editor.window_mgr.focused_window();
    let info = serde_json::json!({
        "buffer_name": buf.name,
        "cursor_row": win.cursor_row + 1,
        "cursor_col": win.cursor_col + 1,
        "line_count": buf.line_count(),
        "modified": buf.modified,
        "mode": format!("{:?}", editor.mode),
    });
    Ok(info.to_string())
}

fn execute_file_read(args: &serde_json::Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let content = std::fs::read_to_string(path).map_err(|e| format!("File read error: {}", e))?;
    let mut output = String::new();
    for (i, line) in content.lines().enumerate() {
        output.push_str(&format!("{:>4} | {}\n", i + 1, line));
    }
    Ok(output)
}

fn execute_editor_state(editor: &Editor) -> Result<String, String> {
    let buf = editor.active_buffer();
    let info = serde_json::json!({
        "mode": format!("{:?}", editor.mode),
        "theme": editor.theme.name,
        "buffer_count": editor.buffers.len(),
        "window_count": editor.window_mgr.window_count(),
        "active_buffer": buf.name,
        "active_buffer_modified": buf.modified,
        "message_log_entries": editor.message_log.len(),
        "debug_session_active": editor.debug_state.is_some(),
        "debug_target": editor.debug_state.as_ref().map(|s| format!("{:?}", s.target)),
        "command_count": editor.commands.len(),
    });
    serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
}

fn execute_window_layout(editor: &Editor) -> Result<String, String> {
    let windows: Vec<serde_json::Value> = editor
        .window_mgr
        .iter_windows()
        .map(|win| {
            let buf_name = editor
                .buffers
                .get(win.buffer_idx)
                .map(|b| b.name.as_str())
                .unwrap_or("<invalid>");
            serde_json::json!({
                "buffer_idx": win.buffer_idx,
                "buffer_name": buf_name,
                "cursor_row": win.cursor_row,
                "cursor_col": win.cursor_col,
                "scroll_offset": win.scroll_offset,
            })
        })
        .collect();
    serde_json::to_string_pretty(&windows).map_err(|e| e.to_string())
}

fn execute_command_list(editor: &Editor) -> Result<String, String> {
    let commands: Vec<serde_json::Value> = editor
        .commands
        .list_commands()
        .iter()
        .map(|cmd| {
            serde_json::json!({
                "name": cmd.name,
                "doc": cmd.doc,
                "source": format!("{:?}", cmd.source),
            })
        })
        .collect();
    serde_json::to_string_pretty(&commands).map_err(|e| e.to_string())
}

fn execute_debug_state(editor: &Editor) -> Result<String, String> {
    match &editor.debug_state {
        None => Ok("No active debug session".into()),
        Some(state) => {
            let threads: Vec<serde_json::Value> = state
                .threads
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "id": t.id,
                        "name": t.name,
                        "stopped": t.stopped,
                    })
                })
                .collect();

            let frames: Vec<serde_json::Value> = state
                .stack_frames
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "id": f.id,
                        "name": f.name,
                        "source": f.source,
                        "line": f.line,
                    })
                })
                .collect();

            let breakpoints: Vec<serde_json::Value> = state
                .breakpoints
                .iter()
                .flat_map(|(source, bps)| {
                    bps.iter().map(move |bp| {
                        serde_json::json!({
                            "id": bp.id,
                            "source": source,
                            "line": bp.line,
                            "verified": bp.verified,
                        })
                    })
                })
                .collect();

            let info = serde_json::json!({
                "target": format!("{:?}", state.target),
                "threads": threads,
                "stack_frames": frames,
                "scopes": state.scopes.iter().map(|s| &s.name).collect::<Vec<_>>(),
                "breakpoints": breakpoints,
                "stopped_location": state.stopped_location,
            });
            serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
        }
    }
}

fn execute_open_file(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;

    // Check if file is already open in a buffer
    let file_path = PathBuf::from(path);
    let canonical = file_path.canonicalize().ok();
    let existing_idx = editor.buffers.iter().enumerate().find_map(|(i, buf)| {
        buf.file_path().and_then(|bp| {
            if bp == file_path || canonical.as_deref() == bp.canonicalize().ok().as_deref() {
                Some(i)
            } else {
                None
            }
        })
    });
    if let Some(idx) = existing_idx {
        let name = editor.buffers[idx].name.clone();
        editor.switch_to_buffer(idx);
        return Ok(format!(
            "Switched to existing buffer '{}' (already open)",
            name
        ));
    }

    // Open new buffer
    editor.open_file(path);
    if editor.status_msg.contains("Error") {
        Err(editor.status_msg.clone())
    } else {
        Ok(format!(
            "Opened '{}' ({} lines)",
            editor.active_buffer().name,
            editor.active_buffer().line_count()
        ))
    }
}

fn execute_switch_buffer(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'name' argument")?;

    let idx = editor
        .find_buffer_by_name(name)
        .ok_or_else(|| format!("No buffer named '{}'", name))?;

    editor.switch_to_buffer(idx);
    Ok(format!("Switched to buffer '{}'", name))
}

fn execute_close_buffer(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let idx = if let Some(name) = args.get("name").and_then(|v| v.as_str()) {
        editor
            .find_buffer_by_name(name)
            .ok_or_else(|| format!("No buffer named '{}'", name))?
    } else {
        editor.active_buffer_idx()
    };

    if editor.buffers[idx].modified {
        return Err(format!(
            "Buffer '{}' has unsaved changes",
            editor.buffers[idx].name
        ));
    }

    let name = editor.buffers[idx].name.clone();
    // Switch to this buffer first so kill-buffer acts on it
    editor.switch_to_buffer(idx);
    editor.dispatch_builtin("kill-buffer");
    Ok(format!("Closed buffer '{}'", name))
}

fn execute_create_file(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");

    let file_path = Path::new(path);

    // Create parent directories if needed
    if let Some(parent) = file_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directories: {}", e))?;
        }
    }

    // Write the file
    std::fs::write(file_path, content).map_err(|e| format!("Failed to create file: {}", e))?;

    // Open it as a buffer
    editor.open_file(path);
    if editor.status_msg.contains("Error") {
        Err(editor.status_msg.clone())
    } else {
        Ok(format!(
            "Created '{}' ({} bytes) and opened as buffer",
            path,
            content.len()
        ))
    }
}

fn execute_project_files(args: &serde_json::Value) -> Result<String, String> {
    let pattern = args.get("pattern").and_then(|v| v.as_str());

    // Try git ls-files first
    let output = std::process::Command::new("git")
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output();

    let files = match output {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout).to_string(),
        _ => {
            // Fallback: list files recursively (limited depth)
            let output = std::process::Command::new("find")
                .args([
                    ".",
                    "-type",
                    "f",
                    "-not",
                    "-path",
                    "./.git/*",
                    "-maxdepth",
                    "5",
                ])
                .output()
                .map_err(|e| format!("Failed to list files: {}", e))?;
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(|l| l.strip_prefix("./").unwrap_or(l).to_string())
                .collect::<Vec<_>>()
                .join("\n")
        }
    };

    // Filter by pattern if provided
    if let Some(pat) = pattern {
        let glob = glob::Pattern::new(pat).map_err(|e| format!("Invalid glob: {}", e))?;
        let filtered: Vec<&str> = files
            .lines()
            .filter(|line| {
                glob.matches(line) || glob.matches(line.rsplit('/').next().unwrap_or(line))
            })
            .collect();
        Ok(format!("{} files\n{}", filtered.len(), filtered.join("\n")))
    } else {
        let count = files.lines().count();
        Ok(format!("{} files\n{}", count, files))
    }
}

fn execute_project_search(args: &serde_json::Value) -> Result<String, String> {
    let pattern = args
        .get("pattern")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'pattern' argument")?;
    let glob_filter = args.get("glob").and_then(|v| v.as_str());
    let max_results = args
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(100) as usize;

    // Try ripgrep first, fall back to grep
    let mut cmd = if which_exists("rg") {
        let mut c = std::process::Command::new("rg");
        c.args(["--line-number", "--no-heading", "--color=never"]);
        if let Some(g) = glob_filter {
            c.args(["--glob", g]);
        }
        c.args(["-m", &max_results.to_string(), pattern]);
        c
    } else {
        let mut c = std::process::Command::new("grep");
        c.args(["-rn", "--color=never"]);
        if let Some(g) = glob_filter {
            c.args(["--include", g]);
        }
        c.args([pattern, "."]);
        c
    };

    let output = cmd.output().map_err(|e| format!("Search failed: {}", e))?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Truncate to max_results lines
    let lines: Vec<&str> = stdout.lines().take(max_results).collect();
    let total = stdout.lines().count();
    let shown = lines.len();

    let mut result = lines.join("\n");
    if total > shown {
        result.push_str(&format!("\n... ({} more results truncated)", total - shown));
    }
    if result.is_empty() {
        result = "No matches found".into();
    }
    Ok(result)
}

/// Check if a command exists on PATH.
fn which_exists(cmd: &str) -> bool {
    std::process::Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn execute_list_buffers(editor: &Editor) -> Result<String, String> {
    let buffers: Vec<serde_json::Value> = editor
        .buffers
        .iter()
        .enumerate()
        .map(|(i, buf)| {
            serde_json::json!({
                "index": i,
                "name": buf.name,
                "modified": buf.modified,
                "active": i == editor.active_buffer_idx(),
                "line_count": buf.line_count(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&buffers).map_err(|e| e.to_string())
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
        tools.extend(ai_specific_tools());
        tools
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success);
        assert!(result.output.contains("bbb"));
        assert!(!result.output.contains("aaa"));
        assert!(!result.output.contains("ccc"));
    }

    #[test]
    fn buffer_read_empty() {
        let mut editor = Editor::new();
        let call = make_call("buffer_read", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success);
    }

    #[test]
    fn cursor_info_returns_json() {
        let mut editor = make_editor_with_text("hello");
        let call = make_call("cursor_info", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success);
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
    }

    #[test]
    fn registry_command_unknown() {
        let mut editor = Editor::new();
        let call = make_call("command_nonexistent", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(!result.success);
        assert!(result.output.contains("Unknown command"));
    }

    #[test]
    fn permission_denied_for_privileged() {
        let mut editor = Editor::new();
        let call = make_call("command_quit", serde_json::json!({}));
        let policy = PermissionPolicy::default(); // allows up to Shell
        let result = execute_tool(&mut editor, &call, &all_tools(), &policy);
        assert!(!result.success);
        assert!(result.output.contains("Permission denied"));
    }

    #[test]
    fn unknown_tool_returns_error() {
        let mut editor = Editor::new();
        let call = make_call("totally_fake_tool", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(!result.success);
        assert!(result.output.contains("Unknown tool"));
    }

    #[test]
    fn list_buffers_returns_metadata() {
        let mut editor = Editor::new();
        let call = make_call("list_buffers", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success);
        let text = editor.active_buffer().text();
        assert!(text.contains("XXX"));
        assert!(!text.contains("bbb"));
    }

    #[test]
    fn editor_state_returns_valid_json() {
        let mut editor = Editor::new();
        let call = make_call("editor_state", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success);
        let windows: Vec<serde_json::Value> = serde_json::from_str(&result.output).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0]["buffer_name"], "[scratch]");
    }

    #[test]
    fn command_list_includes_expected_commands() {
        let mut editor = Editor::new();
        let call = make_call("command_list", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success);
        assert_eq!(result.output, "No active debug session");
    }

    #[test]
    fn debug_state_with_self_debug() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("debug-self");
        let call = make_call("debug_state", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success, "open_file failed: {}", result.output);
        assert_eq!(editor.buffers.len(), 2);
        assert!(editor.active_buffer().text().contains("line1"));

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
        execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success);
        assert_eq!(editor.active_buffer().name, "second");
    }

    #[test]
    fn switch_buffer_nonexistent() {
        let mut editor = Editor::new();
        let call = make_call("switch_buffer", serde_json::json!({"name": "nope"}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success, "close_buffer failed: {}", result.output);
        assert_eq!(editor.buffers.len(), 1);
    }

    #[test]
    fn close_buffer_modified_fails() {
        let mut editor = Editor::new();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'x');

        let call = make_call("close_buffer", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(!result.success);
        assert!(result.output.contains("unsaved"));
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success, "create_file failed: {}", result.output);
        assert_eq!(editor.buffers.len(), 2);
        assert!(editor.active_buffer().text().contains("new file"));
        // File should exist on disk
        assert!(path.exists());

        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn project_files_returns_results() {
        // We're in a git repo, so this should work
        let mut editor = Editor::new();
        let call = make_call("project_files", serde_json::json!({}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
        assert!(result.success, "project_files failed: {}", result.output);
        assert!(result.output.contains("Cargo.toml"));
    }

    #[test]
    fn project_files_with_pattern() {
        let mut editor = Editor::new();
        let call = make_call("project_files", serde_json::json!({"pattern": "*.toml"}));
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
    fn switch_to_buffer_sets_alternate() {
        let mut editor = Editor::new();
        let mut b = mae_core::Buffer::new();
        b.name = "other".into();
        editor.buffers.push(b);

        editor.switch_to_buffer(1);
        assert_eq!(editor.active_buffer_idx(), 1);
        assert_eq!(editor.alternate_buffer_idx, Some(0));
    }
}
