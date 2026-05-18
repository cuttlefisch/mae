//! Document store — per-document locking with WAL-first persistence.
//!
//! `DocStore` manages in-memory CRDT documents backed by a storage backend.
//! The outer `RwLock` protects the map (read to find, write to create/evict).
//! Each document has its own `Mutex` for concurrent access to different docs.

use std::collections::HashMap;
use std::sync::Arc;

use mae_sync::encoding::validate_update;
use mae_sync::text::TextSync;
use sha2::{Digest, Sha256};
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
    /// Timestamp of last activity (update/read).
    last_activity: std::time::Instant,
    /// Number of clients currently connected to this document.
    connected_clients: u32,
}

/// Statistics for a single document.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocStats {
    pub wal_seq: u64,
    pub update_count: u64,
    pub content_length: usize,
    pub idle_secs: u64,
    pub connected_clients: u32,
}

/// Result of a save intent check.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status")]
pub enum SaveIntentResult {
    #[serde(rename = "ok")]
    Ok { server_hash: String },
    #[serde(rename = "conflict")]
    Conflict { server_hash: String },
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
            last_activity: std::time::Instant::now(),
            connected_clients: 0,
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
            doc.last_activity = std::time::Instant::now();
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
        self.compact_doc(doc_name).await
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

    /// Compute SHA-256 content hash for a document.
    pub async fn content_hash(&self, doc_name: &str) -> Result<String, StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        let content = doc.sync.content();
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        Ok(format!("{:x}", hasher.finalize()))
    }

    /// Check if a client's expected hash matches the server's current content hash.
    /// Used before a save-to-disk operation to prevent overwriting concurrent edits.
    pub async fn check_save_intent(
        &self,
        doc_name: &str,
        expected_hash: &str,
    ) -> Result<SaveIntentResult, StorageError> {
        let server_hash = self.content_hash(doc_name).await?;
        if server_hash == expected_hash {
            Ok(SaveIntentResult::Ok { server_hash })
        } else {
            Ok(SaveIntentResult::Conflict { server_hash })
        }
    }

    /// Get statistics for a document.
    pub async fn doc_stats(&self, doc_name: &str) -> Result<DocStats, StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        Ok(DocStats {
            wal_seq: doc.wal_seq,
            update_count: doc.update_count,
            content_length: doc.sync.content().len(),
            idle_secs: doc.last_activity.elapsed().as_secs(),
            connected_clients: doc.connected_clients,
        })
    }

    /// Track a client connecting to a document.
    #[allow(dead_code)]
    pub async fn track_client_connect(&self, doc_name: &str) -> Result<(), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let mut doc = entry.lock().await;
        doc.connected_clients += 1;
        doc.last_activity = std::time::Instant::now();
        Ok(())
    }

    /// Track a client disconnecting from a document.
    #[allow(dead_code)]
    pub async fn track_client_disconnect(&self, doc_name: &str) -> Result<(), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let mut doc = entry.lock().await;
        doc.connected_clients = doc.connected_clients.saturating_sub(1);
        Ok(())
    }

    /// Evict idle documents with no connected clients.
    /// Returns the names of evicted documents.
    pub async fn evict_idle(&self, max_idle_secs: u64) -> Vec<String> {
        let mut to_evict = Vec::new();

        // First pass: identify candidates (read lock).
        {
            let docs = self.docs.read().await;
            for (name, entry) in docs.iter() {
                let doc = entry.lock().await;
                if doc.connected_clients == 0
                    && doc.last_activity.elapsed().as_secs() >= max_idle_secs
                {
                    to_evict.push(name.clone());
                }
            }
        }

        if to_evict.is_empty() {
            return Vec::new();
        }

        // Compact before eviction, then remove.
        for name in &to_evict {
            if let Err(e) = self.compact_doc(name).await {
                warn!(doc = %name, error = %e, "compaction before eviction failed");
            }
        }

        let mut docs = self.docs.write().await;
        let mut evicted = Vec::new();
        for name in &to_evict {
            // Re-check under write lock — a client may have connected.
            if let Some(entry) = docs.get(name) {
                let doc = entry.lock().await;
                if doc.connected_clients == 0
                    && doc.last_activity.elapsed().as_secs() >= max_idle_secs
                {
                    drop(doc);
                    docs.remove(name);
                    evicted.push(name.clone());
                }
            }
        }

        if !evicted.is_empty() {
            info!(count = evicted.len(), "evicted idle documents");
        }

        evicted
    }

    /// Compact a single document (public interface for background tasks).
    pub async fn compact_doc(&self, doc_name: &str) -> Result<(), StorageError> {
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
