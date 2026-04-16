use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_ai::AiCommand;
use mae_core::{CommandSource, Editor, Key, KeyPress, LookupResult, Mode};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, warn};

use crate::bootstrap::load_ai_config;

/// Convert a crossterm KeyEvent into a mae_core KeyPress.
pub fn crossterm_to_keypress(key: &KeyEvent) -> Option<KeyPress> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let mae_key = match key.code {
        KeyCode::Char(ch) => Key::Char(ch),
        KeyCode::Esc => Key::Escape,
        KeyCode::Enter => Key::Enter,
        KeyCode::Backspace => Key::Backspace,
        KeyCode::Tab => Key::Tab,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Home => Key::Home,
        KeyCode::End => Key::End,
        KeyCode::PageUp => Key::PageUp,
        KeyCode::PageDown => Key::PageDown,
        KeyCode::Delete => Key::Delete,
        KeyCode::F(n) => Key::F(n),
        _ => return None,
    };

    Some(KeyPress {
        key: mae_key,
        ctrl,
        alt,
    })
}

pub fn handle_key(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    if editor.mode != Mode::Command {
        editor.status_msg.clear();
    }

    let mode_before = editor.mode;

    // --- Macro recording intercept ---
    // While recording, capture every keystroke into macro_log before dispatch.
    // Exception: a bare `q` in Normal mode with no pending prefix stops recording.
    if editor.macro_recording {
        let is_stop_key = matches!(key.code, KeyCode::Char('q'))
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && editor.mode == Mode::Normal
            && pending_keys.is_empty()
            && editor.pending_char_command.is_none();
        if is_stop_key {
            editor.stop_recording();
            return;
        }
        if let Some(kp) = crossterm_to_keypress(&key) {
            editor.macro_log.push(kp);
        }
    }

    match editor.mode {
        Mode::Normal => handle_normal_mode(editor, scheme, key, pending_keys),
        Mode::Insert => handle_insert_mode(editor, scheme, key, pending_keys),
        Mode::Visual(_) => handle_visual_mode(editor, scheme, key, pending_keys),
        Mode::Command => handle_command_mode(editor, scheme, key, pending_keys, ai_tx),
        Mode::ConversationInput => {
            handle_conversation_input(editor, key, ai_tx);
        }
        Mode::Search => handle_search_mode(editor, key),
        Mode::FilePicker => handle_file_picker_mode(editor, key),
    }

    if editor.mode != mode_before {
        pending_keys.clear();
    }
}

/// Dispatch a command by name, handling both builtins and Scheme commands.
fn dispatch_command(editor: &mut Editor, scheme: &mut SchemeRuntime, name: &str) {
    let source = editor.commands.get(name).map(|c| c.source.clone());

    match source {
        Some(CommandSource::Builtin) => {
            debug!(command = name, source = "builtin", "dispatching command");
            editor.dispatch_builtin(name);
        }
        Some(CommandSource::Scheme(fn_name)) => {
            debug!(command = name, scheme_fn = %fn_name, "dispatching scheme command");
            scheme.inject_editor_state(editor);
            match scheme.call_function(&fn_name) {
                Ok(result) => {
                    scheme.apply_to_editor(editor);
                    if !result.is_empty() {
                        editor.set_status(result);
                    }
                }
                Err(e) => {
                    error!(command = name, scheme_fn = %fn_name, error = %e, "scheme command failed");
                    editor.set_status(format!("Scheme error: {}", e));
                }
            }
        }
        None => {
            if !editor.dispatch_builtin(name) {
                warn!(command = name, "unknown command");
                editor.set_status(format!("Unknown command: {}", name));
            }
        }
    }
}

fn handle_search_mode(editor: &mut Editor, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            editor.mode = Mode::Normal;
            editor.search_input.clear();
            editor.search_state.highlight_active = false;
        }
        KeyCode::Enter => {
            editor.mode = Mode::Normal;
            editor.execute_search();
        }
        KeyCode::Backspace => {
            if editor.search_input.is_empty() {
                editor.mode = Mode::Normal;
            } else {
                editor.search_input.pop();
            }
        }
        KeyCode::Char(ch) => {
            editor.search_input.push(ch);
        }
        _ => {}
    }
}

