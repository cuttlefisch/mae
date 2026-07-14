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
}

/// Build a scene graph from KB nodes and links.
///
/// - `nodes`: KB nodes with id and title
/// - `links`: (source_id, target_id) pairs within the subgraph
/// - `boundary_links`: (source_id, target_id) pairs crossing the subgraph boundary
/// - `starter_ids`: IDs of the starting nodes (highlighted)
pub fn build_kb_graph(
    nodes: &[KbNodeInfo],
    links: &[(String, String)],
    boundary_links: &[(String, String)],
    starter_ids: &[String],
) -> SceneGraph {
    let mut graph = build_kb_graph_positions_only(nodes, links, boundary_links, starter_ids);

    // Run force layout
    let layout = ForceLayout::new(LayoutConfig::default());
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
    links: &[(String, String)],
    boundary_links: &[(String, String)],
    starter_ids: &[String],
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
    let n = nodes.len();
    let disk_radius = ((n as f64 * crate::layout::IDEAL_AREA_PER_NODE) / std::f64::consts::PI)
        .sqrt()
        .max(100.0);
    let golden_angle = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
    let scene_nodes: Vec<SceneNode> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let r = disk_radius * (((i as f64) + 0.5) / (n.max(1) as f64)).sqrt();
            let angle = (i as f64) * golden_angle;
            let x = r * angle.cos();
            let y = r * angle.sin();
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
            }
        })
        .collect();

    // Create edges for internal links
    let mut scene_edges: Vec<SceneEdge> = links
        .iter()
        .filter_map(|(src, tgt)| {
            let s = *id_to_idx.get(src.as_str())?;
            let t = *id_to_idx.get(tgt.as_str())?;
            Some(SceneEdge {
                source: s,
                target: t,
                label: None,
                style: EdgeStyle::default(),
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
    for (src, _tgt) in boundary_links {
        if let Some(entry) = boundary_by_source
            .iter_mut()
            .find(|(s, _)| *s == src.as_str())
        {
            entry.1 += 1;
        } else {
            boundary_by_source.push((src.as_str(), 1));
        }
    }
    for (src, count) in boundary_by_source {
        if let Some(&s) = id_to_idx.get(src) {
            // Boundary target is outside the graph — just show the outgoing edge
            // pointing to the edge of the source node (no target node rendered)
            let label = if count > 1 {
                format!("... (+{count})")
            } else {
                "...".to_string()
            };
            scene_edges.push(SceneEdge {
                source: s,
                target: s, // self-loop as visual indicator
                label: Some(label),
                style: EdgeStyle {
                    color: "#ff6666".to_string(),
                    width: 1.0,
                    dashed: true,
                },
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

/// Map node kind to visual style.
// TODO(graph-view Phase 1): route through MAE's Theme/ThemeResolver system
// (`ui.graph.node.<kind>` keys) instead of this hardcoded palette — tracked
// in the native KB graph view plan's Phase 1 (theme-driven styling section).
// This hardcoded table is a Phase-0-appropriate placeholder that simply
// extends coverage to the real 14-variant `NodeKind`; it is NOT the final
// styling mechanism.
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

    fn nodes_and_links() -> (Vec<KbNodeInfo>, Vec<(String, String)>) {
        let nodes = vec![
            KbNodeInfo {
                id: "concept:buffer".to_string(),
                title: "Buffer".to_string(),
                kind: NodeKind::Concept,
            },
            KbNodeInfo {
                id: "concept:window".to_string(),
                title: "Window".to_string(),
                kind: NodeKind::Concept,
            },
            KbNodeInfo {
                id: "cmd:save".to_string(),
                title: "Save".to_string(),
                kind: NodeKind::Command,
            },
        ];
        let links = vec![
            ("concept:buffer".to_string(), "concept:window".to_string()),
            ("cmd:save".to_string(), "concept:buffer".to_string()),
        ];
        (nodes, links)
    }

    #[test]
    fn build_graph_node_count() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], &[]);
        assert_eq!(graph.nodes.len(), 3);
    }

    #[test]
    fn build_graph_edge_count() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], &[]);
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn build_graph_with_boundary() {
        let (nodes, links) = nodes_and_links();
        let boundary = vec![("concept:buffer".to_string(), "external:xyz".to_string())];
        let graph = build_kb_graph(&nodes, &links, &boundary, &[]);
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
            ("concept:buffer".to_string(), "external:a".to_string()),
            ("concept:buffer".to_string(), "external:b".to_string()),
            ("concept:buffer".to_string(), "external:c".to_string()),
        ];
        let graph = build_kb_graph(&nodes, &links, &boundary, &[]);
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
            ("concept:buffer".to_string(), "external:a".to_string()),
            ("concept:window".to_string(), "external:b".to_string()),
        ];
        let graph = build_kb_graph(&nodes, &links, &boundary, &[]);
        // 2 internal + 2 boundary edges (one per distinct source, each
        // with count 1 so the label stays the plain "...").
        assert_eq!(graph.edges.len(), 4);
        let boundary_edges: Vec<_> = graph.edges[2..].to_vec();
        assert_eq!(boundary_edges.len(), 2);
        assert!(boundary_edges
            .iter()
            .all(|e| e.label.as_deref() == Some("...")));
        let sources: std::collections::HashSet<_> =
            boundary_edges.iter().map(|e| e.source).collect();
        assert_eq!(sources.len(), 2, "each source keeps its own boundary edge");
    }

    #[test]
    fn build_graph_starter_highlighted() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], &["concept:buffer".to_string()]);
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
        }];
        let graph = build_kb_graph(&nodes, &[], &[], &[]);
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
        let graph = build_kb_graph_positions_only(&nodes, &links, &[], &[]);
        let n = nodes.len();
        let disk_radius = ((n as f64 * crate::layout::IDEAL_AREA_PER_NODE) / std::f64::consts::PI)
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
                })
                .collect();
            let graph = build_kb_graph_positions_only(&nodes, &[], &[], &[]);
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
    fn positions_only_and_full_agree_on_topology() {
        // Same node/edge count and starter highlighting either way — only
        // the coordinates differ (before vs after layout).
        let (nodes, links) = nodes_and_links();
        let positions_only = build_kb_graph_positions_only(&nodes, &links, &[], &[]);
        let full = build_kb_graph(&nodes, &links, &[], &[]);
        assert_eq!(positions_only.nodes.len(), full.nodes.len());
        assert_eq!(positions_only.edges.len(), full.edges.len());
    }

    #[test]
    fn build_graph_empty() {
        let graph = build_kb_graph(&[], &[], &[], &[]);
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
            },
            KbNodeInfo {
                id: "b".to_string(),
                title: "A Very Long Title For Testing Width".to_string(),
                kind: NodeKind::Note,
            },
        ];
        let graph = build_kb_graph(&nodes, &[], &[], &[]);
        assert!(graph.nodes[1].width > graph.nodes[0].width);
    }

    #[test]
    fn force_layout_separates_nodes() {
        let nodes = vec![
            KbNodeInfo {
                id: "a".to_string(),
                title: "A".to_string(),
                kind: NodeKind::Note,
            },
            KbNodeInfo {
                id: "b".to_string(),
                title: "B".to_string(),
                kind: NodeKind::Note,
            },
            KbNodeInfo {
                id: "c".to_string(),
                title: "C".to_string(),
                kind: NodeKind::Note,
            },
        ];
        let links = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
        ];
        let graph = build_kb_graph(&nodes, &links, &[], &[]);
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
}
