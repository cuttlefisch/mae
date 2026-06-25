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

/// Magic + version header for the portable checkpoint artifact.
const CHECKPOINT_MAGIC: &[u8] = b"MAEKB1\n";

impl KbCheckpoint {
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Serialize to a portable, self-describing artifact (length-prefixed binary).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(CHECKPOINT_MAGIC);
        write_bytes(&mut out, self.kb_id.as_bytes());
        write_bytes(&mut out, self.content_hash.as_bytes());
        write_bytes(&mut out, &self.collection_state);
        out.extend_from_slice(&(self.nodes.len() as u64).to_le_bytes());
        for (id, state) in &self.nodes {
            write_bytes(&mut out, id.as_bytes());
            write_bytes(&mut out, state);
        }
        out
    }

    /// Parse an artifact and **verify integrity** (recomputed hash must match the
    /// stored one — ADR-032 A5). Errors on bad magic, truncation, or hash mismatch.
    pub fn from_bytes(buf: &[u8]) -> Result<KbCheckpoint, String> {
        let mut r = Reader::new(buf);
        if r.take(CHECKPOINT_MAGIC.len())? != CHECKPOINT_MAGIC {
            return Err("not a MAE KB checkpoint (bad magic)".to_string());
        }
        let kb_id = r.read_string()?;
        let content_hash = r.read_string()?;
        let collection_state = r.read_bytes()?;
        let n = r.read_u64()? as usize;
        let mut nodes = Vec::with_capacity(n);
        for _ in 0..n {
            let id = r.read_string()?;
            let state = r.read_bytes()?;
            nodes.push((id, state));
        }
        let recomputed = checkpoint_hash(&kb_id, &collection_state, &nodes);
        if recomputed != content_hash {
            return Err(format!(
                "checkpoint integrity check failed: content hash mismatch for '{kb_id}'"
            ));
        }
        Ok(KbCheckpoint {
            kb_id,
            collection_state,
            nodes,
            content_hash,
        })
    }
}

/// Export `kb_id` as a portable, content-hashed checkpoint artifact (backup / migration).
pub async fn export_kb(doc_store: &DocStore, kb_id: &str) -> Result<Vec<u8>, String> {
    Ok(checkpoint_kb(doc_store, kb_id).await?.to_bytes())
}

/// Import (restore) a KB from an artifact: verify integrity, then write the collection +
/// node docs into the doc_store. Restore semantics — replaces `kb_id` if it exists.
pub async fn import_kb(doc_store: &DocStore, artifact: &[u8]) -> Result<KbCheckpoint, String> {
    let cp = KbCheckpoint::from_bytes(artifact)?;
    doc_store
        .share_doc(&format!("kbc:{}", cp.kb_id), &cp.collection_state)
        .await
        .map_err(|e| format!("restore collection '{}': {e}", cp.kb_id))?;
    for (id, state) in &cp.nodes {
        doc_store
            .share_doc(&format!("kb:{id}"), state)
            .await
            .map_err(|e| format!("restore node '{id}': {e}"))?;
    }
    Ok(cp)
}

fn write_bytes(out: &mut Vec<u8>, b: &[u8]) {
    out.extend_from_slice(&(b.len() as u64).to_le_bytes());
    out.extend_from_slice(b);
}

/// Minimal bounds-checked cursor for parsing the checkpoint artifact.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or("checkpoint length overflow")?;
        if end > self.buf.len() {
            return Err("truncated checkpoint artifact".to_string());
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn read_u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn read_bytes(&mut self) -> Result<Vec<u8>, String> {
        let n = self.read_u64()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn read_string(&mut self) -> Result<String, String> {
        String::from_utf8(self.read_bytes()?).map_err(|_| "invalid utf8 in checkpoint".to_string())
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
        store
            .share_doc("kbc:kb1", &coll.encode_state())
            .await
            .unwrap();

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
        store
            .apply_update("kb:n1", &n1.encode(), None)
            .await
            .unwrap();
        let c = checkpoint_kb(&store, "kb1").await.unwrap();
        assert_ne!(
            a.content_hash, c.content_hash,
            "edited content ⇒ different hash"
        );
    }

    #[tokio::test]
    async fn export_import_round_trips_a_kb() {
        // Export from a source store, import into a FRESH store, assert the KB restored.
        let src = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
        seed_kb(&src).await;
        let artifact = export_kb(&src, "kb1").await.unwrap();

        let dst = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
        let cp = import_kb(&dst, &artifact).await.unwrap();
        assert_eq!(cp.node_count(), 2);

        // Collection manifest restored.
        let (coll_state, _) = dst.encode_state_and_sv("kbc:kb1").await.unwrap();
        let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&coll_state).unwrap();
        assert_eq!(coll.list_nodes().len(), 2);

        // Node bodies restored (semantic round-trip — the docs are KbNodeDoc maps,
        // not plain text, so check the structured state, not doc_store.content()).
        let (n1s, _) = dst.encode_state_and_sv("kb:n1").await.unwrap();
        assert_eq!(
            mae_sync::kb::KbNodeDoc::from_bytes(&n1s).unwrap().body(),
            "body one"
        );
        let (n2s, _) = dst.encode_state_and_sv("kb:n2").await.unwrap();
        assert_eq!(
            mae_sync::kb::KbNodeDoc::from_bytes(&n2s).unwrap().body(),
            "body two"
        );
    }

    #[tokio::test]
    async fn corrupt_artifact_fails_integrity() {
        let src = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
        seed_kb(&src).await;
        let mut artifact = export_kb(&src, "kb1").await.unwrap();

        // Flip the last payload byte → recomputed hash won't match (or parse fails).
        let last = artifact.len() - 1;
        artifact[last] ^= 0xff;
        let err = KbCheckpoint::from_bytes(&artifact).unwrap_err();
        assert!(
            err.contains("integrity") || err.contains("truncated") || err.contains("mismatch"),
            "expected an integrity/parse failure, got: {err}"
        );
    }
}
