//! Native KB graph view state (`BufferKind::Graph`) — Part C Phase 1 of the
//! KB-graph-view architecture plan.
//!
//! Mirrors `debug_view.rs`'s relationship to `BufferKind::Debug`: the panel
//! is a read-only buffer whose visible content (a flattened
//! `VisualBuffer`) is derived from structured state (`GraphView`) populated
//! by `crate::editor::graph_view_ops`. `GraphView.scene` is the
//! `mae-canvas` scene graph (nodes/edges/viewport/selection); layout
//! (`ForceLayout`) is computed off the main thread by the `mae` binary's
//! `graph_layout_bridge` and applied back via
//! `Editor::apply_graph_layout_result` — this module never runs the O(n^2)
//! force-layout pass itself.
//!
//! @ai-caution: [architecture-debt] At 2,848 lines, well over the 800-line
//! ceiling — `GraphView`'s scene graph, viewport, and flattening logic grew
//! as one file across the KB graph view feature's build-out. Not split
//! (design work, not attempted this pass; round-5 tech-debt pass, 2026-07).
//! Tracked in `.claude/commands/mae-audit.md`'s "Known exceptions" and
//! `ROADMAP.md`'s "Architecture Debt" section — re-verify the line count
//! each audit pass rather than trusting this comment's number to stay current.

use crate::driven_window::DrivenWindow;
use crate::editor::Editor;
use crate::visual_buffer::{VisualBuffer, VisualElement};
use crate::window::WindowId;
use std::collections::HashMap;

/// View state for a `BufferKind::Graph` buffer.
///
/// @ai-caution: [window-lifecycle] Every `HashMap<WindowId, _>` field below
/// (`viewports`, `rendered`, `render_epoch`) MUST be pruned in
/// `Editor::prune_closed_window_graph_state` (`crates/core/src/editor/
/// graph_view_ops.rs`) when a window closes, or it leaks for the session's
/// lifetime. This has already gone wrong twice: `render_epoch`'s prune was
/// omitted from the commit that introduced per-window isolation (#321,
/// `74eec5eb`) and had to be added the next day (`985ee53f`); a later,
/// unrelated OOM-crash fix then re-added a second, independent copy of the
/// same prune elsewhere before the two were unified into the one canonical
/// call site referenced above. Add a new per-window map here → add it to
/// that one function's retain block. Don't add a second prune site.
#[derive(Debug)]
pub struct GraphView {
    /// KB node id the graph is currently centered on (the BFS/subgraph
    /// seed). `None` before the first open.
    pub center_node: Option<String>,
    /// Hop radius passed to `SubgraphSpec::max_depth`.
    pub depth: usize,
    /// Which KB instance owns `center_node`: `None` = primary,
    /// `Some(uuid)` = a federated instance (see `Editor::kb_owner_of`).
    pub kb_instance: Option<String>,
    /// Phase 2 (not wired yet): whether the graph should re-center itself
    /// when the human/AI navigates to a different KB node elsewhere.
    /// Snapshotted from `kb_graph_follow_current_node` on open.
    pub follow_current: bool,
    /// The `mae-canvas` scene graph — nodes, edges, selection, hover. This
    /// is buffer-level/shared topology, legitimately the SAME for every
    /// window showing this graph buffer (see `viewports`/`rendered` below
    /// for the per-window state a second window needs its own copy of).
    pub scene: mae_canvas::scene::SceneGraph,
    /// Per-window pan/zoom/pixel-size (issue #321). A graph buffer can be
    /// shown in more than one window (`:split-vertical` while it's focused
    /// — `WindowManager` has no one-window-per-buffer-kind guard), and each
    /// window's view into the shared `scene` above is independent, the same
    /// way `Window.saved_view_states` already treats scroll/cursor as
    /// per-window rather than per-buffer. Populated lazily (via
    /// `Default::default()`) the first time a window is reflattened;
    /// pruned in `Editor::sync_open_graph_viewports` once a window closes.
    pub viewports: HashMap<WindowId, mae_canvas::scene::Viewport>,
    /// "The window this actor is driving" (Part A `DrivenWindow`), captured
    /// reactively via `follow_focus_away_from` every time editor focus
    /// changes to a window other than the graph's own — see
    /// `Editor::capture_graph_companion_focus`. Used by
    /// `kb-graph-view-select-current` to navigate the *other* (previously
    /// focused) window to the selected node's KB buffer, bypassing
    /// `display_buffer`'s reuse-or-split policy.
    pub companion_window: DrivenWindow,
    /// `scene` flattened into `VisualElement`s for the GUI's
    /// `render_visual_buffer` pipeline (`flatten_scene_graph`), one entry
    /// per window currently showing this buffer (issue #321 — flattening
    /// bakes a specific window's `Viewport` into absolute pixel
    /// coordinates, so the result can't be shared across windows at
    /// different sizes/zoom levels). Rebuilt by `graph_view_ops.rs` on
    /// every open/refresh/navigate/layout-applied/zoom/resize so the GUI
    /// render path never needs to know about `SceneGraph` at all — it just
    /// draws the current window's entry exactly like a `BufferKind::Visual`
    /// buffer.
    pub rendered: HashMap<WindowId, VisualBuffer>,
    /// Part C Phase 3 (`kb_graph_animate`): true while this graph's layout
    /// is still settling — i.e. `ForceLayout::step`'s per-tick settlement
    /// signal (max node displacement) has not yet dropped below
    /// `ANIMATION_SETTLE_EPSILON`. Always `false` when `kb_graph_animate` is
    /// off, in which case `populate_graph_buffer` never sets it and the
    /// one-shot `ForceLayout::run` path (Phase 1/2) behaves exactly as
    /// before Phase 3 existed. Read by `Editor::has_active_graph_animation`,
    /// which `gui_app.rs`'s unified dirty/`ControlFlow::WaitUntil` loop
    /// condition consults (mirroring the scroll-inertia `any_inertia` term)
    /// to decide whether to keep the 60fps wake-up cadence alive.
    pub animating: bool,
    /// Cooling-schedule temperature carried across animation ticks —
    /// mirrors `ForceLayout::run`'s per-iteration linear cooling, but
    /// persisted here (rather than as a loop-local variable) because each
    /// Phase 3 tick is now a separate background round-trip
    /// (`graph_layout_bridge`) instead of one in-process loop. Reset to
    /// `ANIMATION_INITIAL_TEMPERATURE` on every fresh (re)populate; not
    /// meaningful when `animating` is false.
    pub anim_temperature: f64,
    /// The `LayoutConfig` (kind-clustering strength, etc.) computed once by
    /// `populate_graph_buffer` and cached here — every subsequent
    /// `GraphLayoutIntent` this `GraphView` queues (including Phase 3's
    /// per-tick animation requeue in `apply_graph_layout_result`) reads
    /// this cached value rather than rebuilding one, so a background layout
    /// tick can never silently drop back to `LayoutConfig::default()` after
    /// the first request.
    pub layout_config: mae_canvas::layout::LayoutConfig,
    /// Edge count touching each node (parallel to `scene.nodes`), computed
    /// ONCE by `populate_graph_buffer` right after `scene`'s topology is
    /// built — see `node_degrees`'s doc comment for why this is cached
    /// rather than recomputed per-frame/per-hover. Read by both
    /// `flatten_scene_graph` (render radius) and `graph_view_ops.rs`'s
    /// `graph_scene_hit_radii` (hit-test radius), so the two can never
    /// disagree about a node's size.
    pub node_degrees: Vec<u32>,
    /// Asymmetric hover/selection color tween: only the newly-hovered/
    /// selected node tweens INTO its highlight color; the previously
    /// highlighted node snaps back instantly (no fade-out) — one slot, not
    /// N concurrent tweens. Started by `kb_graph_view_hover_at`/the
    /// selection path when the target changes; cleared once
    /// `GraphColorTween::is_complete()`. Ticked by a separate,
    /// main-thread-only mechanism (`Editor::tick_graph_color_tweens`) —
    /// deliberately NOT `animating`/`anim_temperature` above, which is the
    /// off-thread force-layout settling schedule; a trivial color lerp has
    /// no business going through that IPC-shaped plumbing.
    pub color_tween: Option<GraphColorTween>,
    /// Per-window monotonic counter, bumped every time
    /// `Editor::graph_view_reflatten_window` refreshes that window's
    /// `rendered` entry (pan/zoom/hover/selection/tween tick/layout
    /// change — anything that actually changes what's drawn). The GUI's
    /// per-window render cache (`crates/gui/src/lib.rs::WindowRenderCache`)
    /// keys on this ALONGSIDE the buffer's rope-based `generation` — the
    /// rope never changes for a Graph buffer (it has no text content), so
    /// `generation` alone can't tell the cache a pan/zoom/hover genuinely
    /// changed the picture; this counter is what does. Read-only from the
    /// GUI crate's perspective (only `graph_view_reflatten_window` writes
    /// it) — mirrors how `buf.generation` already works for text buffers,
    /// just for the graph's own state instead of rope edits.
    pub render_epoch: HashMap<WindowId, u64>,
    /// How many nodes `extract_subgraph` hid past `kb_graph_node_count_cap`
    /// on the last populate — `0` when the cap wasn't hit. Set by
    /// `Editor::populate_graph_buffer` from `SubgraphResult::hidden_node_count`.
    /// Surfaced via `describe_state`/the `kb_graph_view_open` MCP response
    /// and a one-shot status message so a truncated view is never silently
    /// mistaken for the whole neighborhood.
    pub hidden_node_count: usize,
}

/// An in-flight color transition for one node — see `GraphView.color_tween`.
#[derive(Debug, Clone)]
pub struct GraphColorTween {
    pub node_index: usize,
    pub from_hex: String,
    pub to_hex: String,
    pub started_at: std::time::Instant,
    pub duration: std::time::Duration,
}

impl GraphColorTween {
    /// The eased, interpolated color at the current instant.
    pub fn current_color(&self) -> String {
        let elapsed = self.started_at.elapsed().as_secs_f32();
        let dur = self.duration.as_secs_f32().max(0.0001);
        lerp_hex(&self.from_hex, &self.to_hex, ease_out_cubic(elapsed / dur))
    }

    pub fn is_complete(&self) -> bool {
        self.started_at.elapsed() >= self.duration
    }
}

/// Ease-out cubic: `1 - (1-t)^3`, `t` clamped to `[0, 1]` — starts fast,
/// settles gently, the standard "pop in" curve for a UI highlight.
fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// Parse a `"#rrggbb"` hex string into `(r, g, b)` byte components.
/// `None` for anything malformed — callers fall back to the raw `to`
/// color verbatim rather than panicking on a bad hex string.
fn parse_hex_rgb(hex: &str) -> Option<(u8, u8, u8)> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some((r, g, b))
}

/// Linearly interpolate between two `"#rrggbb"` hex colors at `t` (already
/// eased, `[0, 1]`). Falls back to `to` verbatim if either color fails to
/// parse — a malformed color never panics, it just snaps instead of
/// animating.
fn lerp_hex(from: &str, to: &str, t: f32) -> String {
    let t = t.clamp(0.0, 1.0);
    match (parse_hex_rgb(from), parse_hex_rgb(to)) {
        (Some((fr, fg, fb)), Some((tr, tg, tb))) => {
            let lerp_byte =
                |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
            format!(
                "#{:02x}{:02x}{:02x}",
                lerp_byte(fr, tr),
                lerp_byte(fg, tg),
                lerp_byte(fb, tb)
            )
        }
        _ => to.to_string(),
    }
}

/// Convert `(r, g, b)` byte components to `(hue, saturation, lightness)` —
/// hue in `[0, 360)` degrees, saturation/lightness in `[0, 1]`. Standard
/// HSL colorspace conversion, used by `cap_saturation` to mute an
/// over-vivid theme color while preserving its hue/lightness.
fn rgb_to_hsl(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.0) * 60.0
    } else if max == g {
        ((b - r) / d + 2.0) * 60.0
    } else {
        ((r - g) / d + 4.0) * 60.0
    };
    (h, s, l)
}

/// Inverse of `rgb_to_hsl`.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    if s <= 0.0 {
        let v = (l * 255.0).round().clamp(0.0, 255.0) as u8;
        return (v, v, v);
    }
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = (h / 60.0).rem_euclid(6.0);
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    let to_byte = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    (to_byte(r1), to_byte(g1), to_byte(b1))
}

/// Cap a `"#rrggbb"` hex color's HSL saturation at `max_saturation` (`[0,
/// 1]`), preserving hue and lightness. The mechanism behind
/// `kb_graph_node_saturation_cap`: a fully-saturated ("neon") theme color
/// reads as visually harsh/fatiguing when many nodes of that kind render
/// simultaneously in a dense graph. Muting saturation while keeping hue
/// (the primary categorical differentiator — Okabe-Ito/Wong-style
/// colorblind-safe qualitative palettes are deliberately moderate, not
/// fully-saturated) and lightness (so the WCAG text-contrast guarantee
/// computed downstream, against the ORIGINAL background, stays
/// meaningful) preserves kind differentiation while reducing fatigue.
/// A color already at or below the cap passes through byte-for-byte
/// unchanged (never brightens/re-saturates). Malformed input passes
/// through unchanged rather than panicking.
fn cap_saturation(hex: &str, max_saturation: f32) -> String {
    let Some((r, g, b)) = parse_hex_rgb(hex) else {
        return hex.to_string();
    };
    let max_saturation = max_saturation.clamp(0.0, 1.0);
    let (h, s, l) = rgb_to_hsl(r, g, b);
    if s <= max_saturation {
        return hex.to_string();
    }
    let (nr, ng, nb) = hsl_to_rgb(h, max_saturation, l);
    format!("#{nr:02x}{ng:02x}{nb:02x}")
}

