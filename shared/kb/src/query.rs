//! KbQueryLayer — CozoDB-first query abstraction for knowledge base reads.
//!
//! All runtime KB reads go through `KbQueryLayer`. The trait has implementations
//! for `CozoKbStore` (direct Datalog queries), `FederatedQuery` (multi-store
//! fan-out), and `CachedQueryLayer` (LRU cache wrapper).

use crate::store::{HealthReport, KbStore, Link, SearchHit, SubGraph};
use crate::{CozoKbStore, Node};
use std::sync::Arc;

/// Read-only query interface for knowledge base operations.
///
/// All runtime reads (help buffers, AI tools, search, link navigation)
/// go through this trait instead of the in-memory `KnowledgeBase`.
pub trait KbQueryLayer: Send + Sync {
    /// Get a node by ID.
    fn get(&self, id: &str) -> Option<Node>;

    /// Check if a node exists.
    fn contains(&self, id: &str) -> bool;

    /// Full-text search across node titles and bodies.
    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit>;

    /// Outgoing links from a node (typed, with rel_type).
    fn links_from(&self, id: &str) -> Vec<Link>;

    /// Incoming links to a node (typed, with rel_type).
    fn links_to(&self, id: &str) -> Vec<Link>;

    /// List all node IDs, optionally filtered by prefix.
    fn list_ids(&self, prefix: Option<&str>) -> Vec<String>;

    /// Return (id, title) pairs for all nodes, optionally filtered by prefix.
    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)>;

    /// Return (id, title, body) triples for all nodes.
    /// Body is truncated to `body_limit` chars (0 = no body).
    /// Default implementation calls `id_title_pairs` + `get` per node (slow).
    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Vec<(String, String, String)> {
        self.id_title_pairs(prefix)
            .into_iter()
            .map(|(id, title)| {
                let body = if body_limit > 0 {
                    self.get(&id)
                        .map(|n| n.body.chars().take(body_limit).collect())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                (id, title, body)
            })
            .collect()
    }

    /// Compute a structured health report.
    fn health_report(&self) -> Option<HealthReport>;

    /// BFS neighborhood subgraph around a node.
    fn neighborhood(&self, id: &str, depth: u32) -> Option<SubGraph>;

    /// Graph-relatedness: `(id, score)` for nodes structurally related to
    /// `id` (co-citation / bibliographic coupling / shared tags), distinct
    /// from lexical `search`. Default returns empty so RPC/daemon layers that
    /// don't implement it degrade gracefully; `CozoQueryLayer` overrides.
    fn related(&self, _id: &str, _limit: usize) -> Vec<(String, f64)> {
        Vec::new()
    }

    /// Evict cached entries for node `id` (Phase D3b). A no-op for layers without a
    /// cache (`CozoQueryLayer`, `FederatedQuery`); `LruQueryLayer` overrides it. The
    /// editor calls this when a KB node changes remotely (a `sync_update` from the
    /// daemon) so the next daemon-routed read returns fresh content, not a stale hit.
    fn invalidate(&self, _id: &str) {}

    /// Fetch a node's authoritative CRDT doc state from the daemon (Phase D3b), for
    /// lazy edit hydration on a thin client: the editor applies this to its in-memory
    /// mirror to obtain the node WITH its real lineage before editing. Default `None`
    /// (no daemon / non-RPC layers); `LruQueryLayer` overrides via `kb/node_crdt`.
    fn node_crdt_state(&self, _id: &str) -> Option<Vec<u8>> {
        None
    }

    /// All nodes carrying a TODO state, for the agenda buffer (Phase D thin-client:
    /// the agenda was mirror-only). Default empty (non-cozo layers); `CozoQueryLayer`
    /// + `LruQueryLayer` implement it. The editor applies state/priority/tag filters.
    fn todo_nodes(&self) -> Vec<Node> {
        Vec::new()
    }

    /// Return all known namespace prefixes (e.g., "cmd:", "concept:").
    fn namespace_prefixes(&self) -> Vec<String> {
        let mut prefixes = std::collections::HashSet::new();
        for id in self.list_ids(None) {
            if let Some(colon) = id.find(':') {
                prefixes.insert(format!("{}:", &id[..colon]));
            }
        }
        let mut result: Vec<String> = prefixes.into_iter().collect();
        result.sort();
        result
    }
}

/// `KbQueryLayer` implementation backed by a `CozoKbStore`.
pub struct CozoQueryLayer {
    store: Arc<CozoKbStore>,
}

impl CozoQueryLayer {
    pub fn new(store: Arc<CozoKbStore>) -> Self {
        Self { store }
    }
}

