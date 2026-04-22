//! Shell terminal AI tool implementations.
//!
//! These tools let the AI agent observe and interact with embedded terminal
//! buffers. `shell_list` and `shell_read_output` read cached state from
//! `Editor`; `shell_send_input` queues an intent for the binary to drain.

use mae_core::{BufferKind, Editor};
use serde_json::Value;

/// List all shell terminal buffers with their status.
pub fn execute_shell_list(editor: &Editor) -> Result<String, String> {
    let mut entries = Vec::new();
    for (idx, buf) in editor.buffers.iter().enumerate() {
        if buf.kind == BufferKind::Shell {
            let has_viewport = editor.shell_viewports.contains_key(&idx);
            entries.push(serde_json::json!({
                "buffer_index": idx,
                "name": buf.name,
                "active": idx == editor.active_buffer_idx(),
                "running": has_viewport,
            }));
        }
    }
    if entries.is_empty() {
        Ok("No active shell terminals. Use the `terminal` command to open one.".into())
    } else {
        serde_json::to_string_pretty(&entries).map_err(|e| e.to_string())
    }
}

/// Read recent output from a shell terminal's cached viewport.
pub fn execute_shell_read_output(editor: &Editor, args: &Value) -> Result<String, String> {
    let buf_idx = args
        .get("buffer_index")
        .and_then(|v| v.as_u64())
        .ok_or("Missing required parameter: buffer_index")? as usize;

    let max_lines = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(24) as usize;

    // Verify it's a shell buffer.
    if buf_idx >= editor.buffers.len() || editor.buffers[buf_idx].kind != BufferKind::Shell {
        return Err(format!("Buffer {} is not a shell terminal", buf_idx));
    }

    let viewport = editor.shell_viewports.get(&buf_idx).ok_or_else(|| {
        format!(
            "Shell terminal {} has no cached output (may have exited)",
            buf_idx
        )
    })?;

    // Return last max_lines.
    let start = viewport.len().saturating_sub(max_lines);
    let lines: Vec<&str> = viewport[start..].iter().map(|s| s.as_str()).collect();
    Ok(lines.join("\n"))
}

/// Queue input to be sent to a shell terminal.
pub fn execute_shell_send_input(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let buf_idx = args
        .get("buffer_index")
        .and_then(|v| v.as_u64())
        .ok_or("Missing required parameter: buffer_index")? as usize;

    let input = args
        .get("input")
        .and_then(|v| v.as_str())
        .ok_or("Missing required parameter: input")?;

    // Verify it's a shell buffer.
    if buf_idx >= editor.buffers.len() || editor.buffers[buf_idx].kind != BufferKind::Shell {
        return Err(format!("Buffer {} is not a shell terminal", buf_idx));
    }

    // Process escape sequences in the input string.
    // Supported: \n → CR (Enter), \t → tab, \r → CR, \e → ESC
    let processed = input
        .replace("\\n", "\r") // \n → carriage return (Enter)
        .replace("\\r", "\r") // \r → carriage return
        .replace("\\t", "\t") // \t → tab
        .replace("\\e", "\x1b"); // \e → ESC

    editor.pending_shell_inputs.push((buf_idx, processed));
    Ok(format!("Input queued for shell terminal {}", buf_idx))
}

/// Queue an intent to spawn a new shell terminal buffer.
pub fn execute_terminal_spawn(editor: &mut Editor, args: &Value) -> Result<String, String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("*Terminal*")
        .to_string();
    let command = args
        .get("command")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let buf = mae_core::Buffer::new_shell(&name);
    editor.buffers.push(buf);
    let idx = editor.buffers.len() - 1;

    if let Some(cmd) = command {
        editor.pending_agent_spawns.push((idx, cmd));
        Ok(format!(
            "Agent terminal spawning with command in buffer {}",
            idx
        ))
    } else {
        editor.pending_shell_spawns.push(idx);
        Ok(format!("Interactive terminal spawning in buffer {}", idx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_core::Editor;

    #[test]
    fn shell_list_empty() {
        let editor = Editor::new();
        let result = execute_shell_list(&editor).unwrap();
        assert!(result.contains("No active shell terminals"));
    }

    #[test]
    fn shell_read_output_invalid_buffer() {
        let editor = Editor::new();
        let args = serde_json::json!({"buffer_index": 0});
        let result = execute_shell_read_output(&editor, &args);
        assert!(result.is_err());
    }

    #[test]
    fn shell_send_input_invalid_buffer() {
        let mut editor = Editor::new();
        let args = serde_json::json!({"buffer_index": 0, "input": "ls\n"});
        let result = execute_shell_send_input(&mut editor, &args);
        assert!(result.is_err());
    }

    #[test]
    fn shell_list_with_shell_buffer() {
        let mut editor = Editor::new();
        let buf = mae_core::Buffer::new_shell("*Terminal 1*");
        editor.buffers.push(buf);
        let result = execute_shell_list(&editor).unwrap();
        assert!(result.contains("Terminal 1"));
    }

    #[test]
    fn shell_read_output_with_cached_viewport() {
        let mut editor = Editor::new();
        let buf = mae_core::Buffer::new_shell("*Terminal 1*");
        editor.buffers.push(buf);
        let idx = editor.buffers.len() - 1;
        editor.shell_viewports.insert(
            idx,
            vec!["$ ls".into(), "file1.rs".into(), "file2.rs".into()],
        );
        let args = serde_json::json!({"buffer_index": idx, "lines": 2});
        let result = execute_shell_read_output(&editor, &args).unwrap();
        assert!(result.contains("file1.rs"));
        assert!(result.contains("file2.rs"));
        assert!(!result.contains("$ ls"));
    }

    #[test]
    fn shell_send_input_queues_intent() {
        let mut editor = Editor::new();
        let buf = mae_core::Buffer::new_shell("*Terminal 1*");
        editor.buffers.push(buf);
        let idx = editor.buffers.len() - 1;
        // Need viewport to indicate it's running.
        editor.shell_viewports.insert(idx, vec![]);
        let args = serde_json::json!({"buffer_index": idx, "input": "ls\\n"});
        let result = execute_shell_send_input(&mut editor, &args).unwrap();
        assert!(result.contains("queued"));
        assert_eq!(editor.pending_shell_inputs.len(), 1);
        assert_eq!(editor.pending_shell_inputs[0].0, idx);
        // \n should be converted to \r
        assert_eq!(editor.pending_shell_inputs[0].1, "ls\r");
    }
}
