//! Event bridging between alacritty_terminal and MAE's event loop.

use std::sync::mpsc;

use alacritty_terminal::event::EventListener;

/// Events emitted by the terminal emulator that MAE needs to handle.
#[derive(Debug)]
pub enum ShellEvent {
    /// Terminal bell — flash status bar or play sound.
    Bell,
    /// Terminal title changed (e.g., shell sets window title via escape seq).
    Title(String),
    /// Title reset to default.
    ResetTitle,
    /// Child process exited.
    ChildExit(i32),
    /// Terminal has new content to render.
    Wakeup,
    /// Terminal wants to write data to PTY (e.g., response to device query).
    PtyWrite(String),
    /// Terminal wants to store text in clipboard.
    ClipboardStore(String),
}

/// Bridges alacritty_terminal's `EventListener` to MAE's event system
/// via a channel. Cloneable so the event loop thread can hold a copy.
#[derive(Clone)]
pub struct ShellEventListener {
    tx: mpsc::Sender<ShellEvent>,
}

impl ShellEventListener {
    pub fn new(tx: mpsc::Sender<ShellEvent>) -> Self {
        Self { tx }
    }
}

impl EventListener for ShellEventListener {
    fn send_event(&self, event: alacritty_terminal::event::Event) {
        use alacritty_terminal::event::Event as AE;
        let shell_event = match event {
            AE::Bell => ShellEvent::Bell,
            AE::Title(t) => ShellEvent::Title(t),
            AE::ResetTitle => ShellEvent::ResetTitle,
            AE::Wakeup => ShellEvent::Wakeup,
            AE::PtyWrite(s) => ShellEvent::PtyWrite(s),
            AE::ClipboardStore(_, s) => ShellEvent::ClipboardStore(s),
            AE::Exit => ShellEvent::ChildExit(0),
            // ColorRequest (OSC 10/11/4) is intentionally NOT forwarded.
            // Responding requires locking the terminal FairMutex, which
            // contends with the I/O thread and can freeze the event loop.
            // Theme colors are communicated via COLORFGBG, TERM_BACKGROUND
            // env vars and the ANSI palette (set_theme_colors) instead.
            // Events we don't need to forward yet.
            _ => return,
        };
        let _ = self.tx.send(shell_event);
    }
}
