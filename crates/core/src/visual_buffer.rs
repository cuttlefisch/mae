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
    /// An annular sector ("wedge") — a ring segment between `inner_r` and
    /// `outer_r`, spanning `sweep_angle` radians starting at `start_angle`
    /// (0 = positive x-axis, increasing clockwise in screen space),
    /// optionally with rounded corners. This is MAE's first arc/path shape
    /// primitive (ADR-070) — added for the native chord-diagram wedge/petal
    /// redesign (ADR-071), where each ring node is a wedge instead of a
    /// `Circle`. Deliberately supersedes (in part) ADR-069's earlier "no new
    /// `VisualElement` variant, no GUI/Skia backend changes" constraint
    /// (`crate::graph_view`'s edge-taper code) — see ADR-070's header for the
    /// explicit supersession.
    Wedge {
        cx: f32,
        cy: f32,
        inner_r: f32,
        outer_r: f32,
        /// Radians, 0 = positive x-axis.
        start_angle: f32,
        /// Radians. Must be >= 0; a wedge never wraps past 2π in one
        /// element (draw two wedges for that).
        sweep_angle: f32,
        /// World-unit corner radius, clamped at draw time (see
        /// `wedge_corner_radius_clamp`) so a large requested radius can
        /// never self-intersect the wedge's own geometry — callers may pass
        /// any non-negative value without pre-clamping themselves.
        corner_radius: f32,
        fill: Option<String>,
        stroke: Option<String>,
    },
}

fn one_f32() -> f32 {
    1.0
}

/// Clamp a requested wedge corner radius so rounding can never self-
/// intersect the wedge's own geometry. Ported directly from the reference
/// implementation's `arcPath` clamp (a downstream sister project's chord-
/// diagram redesign, cited in ADR-070/071): the corner radius is bounded by
/// half the wedge's radial thickness, and by half the chord length swept at
/// both the inner and outer radius — whichever is smallest. `epsilon` keeps
/// the clamp strictly inside each bound rather than exactly on it, so the
/// rounded corners never touch/overlap even at the boundary case.
///
/// Never panics or returns NaN/negative output for any finite input,
/// including degenerate wedges (`inner_r > outer_r`, `sweep_angle <= 0`,
/// `outer_r <= 0`) — callers may pass unvalidated geometry and get a safe
/// (zero) corner radius back rather than a crash.
pub fn wedge_corner_radius_clamp(
    inner_r: f32,
    outer_r: f32,
    sweep_angle: f32,
    requested: f32,
) -> f32 {
    const EPSILON: f32 = 0.01;
    if !requested.is_finite() || requested <= 0.0 {
        return 0.0;
    }
    let thickness_bound = (outer_r - inner_r) / 2.0 - EPSILON;
    let outer_chord_bound = (sweep_angle * outer_r) / 2.0 - EPSILON;
    let inner_chord_bound = (sweep_angle * inner_r.max(1.0)) / 2.0 - EPSILON;
    let bound = requested
        .min(thickness_bound)
        .min(outer_chord_bound)
        .min(inner_chord_bound);
    if bound.is_finite() {
        bound.max(0.0)
    } else {
        0.0
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wedge_corner_radius_clamp_passes_through_a_small_request_unchanged() {
        // A request well inside every bound should be returned as-is.
        let clamped = wedge_corner_radius_clamp(10.0, 20.0, 1.0, 1.0);
        assert!((clamped - 1.0).abs() < 1e-6, "expected ~1.0, got {clamped}");
    }

    #[test]
    fn wedge_corner_radius_clamp_bounds_by_radial_thickness() {
        // thickness_bound = (20-10)/2 - 0.01 = 4.99, well below a huge request.
        let clamped = wedge_corner_radius_clamp(10.0, 20.0, 10.0, 1000.0);
        assert!(clamped <= 4.99 + 1e-6, "got {clamped}");
        assert!(clamped > 0.0, "got {clamped}");
    }

    #[test]
    fn wedge_corner_radius_clamp_bounds_by_outer_chord_at_a_narrow_sweep() {
        // A very narrow sweep angle should force a small clamp via the
        // outer-chord bound, even though radial thickness is generous.
        let narrow = wedge_corner_radius_clamp(10.0, 1000.0, 0.01, 1000.0);
        let wide = wedge_corner_radius_clamp(10.0, 1000.0, 3.0, 1000.0);
        assert!(
            narrow < wide,
            "narrower sweep must clamp tighter: narrow={narrow}, wide={wide}"
        );
    }

    #[test]
    fn wedge_corner_radius_clamp_degenerate_inputs_never_panic_or_go_negative() {
        // Adversarial: inner_r > outer_r, zero/negative sweep, zero outer_r,
        // NaN/negative requested radius — every case must return a finite,
        // non-negative value, never panic.
        let cases: &[(f32, f32, f32, f32)] = &[
            (20.0, 10.0, 1.0, 5.0),           // inner > outer
            (10.0, 20.0, 0.0, 5.0),           // zero sweep
            (10.0, 20.0, -1.0, 5.0),          // negative sweep
            (0.0, 0.0, 1.0, 5.0),             // zero radii
            (10.0, 20.0, 1.0, -5.0),          // negative request
            (10.0, 20.0, 1.0, 0.0),           // zero request
            (10.0, 20.0, 1.0, f32::NAN),      // NaN request
            (10.0, 20.0, 1.0, f32::INFINITY), // infinite request
        ];
        for &(inner_r, outer_r, sweep_angle, requested) in cases {
            let clamped = wedge_corner_radius_clamp(inner_r, outer_r, sweep_angle, requested);
            assert!(
                clamped.is_finite() && clamped >= 0.0,
                "case ({inner_r}, {outer_r}, {sweep_angle}, {requested}) produced non-finite/negative: {clamped}"
            );
        }
    }

    #[test]
    fn wedge_corner_radius_clamp_is_deterministic() {
        let a = wedge_corner_radius_clamp(12.0, 34.0, 0.7, 6.0);
        let b = wedge_corner_radius_clamp(12.0, 34.0, 0.7, 6.0);
        assert_eq!(
            a.to_bits(),
            b.to_bits(),
            "identical inputs must produce byte-identical output"
        );
    }

    #[test]
    fn wedge_element_round_trips_through_serde() {
        let el = VisualElement::Wedge {
            cx: 1.0,
            cy: 2.0,
            inner_r: 3.0,
            outer_r: 4.0,
            start_angle: 0.5,
            sweep_angle: 1.2,
            corner_radius: 0.3,
            fill: Some("#abcdef".to_string()),
            stroke: None,
        };
        let json = serde_json::to_string(&el).unwrap();
        let back: VisualElement = serde_json::from_str(&json).unwrap();
        match back {
            VisualElement::Wedge {
                cx,
                cy,
                inner_r,
                outer_r,
                start_angle,
                sweep_angle,
                corner_radius,
                fill,
                stroke,
            } => {
                assert_eq!(cx, 1.0);
                assert_eq!(cy, 2.0);
                assert_eq!(inner_r, 3.0);
                assert_eq!(outer_r, 4.0);
                assert_eq!(start_angle, 0.5);
                assert_eq!(sweep_angle, 1.2);
                assert_eq!(corner_radius, 0.3);
                assert_eq!(fill.as_deref(), Some("#abcdef"));
                assert_eq!(stroke, None);
            }
            other => panic!("expected Wedge, got {other:?}"),
        }
    }
}
