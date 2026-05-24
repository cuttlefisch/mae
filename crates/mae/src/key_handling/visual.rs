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
    if editor.vi.pending_register_prompt {
        editor.vi.pending_register_prompt = false;
        if let KeyCode::Char(ch) = key.code {
            editor.vi.active_register = Some(ch);
            editor.set_status(format!("\"{}", ch));
        } else {
            editor.set_status("");
        }
        return;
    }

    // Handle pending char-argument commands (f/F/t/T or text objects)
    if let Some(cmd) = editor.vi.pending_char_command.take() {
        if let KeyCode::Char(ch) = key.code {
            let had_pending_op = editor.vi.pending_operator.is_some();
            if editor.dispatch_text_object(&cmd, ch) || editor.dispatch_surround(&cmd, ch) {
                // Text object/surround handled it directly — clear dangling state
                editor.vi.pending_operator = None;
                editor.vi.operator_start = None;
                editor.vi.operator_count = None;
            } else {
                editor.dispatch_char_motion(&cmd, ch);
                if had_pending_op {
                    editor.vi.last_motion_linewise = false;
                    editor.apply_pending_operator();
                }
            }
        } else {
            editor.vi.pending_operator = None;
            editor.vi.operator_start = None;
            editor.vi.operator_count = None;
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
            let current = editor.vi.count_prefix.unwrap_or(0);
            editor.vi.count_prefix = Some((current * 10 + digit).min(99999));
            return;
        }
    }
    if let KeyCode::Char('0') = key.code {
        if !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            && editor.vi.count_prefix.is_some()
            && pending_keys.is_empty()
        {
            let current = editor.vi.count_prefix.unwrap_or(0);
            editor.vi.count_prefix = Some((current * 10).min(99999));
            return;
        }
    }

    if key.code == KeyCode::Esc {
        editor.vi.count_prefix = None;
    }

    super::normal::handle_keymap_mode(editor, scheme, key, pending_keys);
}
