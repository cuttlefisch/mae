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
    /// Monotonically increasing save epoch. Incremented on each save_intent.
    save_epoch: u64,
    /// User who last saved this document.
    last_saved_by: Option<String>,
    /// Session ID of the client that shared this document (None if loaded from WAL).
    sharer_session_id: Option<u64>,
}

/// Statistics for a single document.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DocStats {
    pub wal_seq: u64,
    pub update_count: u64,
    pub content_length: usize,
    pub idle_secs: u64,
    pub connected_clients: u32,
    pub save_epoch: u64,
    pub last_saved_by: Option<String>,
}

/// Result of a save intent check.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status")]
pub enum SaveIntentResult {
    #[serde(rename = "ok")]
    Ok {
        server_hash: String,
        save_epoch: u64,
    },
    #[serde(rename = "conflict")]
    Conflict { server_hash: String },
}

/// Thread-safe document store with per-document locking.
pub struct DocStore {
    docs: RwLock<HashMap<String, Arc<Mutex<DocEntry>>>>,
    storage: Arc<dyn StorageBackend>,
    compact_threshold: u64,
    /// Maximum number of documents allowed in memory (0 = unlimited).
    max_documents: usize,
    /// Maximum WAL entries before forced compaction (0 = no forced compaction).
    max_wal_entries: u64,
    /// Maximum document size in bytes before warning (0 = unlimited).
    max_document_size_bytes: usize,
    /// KB metadata registry (kb_id → metadata JSON).
    kb_metas: RwLock<HashMap<String, serde_json::Value>>,
    /// The daemon's signing identity (ADR-026), set in key-auth mode. Present ⇒ the
    /// daemon signs membership ops for KBs it owns (`signer.fingerprint() == owner`)
    /// into the op-log; absent (psk/none) ⇒ legacy unsigned membership only.
    signer: std::sync::OnceLock<Arc<mae_mcp::identity::Identity>>,
    /// Per-KB external trust anchor (ADR-026): the owner pubkey a peer verifies the
    /// op-log genesis against, set for a KB JOINED from a relay we don't trust (the
    /// join-ticket node-id, registered by the dialer). When present, `kb_access`
    /// derives membership from the signed op-log instead of the relay-supplied
    /// `member_roles`. Owned KBs need no anchor (the daemon is itself the authority).
    kb_anchors: RwLock<HashMap<String, [u8; 32]>>,
}

/// Result of applying an update.
#[derive(Debug)]
pub struct ApplyResult {
    /// The update bytes to broadcast to other clients.
    pub update: Vec<u8>,
    /// The WAL sequence ID assigned to this update.
    pub wal_seq: u64,
}

/// Whether a document holds **durable KB content** — a KB collection (`kbc:{kb_id}`)
/// or a KB node (`kb:{node_id}`) — versus ephemeral collab-session state.
///
/// Durability follows the KB-content naming contract (ADR-029/032): KB docs are the
/// source of truth and must survive idle eviction. They may be dropped from memory
/// (and lazy-reloaded on next access), but are **never deleted from storage** — so a
/// hosted KB with no connected client is not destroyed. Everything else (e.g. a
/// transient buffer-collab session) keeps the evict-and-delete behavior. A future
/// refinement could make this an explicit per-doc flag (e.g. to distinguish a hosted
/// KB from a transiently-previewed one); the prefix rule matches today's flows.
pub(crate) fn is_durable_doc(name: &str) -> bool {
    name.starts_with("kb:") || name.starts_with("kbc:")
}

/// Pick the least-recently-used **idle** (no connected client) document to memory-evict
/// when the in-memory working set is full (ADR-032 A2). Uses `try_lock`, so a busy doc is
/// skipped (never evict an in-use entry); returns `None` when nothing is evictable. The
/// caller removes only the in-memory entry — the doc stays on disk and lazy-reloads.
fn pick_lru_evictable(docs: &HashMap<String, Arc<Mutex<DocEntry>>>) -> Option<String> {
    let mut victim: Option<(String, std::time::Instant)> = None;
    for (name, entry) in docs.iter() {
        if let Ok(doc) = entry.try_lock() {
            if doc.connected_clients != 0 {
                continue;
            }
            let older = match &victim {
                Some((_, t)) => doc.last_activity < *t,
                None => true,
            };
            if older {
                victim = Some((name.clone(), doc.last_activity));
            }
        }
    }
    victim.map(|(n, _)| n)
}

