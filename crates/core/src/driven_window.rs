//! `DrivenWindow` — a first-class "window some actor is driving" primitive.
//!
//! MAE previously had no general concept of "a window some actor is driving
//! over time" — only ad-hoc, narrowly-scoped fields (`ai.work_window_id`,
//! `file_tree_window_id`) each hand-rolling their own validate/reuse logic.
//! That absence is why the AI/MCP window cascade bug existed (see
//! `concept:display-policy` / ADR discussion in the architecture plan): three
//! disconnected, process-global window-selection mechanisms with no shared
//! state, so an agent-driven sequence crossing `BufferKind` boundaries
//! (file -> KB node -> shell -> file...) would repeatedly manufacture fresh
//! splits instead of reusing the window it was already driving.
//!
//! `DrivenWindow` is a validated, lazily-(re)created reference to "the window
//! some actor is currently driving" — an AI/MCP session, a graph panel, and
//! (in the future) per-MCP-session actors or other feature panels all use
//! this SAME type. It is not tied to any single `BufferKind`: the actor may
//! redisplay many different kinds of content into the same driven window
//! over its lifetime.
//!
//! Two population strategies are provided as named methods, corresponding to
//! the two real usages this primitive was built for. Both share the same
//! validity check (`get_valid`) — the only genuinely reusable "computation"
//! here — but differ in *when and how* the stored id gets (re)written:
//!
//! - [`DrivenWindow::resolve_persistent`]: the actor OWNS and reuses one
//!   window across many of its own actions, choosing/creating it itself when
//!   none is valid yet (AI/MCP tool-call sequences: "keep using the window
//!   I've been driving").
//! - [`DrivenWindow::follow_focus_away_from`]: the window is CAPTURED
//!   reactively — updated every time focus moves away from a guard window to
//!   something else, with no ownership/choice by the actor (a graph panel:
//!   "whichever window had focus right before attention moved to me").
//!
//! Any future driven-window need (a per-MCP-session target, a future feature
//! panel) should use `DrivenWindow` too, picking whichever existing strategy
//! fits or adding a third named one here if a genuinely new population
//! pattern emerges — this module is the intended single home for all of
//! them, not a per-feature reimplementation.

use crate::window::{WindowId, WindowManager};

/// A validated, lazily-(re)created reference to "the window some actor is
/// currently driving." See module docs for the full rationale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrivenWindow(Option<WindowId>);

impl DrivenWindow {
    /// An empty `DrivenWindow` — no window driven yet.
    pub const fn none() -> Self {
        DrivenWindow(None)
    }

    /// Returns the stored window id if it still refers to a live window,
    /// `None` otherwise (stale references are never surfaced, but are also
    /// not auto-cleared here — callers that want to forget a stale id should
    /// call `set(None)` explicitly, e.g. via `resolve_persistent`).
    pub fn get_valid(&self, window_mgr: &WindowManager) -> Option<WindowId> {
        self.0.filter(|id| window_mgr.window(*id).is_some())
    }

    /// Unconditionally overwrite the stored window id (or clear it with
    /// `None`). Used when the caller has already computed which window to
    /// remember through logic too complex to express as the `create_or_pick`
    /// closure `resolve_persistent` expects (see that method's doc comment).
    pub fn set(&mut self, id: Option<WindowId>) {
        self.0 = id;
    }

    /// Persistent-ownership strategy: reuse the stored window if still valid,
    /// otherwise ask `create_or_pick` to choose one and remember it.
    ///
    /// `create_or_pick` only receives `&WindowManager` (not a full `&mut
    /// Editor`) by design: it keeps this primitive decoupled from
    /// `mae-core::Editor` and free of borrow-checker entanglement with
    /// whatever else the caller's `self` holds. This makes it a clean fit
    /// for callers whose window-selection logic only needs
    /// window/layout state (e.g. the graph view's companion-window
    /// fallback). Callers whose selection logic also needs buffer-kind
    /// lookups or other `Editor`-wide state (e.g. the AI work-window's
    /// multi-step commandeer-or-split fallback) should instead compute the
    /// id themselves and call `get_valid`/`set` directly — see
    /// `Editor::display_buffer_for_agent` for that shape.
    pub fn resolve_persistent(
        &mut self,
        window_mgr: &WindowManager,
        create_or_pick: impl FnOnce(&WindowManager) -> WindowId,
    ) -> WindowId {
        if let Some(id) = self.get_valid(window_mgr) {
            return id;
        }
        let id = create_or_pick(window_mgr);
        self.0 = Some(id);
        id
    }