/// WCAG 2.x relative luminance of a `"#rrggbb"` hex color, `[0, 1]`. `0.0`
/// for malformed input (reads as pure black, the same as `#000000`).
fn relative_luminance(hex: &str) -> f64 {
    let Some((r, g, b)) = parse_hex_rgb(hex) else {
        return 0.0;
    };
    let channel = |c: u8| -> f64 {
        let c = c as f64 / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

/// WCAG 2.x contrast ratio between two `"#rrggbb"` hex colors, `[1, 21]`
/// (1 = identical luminance, 21 = pure black against pure white).
fn contrast_ratio(hex_a: &str, hex_b: &str) -> f64 {
    let (la, lb) = (relative_luminance(hex_a), relative_luminance(hex_b));
    let (lighter, darker) = if la >= lb { (la, lb) } else { (lb, la) };
    (lighter + 0.05) / (darker + 0.05)
}

/// If `fg`'s contrast against `bg` already meets `min_ratio` (WCAG AA for
/// normal text is `4.5`), return `fg` unchanged — preserving its hue
/// (e.g. a node's kind color, or the boundary-edge color) is the point,
/// this is a MINIMUM guarantee, not a forced recolor. Otherwise, push `fg`
/// toward whichever of pure white/black yields the higher contrast against
/// `bg` (i.e. the readable direction for that background), stepping in
/// `0.1` increments and stopping at the first mix that clears the
/// threshold — the smallest nudge that becomes legible, not a jump
/// straight to the endpoint. Falls back to the endpoint itself if even a
/// full mix doesn't clear the threshold (only possible for a `min_ratio`
/// above what black/white against `bg` can achieve, i.e. `bg` itself is
/// near-18%-gray and `min_ratio` is set unreasonably high).
fn ensure_min_contrast(fg: &str, bg: &str, min_ratio: f64) -> String {
    if contrast_ratio(fg, bg) >= min_ratio {
        return fg.to_string();
    }
    let toward_white = contrast_ratio("#ffffff", bg) >= contrast_ratio("#000000", bg);
    let endpoint = if toward_white { "#ffffff" } else { "#000000" };
    for step in 1..=10 {
        let t = step as f32 / 10.0;
        let candidate = lerp_hex(fg, endpoint, t);
        if contrast_ratio(&candidate, bg) >= min_ratio {
            return candidate;
        }
    }
    endpoint.to_string()
}

/// WCAG AA minimum contrast ratio for normal-sized text.
const WCAG_AA_TEXT_CONTRAST: f64 = 4.5;

impl GraphView {
    /// Structured, read-only snapshot of everything a human or the AI
    /// agent might want to introspect about the currently-rendered graph:
    /// which node is hovered/selected, which nodes are rendered at all
    /// (the current ego-network), and which edges connect them. A PURE
    /// method — needs nothing from `Editor`, every field it resolves
    /// already lives on `GraphView`/`SceneGraph` — independently
    /// unit-testable exactly like `flatten_scene_graph`. Resolves
    /// scene-array indices (`selection`, `hovered`, edge `source`/
    /// `target`) to real KB node ids via `scene.nodes[i].id`, so the
    /// introspection boundary exposes semantic identity, not internal
    /// array positions. This is the single shared data source for both
    /// the `kb_graph_view_state` MCP tool (serializes directly) and the
    /// `(kb-graph-view-state)` Scheme primitive (hand-converted to a
    /// `Value`, snapshot-injected — see `crates/scheme/src/runtime/
    /// state_sync_inject_kb.rs`) — parity at the data-shape level.
    pub fn describe_state(&self) -> GraphViewState {
        let node_id = |i: usize| self.scene.nodes.get(i).map(|n| n.id.clone());
        GraphViewState {
            center_node: self.center_node.clone(),
            depth: self.depth,
            kb_instance: self.kb_instance.clone(),
            follow_current: self.follow_current,
            hidden_node_count: self.hidden_node_count,
            selected_node: self.scene.selection.and_then(node_id),
            hovered_node: self.scene.hovered.and_then(node_id),
            nodes: self
                .scene
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| GraphViewNodeState {
                    id: n.id.clone(),
                    title: n.label.clone(),
                    kind: n.kind,
                    x: n.x,
                    y: n.y,
                    pinned: n.pinned,
                    selected: self.scene.selection == Some(i),
                    hovered: self.scene.hovered == Some(i),
                })
                .collect(),
            edges: self
                .scene
                .edges
                .iter()
                .filter_map(|e| {
                    Some(GraphViewEdgeState {
                        source_id: node_id(e.source)?,
                        target_id: node_id(e.target)?,
                        boundary: e.style.dashed,
                        label: e.label.clone(),
                    })
                })
                .collect(),
        }
    }
}

/// Structured introspection snapshot of a `GraphView` — see
/// `GraphView::describe_state`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphViewState {
    pub center_node: Option<String>,
    pub depth: usize,
    pub kb_instance: Option<String>,
    pub follow_current: bool,
    pub hidden_node_count: usize,
    pub selected_node: Option<String>,
    pub hovered_node: Option<String>,
    pub nodes: Vec<GraphViewNodeState>,
    pub edges: Vec<GraphViewEdgeState>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphViewNodeState {
    pub id: String,
    pub title: String,
    pub kind: mae_canvas::scene::NodeKind,
    pub x: f64,
    pub y: f64,
    pub pinned: bool,
    pub selected: bool,
    pub hovered: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GraphViewEdgeState {
    pub source_id: String,
    pub target_id: String,
    /// True for the dashed subgraph-fringe self-loop edges
    /// `mae_canvas::kb_graph::build_kb_graph` emits (`edge.style.dashed`),
    /// NOT the same as the unrelated `NodeStyle.highlighted` "starter node"
    /// flag.
    pub boundary: bool,
    pub label: Option<String>,
}

/// Initial per-tick temperature for a freshly (re)populated animated layout
/// — mirrors `ForceLayout::run`'s first-iteration temperature (`100.0 * (1.0
/// - 0/iters) = 100.0`).
pub const ANIMATION_INITIAL_TEMPERATURE: f64 = 100.0;
/// Per-tick cooling multiplier applied in `apply_graph_layout_result` after
/// each animation tick, floor-clamped at `ANIMATION_TEMPERATURE_FLOOR`. This
/// keeps the schedule self-terminating (temperature converges to a floor
/// rather than staying high indefinitely) without needing `ForceLayout::
/// run`'s fixed total-iteration-count linear schedule, since Phase 3's tick
/// count is open-ended (driven by real elapsed frames, not a precomputed
/// loop bound).
pub const ANIMATION_COOLING_FACTOR: f64 = 0.92;
/// Floor for `anim_temperature` — cooling never drives it to (near-)zero,
/// since forces near equilibrium already produce a shrinking settlement
/// signal on their own (see `ForceLayout::step`'s doc comment); a nonzero
/// floor just avoids the schedule stalling at an unproductively tiny
/// temperature for many ticks before actually reaching equilibrium.
///
/// MUST stay below `ANIMATION_SETTLE_EPSILON / damping` (0.3 / 0.85 ≈
/// 0.35, `ForceLayout::step`'s fixed 0.85 damping) — `step`'s applied
/// displacement is `min(temperature, raw_disp) * damping`, so once
/// temperature has cooled to the floor, applied displacement is capped at
/// `floor * damping` REGARDLESS of the raw force field. A floor at or
/// above that bound (this was `1.0`, i.e. `1.0 * 0.85 = 0.85 >
/// ANIMATION_SETTLE_EPSILON`) makes the settle check permanently
/// unsatisfiable once temperature reaches the floor — for any graph whose
/// true equilibrium residual force doesn't fall to exactly zero (large
/// graphs, or asymmetric kind-clustering forces at a cluster boundary),
/// the layout jitters at the floor forever instead of ever reporting
/// settled. `0.1` gives a `0.1 * 0.85 = 0.085` ceiling, comfortably under
/// the `0.3` threshold, so settling is guaranteed within a bounded number
/// of cooling ticks once temperature reaches the floor.
pub const ANIMATION_TEMPERATURE_FLOOR: f64 = 0.1;
/// Settlement threshold for `ForceLayout::step`'s max-displacement return
/// value: once a tick's max displacement drops below this, `GraphView.
/// animating` flips to `false` and animation ticking stops (until the next
/// open/refresh/set-depth/follow-current repopulate).
pub const ANIMATION_SETTLE_EPSILON: f32 = 0.3;

impl GraphView {
    pub fn new() -> Self {
        GraphView {
            center_node: None,
            depth: 2,
            kb_instance: None,
            follow_current: true,
            scene: mae_canvas::scene::SceneGraph::new(),
            viewports: HashMap::new(),
            companion_window: DrivenWindow::none(),
            rendered: HashMap::new(),
            animating: false,
            anim_temperature: ANIMATION_INITIAL_TEMPERATURE,
            layout_config: mae_canvas::layout::LayoutConfig::default(),
            node_degrees: Vec::new(),
            color_tween: None,
            render_epoch: HashMap::new(),
            hidden_node_count: 0,
        }
    }
}

impl Default for GraphView {
    fn default() -> Self {
        Self::new()
    }
}

/// Direction for graph keyboard/Scheme/MCP navigation — a `mae-core`-local
/// mirror of `mae_canvas::interaction::Direction` (same shape as
/// `graph_view_support`'s `NodeKind` mirror) so that `GraphViewIntent`,
/// constructed by the `mae-scheme` and `mae-ai` crates (neither of which
/// depend on `mae-canvas`), doesn't require a new dependency edge just to
/// name a direction. Converted to the real `mae_canvas::interaction::
/// Direction` at the `Editor::kb_graph_view_navigate` call site — the first
/// place in the dependency graph that already depends on both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphNavDirection {
    Up,
    Down,
    Left,
    Right,
}

impl GraphNavDirection {
    /// Parse a direction name (`"up"`/`"down"`/`"left"`/`"right"`,
    /// case-insensitive) as used by the Scheme `(kb-graph-view-navigate DIR)`
    /// primitive and the `kb_graph_view_navigate` MCP tool.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

impl From<GraphNavDirection> for mae_canvas::interaction::Direction {
    fn from(d: GraphNavDirection) -> Self {
        match d {
            GraphNavDirection::Up => Self::Up,
            GraphNavDirection::Down => Self::Down,
            GraphNavDirection::Left => Self::Left,
            GraphNavDirection::Right => Self::Right,
        }
    }
}

/// Scheme/MCP-originated intents for the graph view, queued into
/// `SharedState::pending_graph_view_intents` by the Scheme primitives in
/// `runtime/kb_graph_view.rs` and drained in order by `apply_to_editor` into
/// the matching `Editor::kb_graph_view_*` method — the same
/// `KbCollabAction`/`queue_kb_collab_action` pattern used for `(kb-share)`
/// etc. (see `crates/core/src/editor/mod.rs::KbCollabAction`).
#[derive(Debug, Clone)]
pub enum GraphViewIntent {
    Open {
        center: Option<String>,
        depth: Option<usize>,
    },
    Close,
    Refresh,
    SetDepth(usize),
    Navigate(GraphNavDirection),
    SelectCurrent,
    /// Issue #322 — set the graph's zoom to an explicit level (not a
    /// pixel-focus-based wheel delta, which has no AI-appropriate
    /// equivalent). Applies to the focused window if it's showing the
    /// graph buffer, else the first window found showing it — see
    /// `Editor::kb_graph_view_zoom_to`.
    ZoomTo(f64),
    /// Issue #322 — pin or unpin a node by KB id, optionally repositioning
    /// it. No drag gesture needed; `SceneNode.pinned` is already fully
    /// respected by `ForceLayout` (pinned nodes are skipped when applying
    /// displacement) — this is pure plumbing onto an existing concept.
    SetPinned {
        id: String,
        pinned: bool,
        pos: Option<(f64, f64)>,
    },
    /// Toggle the graph view between its normal tiled pane and a full-frame
    /// modal overlay with a dimmed background — see
    /// `Editor::kb_graph_view_toggle_overlay`.
    ToggleOverlay,
}

/// WHICH algorithm computes a KB graph's node positions — orthogonal to
/// [`GraphLayoutMode`] below, which is a SCHEDULING concept (one-shot vs.
/// animated-tick) for whichever algorithm is chosen, not an algorithm
/// choice itself. Configured via the `kb_graph_layout_algorithm` option;
/// see `Editor::populate_graph_buffer`'s branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GraphLayoutAlgorithm {
    /// Force-directed physics simulation (Fruchterman-Reingold,
    /// `mae_canvas::layout::ForceLayout`) — nodes seeded on a sunflower
    /// disk, then iteratively refined on a background thread.
    Force,
    /// Nodes evenly spaced around a circle's circumference, relationships
    /// drawn as curved chords through the interior (Circos/D3-chord
    /// style). Computed once, synchronously, with no background
    /// refinement pass — the initial placement IS the final layout.
    #[default]
    Chord,
}

impl GraphLayoutAlgorithm {
    /// Stable wire/config string (matches the option's accepted values).
    pub fn as_str(&self) -> &'static str {
        match self {
            GraphLayoutAlgorithm::Force => "force",
            GraphLayoutAlgorithm::Chord => "chord",
        }
    }

    /// Parse a configured value.
    pub fn parse(s: &str) -> Option<GraphLayoutAlgorithm> {
        match s.trim().to_ascii_lowercase().as_str() {
            "force" => Some(GraphLayoutAlgorithm::Force),
            "chord" => Some(GraphLayoutAlgorithm::Chord),
            _ => None,
        }
    }
}

/// Which background layout pass a `GraphLayoutIntent` requests.
///
/// - `OneShot`: Phase 1/2 behavior (`kb_graph_animate = false`, the
///   default) — run the full cooling schedule inline on the background
///   thread in a single round-trip (`ForceLayout::run`). Unchanged from
///   before Phase 3 existed.
/// - `Tick`: Phase 3 behavior (`kb_graph_animate = true`) — a single
///   `ForceLayout::step` at `temperature`, one round-trip per animation
///   frame. `apply_graph_layout_result` reads the returned settlement
///   signal and, if not yet settled, queues the *next* tick itself — so
///   `graph_view_ops.rs` only ever originates the FIRST tick of a run (on
///   open/refresh/set-depth/follow-current); the chain is self-sustaining
///   after that, reusing this exact same intent/channel/apply machinery
///   rather than a second scheduler.
///
/// Only meaningful for [`GraphLayoutAlgorithm::Force`] — `Chord` never
/// queues a `GraphLayoutIntent` at all (see `populate_graph_buffer`).
#[derive(Debug, Clone, Copy)]
pub enum GraphLayoutMode {
    OneShot { iterations: usize },
    Tick { temperature: f64 },
}

/// A queued request to (re)compute a KB graph's force-directed layout on a
/// background thread (`mae::graph_layout_bridge`). Set by
/// `graph_view_ops.rs` on every open/refresh/set-depth, drained once per GUI
/// event-loop tick by `GuiApp::drain_intents_and_lifecycle` — mirrors the
/// `dap.pending_intents`/`lsp` intent-queue pattern. The TUI has no
/// background bridge (no winit `EventLoopProxy`): this simply sits unread
/// under a TUI/headless runtime, which is harmless — `GraphView.scene`
/// still has the cheap initial circular layout from
/// `mae_canvas::kb_graph::build_kb_graph_positions_only`, and the TUI never
/// renders node positions anyway (it uses `render_graph_view_as_text`'s
/// textual "** Neighborhood" fallback instead).
#[derive(Debug, Clone)]
pub struct GraphLayoutIntent {
    pub buf_idx: usize,
    pub scene: mae_canvas::scene::SceneGraph,
    pub mode: GraphLayoutMode,
    /// Carries `GraphView.layout_config` through to the background
    /// computation — see that field's doc comment for why every intent
    /// (including the Phase 3 tick-requeue) must read the cached value
    /// rather than defaulting.
    pub layout_config: mae_canvas::layout::LayoutConfig,
}

