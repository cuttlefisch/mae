use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::{Editor, KeyPress};
use mae_scheme::SchemeRuntime;

pub(super) fn handle_insert_mode(
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
            super::normal::handle_keymap_mode(editor, scheme, key, pending_keys);
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
        // C-t: indent current line (vim insert-mode indent)
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.dispatch_builtin("indent-line");
            editor.lsp_dismiss_completion();
            return;
        }
        // C-d: dedent (vim, default) or delete-char-forward (Emacs), configurable
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if editor.insert_ctrl_d == "delete-forward" {
                let idx = editor.active_buffer_idx();
                let win = editor.window_mgr.focused_window_mut();
                editor.buffers[idx].delete_char_forward(win);
            } else {
                editor.dispatch_builtin("dedent-line");
            }
            editor.lsp_dismiss_completion();
            return;
        }
        // C-o: execute one normal-mode command, then return to insert
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            editor.insert_mode_oneshot_normal = true;
            editor.set_mode(mae_core::Mode::Normal);
            editor.set_status("-- (insert) -- C-o: one normal command, then back to insert");
            return;
        }
        _ => {}
    }
    super::normal::handle_keymap_mode(editor, scheme, key, pending_keys);
}
