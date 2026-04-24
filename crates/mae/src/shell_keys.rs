//! Shell key handling — ShellInsert mode key dispatch and PTY byte translation.

use mae_core::{Editor, Key, KeyPress, Mode};

use crate::key_handling;

/// Compute the PTY-appropriate cols/rows for a shell in a given buffer,
/// accounting for split window dimensions via `layout_rects()`.
///
/// Falls back to full terminal dimensions if the buffer isn't visible
/// in any window (shouldn't happen in practice).
pub(crate) fn shell_dims_for_buffer(
    editor: &Editor,
    renderer: &dyn mae_renderer::Renderer,
    buf_idx: usize,
) -> (u16, u16) {
    let (term_w, term_h) = renderer.size().unwrap_or((80, 24));
    let window_area = mae_core::WinRect {
        x: 0,
        y: 0,
        width: term_w,
        height: term_h.saturating_sub(2), // status bar + command line
    };
    let rects = editor.window_mgr.layout_rects(window_area);

    // Find the window that owns this buffer.
    for win in editor.window_mgr.iter_windows() {
        if win.buffer_idx == buf_idx {
            if let Some((_, rect)) = rects.iter().find(|(id, _)| *id == win.id) {
                let cols = rect.width.saturating_sub(2).max(2); // border
                let rows = rect.height.saturating_sub(2).max(1); // border
                return (cols, rows);
            }
        }
    }

    // Fallback: full terminal minus chrome.
    (
        term_w.saturating_sub(4).max(2),
        term_h.saturating_sub(4).max(1),
    )
}

/// Handle a key event while in ShellInsert mode.
///
/// Keys are checked against the "shell-insert" keymap first. If the key
/// sequence matches a binding, the command is dispatched. If it's a prefix
/// of a binding, the key is held until more keys arrive. Otherwise, all
/// pending keys are translated to PTY byte sequences and forwarded.
///
/// This replaces the previous hardcoded Ctrl-\ Ctrl-n escape sequence with
/// the standard keymap system — the Lisp machine principle that all
/// user-facing behavior must be hot-reloadable.
pub(crate) fn handle_shell_key(
    editor: &mut Editor,
    key: crossterm::event::KeyEvent,
    shell_terminals: &mut std::collections::HashMap<usize, mae_shell::ShellTerminal>,
    shell_pending_keys: &mut Vec<KeyPress>,
) {
    use mae_core::LookupResult;

    let Some(kp) = key_handling::crossterm_to_keypress(&key) else {
        return;
    };

    shell_pending_keys.push(kp);

    // Look up accumulated keys in the shell-insert keymap.
    let lookup = editor
        .keymaps
        .get("shell-insert")
        .map(|km| km.lookup(shell_pending_keys))
        .unwrap_or(LookupResult::None);

    match lookup {
        LookupResult::Exact(cmd) => {
            let cmd = cmd.to_string();
            shell_pending_keys.clear();
            editor.execute_command(&cmd);
        }
        LookupResult::Prefix => {
            // Wait for more keys — don't send anything to PTY yet.
        }
        LookupResult::None => {
            // No binding matches. Flush all pending keys to the PTY.
            let keys_to_send = std::mem::take(shell_pending_keys);

            let Some(shell) = shell_terminals.get(&editor.active_buffer_idx()) else {
                editor.set_mode(Mode::Normal);
                editor.set_status("Terminal exited — returned to normal mode");
                return;
            };

            if shell.has_exited() {
                editor.set_mode(Mode::Normal);
                editor.set_status("Terminal process has exited");
                return;
            }

            for kp in &keys_to_send {
                let bytes = keypress_to_pty_bytes(kp);
                if !bytes.is_empty() {
                    shell.write_input(&bytes);
                }
            }
        }
    }
}

/// Convert a mae_core KeyPress into PTY byte sequences for the shell.
pub(crate) fn keypress_to_pty_bytes(kp: &KeyPress) -> Vec<u8> {
    match &kp.key {
        Key::Char(c) => {
            if kp.ctrl {
                let byte = (c.to_ascii_lowercase() as u8)
                    .wrapping_sub(b'a')
                    .wrapping_add(1);
                vec![byte]
            } else if kp.alt {
                let mut v = vec![0x1b];
                let mut buf = [0u8; 4];
                v.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                v
            } else {
                let mut buf = [0u8; 4];
                c.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        Key::Enter => vec![b'\r'],
        Key::Backspace => vec![0x7f],
        Key::Tab => vec![b'\t'],
        Key::BackTab => b"\x1b[Z".to_vec(),
        Key::Escape => vec![0x1b],
        Key::Up => b"\x1b[A".to_vec(),
        Key::Down => b"\x1b[B".to_vec(),
        Key::Right => b"\x1b[C".to_vec(),
        Key::Left => b"\x1b[D".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::F(1) => b"\x1bOP".to_vec(),
        Key::F(2) => b"\x1bOQ".to_vec(),
        Key::F(3) => b"\x1bOR".to_vec(),
        Key::F(4) => b"\x1bOS".to_vec(),
        Key::F(5) => b"\x1b[15~".to_vec(),
        Key::F(6) => b"\x1b[17~".to_vec(),
        Key::F(7) => b"\x1b[18~".to_vec(),
        Key::F(8) => b"\x1b[19~".to_vec(),
        Key::F(9) => b"\x1b[20~".to_vec(),
        Key::F(10) => b"\x1b[21~".to_vec(),
        Key::F(11) => b"\x1b[23~".to_vec(),
        Key::F(12) => b"\x1b[24~".to_vec(),
        _ => vec![],
    }
}