/// Resolved, theme-driven styling inputs for `flatten_scene_graph` — sizing
/// from `OptionRegistry`-backed `Editor` fields, colors from the active
/// `Theme` (`ui.graph.*` keys, using `Theme::style`'s dot-notation
/// hierarchy fallback: `ui.graph.node.<kind>` -> `ui.graph.node` ->
/// `ui.graph` -> `ui.text` -> built-in default). Built once per (re)populate
/// by `Editor::graph_style_options` — colors are NEVER hardcoded inside the
/// flattener itself (principle #7/#8).
#[derive(Debug, Clone)]
pub struct GraphStyleOptions {
    /// Base/reference node circle radius (logical px) at `viewport.zoom ==
    /// 1.0`, before degree/zoom adjustments — see `node_render_radius`.
    pub node_radius: f32,
    pub font_size: f32,
    /// Whether `node_render_radius` adds a `node_degree_scale *
    /// sqrt(degree)` bump on top of `node_radius`. Mirrors
    /// `kb_graph_node_size_by_degree`.
    pub size_by_degree: bool,
    /// Logical px added per `sqrt(degree)` when `size_by_degree` is on.
    /// Mirrors `kb_graph_node_degree_scale`.
    pub node_degree_scale: f32,
    /// Whether `node_render_radius` scales the degree-adjusted radius by
    /// `sqrt(viewport.zoom)` — the Sigma.js/org-roam-ui-precedented
    /// sub-linear zoom scaling (see that function's doc comment for why
    /// sub-linear, not full 1:1 geometric, is the established convention).
    /// Mirrors `kb_graph_node_size_scales_with_zoom`.
    pub size_scales_with_zoom: bool,
    /// Exponent applied to `viewport.zoom` when `size_scales_with_zoom` is
    /// on: `zoom.powf(node_zoom_scale_exponent)`. `0.5` (the default)
    /// reproduces the original `sqrt(zoom)` Sigma.js/org-roam-ui behavior.
    /// `1.0` makes node radius shrink at exactly the same rate as
    /// inter-node scene distance, so the visual GAP between two nodes'
    /// edges stays proportionally constant across zoom levels instead of
    /// shrinking faster than the nodes do — the direct lever for "let me
    /// see the distance between nodes when zoomed out". `0.0` disables
    /// zoom scaling entirely (`zoom.powf(0.0) == 1.0` always), equivalent
    /// to `size_scales_with_zoom = false`. Mirrors
    /// `kb_graph_node_zoom_scale_exponent`.
    pub node_zoom_scale_exponent: f32,
    /// Absolute floor (logical px) on the FINAL render radius, applied
    /// after both degree and zoom scaling — guarantees a node never
    /// shrinks below a clickable/visible size even at extreme zoom-out.
    /// Mirrors `kb_graph_node_min_radius`.
    pub node_min_radius: f32,
    /// Absolute ceiling (logical px) on the FINAL render radius. Mirrors
    /// `kb_graph_node_max_radius`.
    pub node_max_radius: f32,
    /// Below this `viewport.zoom` level, node labels are hidden (the
    /// `Circle` is always still pushed — nodes stay visible/clickable
    /// regardless). Mirrors `kb_graph_label_zoom_threshold`.
    pub label_zoom_threshold: f32,
    /// Whether `flatten_scene_graph` suppresses lower-priority overlapping
    /// labels via greedy occlusion culling — see `compute_label_winners`.
    /// Mirrors `kb_graph_label_declutter_enabled`.
    pub label_declutter_enabled: bool,
    /// Curvature of internal (non-boundary) edges as a fraction of edge
    /// length, `0.0` = straight lines. Mirrors `kb_graph_edge_curvature`;
    /// see the curved-edge control-point computation in
    /// `flatten_scene_graph`'s edge loop and `MAX_CURVE_OFFSET_PX`'s cap.
    pub edge_curvature: f32,
    /// Which algorithm computed the scene's node positions — the
    /// edge-curve control-point formula in `flatten_scene_graph` branches
    /// on this (#367): `Chord` pulls toward the circle's center instead of
    /// `Force`'s perpendicular-offset-from-midpoint. Mirrors
    /// `kb_graph_layout_algorithm`.
    pub layout_algorithm: GraphLayoutAlgorithm,
    /// `(node_index, hex_color)` — when set, that node's color is FORCED
    /// to this value, checked before the selected/hovered/kind-color
    /// chain below. Set by the call site (not `from_editor`, which has no
    /// per-`GraphView` knowledge) from an active `GraphView.color_tween`'s
    /// `current_color()`, letting `flatten_scene_graph`'s signature and
    /// existing tests stay untouched by the tween mechanism entirely.
    pub color_override: Option<(usize, String)>,
    /// Whether node circles get a stroke outline (`edge_color`, the same
    /// flat/muted color used for regular edges). Off by default — a
    /// uniform grey ring around EVERY node regardless of its own kind
    /// color read as visual noise unrelated to the node itself, not a
    /// meaningful accent. Mirrors `kb_graph_node_border_enabled`.
    pub node_border_enabled: bool,
    /// Hex fill color per canvas `NodeKind`, indexed via `kind_index`.
    node_colors: [String; 14],
    /// WCAG-AA-legible text color per `NodeKind`, precomputed ONCE here
    /// (not per-node, not per-frame) — see `text_color_for_kind`'s doc
    /// comment for why this exists as a separate array from `node_colors`
    /// rather than calling `ensure_min_contrast` inline in the node loop.
    node_text_colors: [String; 14],
    pub selected_color: String,
    /// Color for the node currently under the mouse cursor (real-time
    /// hover — see `mae_canvas::scene::SceneGraph.hovered`). Loses priority
    /// to `selected_color` when a node is both selected and hovered.
    pub hover_color: String,
    /// WCAG-AA-legible text color for a selected/hovered node's label —
    /// see `node_text_colors`' doc comment. Kept as one combined field
    /// (rather than separate selected/hovered variants) since a node's
    /// label only ever shows ONE of these two colors at a time, following
    /// the exact same selected-wins-over-hovered priority as the fill.
    pub selected_text_color: String,
    pub hover_text_color: String,
    pub edge_color: String,
    pub boundary_edge_color: String,
    /// WCAG-AA-legible text color for a boundary stub's "..."/"... (+N)"
    /// label — see `node_text_colors`' doc comment.
    pub boundary_edge_text_color: String,
    pub background_color: String,
}

/// Stable index of a canvas `NodeKind` into `GraphStyleOptions::node_colors`
/// — declaration order of the enum, kept in sync by
/// `graph_style_options_covers_every_node_kind` below.
fn kind_index(kind: mae_canvas::scene::NodeKind) -> usize {
    use mae_canvas::scene::NodeKind as K;
    match kind {
        K::Index => 0,
        K::Command => 1,
        K::Concept => 2,
        K::Key => 3,
        K::Note => 4,
        K::Project => 5,
        K::Category => 6,
        K::Lesson => 7,
        K::Tutorial => 8,
        K::Meta => 9,
        K::Block => 10,
        K::SchemeApi => 11,
        K::Task => 12,
        K::View => 13,
    }
}

/// Theme keys for each canvas `NodeKind`, in `kind_index` order.
const NODE_KIND_THEME_KEYS: [&str; 14] = [
    "ui.graph.node.index",
    "ui.graph.node.command",
    "ui.graph.node.concept",
    "ui.graph.node.key",
    "ui.graph.node.note",
    "ui.graph.node.project",
    "ui.graph.node.category",
    "ui.graph.node.lesson",
    "ui.graph.node.tutorial",
    "ui.graph.node.meta",
    "ui.graph.node.block",
    "ui.graph.node.scheme_api",
    "ui.graph.node.task",
    "ui.graph.node.view",
];

/// Fallback hex colors matching `crates/canvas/src/kb_graph.rs`'s
/// Phase-0-placeholder `kind_to_style` palette, so a theme that doesn't
/// define `ui.graph.*` (and falls all the way through `Theme::style`'s
/// hierarchy to the built-in default style, which has no `fg`) still gets a
/// sensible, kind-distinct default rather than every node rendering
/// identically.
const NODE_KIND_FALLBACK_HEX: [&str; 14] = [
    "#ffaa4a", "#ff6aff", "#4a9eff", "#4aaaff", "#6a6dff", "#4affff", "#aa4aff", "#6aff6a",
    "#8fff8f", "#ff6a6a", "#cccc4a", "#ffff6a", "#6aff9a", "#ff4aaa",
];

/// Resolve a theme style-key's foreground color to a `"#rrggbb"` hex
/// string, falling back to `fallback` when the theme has no `fg` for that
/// key (including its dot-hierarchy ancestors — see `Theme::style`).
fn theme_hex_fg(editor: &Editor, key: &str, fallback: &str) -> String {
    match editor.theme.style(key).fg {
        Some(color) => {
            let (r, g, b) = crate::theme::Theme::resolve_to_rgb(&color);
            format!("#{r:02x}{g:02x}{b:02x}")
        }
        None => fallback.to_string(),
    }
}

/// Resolve a theme style-key's BACKGROUND color to a `"#rrggbb"` hex
/// string, falling back to `fallback` when the theme has no `bg` for that
/// key. Every shipped theme defines `"ui.graph.background"` with `bg`
/// (never `fg` — it's a background, not a foreground) — `background_color`
/// used to be built via `theme_hex_fg`, which only ever reads `.fg`, so it
/// silently fell back to the Rust-hardcoded default for every theme,
/// always, never actually theme-driven. This sibling function is the fix.
fn theme_hex_bg(editor: &Editor, key: &str, fallback: &str) -> String {
    match editor.theme.style(key).bg {
        Some(color) => {
            let (r, g, b) = crate::theme::Theme::resolve_to_rgb(&color);
            format!("#{r:02x}{g:02x}{b:02x}")
        }
        None => fallback.to_string(),
    }
}

/// Map a single 0.0-1.0 "clustering strength" knob (the
/// `kb_graph_layout_kind_clustering` option) onto the four
/// `mae_canvas::layout::KindAffinityConfig` multipliers `ForceLayout::step`
/// actually consumes. `strength <= 0.0` returns `None` exactly — a provable
/// non-regression at the default-off setting, since `LayoutConfig`'s own
/// `kind_affinity: None` reproduces the pre-clustering layout byte-for-byte.
/// Softens same-kind repulsion and boosts same-kind attraction
/// symmetrically as `strength` rises toward `1.0`; cross-kind pairs are
/// always left at `1.0` (neutral) — clustering pulls same-kind nodes
/// together, it doesn't push different kinds apart beyond the base force.
pub fn kind_affinity_from_strength(
    strength: f32,
) -> Option<mae_canvas::layout::KindAffinityConfig> {
    if strength <= 0.0 {
        return None;
    }
    let strength = strength.min(1.0) as f64;
    Some(mae_canvas::layout::KindAffinityConfig {
        same_kind_repulsion: 1.0 - 0.4 * strength,
        cross_kind_repulsion: 1.0,
        same_kind_attraction: 1.0 + 0.2 * strength,
        cross_kind_attraction: 1.0,
    })
}

impl GraphStyleOptions {
    /// Build from the current `Editor` option values + active theme.
    ///
    /// Runs `ensure_min_contrast` exactly `14 + 3 = 17` times total (once
    /// per `NodeKind`, plus selected/hover/boundary) — NOT once per node.
    /// This function is called once per (re)populate/reflatten, same as
    /// before; the WCAG text-color computation used to be re-run inline
    /// per-node inside `flatten_scene_graph`'s node loop instead, which
    /// for a real ~1300-node/4000-edge subgraph meant ~1300 extra
    /// `powf(2.4)`-heavy WCAG evaluations on the main thread EVERY
    /// re-flatten — including every single animation tick while a
    /// hover/selection color tween was in flight (`tick_graph_color_
    /// tweens` re-flattens every tick). Precomputing per-KIND instead of
    /// per-NODE turns that into a small constant (17), independent of
    /// graph size — confirmed as a real, measurable contributor via a
    /// live `introspect(frame)` capture (~17ms draw phase on a 1358-node
    /// graph, well over the 16.7ms/60fps budget) before this fix.
    pub fn from_editor(editor: &Editor) -> Self {
        // Applied to every resolved node/selected/hover fill color below —
        // see `cap_saturation`'s doc comment. Live-tunable
        // (`kb_graph_node_saturation_cap`, default 0.55) rather than baked
        // into theme files, so it applies uniformly to every theme
        // (bundled or user-authored) and can be turned off entirely
        // (`1.0`) to see a theme's true, unmuted colors.
        let sat_cap = editor.kb_graph_node_saturation_cap;
        let mut node_colors: [String; 14] = Default::default();
        for i in 0..14 {
            let raw = theme_hex_fg(editor, NODE_KIND_THEME_KEYS[i], NODE_KIND_FALLBACK_HEX[i]);
            node_colors[i] = cap_saturation(&raw, sat_cap);
        }
        let background_color = theme_hex_bg(editor, "ui.graph.background", "#0d0d0d");
        let mut node_text_colors: [String; 14] = Default::default();
        for i in 0..14 {
            node_text_colors[i] =
                ensure_min_contrast(&node_colors[i], &background_color, WCAG_AA_TEXT_CONTRAST);
        }
        let selected_color = cap_saturation(
            &theme_hex_fg(editor, "ui.graph.node.selected", "#ff9933"),
            sat_cap,
        );
        let hover_color = cap_saturation(
            &theme_hex_fg(editor, "ui.graph.node.hover", "#66ccff"),
            sat_cap,
        );
        let boundary_edge_color = theme_hex_fg(editor, "ui.graph.edge.boundary", "#ff6666");
        GraphStyleOptions {
            node_radius: editor.kb_graph_node_radius as f32,
            font_size: editor.kb_graph_font_size as f32,
            size_by_degree: editor.kb_graph_node_size_by_degree,
            node_degree_scale: editor.kb_graph_node_degree_scale,
            size_scales_with_zoom: editor.kb_graph_node_size_scales_with_zoom,
            node_zoom_scale_exponent: editor.kb_graph_node_zoom_scale_exponent,
            node_min_radius: editor.kb_graph_node_min_radius as f32,
            node_max_radius: editor.kb_graph_node_max_radius as f32,
            label_zoom_threshold: editor.kb_graph_label_zoom_threshold,
            label_declutter_enabled: editor.kb_graph_label_declutter_enabled,
            edge_curvature: editor.kb_graph_edge_curvature,
            layout_algorithm: editor.kb_graph_layout_algorithm,
            color_override: None,
            node_border_enabled: editor.kb_graph_node_border_enabled,
            node_colors,
            node_text_colors,
            selected_text_color: ensure_min_contrast(
                &selected_color,
                &background_color,
                WCAG_AA_TEXT_CONTRAST,
            ),
            selected_color,
            hover_text_color: ensure_min_contrast(
                &hover_color,
                &background_color,
                WCAG_AA_TEXT_CONTRAST,
            ),
            hover_color,
            edge_color: theme_hex_fg(editor, "ui.graph.edge", "#6a6d7e"),
            boundary_edge_text_color: ensure_min_contrast(
                &boundary_edge_color,
                &background_color,
                WCAG_AA_TEXT_CONTRAST,
            ),
            boundary_edge_color,
            background_color,
        }
    }

    pub(crate) fn color_for_kind(&self, kind: mae_canvas::scene::NodeKind) -> &str {
        &self.node_colors[kind_index(kind)]
    }

    /// The precomputed WCAG-AA-legible text color for a node's kind — see
    /// `node_text_colors`' doc comment for why this is a lookup, not a
    /// per-call `ensure_min_contrast` evaluation.
    pub(crate) fn text_color_for_kind(&self, kind: mae_canvas::scene::NodeKind) -> &str {
        &self.node_text_colors[kind_index(kind)]
    }
}

/// Count edges touching each node (O(E)) — computed ONCE by
/// `populate_graph_buffer` right after `scene` is built and cached on
/// `GraphView.node_degrees`, NOT recomputed per-frame or per-hover: layout
/// ticks only move positions, never change topology, so a cached count
/// stays valid across the whole life of an open graph. A boundary self-loop
/// (see `mae_canvas::kb_graph::build_kb_graph_positions_only`) counts once
/// toward its source, same as any other edge.
pub fn node_degrees(scene: &mae_canvas::scene::SceneGraph) -> Vec<u32> {
    let mut degrees = vec![0u32; scene.nodes.len()];
    for edge in &scene.edges {
        if let Some(d) = degrees.get_mut(edge.source) {
            *d += 1;
        }
        if edge.target != edge.source {
            if let Some(d) = degrees.get_mut(edge.target) {
                *d += 1;
            }
        }
    }
    degrees
}

