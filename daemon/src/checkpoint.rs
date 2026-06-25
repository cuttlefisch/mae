//! KB checkpoint — a content-hashed capture of a KB's full CRDT state (ADR-032 A3).
//!
//! A KB is one `kbc:{kb_id}` collection doc plus N `kb:{node_id}` node docs. A
//! checkpoint captures the collection state plus every node doc the collection's
//! manifest references, with a deterministic content hash over the whole set. It is
//! the trusted **rebuild root** for the cozo projection (ADR-029) and the unit of
//! backup/restore (ADR-032 A4).
//!
//! Consistency: the node list is derived from the *captured* collection state (not a
//! live re-read), so the checkpoint contains exactly the collection plus its listed
//! nodes — a self-consistent set. A globally-atomic instant across docs is neither
//! achievable nor meaningful for a distributed CRDT; each doc is captured at a valid
//! state, and any combination of valid per-doc states is itself a valid, mergeable KB
//! state to restore or rebuild from.

use sha2::{Digest, Sha256};

use crate::doc_store::DocStore;

/// A captured, content-hashed snapshot of a KB's full CRDT state.
#[derive(Debug, Clone)]
pub struct KbCheckpoint {
    pub kb_id: String,
    /// Full yrs state of the `kbc:{kb_id}` collection doc.
    pub collection_state: Vec<u8>,
    /// `(node_id, full yrs state)` for every node in the collection manifest,
    /// sorted by `node_id` for determinism.
    pub nodes: Vec<(String, Vec<u8>)>,
    /// SHA-256 over the canonical serialization of the capture (stable across runs
    /// for identical content). The integrity commitment + restore/rebuild identity.
    pub content_hash: String,
}

impl KbCheckpoint {
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

/// Capture a content-hashed checkpoint of `kb_id` from the doc_store.
pub async fn checkpoint_kb(doc_store: &DocStore, kb_id: &str) -> Result<KbCheckpoint, String> {
    let collection_doc = format!("kbc:{kb_id}");
    let (collection_state, _sv) = doc_store
        .encode_state_and_sv(&collection_doc)
        .await
        .map_err(|e| format!("load collection '{kb_id}': {e}"))?;
    let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&collection_state)
        .map_err(|e| format!("parse collection '{kb_id}': {e}"))?;

    // Derive the node set from the CAPTURED collection (self-consistent).
    let mut node_ids: Vec<String> = coll.list_nodes().into_iter().map(|(id, _)| id).collect();
    node_ids.sort();

    let mut nodes = Vec::with_capacity(node_ids.len());
    for node_id in &node_ids {
        let (state, _sv) = doc_store
            .encode_state_and_sv(&format!("kb:{node_id}"))
            .await
            .map_err(|e| format!("load node '{node_id}': {e}"))?;
        nodes.push((node_id.clone(), state));
    }

    let content_hash = checkpoint_hash(kb_id, &collection_state, &nodes);
    Ok(KbCheckpoint {
        kb_id: kb_id.to_string(),
        collection_state,
        nodes,
        content_hash,
    })
}

/// Deterministic content hash over the capture: length-prefixed kb_id, collection
/// state, then each `(node_id, state)` in the order given (callers pass sorted nodes).
fn checkpoint_hash(kb_id: &str, collection_state: &[u8], nodes: &[(String, Vec<u8>)]) -> String {
    let mut h = Sha256::new();
    let field = |bytes: &[u8], hasher: &mut Sha256| {
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(bytes);
    };
    field(kb_id.as_bytes(), &mut h);
    field(collection_state, &mut h);
    h.update((nodes.len() as u64).to_le_bytes());
    for (id, state) in nodes {
        field(id.as_bytes(), &mut h);
        field(state, &mut h);
    }
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteBackend;
    use std::sync::Arc;

    async fn seed_kb(store: &DocStore) {
        let mut coll = mae_sync::kb::KbCollectionDoc::new("kb1", "owner");
        coll.add_node("n1", "Node 1");
        coll.add_node("n2", "Node 2");
        store.share_doc("kbc:kb1", &coll.encode_state()).await.unwrap();

        let n1 = mae_sync::kb::KbNodeDoc::new("n1", "Node 1", "body one", &[]);
        store.share_doc("kb:n1", &n1.encode()).await.unwrap();
        let n2 = mae_sync::kb::KbNodeDoc::new("n2", "Node 2", "body two", &[]);
        store.share_doc("kb:n2", &n2.encode()).await.unwrap();
    }

    #[tokio::test]
    async fn checkpoint_captures_collection_and_nodes() {
        let store = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
        seed_kb(&store).await;

        let cp = checkpoint_kb(&store, "kb1").await.unwrap();
        assert_eq!(cp.kb_id, "kb1");
        assert_eq!(cp.node_count(), 2);
        assert_eq!(cp.nodes[0].0, "n1");
        assert_eq!(cp.nodes[1].0, "n2");
        assert!(!cp.collection_state.is_empty());
        assert!(!cp.content_hash.is_empty());
    }

    #[tokio::test]
    async fn checkpoint_hash_is_deterministic_and_content_sensitive() {
        let store = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
        seed_kb(&store).await;

        let a = checkpoint_kb(&store, "kb1").await.unwrap();
        let b = checkpoint_kb(&store, "kb1").await.unwrap();
        assert_eq!(a.content_hash, b.content_hash, "same content ⇒ same hash");

        // A node-content change moves the hash.
        let n1 = mae_sync::kb::KbNodeDoc::new("n1", "Node 1", "EDITED", &[]);
        store.apply_update("kb:n1", &n1.encode(), None).await.unwrap();
        let c = checkpoint_kb(&store, "kb1").await.unwrap();
        assert_ne!(a.content_hash, c.content_hash, "edited content ⇒ different hash");
    }
}
