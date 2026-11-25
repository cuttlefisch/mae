//! `DocStore`'s side of publication review (ADR-113): the review tables'
//! transactions, and the freeze of one node for a request (D2, A2, O8).
//!
//! A child module of `doc_store` because the freeze must take the same locks
//! every write takes, in the same order: the document's replace gate
//! (shared), then its entry lock, then the store's writer. Holding the entry
//! lock across the content read, the version check, the high-water read and
//! the history cut is what makes the frozen content, its WAL cutoff and the
//! history version it references describe one and the same state: no edit,
//! share or restore of this document can land between them.
//!
//! @ai-caution: [sync] Never read the content and the high-water mark under
//! two separate acquisitions of the entry lock. An edit between them pairs old
//! content with a later cutoff, and its author drops out of the next
//! revision's author window (A2). `review_freeze_race_tests` drives each
//! interleaving.

use std::any::Any;

use mae_sync::kb::history::{encode_content, VersionKind};
use mae_sync::kb::{KbNodeDoc, MaterializedNode};
use sha2::{Digest, Sha256};

use super::DocStore;
use crate::storage::{Checkpoint, StorageError};

/// A node frozen for a review request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frozen {
    pub node: MaterializedNode,
    /// The canonical encoding of `node` (ADR-116's `maenode/v1`): the
    /// revision's own copy of its source (O8).
    pub content: Vec<u8>,
    /// SHA-256 of `content`, lowercase hex, unkeyed (D2).
    pub source_sha256: String,
    /// `KbNodeDoc::version` of the frozen node: the version the requester saw.
    pub version: String,
    /// The document's WAL high-water mark when it was frozen (A2).
    pub cutoff_wal: u64,
    /// The `review_freeze` history version cut at the freeze (ADR-116 D2.3).
    pub history_seq: u64,
}

#[derive(Debug)]
pub enum FreezeError {
    /// The working copy is no longer at the requested version (D2).
    Stale {
        current: String,
    },
    /// The document does not read as this node (E2E ciphertext, or absent).
    Unreadable(String),
    Storage(StorageError),
}

impl From<StorageError> for FreezeError {
    fn from(e: StorageError) -> Self {
        FreezeError::Storage(e)
    }
}

impl DocStore {
    /// The daemon clock, unix milliseconds (injectable in tests).
    pub fn now_ms(&self) -> u64 {
        (self.clock)()
    }

    /// Run `op` over the review tables in one write transaction, as the
    /// store's one writer. `op`'s error rolls it back.
    pub async fn review_write<T: Send + 'static>(
        &self,
        op: impl FnOnce(&sqlite::Connection) -> Result<T, StorageError> + Send + 'static,
    ) -> Result<T, StorageError> {
        self.review_txn(true, op).await
    }

    /// Run `op` over the review tables in one read snapshot.
    pub async fn review_read<T: Send + 'static>(
        &self,
        op: impl FnOnce(&sqlite::Connection) -> Result<T, StorageError> + Send + 'static,
    ) -> Result<T, StorageError> {
        self.review_txn(false, op).await
    }

    async fn review_txn<T: Send + 'static>(
        &self,
        write: bool,
        op: impl FnOnce(&sqlite::Connection) -> Result<T, StorageError> + Send + 'static,
    ) -> Result<T, StorageError> {
        let out = self
            .storage
            .review_txn(
                write,
                Box::new(move |c| op(c).map(|t| Box::new(t) as Box<dyn Any + Send>)),
            )
            .await?;
        out.downcast::<T>()
            .map(|b| *b)
            .map_err(|_| StorageError::Sqlite("review: a transaction returned another type".into()))
    }

    /// Freeze KB node `node_id` at `version` (ADR-113 D2): its content, the
    /// WAL cutoff that content stands at (A2), and an ADR-116 `review_freeze`
    /// version of it (O8), all under the document's locks. Refused `Stale`
    /// when the working copy has moved on, so a requester freezes what they
    /// saw.
    pub async fn review_freeze(
        &self,
        kb_id: &str,
        node_id: &str,
        version: &str,
    ) -> Result<Frozen, FreezeError> {
        let doc_name = mae_sync::kb_node_doc_name(kb_id, node_id);
        if !self.has_durable_doc(&doc_name).await {
            return Err(FreezeError::Unreadable(format!(
                "{doc_name} has no content"
            )));
        }
        let _gate = self.replace_gate(&doc_name).read().await;
        let entry = self.get_or_create(&doc_name).await?;
        let doc = entry.lock().await;
        let node = KbNodeDoc::from_bytes(&doc.sync.encode_state())
            .map(|n| n.materialize())
            .ok()
            .filter(|n| n.id == node_id)
            .ok_or_else(|| {
                FreezeError::Unreadable(format!("{doc_name} does not read as node {node_id}"))
            })?;
        let current = hex::encode(&node.digest()[..16]);
        if current != version {
            return Err(FreezeError::Stale { current });
        }
        #[cfg(test)]
        Self::pause_at(&self.pauses.freeze).await;
        let cutoff_wal = self.storage.wal_high_water(&doc_name).await?;
        let cp = Checkpoint {
            kb_id: kb_id.to_string(),
            node_id: node_id.to_string(),
            kind: VersionKind::ReviewFreeze,
            content: node.clone(),
            name: None,
            summary: None,
            restored_from: None,
        };
        let now = ((self.clock)() / 1000) as i64;
        let cut = self.storage.history_cut(cp, now).await?.ok_or_else(|| {
            FreezeError::Unreadable(format!("KB '{kb_id}' keeps no version history (E2E)"))
        })?;
        // The version must cover exactly the WAL the content stands at.
        if cut.wal_to != cutoff_wal {
            return Err(FreezeError::Storage(StorageError::Sqlite(format!(
                "review freeze of {doc_name}: history cut at WAL {} but content at {cutoff_wal}",
                cut.wal_to
            ))));
        }
        drop(doc);
        let content = encode_content(&node);
        Ok(Frozen {
            source_sha256: hex::encode(Sha256::digest(&content)),
            content,
            node,
            version: current,
            cutoff_wal,
            history_seq: cut.seq,
        })
    }
}
