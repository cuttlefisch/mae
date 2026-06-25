//! KB projector — deterministic projection of CRDT node docs into the CozoDB query
//! store (ADR-029 / ADR-030).
//!
//! The CRDT (`KbNodeDoc`) is the source of truth; CozoDB is a derived projection. The
//! **structural projection is a pure function of the CRDT state**: parse a node's
//! source text → a cozo node + its links + FTS. Because the parse is
//! deterministic, every peer with the same converged CRDT derives a byte-identical
//! graph (the ADR-029 determinism contract). This is the seam the change feed
//! (`doc_store.apply_update`) drives, covering hub + p2p uniformly.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use mae_kb::{CozoKbStore, KbStore, Node, NodeKind, NodeSource};
use tokio::sync::mpsc;

use crate::doc_store::DocStore;

/// Provides the per-KB cozo projection instance for a `kb_id` (ADR-029, per-KB stores).
/// The daemon implements this over its federation instance stores; tests use an
/// in-memory provider. `store_for` may create the instance on first use.
pub trait ProjectionStores: Send + Sync {
    fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String>;
}

/// Routing state the projector maintains from collection manifests (ADR-029 B3), so a
/// node-doc change (which doesn't carry its `kb_id`) can be routed to the right KB(s).
#[derive(Default)]
struct ProjectionIndex {
    /// `kb_id` → its current projected node set (the last-seen manifest).
    manifests: HashMap<String, HashSet<String>>,
    /// `node_id` → the KBs whose manifest lists it (reverse index for node routing).
    node_to_kbs: HashMap<String, HashSet<String>>,
}

/// Drives the cozo projection from the doc_store change feed (ADR-029 B2/B3). A KB doc
/// change is read from the doc_store and materialized into the **per-KB** cozo instance:
/// a collection change (`kbc:`) updates the manifest (projecting added nodes, deleting
/// removed ones) and the node→kb routing index; a node change (`kb:`) re-projects the
/// node into every KB that lists it. One mechanism for hub + p2p — both land at
/// `doc_store.apply_update`, which emits to the feed.
pub struct Projector {
    doc_store: Arc<DocStore>,
    stores: Arc<dyn ProjectionStores>,
    index: Mutex<ProjectionIndex>,
}

impl Projector {
    pub fn new(doc_store: Arc<DocStore>, stores: Arc<dyn ProjectionStores>) -> Self {
        Self {
            doc_store,
            stores,
            index: Mutex::new(ProjectionIndex::default()),
        }
    }

    /// Project one changed doc. Reading state happens off the doc write path (the
    /// channel decouples); the index lock is never held across an await.
    pub async fn project_doc(&self, doc_name: &str) -> Result<(), String> {
        if let Some(node_id) = doc_name.strip_prefix("kb:") {
            self.project_node_change(node_id).await
        } else if let Some(kb_id) = doc_name.strip_prefix("kbc:") {
            self.project_collection_change(kb_id).await
        } else {
            Ok(())
        }
    }

    /// A node doc changed → re-project it into every KB whose manifest lists it. If no
    /// collection has been seen yet for this node, it's a no-op — the collection change
    /// will project it (and register the routing).
    async fn project_node_change(&self, node_id: &str) -> Result<(), String> {
        let kbs: Vec<String> = {
            let idx = self.index.lock().unwrap();
            idx.node_to_kbs
                .get(node_id)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default()
        };
        if kbs.is_empty() {
            return Ok(());
        }
        let (state, _sv) = self
            .doc_store
            .encode_state_and_sv(&format!("kb:{node_id}"))
            .await
            .map_err(|e| format!("read 'kb:{node_id}': {e}"))?;
        for kb_id in kbs {
            let store = self.stores.store_for(&kb_id)?;
            project_node(&store, node_id, &state)?;
        }
        Ok(())
    }

    /// A collection changed → diff its manifest against the last-seen one: delete removed
    /// nodes from this KB's projection, project added nodes, and update the routing index.
    async fn project_collection_change(&self, kb_id: &str) -> Result<(), String> {
        let (coll_state, _sv) = self
            .doc_store
            .encode_state_and_sv(&format!("kbc:{kb_id}"))
            .await
            .map_err(|e| format!("read 'kbc:{kb_id}': {e}"))?;
        let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&coll_state)
            .map_err(|e| format!("parse 'kbc:{kb_id}': {e}"))?;
        let current: HashSet<String> = coll.list_nodes().into_iter().map(|(id, _)| id).collect();

