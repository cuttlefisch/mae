use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(feature = "gui")]
use crossterm::event::{KeyEventKind, KeyEventState};
use mae_ai::AiCommand;
use mae_core::{
    BufferKind, CommandSource, Editor, Key, KeyPress, LookupResult, Mode, PalettePurpose,
};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, info, warn};

use crate::bootstrap::load_ai_config;

/// Convert a mae_core `KeyPress` into a synthetic crossterm `KeyEvent`.
///
/// Used by the GUI backend so it can reuse the existing `handle_key` logic
/// without duplicating every mode handler. The crossterm event is synthetic
/// (no real terminal event) but has the correct `KeyCode` + modifiers.
#[cfg(feature = "gui")]
pub fn keypress_to_crossterm(kp: &KeyPress) -> KeyEvent {
    let code = match kp.key {
        Key::Char(ch) => KeyCode::Char(ch),
        Key::Escape => KeyCode::Esc,
        Key::Enter => KeyCode::Enter,
        Key::Backspace => KeyCode::Backspace,
        Key::Tab => KeyCode::Tab,
        Key::BackTab => KeyCode::BackTab,
        Key::Up => KeyCode::Up,
        Key::Down => KeyCode::Down,
        Key::Left => KeyCode::Left,
        Key::Right => KeyCode::Right,
        Key::Home => KeyCode::Home,
        Key::End => KeyCode::End,
        Key::PageUp => KeyCode::PageUp,
        Key::PageDown => KeyCode::PageDown,
        Key::Delete => KeyCode::Delete,
        Key::F(n) => KeyCode::F(n),
    };

    let mut modifiers = KeyModifiers::NONE;
    if kp.ctrl {
        modifiers |= KeyModifiers::CONTROL;
    }
    if kp.alt {
        modifiers |= KeyModifiers::ALT;
    }

    KeyEvent {
        code,
        modifiers,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

/// Handle a `KeyPress` from the GUI backend by converting to crossterm format.
///
/// This lets the GUI event loop share the full key dispatch pipeline with
/// the terminal backend without duplicating mode handlers.
#[cfg(feature = "gui")]
pub fn handle_key_from_keypress(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    kp: KeyPress,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    let key_event = keypress_to_crossterm(&kp);
    handle_key(editor, scheme, key_event, pending_keys, ai_tx);
}

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
        KeyCode::BackTab => Key::BackTab,
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

/// Check if the splash screen is currently visible.
fn is_splash_visible(editor: &Editor) -> bool {
    editor.active_buffer().kind == mae_core::BufferKind::Dashboard
}

pub fn handle_key(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    // Input lock is now checked at the event loop level (main.rs) so it
    // covers all modes including ShellInsert. By the time we get here,
    // input_lock is guaranteed None (or the mode is ShellInsert, which
    // is allowed through the lock).

    if editor.mode != Mode::Command {
        editor.status_msg.clear();
    }

    // --- Splash screen navigation intercept ---
    // When the splash is visible, j/k/Up/Down navigate, Enter selects,
    // and any other key dismisses the splash (by inserting into scratch).
    if editor.mode == Mode::Normal && is_splash_visible(editor) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let count = mae_renderer::splash_render::splash_action_count();
                if count > 0 {
                    editor.splash_selection = (editor.splash_selection + 1) % count;
                }
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let count = mae_renderer::splash_render::splash_action_count();
                if count > 0 {
                    editor.splash_selection = (editor.splash_selection + count - 1) % count;
                }
                return;
            }
            KeyCode::Enter => {
                let actions = mae_renderer::splash_render::QUICK_ACTIONS;
                if let Some(&(_, _, cmd)) = actions.get(editor.splash_selection) {
                    // Dismiss splash by inserting a space then clearing it,
                    // so the splash condition no longer holds.
                    editor.dispatch_builtin(cmd);
                }
                return;
            }
            _ => {
                // Any other key dismisses splash and falls through to normal handling.
            }
        }
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
            && editor.pending_char_command.is_none()
            && editor.pending_operator.is_none();
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
        Mode::FileBrowser => handle_file_browser_mode(editor, key),
        Mode::CommandPalette => handle_command_palette_mode(editor, scheme, key),
        Mode::ShellInsert => {} // Handled externally by main.rs (needs ShellTerminal access)
    }

    if editor.mode != mode_before {
        pending_keys.clear();
        editor.fire_hook("mode-change");
    }

    // --- Drain pending Scheme eval requests ---
    // Commands like `eval-line` / `eval-buffer` push code here;
    // the actual evaluation needs the SchemeRuntime which lives in
    // the binary, not in mae-core.
    if !editor.pending_scheme_eval.is_empty() {
        let exprs: Vec<String> = editor.pending_scheme_eval.drain(..).collect();
        for code in &exprs {
            let output = scheme.eval_for_repl(code, editor);
            // Short result → status bar; always append to *Scheme* buffer.
            let lines: Vec<&str> = output.lines().collect();
            if let Some(result_line) = lines.iter().find(|l| l.starts_with("; =>")) {
                editor.set_status(result_line.trim_start_matches("; => "));
            } else if let Some(err_line) = lines.iter().find(|l| l.starts_with("; error")) {
                editor.set_status(*err_line);
            }
            editor.append_to_scheme_repl(&output);
        }
    }

    // --- Drain pending hook evaluations ---
    // Hook points fire in core (save, open, close) and push (hook_name, fn_name)
    // entries. We eval each function here where the SchemeRuntime is available.
    drain_hook_evals(editor, scheme);
}