fn handle_file_picker_mode(editor: &mut Editor, key: KeyEvent) {
    let picker = match editor.file_picker.as_mut() {
        Some(p) => p,
        None => {
            editor.mode = Mode::Normal;
            return;
        }
    };

    match key.code {
        KeyCode::Esc => {
            editor.file_picker = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            if let Some(path) = picker.selected_path() {
                editor.file_picker = None;
                editor.mode = Mode::Normal;
                editor.open_file(&path.to_string_lossy());
            } else {
                editor.file_picker = None;
                editor.mode = Mode::Normal;
                editor.set_status("No file selected");
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            picker.move_up();
        }
        KeyCode::Down | KeyCode::Tab => {
            picker.move_down();
        }
        KeyCode::Backspace => {
            if picker.query.is_empty() {
                editor.file_picker = None;
                editor.mode = Mode::Normal;
            } else {
                picker.query.pop();
                picker.update_filter();
            }
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_up();
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_down();
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.file_picker = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Char(ch) => {
            picker.query.push(ch);
            picker.update_filter();
        }
        _ => {}
    }
}

fn handle_keymap_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        editor.running = false;
        return;
    }

    let Some(kp) = crossterm_to_keypress(&key) else {
        return;
    };

    pending_keys.push(kp);

    let mode_name = match editor.mode {
        Mode::Normal => "normal",
        Mode::Insert => "insert",
        Mode::Visual(_) => "visual",
        Mode::Command | Mode::ConversationInput | Mode::Search | Mode::FilePicker => "command",
    };

    let result = editor
        .keymaps
        .get(mode_name)
        .map(|km| km.lookup(pending_keys))
        .unwrap_or(LookupResult::None);

    match result {
        LookupResult::Exact(cmd) => {
            let cmd = cmd.to_string();
            pending_keys.clear();
            editor.which_key_prefix.clear();
            dispatch_command(editor, scheme, &cmd);
        }
        LookupResult::Prefix => {
            editor.which_key_prefix = pending_keys.clone();
        }
        LookupResult::None => {
            pending_keys.clear();
            if !editor.which_key_prefix.is_empty() {
                editor.set_status("Key not bound");
            }
            editor.which_key_prefix.clear();
        }
    }
}

fn handle_normal_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    // If a char-argument command is pending (f/F/t/T or text objects), capture the next char
    if let Some(cmd) = editor.pending_char_command.take() {
        if let KeyCode::Char(ch) = key.code {
            // Try text object dispatch first, then fall back to char motion
            if !editor.dispatch_text_object(&cmd, ch) {
                editor.dispatch_char_motion(&cmd, ch);
            }
        }
        // Any key (including Escape) clears the pending state
        return;
    }

    // Count prefix accumulation: digits 1-9 start a count, 0 continues it
    if let KeyCode::Char(ch @ '1'..='9') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && pending_keys.is_empty()
        {
            let digit = (ch as usize) - ('0' as usize);
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10 + digit).min(99999));
            return;
        }
    }
    if let KeyCode::Char('0') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && editor.count_prefix.is_some()
            && pending_keys.is_empty()
        {
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10).min(99999));
            return;
        }
    }

    // Escape dismisses which-key popup if active, and clears count prefix
    if key.code == KeyCode::Esc {
        editor.count_prefix = None;
        if !editor.which_key_prefix.is_empty() {
            pending_keys.clear();
            editor.which_key_prefix.clear();
            return;
        }
    }
    handle_keymap_mode(editor, scheme, key, pending_keys);
}

