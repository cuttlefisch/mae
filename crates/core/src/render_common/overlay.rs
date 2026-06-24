//! # Overlay priority — single source of truth (shared by GUI + TUI)
//!
//! The editor draws at most one *fullscreen* overlay at a time (a modal dialog, a
//! fuzzy picker, the command palette, …) on top of the window view; additive popups
//! (LSP completion/hover/etc.) layer over the normal view separately.
//!
//! Historically each backend hard-coded this priority as its own `if/else` chain,
//! and they **diverged**: the GUI drew the blocking `mini_dialog` modal at top
//! priority while the TUI only drew it nested under the command palette — so an
//! async-raised modal (the host-key TOFU prompt) painted no dialog in the TUI
//! (B-22a). The input dispatch had the same class of bug (B-22b).
//!
//! [`active_overlay`] is the ONE place the priority lives. Both render chains derive
//! their dispatch from it, so they cannot drift apart again, and the ordering is
//! unit-tested. A blocking modal is always highest priority — matching the input
//! side, which routes all keys to `mini_dialog` whenever it is present.

use crate::Editor;

/// The single fullscreen overlay currently on top, in priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveOverlay {
    /// A blocking/confirm modal (host-key TOFU prompt, discard confirm, …). Highest
    /// priority: captures input *and* render whenever present, in any mode.
    MiniDialog,
    /// Fuzzy file/command picker.
    FilePicker,
    /// Tree-style file browser.
    FileBrowser,
    /// Fuzzy command palette.
    CommandPalette,
    /// which-key / buffer-keys / leader transient hint.
    WhichKey,
    /// Dashboard splash screen.
    Splash,
    /// No fullscreen overlay — the normal editing view (windows + additive popups).
    None,
}

/// Resolve which fullscreen overlay is active, in the canonical priority order.
/// Both the GUI and TUI render chains dispatch on this so they stay in lock-step.
pub fn active_overlay(editor: &Editor) -> ActiveOverlay {
    if editor.mini_dialog.is_some() {
        ActiveOverlay::MiniDialog
    } else if editor.file_picker.is_some() {
        ActiveOverlay::FilePicker
    } else if editor.file_browser.is_some() {
        ActiveOverlay::FileBrowser
    } else if editor.command_palette.is_some() {
        ActiveOverlay::CommandPalette
    } else if !editor.which_key_prefix.is_empty()
        || editor.buffer_keys_popup
        || editor.leader_active
    {
        ActiveOverlay::WhichKey
    } else if super::splash::should_show_splash(editor) {
        ActiveOverlay::Splash
    } else {
        ActiveOverlay::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command_palette::{MiniDialogContext, MiniDialogState};

    #[test]
    fn mini_dialog_is_top_priority() {
        let mut ed = Editor::new();
        // Baseline: no overlay.
        assert_eq!(active_overlay(&ed), ActiveOverlay::None);
        // which-key/leader transient outranks the normal view.
        ed.leader_active = true;
        assert_eq!(active_overlay(&ed), ActiveOverlay::WhichKey);
        // A blocking modal outranks everything, regardless of other state.
        ed.mini_dialog = Some(MiniDialogState::confirm(
            "Trust?",
            MiniDialogContext::Notification { notif_id: 1 },
        ));
        assert_eq!(
            active_overlay(&ed),
            ActiveOverlay::MiniDialog,
            "a blocking modal must win over every other overlay, in any mode"
        );
        ed.mini_dialog = None;
        assert_eq!(active_overlay(&ed), ActiveOverlay::WhichKey);
        ed.leader_active = false;
        assert_eq!(active_overlay(&ed), ActiveOverlay::None);
    }
}