/// Evaluate all pending hook functions queued by `fire_hook`.
fn drain_hook_evals(editor: &mut Editor, scheme: &mut SchemeRuntime) {
    if editor.pending_hook_evals.is_empty() {
        return;
    }
    let hooks: Vec<(String, String)> = editor.pending_hook_evals.drain(..).collect();
    for (hook_name, fn_name) in hooks {
        scheme.inject_editor_state(editor);
        scheme.inject_value("*hook-name*", &hook_name);
        match scheme.call_function(&fn_name) {
            Ok(_) => scheme.apply_to_editor(editor),
            Err(e) => {
                warn!(hook = %hook_name, fn_name = %fn_name, error = %e, "hook error");
                editor.set_status(format!("Hook error ({}): {}", hook_name, e));
            }
        }
    }
}

/// Returns true if a command is a vim operator (d/c/y) that enters pending state.
fn is_operator_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "operator-delete" | "operator-change" | "operator-yank" | "operator-surround"
    )
}

/// Dispatch a command by name, handling both builtins and Scheme commands.
fn dispatch_command(editor: &mut Editor, scheme: &mut SchemeRuntime, name: &str) {
    let theme_before = editor.theme.name.clone();
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

    // Persist theme change regardless of source (cycle-theme, set-theme, scheme).
    if editor.theme.name != theme_before {
        crate::config::persist_editor_preference("theme", &editor.theme.name);
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
                let creating = picker.query_selected && !path.exists();
                editor.file_picker = None;
                editor.mode = Mode::Normal;
                if creating {
                    // Create parent directories and an empty file, then open it.
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&path, "") {
                        editor.set_status(format!("Cannot create file: {}", e));
                    } else {
                        editor.open_file(&path);
                    }
                } else {
                    editor.open_file(&path);
                }
            } else {
                editor.file_picker = None;
                editor.mode = Mode::Normal;
                editor.set_status("No file selected");
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            picker.move_up();
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_up();
        }
        KeyCode::Down => {
            picker.move_down();
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            picker.move_down();
        }
        KeyCode::Tab => {
            // Try path completion for absolute/home paths first, then
            // Doom-style longest-common-prefix within the current root,
            // then fall back to cycling selection.
            // Both methods have side effects — can't collapse into a match guard.
            let completed = picker.complete_path_tab() || picker.complete_longest_prefix();
            if !completed {
                picker.move_down();
            }
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
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl-U: clear the query line (Emacs/readline style).
            picker.clear_query();
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Ctrl-W: delete last path component or word.
            let q = &picker.query;
            let trimmed = q.trim_end_matches('/');
            let new_end = trimmed.rfind('/').map(|i| i + 1).unwrap_or(0);
            let new_query = picker.query[..new_end].to_string();
            picker.query = new_query;
            picker.update_filter();
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
            // If the query now looks like `~/dir/` or `/abs/path/`,
            // switch the picker root to that directory.
            if ch == '/' && picker.maybe_switch_root() {
                // Root switched — filter already reset by rescan.
            } else {
                picker.update_filter();
            }
        }
        _ => {}
    }
}

