//! MCP sync method handlers (pull-based collaborative editing).

use mae_core::Editor;
use serde_json::Value;

use crate::types::ToolCall;

pub fn dispatch(editor: &mut Editor, call: &ToolCall) -> Option<Result<String, String>> {
    match call.name.as_str() {
        "__mcp_sync_enable" => Some(execute_sync_enable(editor, &call.arguments)),
        "__mcp_sync_state_vector" => Some(execute_sync_state_vector(editor, &call.arguments)),
        "__mcp_sync_update" => Some(execute_sync_update(editor, &call.arguments)),
        "__mcp_sync_full_state" => Some(execute_sync_full_state(editor, &call.arguments)),
        _ => None,
    }
}

fn find_buffer_idx(editor: &Editor, args: &Value) -> Result<usize, String> {
    if let Some(idx) = args.get("buffer").and_then(|v| v.as_u64()) {
        let idx = idx as usize;
        if idx >= editor.buffers.len() {
            return Err(format!("Buffer index {} out of range", idx));
        }
        return Ok(idx);
    }
    if let Some(name) = args.get("buffer").and_then(|v| v.as_str()) {
        return editor
            .find_buffer_by_name(name)
            .ok_or_else(|| format!("No buffer named '{}'", name));
    }
    Err("Missing 'buffer' parameter (name or index)".into())
}

fn execute_sync_enable(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let idx = find_buffer_idx(editor, args)?;
    let client_id = args.get("client_id").and_then(|v| v.as_u64()).unwrap_or(1);

    let buf = &mut editor.buffers[idx];

    // Idempotent: if already enabled, return existing state with `already_enabled` flag
    let already_enabled = buf.sync_doc.is_some();
    if !already_enabled {
        buf.enable_sync(client_id);
    }

    let state = buf.sync_doc.as_ref().unwrap().encode_state();
    let state_b64 = mae_sync::encoding::update_to_base64(&state);

    Ok(serde_json::json!({
        "enabled": true,
        "already_enabled": already_enabled,
        "buffer": buf.name.clone(),
        "state": state_b64,
    })
    .to_string())
}

fn execute_sync_state_vector(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let idx = find_buffer_idx(editor, args)?;
    let buf = &editor.buffers[idx];

    let sync = buf
        .sync_doc
        .as_ref()
        .ok_or_else(|| format!("Buffer '{}' has no sync enabled", buf.name))?;

    let sv = sync.state_vector();
    let sv_b64 = mae_sync::encoding::state_vector_to_base64(&sv);

    Ok(serde_json::json!({
        "state_vector": sv_b64,
        "buffer": buf.name.clone(),
    })
    .to_string())
}

fn execute_sync_update(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let idx = find_buffer_idx(editor, args)?;
    let update_b64 = args
        .get("update")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'update' parameter (base64-encoded)")?;

    let update_bytes =
        mae_sync::encoding::base64_to_update(update_b64).map_err(|e| e.to_string())?;

    if editor.buffers[idx].sync_doc.is_none() {
        return Err(format!(
            "Buffer '{}' has no sync enabled",
            editor.buffers[idx].name
        ));
    }

    // Adjust cursor positions for every window viewing this buffer, exactly
    // like the collab-bridge remote-update path — this AI-driven apply path
    // used to do no cursor adjustment at all (buffer.rs's shared method makes
    // that correctness free instead of a separate reimplementation).
    let window_cursors: Vec<(mae_core::WindowId, usize)> = editor
        .window_mgr
        .iter_windows()
        .filter(|w| w.buffer_idx == idx)
        .map(|w| {
            (
                w.id,
                editor.buffers[idx].char_offset_at(w.cursor_row, w.cursor_col),
            )
        })
        .collect();
    let old_offsets: Vec<usize> = window_cursors.iter().map(|(_, o)| *o).collect();

    let adjusted_offsets = editor.buffers[idx]
        .apply_sync_update_with_cursors(&update_bytes, &old_offsets)
        .map_err(|e| e.to_string())?;

    for ((win_id, _), adjusted) in window_cursors.iter().zip(adjusted_offsets) {
        if let Some(win) = editor.window_mgr.window_mut(*win_id) {
            let (row, col) = editor.buffers[idx].row_col_from_offset(adjusted);
            win.cursor_row = row;
            win.cursor_col = col;
            win.clamp_cursor(&editor.buffers[idx]);
        }
    }

    let content_length = editor.buffers[idx].rope().len_chars();
    Ok(serde_json::json!({
        "applied": true,
        "content_length": content_length,
    })
    .to_string())
}

