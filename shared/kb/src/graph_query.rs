//! Shared graph-neighborhood / relatedness cores behind `kb_graph` and
//! `kb_related` — the MCP tool executors
//! (`crates/ai/src/tool_impls/kb.rs::execute_kb_graph`/`execute_kb_related`)
//! and the `(kb-graph)`/`(kb-related)` Scheme primitives
//! (`crates/scheme/src/runtime/kb_queries.rs`) both call the functions here
//! instead of each re-implementing the walk/ranking themselves — CLAUDE.md
//! principle #8 (shared computation once), applied to the AI/human-parity gap
//! tracked as Phase 0 of the native KB graph view plan.
//!
//! Each surface has a different amount of KB access available to it (the MCP
//! executor sees the full `Editor.kb` — federated query layer, in-memory
//! `KnowledgeBase` federation, or both; the Scheme runtime only carries a
//! single `Arc<dyn KbStore>`, mirroring the existing `kb-links-from` /
//! `kb-links-to` convention). Rather than duplicate the walk/ranking algorithm
//! per backend, each algorithm here is generic over a small backend trait —
//! [`GraphNeighbors`] for [`bfs_neighborhood`], [`RelatedSource`] for
//! [`related_enriched`] — with one implementation per backend shape. The
//! backend supplies raw connectivity/scoring; the shared function supplies
//! the walk/enrichment logic that must never drift between the two call
//! surfaces.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::federation::KbRegistry;
use crate::query::KbQueryLayer;
use crate::store::KbStore;
use crate::KnowledgeBase;

// ---------------------------------------------------------------------------
// kb_graph — BFS neighborhood
// ---------------------------------------------------------------------------

/// One node discovered during a [`bfs_neighborhood`] walk.
#[derive(Debug, Clone)]
pub struct GraphBfsNode {
    pub id: String,
    pub hop: usize,
    /// `None` when `id` was referenced by a link but doesn't actually exist
    /// (a dangling/"missing" neighbor) — `title`/`kind` are `None` too.
    pub missing: bool,
    pub title: Option<String>,
    pub kind: Option<String>,
    /// Federated instance name owning this node, when the backend tracks
    /// federation and the node isn't in the "home" KB. Always `None` for
    /// single-store backends (they have no federation concept).
    pub instance: Option<String>,
}

/// Result of a [`bfs_neighborhood`] walk.
#[derive(Debug, Clone)]
pub struct GraphBfsResult {
    pub root: String,
    pub depth: usize,
    pub nodes: Vec<GraphBfsNode>,
    pub edges: Vec<(String, String)>,
}

/// Backend abstraction for [`bfs_neighborhood`]: everything the walk needs to
/// know about "does this id exist" / "what's it connected to" / "how should
/// it be displayed." This is the seam that lets the MCP `kb_graph` executor
/// and the `(kb-graph)` Scheme primitive share one walk without sharing an
/// `Editor` — see module docs.
pub trait GraphNeighbors {
    fn contains(&self, id: &str) -> bool;
    /// Union of outgoing + incoming neighbor ids (the walk itself is
    /// undirected — it discovers nodes reachable in either direction).
    fn neighbor_ids(&self, id: &str) -> Vec<String>;
    /// Outgoing target ids only (edges in the result are directional).
    fn outgoing_ids(&self, id: &str) -> Vec<String>;
    /// `(title, kind_str, owning_instance_name)` for display, if `id` exists.
    fn describe(&self, id: &str) -> Option<(String, String, Option<String>)>;
}

