//! Fruchterman-Reingold force-directed layout with temperature cooling.
//!
//! Forces per iteration:
//! 1. Repulsion: all pairs pushed apart by k^2/d (Coulomb-like)
//! 2. Attraction: edges pull together by d/k (Hooke's spring)
//! 3. Centering: gentle pull toward origin
//! 4. Temperature-limited displacement

use crate::scene::{SceneEdge, SceneNode};

/// Configuration for the force layout algorithm.
#[derive(Debug, Clone)]
pub struct LayoutConfig {
    /// Repulsion force constant (higher = more spread out).
    pub repulsion: f64,
    /// Attraction force constant (higher = tighter clusters).
    pub attraction: f64,
    /// Damping factor per iteration (0.0-1.0).
    pub damping: f64,
    /// Maximum iterations for `run()`.
    pub max_iterations: usize,
    /// Centering force strength.
    pub centering: f64,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            repulsion: 1.0,
            attraction: 1.0,
            damping: 0.85,
            max_iterations: 100,
            centering: 0.01,
        }
    }
}

/// Force-directed layout engine using Fruchterman-Reingold algorithm.
pub struct ForceLayout {
    config: LayoutConfig,
}

impl ForceLayout {
    pub fn new(config: LayoutConfig) -> Self {
        Self { config }
    }

    /// Compute the ideal distance k = sqrt(area / num_nodes).
    fn ideal_distance(&self, node_count: usize, area: f64) -> f64 {
        if node_count == 0 {
            return 100.0;
        }
        (area / node_count as f64).sqrt()
    }

    /// Run a single iteration of the force layout.
    pub fn step(&self, nodes: &mut [SceneNode], edges: &[SceneEdge], temperature: f64) {
        let n = nodes.len();
        if n == 0 {
            return;
        }

        let area = 800.0 * 600.0; // Default layout area
        let k = self.ideal_distance(n, area);
        let k_sq = k * k;

        // Accumulate displacements
        let mut dx = vec![0.0f64; n];
        let mut dy = vec![0.0f64; n];

        // 1. Repulsion: all pairs
        for i in 0..n {
            for j in (i + 1)..n {
                let dist_x = nodes[i].x - nodes[j].x;
                let dist_y = nodes[i].y - nodes[j].y;
                let dist = (dist_x * dist_x + dist_y * dist_y).sqrt().max(0.01);
                let force = self.config.repulsion * (k_sq / dist);
                let fx = (dist_x / dist) * force;
                let fy = (dist_y / dist) * force;
                dx[i] += fx;
                dy[i] += fy;
                dx[j] -= fx;
                dy[j] -= fy;
            }
        }

        // 2. Attraction: along edges
        for edge in edges {
            let s = edge.source;
            let t = edge.target;
            if s >= n || t >= n {
                continue;
            }
            let dist_x = nodes[s].x - nodes[t].x;
            let dist_y = nodes[s].y - nodes[t].y;
            let dist = (dist_x * dist_x + dist_y * dist_y).sqrt().max(0.01);
            let force = self.config.attraction * (dist * dist) / k;
            let fx = (dist_x / dist) * force;
            let fy = (dist_y / dist) * force;
            dx[s] -= fx;
            dy[s] -= fy;
            dx[t] += fx;
            dy[t] += fy;
        }

        // 3. Centering force
        for i in 0..n {
            dx[i] -= nodes[i].x * self.config.centering;
            dy[i] -= nodes[i].y * self.config.centering;
        }

        // 4. Apply with temperature limit
        for i in 0..n {
            if nodes[i].pinned {
                continue;
            }
            let disp = (dx[i] * dx[i] + dy[i] * dy[i]).sqrt().max(0.01);
            let scale = temperature.min(disp) / disp;
            nodes[i].x += dx[i] * scale * self.config.damping;
            nodes[i].y += dy[i] * scale * self.config.damping;
        }
    }