impl DocStore {
    pub fn new(storage: Arc<dyn StorageBackend>, compact_threshold: u64) -> Self {
        DocStore {
            docs: RwLock::new(HashMap::new()),
            storage,
            compact_threshold,
            max_documents: 0,
            max_wal_entries: 0,
            max_document_size_bytes: 0,
            kb_metas: RwLock::new(HashMap::new()),
            signer: std::sync::OnceLock::new(),
            kb_anchors: RwLock::new(HashMap::new()),
        }
    }

    /// Install the daemon's signing identity (ADR-026). Called once at startup in
    /// key-auth mode; idempotent (a second call is ignored).
    pub fn set_signer(&self, identity: Arc<mae_mcp::identity::Identity>) {
        let _ = self.signer.set(identity);
    }

    /// The daemon's signing identity, if running in key-auth mode.
    pub fn signer(&self) -> Option<&Arc<mae_mcp::identity::Identity>> {
        self.signer.get()
    }

    /// Register the external trust anchor (owner pubkey) for a KB joined from a
    /// relay (ADR-026) — the join-ticket node-id. `kb_access` then derives that KB's
    /// membership from the signed op-log instead of trusting the relay's copy.
    pub async fn set_kb_anchor(&self, kb_id: &str, owner_pubkey: [u8; 32]) {
        self.kb_anchors
            .write()
            .await
            .insert(kb_id.to_string(), owner_pubkey);
    }

    /// The external trust anchor for `kb_id`, if one is registered (joined KBs).
    pub async fn kb_anchor(&self, kb_id: &str) -> Option<[u8; 32]> {
        self.kb_anchors.read().await.get(kb_id).copied()
    }

    /// Set maximum documents allowed in memory. 0 = unlimited.
    pub fn with_max_documents(mut self, max: usize) -> Self {
        self.max_documents = max;
        self
    }

    /// Set maximum WAL entries before forced compaction. 0 = disabled.
    pub fn with_max_wal_entries(mut self, max: u64) -> Self {
        self.max_wal_entries = max;
        self
    }

