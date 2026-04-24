use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mae_core::{Editor, KeyPress};
use mae_scheme::SchemeRuntime;

pub(super) fn handle_visual_mode(
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

    super::normal::handle_keymap_mode(editor, scheme, key, pending_keys);
}
