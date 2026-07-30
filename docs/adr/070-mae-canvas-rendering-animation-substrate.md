# ADR-070: mae-canvas rendering/animation substrate hardening (Wedge primitive, generalized value tween, angular hit-testing)

**Status:** Proposed.
**Supersedes:** ADR-069 (in part — the "no new `VisualElement` variant, no GUI/Skia
backend changes" constraint stated at `crates/core/src/graph_view.rs:2754-2757` no longer
holds once this ADR lands; ADR-069's own edge-bundling scope is otherwise unaffected).
**Depends on:** ADR-014 (editor/daemon workspace + crate boundaries — this substrate lives
in `mae-core`/`mae-canvas`, shared by `crates/gui` today and by ADR-064's future second
binary).
**Relates to:** ADR-064 (second native MAE frontend for visual-design workflows —
explicitly scoped as a second *binary* linking `mae-core`/`mae-canvas` as a library, not a
`BufferKind` bolted onto the existing GUI; this ADR's whole reason to build genuinely
reusable substrate rather than a `graph_view.rs`-local hack is to serve that future
consumer too), ADR-068 (multi-KB DOI-based LOD rendering — the render-tiering this
substrate's new primitives must not break).
**Tracking:** tracker issue TBD (KB graph-view UX overhaul epic — see ADR-071/072/073/074
for the sibling ADRs this one is foundational to).

## Context

MAE's native KB graph view (`SPC h g`, `crates/core/src/graph_view.rs` + the `mae-canvas`
crate) renders every node as a `VisualElement::Circle` regardless of layout algorithm
(force or chord). A downstream sister project (`bilingual-kb-export`, a standalone
browser/SVG-based KB chord-diagram exporter) redesigned its own chord-diagram nodes as
annular-sector wedges with rounded "petal" corners, real hover/neighbor/selected growth
animation, and angle-aware hit-testing — a substantially more polished and legible
diagram at scale, and the direct visual-design reference for this initiative (see ADR-071).

Porting that design natively means MAE's rendering layer needs three things it does not
have today:

1. **A wedge/annular-sector shape primitive.** `VisualElement`
   (`crates/core/src/visual_buffer.rs`) has exactly `Rect | Line | Circle | Text | Curve`
   (the latter a quadratic Bezier via `skia_safe::PathBuilder` + `.quad_to()`). No
   arc/wedge/polygon-path variant exists, and no `arc_to`/`conic_to`/`add_arc` Skia call
   exists anywhere in `crates/gui/src` or `crates/canvas/src` — this would be the first
   real arc-path usage in the codebase, though `skia_safe::Path` already exposes what's
   needed.
2. **A generalized scalar animation (tween).** Exactly one tween exists today,
   `GraphColorTween` (`crates/core/src/graph_view.rs:312-335`) — a hex-color-only lerp
   with `ease_out_cubic`, ticked once per event-loop iteration by
   `Editor::tick_graph_color_tweens`, deliberately kept off the heavier
   physics-animation/IPC-shaped plumbing used for force-layout settling ("a trivial color
   lerp has no business going through that IPC-shaped plumbing" — same file, doc comment
   on the tick function). Hover/neighbor wedge growth needs the identical
   lightweight-main-thread-tick shape, but interpolating a radius (`f32`), not a color.
   Two independently-maintained tween types would immediately violate CLAUDE.md principle
   #8 (shared computation, not duplicated).
3. **Angle-aware hit-testing.** `crates/canvas/src/interaction.rs::hit_test` is a plain
   circle-distance test against a per-node radius array. A wedge is not circularly
   symmetric — a point can be within a wedge's radius range but outside its angular span,
   or vice versa near a neighboring wedge — so hit-testing needs a second dimension
   (angle), which does not exist today.

Bilingual-kb-export's own JS (`crates/export/src/html_graph.rs`, functions `arcPath` and
`refreshWedgeGrowth`) provides directly portable geometry: a clamped corner-radius
annular-sector path construction, and a growth model that adds a bonus **only to the
outer radius** (inner radius and angular span never change) so a wedge always grows
radially outward, never sideways — a real, hard-won fix for a cross-browser bug their
project hit when it tried `transform: scale()` instead (Firefox ignored it for
hit-testing; Chromium grew the wedge sideways, since a wedge's bounding box isn't centered
the way a circle's is). Skia has no equivalent to the browser's automatic
attribute-interpolation, so MAE needs an explicit, real per-frame tween driving this
geometry recomputation — this is the one piece of the design needing genuinely new
engineering, not a port.

## Decision

Three independently-shippable additions to `mae-core`/`mae-canvas`, all Day-1-parallel
(no dependency on each other):

### D1 — `VisualElement::Wedge`

A new annular-sector path variant in `crates/core/src/visual_buffer.rs`:

```rust
Wedge {
    cx: f32,
    cy: f32,
    inner_r: f32,
    outer_r: f32,
    start_angle: f32,   // radians
    sweep_angle: f32,   // radians
    corner_radius: f32, // world units; clamped at draw time, see below
    fill: Option<String>,
    stroke: Option<String>,
},
```

Rendered via a new helper in `crates/gui/src/canvas.rs` (mirroring the existing
`PathBuilder`-based idiom already used by `draw_wavy_underline_at_pixel` and the
`RRect`-based `draw_pixel_rrect`), wired into the render-dispatch match in
`crates/gui/src/lib.rs` (~line 1491 onward, alongside the existing `Circle`/`Curve` arms).
The corner-radius clamp is ported directly from `arcPath`'s clamped math (own values in
world units, not CSS pixels):

