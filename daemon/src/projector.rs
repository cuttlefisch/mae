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
/// The daemon implements this over its federation instance stores
/// ([`crate::projection_stores::DaemonProjectionStores`]); tests use an in-memory
/// provider. `store_for` may create the instance on first use.
///
/// Async because the production implementation resolves through `DaemonState`, which
/// lives behind a `tokio::sync::Mutex`. It follows ADR-054's snapshot-then-drop idiom —
/// take the lock only long enough to clone the `Arc`, never across the Cozo call — so
/// this must be awaited rather than blocking a runtime thread. The alternative, keeping
/// a second registry snapshot beside `DaemonState` purely to make this sync, would
/// duplicate state that can then go stale (principle #8).
#[async_trait::async_trait]
pub trait ProjectionStores: Send + Sync {
    async fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String>;
}

/// What a reconciliation pass found between CRDT truth and the Cozo projection.
///
/// Empty means the projection is exactly what CRDT truth derives — the only outcome a
/// healthy daemon should ever report.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct DriftReport {
    /// In CRDT truth, absent from the projection.
    pub missing: Vec<String>,
    /// In both, but the projected content does not match what the CRDT derives.
    pub differing: Vec<String>,
    /// In the projection, no longer in CRDT truth.
    pub extra: Vec<String>,
}

