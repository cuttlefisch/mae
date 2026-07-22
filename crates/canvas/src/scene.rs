//! Scene graph — nodes, edges, and selection state. Viewport (pan/zoom/
//! pixel-size) is intentionally NOT part of `SceneGraph` — see its doc
//! comment.

use serde::{Deserialize, Serialize};

/// A complete scene graph with positioned nodes and edges.
///
/// Deliberately has NO `Viewport` field — pan/zoom/pixel-size is per-WINDOW
/// state (a graph buffer can be shown in more than one split, each at its
/// own size/zoom), not per-graph. Callers thread a `&Viewport` through
/// explicitly wherever a scene needs projecting to pixels (see
/// `flatten_scene_graph`, `mae_canvas::interaction::*`). `GraphView.viewports:
/// HashMap<WindowId, Viewport>` (`crates/core/src/graph_view.rs`) is the
/// actual per-window store. See issue #321.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneGraph {
    pub nodes: Vec<SceneNode>,
    pub edges: Vec<SceneEdge>,
    pub selection: Option<usize>,
    /// Index into `nodes` of the node currently under the mouse cursor
    /// (real-time hover, distinct from `selection` which is click/keyboard-
    /// driven). `None` when nothing is hovered or the cursor is off the
    /// graph entirely.
    pub hovered: Option<usize>,
}

impl SceneGraph {
    /// Create an empty scene graph.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            selection: None,
            hovered: None,
        }
    }

    /// Get the selected node, if any.
    pub fn selected_node(&self) -> Option<&SceneNode> {
        self.selection.and_then(|i| self.nodes.get(i))
    }

    /// Get the hovered node, if any.
    pub fn hovered_node(&self) -> Option<&SceneNode> {
        self.hovered.and_then(|i| self.nodes.get(i))
    }
}

impl Default for SceneGraph {
    fn default() -> Self {
        Self::new()
    }
}

/// A node in the scene graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneNode {
    pub id: String,
    pub label: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub kind: NodeKind,
    pub style: NodeStyle,
    pub pinned: bool,
    /// MAE's own seeded/built-in content (structurally mirrors
    /// `shared_kb::NodeSource::Seed`, threaded through the same
    /// no-mae-kb-dependency pattern as `NodeKind` above) — the AI-residency
    /// seed-content exemption (#361) needs this per-node so
    /// `kb_graph_view_state` can post-filter the AI's read of an
    /// already-rendered graph buffer without restricting what the human
    /// sees on screen.
    #[serde(default)]
    pub is_seed: bool,
}

/// Kind of scene node.
///
/// Structurally mirrors `shared_kb::NodeKind` (`shared/kb/src/lib.rs`) —
/// same 14 variants, same names — WITHOUT `mae-canvas` taking a hard
/// dependency on `mae-kb` (this crate is deliberately kept a leaf crate with
/// no dependency on the KB layer; see `kb_graph.rs`'s module docs). Callers
/// that have a real `shared_kb::NodeKind` convert it via
/// `mae_core::graph_view_support::shared_kind_to_canvas_kind` (the
/// conversion lives in `crates/core` — the first crate in the dependency
/// graph that can see BOTH `mae-kb` and `mae-canvas` — since an inherent
/// `From` impl can't live in either leaf crate per Rust's orphan rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    /// The help index page (there is usually exactly one of these).
    Index,
    /// An editor command.
    Command,
    /// An architectural concept.
    Concept,
    /// A keybinding or key sequence documentation entry.
    Key,
    /// Free-form user note (org-roam-style).
    Note,
    /// Project node.
    Project,
    /// Grouping node for organizing related concepts.
    Category,
    /// Tutorial lesson.
    Lesson,
    /// Multi-step tutorial track.
    Tutorial,
    /// Composite node whose body is cached from component nodes.
    Meta,
    /// Paragraph-level sub-node for fine-grained linking.
    Block,
    /// Scheme API documentation.
    SchemeApi,
    /// Work item with todo_state/priority/etc.
    Task,
    /// Configurable query+display node (kanban, backlog, sprint, …).
    View,
}

/// Visual style for a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStyle {
    /// Fill color as hex (e.g. "#4a9eff").
    pub fill: String,
    /// Border color as hex.
    pub border: String,
    /// Border width in logical pixels.
    pub border_width: f64,
    /// Whether this is a "starter" node (highlighted).
    pub highlighted: bool,
}

impl Default for NodeStyle {
    fn default() -> Self {
        Self {
            fill: "#2a2d3e".to_string(),
            border: "#4a4d5e".to_string(),
            border_width: 1.0,
            highlighted: false,
        }
    }
}

/// An edge between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneEdge {
    pub source: usize,
    pub target: usize,
    pub label: Option<String>,
    pub style: EdgeStyle,
    /// ADR-030 relationship strength, 0.0-1.0 (`1.0` when not explicitly
    /// authored, or for edges with no underlying KB link weight, e.g.
    /// boundary/self-loop stub edges). Drives the force layout's
    /// attraction strength for this edge — a link the user tagged as
    /// weaker settles at a looser equilibrium distance than the default.
    pub weight: f64,
    /// ADR-030 relationship type (e.g. "implements", "references"). Not
    /// used by the force layout itself (weight drives that) — carried
    /// through for potential future edge styling (color/dash by type).
    pub rel_type: Option<String>,
}

/// Visual style for an edge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeStyle {
    /// Stroke color as hex.
    pub color: String,
    /// Line width in logical pixels.
    pub width: f64,
    /// Whether this edge is dashed (e.g. boundary links).
    pub dashed: bool,
}

impl Default for EdgeStyle {
    fn default() -> Self {
        Self {
            color: "#6a6d7e".to_string(),
            width: 1.0,
            dashed: false,
        }
    }
}

/// Viewport for panning and zooming the scene.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Viewport {
    pub center_x: f64,
    pub center_y: f64,
    pub zoom: f64,
    pub width: f64,
    pub height: f64,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            center_x: 0.0,
            center_y: 0.0,
            zoom: 1.0,
            width: 800.0,
            height: 600.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_scene_graph_is_empty() {
        let sg = SceneGraph::new();
        assert!(sg.nodes.is_empty());
        assert!(sg.edges.is_empty());
        assert!(sg.selection.is_none());
    }

    #[test]
    fn selected_node_returns_correct_node() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(SceneNode {
            id: "n1".to_string(),
            label: "Node 1".to_string(),
            x: 0.0,
            y: 0.0,
            width: 100.0,
            height: 40.0,
            kind: NodeKind::Concept,
            style: NodeStyle::default(),
            pinned: false,
            is_seed: false,
        });
        assert!(sg.selected_node().is_none());
        sg.selection = Some(0);
        assert_eq!(sg.selected_node().unwrap().id, "n1");
    }

    #[test]
    fn default_viewport() {
        let vp = Viewport::default();
        assert_eq!(vp.zoom, 1.0);
        assert_eq!(vp.center_x, 0.0);
    }
}