/// Compute a node's FINAL render radius (logical px) — the single formula
/// both drawing (`flatten_scene_graph`) and hit-testing
/// (`graph_view_ops.rs::graph_scene_hit_radii`) share, so they can never
/// disagree about where a node's clickable/visible boundary actually is.
///
/// Two independent, each-optional adjustments on top of `style.node_radius`
/// (the base/reference size at `zoom == 1.0`):
///
/// - **Degree**: `+ node_degree_scale * sqrt(degree)` when `size_by_degree`
///   is on — hub nodes read as visually prominent, a standard convention
///   (Obsidian sizes graph-view nodes by link count) previously entirely
///   absent here (every node rendered at the same fixed radius regardless
///   of connectivity).
/// - **Zoom**: `* sqrt(viewport.zoom)` when `size_scales_with_zoom` is on —
///   SUB-LINEAR, not full 1:1 geometric scaling. Researched precedent:
///   Sigma.js's documented default is exactly `sqrt(zoom ratio)` ("nodes
///   and edges grow LESS than the zoom"); org-roam-ui ships a tunable
///   "keep node size invariant across zoom" slider defaulting to a similar
///   dampened curve. Pure linear/geometric scaling was considered and
///   rejected: it makes nodes shrink to sub-pixel (unreadable, unclickable)
///   at extreme zoom-out and balloon to overwhelming size at extreme
///   zoom-in — sub-linear scaling still lets zooming out reveal the gaps
///   between nodes (this codebase's fixed-screen-space-only sizing, the
///   PRE-this-change behavior, was the opposite failure mode: node circles
///   never shrank at all, so zooming out to see a large graph just produced
///   an unreadable mass of same-size overlapping circles).
///
/// Finally clamped to `[node_min_radius, node_max_radius]` — the floor
/// guarantees a node is never smaller than clickable/visible even at
/// extreme zoom-out; the ceiling caps degree+zoom growth at extreme
/// zoom-in/high-degree combinations.
pub fn node_render_radius(style: &GraphStyleOptions, degree: u32, zoom: f64) -> f32 {
    let mut r = style.node_radius;
    if style.size_by_degree {
        r += style.node_degree_scale * (degree as f32).sqrt();
    }
    if style.size_scales_with_zoom {
        // Exponent 0.5 (sqrt) is Sigma.js's documented default; 1.0 is
        // full 1:1 geometric scaling (radius shrinks at the SAME rate as
        // inter-node distance, so the gap between two node edges stays
        // proportionally constant regardless of zoom — the lever for
        // "let me see the actual distance between nodes when zoomed
        // out"); 0.0 reproduces the old fixed-screen-space behavior
        // exactly (zoom.powf(0.0) == 1.0 always). See
        // `kb_graph_node_zoom_scale_exponent`'s doc comment.
        r *= (zoom.max(0.0) as f32).powf(style.node_zoom_scale_exponent);
    }
    // `f32::clamp` panics if min > max — the two bounds are independently
    // user-configurable options with no cross-validation at `:set` time, so
    // guard against a misconfigured min > max rather than trust ordering.
    let (lo, hi) = if style.node_min_radius <= style.node_max_radius {
        (style.node_min_radius, style.node_max_radius)
    } else {
        (style.node_max_radius, style.node_min_radius)
    };
    r.clamp(lo, hi)
}

/// Flatten a `mae-canvas` `SceneGraph` into `VisualElement`s for the GUI's
/// `render_visual_buffer` pipeline, projected through one window's
/// `Viewport` (issue #321 — the same shared `scene` can be flattened once
/// per window showing it, each through its own pan/zoom/pixel-size). Edges
/// are emitted before nodes (drawn under them); boundary edges
/// (`SceneEdge.style.dashed`, the subgraph fringe — see
/// `mae_canvas::kb_graph::build_kb_graph`) render as dashed lines using
/// `style.boundary_edge_color`, internal edges as solid lines using
/// `style.edge_color`. Nodes render as a themed circle (selected node uses
/// `style.selected_color`, others their `NodeKind`'s themed color) plus a
/// label `Text` element. Pure function — no `Editor`/theme access, so it's
/// independently unit-testable against a hand-built `SceneGraph` +
/// `Viewport` + `GraphStyleOptions`.
/// Generous fixed screen-space margin (px) added to a node's AABB before
/// culling, to account for its label — text draws to the right of the
/// node and this function has no font-metrics access to measure its real
/// width. Deliberately generous: a false negative here (culling something
/// actually visible) is a correctness bug, a false positive (keeping
/// something that's actually off-screen) just costs one extra draw call.
const LABEL_CULL_MARGIN_PX: f32 = 220.0;

/// Whether a node's screen-space AABB (circle + the generous label margin
/// on its right side) doesn't intersect `[0, width] x [0, height]` at
/// all — if so, its `Circle`/`Text` are skipped entirely by
/// `flatten_scene_graph`. Never affects hit-testing/keyboard-nav/
/// `describe_state` — those operate on `scene`/`Viewport` directly, never
/// on this function's `VisualElement` output.
fn node_is_offscreen(scx: f32, scy: f32, r: f32, viewport: &mae_canvas::scene::Viewport) -> bool {
    let right = scx + r + LABEL_CULL_MARGIN_PX;
    let left = scx - r;
    let top = scy - r;
    let bottom = scy + r;
    right < 0.0 || left > viewport.width as f32 || bottom < 0.0 || top > viewport.height as f32
}

/// Maximum perpendicular offset (screen px) a curved edge's control point
/// can bow away from the straight line between its endpoints — see the
/// curved-edge control-point computation in `flatten_scene_graph`'s edge
/// loop. Also used as `edge_is_offscreen_same_side`'s cull margin, so a
/// curve that bows toward the viewport near a same-side-offscreen edge is
/// never incorrectly culled.
const MAX_CURVE_OFFSET_PX: f32 = 60.0;

/// Whether an edge's two screen-space endpoints are BOTH off-screen on the
/// SAME side — the only case it's safe to cull. Deliberately conservative:
/// a long edge with both endpoints off-screen on DIFFERENT sides might
/// still cross the visible viewport, so it's never culled by this check
/// (no full segment-vs-rect test — that correctness margin is worth more
/// than the extra draw calls saved). Margined by `MAX_CURVE_OFFSET_PX` so
/// a curved edge's bow (which can bring it into view even when both
/// straight-line endpoints are just off-screen) is never wrongly culled.
fn edge_is_offscreen_same_side(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    viewport: &mae_canvas::scene::Viewport,
) -> bool {
    let w = viewport.width as f32;
    let h = viewport.height as f32;
    let m = MAX_CURVE_OFFSET_PX;
    (x1 < -m && x2 < -m)
        || (x1 > w + m && x2 > w + m)
        || (y1 < -m && y2 < -m)
        || (y1 > h + m && y2 > h + m)
}

/// Estimated per-character advance width, as a fraction of `font_size`,
/// for the graph view's label text. This module has no real font-metrics
/// access (pure geometry — Skia painting happens later in
/// `crates/gui/src/lib.rs`), so label width must be ESTIMATED from
/// character count. The GUI backend draws graph-view labels with a single
/// monospace typeface, so a single ratio applies uniformly to every
/// character — a materially better approximation than a general
/// "0.5-0.6x for proportional fonts" rule of thumb would be. `0.6` sits
/// at the generous end of typical monospace advance widths; erring
/// generous only ever over-suppresses a lower-priority neighbor's label,
/// never under-suppresses (which would be a correctness bug) — same
/// generous-margin philosophy as `LABEL_CULL_MARGIN_PX` above.
const LABEL_CHAR_WIDTH_RATIO: f32 = 0.6;

/// Small screen-space gap (px) required between two placed label boxes on
/// top of pure non-overlap, so surviving labels never read as visually
/// merged even when their boxes technically just touch. A fixed
/// rendering-tuning constant, not something a user would reasonably want
/// to vary (principle #7's "truly fixed" carve-out).
const LABEL_DECLUTTER_PADDING_PX: f32 = 2.0;

/// A node's estimated screen-space label bounding box `(x0, y0, x1, y1)`,
/// in the SAME coordinate space the node loop actually draws the label
/// into (`x: scx + r + 4.0`, baseline-left at `y: scy`) — this is the
/// ONLY place either `compute_label_winners` or the real draw computes
/// that offset, so they can never disagree (principle #8, same discipline
/// as `node_render_radius`).
fn label_bbox(scx: f32, scy: f32, r: f32, label: &str, font_size: f32) -> (f32, f32, f32, f32) {
    let x0 = scx + r + 4.0;
    let width = label.chars().count() as f32 * font_size * LABEL_CHAR_WIDTH_RATIO;
    // The real draw's `y` is the text baseline; approximate generous
    // ascent/descent bounds around it (no real font-metrics query
    // available here) — errs tall, same generous-on-purpose philosophy as
    // the width estimate above.
    (x0, scy - font_size, x0 + width, scy + font_size * 0.3)
}

