//! `KbCollectionDoc`: ADR-034 per-KB derived-artifact-sharing settings —
//! pinned `embedding_model`/`chunk_version` and the `share_derived_artifacts`
//! opt-in toggle. The sharing *coordinator* is deliberately not a field here
//! at all: it's `current_lease("enrichment", now).holder_fp` (ADR-033,
//! `collection_lease.rs`) — ADR-034's own text says to reuse ADR-033's
//! election/tiebreak machinery for this, not build a second one.

use yrs::{Map, Transact};

use super::*;

impl KbCollectionDoc {
    /// The pinned embedding model name, if set. `None` ⇒ no pin recorded yet
    /// (a peer with no pin can't yet judge artifact interchangeability and
    /// should recompute locally rather than guess).
    pub fn embedding_model(&self) -> Option<String> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_EMBEDDING_MODEL_KEY)
            .map(|v| v.to_string(&txn))
            .filter(|s| !s.is_empty())
    }

    /// Pin the embedding model. Returns the encoded update.
    pub fn set_embedding_model(&mut self, model: &str) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_EMBEDDING_MODEL_KEY, model);
        txn.encode_update_v1()
    }

    /// The pinned chunk version (ADR-031's cache-key third component). 0 if
    /// never set.
    pub fn chunk_version(&self) -> i64 {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_CHUNK_VERSION_KEY)
            .map(|v| v.to_string(&txn))
            .and_then(|s| s.parse::<i64>().ok())
            .unwrap_or(0)
    }

    /// Pin the chunk version. Returns the encoded update.
    pub fn set_chunk_version(&mut self, chunk_version: i64) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(&mut txn, COLL_CHUNK_VERSION_KEY, chunk_version.to_string());
        txn.encode_update_v1()
    }

    /// Whether this KB's members serve each other cached embedding vectors
    /// peer-to-peer (ADR-034). Absent ⇒ `false` — opt-in, matching this
    /// codebase's own convention for every other new-capability toggle
    /// (`TransportPolicy`, `Encryption`: existing behavior preserved until a
    /// member explicitly opts in).
    pub fn share_derived_artifacts(&self) -> bool {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let txn = self.doc.transact();
        root.get(&txn, COLL_SHARE_ARTIFACTS_KEY)
            .map(|v| v.to_string(&txn))
            .is_some_and(|s| s == "1")
    }

    /// Set the `share_derived_artifacts` toggle. Returns the encoded update.
    pub fn set_share_derived_artifacts(&mut self, enabled: bool) -> Vec<u8> {
        let root = self.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = self.doc.transact_mut();
        root.insert(
            &mut txn,
            COLL_SHARE_ARTIFACTS_KEY,
            if enabled { "1" } else { "0" },
        );
        txn.encode_update_v1()
    }
}
