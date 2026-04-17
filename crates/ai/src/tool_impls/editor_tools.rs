use mae_core::Editor;

pub fn execute_editor_state(editor: &Editor) -> Result<String, String> {
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

pub fn execute_window_layout(editor: &Editor) -> Result<String, String> {
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

pub fn execute_command_list(editor: &Editor) -> Result<String, String> {
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

pub fn execute_set_option(editor: &mut Editor, args: &serde_json::Value) -> Result<String, String> {
    let option = args
        .get("option")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'option' parameter")?;
    let value = args
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'value' parameter")?;
    match option {
        "theme" => {
            editor.set_theme_by_name(value);
            Ok(format!("Theme set to: {}", editor.theme.name))
        }
        "splash_art" => {
            editor.splash_art = Some(value.to_string());
            Ok(format!("Splash art set to: {}", value))
        }
        "line_numbers" => {
            editor.show_line_numbers = value == "true" || value == "on" || value == "1";
            Ok(format!("Line numbers: {}", editor.show_line_numbers))
        }
        "relative_line_numbers" => {
            editor.relative_line_numbers = value == "true" || value == "on" || value == "1";
            Ok(format!(
                "Relative line numbers: {}",
                editor.relative_line_numbers
            ))
        }
        "word_wrap" => {
            editor.word_wrap = value == "true" || value == "on" || value == "1";
            Ok(format!("Word wrap: {}", editor.word_wrap))
        }
        _ => Err(format!(
            "Unknown option: '{}'. Supported: 'theme', 'splash_art', 'line_numbers', 'relative_line_numbers', 'word_wrap'",
            option
        )),
    }
}

pub fn execute_debug_state(editor: &Editor) -> Result<String, String> {
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
