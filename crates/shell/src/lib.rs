//! Terminal emulator for MAE, wrapping `alacritty_terminal`.
//!
//! This crate provides a `ShellTerminal` type that embeds a full VT100/VT500
//! terminal emulator backed by alacritty_terminal. It manages PTY lifecycle,
//! input/output, and exposes grid state for rendering.
//!
//! Design: terminal-first shell (see AD1/AD3 in architecture plan). The real
//! shell runs in a PTY; we provide full terminal emulation so programs like
//! vim, less, top, fzf, and tmux work correctly.

mod event;
mod terminal;

pub use event::{ShellEvent, ShellEventListener};
pub use terminal::ShellTerminal;