        let prev = {
            let idx = self.index.lock().unwrap();
            idx.manifests.get(kb_id).cloned().unwrap_or_default()
        };
        let removed: Vec<String> = prev.difference(&current).cloned().collect();
        let added: Vec<String> = current.difference(&prev).cloned().collect();

        let store = self.stores.store_for(kb_id)?;
        for node_id in &removed {
            if let Err(e) = store.delete_node(node_id) {
                tracing::debug!(kb = %kb_id, node = %node_id, error = %e, "project: delete failed");
            }
        }
        for node_id in &added {
            // Best-effort: the node doc may not have synced yet — it will be projected on
            // its own `kb:` change once it arrives (routing is registered below).
            if let Ok((state, _sv)) = self
                .doc_store
                .encode_state_and_sv(&format!("kb:{node_id}"))
                .await
            {
                if let Err(e) = project_node(&store, node_id, &state) {
                    tracing::debug!(kb = %kb_id, node = %node_id, error = %e, "project: node failed");
                }
            }
        }

        // Update routing: drop removed from node_to_kbs, add current; store the manifest.
        let mut idx = self.index.lock().unwrap();
        for node_id in &removed {
            if let Some(set) = idx.node_to_kbs.get_mut(node_id) {
                set.remove(kb_id);
                if set.is_empty() {
                    idx.node_to_kbs.remove(node_id);
                }
            }
        }
        for node_id in &current {
            idx.node_to_kbs
                .entry(node_id.clone())
                .or_default()
                .insert(kb_id.to_string());
        }
        idx.manifests.insert(kb_id.to_string(), current);
        Ok(())
    }

    /// Rebuild a KB's cozo projection from its CRDT (ADR-029 self-heal / initial
    /// projection): forget the cached manifest so every node re-projects, then run the
    /// collection projection. Because the structural projection is deterministic, the
    /// rebuilt cozo is identical to an incrementally-maintained one — so a corrupt or
    /// deleted cozo store heals by replaying the CRDT. Returns the projected node count.
    pub async fn rebuild_kb(&self, kb_id: &str) -> Result<usize, String> {
        {
            let mut idx = self.index.lock().unwrap();
            if let Some(nodes) = idx.manifests.remove(kb_id) {
                for node_id in nodes {
                    if let Some(set) = idx.node_to_kbs.get_mut(&node_id) {
                        set.remove(kb_id);
                        if set.is_empty() {
                            idx.node_to_kbs.remove(&node_id);
                        }
                    }
                }
            }
        }
        self.project_collection_change(kb_id).await?;
        Ok(self
            .index
            .lock()
            .unwrap()
            .manifests
            .get(kb_id)
            .map_or(0, |s| s.len()))
    }

    /// Drain the change feed, projecting each changed doc until the channel closes.
    pub async fn run(self, mut rx: mpsc::UnboundedReceiver<String>) {
        while let Some(doc_name) = rx.recv().await {
            if let Err(e) = self.project_doc(&doc_name).await {
                tracing::warn!(doc = %doc_name, error = %e, "projection failed");
            }
        }
    }
}

/// Project a single KB node doc — the `KbNodeDoc` yrs state stored at `kb:{node_id}` —
/// into the cozo query store: materialize the node (title/body/tags/kind) + FTS, then
/// wire the **typed** link graph parsed from the node's source text (ADR-030: rel_type/
/// weight/confidence live in the text). Deterministic + idempotent — re-projecting the
/// same state yields the same node + link set.
pub fn project_node(store: &CozoKbStore, node_id: &str, state: &[u8]) -> Result<(), String> {
    let doc = mae_sync::kb::KbNodeDoc::from_bytes(state)
        .map_err(|e| format!("parse node doc '{node_id}': {e}"))?;
    // Kind is a cozo-only projection field (the CRDT carries only content); derive it
    // deterministically from the id namespace.
    let node = Node::from_crdt_doc(&doc, kind_from_id(node_id), NodeSource::Federation);
    store
        .insert_node(&node)
        .map_err(|e| format!("project node '{node_id}': {e}"))?;

    // Replace insert_node's generic links with the typed parse (ADR-030 / Phase C).
    let known = store.known_rel_types().unwrap_or_default();
    let known_ref = (!known.is_empty()).then_some(&known);
    let links: Vec<(String, String, f64, f64)> =
        mae_kb::org::parse_typed_links(&node.body, &node.id, known_ref)
            .into_iter()
            .map(|l| (l.target, l.rel_type, l.weight, l.confidence))
            .collect();
    store
        .replace_node_links(&node.id, &links)
        .map_err(|e| format!("project links for '{node_id}': {e}"))?;
    Ok(())
}

