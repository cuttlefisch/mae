use super::{crossterm_to_keypress, dispatch_command, is_operator_command};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::{BufferKind, LookupResult};
use mae_core::{Editor, KeyPress, Mode};
use mae_scheme::SchemeRuntime;

pub(super) fn handle_keymap_mode(
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
        Mode::GitStatus => "git-status",
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
            // C-o oneshot: return to insert mode after one normal command
            if editor.insert_mode_oneshot_normal && editor.mode == Mode::Normal {
                editor.insert_mode_oneshot_normal = false;
                editor.set_mode(Mode::Insert);
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
pub(super) fn handle_describe_key_await(
    editor: &mut Editor,
    key: KeyEvent,
    pending_keys: &mut Vec<KeyPress>,
) {
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

pub(super) fn handle_normal_mode(
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
                editor.set_mode(Mode::ConversationInput);
                editor.count_prefix = None;
                return;
            }
            _ => {}
        }
    }

    handle_keymap_mode(editor, scheme, key, pending_keys);
}
