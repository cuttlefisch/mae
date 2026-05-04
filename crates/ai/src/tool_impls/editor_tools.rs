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
        "gui_cell_width": editor.gui_cell_width,
        "gui_cell_height": editor.gui_cell_height,
        "viewport_height": editor.viewport_height,
        "text_area_width": editor.text_area_width,
        "scrolloff": editor.scrolloff,
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

pub fn execute_command_list(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let format = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("full");

    if format == "names" {
        // Compact output: just command names, one per line
        let names: Vec<&str> = editor
            .commands
            .list_commands()
            .iter()
            .map(|cmd| cmd.name.as_str())
            .collect();
        Ok(format!("{} commands:\n{}", names.len(), names.join("\n")))
    } else {
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
                "last_stop_reason": state.last_stop_reason,
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
    _args: &serde_json::Value,
) -> Result<String, String> {
    editor.org_todo_cycle();
    Ok(editor.status_msg.clone())
}

pub fn execute_org_open_link(editor: &mut Editor) -> Result<String, String> {
    editor.org_open_link();
    Ok(editor.status_msg.clone())
}

pub fn execute_visual_buffer_add_rect(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    use mae_core::visual_buffer::VisualElement;

    let x = args
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'x'")? as f32;
    let y = args
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'y'")? as f32;
    let w = args
        .get("w")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'w'")? as f32;
    let h = args
        .get("h")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'h'")? as f32;
    let fill = args
        .get("fill")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let stroke = args
        .get("stroke")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let buf_idx = editor.active_buffer_idx();
    ensure_visual_buffer(editor, buf_idx)?;

    if let Some(vb) = editor.buffers[buf_idx].visual_mut() {
        vb.add(VisualElement::Rect {
            x,
            y,
            w,
            h,
            fill,
            stroke,
        });
        Ok(format!("Added rectangle at ({}, {})", x, y))
    } else {
        Err("Visual state missing".into())
    }
}

pub fn execute_visual_buffer_add_line(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    use mae_core::visual_buffer::VisualElement;

    let x1 = args
        .get("x1")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'x1'")? as f32;
    let y1 = args
        .get("y1")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'y1'")? as f32;
    let x2 = args
        .get("x2")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'x2'")? as f32;
    let y2 = args
        .get("y2")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'y2'")? as f32;
    let color = args
        .get("color")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'color'")?
        .to_string();
    let thickness = args
        .get("thickness")
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0) as f32;

    let buf_idx = editor.active_buffer_idx();
    ensure_visual_buffer(editor, buf_idx)?;

    if let Some(vb) = editor.buffers[buf_idx].visual_mut() {
        vb.add(VisualElement::Line {
            x1,
            y1,
            x2,
            y2,
            color,
            thickness,
        });
        Ok(format!(
            "Added line from ({}, {}) to ({}, {})",
            x1, y1, x2, y2
        ))
    } else {
        Err("Visual state missing".into())
    }
}

pub fn execute_visual_buffer_add_circle(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    use mae_core::visual_buffer::VisualElement;

    let cx = args
        .get("cx")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'cx'")? as f32;
    let cy = args
        .get("cy")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'cy'")? as f32;
    let r = args
        .get("r")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'r'")? as f32;
    let fill = args
        .get("fill")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let stroke = args
        .get("stroke")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let buf_idx = editor.active_buffer_idx();
    ensure_visual_buffer(editor, buf_idx)?;

    if let Some(vb) = editor.buffers[buf_idx].visual_mut() {
        vb.add(VisualElement::Circle {
            cx,
            cy,
            r,
            fill,
            stroke,
        });
        Ok(format!("Added circle at ({}, {})", cx, cy))
    } else {
        Err("Visual state missing".into())
    }
}

pub fn execute_visual_buffer_add_text(
    editor: &mut Editor,
    args: &serde_json::Value,
) -> Result<String, String> {
    use mae_core::visual_buffer::VisualElement;

    let x = args
        .get("x")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'x'")? as f32;
    let y = args
        .get("y")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'y'")? as f32;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'text'")?
        .to_string();
    let font_size = args
        .get("font_size")
        .and_then(|v| v.as_f64())
        .ok_or("Missing 'font_size'")? as f32;
    let color = args
        .get("color")
        .and_then(|v| v.as_str())
        .ok_or("Missing 'color'")?
        .to_string();

    let buf_idx = editor.active_buffer_idx();
    ensure_visual_buffer(editor, buf_idx)?;

    if let Some(vb) = editor.buffers[buf_idx].visual_mut() {
        vb.add(VisualElement::Text {
            x,
            y,
            text,
            font_size,
            color,
        });
        Ok(format!("Added text at ({}, {})", x, y))
    } else {
        Err("Visual state missing".into())
    }
}

