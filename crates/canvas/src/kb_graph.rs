//! Bridge between KB data and SceneGraph.
//!
//! Converts KB nodes and links into a positioned scene graph. `mae-canvas`
//! is deliberately kept a leaf crate with no dependency on `mae-kb` — see
//! `crate::scene::NodeKind`'s doc comment for how its `NodeKind` stays a
//! structural mirror of `shared_kb::NodeKind` without a hard dependency
//! edge. Callers pass the node's real kind in via `KbNodeInfo::kind`
//! (converted from `shared_kb::NodeKind` at the `crates/core` call site,
//! the first place in the dependency graph that can see both crates);
//! this module no longer guesses a kind from the id string (the previous
//! `namespace_to_kind` — deleted once real kinds were threaded through, since
//! it was a lossy approximation prone to disagreeing with the actual KB
//! data, e.g. it had no `option:` mapping matching any real `NodeKind`
//! variant, because no such variant exists upstream).

use crate::layout::{ForceLayout, LayoutConfig};
use crate::scene::{EdgeStyle, NodeKind, SceneEdge, SceneGraph, SceneNode};

/// A simplified KB node for graph building (no dependency on mae-kb — see
/// module docs on why `kind` is `crate::scene::NodeKind`, not
/// `shared_kb::NodeKind`).
#[derive(Debug, Clone)]
pub struct KbNodeInfo {
    pub id: String,
    pub title: String,
    pub kind: NodeKind,
    /// See [`crate::scene::SceneNode::is_seed`]'s doc comment (#361).
    pub is_seed: bool,
}

/// A simplified typed KB link for graph building (no dependency on mae-kb
/// — mirrors `KbNodeInfo`'s role for nodes; the `crates/core` call site
/// bridges from `shared_kb::SubgraphLink`).
#[derive(Debug, Clone)]
pub struct KbLinkInfo {
    pub source: String,
    pub target: String,
    pub rel_type: String,
    /// 0.0-1.0, ADR-030 authored/default relationship weight.
    pub weight: f64,
}

/// Build a scene graph from KB nodes and links.
///
/// - `nodes`: KB nodes with id and title
/// - `links`: typed links within the subgraph
/// - `boundary_links`: typed links crossing the subgraph boundary
pub fn build_kb_graph(
    nodes: &[KbNodeInfo],
    links: &[KbLinkInfo],
    boundary_links: &[KbLinkInfo],
    spacing_scale: f64,
) -> SceneGraph {
    let mut graph = build_kb_graph_positions_only(nodes, links, boundary_links, spacing_scale);

    // Run force layout
    let layout = ForceLayout::new(LayoutConfig {
        spacing_scale,
        ..LayoutConfig::default()
    });
    layout.run(&mut graph.nodes, &graph.edges, 50);

    graph
}

/// Build a scene graph WITHOUT running the force-directed layout pass —
/// nodes get only their initial circular positions. Used by MAE's native KB
/// graph view (`crates/core/src/editor/graph_view_ops.rs`) so the (possibly
/// nontrivial, O(n^2)-per-iteration) layout computation can be dispatched to
/// a background thread (`graph_layout_bridge`) instead of running inline —
/// `build_kb_graph` above still runs it synchronously for callers (tests,
/// any future non-backgrounded caller) that want a complete one-call result.
pub fn build_kb_graph_positions_only(
    nodes: &[KbNodeInfo],
    links: &[KbLinkInfo],
    boundary_links: &[KbLinkInfo],
    spacing_scale: f64,
) -> SceneGraph {
    // Build index: id -> node position
    let id_to_idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // Create scene nodes with initial positions via a 2-D "sunflower"
    // (Vogel/Fibonacci-spiral) point distribution — NOT a plain ring. A
    // 1-D ring can't simultaneously satisfy two constraints at once for
    // large n: (a) overall spread small enough for the force-layout's
    // temperature-bounded relaxation budget to actually reach equilibrium
    // from (see `IDEAL_AREA_PER_NODE`'s doc comment), and (b) adjacent
    // nodes non-overlapping — a ring's local point density is forced to
    // scale as 1/n independent of how large its radius is, so a radius
    // small enough for (a) inevitably crams nodes into overlapping
    // hit-circles/render-circles for a large KB subgraph. Vogel's method
    // distributes n points evenly across a genuinely 2-D disk of area
    // `n * IDEAL_AREA_PER_NODE`; average nearest-neighbor spacing works out
    // to a CONSTANT `sqrt(IDEAL_AREA_PER_NODE)` regardless of n, satisfying
    // both constraints at once (and incidentally not reading as an obvious
    // circle outline pre-layout, unlike the plain ring).
    //
    // `* spacing_scale` mirrors `ForceLayout::step`'s identical `area`
    // term exactly (`LayoutConfig::spacing_scale`'s doc comment) — without
    // this, raising `spacing_scale` would widen the force layout's
    // EQUILIBRIUM distance while leaving this INITIAL placement's spread
    // fixed at the old, tighter size, reproducing the exact "nodes settled
    // having barely moved off their initial ring" failure mode
    // `IDEAL_AREA_PER_NODE`'s own doc comment says was already fixed once.
    let n = nodes.len();
    let disk_radius = sqrt_area_radius(n, spacing_scale);
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let positions: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let r = disk_radius * (((i as f64) + 0.5) / (n.max(1) as f64)).sqrt();
            let angle = (i as f64) * golden_angle;
            (r * angle.cos(), r * angle.sin())
        })
        .collect();

    positions_to_scene(nodes, links, boundary_links, &id_to_idx, &positions)
}

/// Build a scene graph with nodes evenly spaced around a circle's
/// circumference — a chord-diagram / Circos-style layout (#367). Unlike
/// [`build_kb_graph_positions_only`]'s sunflower seed, this placement is
/// the FINAL layout: no force-directed refinement follows it (see
/// `Editor::populate_graph_buffer`'s branch on `kb_graph_layout_algorithm`).
///
/// The ring's radius deliberately matches the sunflower disk's SUB-LINEAR
/// (sqrt-of-n) growth rather than growing linearly with `n` to hold
/// adjacent-node arc spacing constant. A constant-spacing ring was tried
/// first and reproduced, in the field, on a ~1300-node KB subgraph: the
/// resulting radius (tens of thousands of scene units) vastly exceeded what
/// the shared `[0.1, 10.0]` zoom range (`crates/canvas/src/interaction.rs`)
/// can zoom out to fit, leaving the diagram permanently too large to see in
/// full. Matching the sunflower's growth rate instead keeps both layout
/// algorithms visually comparable in scale — switching between them via
/// `:set kb_graph_layout_algorithm` doesn't require re-zooming — at the
/// (accepted) cost of per-node arc spacing shrinking as n grows, same as
/// any real chord/Circos diagram rendered at a fixed canvas size.
pub fn build_kb_graph_chord_positions(
    nodes: &[KbNodeInfo],
    links: &[KbLinkInfo],
    boundary_links: &[KbLinkInfo],
    spacing_scale: f64,
) -> SceneGraph {
    let id_to_idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    let positions = chord_ring_positions(nodes.len(), spacing_scale);
    positions_to_scene(nodes, links, boundary_links, &id_to_idx, &positions)
}