/// Key handling for the ranger-style `FileBrowser` overlay.
///
/// Motion keys mirror vim where it makes sense (`j`/`k`, `h`/`l`), with
/// Enter activating the selection (descend or open). A typed query
/// narrows the current directory listing; descending clears it.
///
/// Exit via Esc / `q` / Ctrl-C.
fn handle_file_browser_mode(editor: &mut Editor, key: KeyEvent) {
    use mae_core::file_browser::Activation;

    let browser = match editor.file_browser.as_mut() {
        Some(b) => b,
        None => {
            editor.mode = Mode::Normal;
            return;
        }
    };

    // Ctrl- bindings first so they can't be shadowed by plain-char handling.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('c') => {
                editor.file_browser = None;
                editor.mode = Mode::Normal;
                return;
            }
            KeyCode::Char('j') => {
                browser.move_down();
                return;
            }
            KeyCode::Char('k') => {
                browser.move_up();
                return;
            }
            KeyCode::Char('u') => {
                browser.query.clear();
                browser.update_filter();
                return;
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Esc => {
            editor.file_browser = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Char('q') if browser.query.is_empty() => {
            editor.file_browser = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Enter | KeyCode::Char('l') if browser.query.is_empty() => {
            if let Activation::OpenFile(path) = browser.activate() {
                editor.file_browser = None;
                editor.mode = Mode::Normal;
                editor.open_file(&path);
            }
            // Descended / Nothing: stay in browser mode with refreshed listing.
        }
        // Enter while a query is active: check for path navigation first,
        // then activate the selected entry.
        KeyCode::Enter => {
            // If query looks like an absolute or home-relative path to a
            // directory, navigate there directly.
            let nav_path = if browser.query.starts_with('/') {
                Some(std::path::PathBuf::from(&browser.query))
            } else if browser.query.starts_with("~/") {
                let expanded = mae_core::file_picker::expand_tilde(&browser.query);
                Some(std::path::PathBuf::from(expanded))
            } else {
                None
            };
            if let Some(p) = nav_path {
                if p.is_dir() {
                    browser.cwd = p;
                    browser.refresh();
                } else if let Activation::OpenFile(path) = browser.activate() {
                    editor.file_browser = None;
                    editor.mode = Mode::Normal;
                    editor.open_file(&path);
                }
            } else if let Activation::OpenFile(path) = browser.activate() {
                editor.file_browser = None;
                editor.mode = Mode::Normal;
                editor.open_file(&path);
            }
        }
        KeyCode::Tab => {
            browser.complete_tab();
        }
        KeyCode::Up => browser.move_up(),
        KeyCode::Down => browser.move_down(),
        KeyCode::Char('k') if browser.query.is_empty() => browser.move_up(),
        KeyCode::Char('j') if browser.query.is_empty() => browser.move_down(),
        KeyCode::Char('h') if browser.query.is_empty() => browser.ascend(),
        KeyCode::Backspace => {
            if browser.query.is_empty() {
                // Empty query → Backspace means "go up one directory".
                browser.ascend();
            } else {
                browser.query.pop();
                browser.update_filter();
            }
        }
        KeyCode::Char(ch) => {
            browser.query.push(ch);
            browser.update_filter();
        }
        _ => {}
    }
}

fn handle_command_palette_mode(editor: &mut Editor, scheme: &mut SchemeRuntime, key: KeyEvent) {
    // Pull the selected command name out *before* doing anything that
    // might need a mutable borrow on `editor` (like closing the palette
    // and dispatching). This avoids borrow-checker friction.
    let palette = match editor.command_palette.as_mut() {
        Some(p) => p,
        None => {
            editor.mode = Mode::Normal;
            return;
        }
    };

    match key.code {
        KeyCode::Esc => {
            editor.command_palette = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            let name = palette.selected_name().map(|s| s.to_string());
            let purpose = palette.purpose;
            let query = palette.query.clone();
            editor.command_palette = None;
            editor.mode = Mode::Normal;
            match (name, purpose) {
                (Some(cmd), PalettePurpose::Execute) => dispatch_command(editor, scheme, &cmd),
                (Some(cmd), PalettePurpose::Describe) => {
                    editor.open_help_at(&format!("cmd:{}", cmd))
                }
                (Some(theme), PalettePurpose::SetTheme) => {
                    editor.set_theme_by_name(&theme);
                    crate::config::persist_editor_preference("theme", &theme);
                }
                (Some(node_id), PalettePurpose::HelpSearch) => {
                    editor.open_help_at(&node_id);
                }
                (Some(buf_name), PalettePurpose::SwitchBuffer) => {
                    if let Some(idx) = editor.buffers.iter().position(|b| b.name == buf_name) {
                        editor.switch_to_buffer(idx);
                        editor.sync_mode_to_buffer();
                    }
                }
                (Some(path), PalettePurpose::RecentFile) => {
                    editor.open_file(&path);
                }
                (Some(art), PalettePurpose::SetSplashArt) => {
                    editor.splash_art = Some(art.clone());
                    editor.set_status(format!("Splash art set to: {}", art));
                    crate::config::persist_editor_preference("splash_art", &art);
                }
                (Some(root_str), PalettePurpose::SwitchProject) => {
                    editor.add_project(&root_str);
                }
                (None, PalettePurpose::SwitchProject) => {
                    // No match selected — treat query as a typed path
                    if !query.is_empty() {
                        editor.add_project(&query);
                    } else {
                        editor.set_status("No project selected");
                    }
                }
                (None, _) => editor.set_status("No command selected"),
            }
        }
        KeyCode::Up | KeyCode::BackTab => {
            palette.move_up();
        }
        KeyCode::Down | KeyCode::Tab => {
            palette.move_down();
        }
        KeyCode::Backspace => {
            if palette.query.is_empty() {
                editor.command_palette = None;
                editor.mode = Mode::Normal;
            } else {
                palette.query.pop();
                palette.update_filter();
            }
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            palette.move_up();
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            palette.move_down();
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.command_palette = None;
            editor.mode = Mode::Normal;
        }
        KeyCode::Char(ch) => {
            palette.query.push(ch);
            palette.update_filter();
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
        Mode::Command
        | Mode::ConversationInput
        | Mode::Search
        | Mode::FilePicker
        | Mode::FileBrowser
        | Mode::CommandPalette => "command",
        Mode::ShellInsert => return, // Handled by main.rs handle_shell_key
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
            let had_pending_op = editor.pending_operator.is_some();
            // Multiply operator count with motion count (e.g. 2d3j → 6j)
            if had_pending_op && Editor::is_motion_command(&cmd) {
                if let Some(op_count) = editor.operator_count.take() {
                    let motion_count = editor.count_prefix.unwrap_or(1);
                    editor.count_prefix = Some(op_count * motion_count);
                }
            }
            dispatch_command(editor, scheme, &cmd);
            // After a motion completes with a pending operator, apply the operator
            if had_pending_op && Editor::is_motion_command(&cmd) {
                editor.apply_pending_operator_for_motion(&cmd);
            }
        }
        LookupResult::Prefix => {
            editor.which_key_prefix = pending_keys.clone();
        }
        LookupResult::None => {
            // Operator fallback: try splitting the sequence at each position
            // to find the longest prefix that is an operator command.
            // E.g. `dgg` → split at 1: `d` (operator-delete) + `gg`
            //       `ysw` → split at 2: `ys` (operator-surround) + `w`
            // Longest match wins (try from len-1 down to 1).
            let mut split_at = 0;
            let mut split_cmd = String::new();
            if pending_keys.len() > 1 {
                for i in (1..pending_keys.len()).rev() {
                    if let Some(cmd) = editor
                        .keymaps
                        .get(mode_name)
                        .and_then(|km| km.exact_match(&pending_keys[..i]))
                    {
                        if is_operator_command(cmd) {
                            split_at = i;
                            split_cmd = cmd.to_string();
                            break;
                        }
                    }
                }
            }

            if split_at > 0 {
                let remaining: Vec<KeyPress> = pending_keys[split_at..].to_vec();
                pending_keys.clear();
                editor.which_key_prefix.clear();
                dispatch_command(editor, scheme, &split_cmd);

                // Extract leading digits from remaining keys as count_prefix.
                // This handles sequences like `d3k` where `3` follows the
                // operator and should be consumed as a motion count, not
                // looked up in the keymap.
                let mut digit_end = 0;
                for kp in &remaining {
                    if let mae_core::keymap::Key::Char(ch) = kp.key {
                        if ch.is_ascii_digit() && (ch != '0' || digit_end > 0) {
                            digit_end += 1;
                            continue;
                        }
                    }
                    break;
                }
                if digit_end > 0 {
                    let mut count = 0usize;
                    for kp in &remaining[..digit_end] {
                        if let mae_core::keymap::Key::Char(ch) = kp.key {
                            count = count * 10 + (ch as usize - '0' as usize);
                        }
                    }
                    editor.count_prefix = Some(count.clamp(1, 99999));
                }

                // Re-lookup the remaining keys (after digits) as a new sequence.
                *pending_keys = remaining[digit_end..].to_vec();

                // If all remaining keys were digits, we're waiting for the
                // motion keystroke — operator is pending, count is set.
                if pending_keys.is_empty() {
                    // Nothing more to look up; next keypress will complete.
                    return;
                }

                let result2 = editor
                    .keymaps
                    .get(mode_name)
                    .map(|km| km.lookup(pending_keys))
                    .unwrap_or(LookupResult::None);
                match result2 {
                    LookupResult::Exact(cmd) => {
                        let cmd = cmd.to_string();
                        let had_pending = editor.pending_operator.is_some();
                        // Multiply operator count with motion count
                        if had_pending && Editor::is_motion_command(&cmd) {
                            if let Some(op_count) = editor.operator_count.take() {
                                let motion_count = editor.count_prefix.unwrap_or(1);
                                editor.count_prefix = Some(op_count * motion_count);
                            }
                        }
                        pending_keys.clear();
                        editor.which_key_prefix.clear();
                        dispatch_command(editor, scheme, &cmd);
                        if had_pending && Editor::is_motion_command(&cmd) {
                            editor.apply_pending_operator_for_motion(&cmd);
                        }
                    }
                    LookupResult::Prefix => {
                        // Remaining keys are a prefix (e.g., `g` of `gg`).
                        // Keep them in pending_keys; next keystroke will complete.
                        editor.which_key_prefix = pending_keys.clone();
                    }
                    LookupResult::None => {
                        // Remaining keys also don't match — give up.
                        pending_keys.clear();
                        editor.which_key_prefix.clear();
                        editor.pending_operator = None;
                        editor.operator_start = None;
                        editor.operator_count = None;
                        editor.set_status("Key not bound");
                    }
                }
            } else {
                pending_keys.clear();
                if !editor.which_key_prefix.is_empty() {
                    editor.set_status("Key not bound");
                }
                editor.which_key_prefix.clear();
            }
        }
    }
}

/// Resolve one key sequence while `SPC h k` (describe-key) is armed.
///
/// Accumulates into `pending_keys`, consults the normal keymap, and on
/// `Exact` opens the bound command's help page instead of dispatching
/// it. `Prefix` keeps collecting; `None` reports "not bound". Escape
/// cancels.
fn handle_describe_key_await(editor: &mut Editor, key: KeyEvent, pending_keys: &mut Vec<KeyPress>) {
    // Ctrl-C is always a hard exit.
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        editor.awaiting_key_description = false;
        pending_keys.clear();
        editor.which_key_prefix.clear();
        editor.running = false;
        return;
    }
    if key.code == KeyCode::Esc {
        editor.awaiting_key_description = false;
        pending_keys.clear();
        editor.which_key_prefix.clear();
        editor.set_status("describe-key cancelled");
        return;
    }

    let Some(kp) = crossterm_to_keypress(&key) else {
        return;
    };
    pending_keys.push(kp);

    let result = editor
        .keymaps
        .get("normal")
        .map(|km| km.lookup(pending_keys))
        .unwrap_or(LookupResult::None);

    match result {
        LookupResult::Exact(cmd) => {
            let cmd = cmd.to_string();
            editor.awaiting_key_description = false;
            pending_keys.clear();
            editor.which_key_prefix.clear();
            let id = format!("cmd:{}", cmd);
            if editor.kb.contains(&id) {
                editor.open_help_at(&id);
            } else {
                // Command is bound but has no KB node (rare — all
                // registered commands are seeded). Still useful to tell
                // the user what it resolves to.
                editor.set_status(format!("Key bound to: {} (no help page)", cmd));
            }
        }
        LookupResult::Prefix => {
            editor.which_key_prefix = pending_keys.clone();
        }
        LookupResult::None => {
            editor.awaiting_key_description = false;
            pending_keys.clear();
            editor.which_key_prefix.clear();
            editor.set_status("Key not bound");
        }
    }
}