    /// Run the full layout algorithm with temperature cooling.
    pub fn run(&self, nodes: &mut [SceneNode], edges: &[SceneEdge], iterations: usize) {
        let iters = if iterations == 0 {
            self.config.max_iterations
        } else {
            iterations
        };

        for i in 0..iters {
            // Linear cooling: temperature goes from 100 to ~1
            let temperature = 100.0 * (1.0 - (i as f64 / iters as f64));
            self.step(nodes, edges, temperature.max(1.0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{EdgeStyle, NodeKind, NodeStyle};

    fn make_node(id: &str, x: f64, y: f64) -> SceneNode {
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
        }
    }

    fn make_edge(source: usize, target: usize) -> SceneEdge {
        SceneEdge {
            source,
            target,
            label: None,
            style: EdgeStyle::default(),
        }
    }

    #[test]
    fn empty_graph_no_crash() {
        let layout = ForceLayout::new(LayoutConfig::default());
        let mut nodes: Vec<SceneNode> = vec![];
        layout.run(&mut nodes, &[], 10);
    }

    #[test]
    fn single_node_centers() {
        let layout = ForceLayout::new(LayoutConfig::default());
        let mut nodes = vec![make_node("a", 100.0, 100.0)];
        layout.run(&mut nodes, &[], 50);
        // Centering force should pull toward origin
        assert!(
            nodes[0].x.abs() < 100.0,
            "x={} should move toward 0",
            nodes[0].x
        );
        assert!(
            nodes[0].y.abs() < 100.0,
            "y={} should move toward 0",
            nodes[0].y
        );
    }

    #[test]
    fn two_connected_nodes_converge() {
        let layout = ForceLayout::new(LayoutConfig::default());
        let mut nodes = vec![make_node("a", -200.0, 0.0), make_node("b", 200.0, 0.0)];
        let edges = vec![make_edge(0, 1)];
        layout.run(&mut nodes, &edges, 100);
        let dist = ((nodes[0].x - nodes[1].x).powi(2) + (nodes[0].y - nodes[1].y).powi(2)).sqrt();
        // Connected nodes should settle at a finite equilibrium distance
        // (initial distance was 400; with repulsion they settle near ideal k ~ 490)
        assert!(
            dist < 600.0,
            "connected nodes should settle at bounded distance, dist={}",
            dist
        );
        assert!(dist > 1.0, "nodes shouldn't overlap, dist={}", dist);
    }

    #[test]
    fn disconnected_nodes_repel() {
        let layout = ForceLayout::new(LayoutConfig::default());
        let mut nodes = vec![make_node("a", 0.0, 0.0), make_node("b", 1.0, 0.0)];
        layout.run(&mut nodes, &[], 100);
        let dist = ((nodes[0].x - nodes[1].x).powi(2) + (nodes[0].y - nodes[1].y).powi(2)).sqrt();
        // Disconnected nodes should repel to some distance
        assert!(
            dist > 10.0,
            "disconnected nodes should spread apart, dist={}",
            dist
        );
    }

    #[test]
    fn pinned_node_stays() {
        let layout = ForceLayout::new(LayoutConfig::default());
        let mut nodes = vec![make_node("a", 50.0, 50.0), make_node("b", -50.0, -50.0)];
        nodes[0].pinned = true;
        let edges = vec![make_edge(0, 1)];
        layout.run(&mut nodes, &edges, 50);
        assert_eq!(nodes[0].x, 50.0, "pinned node x should not move");
        assert_eq!(nodes[0].y, 50.0, "pinned node y should not move");
    }

    #[test]
    fn layout_deterministic() {
        let layout = ForceLayout::new(LayoutConfig::default());
        let mut nodes1 = vec![
            make_node("a", 0.0, 0.0),
            make_node("b", 100.0, 0.0),
            make_node("c", 0.0, 100.0),
        ];
        let mut nodes2 = nodes1.clone();
        let edges = vec![make_edge(0, 1), make_edge(1, 2)];
        layout.run(&mut nodes1, &edges, 50);
        layout.run(&mut nodes2, &edges, 50);
        for (n1, n2) in nodes1.iter().zip(nodes2.iter()) {
            assert!((n1.x - n2.x).abs() < 1e-10, "x should be deterministic");
            assert!((n1.y - n2.y).abs() < 1e-10, "y should be deterministic");
        }
    }

    #[test]
    fn no_overlapping_after_layout() {
        let layout = ForceLayout::new(LayoutConfig::default());
        // Place all nodes at origin — they should separate
        let mut nodes: Vec<SceneNode> = (0..5)
            .map(|i| make_node(&format!("n{}", i), 0.0, 0.0))
            .collect();
        // Add tiny perturbation so force directions are well-defined
        for (i, node) in nodes.iter_mut().enumerate() {
            node.x = (i as f64) * 0.1;
            node.y = (i as f64) * 0.1;
        }
        let edges: Vec<SceneEdge> = (0..4).map(|i| make_edge(i, i + 1)).collect();
        layout.run(&mut nodes, &edges, 100);

        // Check no pair of nodes overlap (centers within 20px)
        for i in 0..nodes.len() {
            for j in (i + 1)..nodes.len() {
                let dist =
                    ((nodes[i].x - nodes[j].x).powi(2) + (nodes[i].y - nodes[j].y).powi(2)).sqrt();
                assert!(
                    dist > 10.0,
                    "nodes {} and {} overlap at dist={}",
                    i,
                    j,
                    dist
                );
            }
        }
    }
}