/// BFS neighborhood walk shared by every `kb_graph`-shaped surface. Hop
/// tracking via a visited-map BFS (first-seen hop wins), edges restricted to
/// pairs both inside the walked set — mirrors the algorithm that used to be
/// duplicated inline in `execute_kb_graph`'s two branches.
pub fn bfs_neighborhood(
    backend: &dyn GraphNeighbors,
    id: &str,
    depth: usize,
) -> Result<GraphBfsResult, String> {
    if !backend.contains(id) {
        return Err(format!("No KB node: {}", id));
    }

    let mut hops: HashMap<String, usize> = HashMap::from([(id.to_string(), 0)]);
    let mut queue: VecDeque<(String, usize)> = VecDeque::from([(id.to_string(), 0)]);
    while let Some((cur, h)) = queue.pop_front() {
        if h >= depth {
            continue;
        }
        for n in backend.neighbor_ids(&cur) {
            if !hops.contains_key(&n) {
                hops.insert(n.clone(), h + 1);
                queue.push_back((n, h + 1));
            }
        }
    }

    let mut ids: Vec<String> = hops.keys().cloned().collect();
    ids.sort_by(|a, b| hops[a].cmp(&hops[b]).then_with(|| a.cmp(b)));

    let nodes: Vec<GraphBfsNode> = ids
        .iter()
        .map(|nid| {
            let hop = hops[nid];
            match backend.describe(nid) {
                Some((title, kind, instance)) => GraphBfsNode {
                    id: nid.clone(),
                    hop,
                    missing: false,
                    title: Some(title),
                    kind: Some(kind),
                    instance,
                },
                None => GraphBfsNode {
                    id: nid.clone(),
                    hop,
                    missing: true,
                    title: None,
                    kind: None,
                    instance: None,
                },
            }
        })
        .collect();

    let in_set: HashSet<&String> = hops.keys().collect();
    let mut edges: Vec<(String, String)> = Vec::new();
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for src in &ids {
        for dst in backend.outgoing_ids(src) {
            if in_set.contains(&dst) && seen.insert((src.clone(), dst.clone())) {
                edges.push((src.clone(), dst));
            }
        }
    }

    Ok(GraphBfsResult {
        root: id.to_string(),
        depth,
        nodes,
        edges,
    })
}

/// [`GraphNeighbors`] backed by the federated CozoDB-backed query layer
/// (`Editor.kb.query_layer()`) — the MCP executor's preferred/common path.
/// Never attributes an `instance` (matches `execute_kb_graph`'s original
/// query-layer branch, which didn't either — the query layer already routes
/// to the owning instance internally).
pub struct QueryLayerBackend<'a>(pub &'a dyn KbQueryLayer);

impl GraphNeighbors for QueryLayerBackend<'_> {
    fn contains(&self, id: &str) -> bool {
        self.0.contains(id)
    }

    fn neighbor_ids(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = self.0.links_from(id).into_iter().map(|l| l.dst).collect();
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for l in self.0.links_to(id) {
            if seen.insert(l.src.clone()) {
                out.push(l.src);
            }
        }
        out
    }

    fn outgoing_ids(&self, id: &str) -> Vec<String> {
        self.0.links_from(id).into_iter().map(|l| l.dst).collect()
    }

    fn describe(&self, id: &str) -> Option<(String, String, Option<String>)> {
        self.0
            .get(id)
            .map(|n| (n.title, n.kind.as_str().to_string(), None))
    }
}

/// [`GraphNeighbors`] backed by direct in-memory `KnowledgeBase` federation
/// (`Editor.kb.primary` + `Editor.kb.instances`) — the MCP executor's
/// fallback path, used before a CozoDB query layer is available.
pub struct InMemoryFederatedBackend<'a> {
    pub primary: &'a KnowledgeBase,
    pub instances: &'a HashMap<String, KnowledgeBase>,
    pub registry: &'a KbRegistry,
}

impl GraphNeighbors for InMemoryFederatedBackend<'_> {
    fn contains(&self, id: &str) -> bool {
        self.primary.contains(id) || self.instances.values().any(|kb| kb.contains(id))
    }

    fn neighbor_ids(&self, id: &str) -> Vec<String> {
        let mut out = self.primary.neighbors(id);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for kb in self.instances.values() {
            for n in kb.neighbors(id) {
                if seen.insert(n.clone()) {
                    out.push(n);
                }
            }
        }
        out
    }

    fn outgoing_ids(&self, id: &str) -> Vec<String> {
        let mut out = self.primary.links_from(id);
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for kb in self.instances.values() {
            for l in kb.links_from(id) {
                if seen.insert(l.clone()) {
                    out.push(l);
                }
            }
        }
        out
    }

    fn describe(&self, id: &str) -> Option<(String, String, Option<String>)> {
        if let Some(n) = self.primary.get(id) {
            return Some((n.title.clone(), n.kind.as_str().to_string(), None));
        }
        for (uuid, kb) in self.instances {
            if let Some(n) = kb.get(id) {
                let inst_name = self.registry.find_by_uuid(uuid).map(|i| i.name.clone());
                return Some((n.title.clone(), n.kind.as_str().to_string(), inst_name));
            }
        }
        None
    }
}

/// [`GraphNeighbors`] backed by a single [`KbStore`] — what the Scheme
/// runtime actually has available (`SharedState::kb_store`, synced 1:1 from
/// `Editor.kb.store`, the primary KB's durable store only; see
/// `kb_state.rs`'s doc comment on `KbContext::store`). No federation: this
/// mirrors the scope every other `kb-*` Scheme primitive already has
/// (`kb-links-from`, `kb-links-to`, …), so `(kb-graph)` walks the primary KB
/// only, not federated instances — documented on the primitive itself.
pub struct KbStoreBackend<'a>(pub &'a dyn KbStore);

