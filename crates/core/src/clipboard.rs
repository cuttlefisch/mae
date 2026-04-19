//! Thin shell-out shim for the system clipboard.
//!
//! Vim's `"+` and `"*` registers map to whatever the host platform
//! calls its clipboard. We deliberately keep this dependency-free and
//! shell out to whichever of `wl-copy`/`xclip`/`pbcopy` is on `PATH`,
//! falling back to an error on unknown platforms. This matches the
//! approach `:!cmd` already uses for shell escapes (see `command.rs`)
//! and keeps the `mae-core` crate's dep surface minimal — important
//! because `mae-core` is the crate that embeds in other UI backends.
//!
//! The slower shell-out cost (a ~5ms subprocess spawn) is acceptable:
//! clipboard ops are user-initiated and infrequent. If that becomes a
//! hot path we can swap in `arboard` behind the same API.

use std::io::Write;
use std::process::{Command, Stdio};

/// Error returned when neither a copy tool nor a paste tool is
/// available on the system.
#[derive(Debug)]
pub enum ClipboardError {
    NoBackend,
    Io(std::io::Error),
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardError::NoBackend => {
                f.write_str("no clipboard tool found (tried wl-copy/xclip/pbcopy)")
            }
            ClipboardError::Io(e) => write!(f, "clipboard command failed: {}", e),
        }
    }
}

impl std::error::Error for ClipboardError {}

impl From<std::io::Error> for ClipboardError {
    fn from(e: std::io::Error) -> Self {
        ClipboardError::Io(e)
    }
}

/// Push `text` to the system clipboard. Picks the first available of:
/// `wl-copy` (Wayland), `xclip -selection clipboard` (X11), `pbcopy`
/// (macOS).
pub fn copy(text: &str) -> Result<(), ClipboardError> {
    for (cmd, args) in copy_candidates() {
        if let Some(mut child) = spawn_stdin(cmd, args) {
            if let Some(mut stdin) = child.stdin.take() {
                stdin.write_all(text.as_bytes())?;
            }
            let status = child.wait()?;
            if status.success() {
                return Ok(());
            }
        }
    }
    Err(ClipboardError::NoBackend)
}

/// Read the current clipboard contents. Mirror of [`copy`]: tries
/// `wl-paste`, `xclip -selection clipboard -o`, `pbpaste`.
pub fn paste() -> Result<String, ClipboardError> {
    for (cmd, args) in paste_candidates() {
        let output = Command::new(cmd).args(*args).output();
        if let Ok(out) = output {
            if out.status.success() {
                return Ok(String::from_utf8_lossy(&out.stdout).into_owned());
            }
        }
    }
    Err(ClipboardError::NoBackend)
}

fn copy_candidates() -> &'static [(&'static str, &'static [&'static str])] {
    // Order: Wayland first (modern default), then X11, then macOS.
    &[
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("pbcopy", &[]),
    ]
}

fn paste_candidates() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        ("wl-paste", &["--no-newline", "--type", "text"]),
        ("xclip", &["-selection", "clipboard", "-o"]),
        ("pbpaste", &[]),
    ]
}

fn spawn_stdin(cmd: &str, args: &[&str]) -> Option<std::process::Child> {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}
