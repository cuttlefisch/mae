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
/// area drifts away from the visible circle. `SceneNode` used to also
/// carry `width`/`height` fields from an earlier rectangular-node model;
/// they were removed (#462 audit) once confirmed dead — hit-testing never
/// read them even before removal, since a box built from stored
/// per-node dimensions had already diverged from the circle actually
/// drawn by the time size started varying with degree/zoom.
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

/// Per-node wedge geometry for `hit_test_wedge` — the clickable-area
/// counterpart to a `VisualElement::Wedge`'s drawn geometry (ADR-070 D1/D3).
/// Deliberately mirrors `VisualElement::Wedge`'s own field shape (minus
/// `cx`/`cy`, which come from `SceneNode.x`/`.y` the same way `hit_test`'s
/// `radii` already omits per-node center) so a caller building one from the
/// other can't accidentally desync radius/angle values between what's drawn
/// and what's clickable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WedgeGeom {
    pub inner_r: f64,
    pub outer_r: f64,
    /// Radians, 0 = positive x-axis, increasing clockwise in screen space —
    /// same convention as `VisualElement::Wedge`.
    pub start_angle: f64,
    /// Radians. `<= 0` (or any value that leaves the swept range empty)
    /// degrades to "never hits" rather than panicking or wrapping oddly.
    pub sweep_angle: f64,
}

