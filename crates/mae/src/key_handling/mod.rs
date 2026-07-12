use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(feature = "gui")]
use crossterm::event::{KeyEventKind, KeyEventState};
use mae_ai::AiCommand;
use mae_core::{CommandSource, Editor, Key, KeyPress, Mode};
use mae_scheme::SchemeRuntime;
use tracing::{debug, error, warn};

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
    if kp.shift {
        modifiers |= KeyModifiers::SHIFT;
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
    pending_interactive_event: &mut Option<PendingInteractiveEvent>,
) {
    let key_event = keypress_to_crossterm(&kp);
    handle_key(
        editor,
        scheme,
        key_event,
        pending_keys,
        ai_tx,
        pending_interactive_event,
    );
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

    // For character keys, shift is already encoded in the char itself ('G' vs 'g').
    // Normalize to false so keymap lookups match (parse_key_seq("G") stores shift=false).
    let shift = match mae_key {
        Key::Char(_) => false,
        _ => key.modifiers.contains(KeyModifiers::SHIFT),
    };
    Some(KeyPress {
        key: mae_key,
        ctrl,
        alt,
        shift,
    })
}

/// Check if the splash screen is currently visible.
pub(crate) fn is_splash_visible(editor: &Editor) -> bool {
    editor.active_buffer().kind == mae_core::BufferKind::Dashboard
}

use crate::ai_event_handler::PendingInteractiveEvent;

mod command;
mod command_palette;
pub(crate) mod conversation;
mod file_picker;
mod insert;
mod normal;
mod search;
#[cfg(test)]
mod tests;
mod visual;

pub use command::build_self_test_prompt;

