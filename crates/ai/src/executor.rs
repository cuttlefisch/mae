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
        // shell_exec is handled async in the session, not here
        _ => Err(format!("Unknown tool: {}", call.name)),
    }
}

fn execute_buffer_read(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let buf = editor.active_buffer();
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

    let buf = editor.active_buffer_mut();
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
}
