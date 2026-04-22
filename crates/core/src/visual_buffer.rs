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