fn execute_sync_full_state(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let idx = find_buffer_idx(editor, args)?;
    let buf = &editor.buffers[idx];

    let sync = buf
        .sync_doc
        .as_ref()
        .ok_or_else(|| format!("Buffer '{}' has no sync enabled", buf.name))?;

    let state = sync.encode_state();
    let state_b64 = mae_sync::encoding::update_to_base64(&state);
    let content_length = buf.rope().len_chars();

    Ok(serde_json::json!({
        "state": state_b64,
        "buffer": buf.name.clone(),
        "content_length": content_length,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolCall;
    use mae_core::Editor;
    use serde_json::json;

    fn make_call(name: &str, args: Value) -> ToolCall {
        ToolCall {
            id: "test".to_string(),
            name: name.to_string(),
            arguments: args,
        }
    }

    #[test]
    fn sync_enable_creates_doc() {
        let mut editor = Editor::new();
        let call = make_call(
            "__mcp_sync_enable",
            json!({"buffer": "[scratch]", "client_id": 42}),
        );
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["enabled"], true);
        assert_eq!(parsed["buffer"], "[scratch]");
        assert!(!parsed["state"].as_str().unwrap().is_empty());
    }

    #[test]
    fn sync_enable_idempotent() {
        let mut editor = Editor::new();
        let call = make_call(
            "__mcp_sync_enable",
            json!({"buffer": "[scratch]", "client_id": 1}),
        );
        let r1 = dispatch(&mut editor, &call).unwrap().unwrap();
        let r2 = dispatch(&mut editor, &call).unwrap().unwrap();
        // Both succeed — idempotent
        let p1: Value = serde_json::from_str(&r1).unwrap();
        let p2: Value = serde_json::from_str(&r2).unwrap();
        assert_eq!(p1["enabled"], true);
        assert_eq!(p1["already_enabled"], false);
        assert_eq!(p2["enabled"], true);
        assert_eq!(p2["already_enabled"], true);
    }

    #[test]
    fn sync_state_vector_returns_encoded() {
        let mut editor = Editor::new();
        // Enable sync first
        editor.buffers[0].enable_sync(1);
        // Insert some content
        editor.buffers[0].insert_text_at(0, "X");

        let call = make_call("__mcp_sync_state_vector", json!({"buffer": "[scratch]"}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert!(!parsed["state_vector"].as_str().unwrap().is_empty());
        assert_eq!(parsed["buffer"], "[scratch]");
    }

    #[test]
    fn sync_update_applies_remote_edit() {
        let mut editor = Editor::new();
        // Enable sync on buffer 0
        editor.buffers[0].enable_sync(1);

        // Create a remote doc (client 2) with the same initial state
        let state = editor.buffers[0].sync_doc.as_ref().unwrap().encode_state();
        let mut remote = mae_sync::text::TextSync::with_client_id("", 2);
        remote.apply_update(&state).unwrap();

        // Remote inserts "hello"
        let update = remote.insert(0, "hello");
        let update_b64 = mae_sync::encoding::update_to_base64(&update);

        let call = make_call(
            "__mcp_sync_update",
            json!({"buffer": "[scratch]", "update": update_b64}),
        );
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["applied"], true);

        // Verify the content was applied
        let content: String = editor.buffers[0].rope().to_string();
        assert!(content.contains("hello"));
    }

    #[test]
    fn sync_update_errors_without_enable() {
        let mut editor = Editor::new();
        let call = make_call(
            "__mcp_sync_update",
            json!({"buffer": "[scratch]", "update": "AAAA"}),
        );
        let result = dispatch(&mut editor, &call).unwrap();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no sync enabled"));
    }

    #[test]
    fn sync_full_state_returns_content() {
        let mut editor = Editor::new();
        editor.buffers[0].enable_sync(1);
        editor.buffers[0].insert_text_at(0, "Z");

        let call = make_call("__mcp_sync_full_state", json!({"buffer": "[scratch]"}));
        let result = dispatch(&mut editor, &call).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert!(!parsed["state"].as_str().unwrap().is_empty());
        assert_eq!(parsed["buffer"], "[scratch]");
        assert!(parsed["content_length"].as_u64().unwrap() > 0);

        // Verify it's decodable
        let state_b64 = parsed["state"].as_str().unwrap();
        let bytes = mae_sync::encoding::base64_to_update(state_b64).unwrap();
        let reconstructed = mae_sync::text::TextSync::from_state(&bytes).unwrap();
        assert!(reconstructed.content().contains('Z'));
    }

    #[test]
    fn two_client_roundtrip() {
        let mut editor = Editor::new();

        // Client A enables sync
        let call_a = make_call(
            "__mcp_sync_enable",
            json!({"buffer": "[scratch]", "client_id": 10}),
        );
        dispatch(&mut editor, &call_a).unwrap().unwrap();

        // Client A makes a local edit (simulated through buffer API)
        editor.buffers[0].insert_text_at(0, "Hi");

        // Client B gets full state
        let call_state = make_call("__mcp_sync_full_state", json!({"buffer": "[scratch]"}));
        let result = dispatch(&mut editor, &call_state).unwrap().unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        let state_b64 = parsed["state"].as_str().unwrap();

        // Client B creates local doc and applies state
        let bytes = mae_sync::encoding::base64_to_update(state_b64).unwrap();
        let client_b = mae_sync::text::TextSync::from_state(&bytes).unwrap();

        // Both should have the same content
        let editor_content: String = editor.buffers[0].rope().to_string();
        assert_eq!(client_b.content(), editor_content);
    }
}