fn label_boxes_overlap(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> bool {
    a.0 < b.2 + LABEL_DECLUTTER_PADDING_PX
        && a.2 + LABEL_DECLUTTER_PADDING_PX > b.0
        && a.1 < b.3 + LABEL_DECLUTTER_PADDING_PX
        && a.3 + LABEL_DECLUTTER_PADDING_PX > b.1
}

/// Greedy priority-based label occlusion culling — the standard real-time
/// approximation to the NP-hard optimal label-placement problem, used by
/// cartographic renderers (e.g. city labels dropping out at low zoom
/// while major-city labels persist) and shipped graph-visualization tools
/// (Gephi's "Label Adjust", org-roam-ui's degree-aware label filtering).
/// Returns the set of node indices whose label wins a non-overlapping
/// screen-space slot.
///
/// Priority order: the selected and/or hovered node(s) first (tier 0),
/// then by degree descending, tie-broken by node index ascending — a pure
/// function of `scene`/`degrees`, so the SAME input always produces the
/// SAME visible-label set (no frame-to-frame flicker as ties resolve
/// differently). Offscreen nodes (per `node_is_offscreen`) never enter
/// the candidate pool, so a culled node can never consume a slot that
/// would otherwise go to a visible neighbor. Boundary-stub labels are
/// deliberately NOT part of this pool — see `flatten_scene_graph`'s edge
/// loop, which draws them unconditionally; they carry no degree/selection
/// state and their purpose (a correctness signal: "more graph beyond this
/// depth, here") shouldn't be silently hidden by a denser node label.
fn compute_label_winners(
    scene: &mae_canvas::scene::SceneGraph,
    positions: &[(f32, f32)],
    radii: &[f32],
    degrees: &[u32],
    style: &GraphStyleOptions,
    viewport: &mae_canvas::scene::Viewport,
) -> std::collections::HashSet<usize> {
    let mut candidates: Vec<usize> = (0..scene.nodes.len())
        .filter(|&i| !node_is_offscreen(positions[i].0, positions[i].1, radii[i], viewport))
        .collect();

    candidates.sort_by_key(|&i| {
        let is_priority = scene.selection == Some(i) || scene.hovered == Some(i);
        let tier: u8 = if is_priority { 0 } else { 1 };
        (
            tier,
            std::cmp::Reverse(degrees.get(i).copied().unwrap_or(0)),
            i,
        )
    });

    let mut placed: Vec<(f32, f32, f32, f32)> = Vec::with_capacity(candidates.len());
    let mut winners = std::collections::HashSet::with_capacity(candidates.len());
    for i in candidates {
        let (scx, scy) = positions[i];
        let bbox = label_bbox(scx, scy, radii[i], &scene.nodes[i].label, style.font_size);
        // Early-exit on first overlap found — avoids scanning the full
        // `placed` list for candidates rejected by the very first
        // already-placed box, the common case in a dense hub region
        // (exactly where this pass matters most).
        if !placed.iter().any(|&p| label_boxes_overlap(bbox, p)) {
            placed.push(bbox);
            winners.insert(i);
        }
    }
    winners
}

pub fn flatten_scene_graph(
    scene: &mae_canvas::scene::SceneGraph,
    viewport: &mae_canvas::scene::Viewport,
    style: &GraphStyleOptions,
    degrees: &[u32],
) -> Vec<VisualElement> {
    use mae_canvas::interaction::scene_to_viewport;

    let mut elements = Vec::with_capacity(scene.edges.len() + scene.nodes.len() * 2);
    // Every position below goes through `scene_to_viewport` — the same
    // scene<->screen transform hit-testing/drag/zoom already use via
    // `viewport_to_scene` (see `graph_view_ops.rs::graph_scene_point`) — so
    // drawing and interaction always agree on where a node actually is,
    // driven by the CALLER's chosen `viewport` (kept in sync with that
    // specific window's pixel size by `Editor::graph_view_reflatten_window`,
    // the single call site that invokes this function). Font size and
    // stub/line offsets stay fixed screen-space pixels regardless of zoom.
    // Node radius is the exception — see `node_render_radius`'s doc comment
    // for why it deliberately DOES scale (sub-linearly) with zoom.

    // Precomputed once so the edge loop (which runs BEFORE the node loop
    // below) can size a boundary stub off the SAME real per-node radius the
    // node loop draws — previously the stub used the flat, degree/zoom-
    // unaware `style.node_radius` base, so once node size started varying
    // (degree/zoom scaling), a shrunk node's stub stuck out disproportionately
    // far relative to its own now-smaller circle. Also precomputes each
    // node's screen position — the node loop and the label-declutter pass
    // below both need it, and previously the node loop called
    // `scene_to_viewport` a second time redundantly; one shared computation
    // (principle #8), numerically identical since no further `f64`
    // arithmetic happens on the position between transform and use here.
    let (positions, radii): (Vec<(f32, f32)>, Vec<f32>) = scene
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let (x, y) = scene_to_viewport(viewport, n.x, n.y);
            let degree = degrees.get(i).copied().unwrap_or(0);
            (
                (x as f32, y as f32),
                node_render_radius(style, degree, viewport.zoom),
            )
        })
        .unzip();

    // Scene-origin's viewport-space position — only used by the Chord
    // edge-curve formula below, but panning/zooming shifts where scene
    // `(0,0)` lands on screen, so it must go through the same transform as
    // every node position (never a raw `(0,0)` in viewport space).
    // Precomputed once, outside the edge loop, since it's identical for
    // every edge.
    let (center_x, center_y) = scene_to_viewport(viewport, 0.0, 0.0);

    for edge in &scene.edges {
        let Some(src) = scene.nodes.get(edge.source) else {
            continue;
        };
        let is_boundary = edge.style.dashed;
        let color = if is_boundary {
            style.boundary_edge_color.clone()
        } else {
            style.edge_color.clone()
        };
        let (sx1, sy1) = scene_to_viewport(viewport, src.x, src.y);
        let src_r = radii.get(edge.source).copied().unwrap_or(style.node_radius);
        // A boundary edge is represented as a self-loop (source == target,
        // see `build_kb_graph`) — draw a short stub off to the side instead
        // of a zero-length line, so it's visually distinguishable. The stub
        // offset is sized off the source node's OWN real render radius
        // (`src_r`, computed above), so it stays proportionate to the
        // circle it's attached to regardless of that node's degree/zoom
        // size — not a flat, unrelated screen-space constant.
        let (sx2, sy2) = if edge.target < scene.nodes.len() && edge.target != edge.source {
            let t = &scene.nodes[edge.target];
            scene_to_viewport(viewport, t.x, t.y)
        } else {
            (sx1 + (src_r * 2.0) as f64, sy1 - src_r as f64)
        };
        if edge_is_offscreen_same_side(sx1 as f32, sy1 as f32, sx2 as f32, sy2 as f32, viewport) {
            continue;
        }
        // Curved internal edges. Boundary/self-loop stub edges stay
        // straight regardless of algorithm — dashing here only ever
        // applies to those short stubs, never a distinct-node-to-node
        // edge, so the dash segmenter never needs to learn to dash a curve.
        if !is_boundary && style.edge_curvature > 0.0 {
            let dx = sx2 - sx1;
            let dy = sy2 - sy1;
            let len = (dx * dx + dy * dy).sqrt();
            if len > 0.0 {
                let mid_x = (sx1 + sx2) / 2.0;
                let mid_y = (sy1 + sy2) / 2.0;
                let (ctrl_x, ctrl_y) = match style.layout_algorithm {
                    GraphLayoutAlgorithm::Chord => {
                        // Chord-diagram style: pull the control point
                        // toward the circle's center instead of bowing
                        // perpendicular to the edge — the visually
                        // essential trait of a chord diagram, not just a
                        // cosmetic variant of the Force curve. Capped at
                        // `edge_curvature <= 1.0`'s natural meaning (never
                        // overshoots PAST the center itself).
                        let t = (style.edge_curvature as f64).min(1.0);
                        (
                            mid_x + (center_x - mid_x) * t,
                            mid_y + (center_y - mid_y) * t,
                        )
                    }
                    GraphLayoutAlgorithm::Force => {
                        // A quadratic control point offset perpendicular to
                        // the edge's midpoint, alternating direction by
                        // (source_index + target_index) parity so
                        // adjacent/parallel edges bow apart instead of all
                        // curving identically (a uniform direction
                        // wouldn't reduce overlap at all).
                        let (perp_x, perp_y) = (-dy / len, dx / len);
                        let sign = if (edge.source + edge.target) % 2 == 0 {
                            1.0
                        } else {
                            -1.0
                        };
                        let offset = ((style.edge_curvature as f64) * len)
                            .min(MAX_CURVE_OFFSET_PX as f64)
                            * sign;
                        (mid_x + perp_x * offset, mid_y + perp_y * offset)
                    }
                };
                elements.push(VisualElement::Curve {
                    x1: sx1 as f32,
                    y1: sy1 as f32,
                    ctrl_x: ctrl_x as f32,
                    ctrl_y: ctrl_y as f32,
                    x2: sx2 as f32,
                    y2: sy2 as f32,
                    color,
                    thickness: edge.style.width as f32,
                });
                continue;
            }
        }
        elements.push(VisualElement::Line {
            x1: sx1 as f32,
            y1: sy1 as f32,
            x2: sx2 as f32,
            y2: sy2 as f32,
            color: color.clone(),
            thickness: edge.style.width as f32,
            dashed: is_boundary,
        });
        // A boundary stub's label ("...", or "... (+N)" for a source with
        // multiple collapsed out-of-subgraph links — see
        // `build_kb_graph_positions_only`) was previously computed and
        // exposed through `describe_state()`/introspection but never
        // actually drawn anywhere in the GUI — the red dashed stub had no
        // visible explanation of what it meant. Draw it at the stub's far
        // end, in the same boundary color.
        if let Some(label) = &edge.label {
            // `boundary_edge_text_color` is precomputed ONCE by
            // `GraphStyleOptions::from_editor` (see its doc comment) —
            // the boundary/edge color is tuned to read well as a thin
            // stroke, which doesn't guarantee it's legible as small TEXT,
            // so a WCAG AA minimum is enforced, preserving hue when it
            // already clears the bar.
            elements.push(VisualElement::Text {
                x: sx2 as f32 + 4.0,
                y: sy2 as f32,
                text: label.clone(),
                font_size: style.font_size,
                color: style.boundary_edge_text_color.clone(),
            });
        }
    }

    // Greedy priority-based label occlusion culling (see
    // `compute_label_winners`'s doc comment) — computed ONCE here, gated on
    // the SAME zoom threshold that already hides all labels below it, so
    // the pass is skipped entirely (not just its result discarded) at low
    // zoom or when the feature is disabled.
    let label_winners: Option<std::collections::HashSet<usize>> =
        if style.label_declutter_enabled && viewport.zoom >= style.label_zoom_threshold as f64 {
            Some(compute_label_winners(
                scene, &positions, &radii, degrees, style, viewport,
            ))
        } else {
            None
        };

    for (i, node) in scene.nodes.iter().enumerate() {
        let is_selected = scene.selection == Some(i);
        let is_hovered = scene.hovered == Some(i);
        // Selected always wins over hovered when both target the same
        // node — a deliberate action (click/keyboard-select) outranks
        // incidental mouse position. `color_override` (an in-flight tween
        // toward this node's highlight color) wins over both — it's how
        // the SAME target node reaches selected/hover_color smoothly
        // instead of snapping the instant it becomes selected/hovered.
        let color = match &style.color_override {
            Some((idx, hex)) if *idx == i => hex.clone(),
            _ if is_selected => style.selected_color.clone(),
            _ if is_hovered => style.hover_color.clone(),
            _ => style.color_for_kind(node.kind).to_string(),
        };
        let (scx, scy) = positions[i];
        let r = radii[i];
        if node_is_offscreen(scx, scy, r, viewport) {
            continue;
        }
        elements.push(VisualElement::Circle {
            cx: scx,
            cy: scy,
            r,
            fill: Some(color.clone()),
            // No flat, kind-unrelated outline by default — see
            // `node_border_enabled`'s doc comment.
            stroke: if style.node_border_enabled {
                Some(style.edge_color.clone())
            } else {
                None
            },
        });
        // Label LOD: below the configured zoom threshold, skip the Text
        // element to reduce clutter/draw calls on dense graphs — the
        // Circle above is ALWAYS pushed, so the node stays visible and
        // clickable regardless. Above threshold, a second gate applies:
        // greedy occlusion culling may have suppressed a lower-priority
        // label that would visually overlap a higher-priority one (see
        // `compute_label_winners`) — `label_winners` is `None` when the
        // pass didn't run (declutter off, or already below-threshold),
        // in which case every node's label shows exactly as before this
        // feature existed.
        let show_label = viewport.zoom >= style.label_zoom_threshold as f64
            && label_winners
                .as_ref()
                .map(|w| w.contains(&i))
                .unwrap_or(true);
        if show_label {
            // Same WCAG AA guarantee as the boundary-stub label above, but
            // via the PRECOMPUTED per-kind lookup (`text_color_for_kind`/
            // `selected_text_color`/`hover_text_color` — see
            // `GraphStyleOptions::from_editor`'s doc comment for why this
            // must stay O(1) per node, not a fresh `ensure_min_contrast`
            // call every node every frame). The one exception is an
            // in-flight color tween's one-off interpolated hex, which
            // genuinely can't be precomputed — but at most ONE node tweens
            // at a time (the asymmetric design), so this stays O(1) per
            // frame regardless of graph size.
            let text_color = match &style.color_override {
                Some((idx, hex)) if *idx == i => {
                    ensure_min_contrast(hex, &style.background_color, WCAG_AA_TEXT_CONTRAST)
                }
                _ if is_selected => style.selected_text_color.clone(),
                _ if is_hovered => style.hover_text_color.clone(),
                _ => style.text_color_for_kind(node.kind).to_string(),
            };
            elements.push(VisualElement::Text {
                x: scx + r + 4.0,
                y: scy,
                text: node.label.clone(),
                font_size: style.font_size,
                color: text_color,
            });
        }
    }

    elements
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_canvas::scene::{EdgeStyle, NodeKind, NodeStyle, SceneEdge, SceneGraph, SceneNode};

    fn test_style() -> GraphStyleOptions {
        GraphStyleOptions {
            node_radius: 18.0,
            font_size: 14.0,
            // Matches production defaults. With degree=0 (every existing
            // test here passes an empty/all-zero degrees slice) and
            // Viewport::default()'s zoom=1.0, both scaling terms are
            // no-ops, so `node_render_radius` == `node_radius` exactly —
            // every pre-Phase-C test's radius assertions stay valid
            // unmodified.
            size_by_degree: true,
            node_degree_scale: 4.0,
            size_scales_with_zoom: true,
            // 0.5 == sqrt, matches production default — every pre-existing
            // test's radius assertions stay valid unmodified.
            node_zoom_scale_exponent: 0.5,
            node_min_radius: 4.0,
            node_max_radius: 36.0,
            // Below Viewport::default()'s zoom (1.0), so every pre-Phase-D
            // test here (all using the default viewport) keeps pushing
            // Text elements unmodified.
            label_zoom_threshold: 0.5,
            // Off by default in the test fixture (unlike production's
            // `true`) — every pre-existing test in this file asserts exact
            // `Text` elements for every above-threshold node with no
            // regard to overlap; defaulting this to `true` here would make
            // several of those tests non-deterministically break depending
            // on incidental fixture geometry. New declutter-specific tests
            // build their own style with this set to `true` explicitly,
            // same pattern as `edge_curvature` above.
            label_declutter_enabled: false,
            // 0.0 (straight lines) so every pre-Phase-E edge test here
            // (all asserting `VisualElement::Line`) stays valid unmodified
            // — new curved-edge tests build their own style with a
            // nonzero curvature explicitly.
            edge_curvature: 0.0,
            // Force, matching `edge_curvature: 0.0` above — every existing
            // edge-curve test here asserts the Force perpendicular-offset
            // formula (or a straight line); new chord-mode tests build
            // their own style with `Chord` explicitly, same pattern.
            layout_algorithm: GraphLayoutAlgorithm::Force,
            color_override: None,
            node_border_enabled: false,
            node_colors: [
                "#a1".into(),
                "#a2".into(),
                "#a3".into(),
                "#a4".into(),
                "#a5".into(),
                "#a6".into(),
                "#a7".into(),
                "#a8".into(),
                "#a9".into(),
                "#a10".into(),
                "#a11".into(),
                "#a12".into(),
                "#a13".into(),
                "#a14".into(),
            ],
            // None of the placeholder colors above are real hex (deliberately,
            // for exact-string-equality assertions elsewhere in this test
            // module), so `ensure_min_contrast` always falls back to
            // pure white/black here — computed for real (not hand-typed
            // placeholders) so any test asserting a WCAG contrast ratio
            // against these values (not just string equality) is exercising
            // the genuine code path, matching `from_editor`'s own construction.
            node_text_colors: [
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
                "#ffffff".into(),
            ],
            selected_color: "#selected".into(),
            selected_text_color: "#ffffff".into(),
            hover_color: "#hovered".into(),
            hover_text_color: "#ffffff".into(),
            edge_color: "#edge".into(),
            boundary_edge_color: "#boundary".into(),
            boundary_edge_text_color: "#ffffff".into(),
            background_color: "#bg".into(),
        }
    }

    fn test_node(id: &str, x: f64, y: f64, kind: NodeKind) -> SceneNode {
        SceneNode {
            id: id.to_string(),
            label: id.to_string(),
            x,
            y,
            width: 100.0,
            height: 40.0,
            kind,
            style: NodeStyle::default(),
            pinned: false,
        }
    }

    fn test_edge(source: usize, target: usize) -> SceneEdge {
        SceneEdge {
            source,
            target,
            label: None,
            style: EdgeStyle::default(),
            weight: 1.0,
            rel_type: None,
        }
    }

    #[test]
    fn node_degrees_counts_edges_touching_each_node() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 0.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("c", 0.0, 0.0, NodeKind::Note));
        // a-b, a-c: "a" has degree 2, "b" and "c" degree 1 each.
        scene.edges.push(test_edge(0, 1));
        scene.edges.push(test_edge(0, 2));
        assert_eq!(node_degrees(&scene), vec![2, 1, 1]);
    }

    #[test]
    fn node_degrees_counts_a_self_loop_once() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.edges.push(test_edge(0, 0)); // boundary self-loop stub
        assert_eq!(node_degrees(&scene), vec![1]);
    }

    #[test]
    fn node_render_radius_scales_with_degree_and_caps_at_max() {
        let mut style = test_style();
        style.size_scales_with_zoom = false; // isolate the degree term
        let base = node_render_radius(&style, 0, 1.0);
        let higher_degree = node_render_radius(&style, 25, 1.0);
        assert!(
            higher_degree > base,
            "a higher-degree node should render larger (base={base}, higher_degree={higher_degree})"
        );
        // An absurdly high degree must still respect the ceiling.
        let capped = node_render_radius(&style, 100_000, 1.0);
        assert_eq!(capped, style.node_max_radius);
    }

    #[test]
    fn node_render_radius_flat_when_size_by_degree_is_off() {
        let mut style = test_style();
        style.size_by_degree = false;
        style.size_scales_with_zoom = false;
        assert_eq!(node_render_radius(&style, 0, 1.0), style.node_radius);
        assert_eq!(
            node_render_radius(&style, 50, 1.0),
            style.node_radius,
            "degree must not affect radius when size_by_degree is off"
        );
    }

    #[test]
    fn node_render_radius_scales_sub_linearly_with_zoom_and_respects_the_floor() {
        let mut style = test_style();
        style.size_by_degree = false; // isolate the zoom term
        let at_zoom_1 = node_render_radius(&style, 0, 1.0);
        let zoomed_in = node_render_radius(&style, 0, 4.0);
        let zoomed_out = node_render_radius(&style, 0, 0.25);
        assert!(
            zoomed_in > at_zoom_1,
            "zooming in should grow the radius (at_zoom_1={at_zoom_1}, zoomed_in={zoomed_in})"
        );
        assert!(
            zoomed_out < at_zoom_1,
            "zooming out should shrink the radius (at_zoom_1={at_zoom_1}, zoomed_out={zoomed_out})"
        );
        // sqrt(4.0) = 2.0 exactly — confirms SUB-linear (not 1:1 geometric)
        // scaling: a 4x zoom only doubles the radius, not quadruples it.
        assert!((zoomed_in - at_zoom_1 * 2.0).abs() < 1e-4);
        // At an extreme zoom-out, the floor must still hold.
        let extreme_zoom_out = node_render_radius(&style, 0, 0.001);
        assert_eq!(extreme_zoom_out, style.node_min_radius);
    }

    #[test]
    fn node_render_radius_never_panics_when_min_exceeds_max() {
        // The two bounds are independently user-configurable options with
        // no cross-validation at `:set` time — a misconfigured min > max
        // must not panic `f32::clamp`.
        let mut style = test_style();
        style.node_min_radius = 50.0;
        style.node_max_radius = 10.0;
        let r = node_render_radius(&style, 0, 1.0);
        assert!((10.0..=50.0).contains(&r));
    }

    #[test]
    fn flatten_scene_graph_high_degree_node_renders_larger_circle() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("hub", 0.0, 0.0, NodeKind::Note));
        scene
            .nodes
            .push(test_node("leaf", 200.0, 0.0, NodeKind::Note));
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        // degrees: "hub" has a high degree, "leaf" has none.
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[20, 0]);
        let radius_of = |elements: &[VisualElement], idx: usize| match &elements[idx] {
            VisualElement::Circle { r, .. } => *r,
            other => panic!("expected Circle, got {other:?}"),
        };
        // Edges list is empty here, so elements are [hub_circle, hub_text,
        // leaf_circle, leaf_text] — nodes are emitted in scene order.
        let hub_radius = radius_of(&elements, 0);
        let leaf_radius = radius_of(&elements, 2);
        assert!(
            hub_radius > leaf_radius,
            "the high-degree node must render a larger circle (hub={hub_radius}, leaf={leaf_radius})"
        );
    }

    fn declutter_style() -> GraphStyleOptions {
        let mut style = test_style();
        style.label_declutter_enabled = true;
        style
    }

    /// Two nodes placed at the SAME scene position (so their estimated
    /// label boxes overlap regardless of exact width-estimate math) — one
    /// high-degree, one low-degree, neither selected/hovered.
    fn overlapping_nodes_scene(hi_id: &str, lo_id: &str) -> (SceneGraph, Vec<u32>) {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node(hi_id, 0.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node(lo_id, 0.0, 0.0, NodeKind::Note));
        (scene, vec![20, 0])
    }

    fn text_present_for(elements: &[VisualElement], approx_x: f32) -> bool {
        elements.iter().any(|e| match e {
            VisualElement::Text { x, .. } => (*x - approx_x).abs() < 0.01,
            _ => false,
        })
    }

    #[test]
    fn flatten_suppresses_a_lower_priority_labels_overlapping_a_higher_degree_neighbor() {
        let (scene, degrees) = overlapping_nodes_scene("hub", "leaf");
        let style = declutter_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &degrees);
        // Both circles present (nodes always stay visible/clickable).
        assert_eq!(
            elements
                .iter()
                .filter(|e| matches!(e, VisualElement::Circle { .. }))
                .count(),
            2,
            "both nodes' circles must render regardless of label suppression"
        );
        // Only the higher-degree ("hub") node's label wins the overlapping slot.
        let hub_r = node_render_radius(&style, 20, viewport.zoom);
        let leaf_r = node_render_radius(&style, 0, viewport.zoom);
        assert!(
            text_present_for(&elements, 400.0 + hub_r + 4.0),
            "the higher-degree node's label must win the overlapping slot"
        );
        assert!(
            !text_present_for(&elements, 400.0 + leaf_r + 4.0),
            "the lower-degree node's label must be suppressed"
        );
    }

    #[test]
    fn flatten_never_suppresses_the_selected_nodes_label_even_at_low_degree() {
        let (mut scene, _) = overlapping_nodes_scene("hub", "selected_leaf");
        // Index 0 ("hub") is selected but has the LOW degree — it must
        // still win the overlapping slot over the higher-degree index 1.
        let degrees = vec![0u32, 20u32];
        scene.selection = Some(0);
        let style = declutter_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &degrees);
        let selected_r = node_render_radius(&style, 0, viewport.zoom);
        assert!(
            text_present_for(&elements, 400.0 + selected_r + 4.0),
            "the selected node's label must win despite losing on degree"
        );
    }

    #[test]
    fn flatten_never_suppresses_the_hovered_nodes_label_even_at_low_degree() {
        let (mut scene, _) = overlapping_nodes_scene("hub", "hovered_leaf");
        let degrees = vec![0u32, 20u32];
        scene.hovered = Some(0);
        let style = declutter_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &degrees);
        let hovered_r = node_render_radius(&style, 0, viewport.zoom);
        assert!(
            text_present_for(&elements, 400.0 + hovered_r + 4.0),
            "the hovered node's label must win despite losing on degree"
        );
    }

    #[test]
    fn flatten_label_declutter_disabled_preserves_the_original_always_show_behavior() {
        let (scene, degrees) = overlapping_nodes_scene("hub", "leaf");
        let style = test_style(); // label_declutter_enabled: false
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &degrees);
        assert_eq!(
            elements
                .iter()
                .filter(|e| matches!(e, VisualElement::Text { .. }))
                .count(),
            2,
            "with declutter disabled, both overlapping labels must still show"
        );
    }

    #[test]
    fn flatten_label_declutter_pre_pass_is_skipped_below_the_zoom_threshold() {
        let (scene, degrees) = overlapping_nodes_scene("hub", "leaf");
        let style = declutter_style(); // label_zoom_threshold: 0.5
        let viewport = mae_canvas::scene::Viewport {
            zoom: 0.1, // below threshold
            ..mae_canvas::scene::Viewport::default()
        };
        let elements = flatten_scene_graph(&scene, &viewport, &style, &degrees);
        assert_eq!(
            elements
                .iter()
                .filter(|e| matches!(e, VisualElement::Text { .. }))
                .count(),
            0,
            "below the zoom threshold, neither label shows (unchanged from pre-declutter behavior)"
        );
    }

    #[test]
    fn flatten_boundary_stub_labels_are_exempt_from_node_label_declutter() {
        // A node's label box positioned to overlap a boundary stub's own
        // label — both must render, since boundary stubs never enter the
        // node-label occlusion pool (see `compute_label_winners`'s doc
        // comment).
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.edges.push(SceneEdge {
            source: 0,
            target: 0, // self-loop = boundary indicator
            label: Some("... (+1)".to_string()),
            style: EdgeStyle {
                color: "#unused".to_string(),
                width: 1.0,
                dashed: true,
            },
            weight: 1.0,
            rel_type: None,
        });
        let style = declutter_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[0]);
        let boundary_label_present = elements
            .iter()
            .any(|e| matches!(e, VisualElement::Text { text, .. } if text == "... (+1)"));
        let node_label_present = elements
            .iter()
            .any(|e| matches!(e, VisualElement::Text { text, .. } if text == "a"));
        assert!(boundary_label_present, "boundary stub label must render");
        assert!(
            node_label_present,
            "the node's own label must also render — boundary stubs don't compete for slots"
        );
    }

    #[test]
    fn compute_label_winners_excludes_offscreen_nodes_and_is_deterministic() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("onscreen", 0.0, 0.0, NodeKind::Note));
        // Far enough scene-x that scene_to_viewport places it well beyond
        // the viewport + LABEL_CULL_MARGIN_PX, at Viewport::default()'s
        // zoom/center/size.
        scene
            .nodes
            .push(test_node("offscreen", 10_000.0, 0.0, NodeKind::Note));
        let degrees = vec![0u32, 0u32];
        let style = declutter_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let positions: Vec<(f32, f32)> = scene
            .nodes
            .iter()
            .map(|n| {
                let (x, y) = mae_canvas::interaction::scene_to_viewport(&viewport, n.x, n.y);
                (x as f32, y as f32)
            })
            .collect();
        let radii: Vec<f32> = degrees
            .iter()
            .map(|&d| node_render_radius(&style, d, viewport.zoom))
            .collect();

        let winners1 =
            compute_label_winners(&scene, &positions, &radii, &degrees, &style, &viewport);
        assert!(winners1.contains(&0), "the onscreen node must win a slot");
        assert!(
            !winners1.contains(&1),
            "the offscreen node must never win a slot"
        );

        // Determinism: repeated calls on identical input produce the same set.
        let winners2 =
            compute_label_winners(&scene, &positions, &radii, &degrees, &style, &viewport);
        assert_eq!(winners1, winners2);
    }

    #[test]
    fn ease_out_cubic_endpoints_and_monotonic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert!((ease_out_cubic(1.0) - 1.0).abs() < 1e-6);
        // Monotonically increasing across the range.
        let samples = [0.0, 0.2, 0.4, 0.6, 0.8, 1.0];
        for pair in samples.windows(2) {
            assert!(ease_out_cubic(pair[0]) < ease_out_cubic(pair[1]));
        }
        // "Ease OUT" — starts fast: the first half covers MORE than half
        // the distance (unlike linear, which covers exactly half).
        assert!(ease_out_cubic(0.5) > 0.5);
    }

    #[test]
    fn lerp_hex_endpoints_and_midpoint() {
        assert_eq!(lerp_hex("#000000", "#ffffff", 0.0), "#000000");
        assert_eq!(lerp_hex("#000000", "#ffffff", 1.0), "#ffffff");
        // Midpoint of black->white is mid-gray (127 or 128, rounding-dependent).
        let mid = lerp_hex("#000000", "#ffffff", 0.5);
        assert!(mid == "#808080" || mid == "#7f7f7f");
    }

    #[test]
    fn lerp_hex_falls_back_to_to_color_on_malformed_input() {
        assert_eq!(lerp_hex("not-a-color", "#ff0000", 0.5), "#ff0000");
        assert_eq!(lerp_hex("#ff0000", "also-bad", 0.5), "also-bad");
    }

    #[test]
    fn rgb_to_hsl_and_back_round_trips_within_rounding_tolerance() {
        for (r, g, b) in [
            (255u8, 0u8, 0u8),
            (0, 255, 0),
            (0, 0, 255),
            (255, 255, 255),
            (0, 0, 0),
            (128, 128, 128),
            (204, 36, 29),  // gruvbox-dark red
            (250, 189, 47), // gruvbox-dark bright_yellow
        ] {
            let (h, s, l) = rgb_to_hsl(r, g, b);
            let (r2, g2, b2) = hsl_to_rgb(h, s, l);
            let close = |a: u8, b: u8| (a as i16 - b as i16).abs() <= 1;
            assert!(
                close(r, r2) && close(g, g2) && close(b, b2),
                "round-trip mismatch: ({r},{g},{b}) -> hsl({h},{s},{l}) -> ({r2},{g2},{b2})"
            );
        }
    }

    #[test]
    fn cap_saturation_leaves_a_color_already_at_or_below_the_cap_byte_for_byte_unchanged() {
        // #689d6a (gruvbox aqua) is ~21% saturated — well under a 55% cap.
        assert_eq!(cap_saturation("#689d6a", 0.55), "#689d6a");
    }

    #[test]
    fn cap_saturation_mutes_a_fully_saturated_color_while_preserving_hue_and_lightness() {
        let original = "#fb4934"; // gruvbox-dark bright_red, ~96% saturated
        let capped = cap_saturation(original, 0.55);
        assert_ne!(capped, original);
        let (or, og, ob) = parse_hex_rgb(original).unwrap();
        let (oh, _, ol) = rgb_to_hsl(or, og, ob);
        let (cr, cg, cb) = parse_hex_rgb(&capped).unwrap();
        let (ch, cs, cl) = rgb_to_hsl(cr, cg, cb);
        assert!(
            (oh - ch).abs() < 1.0,
            "hue must be preserved (was {oh}, now {ch})"
        );
        assert!(
            (ol - cl).abs() < 0.02,
            "lightness must be preserved (was {ol}, now {cl})"
        );
        assert!(cs <= 0.56, "saturation must be capped at ~0.55, got {cs}");
    }

    #[test]
    fn cap_saturation_at_1_0_disables_capping_entirely() {
        let original = "#fb4934";
        assert_eq!(cap_saturation(original, 1.0), original);
    }

    #[test]
    fn cap_saturation_falls_back_to_input_unchanged_on_malformed_hex() {
        assert_eq!(cap_saturation("not-a-color", 0.5), "not-a-color");
    }

    #[test]
    fn from_editor_applies_the_saturation_cap_to_every_node_selected_and_hover_color() {
        let mut editor = Editor::new();
        editor.kb_graph_node_saturation_cap = 0.3;
        let style = GraphStyleOptions::from_editor(&editor);
        for hex in style
            .node_colors
            .iter()
            .chain([&style.selected_color, &style.hover_color])
        {
            let (r, g, b) = parse_hex_rgb(hex).expect("valid hex");
            let (_, s, _) = rgb_to_hsl(r, g, b);
            assert!(
                s <= 0.31,
                "{hex} has saturation {s}, exceeding the configured 0.3 cap"
            );
        }
    }

    #[test]
    fn contrast_ratio_known_wcag_values() {
        // Black-on-white (and vice versa) is the WCAG-canonical maximum: 21:1.
        assert!((contrast_ratio("#000000", "#ffffff") - 21.0).abs() < 0.01);
        assert!((contrast_ratio("#ffffff", "#000000") - 21.0).abs() < 0.01);
        // Identical colors: minimum possible ratio, 1:1.
        assert!((contrast_ratio("#808080", "#808080") - 1.0).abs() < 0.01);
    }

    #[test]
    fn ensure_min_contrast_leaves_an_already_legible_color_unchanged() {
        // Preserving hue when it already clears the bar is the whole
        // point — this must NOT get recolored just because a threshold
        // exists.
        let fg = "#000000"; // black on white: 21:1, way above 4.5
        assert_eq!(
            ensure_min_contrast(fg, "#ffffff", WCAG_AA_TEXT_CONTRAST),
            fg
        );
    }

    #[test]
    fn ensure_min_contrast_fixes_a_genuinely_illegible_pairing() {
        // A real-world illegible case: a moderately saturated red text on
        // a near-black background — exactly the "red is hard to read"
        // shape reported live (a color tuned to read fine as a thin
        // stroke, reused verbatim as small text).
        let fg = "#8b2020"; // dark red
        let bg = "#0d0d0d"; // near-black graph background
        assert!(
            contrast_ratio(fg, bg) < WCAG_AA_TEXT_CONTRAST,
            "test setup: fg must start illegible"
        );
        let fixed = ensure_min_contrast(fg, bg, WCAG_AA_TEXT_CONTRAST);
        assert!(
            contrast_ratio(&fixed, bg) >= WCAG_AA_TEXT_CONTRAST,
            "{fixed} must meet WCAG AA contrast against {bg}"
        );
        assert_ne!(fixed, fg, "an illegible color must actually be adjusted");
    }

    #[test]
    fn ensure_min_contrast_pushes_toward_the_readable_endpoint_for_the_background() {
        // Against a light background, illegible text must be darkened
        // (toward black), not lightened (toward white, which would make
        // it WORSE) — and vice versa for a dark background.
        let light_bg = "#f0f0f0";
        let fixed_for_light = ensure_min_contrast("#cccccc", light_bg, WCAG_AA_TEXT_CONTRAST);
        assert!(
            relative_luminance(&fixed_for_light) < relative_luminance("#cccccc"),
            "against a light background, an illegible color must be darkened, not lightened"
        );

        let dark_bg = "#0d0d0d";
        let fixed_for_dark = ensure_min_contrast("#333333", dark_bg, WCAG_AA_TEXT_CONTRAST);
        assert!(
            relative_luminance(&fixed_for_dark) > relative_luminance("#333333"),
            "against a dark background, an illegible color must be lightened, not darkened"
        );
    }

    #[test]
    fn graph_color_tween_current_color_progresses_and_completes() {
        let tween = GraphColorTween {
            node_index: 0,
            from_hex: "#000000".to_string(),
            to_hex: "#ffffff".to_string(),
            started_at: std::time::Instant::now() - std::time::Duration::from_millis(1000),
            duration: std::time::Duration::from_millis(100),
        };
        // Backdated well past its duration — must report complete and
        // its current color must be (at least very close to) the target.
        assert!(tween.is_complete());
        assert_eq!(tween.current_color(), "#ffffff");
    }

    #[test]
    fn graph_color_tween_mid_flight_is_between_endpoints_and_not_complete() {
        let tween = GraphColorTween {
            node_index: 0,
            from_hex: "#000000".to_string(),
            to_hex: "#ffffff".to_string(),
            started_at: std::time::Instant::now(),
            duration: std::time::Duration::from_secs(60), // long enough not to race in CI
        };
        assert!(!tween.is_complete());
        let color = tween.current_color();
        // Very early in a long tween: should be dark, not yet at the target.
        assert_ne!(color, "#ffffff");
    }

    #[test]
    fn kind_affinity_from_strength_zero_is_none() {
        // Provable non-regression: strength <= 0.0 must reproduce the
        // pre-clustering layout exactly (LayoutConfig::default()'s own
        // `kind_affinity: None`), not just "small" multipliers.
        assert_eq!(kind_affinity_from_strength(0.0), None);
        assert_eq!(kind_affinity_from_strength(-1.0), None);
    }

    #[test]
    fn kind_affinity_from_strength_is_monotonic_in_strength() {
        // Sampled across the range, not one hand-picked value — same-kind
        // repulsion should strictly decrease and same-kind attraction
        // should strictly increase as strength rises; cross-kind
        // multipliers stay pinned at neutral (1.0) throughout.
        let samples = [0.1, 0.3, 0.5, 0.7, 1.0];
        let mut prev_repulsion = f64::MAX;
        let mut prev_attraction = f64::MIN;
        for s in samples {
            let cfg = kind_affinity_from_strength(s).unwrap();
            assert_eq!(cfg.cross_kind_repulsion, 1.0);
            assert_eq!(cfg.cross_kind_attraction, 1.0);
            assert!(
                cfg.same_kind_repulsion < prev_repulsion,
                "same_kind_repulsion should strictly decrease as strength rises"
            );
            assert!(
                cfg.same_kind_attraction > prev_attraction,
                "same_kind_attraction should strictly increase as strength rises"
            );
            prev_repulsion = cfg.same_kind_repulsion;
            prev_attraction = cfg.same_kind_attraction;
        }
    }

    #[test]
    fn graph_style_options_background_color_reads_theme_bg_not_fg() {
        // Adversarial case matching every real shipped theme file's actual
        // shape: "ui.graph.background" sets ONLY `bg` (it's a background,
        // never a foreground). Regression for the bug where
        // `background_color` was built via `theme_hex_fg` — which only
        // ever reads `.fg` — so it silently fell back to the Rust-hardcoded
        // default for every theme, always, never actually theme-driven.
        let toml = r##"
[styles]
"ui.graph.background" = { bg = "#123456" }
"##;
        let theme = crate::theme::Theme::from_toml("test", toml).unwrap();
        let mut editor = Editor::new();
        editor.theme = theme;
        let style = GraphStyleOptions::from_editor(&editor);
        assert_eq!(
            style.background_color, "#123456",
            "background_color must read the theme's bg, not fall back to the hardcoded default"
        );
    }

    #[test]
    fn graph_style_options_covers_every_node_kind() {
        // Exhaustiveness of `kind_index`'s match is compiler-enforced; this
        // guards that every index actually resolves back through
        // `color_for_kind` to the SAME slot it was written to (no
        // off-by-one / transposition bug in the parallel const arrays).
        let style = test_style();
        let all = [
            NodeKind::Index,
            NodeKind::Command,
            NodeKind::Concept,
            NodeKind::Key,
            NodeKind::Note,
            NodeKind::Project,
            NodeKind::Category,
            NodeKind::Lesson,
            NodeKind::Tutorial,
            NodeKind::Meta,
            NodeKind::Block,
            NodeKind::SchemeApi,
            NodeKind::Task,
            NodeKind::View,
        ];
        for kind in all {
            let idx = kind_index(kind);
            assert_eq!(style.color_for_kind(kind), style.node_colors[idx]);
        }
        // All 14 indices are distinct (no two kinds collide on one slot).
        let mut seen = std::collections::HashSet::new();
        for kind in all {
            assert!(
                seen.insert(kind_index(kind)),
                "duplicate index for {kind:?}"
            );
        }
    }

    #[test]
    fn text_color_for_kind_uses_the_same_index_as_color_for_kind() {
        // Locks in the array-parity invariant between node_colors and the
        // PRECOMPUTED node_text_colors (the O(n)->O(kinds) perf fix) —
        // an off-by-one here would silently show one kind's node label in
        // a completely unrelated kind's text color.
        let style = test_style();
        let all = [
            NodeKind::Index,
            NodeKind::Command,
            NodeKind::Concept,
            NodeKind::Key,
            NodeKind::Note,
            NodeKind::Project,
            NodeKind::Category,
            NodeKind::Lesson,
            NodeKind::Tutorial,
            NodeKind::Meta,
            NodeKind::Block,
            NodeKind::SchemeApi,
            NodeKind::Task,
            NodeKind::View,
        ];
        for kind in all {
            let idx = kind_index(kind);
            assert_eq!(style.text_color_for_kind(kind), style.node_text_colors[idx]);
        }
    }

    #[test]
    fn from_editor_precomputes_wcag_legible_text_colors_for_every_kind_and_state() {
        // End-to-end guard on the actual `from_editor` construction (not
        // just the hand-built `test_style()` fixture) — every precomputed
        // text color must genuinely meet WCAG AA against that same
        // editor's real graph background, for every node kind plus
        // selected/hovered/boundary.
        let editor = Editor::new();
        let style = GraphStyleOptions::from_editor(&editor);
        let all = [
            NodeKind::Index,
            NodeKind::Command,
            NodeKind::Concept,
            NodeKind::Key,
            NodeKind::Note,
            NodeKind::Project,
            NodeKind::Category,
            NodeKind::Lesson,
            NodeKind::Tutorial,
            NodeKind::Meta,
            NodeKind::Block,
            NodeKind::SchemeApi,
            NodeKind::Task,
            NodeKind::View,
        ];
        for kind in all {
            let text_color = style.text_color_for_kind(kind);
            assert!(
                contrast_ratio(text_color, &style.background_color) >= WCAG_AA_TEXT_CONTRAST,
                "{kind:?}'s precomputed text color {text_color} must meet WCAG AA against {}",
                style.background_color
            );
        }
        for (name, color) in [
            ("selected", &style.selected_text_color),
            ("hover", &style.hover_text_color),
            ("boundary", &style.boundary_edge_text_color),
        ] {
            assert!(
                contrast_ratio(color, &style.background_color) >= WCAG_AA_TEXT_CONTRAST,
                "{name}_text_color {color} must meet WCAG AA against {}",
                style.background_color
            );
        }
    }

    #[test]
    fn from_editor_reads_label_declutter_enabled() {
        let mut editor = Editor::new();
        assert!(GraphStyleOptions::from_editor(&editor).label_declutter_enabled);
        editor.kb_graph_label_declutter_enabled = false;
        assert!(!GraphStyleOptions::from_editor(&editor).label_declutter_enabled);
    }

    #[test]
    fn flatten_skips_text_below_zoom_threshold() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("a", 10.0, 20.0, NodeKind::Concept));
        let style = test_style(); // label_zoom_threshold: 0.5
        let viewport = mae_canvas::scene::Viewport {
            zoom: 0.3, // below threshold
            ..Default::default()
        };
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        // Circle only — Text is hidden, but the node stays visible/clickable.
        assert_eq!(elements.len(), 1);
        assert!(matches!(elements[0], VisualElement::Circle { .. }));
    }

    #[test]
    fn flatten_keeps_text_at_or_above_zoom_threshold() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("a", 10.0, 20.0, NodeKind::Concept));
        let style = test_style(); // label_zoom_threshold: 0.5
        let viewport = mae_canvas::scene::Viewport {
            zoom: 0.5, // exactly at threshold — boundary is inclusive
            ..Default::default()
        };
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        assert_eq!(elements.len(), 2);
        assert!(matches!(elements[1], VisualElement::Text { .. }));
    }

    #[test]
    fn flatten_culls_offscreen_node() {
        let mut scene = SceneGraph::new();
        // Viewport::default() is 800x600 centered at scene origin (0,0) —
        // a node far outside that (e.g. scene x=100000) draws way off any
        // visible screen position.
        scene
            .nodes
            .push(test_node("far", 100_000.0, 0.0, NodeKind::Concept));
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        assert!(
            elements.is_empty(),
            "a node far outside the viewport must be culled"
        );
    }

    #[test]
    fn flatten_keeps_onscreen_node_near_edge_of_viewport() {
        // Guards against an overly-aggressive cull margin: a node just
        // inside the viewport's right edge must still render.
        let mut scene = SceneGraph::new();
        // Viewport::default() width=800, centered at scene x=0 -> screen
        // x=400 is the middle; scene x=390 draws near screen x=790, just
        // inside the 800px-wide viewport.
        scene
            .nodes
            .push(test_node("near_edge", 390.0, 0.0, NodeKind::Concept));
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        assert_eq!(
            elements.len(),
            2,
            "a node just inside the viewport edge must not be culled"
        );
    }

    #[test]
    fn flatten_culls_offscreen_edge_when_both_endpoints_are_offscreen_same_side() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("a", 100_000.0, 0.0, NodeKind::Concept));
        scene
            .nodes
            .push(test_node("b", 100_100.0, 0.0, NodeKind::Concept));
        scene.edges.push(test_edge(0, 1));
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        assert!(
            elements.is_empty(),
            "an edge whose both endpoints are offscreen on the same side must be culled \
             (and both its now-offscreen nodes too)"
        );
    }

    #[test]
    fn flatten_keeps_edge_that_crosses_the_viewport_even_if_both_endpoints_are_offscreen() {
        // Regression guard for the conservative same-side-only rule: a
        // long edge spanning from far off the left to far off the right
        // must NOT be culled, even though neither endpoint is itself
        // on-screen — it visibly crosses the viewport.
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("left", -100_000.0, 0.0, NodeKind::Concept));
        scene
            .nodes
            .push(test_node("right", 100_000.0, 0.0, NodeKind::Concept));
        scene.edges.push(test_edge(0, 1));
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        assert!(
            elements
                .iter()
                .any(|e| matches!(e, VisualElement::Line { .. })),
            "an edge crossing the viewport must be kept even with both endpoints offscreen"
        );
    }

    #[test]
    fn describe_state_is_unaffected_by_culling_or_lod() {
        // Culling/LOD only affects the flattened VisualElement render
        // list — scene-level introspection (selection/hover/topology)
        // must be completely unaffected by an off-screen, low-zoom scene.
        let mut gv = GraphView::new();
        gv.scene
            .nodes
            .push(test_node("far", 100_000.0, 0.0, NodeKind::Concept));
        gv.scene.selection = Some(0);
        let state = gv.describe_state();
        assert_eq!(state.selected_node.as_deref(), Some("far"));
        assert_eq!(state.nodes.len(), 1);
        assert!(state.nodes[0].selected);
    }

    #[test]
    fn flatten_empty_scene_produces_no_elements() {
        let scene = SceneGraph::new();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &test_style(), &[]);
        assert!(elements.is_empty());
    }

    #[test]
    fn flatten_single_node_produces_circle_and_text() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("concept:buffer", 10.0, 20.0, NodeKind::Concept));
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        assert_eq!(elements.len(), 2);
        // Default `Viewport` (center 0,0, zoom 1.0, 800x600) is NOT an
        // identity transform — `scene_to_viewport` centers the origin in
        // the middle of the viewport, so a scene point (10, 20) draws at
        // (10 + width/2, 20 + height/2), matching `graph_scene_point`'s
        // inverse used by hit-testing.
        let (expected_cx, expected_cy) =
            mae_canvas::interaction::scene_to_viewport(&viewport, 10.0, 20.0);
        match &elements[0] {
            VisualElement::Circle {
                cx, cy, r, fill, ..
            } => {
                assert_eq!(*cx, expected_cx as f32);
                assert_eq!(*cy, expected_cy as f32);
                assert_eq!(*r, style.node_radius);
                assert_eq!(
                    fill.as_deref(),
                    Some(style.color_for_kind(NodeKind::Concept))
                );
            }
            other => panic!("expected Circle, got {other:?}"),
        }
        match &elements[1] {
            VisualElement::Text {
                text, font_size, ..
            } => {
                assert_eq!(text, "concept:buffer");
                assert_eq!(*font_size, style.font_size);
            }
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn flatten_selected_node_uses_selected_color() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 100.0, 0.0, NodeKind::Note));
        scene.selection = Some(1);
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        // Node 0 (unselected) circle first, then node 1 (selected) circle.
        match &elements[0] {
            VisualElement::Circle { fill, .. } => {
                assert_eq!(fill.as_deref(), Some(style.color_for_kind(NodeKind::Note)));
            }
            other => panic!("expected Circle, got {other:?}"),
        }
        match &elements[2] {
            VisualElement::Circle { fill, .. } => {
                assert_eq!(fill.as_deref(), Some(style.selected_color.as_str()));
            }
            other => panic!("expected Circle, got {other:?}"),
        }
    }

    #[test]
    fn flatten_hovered_node_uses_hover_color() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 100.0, 0.0, NodeKind::Note));
        scene.hovered = Some(1);
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        match &elements[0] {
            VisualElement::Circle { fill, .. } => {
                assert_eq!(fill.as_deref(), Some(style.color_for_kind(NodeKind::Note)));
            }
            other => panic!("expected Circle, got {other:?}"),
        }
        match &elements[2] {
            VisualElement::Circle { fill, .. } => {
                assert_eq!(fill.as_deref(), Some(style.hover_color.as_str()));
            }
            other => panic!("expected Circle, got {other:?}"),
        }
    }

    #[test]
    fn flatten_selected_and_hovered_same_node_prefers_selected_color() {
        // Regression guard: a future edit that reorders the if/else-if
        // priority in `flatten_scene_graph` must not silently make hover
        // outrank a deliberate selection.
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.selection = Some(0);
        scene.hovered = Some(0);
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        match &elements[0] {
            VisualElement::Circle { fill, .. } => {
                assert_eq!(fill.as_deref(), Some(style.selected_color.as_str()));
            }
            other => panic!("expected Circle, got {other:?}"),
        }
    }

    #[test]
    fn flatten_internal_edge_is_solid() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 50.0, 0.0, NodeKind::Note));
        scene.edges.push(SceneEdge {
            source: 0,
            target: 1,
            label: None,
            style: EdgeStyle::default(),
            weight: 1.0,
            rel_type: None,
        });
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        // Same non-identity default-viewport transform as `flatten_single_
        // node_produces_circle_and_text` — target node (50, 0) draws at
        // (50 + width/2, 0 + height/2).
        let (expected_x2, expected_y2) =
            mae_canvas::interaction::scene_to_viewport(&viewport, 50.0, 0.0);
        match &elements[0] {
            VisualElement::Line {
                dashed,
                color,
                x2,
                y2,
                ..
            } => {
                assert!(!dashed);
                assert_eq!(color, &style.edge_color);
                assert_eq!(*x2, expected_x2 as f32);
                assert_eq!(*y2, expected_y2 as f32);
            }
            other => panic!("expected Line, got {other:?}"),
        }
    }

    #[test]
    fn flatten_curves_internal_edges_when_curvature_is_nonzero() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("a", -100.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 100.0, 0.0, NodeKind::Note));
        scene.edges.push(test_edge(0, 1));
        let mut style = test_style();
        style.edge_curvature = 0.2;
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        match &elements[0] {
            VisualElement::Curve {
                ctrl_x,
                ctrl_y,
                x1,
                y1,
                x2,
                y2,
                ..
            } => {
                // Control point must be offset from the straight-line
                // midpoint (a curve, not a disguised straight line).
                let mid_x = (x1 + x2) / 2.0;
                let mid_y = (y1 + y2) / 2.0;
                assert!(
                    (ctrl_x - mid_x).abs() > 0.01 || (ctrl_y - mid_y).abs() > 0.01,
                    "control point ({ctrl_x}, {ctrl_y}) must be offset from the midpoint \
                     ({mid_x}, {mid_y})"
                );
            }
            other => panic!("expected Curve, got {other:?}"),
        }
    }

    #[test]
    fn flatten_curve_offset_direction_alternates_by_source_target_parity() {
        // Two otherwise-identical edges differing only in (source+target)
        // parity must curve in OPPOSITE perpendicular directions, so
        // parallel/adjacent edges bow apart instead of overlapping.
        let mut scene = SceneGraph::new();
        for i in 0..4 {
            scene
                .nodes
                .push(test_node(&format!("n{i}"), 0.0, 0.0, NodeKind::Note));
        }
        scene.edges.push(test_edge(0, 1)); // sum = 1 (odd)
        scene.edges.push(test_edge(0, 3)); // sum = 3 (odd) — same parity as above
        scene.edges.push(test_edge(0, 2)); // sum = 2 (even) — opposite parity
        let mut style = test_style();
        style.edge_curvature = 0.2;
        let viewport = mae_canvas::scene::Viewport::default();

        // All three edges share the same endpoints geometrically (every
        // node is at the scene origin here), so any ctrl_y sign difference
        // is purely from the parity-based sign flip, not edge geometry.
        // Reposition node "b"/"c"/"d" so the edge actually has nonzero
        // length (a zero-length edge has an undefined perpendicular).
        scene.nodes[1].x = 100.0; // b
        scene.nodes[3].x = 100.0; // d (same position as b)
        scene.nodes[2].x = 100.0; // c (same position as b)
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);

        let ctrl_y_of = |el: &VisualElement| match el {
            VisualElement::Curve { ctrl_y, .. } => *ctrl_y,
            other => panic!("expected Curve, got {other:?}"),
        };
        let odd_a = ctrl_y_of(&elements[0]); // 0-1, sum=1
        let odd_b = ctrl_y_of(&elements[1]); // 0-3, sum=3
        let even = ctrl_y_of(&elements[2]); // 0-2, sum=2

        // Same parity (both odd sums) -> same side.
        assert!(
            (odd_a - 300.0).signum() == (odd_b - 300.0).signum(),
            "same-parity edges should curve to the same side"
        );
        // Opposite parity -> opposite side (300.0 is the straight-line y,
        // since Viewport::default() centers scene y=0 at screen y=height/2=300).
        assert_ne!(
            (odd_a - 300.0).signum(),
            (even - 300.0).signum(),
            "opposite-parity edges should curve to opposite sides"
        );
    }

    /// #367: a Chord-mode edge's control point must be pulled toward the
    /// circle's (viewport-transformed) center, meaningfully DIFFERENT from
    /// the Force-mode perpendicular-offset formula the tests above cover —
    /// not just "some offset exists" (already proven for Force), but the
    /// actual geometric direction differs between the two modes.
    #[test]
    fn flatten_chord_mode_pulls_control_point_toward_viewport_space_scene_origin() {
        let mut scene = SceneGraph::new();
        // Two nodes NOT diametrically opposite (so their straight-line
        // midpoint is offset from the scene origin) — as if placed on a
        // circle of radius 100 at 0 and 90 degrees.
        scene.nodes.push(test_node("a", 100.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 0.0, 100.0, NodeKind::Note));
        scene.edges.push(test_edge(0, 1));
        let viewport = mae_canvas::scene::Viewport::default();
        let (origin_x, origin_y) = mae_canvas::interaction::scene_to_viewport(&viewport, 0.0, 0.0);

        let mut chord_style = test_style();
        chord_style.edge_curvature = 0.5;
        chord_style.layout_algorithm = GraphLayoutAlgorithm::Chord;
        let chord_elements = flatten_scene_graph(&scene, &viewport, &chord_style, &[]);

        let mut force_style = test_style();
        force_style.edge_curvature = 0.5;
        force_style.layout_algorithm = GraphLayoutAlgorithm::Force;
        let force_elements = flatten_scene_graph(&scene, &viewport, &force_style, &[]);

        let (chord_ctrl, chord_mid) = match &chord_elements[0] {
            VisualElement::Curve {
                ctrl_x,
                ctrl_y,
                x1,
                y1,
                x2,
                y2,
                ..
            } => (
                (*ctrl_x as f64, *ctrl_y as f64),
                ((x1 + x2) as f64 / 2.0, (y1 + y2) as f64 / 2.0),
            ),
            other => panic!("expected Curve, got {other:?}"),
        };
        let force_ctrl = match &force_elements[0] {
            VisualElement::Curve { ctrl_x, ctrl_y, .. } => (*ctrl_x as f64, *ctrl_y as f64),
            other => panic!("expected Curve, got {other:?}"),
        };

        let dist =
            |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();

        // Chord's control point must be measurably CLOSER to the scene
        // origin than the plain straight-line midpoint is.
        let mid_to_origin = dist(chord_mid, (origin_x, origin_y));
        let chord_ctrl_to_origin = dist(chord_ctrl, (origin_x, origin_y));
        assert!(
            chord_ctrl_to_origin < mid_to_origin - 1.0,
            "chord control point ({chord_ctrl:?}) must be closer to origin ({origin_x}, \
             {origin_y}) than the midpoint ({chord_mid:?}) is (mid dist {mid_to_origin}, \
             ctrl dist {chord_ctrl_to_origin})"
        );

        // The two algorithms must produce genuinely different control
        // points for the identical edge geometry — not the same formula
        // wearing a different enum tag.
        assert!(
            dist(chord_ctrl, force_ctrl) > 1.0,
            "chord ({chord_ctrl:?}) and force ({force_ctrl:?}) control points must differ"
        );
    }

    /// Same as above, but with a PANNED viewport (`center_x`/`center_y` !=
    /// 0) — proves the center-pull goes through `scene_to_viewport` like
    /// every node position does, rather than pulling toward a raw `(0,0)`
    /// in screen space (which would be wrong once the view has been panned).
    #[test]
    fn flatten_chord_mode_center_pull_accounts_for_a_panned_viewport() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 100.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 0.0, 100.0, NodeKind::Note));
        scene.edges.push(test_edge(0, 1));
        let viewport = mae_canvas::scene::Viewport {
            center_x: 500.0,
            center_y: -300.0,
            ..mae_canvas::scene::Viewport::default()
        };
        let (origin_x, origin_y) = mae_canvas::interaction::scene_to_viewport(&viewport, 0.0, 0.0);
        // Sanity: a real pan actually moves where scene-origin lands.
        assert_ne!((origin_x, origin_y), (400.0, 300.0));

        let mut style = test_style();
        style.edge_curvature = 0.5;
        style.layout_algorithm = GraphLayoutAlgorithm::Chord;
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        let (ctrl, mid) = match &elements[0] {
            VisualElement::Curve {
                ctrl_x,
                ctrl_y,
                x1,
                y1,
                x2,
                y2,
                ..
            } => (
                (*ctrl_x as f64, *ctrl_y as f64),
                ((x1 + x2) as f64 / 2.0, (y1 + y2) as f64 / 2.0),
            ),
            other => panic!("expected Curve, got {other:?}"),
        };
        let dist =
            |a: (f64, f64), b: (f64, f64)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
        assert!(
            dist(ctrl, (origin_x, origin_y)) < dist(mid, (origin_x, origin_y)) - 1.0,
            "must still pull toward the PANNED viewport-space origin, not the unpanned one"
        );
    }

    #[test]
    fn flatten_curve_offset_is_capped_at_max_curve_offset_px() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("a", -10_000.0, 0.0, NodeKind::Note));
        scene
            .nodes
            .push(test_node("b", 10_000.0, 0.0, NodeKind::Note));
        scene.edges.push(test_edge(0, 1));
        let mut style = test_style();
        style.edge_curvature = 0.9; // large fraction of a very long edge
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        match &elements[0] {
            VisualElement::Curve {
                ctrl_x,
                ctrl_y,
                x1,
                y1,
                x2,
                y2,
                ..
            } => {
                let mid_x = (x1 + x2) / 2.0;
                let mid_y = (y1 + y2) / 2.0;
                let offset = ((ctrl_x - mid_x).powi(2) + (ctrl_y - mid_y).powi(2)).sqrt();
                assert!(
                    offset <= MAX_CURVE_OFFSET_PX + 0.01,
                    "offset {offset} must respect the MAX_CURVE_OFFSET_PX cap"
                );
            }
            other => panic!("expected Curve, got {other:?}"),
        }
    }

    #[test]
    fn flatten_boundary_edges_stay_straight_even_with_curvature_enabled() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.edges.push(SceneEdge {
            source: 0,
            target: 0, // self-loop = boundary indicator
            label: Some("...".to_string()),
            style: EdgeStyle {
                color: "#unused".to_string(),
                width: 1.0,
                dashed: true,
            },
            weight: 1.0,
            rel_type: None,
        });
        let mut style = test_style();
        style.edge_curvature = 0.5;
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        assert!(
            matches!(elements[0], VisualElement::Line { .. }),
            "a boundary/self-loop stub edge must stay a straight (dashed) Line even when \
             edge_curvature is enabled"
        );
    }

    #[test]
    fn flatten_boundary_edge_is_dashed_and_uses_boundary_color() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.edges.push(SceneEdge {
            source: 0,
            target: 0, // self-loop = boundary indicator, per build_kb_graph
            label: Some("...".to_string()),
            style: EdgeStyle {
                color: "#unused".to_string(),
                width: 1.0,
                dashed: true,
            },
            weight: 1.0,
            rel_type: None,
        });
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        match &elements[0] {
            VisualElement::Line { dashed, color, .. } => {
                assert!(dashed);
                assert_eq!(color, &style.boundary_edge_color);
            }
            other => panic!("expected Line, got {other:?}"),
        }
        // Regression: a boundary stub's label ("...", "... (+N)") is real
        // data (surfaced via describe_state()) that was never actually
        // drawn anywhere in the GUI — confusing "what are these red lines
        // for" with no visible explanation. It must now render as Text
        // right after the stub Line, contrast-adjusted for legibility
        // (test_style()'s placeholder colors aren't real hex, so
        // ensure_min_contrast falls back to pure white/black here — the
        // point of this assertion is that a WCAG-legible color comes out,
        // not that it equals the raw boundary_edge_color verbatim).
        match &elements[1] {
            VisualElement::Text { text, color, .. } => {
                assert_eq!(text, "...");
                assert!(
                    contrast_ratio(color, &style.background_color) >= WCAG_AA_TEXT_CONTRAST,
                    "boundary stub label color {color} must meet WCAG AA contrast against \
                     the graph background"
                );
            }
            other => panic!("expected Text (the boundary stub's label), got {other:?}"),
        }
    }

    #[test]
    fn flatten_boundary_stub_offset_scales_with_the_source_nodes_real_render_radius() {
        // Regression: the stub used to be sized off the flat, degree/zoom-
        // unaware `style.node_radius` base — once node size started
        // varying (Phase C), a shrunk node's stub stuck out
        // disproportionately far relative to its own smaller circle. The
        // stub must scale with the SAME per-node radius the node's own
        // Circle uses.
        fn stub_endpoint(style: &GraphStyleOptions, degrees: &[u32]) -> (f32, f32) {
            let mut scene = SceneGraph::new();
            scene.nodes.push(test_node("hub", 0.0, 0.0, NodeKind::Note));
            scene.edges.push(SceneEdge {
                source: 0,
                target: 0,
                label: Some("...".to_string()),
                style: EdgeStyle {
                    color: "#unused".to_string(),
                    width: 1.0,
                    dashed: true,
                },
                weight: 1.0,
                rel_type: None,
            });
            let viewport = mae_canvas::scene::Viewport::default();
            let elements = flatten_scene_graph(&scene, &viewport, style, degrees);
            match &elements[0] {
                VisualElement::Line { x2, y2, .. } => (*x2, *y2),
                other => panic!("expected Line, got {other:?}"),
            }
        }

        let mut style = test_style();
        style.size_scales_with_zoom = false; // isolate the degree term
        let (low_degree_x, _) = stub_endpoint(&style, &[0]);
        let (high_degree_x, _) = stub_endpoint(&style, &[40]);
        assert!(
            high_degree_x > low_degree_x,
            "a bigger (higher-degree) node's stub must reach farther out \
             (low={low_degree_x}, high={high_degree_x})"
        );
    }

    #[test]
    fn flatten_skips_edge_with_out_of_range_source() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.edges.push(SceneEdge {
            source: 5, // out of range — must not panic
            target: 0,
            label: None,
            style: EdgeStyle::default(),
            weight: 1.0,
            rel_type: None,
        });
        let style = test_style();
        let viewport = mae_canvas::scene::Viewport::default();
        let elements = flatten_scene_graph(&scene, &viewport, &style, &[]);
        // Only the node's circle+text — the bogus edge is skipped.
        assert_eq!(elements.len(), 2);
    }

    #[test]
    fn describe_state_with_no_nodes_has_no_selected_or_hovered() {
        let mut gv = GraphView::new();
        gv.center_node = Some("concept:buffer".to_string());
        gv.depth = 2;
        let state = gv.describe_state();
        assert_eq!(state.center_node.as_deref(), Some("concept:buffer"));
        assert_eq!(state.depth, 2);
        assert_eq!(state.selected_node, None);
        assert_eq!(state.hovered_node, None);
        assert!(state.nodes.is_empty());
        assert!(state.edges.is_empty());
    }

    #[test]
    fn describe_state_resolves_nodes_edges_selected_and_hovered_to_real_ids() {
        let mut gv = GraphView::new();
        gv.center_node = Some("concept:a".to_string());
        gv.scene
            .nodes
            .push(test_node("concept:a", 0.0, 0.0, NodeKind::Concept));
        gv.scene
            .nodes
            .push(test_node("concept:b", 50.0, 0.0, NodeKind::Note));
        gv.scene.edges.push(SceneEdge {
            source: 0,
            target: 1,
            label: Some("relates-to".to_string()),
            style: EdgeStyle::default(),
            weight: 1.0,
            rel_type: None,
        });
        gv.scene.selection = Some(0);
        gv.scene.hovered = Some(1);

        let state = gv.describe_state();

        assert_eq!(state.selected_node.as_deref(), Some("concept:a"));
        assert_eq!(state.hovered_node.as_deref(), Some("concept:b"));
        assert_eq!(state.nodes.len(), 2);
        assert_eq!(state.nodes[0].id, "concept:a");
        assert!(state.nodes[0].selected);
        assert!(!state.nodes[0].hovered);
        assert_eq!(state.nodes[1].id, "concept:b");
        assert!(!state.nodes[1].selected);
        assert!(state.nodes[1].hovered);
        assert_eq!(state.edges.len(), 1);
        assert_eq!(state.edges[0].source_id, "concept:a");
        assert_eq!(state.edges[0].target_id, "concept:b");
        assert!(!state.edges[0].boundary);
        assert_eq!(state.edges[0].label.as_deref(), Some("relates-to"));
    }

    #[test]
    fn describe_state_marks_dashed_self_loop_edges_as_boundary() {
        let mut gv = GraphView::new();
        gv.scene
            .nodes
            .push(test_node("concept:a", 0.0, 0.0, NodeKind::Concept));
        gv.scene.edges.push(SceneEdge {
            source: 0,
            target: 0,
            label: Some("...".to_string()),
            style: EdgeStyle {
                color: "#unused".to_string(),
                width: 1.0,
                dashed: true,
            },
            weight: 1.0,
            rel_type: None,
        });

        let state = gv.describe_state();

        assert_eq!(state.edges.len(), 1);
        assert!(state.edges[0].boundary);
    }

    #[test]
    fn describe_state_skips_edges_with_out_of_range_endpoints() {
        let mut gv = GraphView::new();
        gv.scene
            .nodes
            .push(test_node("concept:a", 0.0, 0.0, NodeKind::Concept));
        gv.scene.edges.push(SceneEdge {
            source: 5, // out of range — must not panic, must be skipped
            target: 0,
            label: None,
            style: EdgeStyle::default(),
            weight: 1.0,
            rel_type: None,
        });

        let state = gv.describe_state();

        assert!(state.edges.is_empty());
    }
}