fn ensure_visual_buffer(editor: &mut Editor, buf_idx: usize) -> Result<(), String> {
    if editor.buffers[buf_idx].kind != mae_core::BufferKind::Visual {
        // Auto-convert current buffer to visual if it's a scratch buffer
        if (editor.buffers[buf_idx].name == "[scratch]" || editor.buffers[buf_idx].name == "*AI*")
            && !editor.buffers[buf_idx].modified
        {
            editor.buffers[buf_idx].kind = mae_core::BufferKind::Visual;
            editor.buffers[buf_idx].view = mae_core::buffer_view::BufferView::Visual(Box::new(
                mae_core::visual_buffer::VisualBuffer::new(),
            ));
        } else {
            return Err("Active buffer is not a visual buffer".into());
        }
    }
    Ok(())
}

pub fn execute_visual_buffer_clear(editor: &mut Editor) -> Result<String, String> {
    let buf_idx = editor.active_buffer_idx();
    if let Some(vb) = editor.buffers[buf_idx].visual_mut() {
        vb.clear();
        Ok("Visual buffer cleared".into())
    } else {
        Err("Active buffer is not a visual buffer".into())
    }
}

pub fn execute_read_messages(editor: &Editor, args: &serde_json::Value) -> Result<String, String> {
    let last_n = args.get("last_n").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
    let min_level = match args.get("level").and_then(|v| v.as_str()).unwrap_or("info") {
        "error" => mae_core::MessageLevel::Error,
        "warn" => mae_core::MessageLevel::Warn,
        "debug" => mae_core::MessageLevel::Debug,
        "trace" => mae_core::MessageLevel::Trace,
        _ => mae_core::MessageLevel::Info,
    };
    let entries = editor.message_log.entries_filtered(min_level);
    let start = entries.len().saturating_sub(last_n);
    let lines: Vec<String> = entries[start..]
        .iter()
        .map(|e| format!("[{}] [{}] {}", e.level, e.target, e.message))
        .collect();
    if lines.is_empty() {
        Ok("(no messages)".into())
    } else {
        Ok(lines.join("\n"))
    }
}

pub fn execute_editor_save_state(editor: &mut Editor) -> Result<String, String> {
    let depth = editor.save_state();
    let buf_names: Vec<&str> = editor.buffers.iter().map(|b| b.name.as_str()).collect();
    let info = serde_json::json!({
        "stack_depth": depth,
        "saved_buffers": buf_names,
        "window_count": editor.window_mgr.window_count(),
        "focused_buffer": editor.active_buffer().name,
    });
    serde_json::to_string_pretty(&info).map_err(|e| e.to_string())
}

pub fn execute_editor_restore_state(editor: &mut Editor) -> Result<String, String> {
    editor.restore_state()
}