impl GraphNeighbors for KbStoreBackend<'_> {
    fn contains(&self, id: &str) -> bool {
        matches!(self.0.get_node(id), Ok(Some(_)))
    }

    fn neighbor_ids(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = self
            .0
            .links_from(id)
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.dst)
            .collect();
        let mut seen: HashSet<String> = out.iter().cloned().collect();
        for l in self.0.links_to(id).unwrap_or_default() {
            if seen.insert(l.src.clone()) {
                out.push(l.src);
            }
        }
        out
    }

    fn outgoing_ids(&self, id: &str) -> Vec<String> {
        self.0
            .links_from(id)
            .unwrap_or_default()
            .into_iter()
            .map(|l| l.dst)
            .collect()
    }

    fn describe(&self, id: &str) -> Option<(String, String, Option<String>)> {
        self.0
            .get_node(id)
            .ok()
            .flatten()
            .map(|n| (n.title, n.kind.as_str().to_string(), None))
    }
}

// ---------------------------------------------------------------------------
// kb_related — structural relatedness
// ---------------------------------------------------------------------------

/// One ranked result of [`related_enriched`].
#[derive(Debug, Clone)]
pub struct RelatedItem {
    pub id: String,
    pub title: String,
    pub kind: String,
    pub score: f64,
}

/// Backend abstraction for [`related_enriched`]. Kept separate from
/// [`GraphNeighbors`] because "related" is a backend-specific ranking
/// algorithm (co-citation/bibliographic-coupling/shared-tags for the
/// in-memory `KnowledgeBase`, a CozoDB Datalog query for the store) rather
/// than a generic walk — the shared part is "rank then enrich with
/// title/kind," not the ranking heuristic itself.
pub trait RelatedSource {
    /// Scored related-node ids for `id`, already ranked/limited by the
    /// backend's own algorithm.
    fn scored(&self, id: &str, limit: usize) -> Vec<(String, f64)>;
    /// `(title, kind_str)` for display.
    fn describe(&self, id: &str) -> Option<(String, String)>;
}

/// Rank-then-enrich shared by every `kb_related`-shaped surface.
pub fn related_enriched(backend: &dyn RelatedSource, id: &str, limit: usize) -> Vec<RelatedItem> {
    backend
        .scored(id, limit)
        .into_iter()
        .map(|(rid, score)| {
            let (title, kind) = backend.describe(&rid).unwrap_or_default();
            RelatedItem {
                id: rid,
                title,
                kind,
                score,
            }
        })
        .collect()
}

/// [`RelatedSource`] mirroring `execute_kb_related`'s original logic exactly:
/// prefer the federated query layer when it knows `id`, else fall back to
/// whichever in-memory KB (primary or a federated instance) contains it;
/// title/kind lookup always goes through the in-memory federation (matching
/// the original executor, which resolved display fields the same way
/// regardless of which path scored the result).
pub struct FederatedRelatedBackend<'a> {
    pub query: Option<&'a dyn KbQueryLayer>,
    pub primary: &'a KnowledgeBase,
    pub instances: &'a HashMap<String, KnowledgeBase>,
}

impl RelatedSource for FederatedRelatedBackend<'_> {
    fn scored(&self, id: &str, limit: usize) -> Vec<(String, f64)> {
        if let Some(q) = self.query {
            if q.contains(id) {
                return q.related(id, limit);
            }
        }
        if self.primary.contains(id) {
            return self.primary.related(id, limit);
        }
        for kb in self.instances.values() {
            if kb.contains(id) {
                return kb.related(id, limit);
            }
        }
        Vec::new()
    }

    fn describe(&self, id: &str) -> Option<(String, String)> {
        if let Some(n) = self.primary.get(id) {
            return Some((n.title.clone(), n.kind.as_str().to_string()));
        }
        for kb in self.instances.values() {
            if let Some(n) = kb.get(id) {
                return Some((n.title.clone(), n.kind.as_str().to_string()));
            }
        }
        None
    }
}

/// [`RelatedSource`] backed by a single [`KbStore`] (`SharedState::kb_store`)
/// — the Scheme runtime's access path, same primary-KB-only scope as
/// [`KbStoreBackend`] above. Requires `KbStore::related` (new default
/// `NotSupported` trait method; overridden by `CozoKbStore` to delegate to
/// its existing inherent `related`, exactly like `neighborhood`/
/// `shortest_path` already do — see `cozo_store/kb_store_impl.rs`).
pub struct KbStoreRelatedBackend<'a>(pub &'a dyn KbStore);

