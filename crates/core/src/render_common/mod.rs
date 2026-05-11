//! Shared rendering logic used by both GUI (Skia) and TUI (ratatui) backends.
//!
//! This module contains pure-data types and functions that compute *what* to
//! render without depending on *how* to render it.  Backend-specific code
//! converts these shared types into Skia draw calls or ratatui Spans.

pub mod agenda;
pub mod color;
pub mod debug;
pub mod diagnostics;
pub mod file_tree;
pub mod git_status;
pub mod gutter;
pub mod help;
pub mod hover;
pub mod messages;
pub mod shell;
pub mod spans;
pub mod splash;
pub mod status;
