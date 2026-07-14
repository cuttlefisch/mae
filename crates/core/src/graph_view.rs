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

use crate::driven_window::DrivenWindow;
use crate::editor::Editor;
use crate::visual_buffer::{VisualBuffer, VisualElement};
use crate::window::WindowId;
use std::collections::HashMap;

/// View state for a `BufferKind::Graph` buffer.
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
}

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
pub const ANIMATION_TEMPERATURE_FLOOR: f64 = 1.0;
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
    /// Hex fill color per canvas `NodeKind`, indexed via `kind_index`.
    node_colors: [String; 14],
    pub selected_color: String,
    /// Color for the node currently under the mouse cursor (real-time
    /// hover — see `mae_canvas::scene::SceneGraph.hovered`). Loses priority
    /// to `selected_color` when a node is both selected and hovered.
    pub hover_color: String,
    pub edge_color: String,
    pub boundary_edge_color: String,
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
    pub fn from_editor(editor: &Editor) -> Self {
        let mut node_colors: [String; 14] = Default::default();
        for i in 0..14 {
            node_colors[i] =
                theme_hex_fg(editor, NODE_KIND_THEME_KEYS[i], NODE_KIND_FALLBACK_HEX[i]);
        }
        GraphStyleOptions {
            node_radius: editor.kb_graph_node_radius as f32,
            font_size: editor.kb_graph_font_size as f32,
            size_by_degree: editor.kb_graph_node_size_by_degree,
            node_degree_scale: editor.kb_graph_node_degree_scale,
            size_scales_with_zoom: editor.kb_graph_node_size_scales_with_zoom,
            node_min_radius: editor.kb_graph_node_min_radius as f32,
            node_max_radius: editor.kb_graph_node_max_radius as f32,
            label_zoom_threshold: editor.kb_graph_label_zoom_threshold,
            node_colors,
            selected_color: theme_hex_fg(editor, "ui.graph.node.selected", "#ff9933"),
            hover_color: theme_hex_fg(editor, "ui.graph.node.hover", "#66ccff"),
            edge_color: theme_hex_fg(editor, "ui.graph.edge", "#6a6d7e"),
            boundary_edge_color: theme_hex_fg(editor, "ui.graph.edge.boundary", "#ff6666"),
            background_color: theme_hex_bg(editor, "ui.graph.background", "#0d0d0d"),
        }
    }

    fn color_for_kind(&self, kind: mae_canvas::scene::NodeKind) -> &str {
        &self.node_colors[kind_index(kind)]
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
        r *= (zoom.max(0.0) as f32).sqrt();
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

/// Whether an edge's two screen-space endpoints are BOTH off-screen on the
/// SAME side — the only case it's safe to cull. Deliberately conservative:
/// a long edge with both endpoints off-screen on DIFFERENT sides might
/// still cross the visible viewport, so it's never culled by this check
/// (no full segment-vs-rect test — that correctness margin is worth more
/// than the extra draw calls saved).
fn edge_is_offscreen_same_side(
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    viewport: &mae_canvas::scene::Viewport,
) -> bool {
    let w = viewport.width as f32;
    let h = viewport.height as f32;
    (x1 < 0.0 && x2 < 0.0) || (x1 > w && x2 > w) || (y1 < 0.0 && y2 < 0.0) || (y1 > h && y2 > h)
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
        // A boundary edge is represented as a self-loop (source == target,
        // see `build_kb_graph`) — draw a short stub off to the side instead
        // of a zero-length line, so it's visually distinguishable. The stub
        // offset is applied in screen space (after transforming the source
        // node), so it stays a constant visual size regardless of zoom.
        let (sx2, sy2) = if edge.target < scene.nodes.len() && edge.target != edge.source {
            let t = &scene.nodes[edge.target];
            scene_to_viewport(viewport, t.x, t.y)
        } else {
            (
                sx1 + (style.node_radius * 2.0) as f64,
                sy1 - style.node_radius as f64,
            )
        };
        if edge_is_offscreen_same_side(sx1 as f32, sy1 as f32, sx2 as f32, sy2 as f32, viewport) {
            continue;
        }
        elements.push(VisualElement::Line {
            x1: sx1 as f32,
            y1: sy1 as f32,
            x2: sx2 as f32,
            y2: sy2 as f32,
            color,
            thickness: edge.style.width as f32,
            dashed: is_boundary,
        });
    }

    for (i, node) in scene.nodes.iter().enumerate() {
        let is_selected = scene.selection == Some(i);
        let is_hovered = scene.hovered == Some(i);
        // Selected always wins over hovered when both target the same
        // node — a deliberate action (click/keyboard-select) outranks
        // incidental mouse position.
        let color = if is_selected {
            style.selected_color.clone()
        } else if is_hovered {
            style.hover_color.clone()
        } else {
            style.color_for_kind(node.kind).to_string()
        };
        let (scx, scy) = scene_to_viewport(viewport, node.x, node.y);
        let degree = degrees.get(i).copied().unwrap_or(0);
        let r = node_render_radius(style, degree, viewport.zoom);
        if node_is_offscreen(scx as f32, scy as f32, r, viewport) {
            continue;
        }
        elements.push(VisualElement::Circle {
            cx: scx as f32,
            cy: scy as f32,
            r,
            fill: Some(color.clone()),
            stroke: Some(style.edge_color.clone()),
        });
        // Label LOD: below the configured zoom threshold, skip the Text
        // element to reduce clutter/draw calls on dense graphs — the
        // Circle above is ALWAYS pushed, so the node stays visible and
        // clickable regardless.
        if viewport.zoom >= style.label_zoom_threshold as f64 {
            elements.push(VisualElement::Text {
                x: scx as f32 + r + 4.0,
                y: scy as f32,
                text: node.label.clone(),
                font_size: style.font_size,
                color,
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
            node_min_radius: 4.0,
            node_max_radius: 36.0,
            // Below Viewport::default()'s zoom (1.0), so every pre-Phase-D
            // test here (all using the default viewport) keeps pushing
            // Text elements unmodified.
            label_zoom_threshold: 0.5,
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
            selected_color: "#selected".into(),
            hover_color: "#hovered".into(),
            edge_color: "#edge".into(),
            boundary_edge_color: "#boundary".into(),
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
