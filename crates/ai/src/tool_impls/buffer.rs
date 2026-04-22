use mae_core::Editor;

use super::resolve_buffer_idx;

pub fn execute_buffer_read(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
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

pub fn execute_buffer_write(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
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
        editor.recompute_search_matches();
        editor.clamp_all_cursors();
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
        editor.recompute_search_matches();
        editor.clamp_all_cursors();
        Ok(format!(
            "Inserted at line {} ({} chars)",
            start_line,
            content.len()
        ))
    }
}

pub fn execute_cursor_info(editor: &Editor) -> Result<String, String> {
    let buf_idx = editor
        .ai_target_buffer_idx
        .unwrap_or_else(|| editor.active_buffer_idx());
    let buf = &editor.buffers[buf_idx];

    // Find a window showing this buffer to get a cursor position.
    // If multiple windows show it, we use the first one.
    // If no window shows it (unlikely for active/target), we default to (0,0).
    let (row, col) = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.buffer_idx == buf_idx)
        .map(|w| (w.cursor_row, w.cursor_col))
        .unwrap_or((0, 0));

    let info = serde_json::json!({
        "buffer_name": buf.name,
        "cursor_row": row + 1,
        "cursor_col": col + 1,
        "line_count": buf.line_count(),
        "modified": buf.modified,
        "mode": format!("{:?}", editor.mode),
    });
    Ok(info.to_string())
}

pub fn execute_file_read(args: &serde_json::Value) -> Result<String, String> {
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

pub fn execute_list_buffers(editor: &Editor) -> Result<String, String> {
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
