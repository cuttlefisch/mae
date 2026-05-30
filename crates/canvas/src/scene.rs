//! Scene graph — nodes, edges, viewport, and selection state.

use serde::{Deserialize, Serialize};

/// A complete scene graph with positioned nodes and edges.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneGraph {
    pub nodes: Vec<SceneNode>,
    pub edges: Vec<SceneEdge>,
    pub viewport: Viewport,
    pub selection: Option<usize>,
}

impl SceneGraph {
    /// Create an empty scene graph with default viewport.
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            viewport: Viewport::default(),
            selection: None,
        }
    }

    /// Get the selected node, if any.
    pub fn selected_node(&self) -> Option<&SceneNode> {
        self.selection.and_then(|i| self.nodes.get(i))
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
}

/// Kind of scene node — maps to KB node namespaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    Concept,
    Note,
    Command,
    Lesson,
    Scheme,
    Option,
    Custom(String),
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
