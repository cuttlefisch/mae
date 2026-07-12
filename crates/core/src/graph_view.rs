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
    /// The `mae-canvas` scene graph — nodes, edges, viewport, selection.
    pub scene: mae_canvas::scene::SceneGraph,
    /// "The window this actor is driving" (Part A `DrivenWindow`), captured
    /// reactively via `follow_focus_away_from` every time editor focus
    /// changes to a window other than the graph's own — see
    /// `Editor::capture_graph_companion_focus`. Used by
    /// `kb-graph-view-select-current` to navigate the *other* (previously
    /// focused) window to the selected node's KB buffer, bypassing
    /// `display_buffer`'s reuse-or-split policy.
    pub companion_window: DrivenWindow,
    /// `scene` flattened into `VisualElement`s for the GUI's
    /// `render_visual_buffer` pipeline (`flatten_scene_graph`). Rebuilt by
    /// `graph_view_ops.rs` on every open/refresh/navigate/layout-applied so
    /// the GUI render path never needs to know about `SceneGraph` at all —
    /// it just draws `rendered` exactly like a `BufferKind::Visual` buffer.
    pub rendered: VisualBuffer,
}

impl GraphView {
    pub fn new() -> Self {
        GraphView {
            center_node: None,
            depth: 2,
            kb_instance: None,
            follow_current: true,
            scene: mae_canvas::scene::SceneGraph::new(),
            companion_window: DrivenWindow::none(),
            rendered: VisualBuffer::new(),
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
    pub iterations: usize,
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
    pub node_radius: f32,
    pub font_size: f32,
    /// Hex fill color per canvas `NodeKind`, indexed via `kind_index`.
    node_colors: [String; 14],
    pub selected_color: String,
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
            node_colors,
            selected_color: theme_hex_fg(editor, "ui.graph.node.selected", "#ff9933"),
            edge_color: theme_hex_fg(editor, "ui.graph.edge", "#6a6d7e"),
            boundary_edge_color: theme_hex_fg(editor, "ui.graph.edge.boundary", "#ff6666"),
            background_color: theme_hex_fg(editor, "ui.graph.background", "#0d0d0d"),
        }
    }

    fn color_for_kind(&self, kind: mae_canvas::scene::NodeKind) -> &str {
        &self.node_colors[kind_index(kind)]
    }
}

/// Flatten a `mae-canvas` `SceneGraph` into `VisualElement`s for the GUI's
/// `render_visual_buffer` pipeline. Edges are emitted before nodes (drawn
/// under them); boundary edges (`SceneEdge.style.dashed`, the subgraph
/// fringe — see `mae_canvas::kb_graph::build_kb_graph`) render as dashed
/// lines using `style.boundary_edge_color`, internal edges as solid lines
/// using `style.edge_color`. Nodes render as a themed circle (selected node
/// uses `style.selected_color`, others their `NodeKind`'s themed color) plus
/// a label `Text` element. Pure function — no `Editor`/theme access, so it's
/// independently unit-testable against a hand-built `SceneGraph` +
/// `GraphStyleOptions`.
pub fn flatten_scene_graph(
    scene: &mae_canvas::scene::SceneGraph,
    style: &GraphStyleOptions,
) -> Vec<VisualElement> {
    let mut elements = Vec::with_capacity(scene.edges.len() + scene.nodes.len() * 2);

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
        // A boundary edge is represented as a self-loop (source == target,
        // see `build_kb_graph`) — draw a short stub off to the side instead
        // of a zero-length line, so it's visually distinguishable.
        let (x2, y2) = if edge.target < scene.nodes.len() && edge.target != edge.source {
            let t = &scene.nodes[edge.target];
            (t.x as f32, t.y as f32)
        } else {
            (
                src.x as f32 + style.node_radius * 2.0,
                src.y as f32 - style.node_radius,
            )
        };
        elements.push(VisualElement::Line {
            x1: src.x as f32,
            y1: src.y as f32,
            x2,
            y2,
            color,
            thickness: edge.style.width as f32,
            dashed: is_boundary,
        });
    }

    for (i, node) in scene.nodes.iter().enumerate() {
        let is_selected = scene.selection == Some(i);
        let color = if is_selected {
            style.selected_color.clone()
        } else {
            style.color_for_kind(node.kind).to_string()
        };
        elements.push(VisualElement::Circle {
            cx: node.x as f32,
            cy: node.y as f32,
            r: style.node_radius,
            fill: Some(color.clone()),
            stroke: Some(style.edge_color.clone()),
        });
        elements.push(VisualElement::Text {
            x: node.x as f32 + style.node_radius + 4.0,
            y: node.y as f32,
            text: node.label.clone(),
            font_size: style.font_size,
            color,
        });
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
    fn flatten_empty_scene_produces_no_elements() {
        let scene = SceneGraph::new();
        let elements = flatten_scene_graph(&scene, &test_style());
        assert!(elements.is_empty());
    }

    #[test]
    fn flatten_single_node_produces_circle_and_text() {
        let mut scene = SceneGraph::new();
        scene
            .nodes
            .push(test_node("concept:buffer", 10.0, 20.0, NodeKind::Concept));
        let style = test_style();
        let elements = flatten_scene_graph(&scene, &style);
        assert_eq!(elements.len(), 2);
        match &elements[0] {
            VisualElement::Circle {
                cx, cy, r, fill, ..
            } => {
                assert_eq!(*cx, 10.0);
                assert_eq!(*cy, 20.0);
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
        let elements = flatten_scene_graph(&scene, &style);
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
    fn flatten_internal_edge_is_solid() {
        let mut scene = SceneGraph::new();
        scene.nodes.push(test_node("a", 0.0, 0.0, NodeKind::Note));
        scene.nodes.push(test_node("b", 50.0, 0.0, NodeKind::Note));
        scene.edges.push(SceneEdge {
            source: 0,
            target: 1,
            label: None,
            style: EdgeStyle::default(),
        });
        let style = test_style();
        let elements = flatten_scene_graph(&scene, &style);
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
                assert_eq!(*x2, 50.0);
                assert_eq!(*y2, 0.0);
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
        });
        let style = test_style();
        let elements = flatten_scene_graph(&scene, &style);
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
        });
        let style = test_style();
        let elements = flatten_scene_graph(&scene, &style);
        // Only the node's circle+text — the bogus edge is skipped.
        assert_eq!(elements.len(), 2);
    }
}