fn handle_normal_mode(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
    // If we're resolving `SPC h k`, every subsequent keypress feeds
    // into normal-keymap lookup until we get Exact/None. Bypass count
    // prefix, char-await, and help-buffer shortcuts — this interaction
    // is strictly "what command does this key sequence run?"
    if editor.awaiting_key_description {
        handle_describe_key_await(editor, key, pending_keys);
        return;
    }

    // `"<char>` — register prompt. Capture the next char into
    // active_register; Escape cancels. See register_ops.rs for the
    // semantics of each register letter.
    if editor.pending_register_prompt {
        editor.pending_register_prompt = false;
        if let KeyCode::Char(ch) = key.code {
            editor.active_register = Some(ch);
            editor.set_status(format!("\"{}", ch));
        } else {
            editor.set_status("");
        }
        return;
    }

    // If a char-argument command is pending (f/F/t/T or text objects), capture the next char
    if let Some(cmd) = editor.pending_char_command.take() {
        if let KeyCode::Char(ch) = key.code {
            let had_pending_op = editor.pending_operator.is_some();
            // Try text object dispatch first, then fall back to char motion
            if editor.dispatch_text_object(&cmd, ch) || editor.dispatch_surround(&cmd, ch) {
                // Text object/surround handled it directly — clear dangling state
                editor.pending_operator = None;
                editor.operator_start = None;
                editor.operator_count = None;
            } else {
                editor.dispatch_char_motion(&cmd, ch);
                // f/t motions with a pending operator
                if had_pending_op {
                    editor.last_motion_linewise = false;
                    editor.apply_pending_operator();
                }
            }
        } else {
            // Escape or non-char clears pending operator too
            editor.pending_operator = None;
            editor.operator_start = None;
            editor.operator_count = None;
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

    // Escape dismisses which-key popup if active, clears count prefix and pending operator
    if key.code == KeyCode::Esc {
        editor.count_prefix = None;
        editor.pending_operator = None;
        editor.operator_start = None;
        editor.operator_count = None;
        if !editor.which_key_prefix.is_empty() {
            pending_keys.clear();
            editor.which_key_prefix.clear();
            return;
        }
    }

    // Help buffer: intercept only link-navigation and help-specific keys.
    // All normal vim navigation (j/k/G/gg/C-d/C-u/etc.) falls through to
    // the standard keymap — the help buffer is a read-only rope buffer.
    let is_help = {
        let idx = editor.active_buffer_idx();
        editor.buffers[idx].kind == BufferKind::Help
    };
    if is_help && pending_keys.is_empty() {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => {
                editor.help_follow_link();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Tab => {
                editor.help_next_link();
                editor.count_prefix = None;
                return;
            }
            KeyCode::BackTab => {
                editor.help_prev_link();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('q') if !ctrl => {
                editor.help_close();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('o') if ctrl => {
                editor.help_back();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('i') if ctrl => {
                editor.help_forward();
                editor.count_prefix = None;
                return;
            }
            _ => {} // Fall through to normal keymap
        }
    }

    // Debug panel: intercept navigation and action keys.
    // j/k move between interactive items, Enter selects/expands,
    // c/n/s/S drive execution, o toggles output, r refreshes, q closes.
    let is_debug = {
        let idx = editor.active_buffer_idx();
        editor.buffers[idx].kind == BufferKind::Debug
    };
    if is_debug && pending_keys.is_empty() {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('j') if !ctrl => {
                let idx = editor.active_buffer_idx();
                if let Some(view) = editor.buffers[idx].debug_view.as_mut() {
                    view.move_down();
                }
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('k') if !ctrl => {
                let idx = editor.active_buffer_idx();
                if let Some(view) = editor.buffers[idx].debug_view.as_mut() {
                    view.move_up();
                }
                editor.count_prefix = None;
                return;
            }
            KeyCode::Enter => {
                editor.debug_panel_select();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('q') if !ctrl => {
                editor.close_debug_panel();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('o') if !ctrl => {
                editor.debug_toggle_output();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('r') if !ctrl => {
                editor.dap_refresh();
                editor.debug_panel_refresh_if_open();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('c') if !ctrl => {
                editor.dap_continue();
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('n') if !ctrl => {
                editor.dap_step(mae_core::StepKind::Over);
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('s') if !ctrl => {
                editor.dap_step(mae_core::StepKind::In);
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('S') if !ctrl => {
                editor.dap_step(mae_core::StepKind::Out);
                editor.count_prefix = None;
                return;
            }
            _ => {} // Fall through to normal keymap
        }
    }

    // In Normal mode, intercept j/k/G/gg for conversation buffer scrolling
    // and `i` to re-enter ConversationInput mode.
    let is_conv = {
        let idx = editor.active_buffer_idx();
        editor.buffers[idx].conversation.is_some()
    };
    if is_conv && pending_keys.is_empty() {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let count = editor.count_prefix.unwrap_or(1).max(1);
        match key.code {
            KeyCode::Char('j') if !ctrl => {
                let idx = editor.active_buffer_idx();
                if let Some(ref mut conv) = editor.buffers[idx].conversation {
                    conv.scroll_down(count);
                }
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('k') if !ctrl => {
                let idx = editor.active_buffer_idx();
                if let Some(ref mut conv) = editor.buffers[idx].conversation {
                    conv.scroll_up(count);
                }
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('G') if !ctrl => {
                let idx = editor.active_buffer_idx();
                if let Some(ref mut conv) = editor.buffers[idx].conversation {
                    conv.scroll_to_bottom();
                }
                editor.count_prefix = None;
                return;
            }
            KeyCode::Char('i') | KeyCode::Char('a') if !ctrl => {
                editor.mode = Mode::ConversationInput;
                editor.count_prefix = None;
                return;
            }
            _ => {}
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
    // Register prompt (`"<char>` in visual mode — same semantics as Normal).
    if editor.pending_register_prompt {
        editor.pending_register_prompt = false;
        if let KeyCode::Char(ch) = key.code {
            editor.active_register = Some(ch);
            editor.set_status(format!("\"{}", ch));
        } else {
            editor.set_status("");
        }
        return;
    }

    // Handle pending char-argument commands (f/F/t/T or text objects)
    if let Some(cmd) = editor.pending_char_command.take() {
        if let KeyCode::Char(ch) = key.code {
            let had_pending_op = editor.pending_operator.is_some();
            if editor.dispatch_text_object(&cmd, ch) || editor.dispatch_surround(&cmd, ch) {
                // Text object/surround handled it directly — clear dangling state
                editor.pending_operator = None;
                editor.operator_start = None;
                editor.operator_count = None;
            } else {
                editor.dispatch_char_motion(&cmd, ch);
                if had_pending_op {
                    editor.last_motion_linewise = false;
                    editor.apply_pending_operator();
                }
            }
        } else {
            editor.pending_operator = None;
            editor.operator_start = None;
            editor.operator_count = None;
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
    // Ctrl-R <reg> — insert the named register's contents at the cursor.
    // The next char is captured here (after Ctrl-R has already fired).
    // Escape cancels.
    if editor.pending_insert_register {
        editor.pending_insert_register = false;
        if let KeyCode::Char(ch) = key.code {
            editor.insert_from_register(ch);
        }
        return;
    }

    // If the completion popup is visible, Tab/Ctrl-n/Ctrl-p navigate it.
    // When the popup is not visible, Tab falls through to keymap (which will
    // find no binding and do nothing, which is acceptable for now).
    let popup_open = !editor.completion_items.is_empty();

    // Ctrl-R: arm the register-prompt state. Handled before the char
    // dispatch below because `Ctrl-R` without popup would otherwise hit
    // the generic `Char('r')` insertion path.
    if let KeyCode::Char('r') = key.code {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            editor.pending_insert_register = true;
            return;
        }
    }

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
        // C-j / Enter — newline
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, '\n');
            editor.lsp_dismiss_completion();
            return;
        }
        KeyCode::Enter => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].insert_char(win, '\n');
            editor.lsp_dismiss_completion();
            return;
        }
        // C-h / Backspace — delete backward
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_backward(win);
            editor.lsp_request_completion();
            return;
        }
        KeyCode::Backspace => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_backward(win);
            editor.lsp_request_completion();
            return;
        }
        // C-a: go to beginning of line
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let win = editor.window_mgr.focused_window_mut();
            win.move_to_line_start();
            return;
        }
        // C-e: go to end of line
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            win.move_to_line_end(&editor.buffers[idx]);
            return;
        }
        // C-w: delete word backward (bash-style: back to whitespace)
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_word_backward(win);
            editor.lsp_dismiss_completion();
            return;
        }
        // C-u: delete to beginning of line
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_to_line_start(win);
            editor.lsp_dismiss_completion();
            return;
        }
        // C-k: delete to end of line (kill-line)
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_to_line_end(win);
            editor.lsp_dismiss_completion();
            return;
        }
        // C-d: delete char forward
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let idx = editor.active_buffer_idx();
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[idx].delete_char_forward(win);
            editor.lsp_dismiss_completion();
            return;
        }
        // C-o: execute one normal-mode command, then return to insert
        // (defer — requires saving/restoring insert mode; handled via keymap fallthrough for now)
        _ => {}
    }
    handle_keymap_mode(editor, scheme, key, pending_keys);
}

fn conv_submit(editor: &mut Editor, ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>) {
    let buf_idx = editor.active_buffer_idx();

    // Reject submissions while the previous turn is still streaming.
    // Otherwise a user who types faster than the provider responds ends
    // up with a visibly "off by one" transcript: multiple [You] blocks
    // appear before any [AI] reply, and the replies land interleaved
    // with the next batch of prompts. This guard keeps the conversation
    // strictly turn-by-turn so prompts stay aligned with their answers.
    let (already_streaming, has_input) = match editor.buffers[buf_idx].conversation.as_ref() {
        Some(conv) => (conv.streaming, !conv.input_line.is_empty()),
        None => (false, false),
    };
    if !has_input {
        editor.mode = Mode::Normal;
        return;
    }
    if already_streaming {
        editor.set_status("[AI] still responding — wait for the reply or press SPC a a to cancel");
        return;
    }

    let input = editor.buffers[buf_idx]
        .conversation
        .as_mut()
        .map(|conv| {
            let input = conv.input_line.clone();
            conv.push_user(&input);
            conv.input_line.clear();
            conv.input_cursor = 0;
            conv.scroll_to_bottom();
            conv.streaming = true;
            conv.streaming_start = Some(std::time::Instant::now());
            input
        })
        .unwrap_or_default();

    if let Some(tx) = ai_tx {
        if tx.try_send(AiCommand::Prompt(input)).is_err() {
            warn!("AI command channel full or closed — prompt dropped");
        }
        editor.set_status("[AI] Thinking...");
    } else {
        warn!("AI prompt submitted but no AI provider configured");
        editor.set_status("AI not configured. Set ANTHROPIC_API_KEY or OPENAI_API_KEY.");
        if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
            conv.end_streaming();
        }
    }
    editor.mode = Mode::Normal;
}

fn handle_conversation_input(
    editor: &mut Editor,
    key: KeyEvent,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        // --- Mode transitions ---
        KeyCode::Esc => {
            editor.mode = Mode::Normal;
        }
        KeyCode::Enter => {
            conv_submit(editor, ai_tx);
        }

        // --- Cancel / quit ---
        KeyCode::Char('c') if ctrl => {
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

        // --- Cursor movement ---
        KeyCode::Char('a') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_home();
            }
        }
        KeyCode::Char('e') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_end();
            }
        }
        KeyCode::Home => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_home();
            }
        }
        KeyCode::End => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_end();
            }
        }
        KeyCode::Char('b') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_backward();
            }
        }
        KeyCode::Char('f') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_forward();
            }
        }
        KeyCode::Left => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_backward();
            }
        }
        KeyCode::Right => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_move_forward();
            }
        }

        // --- Deletion ---
        KeyCode::Backspace => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_backspace();
            }
        }
        KeyCode::Char('h') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_backspace();
            }
        }
        KeyCode::Delete => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_delete_forward();
            }
        }
        KeyCode::Char('d') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_delete_forward();
            }
        }
        KeyCode::Char('w') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_kill_word_backward();
            }
        }
        KeyCode::Char('u') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_kill_to_start();
            }
        }
        KeyCode::Char('k') if ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_kill_to_end();
            }
        }

        // --- Scroll history (stay in input mode) ---
        KeyCode::PageUp => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.scroll_up(10);
            }
        }
        KeyCode::PageDown => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.scroll_down(10);
            }
        }

        // --- Regular character insertion ---
        KeyCode::Char(ch) if !ctrl => {
            let buf_idx = editor.active_buffer_idx();
            if let Some(ref mut conv) = editor.buffers[buf_idx].conversation {
                conv.input_insert_char(ch);
                // Scroll to bottom when typing so the user sees the prompt.
                conv.scroll_to_bottom();
            }
        }

        _ => {}
    }
}