impl KbQueryLayer for CozoQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        match self.store.get_node(id) {
            Ok(node) => node,
            Err(e) => {
                tracing::warn!(error = %e, id, "CozoQueryLayer::get failed");
                None
            }
        }
    }

    fn contains(&self, id: &str) -> bool {
        matches!(self.store.get_node(id), Ok(Some(_)))
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        self.store.fts_search(query, limit).unwrap_or_default()
    }

    fn links_from(&self, id: &str) -> Vec<Link> {
        self.store.links_from(id).unwrap_or_default()
    }

    fn links_to(&self, id: &str) -> Vec<Link> {
        self.store.links_to(id).unwrap_or_default()
    }

    fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        self.store.list_ids(prefix).unwrap_or_default()
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)> {
        self.store.id_title_pairs(prefix).unwrap_or_default()
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Vec<(String, String, String)> {
        self.store
            .id_title_body_triples(prefix, body_limit)
            .unwrap_or_default()
    }

    fn health_report(&self) -> Option<HealthReport> {
        self.store.health_report().ok()
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Option<SubGraph> {
        self.store.neighborhood(id, depth).ok()
    }

    fn related(&self, id: &str, limit: usize) -> Vec<(String, f64)> {
        self.store.related(id, limit).unwrap_or_default()
    }

    fn todo_nodes(&self) -> Vec<Node> {
        self.store
            .agenda_query(&crate::AgendaFilter::Todo(None))
            .unwrap_or_default()
    }
}

/// Multi-store query layer that fans out reads across primary + instances.
/// Primary is checked first; search results are merged by score.
pub struct FederatedQuery {
    primary: Arc<dyn KbQueryLayer>,
    instances: Vec<(String, Arc<dyn KbQueryLayer>)>,
}

impl FederatedQuery {
    pub fn new(primary: Arc<dyn KbQueryLayer>) -> Self {
        Self {
            primary,
            instances: Vec::new(),
        }
    }

    pub fn add_instance(&mut self, name: String, layer: Arc<dyn KbQueryLayer>) {
        self.instances.push((name, layer));
    }
}

impl KbQueryLayer for FederatedQuery {
    fn get(&self, id: &str) -> Option<Node> {
        if let Some(node) = self.primary.get(id) {
            return Some(node);
        }
        for (_, inst) in &self.instances {
            if let Some(node) = inst.get(id) {
                return Some(node);
            }
        }
        None
    }

    fn contains(&self, id: &str) -> bool {
        self.primary.contains(id) || self.instances.iter().any(|(_, i)| i.contains(id))
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let mut hits = self.primary.search(query, limit);
        let mut seen: std::collections::HashSet<String> =
            hits.iter().map(|h| h.id.clone()).collect();
        for (_, inst) in &self.instances {
            for hit in inst.search(query, limit) {
                if seen.insert(hit.id.clone()) {
                    hits.push(hit);
                }
            }
        }
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        hits
    }

    fn links_from(&self, id: &str) -> Vec<Link> {
        // Return links from whichever store owns the node
        if self.primary.contains(id) {
            return self.primary.links_from(id);
        }
        for (_, inst) in &self.instances {
            if inst.contains(id) {
                return inst.links_from(id);
            }
        }
        Vec::new()
    }

    fn links_to(&self, id: &str) -> Vec<Link> {
        // Merge incoming links from all stores
        let mut links = self.primary.links_to(id);
        for (_, inst) in &self.instances {
            links.extend(inst.links_to(id));
        }
        links
    }

    fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        let mut ids = self.primary.list_ids(prefix);
        let mut seen: std::collections::HashSet<String> = ids.iter().cloned().collect();
        for (_, inst) in &self.instances {
            for id in inst.list_ids(prefix) {
                if seen.insert(id.clone()) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)> {
        let mut pairs = self.primary.id_title_pairs(prefix);
        let mut seen: std::collections::HashSet<String> =
            pairs.iter().map(|(id, _)| id.clone()).collect();
        for (_, inst) in &self.instances {
            for pair in inst.id_title_pairs(prefix) {
                if seen.insert(pair.0.clone()) {
                    pairs.push(pair);
                }
            }
        }
        pairs
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Vec<(String, String, String)> {
        let mut triples = self.primary.id_title_body_triples(prefix, body_limit);
        let mut seen: std::collections::HashSet<String> =
            triples.iter().map(|(id, _, _)| id.clone()).collect();
        for (_, inst) in &self.instances {
            for triple in inst.id_title_body_triples(prefix, body_limit) {
                if seen.insert(triple.0.clone()) {
                    triples.push(triple);
                }
            }
        }
        triples
    }

    fn health_report(&self) -> Option<HealthReport> {
        self.primary.health_report()
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Option<SubGraph> {
        if self.primary.contains(id) {
            return self.primary.neighborhood(id, depth);
        }
        for (_, inst) in &self.instances {
            if inst.contains(id) {
                return inst.neighborhood(id, depth);
            }
        }
        None
    }

    fn related(&self, id: &str, limit: usize) -> Vec<(String, f64)> {
        // Per-instance, like `neighborhood`: relatedness is computed within the
        // instance that owns the node (graph edges don't cross instances).
        if self.primary.contains(id) {
            return self.primary.related(id, limit);
        }
        for (_, inst) in &self.instances {
            if inst.contains(id) {
                return inst.related(id, limit);
            }
        }
        Vec::new()
    }

    fn todo_nodes(&self) -> Vec<Node> {
        let mut out = self.primary.todo_nodes();
        let mut seen: std::collections::HashSet<String> =
            out.iter().map(|n| n.id.clone()).collect();
        for (_, inst) in &self.instances {
            for n in inst.todo_nodes() {
                if seen.insert(n.id.clone()) {
                    out.push(n);
                }
            }
        }
        out
    }
}

/// Fallback query layer wrapping an in-memory `KnowledgeBase`.
/// Used when no CozoDB store is available.
pub struct InMemoryQueryLayer {
    kb: std::sync::Mutex<crate::KnowledgeBase>,
}

impl InMemoryQueryLayer {
    pub fn new(kb: crate::KnowledgeBase) -> Self {
        Self {
            kb: std::sync::Mutex::new(kb),
        }
    }

    /// Get a mutable reference to the underlying KB (for inserts/updates).
    pub fn kb_mut(&self) -> std::sync::MutexGuard<'_, crate::KnowledgeBase> {
        self.kb.lock().unwrap()
    }
}

impl KbQueryLayer for InMemoryQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        self.kb.lock().unwrap().get(id).cloned()
    }

    fn contains(&self, id: &str) -> bool {
        self.kb.lock().unwrap().contains(id)
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let kb = self.kb.lock().unwrap();
        kb.search(query)
            .into_iter()
            .take(limit)
            .map(|id| SearchHit { id, score: 1.0 })
            .collect()
    }

    fn links_from(&self, id: &str) -> Vec<Link> {
        let kb = self.kb.lock().unwrap();
        kb.links_from(id)
            .into_iter()
            .map(|dst| Link {
                src: id.to_string(),
                dst,
                rel_type: "references".to_string(),
                display: None,
                weight: 1.0,
                confidence: 1.0,
            })
            .collect()
    }

    fn links_to(&self, id: &str) -> Vec<Link> {
        let kb = self.kb.lock().unwrap();
        kb.links_to(id)
            .into_iter()
            .map(|src| Link {
                src,
                dst: id.to_string(),
                rel_type: "references".to_string(),
                display: None,
                weight: 1.0,
                confidence: 1.0,
            })
            .collect()
    }

    fn list_ids(&self, prefix: Option<&str>) -> Vec<String> {
        let kb = self.kb.lock().unwrap();
        kb.list_ids(prefix)
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Vec<(String, String)> {
        let kb = self.kb.lock().unwrap();
        kb.list_ids(prefix)
            .into_iter()
            .filter_map(|id| {
                let title = kb.get(&id)?.title.clone();
                Some((id, title))
            })
            .collect()
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Vec<(String, String, String)> {
        let kb = self.kb.lock().unwrap();
        kb.list_ids(prefix)
            .into_iter()
            .filter_map(|id| {
                let node = kb.get(&id)?;
                let body = if body_limit > 0 {
                    node.body.chars().take(body_limit).collect()
                } else {
                    String::new()
                };
                Some((id, node.title.clone(), body))
            })
            .collect()
    }

    fn health_report(&self) -> Option<HealthReport> {
        None // In-memory KB uses KbHealthReport, not store::HealthReport
    }

    fn neighborhood(&self, _id: &str, _depth: u32) -> Option<SubGraph> {
        None
    }

    fn related(&self, id: &str, limit: usize) -> Vec<(String, f64)> {
        self.kb.lock().unwrap().related(id, limit)
    }

    fn todo_nodes(&self) -> Vec<Node> {
        self.kb
            .lock()
            .unwrap()
            .todo_nodes()
            .into_iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, NodeKind};

    #[test]
    fn cozo_query_layer_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("test.cozo")).unwrap());
        store
            .insert_node(&Node::new("test:a", "Alpha", NodeKind::Note, "body text"))
            .unwrap();

        let layer = CozoQueryLayer::new(store);
        assert!(layer.contains("test:a"));
        assert!(!layer.contains("test:b"));

        let node = layer.get("test:a").unwrap();
        assert_eq!(node.title, "Alpha");

        let ids = layer.list_ids(Some("test:"));
        assert!(ids.contains(&"test:a".to_string()));

        let pairs = layer.id_title_pairs(None);
        assert!(pairs.iter().any(|(id, _)| id == "test:a"));
    }

    #[test]
    fn federated_query_primary_first() {
        let tmp = tempfile::tempdir().unwrap();
        let store1 = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        let store2 = Arc::new(CozoKbStore::open(tmp.path().join("inst.cozo")).unwrap());

        store1
            .insert_node(&Node::new("shared", "Primary Version", NodeKind::Note, ""))
            .unwrap();
        store2
            .insert_node(&Node::new("shared", "Instance Version", NodeKind::Note, ""))
            .unwrap();
        store2
            .insert_node(&Node::new("only:inst", "Instance Only", NodeKind::Note, ""))
            .unwrap();

        let primary = Arc::new(CozoQueryLayer::new(store1));
        let inst = Arc::new(CozoQueryLayer::new(store2));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("test".into(), inst);

        // Primary wins for shared IDs
        let node = federated.get("shared").unwrap();
        assert_eq!(node.title, "Primary Version");

        // Instance-only nodes are found
        assert!(federated.contains("only:inst"));
        let node = federated.get("only:inst").unwrap();
        assert_eq!(node.title, "Instance Only");
    }

    #[test]
    fn todo_nodes_via_query_layers() {
        // Cozo layer: only TODO-bearing nodes come back.
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("todo.cozo")).unwrap());
        store
            .insert_node(&Node::new("task:a", "Do A", NodeKind::Task, "").with_todo_state("TODO"))
            .unwrap();
        store
            .insert_node(&Node::new("task:b", "Do B", NodeKind::Task, "").with_todo_state("DONE"))
            .unwrap();
        store
            .insert_node(&Node::new("note:c", "Plain note", NodeKind::Note, ""))
            .unwrap();

        let cozo = Arc::new(CozoQueryLayer::new(store));
        let todos = cozo.todo_nodes();
        let ids: Vec<_> = todos.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"task:a"));
        assert!(ids.contains(&"task:b"));
        assert!(!ids.contains(&"note:c"));

        // In-memory layer mirrors the same TODO set.
        let mut kb = crate::KnowledgeBase::new();
        kb.insert(Node::new("task:x", "X", NodeKind::Task, "").with_todo_state("TODO"));
        kb.insert(Node::new("note:y", "Y", NodeKind::Note, ""));
        let mem = InMemoryQueryLayer::new(kb);
        let mem_todos = mem.todo_nodes();
        assert_eq!(mem_todos.len(), 1);
        assert_eq!(mem_todos[0].id, "task:x");

        // Federated layer dedups primary over instance and unions instance-only.
        let mut federated = FederatedQuery::new(cozo);
        let tmp2 = tempfile::tempdir().unwrap();
        let store2 = Arc::new(CozoKbStore::open(tmp2.path().join("inst.cozo")).unwrap());
        store2
            .insert_node(&Node::new("task:a", "Dup", NodeKind::Task, "").with_todo_state("TODO"))
            .unwrap();
        store2
            .insert_node(
                &Node::new("task:z", "Inst only", NodeKind::Task, "").with_todo_state("TODO"),
            )
            .unwrap();
        federated.add_instance("inst".into(), Arc::new(CozoQueryLayer::new(store2)));
        let fed_ids: Vec<_> = federated
            .todo_nodes()
            .into_iter()
            .map(|n| n.id)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(fed_ids.contains(&"task:a".to_string()));
        assert!(fed_ids.contains(&"task:z".to_string()));
        // Deduped: task:a appears once.
        assert_eq!(
            federated
                .todo_nodes()
                .iter()
                .filter(|n| n.id == "task:a")
                .count(),
            1
        );
    }

    #[test]
    fn in_memory_query_layer() {
        let mut kb = crate::KnowledgeBase::new();
        kb.insert(Node::new(
            "note:a",
            "Alpha",
            NodeKind::Note,
            "body [[note:b]]",
        ));
        kb.insert(Node::new("note:b", "Beta", NodeKind::Note, ""));

        let layer = InMemoryQueryLayer::new(kb);
        assert!(layer.contains("note:a"));
        assert!(!layer.contains("note:c"));

        let links = layer.links_from("note:a");
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].dst, "note:b");

        let backlinks = layer.links_to("note:b");
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].src, "note:a");
    }
}
