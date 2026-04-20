use mae_core::theme::{NamedColor, ThemeColor};
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
        "debug_panel_open": editor.buffers.iter().any(|b| b.kind == mae_core::buffer::BufferKind::Debug),
        "breakpoint_count": editor.debug_state.as_ref().map(|s| s.breakpoint_count()).unwrap_or(0),
        "command_count": editor.commands.len(),
        "renderer": editor.renderer_name,
        "git_branch": editor.git_branch,
        "project_root": editor.project.as_ref().map(|p| p.root.display().to_string()),
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

pub fn execute_get_option(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("all");
    if name == "all" || name.is_empty() {
        let options: Vec<serde_json::Value> = editor
            .option_registry
            .list()
            .iter()
            .filter_map(|def| {
                let (value, _) = editor.get_option(def.name)?;
                Some(serde_json::json!({
                    "name": def.name,
                    "value": value,
                    "type": def.kind.to_string(),
                    "default": def.default_value,
                    "doc": def.doc,
                }))
            })
            .collect();
        serde_json::to_string_pretty(&options).map_err(|e| e.to_string())
    } else {
        let (value, def) = editor
            .get_option(name)
            .ok_or_else(|| format!("Unknown option: '{}'", name))?;
        let info = serde_json::json!({
            "name": def.name,
            "value": value,
            "type": def.kind.to_string(),
            "default": def.default_value,
            "doc": def.doc,
        });
        serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
    }
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
    editor.set_option(option, value)
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

            // Include variables grouped by scope.
            let variables: serde_json::Value = state
                .variables
                .iter()
                .map(|(scope_name, vars)| {
                    let var_list: Vec<serde_json::Value> = vars
                        .iter()
                        .map(|v| {
                            serde_json::json!({
                                "name": v.name,
                                "value": v.value,
                                "type": v.var_type,
                                "variables_reference": v.variables_reference,
                            })
                        })
                        .collect();
                    (scope_name.clone(), serde_json::Value::Array(var_list))
                })
                .collect::<serde_json::Map<String, serde_json::Value>>()
                .into();

            // Include recent output (last 50 lines).
            let output_len = state.output_log.len();
            let output_start = output_len.saturating_sub(50);
            let recent_output: Vec<&str> = state.output_log[output_start..]
                .iter()
                .map(|s| s.as_str())
                .collect();

            let info = serde_json::json!({
                "target": format!("{:?}", state.target),
                "active_thread_id": state.active_thread_id,
                "threads": threads,
                "stack_frames": frames,
                "scopes": state.scopes.iter().map(|s| &s.name).collect::<Vec<_>>(),
                "variables": variables,
                "breakpoints": breakpoints,
                "stopped_location": state.stopped_location,
                "output_log": recent_output,
            });
            serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
        }
    }
}

fn theme_color_to_json(color: &Option<ThemeColor>) -> serde_json::Value {
    match color {
        None => serde_json::Value::Null,
        Some(ThemeColor::Rgb(r, g, b)) => serde_json::json!({
            "type": "rgb",
            "r": r,
            "g": g,
            "b": b,
        }),
        Some(ThemeColor::Named(named)) => {
            let name = match named {
                NamedColor::Black => "Black",
                NamedColor::Red => "Red",
                NamedColor::Green => "Green",
                NamedColor::Yellow => "Yellow",
                NamedColor::Blue => "Blue",
                NamedColor::Magenta => "Magenta",
                NamedColor::Cyan => "Cyan",
                NamedColor::White => "White",
                NamedColor::DarkGray => "DarkGray",
                NamedColor::LightRed => "LightRed",
                NamedColor::LightGreen => "LightGreen",
                NamedColor::LightYellow => "LightYellow",
                NamedColor::LightBlue => "LightBlue",
                NamedColor::LightMagenta => "LightMagenta",
                NamedColor::LightCyan => "LightCyan",
                NamedColor::Gray => "Gray",
            };
            // Also resolve to concrete RGB so callers get actionable values.
            let (r, g, b) = mae_core::theme::Theme::resolve_to_rgb(&ThemeColor::Named(*named));
            serde_json::json!({
                "type": "named",
                "name": name,
                "r": r,
                "g": g,
                "b": b,
            })
        }
    }
}

pub fn execute_theme_inspect(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'key' parameter")?;
    let style = editor.theme.style(key);
    let info = serde_json::json!({
        "fg": theme_color_to_json(&style.fg),
        "bg": theme_color_to_json(&style.bg),
        "bold": style.bold,
        "italic": style.italic,
        "dim": style.dim,
        "underline": style.underline,
    });
    serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
}

