//! mae-canvas — Backend-agnostic scene graph and force-directed layout.
//!
//! Provides a `SceneGraph` with nodes/edges, Fruchterman-Reingold force
//! layout, hit testing, and a KB graph builder. The scene graph is
//! rendering-backend-agnostic — consumers flatten it to platform-specific
//! draw calls (e.g. `VisualElement` for MAE's visual buffers).

pub mod interaction;
pub mod kb_graph;
pub mod layout;
pub mod scene;

pub use interaction::{center_on_node, hit_test, navigate_direction, pan, zoom, Direction};
pub use kb_graph::build_kb_graph;
pub use layout::{ForceLayout, LayoutConfig};
pub use scene::{EdgeStyle, NodeKind, NodeStyle, SceneEdge, SceneGraph, SceneNode, Viewport};