/// Apply the currently selected tab completion to the command line.
fn apply_tab_completion(editor: &mut Editor) {
    if editor.tab_completions.is_empty() {
        return;
    }
    let completion = editor.tab_completions[editor.tab_completion_idx].clone();
    if let Some(space_pos) = editor.command_line.find(' ') {
        let prefix = editor.command_line[..=space_pos].to_string();
        editor.command_line = format!("{}{}", prefix, completion);
    } else {
        editor.command_line = completion;
    }
    editor.command_cursor = editor.command_line.len();
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
            editor.command_cursor = 0;
        }
        KeyCode::Enter => {
            let cmd = editor.command_line.clone();
            editor.mode = Mode::Normal;
            editor.command_line.clear();
            editor.command_cursor = 0;

            // Record in command history before executing
            editor.push_command_history(&cmd);

            // :ai-status — show AI configuration
            if cmd == "ai-status" {
                let config = load_ai_config(editor);
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

            // :self-test [categories] — AI-driven e2e validation
            if cmd == "self-test" || cmd.starts_with("self-test ") {
                let categories = cmd.strip_prefix("self-test").unwrap().trim();
                if let Some(tx) = ai_tx {
                    // Lock input so user keystrokes don't interfere with test state.
                    editor.input_lock = mae_core::InputLock::AiBusy;
                    // Ensure *AI* buffer exists and is visible so the user
                    // can watch self-test progress (tool calls, results, report).
                    editor.open_conversation_buffer();
                    let prompt = build_self_test_prompt(categories);
                    if tx.try_send(AiCommand::Prompt(prompt)).is_err() {
                        warn!("AI self-test prompt dropped");
                        editor.input_lock = mae_core::InputLock::None;
                    }
                    info!(
                        "self-test started, categories={:?}",
                        if categories.is_empty() {
                            "all"
                        } else {
                            categories
                        }
                    );
                    editor.set_status("[AI BUSY — Esc to cancel] Running self-test...");
                } else {
                    editor.set_status("AI not configured — cannot run self-test");
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

            // Try ex-command handler first (handles args like `:theme dracula`,
            // `:e file.txt`, `:help topic`, etc.), then fall back to registered
            // command dispatch for bare names like `:move-down`.
            if !editor.execute_command(&cmd) {
                let cmd_name = cmd.split_whitespace().next().unwrap_or("");
                if editor.commands.contains(cmd_name) {
                    dispatch_command(editor, scheme, cmd_name);
                } else {
                    editor.set_status(format!("Unknown command: {}", cmd));
                }
            }
        }
        KeyCode::Tab => {
            if editor.tab_completions.is_empty() {
                editor.tab_completions = editor.cmdline_completions();
                editor.tab_completion_idx = 0;
            } else {
                editor.tab_completion_idx =
                    (editor.tab_completion_idx + 1) % editor.tab_completions.len();
            }
            apply_tab_completion(editor);
        }
        KeyCode::BackTab => {
            if editor.tab_completions.is_empty() {
                editor.tab_completions = editor.cmdline_completions();
                if !editor.tab_completions.is_empty() {
                    editor.tab_completion_idx = editor.tab_completions.len() - 1;
                }
            } else {
                let len = editor.tab_completions.len();
                editor.tab_completion_idx = (editor.tab_completion_idx + len - 1) % len;
            }
            apply_tab_completion(editor);
        }
        KeyCode::Up => {
            editor.command_history_prev();
        }
        KeyCode::Down => {
            editor.command_history_next();
        }
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.command_history_prev();
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.command_history_next();
        }
        KeyCode::Left => {
            editor.cmdline_move_backward();
        }
        KeyCode::Right => {
            editor.cmdline_move_forward();
        }
        KeyCode::Home => {
            editor.cmdline_move_home();
        }
        KeyCode::End => {
            editor.cmdline_move_end();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_home();
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_end();
        }
        KeyCode::Char('b') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_backward();
        }
        KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_move_forward();
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_delete_word_backward();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_kill_to_start();
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_kill_to_end();
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if editor.command_line.is_empty() {
                // C-d on empty line = abort (like in shells)
                editor.mode = Mode::Normal;
            } else {
                editor.cmdline_delete_forward();
            }
        }
        KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if editor.command_line.is_empty() {
                editor.mode = Mode::Normal;
            } else {
                editor.cmdline_backspace();
            }
        }
        KeyCode::Backspace => {
            if editor.command_line.is_empty() {
                editor.mode = Mode::Normal;
            } else {
                editor.cmdline_backspace();
            }
        }
        KeyCode::Char(ch) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.cmdline_insert_char(ch);
        }
        _ => {}
    }
}

