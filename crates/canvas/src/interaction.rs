//! Hit testing, viewport transforms, and keyboard navigation.

use crate::scene::{SceneGraph, SceneNode, Viewport};

/// Direction for keyboard navigation between nodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// Test whether a scene-space point hits a node. Returns the node index.
///
/// `radii` gives each node's hit-circle radius IN SCENE-SPACE UNITS,
/// PARALLEL to `graph.nodes` (index `i` in `graph.nodes` uses `radii[i]`;
/// a missing entry fails closed — unclickable, never a spurious nonzero
/// default) — the KB graph view renders every node as a circle whose
/// SCREEN-space radius varies by degree and zoom
/// (`GraphStyleOptions::node_radius`/`node_render_radius`, see that
/// function's doc comment), so a caller must convert each node's real
/// screen-space render radius to scene-space (dividing by the current
/// zoom) before calling this
/// (`graph_view_ops.rs::graph_scene_hit_radii`) — otherwise the clickable
/// area drifts away from the visible circle. Deliberately NOT
/// `node.width`/`node.height` — those are leftover fields from an earlier
/// rectangular-node model the renderer no longer uses for the KB graph
/// view; testing against them hit a box that had already diverged from
/// the circle actually drawn.
pub fn hit_test(graph: &SceneGraph, scene_x: f64, scene_y: f64, radii: &[f64]) -> Option<usize> {
    for (i, node) in graph.nodes.iter().enumerate().rev() {
        let radius = radii.get(i).copied().unwrap_or(0.0);
        let dx = scene_x - node.x;
        let dy = scene_y - node.y;
        if dx * dx + dy * dy <= radius * radius {
            return Some(i);
        }
    }
    None
}

/// Convert viewport (screen) coordinates to scene coordinates.
pub fn viewport_to_scene(vp: &Viewport, screen_x: f64, screen_y: f64) -> (f64, f64) {
    let sx = (screen_x - vp.width / 2.0) / vp.zoom + vp.center_x;
    let sy = (screen_y - vp.height / 2.0) / vp.zoom + vp.center_y;
    (sx, sy)
}

/// Convert scene coordinates to viewport (screen) coordinates.
pub fn scene_to_viewport(vp: &Viewport, scene_x: f64, scene_y: f64) -> (f64, f64) {
    let sx = (scene_x - vp.center_x) * vp.zoom + vp.width / 2.0;
    let sy = (scene_y - vp.center_y) * vp.zoom + vp.height / 2.0;
    (sx, sy)
}

/// Pan the viewport by screen-space deltas.
pub fn pan(vp: &mut Viewport, dx: f64, dy: f64) {
    vp.center_x -= dx / vp.zoom;
    vp.center_y -= dy / vp.zoom;
}

/// Zoom the viewport around a focus point (in screen coordinates).
pub fn zoom(vp: &mut Viewport, factor: f64, focus_x: f64, focus_y: f64) {
    let (scene_x, scene_y) = viewport_to_scene(vp, focus_x, focus_y);
    vp.zoom = (vp.zoom * factor).clamp(0.1, 10.0);
    // Adjust center so the focus point stays fixed
    vp.center_x = scene_x - (focus_x - vp.width / 2.0) / vp.zoom;
    vp.center_y = scene_y - (focus_y - vp.height / 2.0) / vp.zoom;
}

/// Set the viewport to an explicit absolute zoom level, clamped to the same
/// [0.1, 10.0] range `zoom()` enforces. Unlike `zoom()`, this takes no pixel
/// focus point and never touches `center_x`/`center_y` — the pan position
/// stays put. Meant for callers with no meaningful screen coordinate to
/// anchor around (e.g. an AI agent's "set the graph zoom to 2x" request),
/// as opposed to a mouse wheel event's inherently pixel-anchored zoom.
pub fn set_zoom(vp: &mut Viewport, target: f64) {
    vp.zoom = target.clamp(0.1, 10.0);
}

/// Navigate to the nearest node in the given direction from the current selection.
pub fn navigate_direction(graph: &mut SceneGraph, dir: Direction) {
    let current = match graph.selection {
        Some(i) if i < graph.nodes.len() => i,
        _ => {
            // No selection — select first node
            if !graph.nodes.is_empty() {
                graph.selection = Some(0);
            }
            return;
        }
    };

    let cx = graph.nodes[current].x;
    let cy = graph.nodes[current].y;

    let mut best: Option<(usize, f64)> = None;

    for (i, node) in graph.nodes.iter().enumerate() {
        if i == current {
            continue;
        }
        let dx = node.x - cx;
        let dy = node.y - cy;

        // Check direction constraint
        let in_direction = match dir {
            Direction::Up => dy < -1.0,
            Direction::Down => dy > 1.0,
            Direction::Left => dx < -1.0,
            Direction::Right => dx > 1.0,
        };

        if !in_direction {
            continue;
        }

        let dist = (dx * dx + dy * dy).sqrt();
        if best.is_none() || dist < best.unwrap().1 {
            best = Some((i, dist));
        }
    }

    if let Some((idx, _)) = best {
        graph.selection = Some(idx);
    }
}

