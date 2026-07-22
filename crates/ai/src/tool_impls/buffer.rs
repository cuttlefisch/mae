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
    if editor.ai.mode == "plan" {
        return Err(
            "buffer_write is disabled in plan mode. Use create_plan to draft changes instead."
                .into(),
        );
    }

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
        // #355: direct rope mutation bumps `generation` but never escalates
        // `redraw_level` on its own -- without this, a subsequent pure
        // scroll/cursor-move frame can serve stale, misaligned syntax spans.
        editor.mark_full_redraw();
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
        editor.mark_full_redraw();
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
    let target_win_id = super::resolve_active_window_id(editor);
    let (buf_idx, row, col, scroll_offset) = editor
        .window_mgr
        .iter_windows()
        .find(|w| w.id == target_win_id)
        .map(|w| (w.buffer_idx, w.cursor_row, w.cursor_col, w.scroll_offset))
        .unwrap_or_else(|| {
            // Fallback: use ai_target_buffer_idx or active buffer.
            let idx = editor
                .ai
                .target_buffer_idx
                .unwrap_or_else(|| editor.active_buffer_idx());
            let win_data = editor
                .window_mgr
                .iter_windows()
                .find(|w| w.buffer_idx == idx)
                .map(|w| (w.cursor_row, w.cursor_col, w.scroll_offset))
                .unwrap_or((0, 0, 0));
            (idx, win_data.0, win_data.1, win_data.2)
        });
    let buf = &editor.buffers[buf_idx];

    let info = serde_json::json!({
        "buffer_name": buf.name,
        "cursor_row": row + 1,
        "cursor_col": col + 1,
        "line_count": buf.line_count(),
        "modified": buf.modified,
        "mode": format!("{:?}", editor.mode),
        "scroll_offset": scroll_offset,
        "viewport_height": editor.viewport_height,
    });
    Ok(info.to_string())
}

pub fn execute_file_read(args: &serde_json::Value) -> Result<String, String> {
    let raw_path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'path' argument")?;
    let path = mae_core::file_picker::expand_tilde(raw_path);
    let content = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "File read error: {} (path: {}). Hint: use absolute paths — call audit_configuration for correct config paths.",
            e, path
        )
    })?;
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
            // Find window(s) showing this buffer for targeting info.
            let window_ids: Vec<u32> = editor
                .window_mgr
                .iter_windows()
                .filter(|w| w.buffer_idx == i)
                .map(|w| w.id)
                .collect();
            let mut obj = serde_json::json!({
                "index": i,
                "name": buf.name,
                "modified": buf.modified,
                "active": i == editor.active_buffer_idx(),
                "line_count": buf.line_count(),
            });
            if !window_ids.is_empty() {
                obj["window_ids"] = serde_json::json!(window_ids);
            }
            obj
        })
        .collect();
    serde_json::to_string_pretty(&buffers).map_err(|e| e.to_string())
}

#[cfg(test)]
mod buffer_write_tests {
    use super::*;
    use mae_core::redraw::RedrawLevel;

    /// #355: `buffer_write` mutates the rope directly (bumping `generation`)
    /// but previously never escalated `redraw_level` -- leaving a stale
    /// syntax-span cache in place until the next keystroke. Regression guard
    /// for both mutation branches (replace-range and insert-before-line).
    #[test]
    fn execute_buffer_write_replace_range_escalates_redraw_level() {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\nline three\n");
        editor.redraw_level = RedrawLevel::None;

        execute_buffer_write(
            &mut editor,
            &serde_json::json!({"start_line": 2, "end_line": 3, "content": "replaced\n"}),
        )
        .unwrap();

        assert!(
            editor.redraw_level >= RedrawLevel::Full,
            "expected redraw_level escalated to Full after a direct rope \
             mutation, got {:?}",
            editor.redraw_level
        );
    }

    #[test]
    fn execute_buffer_write_insert_before_line_escalates_redraw_level() {
        let mut editor = Editor::new();
        editor.buffers[0].insert_text_at(0, "line one\nline two\n");
        editor.redraw_level = RedrawLevel::None;

        execute_buffer_write(
            &mut editor,
            &serde_json::json!({"start_line": 1, "content": "inserted\n"}),
        )
        .unwrap();

        assert!(
            editor.redraw_level >= RedrawLevel::Full,
            "expected redraw_level escalated to Full after a direct rope \
             mutation, got {:?}",
            editor.redraw_level
        );
    }
}