```
cr = max(0, min(
    corner_radius,
    (outer_r - inner_r) / 2 - epsilon,
    (sweep_angle * outer_r) / 2 - epsilon,
    (sweep_angle * max(inner_r, 1.0)) / 2 - epsilon,
))
```

so a corner radius can never over-round into self-intersection, regardless of caller
input — this clamp is the load-bearing degenerate-input guard and must never panic or
emit NaN/negative geometry.

### D2 — Generalized `ValueTween<T>`

A new `crates/core/src/tween.rs` module promoting `GraphColorTween`'s shape
(`started_at: Instant`, `duration: Duration`, `ease_out_cubic`, `is_complete()`) into a
small trait + generic:

```rust
trait Lerpable: Clone {
    fn lerp(&self, other: &Self, t: f32) -> Self;
}
// impl Lerpable for String (hex color, existing behavior)
// impl Lerpable for f32   (scalar — new, for radius/growth)

struct ValueTween<T: Lerpable> {
    from: T,
    to: T,
    started_at: Instant,
    duration: Duration,
}
```

`GraphColorTween` becomes a type alias (`ValueTween<String>`) so every existing call site
keeps compiling unchanged — this is a behavior-preserving generalization, not a rename.
The single main-thread tick loop (`Editor::tick_graph_color_tweens`, generalized to tick
both color and scalar tweens) keeps the existing `WaitUntil`-gating behavior: the 60fps
redraw cadence stays active only while at least one tween of either kind is running.

### D3 — Angular-sector hit-testing

A new function alongside the existing `hit_test` in `crates/canvas/src/interaction.rs`,
sharing a `WedgeGeom` struct with D1 so the geometry that's drawn and the geometry that's
clickable cannot independently drift apart:

```rust
fn hit_test_wedge(scene_x: f32, scene_y: f32, wedges: &[WedgeGeom]) -> Option<usize>
```

testing `(radius ∈ [inner_r, outer_r]) && (angle ∈ [start_angle, start_angle +
sweep_angle])` per candidate (angle normalized mod 2π before comparison), topmost-wins,
missing-entry-fails-closed — mirroring `hit_test`'s own existing test-naming/behavior
conventions exactly (`interaction.rs`'s existing suite is the direct template).

This is scoped as a **second, parallel function**, not a generalization of `hit_test`
itself into a shape-enum-dispatched single function — Force-mode nodes stay circles, and
consolidating the two call sites is a larger, separate refactor better done once both
shapes are independently proven. A future cleanup ADR may revisit this.

## New options (this ADR's plumbing; consumed starting in ADR-071)

Following the existing `opt!(...)` pattern in `crates/core/src/options.rs` exactly
(alongside the existing `kb_graph_color_tween_enabled`/`kb_graph_color_tween_duration_ms`
precedent):

- `kb_graph_wedge_enabled` (bool, default `false` — per CLAUDE.md principle #12, a new
  potentially-costly rendering feature ships opt-in until proven, same posture as
  `kb_graph_edge_taper_enabled`/`kb_graph_multi_kb_full_corpus`)
- `kb_graph_wedge_corner_radius_fraction` (float)
- `kb_graph_wedge_hover_growth_factor` (float)
- `kb_graph_wedge_neighbor_growth_fraction` (float)
- `kb_graph_wedge_gap_radians` (float, default `0.0` — flush; separation is rounding-only,
  matching the sister project's own default)

## Consequences

- First real Skia arc-path usage in the codebase — a deliberate, justified addition
  (reusable substrate for both the existing GUI and ADR-064's future second frontend), not
  an accidental scope-creep of the graph-view feature.
- `ADR-069`'s edge-bundling scope is otherwise unaffected: it still reuses
  `Line`/`Curve` for bundled edge rendering, independent of node shape.
- Two parallel hit-test functions (circle, wedge) is accepted short-term duplication in
  exchange for not risking a larger, riskier `hit_test` generalization before either shape
  is proven in production.

## Verification

Per CLAUDE.md principle #14 (adversarial, not confirmation): comparative tests (Wedge vs.
an equivalent-area Circle, assert bbox/area within tolerance — not a hand-picked single
case), degenerate-input guards (zero sweep angle, `inner_r > outer_r`, a corner radius
exceeding the self-intersection bound — must clamp, never panic/NaN/negative-radius),
determinism (identical wedge params render byte-identical output run twice). Tween tests
must use an injected/frozen clock, never real `Instant::now()`, so they are not flaky
against wall-clock timing. Hit-test tests mirror `hit_test`'s own existing test names 1:1
(`hit_test_wedge_inside_node`, `hit_test_wedge_outside_node`,
`hit_test_wedge_respects_angular_bounds`, `hit_test_wedge_missing_radius_entry_fails_closed`,
`hit_test_wedge_topmost_wins`).