fn handle_visual_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    // Handle pending char-argument commands (f/F/t/T or text objects)
    if let Some(cmd) = editor.pending_char_command.take() {
        if let KeyCode::Char(ch) = key.code {
            if !editor.dispatch_text_object(&cmd, ch) {
                editor.dispatch_char_motion(&cmd, ch);
            }
        }
        return;
    }

    // Count prefix accumulation (same as normal mode)
    if let KeyCode::Char(ch @ '1'..='9') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && pending_keys.is_empty()
        {
            let digit = (ch as usize) - ('0' as usize);
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10 + digit).min(99999));
            return;
        }
    }
    if let KeyCode::Char('0') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && editor.count_prefix.is_some()
            && pending_keys.is_empty()
        {
            let current = editor.count_prefix.unwrap_or(0);
            editor.count_prefix = Some((current * 10).min(99999));
            return;
        }
    }

    if key.code == KeyCode::Esc {
        editor.count_prefix = None;
    }

    handle_keymap_mode(editor, scheme, key, pending_keys);
}

fn handle_insert_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    // If the completion popup is visible, Tab/Ctrl-n/Ctrl-p navigate it.
    // When the popup is not visible, Tab falls through to keymap (which will
    // find no binding and do nothing, which is acceptable for now).
    let popup_open = !editor.completion_items.is_empty();

    match key.code {
        KeyCode::Tab if popup_open => {
            editor.lsp_accept_completion();
            return;
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) && popup_open => {
            editor.lsp_complete_next();
            return;
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) && popup_open => {
            editor.lsp_complete_prev();
            return;
        }
        KeyCode::Esc if popup_open => {
            editor.lsp_dismiss_completion();
            // Also exit insert mode (fall through to keymap which handles Esc).
            handle_keymap_mode(editor, scheme, key, pending_keys);
            return;
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, ch);
            // Trigger completion after word characters.
            if ch.is_alphanumeric() || ch == '_' {
                editor.lsp_request_completion();
            } else {
                // Non-word character dismisses popup.
                editor.lsp_dismiss_completion();
            }
            return;
        }
        KeyCode::Enter => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, '\n');
            editor.lsp_dismiss_completion();
            return;
        }
        KeyCode::Backspace => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_backward(win);
            // Re-trigger completion after backspace (word may still be valid).
            editor.lsp_request_completion();
            return;
        }
        _ => {}
    }
    handle_keymap_mode(editor, scheme, key, pending_keys);
}

fn handle_conversation_input(
    editor: &mut Editor,
    key: KeyEvent,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    match key.code {
        KeyCode::Esc => {
            editor.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            let buf_idx = editor.active_buffer_idx();
            let mut input = String::new();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                input = conv.input_line.clone();
                if !input.is_empty() {
                    conv.push_user(&input);
                    conv.input_line.clear();
                    conv.streaming = true;
                    conv.streaming_start = Some(std::time::Instant::now());
                }
            }
            if !input.is_empty() {
                if let Some(tx) = ai_tx {
                    if tx.try_send(AiCommand::Prompt(input)).is_err() {
                        warn!("AI command channel full or closed — prompt dropped");
                    }
                    editor.set_status("[AI] Thinking...");
                } else {
                    warn!("AI prompt submitted but no AI provider configured");
                    editor
                        .set_status("AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
                    if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                        conv.streaming = false;
                        conv.streaming_start = None;
                    }
                }
            }
            editor.mode = Mode::Normal;
        }
        KeyCode::Backspace => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_line.pop();
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Cancel streaming if active, otherwise exit mode
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                if conv.streaming {
                    info!("user cancelled AI streaming");
                    conv.streaming = false;
                    conv.streaming_start = None;
                    conv.push_system("[cancelled]");
                    if let Some(tx) = ai_tx {
                        if tx.try_send(AiCommand::Cancel).is_err() {
                            warn!("failed to send cancel to AI session");
                        }
                    }
                    return;
                }
            }
            editor.running = false;
        }
        KeyCode::Char(ch) => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_line.push(ch);
            }
        }
        _ => {}
    }
}

