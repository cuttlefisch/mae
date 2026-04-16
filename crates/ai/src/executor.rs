use mae_core::Editor;

use crate::tools::PermissionPolicy;
use crate::types::*;

use crate::tool_impls::{
    execute_buffer_read, execute_buffer_write, execute_close_buffer, execute_command_list,
    execute_create_file, execute_cursor_info, execute_debug_state, execute_editor_state,
    execute_file_read, execute_list_buffers, execute_lsp_diagnostics, execute_open_file,
    execute_project_files, execute_project_search, execute_switch_buffer, execute_syntax_tree,
    execute_window_layout,
};

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
        "lsp_diagnostics",
        "syntax_tree",
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
        "lsp_diagnostics" => execute_lsp_diagnostics(editor, &call.arguments),
        "syntax_tree" => execute_syntax_tree(editor, &call.arguments),
        // shell_exec is handled async in the session, not here
        _ => Err(format!("Unknown tool: {}", call.name)),
    }
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
        let result = execute_tool(
            &mut editor,
            &call,
            &all_tools(),
            &PermissionPolicy::default(),
        );
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
}
