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
        /// Stroke opacity, 0.0-1.0. `#[serde(default = "one_f32")]` (not a
        /// plain `#[serde(default)]`, which would deserialize old/missing
        /// snapshots to `0.0` — fully transparent) so pre-existing
        /// snapshots/callers stay fully opaque, matching behavior before
        /// this field existed. Added for `kb_graph_edge_alpha` (#367
        /// follow-up) so dense chord-diagram edges can stay readable
        /// instead of overlapping into a solid mass.
        #[serde(default = "one_f32")]
        alpha: f32,
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
        /// Rotation in degrees, clockwise, applied around `(x, y)` before
        /// drawing. `0.0` (the `#[serde(default)]`) is a plain, unrotated
        /// draw — every non-graph-view caller and Force-mode graph labels.
        /// Used by chord-mode graph labels (`graph_view::chord_label_placement`)
        /// to orient each label radially around the ring.
        #[serde(default)]
        rotation_degrees: f32,
        /// When true, `(x, y)` is the text's END (not start) — the GUI
        /// draw call measures the string and offsets backward so it grows
        /// away from `(x, y)` instead of from it. Used together with
        /// `rotation_degrees` for the far half of a chord-diagram ring, so
        /// the flipped-180° label still reads right-side-up extending
        /// outward from its node instead of back into the ring's interior.
        #[serde(default)]
        right_align: bool,
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
        /// See `Line::alpha`'s doc comment — identical role and default.
        #[serde(default = "one_f32")]
        alpha: f32,
    },
}

fn one_f32() -> f32 {
    1.0
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