    /// Set maximum document size (bytes) before warning. 0 = unlimited.
    pub fn with_max_document_size(mut self, max: usize) -> Self {
        self.max_document_size_bytes = max;
        self
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

        // ADR-032 (Phase A2): max_documents bounds the IN-MEMORY working set, not the
        // durable set. When full, memory-evict the least-recently-used idle doc (kept on
        // disk, lazy-reloaded later) to make room — so a large KB loads via LRU instead of
        // erroring. If every doc is actively connected, exceed the soft cap rather than
        // fail a load.
        if self.max_documents > 0 && docs.len() >= self.max_documents {
            match pick_lru_evictable(&docs) {
                Some(victim) => {
                    docs.remove(&victim);
                    debug!(doc = %victim, "lru-evicted idle doc from memory to make room (retained on disk)");
                }
                None => {
                    warn!(
                        in_memory = docs.len(),
                        max = self.max_documents,
                        "doc working set over capacity but all docs active — growing past the soft cap"
                    );
                }
            }
        }

        let (sync, wal_seq) = match self.storage.load_document(doc_name).await? {
            Some(state) => {
                let mut sync = if let Some(snapshot) = state.snapshot {
                    TextSync::from_state(&snapshot)
                        .map_err(|e| StorageError::Sqlite(format!("bad snapshot: {e}")))?
                } else {
                    TextSync::empty_relay()
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
                (TextSync::empty_relay(), 0)
            }
        };

        let entry = Arc::new(Mutex::new(DocEntry {
            sync,
            wal_seq,
            update_count: 0,
            last_activity: std::time::Instant::now(),
            connected_clients: 0,
            save_epoch: 0,
            last_saved_by: None,
            sharer_session_id: None,
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
        debug!(
            doc = doc_name,
            update_len = update.len(),
            wal_id,
            "apply_update: WAL appended"
        );

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

            // Warn if document exceeds max size (don't reject — CRDT convergence).
            if self.max_document_size_bytes > 0 {
                let content_len = doc.sync.content().len();
                if content_len > self.max_document_size_bytes {
                    warn!(
                        doc = doc_name,
                        size = content_len,
                        limit = self.max_document_size_bytes,
                        "document exceeds max size limit"
                    );
                }
            }

            // Force compaction at WAL entry hard limit.
            let forced = self.max_wal_entries > 0 && doc.update_count >= self.max_wal_entries;
            forced || doc.update_count >= self.compact_threshold
        };

        if should_compact {
            self.compact(doc_name).await?;
            debug!(doc = doc_name, "apply_update: compacted");
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

    /// Delete a document from memory and storage.
    pub async fn delete_doc(&self, doc_name: &str) -> Result<(), StorageError> {
        // Remove from in-memory map.
        {
            let mut docs = self.docs.write().await;
            docs.remove(doc_name);
        }
        // Remove from persistent storage.
        self.storage.delete_document(doc_name).await?;
        info!(doc = doc_name, "document deleted");
        Ok(())
    }

    /// List all in-memory documents.
    pub async fn document_names(&self) -> Vec<String> {
        let docs = self.docs.read().await;
        docs.keys().cloned().collect()
    }

    /// Number of documents currently in memory.
    pub async fn document_count(&self) -> usize {
        let docs = self.docs.read().await;
        docs.len()
    }

    /// Check if a document exists in memory.
    pub async fn has_doc(&self, name: &str) -> bool {
        let docs = self.docs.read().await;
        docs.contains_key(name)
    }

    /// Find a document by suffix matching. Returns the full doc name if exactly
    /// one document ends with `/<suffix>` or `:<suffix>`. Returns None if zero
    /// or multiple matches (ambiguous).
    pub async fn find_doc_by_suffix(&self, suffix: &str) -> Option<String> {
        let docs = self.docs.read().await;
        // Exact match takes priority.
        if docs.contains_key(suffix) {
            return Some(suffix.to_string());
        }
        let mut matches: Vec<&String> = docs
            .keys()
            .filter(|k| {
                k.ends_with(&format!("/{}", suffix)) || k.ends_with(&format!(":{}", suffix))
            })
            .collect();
        if matches.len() == 1 {
            Some(matches.remove(0).clone())
        } else {
            None // ambiguous or no match
        }
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
        Ok(hex::encode(hasher.finalize()))
    }

    /// Check if a client's expected hash matches the server's current content hash.
    /// Used before a save-to-disk operation to prevent overwriting concurrent edits.
    /// On success, increments save_epoch and returns it.
    pub async fn check_save_intent(
        &self,
        doc_name: &str,
        expected_hash: &str,
    ) -> Result<SaveIntentResult, StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let mut doc = entry.lock().await;
        let content = doc.sync.content();
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let server_hash = hex::encode(hasher.finalize());
        if server_hash == expected_hash {
            doc.save_epoch += 1;
            Ok(SaveIntentResult::Ok {
                server_hash,
                save_epoch: doc.save_epoch,
            })
        } else {
            Ok(SaveIntentResult::Conflict { server_hash })
        }
    }

    /// Record a completed save. Updates metadata for tracking.
    pub async fn record_save(&self, doc_name: &str, saved_by: &str) -> Result<(), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let mut doc = entry.lock().await;
        doc.last_saved_by = Some(saved_by.to_string());
        doc.last_activity = std::time::Instant::now();
        Ok(())
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
            save_epoch: doc.save_epoch,
            last_saved_by: doc.last_saved_by.clone(),
        })
    }

    /// Track a client connecting to a document.
    pub async fn track_client_connect(&self, doc_name: &str) -> Result<(), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let mut doc = entry.lock().await;
        doc.connected_clients += 1;
        doc.last_activity = std::time::Instant::now();
        debug!(
            doc = doc_name,
            connected_clients = doc.connected_clients,
            "track_client_connect"
        );
        Ok(())
    }

