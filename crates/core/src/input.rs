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
    /// A mouse button was clicked at (row, col) in cell coordinates.
    MouseClick {
        row: u16,
        col: u16,
        button: MouseButton,
    },
    /// Mouse wheel scrolled (positive = up, negative = down).
    MouseScroll { delta: i16 },
}

/// Mouse button identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
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

    #[test]
    fn input_event_mouse_click() {
        let event = InputEvent::MouseClick {
            row: 10,
            col: 5,
            button: MouseButton::Left,
        };
        if let InputEvent::MouseClick { row, col, button } = event {
            assert_eq!(row, 10);
            assert_eq!(col, 5);
            assert_eq!(button, MouseButton::Left);
        } else {
            panic!("expected MouseClick variant");
        }
    }

    #[test]
    fn input_event_mouse_scroll() {
        let up = InputEvent::MouseScroll { delta: 3 };
        let down = InputEvent::MouseScroll { delta: -3 };
        assert_eq!(up, InputEvent::MouseScroll { delta: 3 });
        assert_eq!(down, InputEvent::MouseScroll { delta: -3 });
    }

    #[test]
    fn mouse_button_variants() {
        assert_ne!(MouseButton::Left, MouseButton::Right);
        assert_ne!(MouseButton::Left, MouseButton::Middle);
        assert_ne!(MouseButton::Right, MouseButton::Middle);
        // Clone + Copy
        let b = MouseButton::Left;
        let b2 = b;
        assert_eq!(b, b2);
    }
}
