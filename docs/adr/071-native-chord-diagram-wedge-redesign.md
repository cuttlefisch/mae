# ADR-071: Native chord-diagram wedge/petal redesign (geometry, state layering, options)

**Status:** Proposed.
**Depends on:** ADR-070 (`VisualElement::Wedge`, generalized `ValueTween<T>`,
angular-sector hit-testing — this ADR is the first real consumer of all three).
**Extends:** ADR-068 (multi-KB DOI-based LOD rendering — the render-tier/label-priority
machinery in `flatten_scene_graph_cached`/`compute_label_winners` this redesign's node
emission must keep working under, unchanged).
**Amends:** ADR-069 (force-directed edge bundling — that ADR's edge rendering anchors
each edge at a node's *center point* (`src.x, src.y`, `crates/core/src/graph_view.rs:2672`);
once nodes are wedges rather than circles, edges must anchor at the wedge's *inner-edge*
minus a small inset instead, for Chord mode only — see Decision D3 below. ADR-069's own
edge-bundling algorithm/architecture is otherwise unaffected).
**Relates to:** issue #367 (original chord layout), issue #462 (multi-KB chord view).
**Tracking:** tracker issue TBD (see ADR-070's header for the epic this belongs to).

## Context

MAE's Chord-mode graph view (`kb_graph_layout_algorithm = "chord"`) places each node as a
single point on a ring (`chord_ring_positions`, `crates/canvas/src/kb_graph.rs`) and
renders it as a plain circle. A downstream sister project's own chord-diagram redesign
(the direct visual reference for this work) replaced circular nodes with annular-sector
wedges — "flower petal" styled via rounded corners only, no stroke — and added a real
connected-node ("neighbor of the current selection") highlight, both requested directly
by this initiative. ADR-070 supplies the rendering/animation/hit-testing primitives this
ADR needs to actually build the redesign; this ADR is the design for *using* them.

Two real, hard-won lessons from the sister project's own development are worth respecting
rather than rediscovering the hard way:

- **Wedge thickness must be uniform, not degree-scaled.** An earlier per-node-thickness-
  bonus version of their design caused ~20% bulge-artifact swings on a real 168-node
  export. Node importance is still conveyed via color/growth state, just not thickness.
- **Angular span must never grow past its nominal slot**, even to satisfy a minimum
  hit-target size on hover/selection — "zero overlap is the hard invariant every time...
  lets the hit target degrade gracefully instead" (their own framing). Only the *outer
  radius* grows on hover/neighbor state (ADR-070 D2's tween), never the angular span.

Both lessons are adopted as hard invariants below (gate G4 in the tracker issue).

## Decision

### D1 — Wedge geometry for Chord mode

`chord_ring_positions` (`crates/canvas/src/kb_graph.rs`) is extended to also compute, per
node: an angular slot (`2π / n`, evenly divided — unchanged from today's point placement,
just carrying a real span now instead of implying zero width) and a **uniform** wedge
thickness derived from `kb_graph_node_radius`-equivalent sizing (not
`kb_graph_node_size_by_degree`'s degree scaling, which stays Force-mode-only — the two
layout algorithms are already separate branches with no shared code path to disturb).
`flatten_scene_graph`'s Chord-mode node-emission arm switches from `VisualElement::Circle`
to `VisualElement::Wedge` when `kb_graph_wedge_enabled` is set. `chord_label_placement`'s
existing angle-aware text rotation (already computes a rotation angle + anchor side for
radially-outward-reading labels) is reused entirely unchanged — it already assumed an
angular position, and now has a real wedge shape to sit against instead of a bare point.

### D2 — Neighbor-of-selection + state layering

A new adjacency lookup, computed on-demand from `scene.edges` given `scene.selection`
(not cached as a second piece of state — an on-demand `O(edges)` scan is cheap at the node
counts this diagram targets and cannot drift out of sync with `selection` the way a cached
set could). State precedence, adopted directly from the sister project's design:

| State | Trigger | Effect | Precedence |
|---|---|---|---|
| `hovered` | cursor over a wedge | transient: outer-radius growth (ADR-070 D2 tween) + a subtle lift/shadow effect | wins geometry-growth priority over `neighbor` when both apply to the same node |
| `selected` | current graph selection | standing: recolor fill to the existing "selected" theme color | independent of `neighbor`/`visited` |
| `neighbor` | directly linked to `selected` | standing: recolor fill to a color DIFFERENT from `selected`'s, so "this is current" and "this is connected to current" are never visually confused | applies independently of hover; a neighbor that's also hovered gets `hovered`'s (larger) growth, not its own smaller growth bonus |
| `visited` | previously navigated to in this session | opacity-only inset marker dot at the wedge's mid-angle/mid-radius; hidden on the currently-selected node itself (selected styling already conveys "you are here") | lowest precedence, purely additive, never competes with the above for a color/geometry channel |

`visited` reuses `KbView.back_stack`/`forward_stack` (`crates/core/src/kb_view.rs`) as its
data source directly — no second, independently-maintained history mechanism. The overall
precedence-tier concept mirrors an existing, proven precedent in this same file:
`compute_label_winners`'s tier-0 "selected and/or hovered node(s) first" priority ordering
(`graph_view.rs:2378-2429`) — this redesign extends that same tiering idea with a
`neighbor`/`visited` tier, not a new, unrelated mechanism.

On any selection change, ALL node states are cleared and reapplied from scratch (not a
precise incremental delta) — matching the sister project's own explicit simplicity
tradeoff ("cheap at the node counts this widget targets," their own words) rather than
introducing incremental-update bookkeeping bugs for a cost that isn't measurable in
practice at MAE's real KB sizes.

### D3 — Edge anchor points move to the wedge inner edge (amends ADR-069)

For Chord mode specifically, edge start/end points
(`graph_view.rs:2672,2683-2687`'s `sx1,sy1`/`sx2,sy2`) move from the node's center point
to its wedge's inner-edge radius, minus a small fixed inset — so an edge visually meets
the slice it belongs to, rather than appearing to originate from empty space inside it.
Force mode is unaffected (its nodes stay circles; center-anchored edges are already
correct for a circular node and have no reason to change).

## New options (consumed here; declared as ADR-070's plumbing)

`kb_graph_wedge_enabled`, `kb_graph_wedge_corner_radius_fraction`,
`kb_graph_wedge_hover_growth_factor`, `kb_graph_wedge_neighbor_growth_fraction`,
`kb_graph_wedge_gap_radians` — all defined in ADR-070, all consumed by this ADR's Chord-mode
rendering path. `kb_graph_wedge_enabled` ships `false`; flipping the Chord-mode default
(replacing circles outright) is an explicit later decision, not part of this ADR.

## Consequences

- Chord-mode and Force-mode node rendering genuinely diverge in shape (wedge vs. circle)
  and thickness policy (uniform vs. degree-scaled) — this is intentional, not an
  inconsistency: the two layout algorithms already have separate code paths, and the
  design lessons motivating uniform thickness are chord-ring-specific (a point on a ring
  has neighbors on both sides competing for the same angular budget; a force-directed
  circle does not).
- `hit_test`/`hit_test_wedge` remain two parallel functions per ADR-070 D3's own scoping —
  this ADR does not attempt a shape-generalized single hit-test function.

## Verification

Per CLAUDE.md principle #14: comparative tests (angular span scales with node count,
compared across counts, not a single hand-picked count), the zero-overlap invariant
verified at real node counts (not a 2-3 node toy fixture), hit-test boundary-edge tests
at exactly a wedge's start/end angle (not just its center), a selection-change test
asserting exactly the edge-connected neighbors of the new selection get the `neighbor`
recolor (and no others), a regression test that hovering a non-selected node's neighbor
doesn't corrupt the selected node's own state, and a determinism test (identical scene +
selection sequence renders byte-identical twice, with a frozen clock for any tween
involved per ADR-070's own tween-testing requirement). Manual verification: `SPC h g` in a
running `mae --gui` build, hover/select/click through a real multi-node KB, visually
confirm growth is radial (not sideways) and neighbor/selected colors are distinguishable.