    /// Track a client disconnecting from a document.
    pub async fn track_client_disconnect(&self, doc_name: &str) -> Result<(), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let mut doc = entry.lock().await;
        doc.connected_clients = doc.connected_clients.saturating_sub(1);
        debug!(
            doc = doc_name,
            connected_clients = doc.connected_clients,
            "track_client_disconnect"
        );
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
                    info!(doc = %name, idle_secs = doc.last_activity.elapsed().as_secs(), "evict_idle: evicting document");
                    drop(doc);
                    docs.remove(name);
                    evicted.push(name.clone());
                }
            }
        }

        if !evicted.is_empty() {
            info!(count = evicted.len(), "evicted idle documents");
        }

        // BUG B fix: delete evicted EPHEMERAL docs from storage so recovery doesn't
        // reload them. ADR-032: durable KB content (kbc:/kb:) is memory-evicted ONLY —
        // never deleted — so a hosted KB with no connected client survives on disk and
        // lazy-reloads on next access (it was compacted above, so the snapshot is fresh).
        drop(docs); // release write lock before async storage calls
        for name in &evicted {
            if is_durable_doc(name) {
                debug!(doc = %name, "evict_idle: durable KB doc memory-evicted, retained on disk");
                continue;
            }
            if let Err(e) = self.storage.delete_document(name).await {
                warn!(doc = %name, error = %e, "storage delete after eviction failed");
            }
        }

        evicted
    }

    /// Encode full state and state vector atomically (single lock acquisition).
    /// Used by `sync/resync` to satisfy INV-2 (state vector consistency).
    pub async fn encode_state_and_sv(
        &self,
        doc_name: &str,
    ) -> Result<(Vec<u8>, Vec<u8>), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        let state = doc.sync.encode_state();
        let sv = doc.sync.state_vector();
        Ok((state, sv))
    }

    /// Encode diff and state vector atomically (single lock acquisition).
    /// Used by `sync/diff` to satisfy INV-2 (state vector consistency).
    pub async fn encode_diff_and_sv(
        &self,
        doc_name: &str,
        remote_sv: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), StorageError> {
        let entry = self.get_or_create(doc_name).await?;
        let doc = entry.lock().await;
        let diff = mae_sync::encoding::encode_diff(doc.sync.doc(), remote_sv)
            .map_err(|e| StorageError::Sqlite(format!("diff encoding: {e}")))?;
        let sv = doc.sync.state_vector();
        Ok((diff, sv))
    }

    /// Atomically share a document: delete old, create new, apply update, set connected_clients=1.
    /// Used by `sync/share` to satisfy INV-5 (connected_clients accuracy).
    pub async fn share_doc(
        &self,
        doc_name: &str,
        update: &[u8],
    ) -> Result<ApplyResult, StorageError> {
        // Validate before touching anything.
        validate_update(update)
            .map_err(|e| StorageError::Sqlite(format!("invalid update: {e}")))?;

        // Delete old doc from storage. Log errors — silent swallow could
        // lead to corrupted recovery if WAL append succeeds but old data remains.
        if let Err(e) = self.storage.delete_document(doc_name).await {
            warn!(doc = doc_name, error = %e, "share_doc: failed to delete old document from storage");
        }

        // Remove old in-memory entry.
        {
            let mut docs = self.docs.write().await;
            docs.remove(doc_name);
        }

        // WAL append first (durability).
        let wal_id = self.storage.wal_append(doc_name, update, None).await?;

        // Create new doc, apply update, set connected_clients=1.
        let entry = self.get_or_create(doc_name).await?;
        {
            let mut doc = entry.lock().await;
            doc.sync
                .apply_update(update)
                .map_err(|e| StorageError::Sqlite(format!("apply failed: {e}")))?;
            doc.wal_seq = wal_id;
            doc.update_count = 1;
            doc.last_activity = std::time::Instant::now();
            doc.connected_clients = 1; // BUG D fix: sharer is connected
        }
        info!(
            doc = doc_name,
            wal_seq = wal_id,
            update_len = update.len(),
            "share_doc: document shared"
        );

        Ok(ApplyResult {
            update: update.to_vec(),
            wal_seq: wal_id,
        })
    }

    /// Set the sharer session ID for a document.
    pub async fn set_sharer_session(&self, doc_name: &str, session_id: u64) {
        let docs = self.docs.read().await;
        if let Some(entry) = docs.get(doc_name) {
            let mut doc = entry.lock().await;
            doc.sharer_session_id = Some(session_id);
        }
    }

    /// Check if a session is the sharer for a document.
    pub async fn is_sharer(&self, doc_name: &str, session_id: u64) -> bool {
        let docs = self.docs.read().await;
        if let Some(entry) = docs.get(doc_name) {
            let doc = entry.lock().await;
            doc.sharer_session_id == Some(session_id)
        } else {
            false
        }
    }

    /// Clear the sharer for a document (called on sharer disconnect).
    pub async fn clear_sharer(&self, doc_name: &str) {
        let docs = self.docs.read().await;
        if let Some(entry) = docs.get(doc_name) {
            let mut doc = entry.lock().await;
            doc.sharer_session_id = None;
        }
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
        info!(
            doc = doc_name,
            wal_seq,
            state_len = state.len(),
            "compact_doc: snapshot written"
        );
        Ok(())
    }

    // --- KB metadata registry ---

    /// Store metadata for a shared KB (lightweight, non-CRDT).
    pub async fn set_kb_meta(&self, kb_id: &str, meta: serde_json::Value) {
        self.kb_metas.write().await.insert(kb_id.to_string(), meta);
    }

    /// List all registered KB metadata entries.
    pub async fn list_kb_metas(&self) -> Vec<serde_json::Value> {
        self.kb_metas.read().await.values().cloned().collect()
    }

    /// Remove KB metadata by ID.
    pub async fn remove_kb_meta(&self, kb_id: &str) {
        self.kb_metas.write().await.remove(kb_id);
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
    async fn apply_update_persists_to_wal() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend.clone(), 500);

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // WAL should have an entry.
        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert!(!state.wal_tail.is_empty(), "WAL should have entries");
    }

    #[tokio::test]
    async fn get_or_create_loads_from_storage() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());

        // Phase 1: create doc, persist, then evict from memory.
        {
            let store = DocStore::new(backend.clone(), 500);
            let mut ts = TextSync::with_client_id("", 1);
            let update = ts.insert(0, "persisted content");
            store.apply_update("doc1", &update, Some(1)).await.unwrap();
            store.compact_doc("doc1").await.unwrap();
        }

        // Phase 2: new store instance loads from storage.
        {
            let store = DocStore::new(backend.clone(), 500);
            let content = store.content("doc1").await.unwrap();
            assert_eq!(content, "persisted content");
        }
    }

    #[tokio::test]
    async fn evict_idle_deletes_from_storage() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend.clone(), 500);

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "evict me");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // Evict with 0 idle threshold (immediate).
        let evicted = store.evict_idle(0).await;
        assert_eq!(evicted, vec!["doc1"]);

        // BUG B regression: storage should also be cleared.
        let docs = backend.list_documents().await.unwrap();
        assert!(
            docs.is_empty(),
            "storage should be empty after eviction, got: {:?}",
            docs
        );
    }

    #[tokio::test]
    async fn durable_kb_doc_survives_idle_eviction() {
        // ADR-032 (Phase A1): a hosted KB with no connected client must NOT be
        // destroyed by idle eviction — durable KB docs (kbc:/kb:) are memory-evicted
        // only, never deleted from disk; ephemeral docs keep the evict-and-delete path.
        let store = test_store();

        // A durable KB node doc with content; the sharer then disconnects.
        let mut ts = TextSync::with_client_id("", 1);
        let kb_update = ts.insert(0, "ZEPHYRINE");
        store.share_doc("kb:concept:x", &kb_update).await.unwrap();
        store.track_client_disconnect("kb:concept:x").await.unwrap();

        // An ephemeral (non-KB) doc.
        let mut ts2 = TextSync::with_client_id("", 2);
        let eph_update = ts2.insert(0, "scratch");
        store.share_doc("scratch:buf", &eph_update).await.unwrap();
        store.track_client_disconnect("scratch:buf").await.unwrap();

        // Idle-evict everything (threshold 0).
        let evicted = store.evict_idle(0).await;
        assert!(evicted.contains(&"kb:concept:x".to_string()));
        assert!(evicted.contains(&"scratch:buf".to_string()));

        // Both are dropped from memory.
        assert!(!store.has_doc("kb:concept:x").await);
        assert!(!store.has_doc("scratch:buf").await);

        // The durable KB doc survives on disk and lazy-reloads with its content intact.
        assert_eq!(
            store.content("kb:concept:x").await.unwrap(),
            "ZEPHYRINE",
            "durable KB doc must survive idle eviction on disk"
        );
        // The ephemeral doc was deleted from storage → reloads empty.
        assert_eq!(
            store.content("scratch:buf").await.unwrap(),
            "",
            "ephemeral doc should be deleted from storage on eviction"
        );
    }

    #[tokio::test]
    async fn evict_skips_active_docs() {
        let store = test_store();

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "active doc");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // Mark as having a connected client.
        store.track_client_connect("doc1").await.unwrap();

        let evicted = store.evict_idle(0).await;
        assert!(evicted.is_empty(), "active docs should not be evicted");
    }

    #[tokio::test]
    async fn compact_creates_snapshot_trims_wal() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend.clone(), 500);

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "compact me");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        store.compact_doc("doc1").await.unwrap();

        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert!(
            state.snapshot.is_some(),
            "snapshot should exist after compaction"
        );
        assert!(
            state.wal_tail.is_empty(),
            "WAL should be trimmed after compaction"
        );
    }

    #[tokio::test]
    async fn recovery_loads_all_docs() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());

        // Create 3 docs, compact them (so they have snapshots).
        {
            let store = DocStore::new(backend.clone(), 500);
            let mut ts = TextSync::with_client_id("", 1);
            for name in &["alpha", "beta", "gamma"] {
                let update = ts.insert(0, name);
                store.apply_update(name, &update, Some(1)).await.unwrap();
                store.compact_doc(name).await.unwrap();
            }
        }

        // New store should find all docs in storage.
        let docs = backend.list_documents().await.unwrap();
        assert_eq!(docs.len(), 3, "all 3 docs should be in storage");
    }

    #[tokio::test]
    async fn encode_state_and_sv_consistent() {
        let store = test_store();

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "consistent");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // Atomic: both from same lock.
        let (state, sv) = store.encode_state_and_sv("doc1").await.unwrap();
        assert!(!state.is_empty());
        assert!(!sv.is_empty());

        // Verify they describe the same doc state: applying state to empty doc
        // should produce a doc whose sv matches.
        let ts2 = TextSync::from_state(&state).unwrap();
        assert_eq!(ts2.content(), "consistent");
    }

    #[tokio::test]
    async fn share_doc_atomic() {
        let store = test_store();

        // Create an initial doc.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "old content");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // Share replaces with new content.
        let ts2 = TextSync::new("new content");
        let new_state = ts2.encode_state();
        let result = store.share_doc("doc1", &new_state).await.unwrap();
        assert!(result.wal_seq > 0);

        // Content should be new, not concatenated.
        let content = store.content("doc1").await.unwrap();
        assert_eq!(content, "new content");

        // connected_clients should be 1 (BUG D regression).
        let stats = store.doc_stats("doc1").await.unwrap();
        assert_eq!(stats.connected_clients, 1);
    }

    #[tokio::test]
    async fn client_disconnect_decrements_count() {
        let store = test_store();

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "test");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        store.track_client_connect("doc1").await.unwrap();
        let stats = store.doc_stats("doc1").await.unwrap();
        assert_eq!(stats.connected_clients, 1);

        store.track_client_disconnect("doc1").await.unwrap();
        let stats = store.doc_stats("doc1").await.unwrap();
        assert_eq!(stats.connected_clients, 0);
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

    #[tokio::test]
    async fn max_documents_lru_evicts_to_make_room() {
        // ADR-032 (Phase A2): max_documents bounds the in-memory working set; a load
        // past the cap memory-evicts the LRU idle doc (kept on disk) instead of erroring.
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend, 500).with_max_documents(2);

        let mut ts1 = TextSync::with_client_id("", 1);
        let u1 = ts1.insert(0, "one");
        store.apply_update("doc1", &u1, Some(1)).await.unwrap();
        let mut ts2 = TextSync::with_client_id("", 2);
        let u2 = ts2.insert(0, "two");
        store.apply_update("doc2", &u2, Some(2)).await.unwrap();

        // The third load does NOT error — it LRU-evicts an idle doc from memory.
        let mut ts3 = TextSync::with_client_id("", 3);
        let u3 = ts3.insert(0, "three");
        store.apply_update("doc3", &u3, Some(3)).await.unwrap();
        assert!(
            store.document_count().await <= 2,
            "in-memory working set must respect the cap"
        );

        // All three are retrievable — the LRU-evicted one reloads from disk.
        assert_eq!(store.content("doc1").await.unwrap(), "one");
        assert_eq!(store.content("doc2").await.unwrap(), "two");
        assert_eq!(store.content("doc3").await.unwrap(), "three");
    }

    #[tokio::test]
    async fn large_kb_loads_past_the_memory_cap() {
        // A KB with more node docs than the in-memory cap must fully load (every node
        // retrievable), bounded by LRU memory eviction — the RoamNotes-scale case.
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend, 500).with_max_documents(4);

        for i in 0..20u32 {
            let mut ts = TextSync::with_client_id("", i as u64 + 1);
            let u = ts.insert(0, &format!("node {i}"));
            store
                .apply_update(&format!("kb:node:{i}"), &u, Some(i as u64 + 1))
                .await
                .unwrap();
        }
        assert!(
            store.document_count().await <= 4,
            "memory bounded by the cap, got {}",
            store.document_count().await
        );
        // Every node is still retrievable (reloads from disk on access).
        for i in 0..20u32 {
            assert_eq!(
                store.content(&format!("kb:node:{i}")).await.unwrap(),
                format!("node {i}")
            );
        }
    }

    #[tokio::test]
    async fn max_documents_allows_existing() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend, 500).with_max_documents(2);

        let mut ts = TextSync::with_client_id("", 1);
        let u1 = ts.insert(0, "hello");
        let u2 = ts.insert(5, " world");

        // Create both documents.
        store.apply_update("doc1", &u1, Some(1)).await.unwrap();
        store.apply_update("doc2", &u1, Some(2)).await.unwrap();

        // Applying a second update to an existing document must succeed even
        // though the map is at capacity — get_or_create takes the fast path.
        store
            .apply_update("doc1", &u2, Some(1))
            .await
            .expect("second update to existing doc must succeed at capacity");

        let content = store.content("doc1").await.unwrap();
        assert_eq!(content, "hello world");
    }

    #[tokio::test]
    async fn max_wal_entries_forces_compaction() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        // compact_threshold is high (500), but max_wal_entries is low (3).
        let store = DocStore::new(backend.clone(), 500).with_max_wal_entries(3);

        let mut ts = TextSync::with_client_id("", 1);
        for i in 0..5 {
            let update = ts.insert(i, "x");
            store.apply_update("doc1", &update, Some(1)).await.unwrap();
        }

        // After 5 updates with max_wal_entries=3, forced compaction should have run.
        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert!(
            state.snapshot.is_some(),
            "snapshot should exist after forced WAL compaction"
        );
    }

    #[tokio::test]
    async fn has_doc_returns_true_for_existing() {
        let store = test_store();
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();
        assert!(store.has_doc("doc1").await);
        assert!(!store.has_doc("nonexistent").await);
    }

    #[tokio::test]
    async fn find_doc_by_suffix_exact_match() {
        let store = test_store();
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        store
            .apply_update("test.txt", &update, Some(1))
            .await
            .unwrap();
        assert_eq!(
            store.find_doc_by_suffix("test.txt").await,
            Some("test.txt".to_string())
        );
    }

    #[tokio::test]
    async fn find_doc_by_suffix_file_address() {
        let store = test_store();
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        store
            .apply_update("file:no-project/test.txt", &update, Some(1))
            .await
            .unwrap();
        assert_eq!(
            store.find_doc_by_suffix("test.txt").await,
            Some("file:no-project/test.txt".to_string())
        );
    }

    #[tokio::test]
    async fn find_doc_by_suffix_no_match() {
        let store = test_store();
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();
        assert_eq!(store.find_doc_by_suffix("nonexistent").await, None);
    }

    #[tokio::test]
    async fn find_doc_by_suffix_ambiguous() {
        let store = test_store();
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        // Two docs that both end with /test.txt
        store
            .apply_update("file:proj-a/test.txt", &update, Some(1))
            .await
            .unwrap();
        store
            .apply_update("file:proj-b/test.txt", &update, Some(1))
            .await
            .unwrap();
        // Ambiguous — should return None
        assert_eq!(store.find_doc_by_suffix("test.txt").await, None);
    }

    #[tokio::test]
    async fn large_document_warns_but_accepts() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        // Set max_document_size to 5 bytes — any real content will exceed it.
        let store = DocStore::new(backend, 500).with_max_document_size(5);

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello world, this exceeds the limit");

        // Should succeed (warning only, no rejection).
        let result = store.apply_update("doc1", &update, Some(1)).await;
        assert!(
            result.is_ok(),
            "large document should be accepted with warning"
        );

        let content = store.content("doc1").await.unwrap();
        assert_eq!(content, "hello world, this exceeds the limit");
    }

    #[tokio::test]
    async fn share_doc_error_logged_not_swallowed() {
        let store = test_store();

        // Create an initial document.
        let mut ts = TextSync::with_client_id("", 1);
        let initial = ts.insert(0, "old content");
        store.apply_update("doc1", &initial, Some(1)).await.unwrap();

        // share_doc replaces the document with brand-new content.
        // The happy path must still produce the correct content even after the
        // internal delete (which logs errors instead of swallowing them via `let _ =`).
        let ts2 = TextSync::new("replaced content");
        let new_state = ts2.encode_state();
        let result = store.share_doc("doc1", &new_state).await;
        assert!(
            result.is_ok(),
            "share_doc must succeed on the happy path: {:?}",
            result.err()
        );

        let content = store.content("doc1").await.unwrap();
        assert_eq!(
            content, "replaced content",
            "share_doc must replace document content, not append"
        );

        // connected_clients is set to 1 by share_doc (BUG D invariant).
        let stats = store.doc_stats("doc1").await.unwrap();
        assert_eq!(stats.connected_clients, 1);
    }

    #[tokio::test]
    async fn sharer_session_tracking() {
        let store = test_store();
        let ts = TextSync::new("content");
        let state = ts.encode_state();
        store.share_doc("doc1", &state).await.unwrap();

        // Initially no sharer.
        assert!(!store.is_sharer("doc1", 42).await);

        // Set sharer.
        store.set_sharer_session("doc1", 42).await;
        assert!(store.is_sharer("doc1", 42).await);
        assert!(!store.is_sharer("doc1", 99).await);

        // Clear sharer.
        store.clear_sharer("doc1").await;
        assert!(!store.is_sharer("doc1", 42).await);
    }

    // --- WU-D: branch-level coverage tests ---

    #[tokio::test]
    async fn concurrent_tokio_spawn_access_same_doc() {
        let store = Arc::new(test_store());

        // Create initial doc.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "base");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // Spawn 10 concurrent tasks that all write to the same doc.
        let mut handles = Vec::new();
        for i in 0u64..10 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                let mut ts = TextSync::with_client_id("", 100 + i);
                // Read state first to get correct base.
                let sv = store.state_vector("doc1").await.unwrap();
                let _ = sv; // just ensure no deadlock
                let update = ts.insert(0, &format!("{i}"));
                store
                    .apply_update("doc1", &update, Some(100 + i))
                    .await
                    .unwrap();
            }));
        }

        // 5 second timeout — if this deadlocks, the test fails.
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for h in handles {
                h.await.unwrap();
            }
        })
        .await;
        assert!(result.is_ok(), "concurrent access must not deadlock");

        // All 10 spawned tasks + initial should have contributed.
        let content = store.content("doc1").await.unwrap();
        assert!(
            content.len() >= 14,
            "content should contain all contributions, got: {content}"
        );
    }

    #[tokio::test]
    async fn compaction_during_active_edits() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = Arc::new(DocStore::new(backend.clone(), 3)); // compact every 3

        // Make 10 edits in a tight loop — compaction will trigger at 3, 6, 9.
        let mut ts = TextSync::with_client_id("", 1);
        for i in 0u32..10 {
            let update = ts.insert(i, "x");
            store.apply_update("doc1", &update, Some(1)).await.unwrap();
        }

        // Content must be intact despite multiple compactions.
        let content = store.content("doc1").await.unwrap();
        assert_eq!(content.len(), 10, "all 10 chars must survive compaction");

        // Snapshot must exist.
        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert!(state.snapshot.is_some());

        // Now reload from storage in a fresh store and verify.
        let store2 = DocStore::new(backend.clone(), 500);
        let content2 = store2.content("doc1").await.unwrap();
        assert_eq!(content, content2, "reloaded content must match");
    }

    #[tokio::test]
    async fn idle_eviction_with_short_timeout() {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        let store = DocStore::new(backend.clone(), 500);

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "evict after timeout");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // With idle threshold 0, eviction is immediate.
        let evicted = store.evict_idle(0).await;
        assert_eq!(evicted, vec!["doc1"]);
        assert_eq!(store.document_count().await, 0);

        // Reload from storage (doc should be gone since eviction deletes).
        let docs = backend.list_documents().await.unwrap();
        assert!(docs.is_empty(), "eviction should remove from storage too");
    }

    #[tokio::test]
    async fn save_intent_and_committed() {
        let store = test_store();

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "saveme");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        // Save intent with correct hash.
        let content = store.content("doc1").await.unwrap();
        let hash = {
            let mut h = Sha256::new();
            h.update(content.as_bytes());
            hex::encode(h.finalize())
        };

        let result = store.check_save_intent("doc1", &hash).await.unwrap();
        match result {
            SaveIntentResult::Ok { save_epoch, .. } => {
                assert!(save_epoch > 0);
            }
            SaveIntentResult::Conflict { .. } => {
                panic!("expected Ok, got Conflict");
            }
        }
    }

    #[tokio::test]
    async fn save_intent_conflict_on_wrong_hash() {
        let store = test_store();

        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "content");
        store.apply_update("doc1", &update, Some(1)).await.unwrap();

        let result = store.check_save_intent("doc1", "wrong-hash").await.unwrap();
        match result {
            SaveIntentResult::Conflict { server_hash } => {
                assert!(!server_hash.is_empty());
            }
            SaveIntentResult::Ok { .. } => {
                panic!("expected Conflict, got Ok");
            }
        }
    }

    #[tokio::test]
    async fn content_of_nonexistent_doc() {
        let store = test_store();
        let content = store.content("nonexistent").await.unwrap();
        assert_eq!(content, "", "nonexistent doc should return empty string");
    }

    #[tokio::test]
    async fn multiple_share_doc_replaces() {
        let store = test_store();

        for i in 0..5 {
            let ts = TextSync::new(&format!("version {i}"));
            let state = ts.encode_state();
            store.share_doc("doc1", &state).await.unwrap();
        }

        let content = store.content("doc1").await.unwrap();
        assert_eq!(content, "version 4", "last share_doc wins");
    }

    #[tokio::test]
    async fn wal_seq_monotonic_across_updates() {
        let store = test_store();

        let mut ts = TextSync::with_client_id("", 1);
        let mut last_seq = 0u64;
        for i in 0u32..10 {
            let update = ts.insert(i, "x");
            let result = store.apply_update("doc1", &update, Some(1)).await.unwrap();
            assert!(
                result.wal_seq > last_seq,
                "wal_seq must increase monotonically"
            );
            last_seq = result.wal_seq;
        }
    }
}
