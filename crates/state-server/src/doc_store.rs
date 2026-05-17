//! Document store — per-document locking with WAL-first persistence.
//!
//! `DocStore` manages in-memory CRDT documents backed by a storage backend.
//! The outer `RwLock` protects the map (read to find, write to create/evict).
//! Each document has its own `Mutex` for concurrent access to different docs.

use std::collections::HashMap;
use std::sync::Arc;

use mae_sync::encoding::validate_update;
use mae_sync::text::TextSync;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, warn};

use crate::storage::{StorageBackend, StorageError};

/// Per-document state.
struct DocEntry {
    sync: TextSync,
    /// Last WAL sequence ID applied.
    wal_seq: u64,
    /// Updates since last compaction.
    update_count: u64,
}

/// Thread-safe document store with per-document locking.
pub struct DocStore {
    docs: RwLock<HashMap<String, Arc<Mutex<DocEntry>>>>,
    storage: Arc<dyn StorageBackend>,
    compact_threshold: u64,
}

/// Result of applying an update.
pub struct ApplyResult {
    /// The update bytes to broadcast to other clients.
    pub update: Vec<u8>,
    /// The WAL sequence ID assigned to this update.
    pub wal_seq: u64,
}

impl DocStore {
    pub fn new(storage: Arc<dyn StorageBackend>, compact_threshold: u64) -> Self {
        DocStore {
            docs: RwLock::new(HashMap::new()),
            storage,
            compact_threshold,
        }
    }

    /// Get or create a document. Loads from storage if not in memory.
    async fn get_or_create(&self, doc_name: &str) -> Result<Arc<Mutex<DocEntry>>, StorageError> {
        // Fast path: read lock.
        {
            let docs = self.docs.read().await;
            if let Some(entry) = docs.get(doc_name) {
                return Ok(Arc::clone(entry));
            }
        }

        // Slow path: write lock + load from storage.
        let mut docs = self.docs.write().await;
        // Double-check after acquiring write lock.
        if let Some(entry) = docs.get(doc_name) {
            return Ok(Arc::clone(entry));
        }

        let (sync, wal_seq) = match self.storage.load_document(doc_name).await? {
            Some(state) => {
                let mut sync = if let Some(snapshot) = state.snapshot {
                    TextSync::from_state(&snapshot)
                        .map_err(|e| StorageError::Sqlite(format!("bad snapshot: {e}")))?
                } else {
                    TextSync::new("")
                };

                let mut last_id = 0u64;
                for entry in &state.wal_tail {
                    sync.apply_update(&entry.update)
                        .map_err(|e| StorageError::Sqlite(format!("WAL replay: {e}")))?;
                    last_id = entry.id;
                }

                info!(
                    doc = doc_name,
                    wal_entries = state.wal_tail.len(),
                    "recovered document from storage"
                );
                (sync, last_id)
            }
            None => {
                debug!(doc = doc_name, "new document created");
                (TextSync::new(""), 0)
            }
        };

        let entry = Arc::new(Mutex::new(DocEntry {
            sync,
            wal_seq,
            update_count: 0,
        }));
        docs.insert(doc_name.to_string(), Arc::clone(&entry));
        Ok(entry)
    }

    /// Apply an update to a document: validate -> WAL append -> apply in memory.
    /// Returns the update bytes for broadcasting.
    pub async fn apply_update(
        &self,
        doc_name: &str,
        update: &[u8],
        client_id: Option<u64>,
    ) -> Result<ApplyResult, StorageError> {
        // Validate before touching storage.
        validate_update(update)
            .map_err(|e| StorageError::Sqlite(format!("invalid update: {e}")))?;

        // WAL append first (durability).
        let wal_id = self.storage.wal_append(doc_name, update, client_id).await?;

        // Apply to in-memory document.
        let entry = self.get_or_create(doc_name).await?;
        let should_compact = {
            let mut doc = entry.lock().await;
            doc.sync
                .apply_update(update)
                .map_err(|e| StorageError::Sqlite(format!("apply failed: {e}")))?;
            doc.wal_seq = wal_id;
            doc.update_count += 1;
            doc.update_count >= self.compact_threshold
        };

        if should_compact {
            self.compact(doc_name).await?;
        }

        Ok(ApplyResult {
            update: update.to_vec(),
            wal_seq: wal_id,
        })
    }