    /// Follow-previous-focus strategy: call from wherever editor focus
    /// changes (e.g. `Editor::focus_window_at`). Updates the stored id to
    /// whatever was JUST focused, but only when that's not `guard_window`
    /// itself — so clicking around inside the guarded panel repeatedly
    /// doesn't overwrite the captured companion with the panel's own id.
    pub fn follow_focus_away_from(&mut self, newly_focused: WindowId, guard_window: WindowId) {
        if newly_focused != guard_window {
            self.0 = Some(newly_focused);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_valid_none_when_never_set() {
        let wm = WindowManager::new(0);
        let dw = DrivenWindow::none();
        assert_eq!(dw.get_valid(&wm), None);
    }

    #[test]
    fn get_valid_some_when_window_exists() {
        let wm = WindowManager::new(0);
        let id = wm.focused_id();
        let dw = DrivenWindow(Some(id));
        assert_eq!(dw.get_valid(&wm), Some(id));
    }

    #[test]
    fn get_valid_none_when_window_stale() {
        let wm = WindowManager::new(0);
        // 999 was never allocated by this WindowManager.
        let dw = DrivenWindow(Some(999));
        assert_eq!(dw.get_valid(&wm), None);
    }

    #[test]
    fn set_overwrites_stored_id() {
        let mut dw = DrivenWindow::none();
        dw.set(Some(42));
        assert_eq!(dw.0, Some(42));
        dw.set(None);
        assert_eq!(dw.0, None);
    }

    #[test]
    fn resolve_persistent_reuses_valid_id_without_calling_create() {
        let mut wm = WindowManager::new(0);
        let area = crate::window::Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        };
        let split_id = wm
            .split(crate::window::SplitDirection::Vertical, 0, area)
            .expect("split should succeed");
        let mut dw = DrivenWindow(Some(split_id));

        let mut create_called = false;
        let resolved = dw.resolve_persistent(&wm, |_wm| {
            create_called = true;
            0 // would be wrong if called
        });

        assert_eq!(resolved, split_id);
        assert!(
            !create_called,
            "create_or_pick must not run when id is valid"
        );
        assert_eq!(dw.get_valid(&wm), Some(split_id));
    }

    #[test]
    fn resolve_persistent_recreates_on_stale_id() {
        let wm = WindowManager::new(0);
        let mut dw = DrivenWindow(Some(999)); // stale — never allocated

        let resolved = dw.resolve_persistent(&wm, |wm| wm.focused_id());

        assert_eq!(resolved, wm.focused_id());
        assert_eq!(dw.0, Some(wm.focused_id()));
    }

    #[test]
    fn resolve_persistent_picks_when_never_set() {
        let wm = WindowManager::new(0);
        let mut dw = DrivenWindow::none();

        let resolved = dw.resolve_persistent(&wm, |wm| wm.focused_id());

        assert_eq!(resolved, wm.focused_id());
        assert_eq!(dw.get_valid(&wm), Some(wm.focused_id()));
    }

    #[test]
    fn follow_focus_away_from_updates_on_non_guard_focus() {
        let mut dw = DrivenWindow::none();
        dw.follow_focus_away_from(5, /* guard */ 1);
        assert_eq!(dw.0, Some(5));
    }

    #[test]
    fn follow_focus_away_from_ignores_guard_window_focus() {
        let mut dw = DrivenWindow(Some(5));
        // Focus moved TO the guard window itself (e.g. clicking inside the
        // graph panel) — must not overwrite the captured companion.
        dw.follow_focus_away_from(1, /* guard */ 1);
        assert_eq!(
            dw.0,
            Some(5),
            "guard-window focus must not overwrite the captured id"
        );
    }

    #[test]
    fn follow_focus_away_from_tracks_repeated_focus_changes() {
        let mut dw = DrivenWindow::none();
        let guard = 1;
        // Focus bounces between several non-guard windows — always captures
        // the latest.
        dw.follow_focus_away_from(2, guard);
        assert_eq!(dw.0, Some(2));
        dw.follow_focus_away_from(3, guard);
        assert_eq!(dw.0, Some(3));
        // Then focus moves to the guard (graph) window — ignored.
        dw.follow_focus_away_from(guard, guard);
        assert_eq!(dw.0, Some(3));
        // Then away again.
        dw.follow_focus_away_from(4, guard);
        assert_eq!(dw.0, Some(4));
    }
}
