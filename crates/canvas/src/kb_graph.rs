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
use crate::scene::{EdgeStyle, NodeKind, NodeStyle, SceneEdge, SceneGraph, SceneNode};

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
/// - `starter_ids`: IDs of the starting nodes (highlighted)
pub fn build_kb_graph(
    nodes: &[KbNodeInfo],
    links: &[KbLinkInfo],
    boundary_links: &[KbLinkInfo],
    starter_ids: &[String],
    spacing_scale: f64,
) -> SceneGraph {
    let mut graph =
        build_kb_graph_positions_only(nodes, links, boundary_links, starter_ids, spacing_scale);

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
    starter_ids: &[String],
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
    let disk_radius = ((n as f64 * crate::layout::IDEAL_AREA_PER_NODE * spacing_scale)
        / std::f64::consts::PI)
        .sqrt()
        .max(100.0);
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let positions: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let r = disk_radius * (((i as f64) + 0.5) / (n.max(1) as f64)).sqrt();
            let angle = (i as f64) * golden_angle;
            (r * angle.cos(), r * angle.sin())
        })
        .collect();

    positions_to_scene(
        nodes,
        links,
        boundary_links,
        starter_ids,
        &id_to_idx,
        &positions,
    )
}

/// Build a scene graph with nodes evenly spaced around a circle's
/// circumference — a chord-diagram / Circos-style layout (#367). Unlike
/// [`build_kb_graph_positions_only`]'s sunflower seed, this placement is
/// the FINAL layout: no force-directed refinement follows it (see
/// `Editor::populate_graph_buffer`'s branch on `kb_graph_layout_algorithm`),
/// so there's no "must stay small enough for a bounded cooling schedule to
/// converge from" constraint the sunflower's disk radius has to satisfy —
/// a ring whose circumference grows linearly with `n` (keeping adjacent-
/// node ARC spacing constant, mirroring `IDEAL_AREA_PER_NODE`'s role for
/// the sunflower disk) is the geometrically correct, not merely
/// acceptable, shape here.
pub fn build_kb_graph_chord_positions(
    nodes: &[KbNodeInfo],
    links: &[KbLinkInfo],
    boundary_links: &[KbLinkInfo],
    starter_ids: &[String],
    spacing_scale: f64,
) -> SceneGraph {
    let id_to_idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    let n = nodes.len();
    // Adjacent-node arc spacing = 2*pi*r / n; solving for r so that spacing
    // equals sqrt(IDEAL_AREA_PER_NODE * spacing_scale) (the same per-node
    // "personal space" constant the sunflower disk targets) gives a radius
    // that grows LINEARLY in n -- correct for a 1-D ring, unlike the
    // sunflower's deliberately sub-linear (sqrt) 2-D disk growth.
    let spacing = (crate::layout::IDEAL_AREA_PER_NODE * spacing_scale).sqrt();
    let radius = (n as f64 * spacing / (2.0 * std::f64::consts::PI)).max(100.0);
    let positions: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let angle = (i as f64) * 2.0 * std::f64::consts::PI / (n.max(1) as f64);
            (radius * angle.cos(), radius * angle.sin())
        })
        .collect();

    positions_to_scene(
        nodes,
        links,
        boundary_links,
        starter_ids,
        &id_to_idx,
        &positions,
    )
}