    /// Get the state vector for a document (for sync protocol).
    pub async fn state_vector(&self, doc_name: &str) -> Result<Vec<u8>, StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        Ok(doc.sync.state_vector())
    }

    /// Encode the full state for a document (for new client sync).
    pub async fn encode_state(&self, doc_name: &str) -> Result<Vec<u8>, StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        Ok(doc.sync.encode_state())
    }

    /// Get text content of a document.
    pub async fn content(&self, doc_name: &str) -> Result<String, StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        Ok(doc.sync.content())
    }

    /// Compact a document: snapshot + WAL trim.
    async fn compact(&self, doc_name: &str) -> Result<(), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let (state, wal_seq) = {
            let mut doc = entry.lock().await;
            let state = doc.sync.encode_state();
            let seq = doc.wal_seq;
            doc.update_count = 0;
            (state, seq)
        };
        self.storage.compact(doc_name, &state, wal_seq).await?;
        Ok(())
    }

    /// Compact all documents (e.g. on shutdown).
    pub async fn compact_all(&self) -> Result<(), StorageError> {
        let names: Vec<String> = {
            let docs = self.docs.read().await;
            docs.keys().cloned().collect()
        };
        for name in names {
            if let Err(e) = self.compact(&name).await {
                warn!(doc = %name, error = %e, "compaction failed on shutdown");
            }
        }
        Ok(())
    }

    /// List all in-memory documents.
    pub async fn document_names(&self) -> Vec<String> {
        let docs = self.docs.read().await;
        docs.keys().cloned().collect()
    }

    /// Number of documents currently in memory.
    #[allow(dead_code)]
    pub async fn document_count(&self) -> usize {
        let docs = self.docs.read().await;
        docs.len()
    }

    /// Compute a diff from a given state vector (for reconnect protocol).
    pub async fn encode_diff(
        &self,
        doc_name: &str,
        remote_sv: &[u8],
    ) -> Result<Vec<u8>, StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        mae_sync::encoding::encode_diff(doc.sync.doc(), remote_sv)
            .map_err(|e| StorageError::Sqlite(format!("diff encoding: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteBackend;
    use mae_sync::text::TextSync;

    fn test_store() -> DocStore {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        DocStore::new(backend, 500)
    }

    #[tokio::test]
    async fn apply_and_read() {
        let store = test_store();

        // Generate a valid yrs update.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello world");

        let result = store.apply_update("doc1", &update, Some(1)).await.unwrap();
        assert!(result.wal_seq > 0);

        let content = store.content("doc1").await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn state_vector_and_diff() {
        let store = test_store();

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        let sv = store.state_vector("doc1").await.unwrap();
        assert!(!sv.is_empty());

        // A new client with empty state vector gets the full diff.
        let empty_sv = TextSync::new("").state_vector();
        let diff = store.encode_diff("doc1", &empty_sv).await.unwrap();
        assert!(!diff.is_empty());
    }

    #[tokio::test]
    async fn invalid_update_rejected() {
        let store = test_store();
        let result = store.apply_update("doc1", b"garbage", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn concurrent_docs() {
        let store = test_store();

        let mut ts1 = TextSync::with_client_id("", 1);
        let mut ts2 = TextSync::with_client_id("", 2);
        let u1 = ts1.insert(0, "doc1");
        let u2 = ts2.insert(0, "doc2");

        store.apply_update("a", &u1, Some(1)).await.unwrap();
        store.apply_update("b", &u2, Some(2)).await.unwrap();

        assert_eq!(store.content("a").await.unwrap(), "doc1");
        assert_eq!(store.content("b").await.unwrap(), "doc2");
        assert_eq!(store.document_count().await, 2);
    }

    #[tokio::test]
    async fn compaction_on_threshold() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend.clone(), 3); // compact every 3

        let mut ts = TextSync::with_client_id("", 1);
        for i in 0..5 {
            let update = ts.insert(i, "x");
            store.apply_update("doc1", &update, Some(1)).await.unwrap();
        }

        // After 5 updates with threshold 3, compaction should have run.
        let state = backend.load_document("doc1").await.unwrap().unwrap();
        // Snapshot should exist after compaction.
        assert!(state.snapshot.is_some());
    }

    #[tokio::test]
    async fn compact_all_on_shutdown() {
        let store = test_store();
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "persist me");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        store.compact_all().await.unwrap();
        // No error — success.
    }

    #[tokio::test]
    async fn document_names() {
        let store = test_store();
        let mut ts = TextSync::with_client_id("", 1);
        let u1 = ts.insert(0, "a");
        store.apply_update("alpha", &u1, None).await.unwrap();
        store.apply_update("beta", &u1, None).await.unwrap();

        let mut names = store.document_names().await;
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
