//! Bridge between KB data and SceneGraph.
//!
//! Converts KB nodes and links into a positioned scene graph,
//! mapping node namespaces to visual kinds and styles.

use crate::layout::{ForceLayout, LayoutConfig};
use crate::scene::{EdgeStyle, NodeKind, NodeStyle, SceneEdge, SceneGraph, SceneNode, Viewport};

/// A simplified KB node for graph building (no dependency on mae-kb).
#[derive(Debug, Clone)]
pub struct KbNodeInfo {
    pub id: String,
    pub title: String,
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
    // Build index: id -> node position
    let id_to_idx: std::collections::HashMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id.as_str(), i))
        .collect();

    // Create scene nodes with initial positions on a circle
    let n = nodes.len();
    let radius = (n as f64 * 30.0).max(100.0);
    let scene_nodes: Vec<SceneNode> = nodes
        .iter()
        .enumerate()
        .map(|(i, node)| {
            let angle = 2.0 * std::f64::consts::PI * (i as f64) / (n.max(1) as f64);
            let x = radius * angle.cos();
            let y = radius * angle.sin();
            let kind = namespace_to_kind(&node.id);
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

    // Add boundary links as dashed red edges
    for (src, _tgt) in boundary_links {
        if let Some(&s) = id_to_idx.get(src.as_str()) {
            // Boundary target is outside the graph — just show the outgoing edge
            // pointing to the edge of the source node (no target node rendered)
            scene_edges.push(SceneEdge {
                source: s,
                target: s, // self-loop as visual indicator
                label: Some("...".to_string()),
                style: EdgeStyle {
                    color: "#ff6666".to_string(),
                    width: 1.0,
                    dashed: true,
                },
            });
        }
    }

    let mut graph = SceneGraph {
        nodes: scene_nodes,
        edges: scene_edges,
        viewport: Viewport::default(),
        selection: if n > 0 { Some(0) } else { None },
    };

    // Run force layout
    let layout = ForceLayout::new(LayoutConfig::default());
    layout.run(&mut graph.nodes, &graph.edges, 50);

    graph
}

/// Map KB node ID namespace to visual kind.
fn namespace_to_kind(id: &str) -> NodeKind {
    if id.starts_with("cmd:") {
        NodeKind::Command
    } else if id.starts_with("concept:") {
        NodeKind::Concept
    } else if id.starts_with("lesson:") {
        NodeKind::Lesson
    } else if id.starts_with("scheme:") {
        NodeKind::Scheme
    } else if id.starts_with("option:") {
        NodeKind::Option
    } else {
        NodeKind::Note
    }
}

/// Map node kind to visual style.
fn kind_to_style(kind: &NodeKind, highlighted: bool) -> NodeStyle {
    let (fill, border) = match kind {
        NodeKind::Concept => ("#1a3a5c", "#4a9eff"),
        NodeKind::Command => ("#3a1a3a", "#ff6aff"),
        NodeKind::Lesson => ("#1a3a1a", "#6aff6a"),
        NodeKind::Scheme => ("#3a3a1a", "#ffff6a"),
        NodeKind::Option => ("#2a2a2a", "#aaaaaa"),
        NodeKind::Note => ("#2a2d3e", "#6a6dff"),
        NodeKind::Custom(_) => ("#2a2d3e", "#4a4d5e"),
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
            },
            KbNodeInfo {
                id: "concept:window".to_string(),
                title: "Window".to_string(),
            },
            KbNodeInfo {
                id: "cmd:save".to_string(),
                title: "Save".to_string(),
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
    fn build_graph_starter_highlighted() {
        let (nodes, links) = nodes_and_links();
        let graph = build_kb_graph(&nodes, &links, &[], &["concept:buffer".to_string()]);
        assert!(graph.nodes[0].style.highlighted);
        assert!(!graph.nodes[1].style.highlighted);
    }

    #[test]
    fn namespace_mapping() {
        assert_eq!(namespace_to_kind("cmd:save"), NodeKind::Command);
        assert_eq!(namespace_to_kind("concept:buffer"), NodeKind::Concept);
        assert_eq!(namespace_to_kind("lesson:intro"), NodeKind::Lesson);
        assert_eq!(namespace_to_kind("scheme:define"), NodeKind::Scheme);
        assert_eq!(namespace_to_kind("option:theme"), NodeKind::Option);
        assert_eq!(namespace_to_kind("my-note"), NodeKind::Note);
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
            },
            KbNodeInfo {
                id: "b".to_string(),
                title: "A Very Long Title For Testing Width".to_string(),
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
            },
            KbNodeInfo {
                id: "b".to_string(),
                title: "B".to_string(),
            },
            KbNodeInfo {
                id: "c".to_string(),
                title: "C".to_string(),
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