pub fn handle_command_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    pending_keys.clear();
    match key.code {
        KeyCode::Esc => {
            editor.mode = Mode::Normal;
            editor.command_line.clear();
        }
        KeyCode::Enter => {
            let cmd = editor.command_line.clone();
            editor.mode = Mode::Normal;
            editor.command_line.clear();

            // Record in command history before executing
            editor.push_command_history(&cmd);

            // :ai-status — show AI configuration
            if cmd == "ai-status" {
                let config = load_ai_config();
                if let Some(ref cfg) = config {
                    editor.set_status(format!(
                        "AI: provider={}, model={}, connected={}",
                        cfg.provider_type,
                        cfg.model,
                        ai_tx.is_some()
                    ));
                } else {
                    editor.set_status(
                        "AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY env var.",
                    );
                }
                return;
            }

            // :ai <prompt> — send to AI agent
            if let Some(prompt) = cmd.strip_prefix("ai ") {
                let prompt = prompt.trim();
                if prompt.is_empty() {
                    editor.set_status("Usage: :ai <prompt>");
                    return;
                }
                if let Some(tx) = ai_tx {
                    info!(
                        prompt_len = prompt.len(),
                        "sending AI prompt via command mode"
                    );
                    if tx.try_send(AiCommand::Prompt(prompt.to_string())).is_err() {
                        warn!("AI command channel full or closed — prompt dropped");
                    }
                    editor.set_status("[AI] Thinking...");
                } else {
                    warn!("AI prompt submitted but no AI provider configured");
                    editor
                        .set_status("AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
                }
                return;
            }

            // :eval <scheme> — Scheme REPL
            if let Some(code) = cmd.strip_prefix("eval ") {
                let code = code.trim();
                if code.is_empty() {
                    editor.set_status("eval: no expression given");
                    return;
                }
                debug!(code, "evaluating scheme expression");
                scheme.inject_editor_state(editor);
                match scheme.eval(code) {
                    Ok(result) => {
                        scheme.apply_to_editor(editor);
                        debug!(result = %result, "scheme eval succeeded");
                        if result.is_empty() {
                            editor.set_status("(ok)");
                        } else {
                            editor.set_status(result);
                        }
                    }
                    Err(e) => {
                        error!(code, error = %e, "scheme eval failed");
                        editor.set_status(format!("Scheme error: {}", e));
                    }
                }
                return;
            }

            // Registered command name (e.g., :move-down, :count-lines)
            let cmd_name = cmd.split_whitespace().next().unwrap_or("");
            if editor.commands.contains(cmd_name) {
                dispatch_command(editor, scheme, cmd_name);
            } else {
                // Fall back to ex commands (:w, :q, :q!, :wq, :e path)
                editor.execute_command(&cmd);
            }
        }
        KeyCode::Tab => {
            // Tab completion for :e <path>
            if editor.command_line.starts_with("e ") {
                let path_part = &editor.command_line[2..];
                if editor.tab_completions.is_empty() {
                    editor.tab_completions = mae_core::file_picker::complete_path(path_part);
                    editor.tab_completion_idx = 0;
                } else {
                    editor.tab_completion_idx =
                        (editor.tab_completion_idx + 1) % editor.tab_completions.len();
                }
                if !editor.tab_completions.is_empty() {
                    let completion = editor.tab_completions[editor.tab_completion_idx].clone();
                    editor.command_line = format!("e {}", completion);
                }
            }
        }
        KeyCode::Up => {
            editor.command_history_prev();
        }
        KeyCode::Down => {
            editor.command_history_next();
        }
        KeyCode::Backspace => {
            if editor.command_line.is_empty() {
                editor.mode = Mode::Normal;
            } else {
                editor.command_line.pop();
                editor.tab_completions.clear();
            }
        }
        KeyCode::Char(ch) => {
            editor.command_line.push(ch);
            editor.tab_completions.clear();
        }
        _ => {}
    }
}