pub fn execute_shell_scrollback(
    editor: &Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let buf_idx = args
        .get("buffer_index")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or_else(|| editor.active_buffer_idx());
    let offset = args.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let lines = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(50) as usize;

    let viewport = editor
        .shell_viewports
        .get(&buf_idx)
        .ok_or_else(|| format!("No shell viewport data for buffer index {}", buf_idx))?;

    if viewport.is_empty() {
        return Ok(String::new());
    }

    let total = viewport.len();
    let start = total.saturating_sub(offset + lines);
    let end = total.saturating_sub(offset);
    let slice = &viewport[start..end];
    Ok(slice.join("\n"))
}

pub fn execute_mouse_event(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let event_type = args
        .get("event_type")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'event_type' parameter")?;

    match event_type {
        "scroll" => {
            let delta = args.get("delta").and_then(|v| v.as_i64()).unwrap_or(0) as i16;
            editor.handle_mouse_scroll(delta);
            Ok("ok".into())
        }
        "click" => {
            let row = args.get("row").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let col = args.get("col").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let button = match args
                .get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("left")
            {
                "right" => mae_core::MouseButton::Right,
                "middle" => mae_core::MouseButton::Middle,
                _ => mae_core::MouseButton::Left,
            };
            editor.handle_mouse_click(row, col, button);
            Ok("ok".into())
        }
        other => Err(format!(
            "Unknown event_type '{}': expected 'scroll' or 'click'",
            other
        )),
    }
}

pub fn execute_event_recording(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");

    match action {
        "start" => {
            editor.event_recorder.start_recording();
            Ok("Recording started".into())
        }
        "stop" => {
            editor.event_recorder.stop_recording();
            Ok(format!(
                "Recording stopped ({} events)",
                editor.event_recorder.event_count()
            ))
        }
        "status" => {
            let status = serde_json::json!({
                "recording": editor.event_recorder.is_recording(),
                "event_count": editor.event_recorder.event_count(),
                "duration_us": editor.event_recorder.duration_us(),
            });
            serde_json::to_string_pretty(&status).map_err(|e| e.to_string())
        }
        "dump" => {
            let last_n = args.get("last_n").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
            Ok(editor.event_recorder.dump_json(last_n))
        }
        other => Err(format!(
            "Unknown action: '{}'. Use: start, stop, status, dump",
            other
        )),
    }
}

pub fn execute_render_inspect(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let _row = args
        .get("row")
        .and_then(|v| v.as_u64())
        .ok_or("Missing 'row' parameter")? as usize;
    let _col = args
        .get("col")
        .and_then(|v| v.as_u64())
        .ok_or("Missing 'col' parameter")? as usize;

    // Window rects are computed at render time from a total area we don't store.
    // For single-window layouts (the common case), the focused window covers
    // the entire content area. For splits, we report the focused window since
    // we cannot resolve exact coordinates without the render area.
    let win = editor.window_mgr.focused_window();
    let buf = editor.buffers.get(win.buffer_idx);
    let (buffer_name, buffer_kind) = match buf {
        Some(b) => (
            serde_json::Value::String(b.name.clone()),
            format!("{:?}", b.kind),
        ),
        None => (serde_json::Value::Null, "Unknown".into()),
    };

    let style = editor.theme.style("ui.text");
    let info = serde_json::json!({
        "buffer_name": buffer_name,
        "buffer_kind": buffer_kind,
        "theme_fg": theme_color_to_json(&style.fg),
        "theme_bg": theme_color_to_json(&style.bg),
    });
    serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
}

pub fn execute_trigger_hook(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let hook_name = args
        .get("hook_name")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'hook_name' parameter")?;

    if !mae_core::hooks::HOOK_NAMES.contains(&hook_name) {
        return Err(format!("Invalid hook name: '{}'", hook_name));
    }

    editor.fire_hook(hook_name);
    Ok(format!("Hook '{}' triggered", hook_name))
}

pub fn execute_org_cycle(editor: &mut Editor) -> Result<String, String> {
    editor.org_cycle();
    Ok(editor.status_msg.clone())
}

pub fn execute_org_todo_cycle(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    let forward = args
        .get("forward")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    editor.org_todo_cycle(forward);
    Ok(editor.status_msg.clone())
}

pub fn execute_org_open_link(editor: &mut Editor) -> Result<String, String> {
    editor.org_open_link();
    Ok(editor.status_msg.clone())
}
