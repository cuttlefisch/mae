//! Unified input events, independent of rendering backend.
//!
//! Both the terminal backend (crossterm) and the GUI backend (winit) produce
//! `InputEvent` values. The main loop consumes them without caring about the
//! source. This is the input-side complement to the `Renderer` trait on the
//! output side.

use crate::KeyPress;

/// A backend-agnostic input event.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// A key was pressed (or auto-repeated).
    Key(KeyPress),
    /// The viewport was resized to (width, height) in the backend's native
    /// units (columns for terminal, pixels for GUI).
    Resize(u16, u16),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Key;

    #[test]
    fn input_event_key_round_trip() {
        let kp = KeyPress {
            key: Key::Char('x'),
            ctrl: true,
            alt: false,
        };
        let event = InputEvent::Key(kp.clone());
        if let InputEvent::Key(got) = event {
            assert_eq!(got, kp);
        } else {
            panic!("expected Key variant");
        }
    }

    #[test]
    fn input_event_resize() {
        let event = InputEvent::Resize(120, 40);
        assert_eq!(event, InputEvent::Resize(120, 40));
    }
}