/// Local (centered-at-origin) chord-ring positions for `n` nodes — extracted
/// from `build_kb_graph_chord_positions` (CLAUDE.md #8) so
/// `build_multi_kb_chord_positions` below can reuse the exact same trig for
/// each diagram's OWN ring before translating it to its grid-cell offset,
/// rather than duplicating it.
fn chord_ring_positions(n: usize, spacing_scale: f64) -> Vec<(f64, f64)> {
    let radius = sqrt_area_radius(n, spacing_scale);
    (0..n)
        .map(|i| {
            let angle = (i as f64) * 2.0 * std::f64::consts::PI / (n.max(1) as f64);
            (radius * angle.cos(), radius * angle.sin())
        })
        .collect()
}

/// Radius of a circle/disk whose AREA is `n * IDEAL_AREA_PER_NODE *
/// spacing_scale` — shared by the sunflower disk (`build_kb_graph_positions_only`)
/// and the chord ring (`build_kb_graph_chord_positions`, CLAUDE.md #8) so
/// both layouts grow at the same sub-linear (sqrt-of-n) rate and stay
/// visually comparable in scale, within the same `[0.1, 10.0]` zoom range
/// (`crates/canvas/src/interaction.rs`) regardless of which algorithm is
/// active. `.max(100.0)` keeps tiny graphs from collapsing to a point.
fn sqrt_area_radius(n: usize, spacing_scale: f64) -> f64 {
    ((n as f64 * crate::layout::IDEAL_AREA_PER_NODE * spacing_scale) / std::f64::consts::PI)
        .sqrt()
        .max(100.0)
}

/// Shared node/edge assembly for every `build_kb_graph*` variant (CLAUDE.md
/// #8) — the only thing that differs between them is HOW `positions` was
/// computed (sunflower disk, circular ring, ...); the internal/boundary
/// edge-building logic (including the boundary-link dedup-by-source stub
/// collapsing) is identical regardless.
fn positions_to_scene(
    nodes: &[KbNodeInfo],
    links: &[KbLinkInfo],
    boundary_links: &[KbLinkInfo],
    id_to_idx: &std::collections::HashMap<&str, usize>,
    positions: &[(f64, f64)],
) -> SceneGraph {
    let n = nodes.len();
    let scene_nodes: Vec<SceneNode> = nodes
        .iter()
        .zip(positions.iter())
        .map(|(node, &(x, y))| SceneNode {
            id: node.id.clone(),
            label: node.title.clone(),
            x,
            y,
            kind: node.kind,
            pinned: false,
            is_seed: node.is_seed,
        })
        .collect();

    // Create edges for internal links. A genuine self-referential KB link
    // (`source == target` — an authored relationship, distinct from the
    // boundary self-loop CONVENTION below, which uses source==target only
    // as a "no real target to draw" placeholder) used to render with
    // `EdgeStyle::default()` like any other edge: unlabeled and not
    // dashed. `flatten_scene_graph`'s edge loop already special-cases
    // ANY `source == target` edge into the same short offset-stub
    // position as a boundary link (it can't do otherwise — there's no
    // distinct node to draw a line TO), so an undecorated self-link was
    // visually IDENTICAL to rendering noise/an artifact: a bare stub with
    // no explanation (#462 audit). Reuses the boundary stub's own
    // established visual precedent (dashed + a label) so a real self-link
    // instead reads as recognizably intentional.
    let mut scene_edges: Vec<SceneEdge> = links
        .iter()
        .filter_map(|link| {
            let s = *id_to_idx.get(link.source.as_str())?;
            let t = *id_to_idx.get(link.target.as_str())?;
            let is_self_link = s == t;
            Some(SceneEdge {
                source: s,
                target: t,
                label: if is_self_link {
                    Some("self".to_string())
                } else {
                    None
                },
                style: if is_self_link {
                    EdgeStyle {
                        dashed: true,
                        ..EdgeStyle::default()
                    }
                } else {
                    EdgeStyle::default()
                },
                weight: link.weight,
                // Real relationship data survives even for a self-link
                // (unlike the boundary stub below, whose `rel_type` is
                // always `None` — see that field's comment there and
                // `GraphView::describe_state`'s `boundary` flag, which
                // relies on exactly this distinction to tell a genuine
                // self-link apart from a boundary/fringe stub even though
                // both are dashed).
                rel_type: Some(link.rel_type.clone()),
            })
        })
        .collect();

    // Add boundary links as dashed red edges — one per SOURCE node, not
    // one per (source, target) pair. A boundary link's target is never
    // rendered (it's outside the subgraph), so the self-loop below already
    // discards the target's identity — it's a generic "there's more beyond
    // this depth" indicator, not a specific connection. Without
    // deduplicating by source first, a hub node with many out-of-subgraph
    // links (e.g. a category node connected to hundreds of other nodes)
    // produced hundreds of visually-identical, perfectly-overlapping stub
    // edges: pure waste for rendering, and for anything introspecting
    // `SceneGraph.edges` (e.g. `kb-graph-view-state`) it made an otherwise
    // small subgraph look like it had hundreds of edges. The count is
    // preserved in the label instead of silently dropped, so "this node
    // has N more connections beyond what's shown" is still visible.
    //
    // `boundary_order` (first-seen source order, matching the previous
    // `Vec`-scan's output byte-for-byte) + `boundary_counts` (a `HashMap`
    // for O(1) lookup/increment) replaces the old `Vec<(&str, usize)>` +
    // `.iter_mut().find(...)` linear scan — O(boundary_links *
    // distinct_sources) in the worst case (a hub node with hundreds of
    // boundary links was exactly the case this whole dedup exists to
    // handle cheaply) — with O(boundary_links) total.
    let mut boundary_order: Vec<&str> = Vec::new();
    let mut boundary_counts: std::collections::HashMap<&str, usize> =
        std::collections::HashMap::new();
    for link in boundary_links {
        let src = link.source.as_str();
        match boundary_counts.entry(src) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                *e.get_mut() += 1;
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                boundary_order.push(src);
                e.insert(1);
            }
        }
    }
    for src in boundary_order {
        let count = boundary_counts[src];
        if let Some(&s) = id_to_idx.get(src) {
            // Boundary target is outside the graph — just show the outgoing edge
            // pointing to the edge of the source node (no target node rendered).
            // Always include the count, even at 1 — a bare "..." with no count
            // reads as an unexplained stray mark (reported live: a user seeing
            // it had no way to tell it meant "1 more link not shown here" vs.
            // some rendering glitch); "... (+1)" is unambiguous regardless of
            // count.
            let label = format!("... (+{count})");
            scene_edges.push(SceneEdge {
                source: s,
                target: s, // self-loop as visual indicator
                label: Some(label),
                style: EdgeStyle {
                    color: "#ff6666".to_string(),
                    width: 1.0,
                    dashed: true,
                },
                // A boundary stub represents one-or-more collapsed links of
                // possibly differing weight/type — no single value applies,
                // so it's left at the layout-neutral default (self-loops
                // apply zero attraction force regardless, per
                // `ForceLayout::step`).
                weight: 1.0,
                rel_type: None,
            });
        }
    }

    SceneGraph {
        nodes: scene_nodes,
        edges: scene_edges,
        selection: if n > 0 { Some(0) } else { None },
        hovered: None,
    }
}