/// Build the self-test prompt from the embedded template.
///
/// If `categories` is empty, all test categories run. Otherwise only the
/// named categories execute and everything else is reported as SKIP.
pub fn build_self_test_prompt(categories: &str) -> String {
    let base = include_str!("self_test_prompt.md");
    if categories.is_empty() {
        format!(
            "You are running MAE's self-test suite. Execute ALL test categories.\n\n{}",
            base
        )
    } else {
        format!(
            "You are running MAE's self-test suite. Execute ONLY these categories: {}. \
             Report all others as SKIP.\n\n{}",
            categories, base
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_self_test_prompt_all_categories() {
        let prompt = build_self_test_prompt("");
        assert!(!prompt.is_empty());
        assert!(prompt.contains("Execute ALL test categories"));
        assert!(prompt.contains("self_test_suite"));
    }

    #[test]
    fn build_self_test_prompt_filtered() {
        let prompt = build_self_test_prompt("editing");
        assert!(prompt.contains("Execute ONLY these categories: editing"));
        assert!(prompt.contains("Report all others as SKIP"));
    }

    #[test]
    fn build_self_test_prompt_multi_category() {
        let prompt = build_self_test_prompt("editing,help");
        assert!(prompt.contains("Execute ONLY these categories: editing,help"));
    }
}
