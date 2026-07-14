//! Visual scene-graph buffer state (Phase 1).

use serde::{Deserialize, Serialize};

/// A single graphical element in a visual buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VisualElement {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Option<String>,   // hex color
        stroke: Option<String>, // hex color
    },
    Line {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        color: String,
        thickness: f32,
        /// Render as a dashed stroke instead of solid. Added for the native
        /// KB graph view (`crate::graph_view::flatten_scene_graph`) to
        /// distinguish boundary edges (subgraph fringe links) from internal
        /// ones — defaults to `false` everywhere else via
        /// `#[serde(default)]` so existing visual-buffer callers/snapshots
        /// are unaffected.
        #[serde(default)]
        dashed: bool,
    },
    Circle {
        cx: f32,
        cy: f32,
        r: f32,
        fill: Option<String>,
        stroke: Option<String>,
    },
    Text {
        x: f32,
        y: f32,
        text: String,
        font_size: f32,
        color: String,
    },
    /// A quadratic bezier curve — used by the native KB graph view
    /// (`crate::graph_view::flatten_scene_graph`) for edges, so adjacent/
    /// parallel edges bow apart instead of overlapping as straight lines.
    /// `(ctrl_x, ctrl_y)` is the single quadratic control point.
    Curve {
        x1: f32,
        y1: f32,
        ctrl_x: f32,
        ctrl_y: f32,
        x2: f32,
        y2: f32,
        color: String,
        thickness: f32,
    },
}

/// Structured state for `BufferKind::Visual`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VisualBuffer {
    pub elements: Vec<VisualElement>,
}

impl VisualBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.elements.clear();
    }

    pub fn add(&mut self, element: VisualElement) {
        self.elements.push(element);
    }
}