impl RelatedSource for KbStoreRelatedBackend<'_> {
    fn scored(&self, id: &str, limit: usize) -> Vec<(String, f64)> {
        self.0.related(id, limit).unwrap_or_default()
    }

    fn describe(&self, id: &str) -> Option<(String, String)> {
        self.0
            .get_node(id)
            .ok()
            .flatten()
            .map(|n| (n.title, n.kind.as_str().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, NodeKind};

    fn kb_with(nodes: &[(&str, &str, &[&str])]) -> KnowledgeBase {
        let mut kb = KnowledgeBase::new();
        for (id, title, links) in nodes {
            let body = links
                .iter()
                .map(|l| format!("[[{l}]]"))
                .collect::<Vec<_>>()
                .join(" ");
            kb.insert(Node::new(*id, *title, NodeKind::Note, body));
        }
        kb
    }

    #[test]
    fn in_memory_backend_bfs_finds_two_hop_neighbor() {
        // a -> b -> c, depth 2 from a must reach c.
        let kb = kb_with(&[("a", "A", &["b"]), ("b", "B", &["c"]), ("c", "C", &[])]);
        let instances = HashMap::new();
        let registry = KbRegistry::default();
        let backend = InMemoryFederatedBackend {
            primary: &kb,
            instances: &instances,
            registry: &registry,
        };
        let result = bfs_neighborhood(&backend, "a", 2).unwrap();
        let ids: HashSet<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("a"));
        assert!(ids.contains("b"));
        assert!(ids.contains("c"), "depth-2 walk must reach c: {ids:?}");
        let c = result.nodes.iter().find(|n| n.id == "c").unwrap();
        assert_eq!(c.hop, 2);
    }

    #[test]
    fn in_memory_backend_bfs_stops_at_depth() {
        let kb = kb_with(&[("a", "A", &["b"]), ("b", "B", &["c"]), ("c", "C", &[])]);
        let instances = HashMap::new();
        let registry = KbRegistry::default();
        let backend = InMemoryFederatedBackend {
            primary: &kb,
            instances: &instances,
            registry: &registry,
        };
        let result = bfs_neighborhood(&backend, "a", 1).unwrap();
        let ids: HashSet<&str> = result.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("b"));
        assert!(!ids.contains("c"), "depth-1 walk must NOT reach c: {ids:?}");
    }

    #[test]
    fn bfs_neighborhood_unknown_root_is_an_error() {
        let kb = kb_with(&[("a", "A", &[])]);
        let instances = HashMap::new();
        let registry = KbRegistry::default();
        let backend = InMemoryFederatedBackend {
            primary: &kb,
            instances: &instances,
            registry: &registry,
        };
        let err = bfs_neighborhood(&backend, "no:such:node", 1).unwrap_err();
        assert!(err.contains("No KB node"));
    }

    #[test]
    fn bfs_neighborhood_reports_dangling_neighbor_as_missing() {
        // a links to "ghost", which doesn't exist as a node.
        let kb = kb_with(&[("a", "A", &["ghost"])]);
        let instances = HashMap::new();
        let registry = KbRegistry::default();
        let backend = InMemoryFederatedBackend {
            primary: &kb,
            instances: &instances,
            registry: &registry,
        };
        let result = bfs_neighborhood(&backend, "a", 1).unwrap();
        let ghost = result.nodes.iter().find(|n| n.id == "ghost").unwrap();
        assert!(ghost.missing);
        assert!(ghost.title.is_none());
    }

    #[test]
    fn related_enriched_empty_when_backend_scores_nothing() {
        struct EmptyBackend;
        impl RelatedSource for EmptyBackend {
            fn scored(&self, _id: &str, _limit: usize) -> Vec<(String, f64)> {
                Vec::new()
            }
            fn describe(&self, _id: &str) -> Option<(String, String)> {
                None
            }
        }
        let items = related_enriched(&EmptyBackend, "a", 10);
        assert!(items.is_empty());
    }

    #[test]
    fn related_enriched_carries_score_through_to_output() {
        struct FixedBackend;
        impl RelatedSource for FixedBackend {
            fn scored(&self, _id: &str, _limit: usize) -> Vec<(String, f64)> {
                vec![("b".to_string(), 2.5), ("c".to_string(), 1.0)]
            }
            fn describe(&self, id: &str) -> Option<(String, String)> {
                Some((format!("Title-{id}"), "note".to_string()))
            }
        }
        let items = related_enriched(&FixedBackend, "a", 10);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "b");
        assert_eq!(items[0].score, 2.5);
        assert_eq!(items[0].title, "Title-b");
        assert_eq!(items[1].score, 1.0);
    }
}
