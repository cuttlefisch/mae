//! Shared idle-dispatch mechanism (Part B of the KB-graph-view plan).
//!
//! No "pause N ms, then act" mechanism existed before this file: the GUI's
//! fixed 100ms `MaeEvent::IdleTick` only drove `Editor::idle_work()`
//! (background housekeeping — reparse, swap writes, git-diff polling —
//! explicitly documented as never triggering a redraw), and the TUI had no
//! idle poller at all. `Editor::on_idle_tick` is the single entry point both
//! backends call (GUI: alongside the existing `idle_work()` call in the
//! `IdleTick` handler; TUI: from the main loop's timed event-poll) once per
//! idle-tick interval, passing how long the editor has been idle (no input)
//! in milliseconds.
//!
//! Each feature below is a small, independent block reading its own
//! `OptionRegistry`-sourced delay and its own trigger condition. A generic
//! pub/sub registry is premature for two consumers — this plain shared
//! function with guarded blocks is the right-sized shared computation
//! (principle #8).

use super::Editor;

impl Editor {
    /// Called periodically (roughly every idle-tick interval) with the
    /// number of milliseconds since the editor last saw input. Dispatches to
    /// each idle-triggered feature once its own configured delay has
    /// elapsed. Returns `true` if any feature needs the caller to force a
    /// redraw this tick (idle work otherwise never triggers one).
    pub fn on_idle_tick(&mut self, idle_ms: u64) -> bool {
        let mut needs_redraw = false;
        if idle_ms >= self.which_key_idle_delay {
            needs_redraw |= self.maybe_show_which_key_popup();
        }
        if idle_ms >= self.kb_preview_idle_delay {
            needs_redraw |= self.maybe_show_kb_preview_popup();
        }
        needs_redraw
    }

    /// Whether the which-key popup should currently be painted for the
    /// active leader transient keypad: `leader_active` AND at least
    /// `which_key_idle_delay` ms have elapsed since it was activated. Pure
    /// and time-based (re-evaluated fresh on every render by
    /// `render_common::overlay::active_overlay`) — with the default delay of
    /// 0ms this is true the instant `leader_active` flips, i.e. immediate,
    /// matching the pre-Part-B behavior exactly. Non-default delays need
    /// `maybe_show_which_key_popup` (below) to force the *first* redraw
    /// after the pure pause, since nothing else changes editor state while
    /// idle.
    pub fn leader_popup_ready(&self) -> bool {
        self.leader_active
            && self.leader_activated_at.is_some_and(|activated_at| {
                activated_at.elapsed().as_millis() as u64 >= self.which_key_idle_delay
            })
    }

    /// Closes ROADMAP #83: force exactly one redraw once `which_key_idle_delay`
    /// has elapsed while the leader keypad is open, so the which-key popup
    /// (gated by `leader_popup_ready`) actually paints. During a pure idle
    /// pause nothing else marks the editor dirty, so without this hook a
    /// nonzero delay would mean the popup never appears. Idempotent per
    /// activation via `which_key_popup_redraw_done` — avoids re-marking a
    /// full redraw on every subsequent idle tick while the popup just sits
    /// on screen unchanged.
    fn maybe_show_which_key_popup(&mut self) -> bool {
        if self.leader_popup_ready() && !self.which_key_popup_redraw_done {
            self.which_key_popup_redraw_done = true;
            self.mark_full_redraw();
            true
        } else {
            false
        }
    }

    /// Part D (KB-link hover preview): once `kb_preview_idle_delay` ms of
    /// idle time has elapsed, show a preview popup if the focused buffer is
    /// KB-kind, `kb_preview_on_hover` is enabled, and the cursor is
    /// currently sitting on a KB link. Delegates the actual lookup/populate
    /// work to `Editor::kb_preview_show_at_cursor` (`kb_preview_ops.rs`,
    /// shared with the manual `kb-preview` command) with `force = false` so
    /// repeated idle ticks over a motionless cursor don't keep re-fetching
    /// KB content or forcing a redraw — mirrors `maybe_show_which_key_popup`
    /// above (force exactly one redraw to reveal a popup that pure idle
    /// time alone doesn't otherwise mark dirty).
    fn maybe_show_kb_preview_popup(&mut self) -> bool {
        if !self.kb_preview_on_hover {
            return false;
        }
        if self.kb_preview_show_at_cursor(false) {
            self.mark_full_redraw();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_key_idle_delay_default_is_zero_and_popup_ready_immediately() {
        let mut ed = Editor::new();
        assert_eq!(ed.which_key_idle_delay, 0, "default must stay immediate");
        ed.set_leader_active(true);
        assert!(
            ed.leader_popup_ready(),
            "0ms delay must be ready the instant leader mode activates"
        );
    }

    #[test]
    fn nonzero_delay_gates_popup_until_elapsed() {
        let mut ed = Editor::new();
        ed.which_key_idle_delay = 50;
        ed.set_leader_active(true);
        assert!(
            !ed.leader_popup_ready(),
            "a fresh activation must not be ready before the delay elapses"
        );
        // Simulate elapsed idle time by backdating the activation instant
        // rather than sleeping — keeps this test fast and deterministic.
        ed.leader_activated_at =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(60));
        assert!(
            ed.leader_popup_ready(),
            "must become ready once elapsed time passes the configured delay"
        );
    }

    #[test]
    fn leader_popup_ready_false_when_leader_inactive() {
        let mut ed = Editor::new();
        ed.which_key_idle_delay = 0;
        assert!(!ed.leader_active);
        assert!(
            !ed.leader_popup_ready(),
            "must never be ready while the leader keypad isn't active"
        );
    }

    #[test]
    fn on_idle_tick_forces_redraw_exactly_once_per_activation() {
        let mut ed = Editor::new();
        ed.which_key_idle_delay = 10;
        ed.set_leader_active(true);
        ed.leader_activated_at =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(20));

        assert!(
            ed.on_idle_tick(20),
            "first tick past the delay must request a redraw"
        );
        assert!(
            !ed.on_idle_tick(20),
            "a second idle tick with nothing changed must not re-request one"
        );

        // Reactivating (e.g. the user cancels and reopens the keypad) resets
        // the one-shot guard for the new activation.
        ed.set_leader_active(false);
        ed.set_leader_active(true);
        ed.leader_activated_at =
            Some(std::time::Instant::now() - std::time::Duration::from_millis(20));
        assert!(
            ed.on_idle_tick(20),
            "a fresh activation must be able to force a redraw again"
        );
    }

    #[test]
    fn on_idle_tick_no_redraw_needed_while_leader_inactive() {
        let mut ed = Editor::new();
        assert!(
            !ed.on_idle_tick(10_000),
            "no leader session, nothing to show"
        );
    }
}
