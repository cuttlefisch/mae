use mae_core::Editor;

use crate::tool_impls::{
    execute_audit_configuration, execute_buffer_read, execute_buffer_write, execute_close_buffer,
    execute_command_list, execute_create_file, execute_cursor_info, execute_debug_state,
    execute_editor_restore_state, execute_editor_save_state, execute_editor_state,
    execute_file_read, execute_get_option, execute_image_info, execute_image_list,
    execute_list_buffers, execute_open_file, execute_project_files, execute_project_info,
    execute_project_search, execute_read_messages, execute_rename_file, execute_set_option,
    execute_switch_buffer, execute_switch_project, execute_syntax_tree, execute_window_layout,
};
use crate::types::ToolCall;

/// Queue a Scheme expression for evaluation. The actual eval happens in the
/// AI event handler where the SchemeRuntime is available. The result replaces
/// the "queued" placeholder before being sent back to the AI.
fn execute_eval_scheme(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let code = args
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'code' argument")?;

    editor.pending_scheme_eval.push(code.to_string());
    // Placeholder — replaced by drain_pending_scheme_evals in ai_event_handler.
    Ok("eval_scheme: queued".into())
}

/// Execute a registered editor command by name (MCP tool handler).
/// Uses `dispatch_builtin_in_target()` so the command operates on the
/// AI target window (if set via `set_ai_target`) rather than the
/// human-focused window. This is the `with-current-buffer` pattern.
fn execute_command_dispatch(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let cmd = args
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'command' argument")?;
    if editor.dispatch_builtin_in_target(cmd) {
        Ok(format!("Executed: {}", cmd))
    } else {
        Err(format!("Unknown command: {}", cmd))
    }
}

/// Set the AI's target buffer/window for subsequent tool calls.
fn execute_set_ai_target(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    // Clear targeting if requested.
    if args.get("clear").and_then(|v| v.as_bool()).unwrap_or(false) {
        editor.ai_target_buffer_idx = None;
        editor.ai_target_window_id = None;
        return Ok("AI target cleared (using focused window)".into());
    }

    // Target by buffer name.
    if let Some(name) = args.get("buffer_name").and_then(|v| v.as_str()) {
        let idx = editor
            .find_buffer_by_name(name)
            .ok_or_else(|| format!("No buffer named '{}'", name))?;
        editor.ai_target_buffer_idx = Some(idx);
        // Also set window target if a window shows this buffer.
        if let Some(win) = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == idx)
        {
            editor.ai_target_window_id = Some(win.id);
        }
        return Ok(format!("AI target set to buffer '{}'", name));
    }

    // Target by window ID.
    if let Some(wid) = args.get("window_id").and_then(|v| v.as_u64()) {
        let wid = wid as u32;
        let win = editor
            .window_mgr
            .iter_windows()
            .find(|w| w.id == wid)
            .ok_or_else(|| format!("No window with id {}", wid))?;
        let buf_idx = win.buffer_idx;
        editor.ai_target_window_id = Some(wid);
        editor.ai_target_buffer_idx = Some(buf_idx);
        return Ok(format!(
            "AI target set to window {} (buffer '{}')",
            wid, editor.buffers[buf_idx].name
        ));
    }

    Err("Provide 'buffer_name', 'window_id', or 'clear: true'".into())
}

/// Dispatch core editor tools: buffer, cursor, file, project, editor state, options.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "buffer_read" => execute_buffer_read(editor, &call.arguments),
        "buffer_write" => execute_buffer_write(editor, &call.arguments),
        "cursor_info" => execute_cursor_info(editor),
        "file_read" => execute_file_read(&call.arguments),
        "list_buffers" => execute_list_buffers(editor),
        "editor_state" => execute_editor_state(editor),
        "read_messages" => execute_read_messages(editor, &call.arguments),
        "window_layout" => execute_window_layout(editor),
        "command_list" => execute_command_list(editor, &call.arguments),
        "debug_state" => execute_debug_state(editor),
        "open_file" => execute_open_file(editor, &call.arguments),
        "switch_buffer" => execute_switch_buffer(editor, &call.arguments),
        "close_buffer" => execute_close_buffer(editor, &call.arguments),
        "create_file" => execute_create_file(editor, &call.arguments),
        "project_files" => execute_project_files(&call.arguments),
        "project_info" => execute_project_info(editor),
        "project_search" => execute_project_search(&call.arguments),
        "switch_project" => execute_switch_project(editor, &call.arguments),
        "syntax_tree" => execute_syntax_tree(editor, &call.arguments),
        "get_option" => execute_get_option(editor, &call.arguments),
        "set_option" => execute_set_option(editor, &call.arguments),
        "rename_file" => execute_rename_file(editor, &call.arguments),
        "editor_save_state" => execute_editor_save_state(editor),
        "editor_restore_state" => execute_editor_restore_state(editor),
        "eval_scheme" => execute_eval_scheme(editor, &call.arguments),
        "audit_configuration" => execute_audit_configuration(editor),
        "execute_command" => execute_command_dispatch(editor, &call.arguments),
        "toggle_file_tree" => {
            editor.dispatch_builtin("file-tree-toggle");
            Ok("File tree toggled".into())
        }
        "image_info" => execute_image_info(&call.arguments),
        "image_list" => execute_image_list(editor),
        "set_ai_target" => execute_set_ai_target(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}
