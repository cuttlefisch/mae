//! Graph query helpers: shortest path, neighborhood subgraph, and
//! graph+tag relatedness scoring — built on the link primitives in
//! `links.rs` (bibliographic coupling / co-citation / direct adjacency /
//! shared tags, mirroring `crate::KnowledgeBase::related`).

use super::util::cozo_err;
use super::*;

impl CozoKbStore {
    /// Find shortest path between two nodes — actually a **reachability
    /// check**, not real shortest-path reconstruction (see doc below).
    ///
    /// Previously implemented as recursive Datalog with `d + 1` inside a
    /// rule head (`reach[node, d + 1] := ...`); CozoDB's parser rejects
    /// arithmetic expressions in rule-head position, so every call to this
    /// function unconditionally errored ("query parser has encountered
    /// unexpected input") — `neighborhood` right below already carries a
    /// comment flagging this exact issue and avoids it via iterative
    /// expansion in Rust instead of recursive Datalog; this function just
    /// hadn't been brought in line with that fix yet (CLAUDE.md principle
    /// #15 — fixing the drift, not just documenting around it). Same
    /// iterative-BFS shape as `neighborhood`, capped at 10 hops, walking
    /// links in both directions (undirected reachability, matching the
    /// original two-rule query).
    pub fn shortest_path(&self, from: &str, to: &str) -> Result<Vec<String>, KbStoreError> {
        const MAX_DEPTH: usize = 10;

        if from == to {
            return Ok(vec![from.to_string(), to.to_string()]);
        }

        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        visited.insert(from.to_string());
        let mut frontier = vec![from.to_string()];

        for _ in 0..MAX_DEPTH {
            let mut next_frontier = Vec::new();
            for node_id in &frontier {
                let mut neighbors: Vec<String> = self
                    .links_from(node_id)?
                    .into_iter()
                    .map(|l| l.dst)
                    .collect();
                neighbors.extend(self.links_to(node_id)?.into_iter().map(|l| l.src));
                for neighbor in neighbors {
                    if neighbor == to {
                        // Path exists — return from/to (full intermediate-hop
                        // reconstruction is complex in Datalog; not attempted).
                        return Ok(vec![from.to_string(), to.to_string()]);
                    }
                    if visited.insert(neighbor.clone()) {
                        next_frontier.push(neighbor);
                    }
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
        }

        Ok(vec![])
    }
    /// Get neighborhood subgraph around a node up to a given depth.
    pub fn neighborhood(&self, id: &str, depth: u32) -> Result<SubGraph, KbStoreError> {
        // Use simple multi-hop expansion without recursion depth tracking
        // to avoid CozoDB parser issues with `d + 1` syntax.
        // Collect all reachable nodes within `depth` hops.
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut frontier = vec![id.to_string()];
        visited.insert(id.to_string());

        for _ in 0..depth {
            let mut next_frontier = Vec::new();
            for node_id in &frontier {
                let out = self.links_from(node_id)?;
                for link in &out {
                    if visited.insert(link.dst.clone()) {
                        next_frontier.push(link.dst.clone());
                    }
                }
                let inc = self.links_to(node_id)?;
                for link in &inc {
                    if visited.insert(link.src.clone()) {
                        next_frontier.push(link.src.clone());
                    }
                }
            }
            frontier = next_frontier;
        }

        // Collect node info
        let mut nodes = Vec::new();
        for nid in &visited {
            if let Some(node) = self.get_node(nid)? {
                nodes.push((node.id, node.title));
            }
        }

        // Collect edges between visited nodes
        let mut edges = Vec::new();
        for nid in &visited {
            for link in self.links_from(nid)? {
                if visited.contains(&link.dst) {
                    edges.push((link.src, link.dst, link.rel_type));
                }
            }
        }

        Ok(SubGraph { nodes, edges })
    }
    /// Graph-relatedness over the typed link graph + tags — the Cozo-backed
    /// twin of [`crate::KnowledgeBase::related`]. Same four signals (direct
    /// link, bibliographic coupling, co-citation, shared tags), same weights,
    /// so results match the in-memory path. Built from the Datalog-backed
    /// `links_from`/`links_to` primitives (mirrors `neighborhood`'s approach,
    /// which avoids fragile recursive/self-join Datalog).
    pub fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        let Some(node) = self.get_node(id)? else {
            return Ok(Vec::new());
        };
        const W_DIRECT: f64 = 2.0;
        const W_COUPLING: f64 = 1.0;
        const W_COCITATION: f64 = 1.0;
        const W_TAG: f64 = 0.5;

        let out: Vec<String> = self.links_from(id)?.into_iter().map(|l| l.dst).collect();
        let inn: Vec<String> = self.links_to(id)?.into_iter().map(|l| l.src).collect();
        let tags: std::collections::HashSet<String> = node.tags.into_iter().collect();

        let mut score: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

        // Bibliographic coupling: other nodes that link to the same targets.
        for target in &out {
            for l in self.links_to(target)? {
                if l.src != id {
                    *score.entry(l.src).or_default() += W_COUPLING;
                }
            }
        }
        // Co-citation: other nodes cited by the same sources.
        for src in &inn {
            for l in self.links_from(src)? {
                if l.dst != id {
                    *score.entry(l.dst).or_default() += W_COCITATION;
                }
            }
        }
        // Direct adjacency (either direction) is the strongest signal.
        for c in out.iter().chain(inn.iter()) {
            if c != id {
                *score.entry(c.clone()).or_default() += W_DIRECT;
            }
        }
        // Shared tags — one bulk query over `tags_json`, parsed in Rust (tags
        // aren't a relation, so this can't be a pure Datalog join).
        if !tags.is_empty() {
            let rows = self
                .run_immut("?[id, tags_json] := *nodes{id, tags_json}")
                .map_err(cozo_err)?;
            for row in &rows.rows {
                let Some(cid) = row.first().and_then(|v| v.get_str()) else {
                    continue;
                };
                if cid == id {
                    continue;
                }
                let ctags: Vec<String> = row
                    .get(1)
                    .and_then(|v| v.get_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let shared = ctags.iter().filter(|t| tags.contains(*t)).count();
                if shared > 0 {
                    *score.entry(cid.to_string()).or_default() += W_TAG * shared as f64;
                }
            }
        }

        let mut scored: Vec<(String, f64)> = score.into_iter().collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        Ok(scored)
    }
}
