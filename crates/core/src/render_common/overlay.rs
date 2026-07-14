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
    /// Native KB graph view in full-frame overlay mode (dimmed background,
    /// `kb-graph-view-toggle-overlay`) — deliberately below `WhichKey` (a
    /// leader-key hint should still show over it) but above `Splash` (an
    /// intentionally-toggled view outranks the idle dashboard).
    GraphView,
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
        || editor.leader_popup_ready()
    {
        ActiveOverlay::WhichKey
    } else if editor.kb_graph_view_overlay_active
        && editor
            .buffers
            .iter()
            .any(|b| b.kind == crate::BufferKind::Graph)
    {
        ActiveOverlay::GraphView
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
        // which-key/leader transient outranks the normal view. Uses
        // `set_leader_active` (not the bare field) so `leader_activated_at`
        // is populated — required for `leader_popup_ready()`'s
        // `which_key_idle_delay` gate (default 0ms = immediate) to resolve
        // true, matching pre-idle-delay behavior exactly.
        ed.set_leader_active(true);
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
        ed.set_leader_active(false);
        assert_eq!(active_overlay(&ed), ActiveOverlay::None);
    }

    #[test]
    fn graph_view_overlay_requires_both_the_flag_and_an_open_graph_buffer() {
        let mut ed = Editor::new();

        // Flag alone, no graph buffer open — must not activate the overlay.
        ed.kb_graph_view_overlay_active = true;
        assert_ne!(
            active_overlay(&ed),
            ActiveOverlay::GraphView,
            "toggling the flag with no graph view open must not show an empty overlay"
        );

        // Open a graph buffer — now the flag takes effect.
        ed.kb_graph_view_open(Some("index".to_string()), Some(1));
        assert_eq!(active_overlay(&ed), ActiveOverlay::GraphView);

        // Flipping the flag back off (e.g. via kb_graph_view_toggle_overlay)
        // returns to the normal view even with the graph buffer still open.
        ed.kb_graph_view_overlay_active = false;
        assert_ne!(active_overlay(&ed), ActiveOverlay::GraphView);
    }

    #[test]
    fn which_key_outranks_the_graph_view_overlay() {
        let mut ed = Editor::new();
        ed.kb_graph_view_open(Some("index".to_string()), Some(1));
        ed.kb_graph_view_overlay_active = true;
        assert_eq!(active_overlay(&ed), ActiveOverlay::GraphView);

        ed.set_leader_active(true);
        assert_eq!(
            active_overlay(&ed),
            ActiveOverlay::WhichKey,
            "a leader-key hint must still show over the graph overlay"
        );
    }

    #[test]
    fn toggle_overlay_flips_the_flag_and_reports_the_new_state() {
        let mut ed = Editor::new();
        // No graph open — no-op.
        assert!(!ed.kb_graph_view_toggle_overlay());
        assert!(!ed.kb_graph_view_overlay_active);

        ed.kb_graph_view_open(Some("index".to_string()), Some(1));
        assert!(ed.kb_graph_view_toggle_overlay());
        assert!(ed.kb_graph_view_overlay_active);
        assert!(!ed.kb_graph_view_toggle_overlay());
        assert!(!ed.kb_graph_view_overlay_active);
    }

    #[test]
    fn which_key_idle_delay_defers_the_overlay_until_elapsed() {
        // Regression for ROADMAP #83: a nonzero `which_key_idle_delay` must
        // hide the which-key overlay right after leader-mode activation and
        // only reveal it once that much idle time has actually passed.
        let mut ed = Editor::new();
        ed.which_key_idle_delay = 50;
        ed.set_leader_active(true);
        assert_eq!(
            active_overlay(&ed),
            ActiveOverlay::None,
            "must not show immediately when a nonzero delay is configured"
        );
        // Backdate the activation instant instead of sleeping — deterministic.
        ed.leader_activated_at =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(60));
        assert_eq!(
            active_overlay(&ed),
            ActiveOverlay::WhichKey,
            "must show once the configured idle delay has elapsed"
        );
    }
}