/// Derive a node's kind from its id namespace (e.g. `concept:x` → Concept), defaulting
/// to Note. A deterministic rule so the projection converges across peers; richer
/// kind handling (from in-text org metadata) lands with ADR-030 (Phase C).
fn kind_from_id(node_id: &str) -> NodeKind {
    match node_id.split(':').next().unwrap_or("") {
        "concept" => NodeKind::Concept,
        "cmd" | "command" => NodeKind::Command,
        "lesson" => NodeKind::Lesson,
        "tutorial" => NodeKind::Tutorial,
        "category" => NodeKind::Category,
        "meta" => NodeKind::Meta,
        _ => NodeKind::Note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_kb::store::SearchHit;

    /// In-memory per-KB store provider for tests (creates an `open_mem` cozo per kb_id).
    struct MemStores(Mutex<HashMap<String, Arc<CozoKbStore>>>);
    impl MemStores {
        fn new() -> Arc<Self> {
            Arc::new(Self(Mutex::new(HashMap::new())))
        }
    }
    impl ProjectionStores for MemStores {
        fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String> {
            let mut m = self.0.lock().unwrap();
            Ok(Arc::clone(m.entry(kb_id.to_string()).or_insert_with(
                || Arc::new(CozoKbStore::open_mem().unwrap()),
            )))
        }
    }

    #[test]
    fn project_node_materializes_node_links_and_fts() {
        let store = CozoKbStore::open_mem().unwrap();
        let doc = mae_sync::kb::KbNodeDoc::new(
            "concept:rope",
            "Rope",
            "The rope buffer structure. See [[concept:buffer][the buffer]]{w=0.8 c=0.9}.",
            &["alpha".to_string()],
        );
        project_node(&store, "concept:rope", &doc.encode()).unwrap();

        // The node is materialized with kind derived from the namespace.
        let n = store.get_node("concept:rope").unwrap().unwrap();
        assert_eq!(n.title, "Rope");
        assert_eq!(n.kind, NodeKind::Concept);
        assert!(n.tags.contains(&"alpha".to_string()));

        // FTS finds it.
        let hits: Vec<SearchHit> = store.fts_search("rope buffer", 10).unwrap();
        assert!(
            hits.iter().any(|h| h.id == "concept:rope"),
            "FTS should find the node"
        );

        // The body link is projected as a TYPED edge with its in-text weight/confidence
        // (ADR-030). Untyped → rel_type "references"; the attribute group sets w/c.
        let links = store.links_from("concept:rope").unwrap();
        let link = links
            .iter()
            .find(|l| l.dst == "concept:buffer")
            .unwrap_or_else(|| panic!("link not projected, got: {links:?}"));
        assert_eq!(link.rel_type, "references");
        assert_eq!(link.weight, 0.8);
        assert_eq!(link.confidence, 0.9);
    }

    #[test]
    fn projection_is_deterministic_across_stores() {
        // Same CRDT state ⇒ identical projected node + links on two independent stores
        // (the ADR-029 determinism contract, at the single-node level).
        let doc = mae_sync::kb::KbNodeDoc::new(
            "concept:x",
            "X",
            "links [[concept:a]] and [[concept:b]]",
            &[],
        );
        let state = doc.encode();

        let project_all = |store: &CozoKbStore| -> (String, Vec<String>) {
            project_node(store, "concept:x", &state).unwrap();
            let title = store.get_node("concept:x").unwrap().unwrap().title;
            let mut dsts: Vec<String> = store
                .links_from("concept:x")
                .unwrap()
                .into_iter()
                .map(|l| l.dst)
                .collect();
            dsts.sort();
            (title, dsts)
        };

        let a = project_all(&CozoKbStore::open_mem().unwrap());
        let b = project_all(&CozoKbStore::open_mem().unwrap());
        assert_eq!(a, b, "the structural projection must be deterministic");
        assert_eq!(a.1, vec!["concept:a".to_string(), "concept:b".to_string()]);
    }

    #[tokio::test]
    async fn change_feed_emits_only_durable_kb_docs() {
        // ADR-029 B2: a KB doc mutation emits to the change feed; an ephemeral doc does not.
        use crate::storage::SqliteBackend;
        let doc_store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let (tx, mut rx) = mpsc::unbounded_channel();
        doc_store.set_change_feed(tx);

        let node = mae_sync::kb::KbNodeDoc::new("concept:x", "X", "x", &[]);
        doc_store
            .apply_update("kb:concept:x", &node.encode(), None)
            .await
            .unwrap();
        let scratch = mae_sync::kb::KbNodeDoc::new("s", "S", "s", &[]);
        doc_store
            .apply_update("scratch:buf", &scratch.encode(), None)
            .await
            .unwrap();

        assert_eq!(rx.recv().await.unwrap(), "kb:concept:x");
        assert!(
            rx.try_recv().is_err(),
            "ephemeral docs must not emit changes"
        );
    }

    #[tokio::test]
    async fn collection_change_projects_routes_and_deletes_nodes() {
        // ADR-029 B3: a collection change projects its nodes into the KB's per-KB cozo
        // instance + registers node→kb routing; a later node change is routed there; a
        // node removed from the manifest is deleted from the projection.
        use crate::storage::SqliteBackend;
        let doc_store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let stores = MemStores::new();
        let projector = Projector::new(Arc::clone(&doc_store), stores.clone());

        // Seed two node docs + a collection listing them.
        let a = mae_sync::kb::KbNodeDoc::new("concept:a", "A", "see [[concept:b]]", &[]);
        doc_store
            .apply_update("kb:concept:a", &a.encode(), None)
            .await
            .unwrap();
        let b = mae_sync::kb::KbNodeDoc::new("concept:b", "B", "b body", &[]);
        doc_store
            .apply_update("kb:concept:b", &b.encode(), None)
            .await
            .unwrap();
        let mut coll = mae_sync::kb::KbCollectionDoc::new("kb1", "owner");
        coll.add_node("concept:a", "A");
        coll.add_node("concept:b", "B");
        doc_store
            .share_doc("kbc:kb1", &coll.encode_state())
            .await
            .unwrap();

        // Project the collection → both nodes land in kb1's store; routing registered.
        projector.project_doc("kbc:kb1").await.unwrap();
        let store = stores.store_for("kb1").unwrap();
        assert_eq!(store.get_node("concept:a").unwrap().unwrap().title, "A");
        assert_eq!(store.get_node("concept:b").unwrap().unwrap().title, "B");

        // Edit concept:a's title on its EXISTING CRDT lineage (a real edit — applying a
        // fresh independent doc would merge, not replace). The node change is routed to kb1.
        let (a_state, _sv) = doc_store.encode_state_and_sv("kb:concept:a").await.unwrap();
        let mut a_doc = mae_sync::kb::KbNodeDoc::from_bytes_with_client_id(&a_state, 999).unwrap();
        let edit = a_doc.set_title("A2");
        doc_store
            .apply_update("kb:concept:a", &edit, None)
            .await
            .unwrap();
        projector.project_doc("kb:concept:a").await.unwrap();
        assert_eq!(store.get_node("concept:a").unwrap().unwrap().title, "A2");

        // Remove concept:b from the manifest → it's deleted from the projection.
        let mut coll2 = mae_sync::kb::KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
        coll2.remove_node("concept:b");
        doc_store
            .share_doc("kbc:kb1", &coll2.encode_state())
            .await
            .unwrap();
        projector.project_doc("kbc:kb1").await.unwrap();
        assert!(
            store.get_node("concept:b").unwrap().is_none(),
            "a node removed from the manifest is deleted from the projection"
        );
        assert!(
            store.get_node("concept:a").unwrap().is_some(),
            "kept nodes remain"
        );
    }

    #[tokio::test]
    async fn rebuild_kb_reprojects_the_whole_kb_from_crdt() {
        // ADR-029 self-heal: rebuild repopulates a KB's projection from the CRDT (e.g.
        // after the cozo store is lost). The deterministic projection ⇒ identical result.
        use crate::storage::SqliteBackend;
        let doc_store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let stores = MemStores::new();
        let projector = Projector::new(Arc::clone(&doc_store), stores.clone());

        let a = mae_sync::kb::KbNodeDoc::new("concept:a", "A", "a", &[]);
        doc_store
            .apply_update("kb:concept:a", &a.encode(), None)
            .await
            .unwrap();
        let mut coll = mae_sync::kb::KbCollectionDoc::new("kb1", "owner");
        coll.add_node("concept:a", "A");
        doc_store
            .share_doc("kbc:kb1", &coll.encode_state())
            .await
            .unwrap();

        let n = projector.rebuild_kb("kb1").await.unwrap();
        assert_eq!(n, 1, "one node projected");
        let store = stores.store_for("kb1").unwrap();
        assert_eq!(store.get_node("concept:a").unwrap().unwrap().title, "A");
    }
}