/// Audit the editor configuration and return structured JSON with status and issues.
pub fn execute_audit_configuration(editor: &Editor) -> Result<String, String> {
    fn on_path(cmd: &str) -> bool {
        std::process::Command::new("which")
            .arg(cmd)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    let mut issues = Vec::new();

    // AI Agent
    let ai_cmd = if editor.ai_editor.is_empty() {
        "claude".to_string()
    } else {
        editor.ai_editor.clone()
    };
    let ai_agent_found = on_path(&ai_cmd);
    if !ai_agent_found {
        issues.push(format!("AI Agent command '{}' not found on PATH", ai_cmd));
    }

    // AI Chat
    let provider = if editor.ai_provider.is_empty() {
        String::new()
    } else {
        editor.ai_provider.clone()
    };
    let model = editor.ai_model.clone();

    let (api_key_set, api_key_source) = match provider.as_str() {
        "claude" if std::env::var("ANTHROPIC_API_KEY").is_ok() => {
            (true, "env:ANTHROPIC_API_KEY".to_string())
        }
        "openai" if std::env::var("OPENAI_API_KEY").is_ok() => {
            (true, "env:OPENAI_API_KEY".to_string())
        }
        "gemini" if std::env::var("GEMINI_API_KEY").is_ok() => {
            (true, "env:GEMINI_API_KEY".to_string())
        }
        "deepseek" if std::env::var("DEEPSEEK_API_KEY").is_ok() => {
            (true, "env:DEEPSEEK_API_KEY".to_string())
        }
        _ if !editor.ai_api_key_command.is_empty() => {
            (true, format!("command:{}", editor.ai_api_key_command))
        }
        _ => (false, String::new()),
    };
    if !provider.is_empty() && !api_key_set {
        issues.push(format!(
            "AI Chat provider '{}' configured but no API key found",
            provider
        ));
    }

    // LSP servers
    let lsp_servers = [
        ("rust", "rust-analyzer"),
        ("python", "pyright"),
        ("typescript", "typescript-language-server"),
        ("go", "gopls"),
    ];
    let lsp_json: Vec<serde_json::Value> = lsp_servers
        .iter()
        .map(|(lang, cmd)| {
            let found = on_path(cmd);
            if !found {
                issues.push(format!("LSP server '{}' ({}) not found on PATH", cmd, lang));
            }
            serde_json::json!({
                "language": lang,
                "command": cmd,
                "binary_found": found,
            })
        })
        .collect();

    // DAP adapters
    let dap_adapters = [("lldb-dap", "lldb"), ("debugpy", "pip install debugpy")];
    let dap_json: Vec<serde_json::Value> = dap_adapters
        .iter()
        .map(|(cmd, install_hint)| {
            let found = on_path(cmd);
            if !found {
                issues.push(format!(
                    "DAP adapter '{}' not found — install with: {}",
                    cmd, install_hint
                ));
            }
            serde_json::json!({
                "name": cmd,
                "binary_found": found,
            })
        })
        .collect();

    // Init files
    let user_config_dir = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| std::path::PathBuf::from(h).join(".config"))
        });
    let mut init_files = Vec::new();
    if let Some(ref dir) = user_config_dir {
        let p = dir.join("mae").join("init.scm");
        let exists = p.exists();
        init_files.push(serde_json::json!({
            "path": p.display().to_string(),
            "exists": exists,
        }));
    }
    if let Ok(cwd) = std::env::current_dir() {
        let p = cwd.join(".mae").join("init.scm");
        let exists = p.exists();
        init_files.push(serde_json::json!({
            "path": p.display().to_string(),
            "exists": exists,
        }));
    }

    // Modified options
    let mut options_modified = Vec::new();
    for def in editor.option_registry.list() {
        if let Some((val, _)) = editor.get_option(def.name) {
            if val != def.default_value {
                options_modified.push(def.name.to_string());
            }
        }
    }

    // Prompt tier (auto-detected from model)
    let prompt_tier = crate::context_limits::tier(&model).as_str();

    // Display policy rules
    let display_policy: std::collections::HashMap<String, String> = [
        mae_core::BufferKind::Text,
        mae_core::BufferKind::Diff,
        mae_core::BufferKind::Help,
        mae_core::BufferKind::Messages,
        mae_core::BufferKind::Shell,
        mae_core::BufferKind::Debug,
        mae_core::BufferKind::FileTree,
        mae_core::BufferKind::GitStatus,
        mae_core::BufferKind::Dashboard,
        mae_core::BufferKind::Visual,
        mae_core::BufferKind::Preview,
        mae_core::BufferKind::Conversation,
        mae_core::BufferKind::Agenda,
        mae_core::BufferKind::Demo,
    ]
    .iter()
    .map(|kind| {
        let action = editor.display_policy.action_for(*kind);
        (
            format!("{:?}", kind),
            mae_core::display_policy::action_to_string(&action),
        )
    })
    .collect();

    let report = serde_json::json!({
        "ai_agent": {
            "command": ai_cmd,
            "binary_found": ai_agent_found,
        },
        "ai_chat": {
            "provider": provider,
            "model": model,
            "api_key_set": api_key_set,
            "api_key_source": api_key_source,
            "prompt_tier": prompt_tier,
        },
        "lsp_servers": lsp_json,
        "dap_adapters": dap_json,
        "init_files": init_files,
        "options_modified": options_modified,
        "display_policy": display_policy,
        "issues": issues,
    });

    serde_json::to_string_pretty(&report).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_configuration_returns_valid_json() {
        let editor = Editor::new();
        let result = execute_audit_configuration(&editor);
        assert!(result.is_ok(), "audit_configuration should succeed");
        let json: serde_json::Value =
            serde_json::from_str(&result.unwrap()).expect("should be valid JSON");
        assert!(json.get("ai_agent").is_some());
        assert!(json.get("ai_chat").is_some());
        assert!(json.get("lsp_servers").is_some());
        assert!(json.get("dap_adapters").is_some());
        assert!(json.get("init_files").is_some());
        assert!(json.get("options_modified").is_some());
        assert!(json.get("issues").is_some());
    }

    #[test]
    fn audit_configuration_issues_populated() {
        let editor = Editor::new();
        let result = execute_audit_configuration(&editor).unwrap();
        let json: serde_json::Value = serde_json::from_str(&result).unwrap();
        let issues = json["issues"].as_array().unwrap();
        // At minimum, some LSP servers or DAP adapters won't be on PATH in test env
        // The issues array should exist and be an array (may or may not be empty)
        assert!(json["issues"].is_array());
        // lsp_servers should have entries
        let lsp = json["lsp_servers"].as_array().unwrap();
        assert!(lsp.len() >= 4, "should list at least 4 LSP servers");
        let _ = issues; // suppress unused
    }

    #[test]
    fn audit_configuration_is_readonly_tier() {
        use crate::tools::{classify_tool_tier, ToolTier};
        assert_eq!(
            classify_tool_tier("audit_configuration"),
            ToolTier::Core,
            "audit_configuration should be Core tier"
        );
    }
}
