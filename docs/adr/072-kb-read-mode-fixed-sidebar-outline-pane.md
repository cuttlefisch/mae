# ADR-072: KB read mode — fixed-width diagram sidebar + outline/history pane

**Status:** Proposed.
**Depends on:** ADR-071 (the redesigned wedge/petal chord diagram — this ADR's "polished
reading experience" claim rests on that redesign existing; KB read mode packages it, it
doesn't require it in the strict sense — the layout works with the unredesigned diagram
too — but shipping order should follow ADR-071).
**Relates to:** ADR-016 (artifact/BufferView interaction model — this ADR's new
outline/history pane is a new `BufferView`-adjacent concept in the same spirit),
concept:display-policy.
**Tracking:** tracker issue TBD (see ADR-070's header).

## Context

MAE already has, separately, every *behavioral* piece a "KB read mode" needs:

- A live KB-content reading buffer, `BufferKind::Kb`/`KbView`
  (`crates/core/src/kb_view.rs`) — doc-commented as deliberately live ("rendering pulls
  the node body from the KB on each frame; `KbView` stores only pointers, never body
  text"), with browser-style `back_stack`/`forward_stack` navigation history already
  built in.
- Content→diagram auto-follow: `GraphView.follow_current`
  (option `kb_graph_follow_current_node`, default `true`) drives
  `Editor::maybe_follow_kb_graph_view` (`crates/core/src/editor/graph_view_ops.rs`),
  called after every dispatched command — it re-centers the graph in place (never
  re-splits) whenever the active KB buffer's node changes. This already IS "the diagram
  auto-updates as you read."
- Diagram→content click-navigate: `GraphView.companion_window`/`DrivenWindow`
  (`crates/core/src/driven_window.rs`) plus `navigate_companion_window_to_node`
  (`graph_view_ops.rs`) — clicking a graph node writes the target node directly into a
  captured companion window's buffer, bypassing normal split/reuse policy.

What's missing is **packaging**, not follow-logic: a user today must hand-open and
hand-arrange a `BufferKind::Kb` window and a `BufferKind::Graph` window as two separate
actions with no fixed sidebar sizing, and there is no third pane at all for an outline/
navigation-history view (the direct visual reference project's `#history-panel`). Two
concrete architectural gaps block this:

1. **No fixed-pixel-width split exists.** `DisplayAction::ReuseOrSplit`
   (`crates/core/src/display_policy.rs:21-24`) carries only `ratio: f32` — a proportion of
   the current window rect, re-flowing on every resize. `BufferKind::Kb`'s own current
   default (`display_policy.rs:101`) is `ReuseOrSplit { Vertical, ratio: 0.5 }`;
   `BufferKind::FileTree`'s `ratio: 0.2` is the closest existing analog to a narrow
   sidebar, but it's still a percentage, not a fixed pixel/column width.
2. **No outline/history third pane exists among current `BufferView` variants**
   (`crates/core/src/buffer_view.rs`) — this is genuinely new UI surface, not a
   repackaging of something that already exists.

## Decision

### D1 — `SplitSizing::FixedPixels`

Extend `DisplayAction::ReuseOrSplit` with a sizing mode (additive to the enum — every
existing `ReuseOrSplit { ratio, .. }` construction keeps compiling and behaving
identically):

```rust
pub enum SplitSizing {
    Ratio(f32),
    FixedPixels(u16),
}
ReuseOrSplit { direction: SplitDirection, sizing: SplitSizing }
```

The consumer of this policy value, `crates/core/src/window.rs`'s `LayoutNode::Split`
geometry resolution, recomputes an *effective* `ratio` from `FixedPixels(px) / current
_rect_width` on every layout pass (never stores a stale precomputed ratio) — so a fixed
sidebar genuinely holds its pixel width across a window resize, unlike a ratio-based split.
Below MAE's existing `MIN_WINDOW_WIDTH` floor, the sidebar clamps to that minimum rather
than producing a negative/zero-width pane, matching the window manager's existing
resize-clamping convention.

### D2 — Outline/history pane (new)

A new pane, populated from `KbView.back_stack`/`forward_stack`
(`crates/core/src/kb_view.rs`) — the exact same data the reading pane's own
back/forward navigation already tracks, surfaced as a visible, clickable list rather than
invisible history state. This is scoped narrowly: a read-only list of recently-visited
node titles/ids for this reading session, click-to-navigate (reusing the reading pane's
existing `navigate_to`), no independent editing/interaction model of its own. The exact
`BufferView`/rendering representation (a lightweight variant vs. reusing an existing
list-rendering primitive) is left to implementation — this ADR fixes the *data source*
(session `back_stack`/`forward_stack`, not a new independently-maintained history) and the
*scope* (read-only navigation list), not the rendering mechanics.

### D3 — KB read mode entry point

A single new command/entry point (name TBD at implementation time, e.g. `kb-read-mode`)
that, in one action:

1. Opens/reuses a `BufferKind::Kb` buffer as the dominant pane (existing
   `open_help_at`/`ensure_kb_buffer_idx` machinery, unchanged).
2. Opens/reuses a `BufferKind::Graph` buffer in a `FixedPixels`-sized sidebar (D1) —
   default width chosen to match the visual reference's ~280-300px feel, exposed as a new
   option rather than hardcoded (per CLAUDE.md principle #7 — no hardcoding).
3. Opens the new outline/history pane (D2) stacked in the same sidebar, below the diagram.
4. Relies entirely on the **already-existing** `follow_current`/`companion_window`
   mechanisms for the live coupling in both directions — this phase adds zero new
   follow-logic, only the coupled-layout packaging and the new pane.

## Consequences

- `SplitSizing` is additive, not a breaking change to any existing `ReuseOrSplit` caller.
- The outline/history pane is a genuinely new UI surface (per the user's explicit decision
  to include it in v1 rather than defer it) — it is the single largest net-new piece of
  UI in this ADR relative to Workstream 2's other phases, which are otherwise
  overwhelmingly reuse of existing mechanisms.

## Verification

An integration-level test opening KB read mode and asserting: the sidebar holds a fixed
pixel width across a simulated resize (comparative test: same resize sequence, `Ratio` vs.
`FixedPixels`, assert the fixed one holds width while the ratio one doesn't); a degenerate
case where the window is narrower than the configured fixed width clamps gracefully
(no negative/zero-width pane); navigating a KB link updates the diagram's selection
(regression coverage for the existing `follow_current` mechanism, exercised through the
new entry point specifically); clicking a diagram wedge navigates the reading pane
(regression coverage for `companion_window`, same); the outline/history pane reflects
`back_stack`/`forward_stack` state accurately after a navigation sequence. Manual
verification: open KB read mode against a real KB, confirm the three-pane layout renders
as expected and all three panes stay in sync while reading/clicking/navigating.
