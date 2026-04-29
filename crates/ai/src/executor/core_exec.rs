use mae_core::Editor;

use crate::tool_impls::{
    execute_buffer_read, execute_buffer_write, execute_close_buffer, execute_command_list,
    execute_create_file, execute_cursor_info, execute_debug_state, execute_editor_restore_state,
    execute_editor_save_state, execute_editor_state, execute_file_read, execute_get_option,
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
        _ => return None,
    };
    Some(result)
}