/// Test whether a scene-space point hits a wedge-shaped node (Chord-mode
/// nodes, ADR-071) — the angular-sector counterpart to `hit_test`'s plain
/// circle-distance test (Force-mode nodes stay circles, so both functions
/// coexist; see ADR-070 D3 for why this is a parallel function rather than
/// a `hit_test` generalization).
///
/// `wedges` is PARALLEL to `graph.nodes`, exactly like `hit_test`'s `radii`
/// — a missing entry fails closed (unclickable), never a spurious hit.
pub fn hit_test_wedge(
    graph: &SceneGraph,
    scene_x: f64,
    scene_y: f64,
    wedges: &[WedgeGeom],
) -> Option<usize> {
    const TWO_PI: f64 = std::f64::consts::PI * 2.0;
    for (i, node) in graph.nodes.iter().enumerate().rev() {
        let Some(w) = wedges.get(i) else {
            continue;
        };
        let dx = scene_x - node.x;
        let dy = scene_y - node.y;
        let r = (dx * dx + dy * dy).sqrt();
        if r < w.inner_r || r > w.outer_r {
            continue;
        }
        let angle = dy.atan2(dx).rem_euclid(TWO_PI);
        let start = w.start_angle.rem_euclid(TWO_PI);
        let end = start + w.sweep_angle;
        let in_range = if w.sweep_angle <= 0.0 {
            false
        } else if end <= TWO_PI {
            angle >= start && angle <= end
        } else {
            // The swept range wraps past 2π (e.g. start=350°, sweep=20° ->
            // covers [350°,360°) U [0°,10°]) — split into the two arcs.
            angle >= start || angle <= end - TWO_PI
        };
        if in_range {
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

/// Zoom the viewport around a focus point (in screen coordinates). Lower
/// bound is `vp.min_zoom` — dynamic per-scene, not a flat constant, see
/// that field's doc comment — upper bound is a flat `10.0` (no reported
/// need for a dynamic ceiling).
pub fn zoom(vp: &mut Viewport, factor: f64, focus_x: f64, focus_y: f64) {
    let (scene_x, scene_y) = viewport_to_scene(vp, focus_x, focus_y);
    vp.zoom = (vp.zoom * factor).clamp(vp.min_zoom, 10.0);
    // Adjust center so the focus point stays fixed
    vp.center_x = scene_x - (focus_x - vp.width / 2.0) / vp.zoom;
    vp.center_y = scene_y - (focus_y - vp.height / 2.0) / vp.zoom;
}

/// Set the viewport to an explicit absolute zoom level, clamped to the same
/// `[vp.min_zoom, 10.0]` range `zoom()` enforces. Unlike `zoom()`, this
/// takes no pixel focus point and never touches `center_x`/`center_y` — the
/// pan position stays put. Meant for callers with no meaningful screen
/// coordinate to anchor around (e.g. an AI agent's "set the graph zoom to
/// 2x" request), as opposed to a mouse wheel event's inherently
/// pixel-anchored zoom.
pub fn set_zoom(vp: &mut Viewport, target: f64) {
    vp.zoom = target.clamp(vp.min_zoom, 10.0);
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
    use crate::scene::{NodeKind, SceneGraph, SceneNode};

    fn test_node(id: &str, x: f64, y: f64) -> SceneNode {
        SceneNode {
            id: id.to_string(),
            label: id.to_string(),
            x,
            y,
            kind: NodeKind::Concept,
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
    fn hit_test_wedge_inside_node() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        // Wedge spans 0..PI/2 (the first quadrant), radius 10..20.
        let w = WedgeGeom {
            inner_r: 10.0,
            outer_r: 20.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        };
        // At angle PI/4 (mid-sweep), radius 15 (mid-thickness) — well inside.
        let (x, y) = (
            15.0 * (std::f64::consts::FRAC_PI_4).cos(),
            15.0 * (std::f64::consts::FRAC_PI_4).sin(),
        );
        assert_eq!(hit_test_wedge(&sg, x, y, &[w]), Some(0));
    }

    #[test]
    fn hit_test_wedge_outside_node() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        let w = WedgeGeom {
            inner_r: 10.0,
            outer_r: 20.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        };
        // Correct angle, but radius far outside [inner_r, outer_r].
        assert_eq!(hit_test_wedge(&sg, 100.0, 0.0, &[w]), None);
        // Correct radius, but wrong angle (opposite side of the circle).
        assert_eq!(hit_test_wedge(&sg, -15.0, 0.0, &[w]), None);
    }

    #[test]
    fn hit_test_wedge_respects_angular_bounds() {
        // A narrow wedge (PI/6 wide) — a point at an angle well outside the
        // sweep must miss even though its radius is correct.
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        let w = WedgeGeom {
            inner_r: 10.0,
            outer_r: 20.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_6,
        };
        // Angle PI (180 degrees) is far outside [0, PI/6].
        assert_eq!(hit_test_wedge(&sg, -15.0, 0.0, &[w]), None);
    }

    #[test]
    fn hit_test_wedge_boundary_edges_are_inclusive() {
        // Exactly at start_angle and exactly at start_angle+sweep_angle
        // must both hit — the boundary is inclusive on both ends, not just
        // the wedge's angular center.
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        let w = WedgeGeom {
            inner_r: 10.0,
            outer_r: 20.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        };
        // Exactly at start_angle (0 radians): point (15, 0).
        assert_eq!(hit_test_wedge(&sg, 15.0, 0.0, &[w]), Some(0));
        // Exactly at start_angle + sweep_angle (PI/2 radians): point (0, 15).
        assert_eq!(hit_test_wedge(&sg, 0.0, 15.0, &[w]), Some(0));
        // Just past the end boundary must miss.
        let past_end = std::f64::consts::FRAC_PI_2 + 0.01;
        assert_eq!(
            hit_test_wedge(&sg, 15.0 * past_end.cos(), 15.0 * past_end.sin(), &[w]),
            None
        );
    }

    #[test]
    fn hit_test_wedge_radius_boundary_edges_are_inclusive() {
        // Exactly at inner_r and exactly at outer_r must both hit.
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        let w = WedgeGeom {
            inner_r: 10.0,
            outer_r: 20.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::FRAC_PI_2,
        };
        assert_eq!(hit_test_wedge(&sg, 10.0, 0.0, &[w]), Some(0)); // exactly inner_r
        assert_eq!(hit_test_wedge(&sg, 20.0, 0.0, &[w]), Some(0)); // exactly outer_r
        assert_eq!(hit_test_wedge(&sg, 9.99, 0.0, &[w]), None); // just inside inner_r (hole)
        assert_eq!(hit_test_wedge(&sg, 20.01, 0.0, &[w]), None); // just outside outer_r
    }

    #[test]
    fn hit_test_wedge_missing_radius_entry_fails_closed() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        sg.nodes.push(test_node("b", 200.0, 0.0));
        // "a"'s own wedge does NOT reach anywhere near "b" (outer_r 50 vs.
        // the 200-unit distance between them), so only a (missing, and
        // thus defaulted-unclickable) entry for "b" itself could cause a
        // hit at "b"'s own location.
        let w = WedgeGeom {
            inner_r: 0.0,
            outer_r: 50.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::TAU,
        };
        // Only ONE entry provided (for "a") — "b" has no wedge entry, so a
        // point right on "b"'s own center must still miss.
        assert_eq!(hit_test_wedge(&sg, 200.0, 0.0, &[w]), None);
    }

    #[test]
    fn hit_test_wedge_topmost_wins() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        sg.nodes.push(test_node("a-dup", 0.0, 0.0)); // same center, overlapping wedge
        let w = WedgeGeom {
            inner_r: 0.0,
            outer_r: 20.0,
            start_angle: 0.0,
            sweep_angle: std::f64::consts::TAU,
        };
        assert_eq!(hit_test_wedge(&sg, 5.0, 0.0, &[w, w]), Some(1));
    }

    #[test]
    fn hit_test_wedge_zero_or_negative_sweep_never_hits() {
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        let zero = WedgeGeom {
            inner_r: 0.0,
            outer_r: 20.0,
            start_angle: 0.0,
            sweep_angle: 0.0,
        };
        let negative = WedgeGeom {
            sweep_angle: -1.0,
            ..zero
        };
        assert_eq!(hit_test_wedge(&sg, 5.0, 0.0, &[zero]), None);
        assert_eq!(hit_test_wedge(&sg, 5.0, 0.0, &[negative]), None);
    }

    #[test]
    fn hit_test_wedge_handles_wraparound_past_2pi() {
        // start_angle near 2*PI with a sweep that wraps past it into the
        // low-angle range on the other side.
        let mut sg = SceneGraph::new();
        sg.nodes.push(test_node("a", 0.0, 0.0));
        let w = WedgeGeom {
            inner_r: 10.0,
            outer_r: 20.0,
            start_angle: std::f64::consts::TAU - 0.2, // ~350 degrees
            sweep_angle: 0.4,                         // wraps ~10 degrees past 0
        };
        // A point at angle ~0.1 rad (just past the wrap) should hit.
        let angle: f64 = 0.1;
        assert_eq!(
            hit_test_wedge(&sg, 15.0 * angle.cos(), 15.0 * angle.sin(), &[w]),
            Some(0)
        );
        // A point at angle PI (opposite side) should miss.
        assert_eq!(hit_test_wedge(&sg, -15.0, 0.0, &[w]), None);
    }

    #[test]
    fn viewport_transform_roundtrip() {
        let vp = Viewport {
            center_x: 50.0,
            center_y: 50.0,
            zoom: 2.0,
            width: 800.0,
            height: 600.0,
            ..Default::default()
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