// --- Multi-KB chord composition (#462 PR4) ---

/// One diagram's worth of KB data for [`build_multi_kb_chord_positions`] —
/// the per-instance input, mirroring what a single-instance
/// `build_kb_graph_chord_positions` call would take, plus the two things a
/// multi-diagram composition additionally needs: which instance this is
/// (`instance`, for the global `(instance, id)` index map cross-instance
/// links resolve through) and a human-readable `name` for the diagram's
/// caption (`mae-canvas` has no `mae-kb`/`KbRegistry` dependency, so the
/// display name must be resolved by the caller — see `KbNodeInfo`'s doc
/// comment for the same no-mae-kb-dependency pattern applied to nodes).
#[derive(Debug, Clone)]
pub struct KbInstanceSubgraph {
    /// `None` = primary, `Some(uuid)` = a federated instance — matches
    /// `GraphView.kb_instance`'s convention.
    pub instance: Option<String>,
    /// Display name for this diagram's caption (e.g. the registered KB
    /// instance's `name`, or "Primary" for the primary KB).
    pub name: String,
    pub nodes: Vec<KbNodeInfo>,
    /// Internal links (both endpoints within this diagram).
    pub links: Vec<KbLinkInfo>,
    /// Boundary links that stay plain stubs for THIS diagram (same-instance
    /// truncated, or unresolvable) — see
    /// `Editor::partition_boundary_links_by_instance`. Genuine cross-instance
    /// links are passed separately, via `build_multi_kb_chord_positions`'s
    /// own `cross_instance_links` parameter, not here.
    pub boundary_links: Vec<KbLinkInfo>,
    /// The BFS seed node id(s) this diagram was extracted from — used to
    /// pick a sensible initial `SceneGraph.selection` (the seed node,
    /// specifically, rather than an arbitrary "first node in iteration
    /// order", which `extract_subgraph`'s `HashSet`-backed node collection
    /// does not guarantee to be the seed).
    pub starter_ids: Vec<String>,
}

/// A cross-instance link for [`build_multi_kb_chord_positions`] — the
/// canvas-crate mirror of `mae_kb::CrossInstanceLink` (same no-mae-kb-
/// dependency pattern as `KbLinkInfo`/`KbNodeInfo`). Carries BOTH endpoints'
/// instance identity (`source_instance` in addition to `target_instance`)
/// — deliberately, unlike `mae_kb::CrossInstanceLink` (which only needs
/// `target_instance`, since its source is always implicitly "the subgraph
/// this was extracted from") — because resolving a global scene index
/// requires an unambiguous `(instance, id)` key: two independently-authored
/// KBs can legitimately reuse the same bare id (e.g. both happen to have a
/// node called `"index"`), so resolving by id alone would risk silently
/// wiring an edge to the WRONG diagram's node.
#[derive(Debug, Clone)]
pub struct KbCrossInstanceLinkInfo {
    pub source: String,
    pub source_instance: Option<String>,
    pub target: String,
    pub target_instance: Option<String>,
    pub rel_type: String,
    pub weight: f64,
}

/// Caption metadata for one diagram in a multi-KB chord composition — the
/// GUI's `flatten_scene_graph`-adjacent render path (`push_diagram_labels`,
/// `crates/core/src/graph_view.rs`) draws `name` as a `VisualElement::Text`
/// anchored above `(center_x, center_y - radius)`; the TUI's
/// `render_graph_view_as_text` groups its "** Neighborhood" listing by
/// these same entries (in order — see that function's doc comment for the
/// contiguous-node-block invariant this relies on).
#[derive(Debug, Clone)]
pub struct DiagramLabel {
    pub instance: Option<String>,
    pub name: String,
    pub center_x: f64,
    pub center_y: f64,
    pub radius: f64,
    pub node_count: usize,
}