/// Center the viewport on a specific node.
pub fn center_on_node(vp: &mut Viewport, node: &SceneNode) {
    vp.center_x = node.x;
    vp.center_y = node.y;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{NodeKind, NodeStyle, SceneGraph, SceneNode};

    fn test_node(id: &str, x: f64, y: f64) -> SceneNode {
        SceneNode {
            id: id.to_string(),
            label: id.to_string(),
            x,
            y,
            width: 100.0,
            height: 40.0,
            kind: NodeKind::Concept,
            style: NodeStyle::default(),
            pinned: false,
            is_seed: false,
        }
    }

    #[test]
    fn hit_test_inside_node() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 100.0, 100.0));
        assert_eq!(hit_test(&sg, 100.0, 100.0, &[50.0]), Some(0));
        assert_eq!(hit_test(&sg, 140.0, 110.0, &[50.0]), Some(0));
    }

    #[test]
    fn hit_test_outside_node() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 100.0, 100.0));
        assert_eq!(hit_test(&sg, 300.0, 300.0, &[50.0]), None);
    }

    #[test]
    fn hit_test_respects_the_given_radius() {
        // A point just inside the boundary hits; the same point just
        // outside a smaller radius misses — confirms the circular distance
        // check (not a leftover rectangular width/height check).
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        assert_eq!(hit_test(&sg, 18.0, 0.0, &[18.0]), Some(0));
        assert_eq!(hit_test(&sg, 18.0, 0.0, &[10.0]), None);
    }

    #[test]
    fn hit_test_uses_per_node_radius() {
        // A big node's larger radius is honored; a small neighbor's
        // smaller radius doesn't over-claim territory it shouldn't.
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("big", 0.0, 0.0));
        sg.nodes.push(test_node("small", 100.0, 0.0));
        let radii = [40.0, 5.0];
        // Well inside the big node's larger radius.
        assert_eq!(hit_test(&sg, 30.0, 0.0, &radii), Some(0));
        // Just outside the small node's tiny radius.
        assert_eq!(hit_test(&sg, 108.0, 0.0, &radii), None);
        // Inside the small node's tiny radius.
        assert_eq!(hit_test(&sg, 102.0, 0.0, &radii), Some(1));
    }

    #[test]
    fn hit_test_missing_radius_entry_fails_closed() {
        // A radii slice shorter than graph.nodes must never grant a
        // spurious hit — missing entries are unclickable (radius 0), not a
        // default nonzero radius. `radii` has only ONE entry (for "a"), so
        // "b" (index 1) has no entry. Click 1 unit off "b"'s own center —
        // with ANY nonzero radius this would hit; a miss here proves the
        // missing entry really resolved to 0.0.
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        sg.nodes.push(test_node("b", 200.0, 0.0));
        assert_eq!(hit_test(&sg, 201.0, 0.0, &[50.0]), None);
    }

    #[test]
    fn hit_test_topmost_wins() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 100.0, 100.0));
        sg.nodes.push(test_node("b", 110.0, 100.0)); // overlapping
                                                     // Later node wins (rendered on top)
        assert_eq!(hit_test(&sg, 105.0, 100.0, &[50.0, 50.0]), Some(1));
    }

    #[test]
    fn viewport_transform_roundtrip() {
        let vp = Viewport {
            center_x: 50.0,
            center_y: 50.0,
            zoom: 2.0,
            width: 800.0,
            height: 600.0,
        };
        let (sx, sy) = viewport_to_scene(&vp, 400.0, 300.0);
        let (back_x, back_y) = scene_to_viewport(&vp, sx, sy);
        assert!((back_x - 400.0).abs() < 0.001);
        assert!((back_y - 300.0).abs() < 0.001);
    }

    #[test]
    fn pan_moves_viewport() {
        let mut vp = Viewport::default();
        pan(&mut vp, 100.0, 50.0);
        assert!(vp.center_x < 0.0);
        assert!(vp.center_y < 0.0);
    }

    #[test]
    fn zoom_clamps() {
        let mut vp = Viewport::default();
        zoom(&mut vp, 100.0, 400.0, 300.0); // extreme zoom in
        assert!(vp.zoom <= 10.0);
        let mut vp2 = Viewport::default();
        zoom(&mut vp2, 0.001, 400.0, 300.0); // extreme zoom out
        assert!(vp2.zoom >= 0.1);
    }

    #[test]
    fn set_zoom_sets_the_exact_level_and_never_touches_pan() {
        let mut vp = Viewport {
            center_x: 42.0,
            center_y: -17.0,
            ..Viewport::default()
        };
        set_zoom(&mut vp, 2.5);
        assert_eq!(vp.zoom, 2.5);
        assert_eq!(vp.center_x, 42.0, "set_zoom must never touch pan");
        assert_eq!(vp.center_y, -17.0, "set_zoom must never touch pan");
    }

    #[test]
    fn set_zoom_clamps_to_the_same_range_as_zoom() {
        let mut vp = Viewport::default();
        set_zoom(&mut vp, 999.0);
        assert_eq!(vp.zoom, 10.0);
        set_zoom(&mut vp, -5.0);
        assert_eq!(vp.zoom, 0.1);
    }

    #[test]
    fn navigate_direction_selects_nearest() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("center", 0.0, 0.0));
        sg.nodes.push(test_node("right", 200.0, 0.0));
        sg.nodes.push(test_node("far-right", 400.0, 0.0));
        sg.nodes.push(test_node("below", 0.0, 200.0));
        sg.selection = Some(0);

        navigate_direction(&mut sg, Direction::Right);
        assert_eq!(sg.selection, Some(1)); // nearest right

        sg.selection = Some(0);
        navigate_direction(&mut sg, Direction::Down);
        assert_eq!(sg.selection, Some(3)); // below
    }

    #[test]
    fn navigate_no_selection_selects_first() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        navigate_direction(&mut sg, Direction::Right);
        assert_eq!(sg.selection, Some(0));
    }

    #[test]
    fn center_on_node_updates_viewport() {
        let mut vp = Viewport::default();
        let node = test_node("target", 500.0, 300.0);
        center_on_node(&mut vp, &node);
        assert_eq!(vp.center_x, 500.0);
        assert_eq!(vp.center_y, 300.0);
    }
}