impl DriftReport {
    /// No drift at all.
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.differing.is_empty() && self.extra.is_empty()
    }

    /// Total number of nodes affected.
    pub fn total(&self) -> usize {
        self.missing.len() + self.differing.len() + self.extra.len()
    }
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
            let idx = self.index.lock().unwrap_or_else(|e| e.into_inner());
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
            let store = self.stores.store_for(&kb_id).await?;
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
            let idx = self.index.lock().unwrap_or_else(|e| e.into_inner());
            idx.manifests.get(kb_id).cloned().unwrap_or_default()
        };
        let removed: Vec<String> = prev.difference(&current).cloned().collect();
        let added: Vec<String> = current.difference(&prev).cloned().collect();

        let store = self.stores.store_for(kb_id).await?;
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
        let mut idx = self.index.lock().unwrap_or_else(|e| e.into_inner());
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
            let mut idx = self.index.lock().unwrap_or_else(|e| e.into_inner());
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

    /// Compare a KB's live Cozo projection against CRDT truth, and optionally heal it.
    ///
    /// The **offline bulk verification** tier. A projection maintained incrementally from
    /// a change feed can drift for reasons the feed cannot see — a dropped emit, a failed
    /// write, a direct edit to the store, a crash between the CRDT write and the
    /// projection. ADR-029 makes the projection a pure deterministic function of CRDT
    /// state, so drift is always detectable by re-deriving and comparing; this is that
    /// check made routine rather than theoretical.
    ///
    /// Returns a [`DriftReport`]. With `heal` set, differing and missing nodes are
    /// re-projected and extra ones deleted, so the call is **idempotent**: running it
    /// twice in a row must report zero drift the second time. That idempotency is what
    /// makes it safe to run repeatedly, which is the property LinkedIn's migration work
    /// identifies as the enabler for a self-healing verification loop.
    ///
    /// Deliberately compares **materialized content**, not encoded bytes: two stores can
    /// hold the same node with different internal representation, and a byte comparison
    /// would report drift that does not exist.
    pub async fn reconcile_kb(&self, kb_id: &str, heal: bool) -> Result<DriftReport, String> {
        let (coll_state, _sv) = self
            .doc_store
            .encode_state_and_sv(&format!("kbc:{kb_id}"))
            .await
            .map_err(|e| format!("read 'kbc:{kb_id}': {e}"))?;
        let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&coll_state)
            .map_err(|e| format!("parse 'kbc:{kb_id}': {e}"))?;
        let truth: HashSet<String> = coll.list_nodes().into_iter().map(|(id, _)| id).collect();

        let store = self.stores.store_for(kb_id).await?;
        let mut report = DriftReport::default();

        // Nodes the projection is missing, or holds with the wrong content.
        for node_id in &truth {
            let Ok((state, _sv)) = self
                .doc_store
                .encode_state_and_sv(&format!("kb:{node_id}"))
                .await
            else {
                // No node doc yet — the manifest lists it but content has not synced.
                // Not drift: there is nothing to project.
                continue;
            };
            let expected = mae_sync::kb::KbNodeDoc::from_bytes(&state)
                .map_err(|e| format!("parse 'kb:{node_id}': {e}"))?;
            match store.get_node(node_id) {
                Ok(Some(actual)) => {
                    if actual.title != expected.title()
                        || actual.body != expected.body()
                        || actual.tags != expected.tags()
                    {
                        report.differing.push(node_id.clone());
                        if heal {
                            project_node(&store, node_id, &state)?;
                        }
                    }
                }
                _ => {
                    report.missing.push(node_id.clone());
                    if heal {
                        project_node(&store, node_id, &state)?;
                    }
                }
            }
        }

        // Nodes the projection holds that CRDT truth no longer lists.
        if let Ok(ids) = store.list_ids(None) {
            for id in ids {
                if !truth.contains(&id) {
                    report.extra.push(id.clone());
                    if heal {
                        let _ = store.delete_node(&id);
                    }
                }
            }
        }

        report.missing.sort();
        report.differing.sort();
        report.extra.sort();
        Ok(report)
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

    // Replace insert_node's generic links with the typed parse (ADR-030 / Phase C):
    // rel/weight/confidence come from each link's inline `?query`.
    let links: Vec<(String, String, f64, f64)> =
        mae_kb::org::parse_typed_links(&node.body, &node.id)
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
    #[async_trait::async_trait]
    impl ProjectionStores for MemStores {
        async fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String> {
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
            "The rope buffer structure. See [[concept:buffer?w=0.8&c=0.9][the buffer]].",
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
        // (ADR-030). No `?rel=` → rel_type "references"; the `?w=&c=` query sets w/c.
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
        let store = stores.store_for("kb1").await.unwrap();
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
        let store = stores.store_for("kb1").await.unwrap();
        assert_eq!(store.get_node("concept:a").unwrap().unwrap().title, "A");
    }

    #[tokio::test]
    async fn poisoned_index_lock_does_not_cascade_into_the_next_call() {
        // Adversarial (principle #14): a real panic while the index lock is held
        // must not poison the mutex into permanently failing every SUBSEQUENT
        // project_doc call — nothing about the panicking caller's failure has
        // anything to do with the next one.
        use crate::storage::SqliteBackend;
        let doc_store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let stores = MemStores::new();
        let projector = Arc::new(Projector::new(Arc::clone(&doc_store), stores));

        // Poison the index mutex: a thread panics while holding the lock (the
        // standard Rust poisoning trigger).
        let p2 = Arc::clone(&projector);
        let handle = std::thread::spawn(move || {
            let _guard = p2.index.lock().unwrap();
            panic!("simulated failure while holding the index lock");
        });
        assert!(
            handle.join().is_err(),
            "the poisoning thread should have panicked"
        );

        // A real production call through the now-poisoned lock must succeed, not
        // cascade-panic.
        let result = projector.project_doc("kb:concept:a").await;
        assert!(
            result.is_ok(),
            "a poisoned index lock must not cascade into a panic on the next call: {result:?}"
        );
    }
}

#[cfg(test)]
mod gate_c_tests {
    use super::*;
    use crate::storage::SqliteBackend;
    use mae_kb::store::SearchHit;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    struct MemStores(StdMutex<HashMap<String, Arc<CozoKbStore>>>);
    impl MemStores {
        fn new() -> Arc<Self> {
            Arc::new(Self(StdMutex::new(HashMap::new())))
        }
    }
    #[async_trait::async_trait]
    impl ProjectionStores for MemStores {
        async fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String> {
            let mut m = self.0.lock().unwrap();
            Ok(Arc::clone(m.entry(kb_id.to_string()).or_insert_with(
                || Arc::new(CozoKbStore::open_mem().unwrap()),
            )))
        }
    }