pub fn handle_key(
    editor: &mut Editor,
    scheme: &mut SchemeRuntime,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
    ai_tx: &Option<tokio::sync::mpsc::Sender<AiCommand>>,
    pending_interactive_event: &mut Option<PendingInteractiveEvent>,
) {
    // A blocking/confirm mini-dialog (e.g. the host-key TOFU prompt, ADR-024 R4)
    // captures ALL input until answered — route to it before any mode, AI, or
    // conversation handling, in every mode (not just command-palette). Without
    // this, an async-raised modal (e.g. raised while the *AI* buffer is focused)
    // is unanswerable and keys leak to the underlying buffer (B-22).
    if editor.mini_dialog.is_some() {
        command_palette::handle_command_palette_mode(editor, scheme, key);
        return;
    }

    // Double Esc to cancel AI
    if key.code == KeyCode::Esc && editor.ai.streaming {
        let now = std::time::Instant::now();
        if let Some(last) = editor.ai.last_esc_time {
            if now.duration_since(last).as_millis() < 500 {
                editor.ai.cancel_requested = true;
                editor.set_status("AI interrupted (double-esc)");
                editor.ai.last_esc_time = None;
                return;
            }
        }
        editor.ai.last_esc_time = Some(now);
    } else if key.code != KeyCode::Esc {
        editor.ai.last_esc_time = None;
    }

    // Toggle collapse in conversation buffers (Normal mode)
    if editor.mode == Mode::Normal {
        let idx = editor.active_buffer_idx();
        if editor.buffers[idx].conversation().is_some()
            && (key.code == KeyCode::Enter || key.code == KeyCode::Tab)
        {
            let win = editor.window_mgr.focused_window();
            let row = win.cursor_row;
            if let Some(conv) = editor.buffers[idx].conversation_mut() {
                let lines = conv.rendered_lines();
                if let Some(line) = lines.get(row) {
                    if let Some(entry_idx) = line.entry_index {
                        conv.toggle_collapsed(entry_idx);
                        editor.sync_conversation_buffer_rope();
                        return;
                    }
                }
            }
        }
    }

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
    if editor.mode == Mode::Normal
        && !editor.leader_active
        && is_splash_visible(editor)
        && pending_keys.is_empty()
    {
        debug!(key_code = ?key.code, splash_selection = editor.splash_selection, "splash intercept");
        match key.code {
            // Vi (j/k) + arrows + CUA (C-n/C-p) all move the selection, so the
            // dashboard feels native in both the doom and non-modal flavors.
            KeyCode::Char('j') | KeyCode::Down => {
                let count = mae_core::render_common::splash::splash_action_count();
                if count > 0 {
                    editor.splash_selection = (editor.splash_selection + 1) % count;
                }
                return;
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let count = mae_core::render_common::splash::splash_action_count();
                if count > 0 {
                    editor.splash_selection = (editor.splash_selection + 1) % count;
                }
                return;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let count = mae_core::render_common::splash::splash_action_count();
                if count > 0 {
                    editor.splash_selection = (editor.splash_selection + count - 1) % count;
                }
                return;
            }
            KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let count = mae_core::render_common::splash::splash_action_count();
                if count > 0 {
                    editor.splash_selection = (editor.splash_selection + count - 1) % count;
                }
                return;
            }
            KeyCode::Enter => {
                let actions = mae_core::render_common::splash::QUICK_ACTIONS;
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

    // --- Which-key scroll intercept ---
    // When the which-key popup is visible, C-j/C-k/C-n/C-p scroll it.
    if editor.mode == Mode::Normal && !editor.which_key_prefix.is_empty() {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match (key.code, ctrl) {
            (KeyCode::Char('j'), true) | (KeyCode::Char('n'), true) | (KeyCode::Down, _) => {
                editor.which_key_scroll = editor.which_key_scroll.saturating_add(1);
                return;
            }
            (KeyCode::Char('k'), true) | (KeyCode::Char('p'), true) | (KeyCode::Up, _) => {
                editor.which_key_scroll = editor.which_key_scroll.saturating_sub(1);
                return;
            }
            (KeyCode::Char('d'), true) => {
                editor.which_key_scroll = editor.which_key_scroll.saturating_add(5);
                return;
            }
            (KeyCode::Char('u'), true) => {
                editor.which_key_scroll = editor.which_key_scroll.saturating_sub(5);
                return;
            }
            _ => {} // fall through to normal dispatch
        }
    }

    let mode_before = editor.mode;

    // --- Macro recording intercept ---
    // While recording, capture every keystroke into macro_log before dispatch.
    // Exception: a bare `q` in Normal mode with no pending prefix stops recording.
    if editor.vi.macro_recording {
        let is_stop_key = matches!(key.code, KeyCode::Char('q'))
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
            && editor.mode == Mode::Normal
            && pending_keys.is_empty()
            && editor.vi.pending_char_command.is_none()
            && editor.vi.pending_operator.is_none();
        if is_stop_key {
            editor.stop_recording();
            return;
        }
        if let Some(kp) = crossterm_to_keypress(&key) {
            editor.vi.macro_log.push(kp);
        }
    }

    // --- Normal-mode Enter-to-submit on conversation input buffer ---
    // handle_normal_mode doesn't have ai_tx, so we intercept here.
    if editor.mode == Mode::Normal && key.code == KeyCode::Enter {
        if let Some(ref pair) = editor.ai.conversation_pair.clone() {
            if editor.active_buffer_idx() == pair.input_buffer_idx {
                editor.set_mode(Mode::ConversationInput);
                conversation::submit_conversation_prompt(editor, ai_tx, pending_interactive_event);
                return;
            }
        }
    }

    // Transient keypad/leader layer overrides the mode handler: keys resolve
    // against the shared `leader` keymap (the mae which-key tree). Esc / C-g
    // cancel without executing; otherwise route through the keymap handler
    // (which sees `leader` via current_keymap_names and pops the layer after one
    // command). Lets `C-;` (non-modal/Insert) and `SPC` (doom/Normal) share one
    // leader tree without a dedicated mode. Falls through to the common tail
    // below (mode-change hook + pending Scheme-eval drain).
    if editor.leader_active {
        let is_cancel = key.code == KeyCode::Esc
            || (key.code == KeyCode::Char('g') && key.modifiers.contains(KeyModifiers::CONTROL));
        if is_cancel {
            editor.set_leader_active(false);
            pending_keys.clear();
            editor.clear_which_key_prefix();
            editor.set_status("");
            editor.fire_hook("leader-cancel");
        } else {
            normal::handle_keymap_mode(editor, scheme, key, pending_keys);
        }
    } else {
        match editor.mode {
            Mode::Normal => normal::handle_normal_mode(editor, scheme, key, pending_keys),
            Mode::Insert => insert::handle_insert_mode(editor, scheme, key, pending_keys),
            Mode::Visual(_) => visual::handle_visual_mode(editor, scheme, key, pending_keys),
            Mode::Command => {
                command::handle_command_mode(
                    editor,
                    scheme,
                    key,
                    pending_keys,
                    ai_tx,
                    pending_interactive_event,
                );
            }
            Mode::ConversationInput => {
                conversation::handle_conversation_input(
                    editor,
                    scheme,
                    key,
                    ai_tx,
                    pending_interactive_event,
                );
            }
            Mode::Search => search::handle_search_mode(editor, key),
            Mode::FilePicker => file_picker::handle_file_picker_mode(editor, key),
            Mode::FileBrowser => file_picker::handle_file_browser_mode(editor, key),
            Mode::CommandPalette => {
                command_palette::handle_command_palette_mode(editor, scheme, key)
            }
            // GitStatus buffers use Mode::Normal + buffer-kind overlay keymap
            Mode::ShellInsert => {} // Handled externally by main.rs (needs ShellTerminal access)
        }
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
        // Unified Scheme-eval path (core rule: the human, MCP clients, and the
        // AI peer all call the SAME primitives). Interactive eval (SPC e b /
        // eval-line / eval-region) routes through the exact same drain the
        // MCP/AI peer uses, so yield/hook handling is identical. The previous
        // interactive-only `eval_for_repl` used the non-yielding `vm.eval`,
        // which could not drain hooks fired mid-eval (e.g. `option-change` from
        // `set-option!`) and failed with "expected procedure, got void".
        if let Some(output) = crate::ai_event_handler::drain_pending_scheme_evals(editor, scheme) {
            // Surface the last result/error to the status bar for interactive use.
            if let Some(line) = output
                .lines()
                .rev()
                .find(|l| l.starts_with("; =>") || l.starts_with("; error"))
            {
                editor.set_status(line.trim_start_matches("; => "));
            }
        }
    }

    // --- Drain pending hook evaluations ---
    // Hook points fire in core (save, open, close) and push (hook_name, fn_name)
    // entries. We eval each function here where the SchemeRuntime is available.
    drain_hook_evals(editor, scheme);

    // --- Suppress gutter change indicators on *ai-input* buffer ---
    // The input buffer is ephemeral — gutter markers and [+] modified flag are meaningless.
    // This runs after ALL modes (Normal, ConversationInput, Visual, etc.) to catch every path.
    if let Some(ref pair) = editor.ai.conversation_pair {
        if pair.input_buffer_idx < editor.buffers.len() {
            let buf = &mut editor.buffers[pair.input_buffer_idx];
            buf.changed_lines.clear();
            buf.modified = false;
        }
    }
}

/// Evaluate all pending hook functions queued by `fire_hook`.
pub(crate) fn drain_hook_evals(editor: &mut Editor, scheme: &mut SchemeRuntime) {
    if editor.pending_hook_evals.is_empty() {
        return;
    }
    let hooks: Vec<(String, String)> = std::mem::take(&mut editor.pending_hook_evals);
    for (hook_name, fn_name) in hooks {
        // Try builtin command dispatch first — it's cheaper than a Scheme eval
        // and avoids triggering VM error-handling paths that can corrupt global
        // Scheme state (e.g. `format-before-save` is a dispatch arm, not a
        // Scheme function).
        if editor.dispatch_with_multicursor(&fn_name) {
            continue;
        }
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
pub(crate) fn is_operator_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "operator-delete" | "operator-change" | "operator-yank" | "operator-surround"
    )
}

/// Dispatch a command by name, handling both builtins and Scheme commands.
/// Fires command-pre/command-post hooks and per-command :before/:after advice.
pub(crate) fn dispatch_command(editor: &mut Editor, scheme: &mut SchemeRuntime, name: &str) {
    let theme_before = editor.theme.name.clone();
    editor.current_command = name.to_string();

    // Fire command-pre hook
    editor.fire_hook("command-pre");

    // Fire :before advice for this command
    let before_advice = editor
        .hooks
        .get_advice(name, mae_core::hooks::AdviceKind::Before);
    for fn_name in &before_advice {
        scheme.inject_editor_state(editor);
        if let Err(e) = scheme.call_function(fn_name) {
            warn!(command = name, advice = %fn_name, error = %e, "before-advice error");
        } else {
            scheme.apply_to_editor(editor);
        }
    }

    let source = editor.commands.get(name).map(|c| c.source.clone());

    match source {
        Some(CommandSource::Builtin) => {
            debug!(command = name, source = "builtin", "dispatching command");
            editor.dispatch_with_multicursor(name);
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
        Some(CommandSource::Autoload { feature }) => {
            debug!(command = name, feature = %feature, "autoloading feature for command");
            match scheme.require_feature(&feature) {
                Ok(()) => {
                    scheme.apply_to_editor(editor);
                    // After loading, the command should now be a Scheme command.
                    // Re-dispatch.
                    let new_source = editor.commands.get(name).map(|c| c.source.clone());
                    if let Some(CommandSource::Scheme(fn_name)) = new_source {
                        scheme.inject_editor_state(editor);
                        match scheme.call_function(&fn_name) {
                            Ok(result) => {
                                scheme.apply_to_editor(editor);
                                if !result.is_empty() {
                                    editor.set_status(result);
                                }
                            }
                            Err(e) => {
                                error!(command = name, error = %e, "autoloaded command failed");
                                editor.set_status(format!("Scheme error: {}", e));
                            }
                        }
                    } else {
                        editor.dispatch_with_multicursor(name);
                    }
                }
                Err(e) => {
                    error!(command = name, feature = %feature, error = %e, "autoload require failed");
                    editor.set_status(format!("Autoload error: {}", e));
                }
            }
        }
        None => {
            if !editor.dispatch_with_multicursor(name) {
                warn!(command = name, "unknown command");
                editor.set_status(format!("Unknown command: {}", name));
            }
        }
    }

    // Fire :after advice for this command
    let after_advice = editor
        .hooks
        .get_advice(name, mae_core::hooks::AdviceKind::After);
    for fn_name in &after_advice {
        scheme.inject_editor_state(editor);
        if let Err(e) = scheme.call_function(fn_name) {
            warn!(command = name, advice = %fn_name, error = %e, "after-advice error");
        } else {
            scheme.apply_to_editor(editor);
        }
    }

    // Fire command-post hook
    editor.fire_hook("command-post");

    // Persist theme change regardless of source (cycle-theme, set-theme, scheme).
    if editor.theme.name != theme_before {
        crate::config::persist_editor_preference("theme", &editor.theme.name);
    }
}