/// Shared node/edge assembly for every `build_kb_graph*` variant (CLAUDE.md
/// #8) — the only thing that differs between them is HOW `positions` was
/// computed (sunflower disk, circular ring, ...); node styling and the
/// internal/boundary edge-building logic (including the boundary-link
/// dedup-by-source stub collapsing) is identical regardless.
fn positions_to_scene(
    nodes: &[KbNodeInfo],
    links: &[KbLinkInfo],
    boundary_links: &[KbLinkInfo],
    starter_ids: &[String],
    id_to_idx: &std::collections::HashMap<&str, usize>,
    positions: &[(f64, f64)],
) -> SceneGraph {
    let n = nodes.len();
    let scene_nodes: Vec<SceneNode> = nodes
        .iter()
        .zip(positions.iter())
        .map(|(node, &(x, y))| {
            let kind = node.kind;
            let is_starter = starter_ids.contains(&node.id);
            let style = kind_to_style(&kind, is_starter);
            let width = (node.title.len() as f64 * 8.0 + 20.0).clamp(80.0, 200.0);
            SceneNode {
                id: node.id.clone(),
                label: node.title.clone(),
                x,
                y,
                width,
                height: 36.0,
                kind,
                style,
                pinned: false,
                is_seed: node.is_seed,
            }
        })
        .collect();

    // Create edges for internal links
    let mut scene_edges: Vec<SceneEdge> = links
        .iter()
        .filter_map(|link| {
            let s = *id_to_idx.get(link.source.as_str())?;
            let t = *id_to_idx.get(link.target.as_str())?;
            Some(SceneEdge {
                source: s,
                target: t,
                label: None,
                style: EdgeStyle::default(),
                weight: link.weight,
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
    let mut boundary_by_source: Vec<(&str, usize)> = Vec::new();
    for link in boundary_links {
        if let Some(entry) = boundary_by_source
            .iter_mut()
            .find(|(s, _)| *s == link.source.as_str())
        {
            entry.1 += 1;
        } else {
            boundary_by_source.push((link.source.as_str(), 1));
        }
    }
    for (src, count) in boundary_by_source {
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

/// Map node kind to visual style. Populates `SceneNode.style`, but that
/// field is NOT read by the GUI render path — `flatten_scene_graph`
/// (`crates/core/src/graph_view.rs`) colors nodes via
/// `GraphStyleOptions::color_for_kind`, theme-driven (`ui.graph.node.<kind>`
/// keys) with `NODE_KIND_FALLBACK_HEX` as its own fallback when a theme
/// doesn't define them — the routing this function's old TODO asked for
/// already happened, just not through this function. Kept for
/// `SceneNode.style`'s struct completeness (e.g. a non-KB `mae-canvas`
/// caller building a `SceneGraph` by hand) rather than deleted outright.
fn kind_to_style(kind: &NodeKind, highlighted: bool) -> NodeStyle {
    let (fill, border) = match kind {
        NodeKind::Index => ("#3a2a1a", "#ffaa4a"),
        NodeKind::Command => ("#3a1a3a", "#ff6aff"),
        NodeKind::Concept => ("#1a3a5c", "#4a9eff"),
        NodeKind::Key => ("#1a2a3a", "#4aaaff"),
        NodeKind::Note => ("#2a2d3e", "#6a6dff"),
        NodeKind::Project => ("#1a3a3a", "#4affff"),
        NodeKind::Category => ("#2a1a3a", "#aa4aff"),
        NodeKind::Lesson => ("#1a3a1a", "#6aff6a"),
        NodeKind::Tutorial => ("#1a3a1a", "#8fff8f"),
        NodeKind::Meta => ("#3a1a1a", "#ff6a6a"),
        NodeKind::Block => ("#2a2a1a", "#cccc4a"),
        NodeKind::SchemeApi => ("#3a3a1a", "#ffff6a"),
        NodeKind::Task => ("#1a2a1a", "#6aff9a"),
        NodeKind::View => ("#2a1a2a", "#ff4aaa"),
    };
    NodeStyle {
        fill: fill.to_string(),
        border: if highlighted {
            "#ff9933".to_string()
        } else {
            border.to_string()
        },
        border_width: if highlighted { 2.0 } else { 1.0 },
        highlighted,
    }
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
        let graph = build_kb_graph(&nodes, &links, &[], &[], 1.0);
        assert_eq!(graph.nodes.len(), 3);
    }

    #[test]
    fn build_graph_edge_count() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], &[], 1.0);
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn build_graph_with_boundary() {
        let (nodes, links) = nodes_and_links();
        let boundary = vec![link("concept:buffer", "external:xyz")];
        let graph = build_kb_graph(&nodes, &links, &boundary, &[], 1.0);
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
        let graph = build_kb_graph(&nodes, &links, &boundary, &[], 1.0);
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
        let graph = build_kb_graph(&nodes, &links, &boundary, &[], 1.0);
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
    fn build_graph_starter_highlighted() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], &["concept:buffer".to_string()], 1.0);
        assert!(graph.nodes[0].style.highlighted);
        assert!(!graph.nodes[1].style.highlighted);
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
        let graph = build_kb_graph(&nodes, &[], &[], &[], 1.0);
        assert_eq!(graph.nodes[0].kind, NodeKind::Task);
    }

    #[test]
    fn kind_to_style_covers_every_variant() {
        // Exhaustiveness is already enforced by the compiler (no default
        // arm in kind_to_style's match) — this just guards against a
        // variant silently sharing a placeholder color with the wrong
        // neighbor by construction accident (each call must at least run
        // without panicking for all 14 real NodeKind variants).
        let all = [
            NodeKind::Index,
            NodeKind::Command,
            NodeKind::Concept,
            NodeKind::Key,
            NodeKind::Note,
            NodeKind::Project,
            NodeKind::Category,
            NodeKind::Lesson,
            NodeKind::Tutorial,
            NodeKind::Meta,
            NodeKind::Block,
            NodeKind::SchemeApi,
            NodeKind::Task,
            NodeKind::View,
        ];
        for kind in all {
            let style = kind_to_style(&kind, false);
            assert!(style.fill.starts_with('#'), "{kind:?} fill: {style:?}");
            assert!(style.border.starts_with('#'), "{kind:?} border: {style:?}");
        }
    }

    #[test]
    fn positions_only_skips_force_layout() {
        // Nodes placed on the initial sunflower-spiral stay EXACTLY there
        // (no force-layout displacement) — confirms
        // `build_kb_graph_positions_only` really is layout-free, the
        // property `graph_view_ops.rs` depends on to defer layout to the
        // background bridge.
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph_positions_only(&nodes, &links, &[], &[], 1.0);
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
            let graph = build_kb_graph_positions_only(&nodes, &[], &[], &[], 1.0);
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
        let tight = build_kb_graph_positions_only(&nodes, &[], &[], &[], 1.0);
        let wide = build_kb_graph_positions_only(&nodes, &[], &[], &[], 4.0);
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
        // Same node/edge count and starter highlighting either way — only
        // the coordinates differ (before vs after layout).
        let (nodes, links) = nodes_and_links();
        let positions_only = build_kb_graph_positions_only(&nodes, &links, &[], &[], 1.0);
        let full = build_kb_graph(&nodes, &links, &[], &[], 1.0);
        assert_eq!(positions_only.nodes.len(), full.nodes.len());
        assert_eq!(positions_only.edges.len(), full.edges.len());
    }

    #[test]
    fn build_graph_empty() {
        let graph = build_kb_graph(&[], &[], &[], &[], 1.0);
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }

    #[test]
    fn node_width_scales_with_label() {
        let nodes = vec![
            KbNodeInfo {
                id: "a".to_string(),
                title: "Hi".to_string(),
                kind: NodeKind::Note,
                is_seed: false,
            },
            KbNodeInfo {
                id: "b".to_string(),
                title: "A Very Long Title For Testing Width".to_string(),
                kind: NodeKind::Note,
                is_seed: false,
            },
        ];
        let graph = build_kb_graph(&nodes, &[], &[], &[], 1.0);
        assert!(graph.nodes[1].width > graph.nodes[0].width);
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
        let graph = build_kb_graph(&nodes, &links, &[], &[], 1.0);
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
        let graph = build_kb_graph_chord_positions(&nodes, &links, &[], &[], 1.0);
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
        let graph = build_kb_graph_chord_positions(&nodes, &[], &[], &[], 1.0);
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
        let graph = build_kb_graph_chord_positions(&nodes, &[], &[], &[], 1.0);
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
    fn chord_positions_radius_grows_with_node_count() {
        // A ring's circumference must grow with n to keep adjacent-node
        // arc spacing from collapsing toward zero — unlike the sunflower
        // disk's deliberately sub-linear growth, linear growth is CORRECT
        // here (see the function's doc comment).
        fn radius_for(n: usize) -> f64 {
            let nodes: Vec<KbNodeInfo> = (0..n)
                .map(|i| KbNodeInfo {
                    id: format!("n{i}"),
                    title: "x".to_string(),
                    kind: NodeKind::Concept,
                    is_seed: false,
                })
                .collect();
            let graph = build_kb_graph_chord_positions(&nodes, &[], &[], &[], 1.0);
            (graph.nodes[0].x.powi(2) + graph.nodes[0].y.powi(2)).sqrt()
        }
        let small = radius_for(8);
        let large = radius_for(200);
        assert!(
            large > small * 5.0,
            "radius for n=200 ({large}) should grow substantially past n=8 ({small})"
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
        let chord = build_kb_graph_chord_positions(&nodes, &links, &boundary, &[], 1.0);
        let positions_only = build_kb_graph_positions_only(&nodes, &links, &boundary, &[], 1.0);
        assert_eq!(chord.edges.len(), positions_only.edges.len());
        assert_eq!(chord.edges.len(), 3); // 2 internal + 1 collapsed boundary stub
    }

    #[test]
    fn chord_positions_empty_graph() {
        let graph = build_kb_graph_chord_positions(&[], &[], &[], &[], 1.0);
        assert!(graph.nodes.is_empty());
        assert!(graph.edges.is_empty());
    }
}
