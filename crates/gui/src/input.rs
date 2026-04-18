//! winit KeyEvent → mae_core InputEvent / KeyPress translation.
//!
//! This module maps OS-level key events from winit into the editor's
//! backend-agnostic input types. The GUI backend produces `InputEvent`
//! values that the main loop consumes identically to crossterm events.

use mae_core::{InputEvent, Key, KeyPress};
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key as WinitKey, NamedKey};

/// Convert a winit KeyEvent to a mae_core KeyPress, if possible.
///
/// Returns None for key releases, unrecognized keys, or modifier-only
/// events (Shift, Ctrl, Alt, Super pressed alone).
pub fn winit_key_to_keypress(event: &KeyEvent) -> Option<KeyPress> {
    if event.state != ElementState::Pressed {
        return None;
    }

    let mae_key = logical_key_to_mae(&event.logical_key)?;

    // Note: Ctrl/Alt modifier state is injected by the caller from
    // winit's ModifiersState, since KeyEvent doesn't carry modifiers.
    Some(KeyPress {
        key: mae_key,
        ctrl: false,
        alt: false,
    })
}

/// Convert a winit KeyEvent to an InputEvent, applying modifier state.
/// Used by the GUI event loop (main.rs with --gui flag).
#[allow(dead_code)]
pub fn winit_event_to_input(event: &KeyEvent, ctrl: bool, alt: bool) -> Option<InputEvent> {
    let mut kp = winit_key_to_keypress(event)?;
    kp.ctrl = ctrl;
    kp.alt = alt;

    // Ctrl+letter normalization: winit may produce uppercase characters
    // when Ctrl is held. Normalize to lowercase for consistency with the
    // terminal backend.
    if ctrl {
        if let Key::Char(ch) = kp.key {
            if ch.is_ascii_alphabetic() {
                kp.key = Key::Char(ch.to_ascii_lowercase());
            }
        }
    }

    Some(InputEvent::Key(kp))
}

/// Internal helper: translate a winit logical key to a mae Key.
/// Extracted from `winit_key_to_keypress` so it can be tested without
/// constructing winit KeyEvent (which has a platform-specific `pub(crate)` field).
fn logical_key_to_mae(key: &WinitKey) -> Option<Key> {
    match key {
        WinitKey::Character(s) => {
            let ch = s.chars().next()?;
            Some(Key::Char(ch))
        }
        WinitKey::Named(named) => match named {
            NamedKey::Escape => Some(Key::Escape),
            NamedKey::Enter => Some(Key::Enter),
            NamedKey::Backspace => Some(Key::Backspace),
            NamedKey::Tab => Some(Key::Tab),
            NamedKey::ArrowUp => Some(Key::Up),
            NamedKey::ArrowDown => Some(Key::Down),
            NamedKey::ArrowLeft => Some(Key::Left),
            NamedKey::ArrowRight => Some(Key::Right),
            NamedKey::Home => Some(Key::Home),
            NamedKey::End => Some(Key::End),
            NamedKey::PageUp => Some(Key::PageUp),
            NamedKey::PageDown => Some(Key::PageDown),
            NamedKey::Delete => Some(Key::Delete),
            NamedKey::F1 => Some(Key::F(1)),
            NamedKey::F2 => Some(Key::F(2)),
            NamedKey::F3 => Some(Key::F(3)),
            NamedKey::F4 => Some(Key::F(4)),
            NamedKey::F5 => Some(Key::F(5)),
            NamedKey::F6 => Some(Key::F(6)),
            NamedKey::F7 => Some(Key::F(7)),
            NamedKey::F8 => Some(Key::F(8)),
            NamedKey::F9 => Some(Key::F(9)),
            NamedKey::F10 => Some(Key::F(10)),
            NamedKey::F11 => Some(Key::F(11)),
            NamedKey::F12 => Some(Key::F(12)),
            NamedKey::Space => Some(Key::Char(' ')),
            NamedKey::Shift | NamedKey::Control | NamedKey::Alt | NamedKey::Super => None,
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    #[test]
    fn character_key() {
        let key = WinitKey::Character(SmolStr::new("a"));
        let mae = logical_key_to_mae(&key).unwrap();
        assert_eq!(mae, Key::Char('a'));
    }

    #[test]
    fn escape_key() {
        let key = WinitKey::Named(NamedKey::Escape);
        assert_eq!(logical_key_to_mae(&key).unwrap(), Key::Escape);
    }

    #[test]
    fn arrow_keys() {
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::ArrowUp)).unwrap(),
            Key::Up
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::ArrowDown)).unwrap(),
            Key::Down
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::ArrowLeft)).unwrap(),
            Key::Left
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::ArrowRight)).unwrap(),
            Key::Right
        );
    }

    #[test]
    fn modifier_only_returns_none() {
        assert!(logical_key_to_mae(&WinitKey::Named(NamedKey::Control)).is_none());
        assert!(logical_key_to_mae(&WinitKey::Named(NamedKey::Shift)).is_none());
        assert!(logical_key_to_mae(&WinitKey::Named(NamedKey::Alt)).is_none());
        assert!(logical_key_to_mae(&WinitKey::Named(NamedKey::Super)).is_none());
    }

    #[test]
    fn space_key() {
        let key = WinitKey::Named(NamedKey::Space);
        assert_eq!(logical_key_to_mae(&key).unwrap(), Key::Char(' '));
    }

    #[test]
    fn function_keys() {
        let named_keys = [
            (NamedKey::F1, 1),
            (NamedKey::F2, 2),
            (NamedKey::F3, 3),
            (NamedKey::F4, 4),
            (NamedKey::F5, 5),
            (NamedKey::F6, 6),
            (NamedKey::F7, 7),
            (NamedKey::F8, 8),
            (NamedKey::F9, 9),
            (NamedKey::F10, 10),
            (NamedKey::F11, 11),
            (NamedKey::F12, 12),
        ];
        for (named, n) in named_keys {
            assert_eq!(
                logical_key_to_mae(&WinitKey::Named(named)).unwrap(),
                Key::F(n)
            );
        }
    }

    #[test]
    fn special_keys() {
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::Enter)).unwrap(),
            Key::Enter
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::Backspace)).unwrap(),
            Key::Backspace
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::Tab)).unwrap(),
            Key::Tab
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::Home)).unwrap(),
            Key::Home
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::End)).unwrap(),
            Key::End
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::PageUp)).unwrap(),
            Key::PageUp
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::PageDown)).unwrap(),
            Key::PageDown
        );
        assert_eq!(
            logical_key_to_mae(&WinitKey::Named(NamedKey::Delete)).unwrap(),
            Key::Delete
        );
    }
}