/// Build ONE merged `SceneGraph` composing N KB instances' subgraphs as a
/// grid of "small multiples" chord diagrams — issue #462's multi-KB graph
/// view. Each `diagrams[i]` gets its own chord ring (identical trig to
/// `build_kb_graph_chord_positions`, via `chord_ring_positions`), placed at
/// a grid cell computed from `ceil(sqrt(diagrams.len()))` columns, row-major;
/// cell size is NOT fixed — each row/column's extent is the largest radius
/// (`sqrt_area_radius`) among the diagrams occupying it, so a denser diagram
/// gets proportionally more room instead of overlapping a fixed-size
/// neighbor. The whole grid is then re-centered so its bounding box sits at
/// the origin — this is what makes the `diagrams.len() == 1` case produce
/// output IDENTICAL to a plain `build_kb_graph_chord_positions` call (see
/// the consistency-guard test), not merely "close".
///
/// `cross_instance_links` are resolved against the SAME global `(instance,
/// id) -> index` map built while merging each diagram's own nodes in, and
/// turned into ordinary `SceneEdge`s (dashed, carrying a real `rel_type` —
/// the same "dashed + `rel_type: Some(..)`" convention `positions_to_scene`
/// already uses for a genuine self-link, so `GraphView::describe_state`'s
/// existing boundary-vs-real-edge disambiguation keeps working unmodified).
/// A link whose `target_instance` isn't among the rendered `diagrams`
/// (stale/unregistered/filtered by `kb_graph_multi_kb_max_related_instances`)
/// is DROPPED WITH A COUNT (the returned `usize`) — mirroring
/// `SubgraphResult::hidden_node_count`'s "never silently lost" precedent —
/// rather than panicking on a dangling index or being silently omitted.
///
/// @ai-caution: [architecture] Cross-instance links are resolved into
/// ordinary `SceneEdge`s against the MERGED node index space built INSIDE
/// this function. Any future caller building a `SceneGraph` outside this
/// function must not construct cross-instance `SceneEdge`s by hand against
/// per-diagram-LOCAL indices — the whole point of merging here is that
/// `SceneEdge`'s index is only ever valid within its OWN `SceneGraph`; a
/// per-diagram-local index is meaningless once nodes from multiple diagrams
/// share one `nodes` vec.
///
/// `grid_gap_factor` is the extra spacing between adjacent grid cells, as a
/// FRACTION of the larger-radius diagram sharing that row/column boundary —
/// never a fixed pixel gap, so the breathing room (needed for each diagram's
/// own name caption, drawn just above it) scales the same sub-linear way
/// every other distance in this module does (see `sqrt_area_radius`'s doc
/// comment). Mirrors `spacing_scale`'s own plumbing: `mae-canvas` has no
/// `OptionRegistry`/`Editor` access, so this is threaded in as a plain
/// function parameter — the caller (`kb_graph_multi_kb_grid_gap_factor`
/// option, `crates/core/src/editor/graph_view_ops.rs`) is the one place that
/// can read it from the registry.
pub fn build_multi_kb_chord_positions(
    diagrams: &[KbInstanceSubgraph],
    cross_instance_links: &[KbCrossInstanceLinkInfo],
    spacing_scale: f64,
    grid_gap_factor: f64,
) -> (SceneGraph, Vec<DiagramLabel>, usize) {
    if diagrams.is_empty() {
        return (SceneGraph::new(), Vec::new(), cross_instance_links.len());
    }

    let cols = (diagrams.len() as f64).sqrt().ceil().max(1.0) as usize;
    let rows = diagrams.len().div_ceil(cols);

    let radii: Vec<f64> = diagrams
        .iter()
        .map(|d| sqrt_area_radius(d.nodes.len(), spacing_scale))
        .collect();

    let mut row_max = vec![0.0_f64; rows];
    let mut col_max = vec![0.0_f64; cols];
    for (i, &r) in radii.iter().enumerate() {
        row_max[i / cols] = row_max[i / cols].max(r);
        col_max[i % cols] = col_max[i % cols].max(r);
    }

    // Cumulative cell centers along each axis: column `c`'s center sits
    // `col_max[c]` past the previous column's far edge, then the cursor
    // advances by `col_max[c] * (1 + grid_gap_factor)` to clear this
    // column's far edge plus breathing room before the next one. Rows
    // mirror this exactly.
    let mut col_x = vec![0.0_f64; cols];
    let mut cursor = 0.0_f64;
    for (c, slot) in col_x.iter_mut().enumerate() {
        cursor += col_max[c];
        *slot = cursor;
        cursor += col_max[c] * (1.0 + grid_gap_factor);
    }
    let mut row_y = vec![0.0_f64; rows];
    let mut cursor = 0.0_f64;
    for (r, slot) in row_y.iter_mut().enumerate() {
        cursor += row_max[r];
        *slot = cursor;
        cursor += row_max[r] * (1.0 + grid_gap_factor);
    }

    // Re-center the whole grid's bounding box on the origin — see this
    // function's doc comment for why this is what makes the single-diagram
    // case reproduce `build_kb_graph_chord_positions` byte-for-byte.
    let center_of = |coords: &[f64], maxes: &[f64]| -> f64 {
        let min = coords[0] - maxes[0];
        let max = coords[coords.len() - 1] + maxes[maxes.len() - 1];
        (min + max) / 2.0
    };
    let grid_center_x = center_of(&col_x, &col_max);
    for x in col_x.iter_mut() {
        *x -= grid_center_x;
    }
    let grid_center_y = center_of(&row_y, &row_max);
    for y in row_y.iter_mut() {
        *y -= grid_center_y;
    }

    let mut global_nodes: Vec<SceneNode> = Vec::new();
    let mut global_edges: Vec<SceneEdge> = Vec::new();
    let mut labels: Vec<DiagramLabel> = Vec::with_capacity(diagrams.len());
    // Keyed by (instance, node id) rather than bare id — see
    // `KbCrossInstanceLinkInfo`'s doc comment for why bare-id resolution
    // would be ambiguous across independently-authored KBs.
    let mut index_map: std::collections::HashMap<(Option<String>, String), usize> =
        std::collections::HashMap::new();
    let mut selection: Option<usize> = None;

    for (i, diagram) in diagrams.iter().enumerate() {
        let (cx, cy) = (col_x[i % cols], row_y[i / cols]);
        let local_positions: Vec<(f64, f64)> =
            chord_ring_positions(diagram.nodes.len(), spacing_scale)
                .into_iter()
                .map(|(x, y)| (x + cx, y + cy))
                .collect();
        let local_id_to_idx: std::collections::HashMap<&str, usize> = diagram
            .nodes
            .iter()
            .enumerate()
            .map(|(j, n)| (n.id.as_str(), j))
            .collect();
        let local_scene = positions_to_scene(
            &diagram.nodes,
            &diagram.links,
            &diagram.boundary_links,
            &local_id_to_idx,
            &local_positions,
        );

        let base = global_nodes.len();
        for (j, node) in local_scene.nodes.into_iter().enumerate() {
            index_map.insert(
                (diagram.instance.clone(), diagram.nodes[j].id.clone()),
                base + j,
            );
            global_nodes.push(node);
        }
        for edge in local_scene.edges {
            global_edges.push(SceneEdge {
                source: edge.source + base,
                target: edge.target + base,
                ..edge
            });
        }
        if selection.is_none() {
            selection = diagram.starter_ids.iter().find_map(|sid| {
                index_map
                    .get(&(diagram.instance.clone(), sid.clone()))
                    .copied()
            });
        }
        labels.push(DiagramLabel {
            instance: diagram.instance.clone(),
            name: diagram.name.clone(),
            center_x: cx,
            center_y: cy,
            radius: radii[i],
            node_count: diagram.nodes.len(),
        });
    }

    // Final precise re-centering pass on the ACTUAL populated node
    // positions (not just the grid cells above) — skipped for a single
    // diagram, to keep that case's consistency guarantee (see this
    // function's doc comment) exact.
    //
    // The grid-cell centering above assumes each diagram's own local
    // chord ring is symmetric around ITS cell center — true for the
    // CENTROID of an n>=2-node ring (points evenly spaced around a circle
    // always average to the center by rotational symmetry), but NOT
    // generally true of the ring's bounding box (e.g. 3 points at 0°/120°/
    // 240° have an x-extent of `[-r/2, r]`, not `[-r, r]`), and actively
    // FALSE for a 1-node "ring" (a single point sits at local `(radius,
    // 0)`, nowhere near its own cell center). Left uncorrected, a small
    // (especially 1-node) diagram skews the merged scene's TRUE bounding
    // box away from the origin that `zoom_to_fit`'s viewport-center-stays-
    // at-`(0,0)` assumption relies on — confirmed live by a real 2-diagram
    // (1 node each) test case whose fitted zoom still let a node's screen
    // position land outside the viewport.
    if diagrams.len() > 1 && !global_nodes.is_empty() {
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for n in &global_nodes {
            min_x = min_x.min(n.x);
            max_x = max_x.max(n.x);
            min_y = min_y.min(n.y);
            max_y = max_y.max(n.y);
        }
        let shift_x = (min_x + max_x) / 2.0;
        let shift_y = (min_y + max_y) / 2.0;
        for n in global_nodes.iter_mut() {
            n.x -= shift_x;
            n.y -= shift_y;
        }
        for label in labels.iter_mut() {
            label.center_x -= shift_x;
            label.center_y -= shift_y;
        }
    }

    // Fall back to "first node overall", mirroring
    // `positions_to_scene`'s own `Some(0)`-if-nonempty convention, when no
    // starter id could be resolved (e.g. a diagram whose starter node
    // itself got truncated out by a node cap).
    if selection.is_none() && !global_nodes.is_empty() {
        selection = Some(0);
    }

    let mut hidden_cross_instance_link_count = 0;
    for link in cross_instance_links {
        let src_key = (link.source_instance.clone(), link.source.clone());
        let tgt_key = (link.target_instance.clone(), link.target.clone());
        match (index_map.get(&src_key), index_map.get(&tgt_key)) {
            (Some(&s), Some(&t)) => {
                let target_diagram_name = diagrams
                    .iter()
                    .find(|d| d.instance == link.target_instance)
                    .map(|d| d.name.clone());
                global_edges.push(SceneEdge {
                    source: s,
                    target: t,
                    label: target_diagram_name.map(|n| format!("→ {n}")),
                    // Reuses the established "dashed + real rel_type"
                    // convention a genuine self-link already renders with
                    // (see `positions_to_scene`'s doc comment) — visually
                    // distinguishable from a plain internal edge via the
                    // boundary-edge color, while `describe_state`'s
                    // `rel_type.is_none()` check correctly does NOT
                    // classify this as a boundary/fringe stub (it carries
                    // a real `rel_type`, same as a self-link).
                    style: EdgeStyle {
                        dashed: true,
                        ..EdgeStyle::default()
                    },
                    weight: link.weight,
                    rel_type: Some(link.rel_type.clone()),
                });
            }
            _ => hidden_cross_instance_link_count += 1,
        }
    }

    (
        SceneGraph {
            nodes: global_nodes,
            edges: global_edges,
            selection,
            hovered: None,
        },
        labels,
        hidden_cross_instance_link_count,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(source: &str, target: &str) -> KbLinkInfo {
        KbLinkInfo {
            source: source.to_string(),
            target: target.to_string(),
            rel_type: "references".to_string(),
            weight: 1.0,
        }
    }

    fn nodes_and_links() -> (Vec<KbNodeInfo>, Vec<KbLinkInfo>) {
        let nodes = vec![
            KbNodeInfo {
                id: "concept:buffer".to_string(),
                title: "Buffer".to_string(),
                kind: NodeKind::Concept,
                is_seed: false,
            },
            KbNodeInfo {
                id: "concept:window".to_string(),
                title: "Window".to_string(),
                kind: NodeKind::Concept,
                is_seed: false,
            },
            KbNodeInfo {
                id: "cmd:save".to_string(),
                title: "Save".to_string(),
                kind: NodeKind::Command,
                is_seed: false,
            },
        ];
        let links = vec![
            link("concept:buffer", "concept:window"),
            link("cmd:save", "concept:buffer"),
        ];
        (nodes, links)
    }

    #[test]
    fn build_graph_node_count() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], 1.0);
        assert_eq!(graph.nodes.len(), 3);
    }

    #[test]
    fn build_graph_edge_count() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], 1.0);
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn build_graph_with_boundary() {
        let (nodes, links) = nodes_and_links();
        let boundary = vec![link("concept:buffer", "external:xyz")];
        let graph = build_kb_graph(&nodes, &links, &boundary, 1.0);
        // 2 internal + 1 boundary edge
        assert_eq!(graph.edges.len(), 3);
        assert!(graph.edges[2].style.dashed);
    }

    #[test]
    fn build_graph_boundary_links_from_the_same_source_collapse_to_one_edge() {
        // Regression guard: a hub node (e.g. a category node) with MANY
        // out-of-subgraph links previously produced one visually-identical,
        // perfectly-overlapping self-loop stub edge PER boundary link —
        // e.g. 150 boundary links from one source node meant 150 duplicate
        // edges. Real distinct targets collapse to one edge per source,
        // since the self-loop already discards target identity.
        let (nodes, links) = nodes_and_links();
        let boundary = vec![
            link("concept:buffer", "external:a"),
            link("concept:buffer", "external:b"),
            link("concept:buffer", "external:c"),
        ];
        let graph = build_kb_graph(&nodes, &links, &boundary, 1.0);
        // 2 internal + 1 collapsed boundary edge (not 3 boundary edges).
        assert_eq!(graph.edges.len(), 3);
        let boundary_edge = &graph.edges[2];
        assert!(boundary_edge.style.dashed);
        assert_eq!(boundary_edge.source, boundary_edge.target);
        assert_eq!(boundary_edge.label.as_deref(), Some("... (+3)"));
    }

    #[test]
    fn build_graph_boundary_links_from_different_sources_stay_separate() {
        let (nodes, links) = nodes_and_links();
        let boundary = vec![
            link("concept:buffer", "external:a"),
            link("concept:window", "external:b"),
        ];
        let graph = build_kb_graph(&nodes, &links, &boundary, 1.0);
        // 2 internal + 2 boundary edges (one per distinct source, each
        // with its own count-1 label — always includes the count, even at
        // 1, so the label is unambiguous rather than a bare "...").
        assert_eq!(graph.edges.len(), 4);
        let boundary_edges: Vec<_> = graph.edges[2..].to_vec();
        assert_eq!(boundary_edges.len(), 2);
        assert!(boundary_edges
            .iter()
            .all(|e| e.label.as_deref() == Some("... (+1)")));
        let sources: std::collections::HashSet<_> =
            boundary_edges.iter().map(|e| e.source).collect();
        assert_eq!(sources.len(), 2, "each source keeps its own boundary edge");
    }

    #[test]
    fn build_graph_boundary_links_preserve_first_seen_source_order_and_per_source_counts() {
        // Regression guard for the O(n^2)-ish `Vec` + linear-scan dedup ->
        // `HashMap`-backed dedup swap (#462 audit finding): a `HashMap`'s
        // OWN iteration order is unspecified, so the fix must explicitly
        // track first-seen order in a side `Vec` rather than iterating the
        // map directly — this pins that the boundary stub edges come out
        // in the exact order their sources FIRST appeared in
        // `boundary_links`, with each source's own count aggregated
        // correctly, regardless of how the links interleave.
        let (mut nodes, links) = nodes_and_links();
        nodes.push(KbNodeInfo {
            id: "concept:extra".to_string(),
            title: "Extra".to_string(),
            kind: NodeKind::Concept,
            is_seed: false,
        });
        let boundary = vec![
            link("concept:window", "external:a"), // window first-seen
            link("concept:buffer", "external:b"), // buffer first-seen
            link("concept:window", "external:c"), // window again (count 2)
            link("concept:extra", "external:d"),  // extra first-seen
            link("concept:buffer", "external:e"), // buffer again (count 2)
        ];
        let graph = build_kb_graph(&nodes, &links, &boundary, 1.0);
        let boundary_edges: Vec<_> = graph.edges[2..].to_vec();
        assert_eq!(
            boundary_edges.len(),
            3,
            "one collapsed edge per distinct source"
        );

        let window_idx = nodes.iter().position(|n| n.id == "concept:window").unwrap();
        let buffer_idx = nodes.iter().position(|n| n.id == "concept:buffer").unwrap();
        let extra_idx = nodes.iter().position(|n| n.id == "concept:extra").unwrap();

        assert_eq!(
            boundary_edges[0].source, window_idx,
            "first-seen source (window) must be the first boundary stub"
        );
        assert_eq!(boundary_edges[0].label.as_deref(), Some("... (+2)"));
        assert_eq!(
            boundary_edges[1].source, buffer_idx,
            "second-seen source (buffer) must be the second boundary stub"
        );
        assert_eq!(boundary_edges[1].label.as_deref(), Some("... (+2)"));
        assert_eq!(
            boundary_edges[2].source, extra_idx,
            "third-seen source (extra) must be the third boundary stub"
        );
        assert_eq!(boundary_edges[2].label.as_deref(), Some("... (+1)"));
    }

    #[test]
    fn build_graph_self_link_renders_as_a_distinguishable_dashed_labeled_stub() {
        // #462 audit fix: a genuine self-referential KB link (source ==
        // target, an authored relationship — NOT the boundary-link
        // convention, which uses source==target only as a "no real
        // target to draw" placeholder) previously rendered with
        // `EdgeStyle::default()` like any other internal link: unlabeled,
        // not dashed — visually IDENTICAL to rendering noise, since
        // `flatten_scene_graph` already draws any `source == target` edge
        // as a short offset stub regardless. It must now be
        // distinguishable: dashed, and carrying a "self" label.
        let (nodes, links) = nodes_and_links();
        let mut links = links;
        links.push(link("concept:buffer", "concept:buffer"));
        let graph = build_kb_graph(&nodes, &links, &[], 1.0);

        let self_link = graph
            .edges
            .iter()
            .find(|e| e.source == e.target)
            .expect("the self-link must produce an edge");
        assert!(
            self_link.style.dashed,
            "a genuine self-link must be dashed, distinguishing it from a plain unlabeled stub"
        );
        assert_eq!(
            self_link.label.as_deref(),
            Some("self"),
            "a genuine self-link must carry a recognizable label"
        );
        // Unlike a boundary stub (whose rel_type is always discarded/
        // None), the self-link's real relationship type survives — this
        // is exactly what `GraphView::describe_state` uses to tell the
        // two apart despite both being dashed.
        assert_eq!(self_link.rel_type.as_deref(), Some("references"));
    }

    #[test]
    fn build_graph_uses_the_kind_passed_in_kb_node_info() {
        // The kind comes straight from KbNodeInfo now (no more id-string
        // guessing) — a node whose id looks like a concept but is tagged
        // Task must come out as Task.
        let nodes = vec![KbNodeInfo {
            id: "concept:not-really".to_string(),
            title: "Fooled you".to_string(),
            kind: NodeKind::Task,
            is_seed: false,
        }];
        let graph = build_kb_graph(&nodes, &[], &[], 1.0);
        assert_eq!(graph.nodes[0].kind, NodeKind::Task);
    }

    #[test]
    fn positions_only_skips_force_layout() {
        // Nodes placed on the initial sunflower-spiral stay EXACTLY there
        // (no force-layout displacement) — confirms
        // `build_kb_graph_positions_only` really is layout-free, the
        // property `graph_view_ops.rs` depends on to defer layout to the
        // background bridge.
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph_positions_only(&nodes, &links, &[], 1.0);
        let n = nodes.len();
        let disk_radius = ((n as f64 * crate::layout::IDEAL_AREA_PER_NODE * 1.0)
            / std::f64::consts::PI)
            .sqrt()
            .max(100.0);
        let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
        for (i, node) in graph.nodes.iter().enumerate() {
            let r = disk_radius * (((i as f64) + 0.5) / (n.max(1) as f64)).sqrt();
            let angle = (i as f64) * golden_angle;
            assert!((node.x - r * angle.cos()).abs() < 1e-9);
            assert!((node.y - r * angle.sin()).abs() < 1e-9);
        }
    }

    #[test]
    fn positions_only_spacing_stays_roughly_constant_regardless_of_node_count() {
        // The whole point of the sunflower distribution over a plain ring:
        // average nearest-neighbor spacing must NOT shrink toward zero as
        // the node count grows (a ring's does, since its local density is
        // 1/n independent of radius) — it should stay near
        // `sqrt(IDEAL_AREA_PER_NODE)` = 100 regardless of n, so nodes never
        // start out visually/hit-test overlapping no matter how large a KB
        // subgraph is opened.
        fn min_pairwise_dist(n: usize) -> f64 {
            let nodes: Vec<KbNodeInfo> = (0..n)
                .map(|i| KbNodeInfo {
                    id: format!("n{i}"),
                    title: "x".to_string(),
                    kind: NodeKind::Concept,
                    is_seed: false,
                })
                .collect();
            let graph = build_kb_graph_positions_only(&nodes, &[], &[], 1.0);
            let mut min_dist = f64::MAX;
            for i in 0..graph.nodes.len() {
                for j in (i + 1)..graph.nodes.len() {
                    let dx = graph.nodes[i].x - graph.nodes[j].x;
                    let dy = graph.nodes[i].y - graph.nodes[j].y;
                    min_dist = min_dist.min((dx * dx + dy * dy).sqrt());
                }
            }
            min_dist
        }

        // A real KB-sized subgraph (matches the ~1000-node depth-2 case
        // observed live) must still keep its nodes meaningfully apart, not
        // crammed into overlapping hit-circles.
        let min_dist_large = min_pairwise_dist(1000);
        assert!(
            min_dist_large > 30.0,
            "min pairwise spacing at n=1000 collapsed to {min_dist_large}, nodes will overlap"
        );
    }

    #[test]
    fn positions_only_initial_spread_scales_with_spacing_scale() {
        // Regression guard for the exact failure mode this parameter exists
        // to avoid: raising `spacing_scale` must widen the INITIAL sunflower
        // placement's spread too, not just the force layout's later
        // equilibrium distance — otherwise a large `spacing_scale` default
        // would reproduce "nodes settled having barely moved off their
        // initial ring" on every graph-view open.
        let nodes: Vec<KbNodeInfo> = (0..50)
            .map(|i| KbNodeInfo {
                id: format!("n{i}"),
                title: "x".to_string(),
                kind: NodeKind::Concept,
                is_seed: false,
            })
            .collect();
        let tight = build_kb_graph_positions_only(&nodes, &[], &[], 1.0);
        let wide = build_kb_graph_positions_only(&nodes, &[], &[], 4.0);
        let max_radius = |g: &SceneGraph| {
            g.nodes
                .iter()
                .map(|n| (n.x * n.x + n.y * n.y).sqrt())
                .fold(0.0_f64, f64::max)
        };
        assert!(
            max_radius(&wide) > max_radius(&tight),
            "a larger spacing_scale must widen the initial placement's spread"
        );
    }

    #[test]
    fn positions_only_and_full_agree_on_topology() {
        // Same node/edge count either way — only the coordinates differ
        // (before vs after layout).
        let (nodes, links) = nodes_and_links();
        let positions_only = build_kb_graph_positions_only(&nodes, &links, &[], 1.0);
        let full = build_kb_graph(&nodes, &links, &[], 1.0);
        assert_eq!(positions_only.nodes.len(), full.nodes.len());
        assert_eq!(positions_only.edges.len(), full.edges.len());
    }

    #[test]
    fn build_graph_empty() {
        let graph = build_kb_graph(&[], &[], &[], 1.0);
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn force_layout_separates_nodes() {
        let nodes = vec![
            KbNodeInfo {
                id: "a".to_string(),
                title: "A".to_string(),
                kind: NodeKind::Note,
                is_seed: false,
            },
            KbNodeInfo {
                id: "b".to_string(),
                title: "B".to_string(),
                kind: NodeKind::Note,
                is_seed: false,
            },
            KbNodeInfo {
                id: "c".to_string(),
                title: "C".to_string(),
                kind: NodeKind::Note,
                is_seed: false,
            },
        ];
        let links = vec![link("a", "b"), link("b", "c")];
        let graph = build_kb_graph(&nodes, &links, &[], 1.0);
        // After layout, nodes should not be at identical positions
        let positions: Vec<(f64, f64)> = graph.nodes.iter().map(|n| (n.x, n.y)).collect();
        for i in 0..positions.len() {
            for j in (i + 1)..positions.len() {
                let dist = ((positions[i].0 - positions[j].0).powi(2)
                    + (positions[i].1 - positions[j].1).powi(2))
                .sqrt();
                assert!(dist > 1.0, "nodes {} and {} too close: dist={}", i, j, dist);
            }
        }
    }

    // --- #367: chord-diagram (circular) layout ---

    #[test]
    fn chord_positions_preserves_node_count() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph_chord_positions(&nodes, &links, &[], 1.0);
        assert_eq!(graph.nodes.len(), 3);
    }

    #[test]
    fn chord_positions_every_node_is_equidistant_from_the_origin() {
        let nodes: Vec<KbNodeInfo> = (0..8)
            .map(|i| KbNodeInfo {
                id: format!("n{i}"),
                title: "x".to_string(),
                kind: NodeKind::Concept,
                is_seed: false,
            })
            .collect();
        let graph = build_kb_graph_chord_positions(&nodes, &[], &[], 1.0);
        let radii: Vec<f64> = graph
            .nodes
            .iter()
            .map(|n| (n.x * n.x + n.y * n.y).sqrt())
            .collect();
        let first = radii[0];
        for (i, r) in radii.iter().enumerate() {
            assert!(
                (r - first).abs() < 1e-6,
                "node {i} radius {r} differs from node 0's radius {first} — not on a circle"
            );
        }
    }

    #[test]
    fn chord_positions_are_evenly_angularly_spaced() {
        // Adversarial (#14): assert the ACTUAL angular spacing between
        // adjacent nodes is uniform, not just that positions differ — a
        // plausible-but-wrong bunched-up placement (e.g. all nodes crammed
        // into one arc) would still produce "different" positions but
        // would fail this specific check.
        let n = 6;
        let nodes: Vec<KbNodeInfo> = (0..n)
            .map(|i| KbNodeInfo {
                id: format!("n{i}"),
                title: "x".to_string(),
                kind: NodeKind::Concept,
                is_seed: false,
            })
            .collect();
        let graph = build_kb_graph_chord_positions(&nodes, &[], &[], 1.0);
        let angles: Vec<f64> = graph.nodes.iter().map(|n| n.y.atan2(n.x)).collect();
        let mut sorted = angles.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let expected_gap = 2.0 * std::f64::consts::PI / n as f64;
        for i in 0..sorted.len() {
            let next = sorted[(i + 1) % sorted.len()];
            let gap = if i + 1 < sorted.len() {
                next - sorted[i]
            } else {
                (next + 2.0 * std::f64::consts::PI) - sorted[i]
            };
            assert!(
                (gap - expected_gap).abs() < 1e-6,
                "angular gap {gap} at index {i} differs from expected {expected_gap} — nodes not evenly spaced"
            );
        }
    }

    #[test]
    fn chord_positions_radius_grows_sub_linearly_with_node_count() {
        // Regression guard for the field bug this ratio was rewritten to
        // fix: a ring radius growing LINEARLY with n (to hold adjacent-node
        // arc spacing constant) blew up to tens of thousands of scene units
        // on a real ~1300-node subgraph — far past what the shared
        // [0.1, 10.0] zoom range can ever zoom out far enough to fit. The
        // radius must instead grow at the same sub-linear (sqrt-of-n) rate
        // as the sunflower disk, matching `build_kb_graph_positions_only`'s
        // scale.
        fn radius_for(n: usize) -> f64 {
            let nodes: Vec<KbNodeInfo> = (0..n)
                .map(|i| KbNodeInfo {
                    id: format!("n{i}"),
                    title: "x".to_string(),
                    kind: NodeKind::Concept,
                    is_seed: false,
                })
                .collect();
            let graph = build_kb_graph_chord_positions(&nodes, &[], &[], 1.0);
            (graph.nodes[0].x.powi(2) + graph.nodes[0].y.powi(2)).sqrt()
        }
        // 25x the node count (8 -> 200) should grow radius by sqrt(25) = 5x,
        // not by 25x (linear) — assert it lands close to the sqrt
        // prediction and nowhere near the linear one.
        let small = radius_for(8);
        let large = radius_for(200);
        let ratio = large / small;
        assert!(
            (ratio - 5.0).abs() < 0.5,
            "radius ratio for n=8->200 was {ratio}, expected ~5.0 (sqrt(25)) for sub-linear growth"
        );
        assert!(
            ratio < 25.0 * 0.5,
            "radius ratio {ratio} is too close to linear (25x) growth — the field-bug regression"
        );
    }

    #[test]
    fn chord_positions_edges_match_positions_only_edge_building() {
        // Same shared edge-building logic as build_kb_graph_positions_only
        // (only node placement differs) — internal + boundary link counts
        // must match exactly.
        let (nodes, links) = nodes_and_links();
        let boundary = vec![
            link("concept:buffer", "external:a"),
            link("concept:buffer", "external:b"),
        ];
        let chord = build_kb_graph_chord_positions(&nodes, &links, &boundary, 1.0);
        let positions_only = build_kb_graph_positions_only(&nodes, &links, &boundary, 1.0);
        assert_eq!(chord.edges.len(), positions_only.edges.len());
        assert_eq!(chord.edges.len(), 3); // 2 internal + 1 collapsed boundary stub
    }

    #[test]
    fn chord_positions_empty_graph() {
        let graph = build_kb_graph_chord_positions(&[], &[], &[], 1.0);
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }

    // --- #462: multi-KB chord composition ---

    fn diagram(instance: Option<&str>, name: &str, ids: &[&str]) -> KbInstanceSubgraph {
        KbInstanceSubgraph {
            instance: instance.map(str::to_string),
            name: name.to_string(),
            nodes: ids
                .iter()
                .map(|id| KbNodeInfo {
                    id: id.to_string(),
                    title: id.to_string(),
                    kind: NodeKind::Concept,
                    is_seed: false,
                })
                .collect(),
            links: Vec::new(),
            boundary_links: Vec::new(),
            starter_ids: ids.first().map(|s| s.to_string()).into_iter().collect(),
        }
    }

    fn cross_link(
        source: &str,
        source_instance: Option<&str>,
        target: &str,
        target_instance: Option<&str>,
    ) -> KbCrossInstanceLinkInfo {
        KbCrossInstanceLinkInfo {
            source: source.to_string(),
            source_instance: source_instance.map(str::to_string),
            target: target.to_string(),
            target_instance: target_instance.map(str::to_string),
            rel_type: "references".to_string(),
            weight: 1.0,
        }
    }

    #[test]
    fn multi_kb_single_diagram_matches_plain_chord_positions_byte_for_byte() {
        // Consistency guard: a single diagram with no cross-instance links
        // must reproduce `build_kb_graph_chord_positions`'s own output
        // exactly — the re-centering math must collapse to a no-op for n=1.
        let d = diagram(None, "Primary", &["a", "b", "c"]);
        let (multi_scene, labels, hidden) =
            build_multi_kb_chord_positions(std::slice::from_ref(&d), &[], 1.0, 0.6);
        let plain = build_kb_graph_chord_positions(&d.nodes, &d.links, &d.boundary_links, 1.0);

        assert_eq!(hidden, 0);
        assert_eq!(labels.len(), 1);
        assert_eq!(multi_scene.nodes.len(), plain.nodes.len());
        for (m, p) in multi_scene.nodes.iter().zip(plain.nodes.iter()) {
            assert!((m.x - p.x).abs() < 1e-9, "x mismatch: {} vs {}", m.x, p.x);
            assert!((m.y - p.y).abs() < 1e-9, "y mismatch: {} vs {}", m.y, p.y);
        }
    }

    #[test]
    fn multi_kb_grid_gap_factor_actually_widens_inter_diagram_spacing() {
        // A6 config-gap fix: `grid_gap_factor` replaced a hardcoded
        // `DIAGRAM_GRID_GAP_FACTOR = 0.6` constant. This proves the
        // parameter is genuinely wired into the layout math (not merely
        // accepted and ignored) — a larger factor must strictly widen the
        // gap between adjacent diagrams' label centers, and 0.0 must pack
        // them tighter than the old 0.6 default.
        let diagrams = || {
            vec![
                diagram(None, "Primary", &["a1", "a2", "a3"]),
                diagram(Some("uuid-b"), "Notes", &["b1", "b2", "b3"]),
            ]
        };
        let center_gap = |gap_factor: f64| {
            let (_, labels, _) = build_multi_kb_chord_positions(&diagrams(), &[], 1.0, gap_factor);
            assert_eq!(labels.len(), 2);
            (labels[0].center_x - labels[1].center_x).abs()
        };

        let gap_zero = center_gap(0.0);
        let gap_default = center_gap(0.6);
        let gap_wide = center_gap(2.0);
        assert!(
            gap_zero < gap_default,
            "0.0 gap ({gap_zero}) must pack tighter than the 0.6 default ({gap_default})"
        );
        assert!(
            gap_default < gap_wide,
            "the 0.6 default ({gap_default}) must be tighter than a 2.0 gap ({gap_wide})"
        );
    }

    #[test]
    fn multi_kb_two_instances_do_not_overlap() {
        let a = diagram(None, "Primary", &["a1", "a2", "a3"]);
        let b = diagram(Some("uuid-b"), "Notes", &["b1", "b2", "b3"]);
        let (scene, labels, hidden) = build_multi_kb_chord_positions(&[a, b], &[], 1.0, 0.6);
        assert_eq!(hidden, 0);
        assert_eq!(labels.len(), 2);
        assert_eq!(scene.nodes.len(), 6);

        // No two nodes from DIFFERENT diagrams may be closer than either
        // diagram's own node-to-node spacing would allow — a coarse,
        // adversarial "did the grid actually separate them" check rather
        // than merely asserting the labels' center points differ.
        let dist = |i: usize, j: usize| {
            let (n1, n2) = (&scene.nodes[i], &scene.nodes[j]);
            ((n1.x - n2.x).powi(2) + (n1.y - n2.y).powi(2)).sqrt()
        };
        let min_cross_diagram_dist = (0..3)
            .flat_map(|i| (3..6).map(move |j| (i, j)))
            .map(|(i, j)| dist(i, j))
            .fold(f64::MAX, f64::min);
        let min_radius = labels.iter().map(|l| l.radius).fold(f64::MAX, f64::min);
        assert!(
            min_cross_diagram_dist > min_radius,
            "cross-diagram node distance {min_cross_diagram_dist} did not clear a single \
             diagram's own radius {min_radius} — diagrams likely overlap"
        );
    }

    #[test]
    fn multi_kb_three_instances_grid_is_two_columns_and_every_diagram_present() {
        // Adversarial (#14): three instances, not two — ceil(sqrt(3)) == 2
        // columns, so this exercises the partial-last-row grid path a
        // 2-instance test never reaches.
        let diagrams = vec![
            diagram(None, "Primary", &["a"]),
            diagram(Some("uuid-b"), "Beta", &["b"]),
            diagram(Some("uuid-c"), "Gamma", &["c"]),
        ];
        let (scene, labels, hidden) = build_multi_kb_chord_positions(&diagrams, &[], 1.0, 0.6);
        assert_eq!(hidden, 0);
        assert_eq!(scene.nodes.len(), 3);
        assert_eq!(labels.len(), 3);
        for name in ["Primary", "Beta", "Gamma"] {
            assert!(
                labels.iter().any(|l| l.name == name),
                "diagram '{name}' missing from labels"
            );
        }
        // ceil(sqrt(3)) == 2 distinct column x-centers among the 3 diagrams.
        let mut xs: Vec<f64> = labels.iter().map(|l| l.center_x).collect();
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        assert_eq!(
            xs.len(),
            2,
            "expected ceil(sqrt(3)) = 2 distinct grid columns, got {}",
            xs.len()
        );
    }

    #[test]
    fn multi_kb_cross_link_resolves_to_correct_global_index_scene_edge() {
        let a = diagram(None, "Primary", &["a1", "a2"]);
        let b = diagram(Some("uuid-b"), "Notes", &["b1", "b2"]);
        let links = vec![cross_link("a1", None, "b2", Some("uuid-b"))];
        let (scene, _labels, hidden) = build_multi_kb_chord_positions(&[a, b], &links, 1.0, 0.6);
        assert_eq!(hidden, 0);
        // Global layout: diagram a occupies [0,2), diagram b occupies [2,4).
        // a1 -> index 0, b2 -> index 3.
        let cross_edge = scene
            .edges
            .iter()
            .find(|e| e.rel_type.as_deref() == Some("references") && e.style.dashed)
            .expect("cross-instance edge must be present");
        assert_eq!(cross_edge.source, 0, "a1 must resolve to global index 0");
        assert_eq!(cross_edge.target, 3, "b2 must resolve to global index 3");
    }

    #[test]
    fn multi_kb_cross_link_to_an_unrendered_instance_is_dropped_with_a_count_not_a_panic() {
        let a = diagram(None, "Primary", &["a1"]);
        // Target instance "uuid-ghost" is not among the rendered diagrams —
        // simulates a stale/unregistered/filtered-out related instance.
        let links = vec![cross_link("a1", None, "ghost-node", Some("uuid-ghost"))];
        let (scene, labels, hidden) = build_multi_kb_chord_positions(&[a], &links, 1.0, 0.6);
        assert_eq!(
            hidden, 1,
            "the dangling cross-link must be counted, not silently lost"
        );
        assert_eq!(labels.len(), 1);
        // No edge was fabricated for it, and nothing panicked resolving it.
        assert!(scene
            .edges
            .iter()
            .all(|e| !(e.style.dashed && e.rel_type.as_deref() == Some("references"))));
    }

    #[test]
    fn multi_kb_empty_diagrams_produces_an_empty_scene_and_counts_every_link_as_hidden() {
        let links = vec![cross_link("a1", None, "b1", Some("uuid-b"))];
        let (scene, labels, hidden) = build_multi_kb_chord_positions(&[], &links, 1.0, 0.6);
        assert!(scene.nodes.is_empty());
        assert!(labels.is_empty());
        assert_eq!(hidden, 1);
    }
}
