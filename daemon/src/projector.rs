//! KB projector — deterministic projection of CRDT node docs into the CozoDB query
//! store (ADR-029 / ADR-030).
//!
//! The CRDT (`KbNodeDoc`) is the source of truth; CozoDB is a derived projection. The
//! **structural projection is a pure function of the CRDT state**: parse a node's
//! source text → a cozo node + its links + FTS. Because the parse is
//! deterministic, every peer with the same converged CRDT derives a byte-identical
//! graph (the ADR-029 determinism contract). This is the seam the change feed
//! (`doc_store.apply_update`) drives, covering hub + p2p uniformly.

use std::sync::Arc;

use mae_kb::{CozoKbStore, KbStore, Node, NodeKind, NodeSource};
use tokio::sync::mpsc;

use crate::doc_store::DocStore;

/// Drives the cozo projection from the doc_store change feed (ADR-029 B2). It reads a
/// changed KB doc's current CRDT state from the doc_store and materializes it into the
/// cozo query store. One mechanism for hub + p2p — both land at `doc_store.apply_update`,
/// which emits to the feed.
pub struct Projector {
    doc_store: Arc<DocStore>,
    cozo: Arc<CozoKbStore>,
}

impl Projector {
    pub fn new(doc_store: Arc<DocStore>, cozo: Arc<CozoKbStore>) -> Self {
        Self { doc_store, cozo }
    }

    /// Project one changed doc: read its current CRDT state from the doc_store and
    /// write it into cozo. Node docs (`kb:`) materialize the node; collection docs
    /// (`kbc:`) are handled in B3/B4. Reading the state outside the doc lock (the
    /// channel decouples) keeps the projection off the write path.
    pub async fn project_doc(&self, doc_name: &str) -> Result<(), String> {
        if let Some(node_id) = doc_name.strip_prefix("kb:") {
            let (state, _sv) = self
                .doc_store
                .encode_state_and_sv(doc_name)
                .await
                .map_err(|e| format!("read '{doc_name}': {e}"))?;
            project_node(&self.cozo, node_id, &state)
        } else {
            // `kbc:` collection projection (manifest + node deletions) lands in B3/B4.
            Ok(())
        }
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
/// into the cozo query store: materialize the node (title/body/tags/kind) + its body
/// links + FTS via `insert_node` (which parses `[[…]]` links and maintains the index).
/// Idempotent: re-projecting the same state upserts the same node and replaces its
/// links. Typed-link metadata (rel_type/weight/confidence from an extended in-text
/// grammar) lands with ADR-030 in Phase C; B1 is the structural backbone.
pub fn project_node(store: &CozoKbStore, node_id: &str, state: &[u8]) -> Result<(), String> {
    let doc = mae_sync::kb::KbNodeDoc::from_bytes(state)
        .map_err(|e| format!("parse node doc '{node_id}': {e}"))?;
    // Kind is a cozo-only projection field (the CRDT carries only content); derive it
    // deterministically from the id namespace.
    let node = Node::from_crdt_doc(&doc, kind_from_id(node_id), NodeSource::Federation);
    store
        .insert_node(&node)
        .map_err(|e| format!("project node '{node_id}': {e}"))?;
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

    #[test]
    fn project_node_materializes_node_links_and_fts() {
        let store = CozoKbStore::open_mem().unwrap();
        let doc = mae_sync::kb::KbNodeDoc::new(
            "concept:rope",
            "Rope",
            "The rope buffer structure. See [[concept:buffer]] for usage.",
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

        // The body link is projected into the graph.
        let links = store.links_from("concept:rope").unwrap();
        assert!(
            links.iter().any(|l| l.dst == "concept:buffer"),
            "the body link should be projected into the graph, got: {links:?}"
        );
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
    async fn change_feed_drives_node_projection() {
        // ADR-029 B2: a doc_store mutation on a KB node emits to the change feed; the
        // Projector reads the new state and materializes it into cozo (the live loop
        // that fixes the stale-daemon-store bug). An ephemeral doc emits nothing.
        use crate::storage::SqliteBackend;

        let doc_store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let (tx, mut rx) = mpsc::unbounded_channel();
        doc_store.set_change_feed(tx);
        let cozo = Arc::new(CozoKbStore::open_mem().unwrap());

        // Mutate a KB node doc + an ephemeral doc.
        let node = mae_sync::kb::KbNodeDoc::new("concept:x", "X", "body [[concept:y]]", &[]);
        doc_store
            .apply_update("kb:concept:x", &node.encode(), None)
            .await
            .unwrap();
        let scratch = mae_sync::kb::KbNodeDoc::new("s", "S", "scratch", &[]);
        doc_store
            .apply_update("scratch:buf", &scratch.encode(), None)
            .await
            .unwrap();

        // Only the durable KB doc is on the feed.
        let changed = rx.recv().await.unwrap();
        assert_eq!(changed, "kb:concept:x");
        assert!(
            rx.try_recv().is_err(),
            "ephemeral docs must not emit changes"
        );

        // The projector materializes it into cozo.
        let projector = Projector::new(Arc::clone(&doc_store), Arc::clone(&cozo));
        projector.project_doc(&changed).await.unwrap();
        let n = cozo.get_node("concept:x").unwrap().unwrap();
        assert_eq!(n.title, "X");
        assert!(cozo
            .links_from("concept:x")
            .unwrap()
            .iter()
            .any(|l| l.dst == "concept:y"));
    }
}
