use mae_core::Editor;

use crate::tool_impls::execute_lsp_diagnostics;
use crate::types::ToolCall;

/// Dispatch synchronous LSP tools (diagnostics, rename, format, code action).
/// Deferred LSP tools (definition, references, hover, symbols) are handled
/// directly in `execute_tool()` in mod.rs before reaching this dispatcher.
/// Returns `Some(result)` if the tool was handled, `None` otherwise.
pub(super) fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    let result = match call.name.as_str() {
        "lsp_diagnostics" => execute_lsp_diagnostics(editor, &call.arguments),
        "lsp_rename" => execute_lsp_rename(editor, &call.arguments),
        "lsp_format" => execute_lsp_format(editor, &call.arguments),
        "lsp_code_action" => execute_lsp_code_action(editor, &call.arguments),
        _ => return None,
    };
    Some(result)
}

fn execute_lsp_rename(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing required parameter 'new_name'".to_string())?;

    let idx = crate::tool_impls::resolve_buffer_idx(editor, args)?;
    let buf = &editor.buffers[idx];
    let path = buf
        .file_path()
        .ok_or_else(|| "buffer has no file path".to_string())?;
    let uri = mae_core::path_to_uri(path);
    let language_id =
        mae_core::language_id_from_path(path).unwrap_or_else(|| "plaintext".to_string());

    // Position: args override → cursor
    let (target_row, target_col) = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.buffer_idx == idx)
        .map(|w| (w.cursor_row, w.cursor_col))
        .unwrap_or((0, 0));
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .map(|l| l.saturating_sub(1) as u32)
        .unwrap_or(target_row as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|c| c.saturating_sub(1) as u32)
        .unwrap_or(target_col as u32);

    editor
        .pending_lsp_requests
        .push(mae_core::LspIntent::Rename {
            uri,
            language_id,
            line,
            character,
            new_name: new_name.to_string(),
        });

    Ok(format!("LSP rename queued: → '{}'", new_name))
}

fn execute_lsp_format(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let idx = crate::tool_impls::resolve_buffer_idx(editor, args)?;
    let buf = &editor.buffers[idx];
    let path = buf
        .file_path()
        .ok_or_else(|| "buffer has no file path".to_string())?;
    let uri = mae_core::path_to_uri(path);
    let language_id =
        mae_core::language_id_from_path(path).unwrap_or_else(|| "plaintext".to_string());

    let start_line = args.get("start_line").and_then(|v| v.as_u64());
    let end_line = args.get("end_line").and_then(|v| v.as_u64());

    if let (Some(sl), Some(el)) = (start_line, end_line) {
        editor
            .pending_lsp_requests
            .push(mae_core::LspIntent::RangeFormat {
                uri,
                language_id,
                start_line: (sl.saturating_sub(1)) as u32,
                start_char: 0,
                end_line: (el.saturating_sub(1)) as u32,
                end_char: 0,
            });
        Ok(format!("LSP range format queued: lines {}-{}", sl, el))
    } else {
        editor
            .pending_lsp_requests
            .push(mae_core::LspIntent::Format { uri, language_id });
        Ok("LSP format queued".to_string())
    }
}

fn execute_lsp_code_action(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let idx = crate::tool_impls::resolve_buffer_idx(editor, args)?;
    let buf = &editor.buffers[idx];
    let path = buf
        .file_path()
        .ok_or_else(|| "buffer has no file path".to_string())?;
    let uri = mae_core::path_to_uri(path);
    let language_id =
        mae_core::language_id_from_path(path).unwrap_or_else(|| "plaintext".to_string());

    // Position: args override → cursor
    let (target_row, target_col) = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.buffer_idx == idx)
        .map(|w| (w.cursor_row, w.cursor_col))
        .unwrap_or((0, 0));
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .map(|l| l.saturating_sub(1) as u32)
        .unwrap_or(target_row as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|c| c.saturating_sub(1) as u32)
        .unwrap_or(target_col as u32);

    // Queue code action request
    editor
        .pending_lsp_requests
        .push(mae_core::LspIntent::CodeAction {
            uri,
            language_id,
            line,
            character,
        });

    // Return existing code actions if any
    if let Some(ref menu) = editor.code_action_menu {
        let actions: Vec<String> = menu
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| format!("{}: {}", i, item.title))
            .collect();
        Ok(format!("Code actions available:\n{}", actions.join("\n")))
    } else {
        Ok("LSP code action request queued".to_string())
    }
}