    /// Seed a doc store with a collection listing `nodes`, and each node's doc.
    async fn seed(doc_store: &Arc<DocStore>, kb_id: &str, nodes: &[(&str, &str, &str)]) {
        let mut coll = mae_sync::kb::KbCollectionDoc::new(kb_id, "owner");
        for (id, title, body) in nodes {
            let n = mae_sync::kb::KbNodeDoc::new(id, title, body, &[]);
            doc_store
                .apply_update(&format!("kb:{id}"), &n.encode(), None)
                .await
                .unwrap();
            coll.add_node(id, title);
        }
        doc_store
            .apply_update(&format!("kbc:{kb_id}"), &coll.encode_state(), None)
            .await
            .unwrap();
    }

    fn mem_store() -> Arc<DocStore> {
        Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ))
    }

    /// GATE C.1 — a CRDT node write is visible to a Cozo-backed query.
    ///
    /// This is the whole point of wiring the projector, and it failed before it: the
    /// change feed had no subscriber, so `kb/search` and every other `kb/query.*` caller
    /// read a projection that no CRDT write ever reached.
    #[tokio::test]
    async fn a_crdt_node_write_becomes_visible_to_a_cozo_query() {
        let doc_store = mem_store();
        let stores = MemStores::new();
        let projector = Projector::new(Arc::clone(&doc_store), stores.clone());

        seed(
            &doc_store,
            "kb1",
            &[("concept:rope", "Rope", "the rope buffer structure")],
        )
        .await;
        projector.project_doc("kbc:kb1").await.unwrap();

        let store = stores.store_for("kb1").await.unwrap();
        let hits: Vec<SearchHit> = store.fts_search("rope buffer", 10).unwrap();
        assert!(
            hits.iter().any(|h| h.id == "concept:rope"),
            "a CRDT-written node must be findable through the cozo projection: {hits:?}"
        );

        // And a subsequent EDIT reaches the projection too — not just the initial seed.
        let mut edited = mae_sync::kb::KbNodeDoc::from_bytes(
            &doc_store
                .encode_state_and_sv("kb:concept:rope")
                .await
                .unwrap()
                .0,
        )
        .unwrap();
        let upd = edited.set_body("now mentions zippers instead");
        doc_store
            .apply_update("kb:concept:rope", &upd, None)
            .await
            .unwrap();
        projector.project_doc("kb:concept:rope").await.unwrap();

        let n = store.get_node("concept:rope").unwrap().unwrap();
        assert_eq!(
            n.body, "now mentions zippers instead",
            "an edit after the initial projection must also land"
        );
    }

    /// GATE C.2 — re-projection is idempotent and order-independent.
    ///
    /// ADR-029's determinism contract: the projection is a pure function of CRDT state,
    /// so replaying the same changes in any order converges on the same rows. Without
    /// this, a reconciliation pass could itself introduce drift.
    #[tokio::test]
    async fn reprojection_is_idempotent_and_order_independent() {
        let nodes = [
            ("concept:a", "A", "see [[concept:b]]"),
            ("concept:b", "B", "b body"),
            ("concept:c", "C", "c body with words"),
        ];

        // Two projectors, same CRDT state, different apply orders.
        let mut snapshots = Vec::new();
        for order in [[0usize, 1, 2], [2, 0, 1]] {
            let doc_store = mem_store();
            let stores = MemStores::new();
            let projector = Projector::new(Arc::clone(&doc_store), stores.clone());
            seed(&doc_store, "kb1", &nodes).await;

            projector.project_doc("kbc:kb1").await.unwrap();
            for i in order {
                projector
                    .project_doc(&format!("kb:{}", nodes[i].0))
                    .await
                    .unwrap();
            }
            // Project everything a second time — idempotence.
            for i in order {
                projector
                    .project_doc(&format!("kb:{}", nodes[i].0))
                    .await
                    .unwrap();
            }

            let store = stores.store_for("kb1").await.unwrap();
            let mut rows: Vec<(String, String, String)> = store
                .list_ids(None)
                .unwrap()
                .into_iter()
                .map(|id| {
                    let n = store.get_node(&id).unwrap().unwrap();
                    (n.id, n.title, n.body)
                })
                .collect();
            rows.sort();
            // Links must not accumulate on re-projection either.
            let links = store.links_from("concept:a").unwrap();
            assert_eq!(
                links.len(),
                1,
                "re-projecting must replace links, not append: {links:?}"
            );
            snapshots.push(rows);
        }

        assert_eq!(
            snapshots[0], snapshots[1],
            "apply order changed the projection — determinism contract broken"
        );
        assert_eq!(snapshots[0].len(), 3);
    }

    /// GATE C.3 + C.4 — reconciliation reports zero drift on a healthy projection, and
    /// an INJECTED mismatch is both detected and healed.
    ///
    /// The injection is the part that matters. A verification pass that has never caught
    /// anything is indistinguishable from one that cannot: asserting "reports clean" on a
    /// projection the same code just built proves nothing on its own. So each drift
    /// class is forced deliberately and must be seen.
    #[tokio::test]
    async fn reconciliation_detects_and_heals_every_drift_class() {
        let doc_store = mem_store();
        let stores = MemStores::new();
        let projector = Projector::new(Arc::clone(&doc_store), stores.clone());

        seed(
            &doc_store,
            "kb1",
            &[
                ("concept:a", "A", "a body"),
                ("concept:b", "B", "b body"),
                ("concept:c", "C", "c body"),
            ],
        )
        .await;
        projector.project_doc("kbc:kb1").await.unwrap();

        // Baseline: a freshly built projection has no drift.
        let clean = projector.reconcile_kb("kb1", false).await.unwrap();
        assert!(
            clean.is_clean(),
            "healthy projection reported drift: {clean:?}"
        );

        // Inject all three classes directly into the store, behind the projector's back —
        // exactly what a dropped emit, a failed write or a crash would leave.
        let store = stores.store_for("kb1").await.unwrap();
        store.delete_node("concept:a").unwrap(); // missing
        let mut tampered = store.get_node("concept:b").unwrap().unwrap();
        tampered.body = "TAMPERED — not what the CRDT says".to_string();
        store.insert_node(&tampered).unwrap(); // differing
        let ghost = mae_kb::Node::new("concept:ghost", "Ghost", NodeKind::Note, "not in the CRDT");
        store.insert_node(&ghost).unwrap(); // extra

        // Detection, without healing.
        let drift = projector.reconcile_kb("kb1", false).await.unwrap();
        assert_eq!(drift.missing, vec!["concept:a".to_string()], "{drift:?}");
        assert_eq!(drift.differing, vec!["concept:b".to_string()], "{drift:?}");
        assert_eq!(drift.extra, vec!["concept:ghost".to_string()], "{drift:?}");
        assert_eq!(drift.total(), 3);

        // Still present after a detect-only pass — reporting must not mutate.
        assert!(store.get_node("concept:ghost").unwrap().is_some());

        // Heal, then verify against CRDT truth rather than against the report.
        let healed = projector.reconcile_kb("kb1", true).await.unwrap();
        assert_eq!(healed.total(), 3, "the healing pass reports what it fixed");
        assert_eq!(
            store.get_node("concept:a").unwrap().unwrap().body,
            "a body",
            "missing node restored from CRDT truth"
        );
        assert_eq!(
            store.get_node("concept:b").unwrap().unwrap().body,
            "b body",
            "tampered node restored from CRDT truth"
        );
        assert!(
            store.get_node("concept:ghost").unwrap().is_none(),
            "node absent from CRDT truth removed from the projection"
        );

        // Idempotent: a second pass finds nothing left to do.
        let after = projector.reconcile_kb("kb1", true).await.unwrap();
        assert!(
            after.is_clean(),
            "reconciliation is not idempotent — second pass still reports {after:?}"
        );
    }
}
