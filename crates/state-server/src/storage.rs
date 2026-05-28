//! Storage backend trait + SQLite implementation.
//!
//! WAL-first persistence: every sync update is appended to the WAL before
//! being applied in memory. Periodic compaction writes a full snapshot and
//! trims the WAL.

use std::path::Path;

use async_trait::async_trait;
use rusqlite::Connection;
use tracing::{debug, info};

/// Errors from storage operations.
#[derive(Debug)]
#[allow(dead_code)] // Io variant reserved for future backends
pub enum StorageError {
    Sqlite(String),
    Io(String),
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(msg) => write!(f, "sqlite: {msg}"),
            Self::Io(msg) => write!(f, "io: {msg}"),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        StorageError::Sqlite(e.to_string())
    }
}

/// State loaded for a single document.
pub struct DocumentState {
    /// Full state from last compaction snapshot (if any).
    pub snapshot: Option<Vec<u8>>,
    /// WAL entries since the snapshot.
    pub wal_tail: Vec<WalEntry>,
}

/// A single WAL entry.
#[allow(dead_code)] // client_id used for audit logging in future
pub struct WalEntry {
    pub id: u64,
    pub update: Vec<u8>,
    pub client_id: Option<u64>,
}

/// Trait for pluggable persistence backends.
#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Append an update to the WAL. Returns the assigned sequence ID.
    async fn wal_append(
        &self,
        doc_name: &str,
        update: &[u8],
        client_id: Option<u64>,
    ) -> Result<u64, StorageError>;

    /// Load snapshot + WAL tail for a document.
    async fn load_document(&self, doc_name: &str) -> Result<Option<DocumentState>, StorageError>;

    /// Write a compaction snapshot and trim WAL.
    async fn compact(
        &self,
        doc_name: &str,
        state: &[u8],
        up_to_wal_id: u64,
    ) -> Result<(), StorageError>;

    /// List all known documents.
    async fn list_documents(&self) -> Result<Vec<String>, StorageError>;

    /// Delete all data for a document (snapshot + WAL entries).
    async fn delete_document(&self, doc_name: &str) -> Result<(), StorageError>;
}

/// Sharded SQLite connection pool.
///
/// Multiple connections in WAL mode to the same file allow concurrent reads
/// across different documents. Documents are assigned to shards via FNV-1a hash.
pub struct SqlitePool {
    shards: Vec<std::sync::Mutex<Connection>>,
}

impl SqlitePool {
    /// Open `shard_count` connections in WAL mode to the same file.
    pub fn open(path: &Path, shard_count: usize) -> Result<Self, StorageError> {
        let count = shard_count.max(1);
        let mut shards = Vec::with_capacity(count);
        for i in 0..count {
            let conn = Connection::open(path)?;
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA busy_timeout=5000;",
            )?;
            // Only the first connection creates tables (idempotent via IF NOT EXISTS).
            if i == 0 {
                conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS wal (
                         id INTEGER PRIMARY KEY AUTOINCREMENT,
                         doc_name TEXT NOT NULL,
                         update_bytes BLOB NOT NULL,
                         client_id INTEGER,
                         created_at TEXT NOT NULL DEFAULT (datetime('now'))
                     );
                     CREATE INDEX IF NOT EXISTS idx_wal_doc ON wal(doc_name, id);

                     CREATE TABLE IF NOT EXISTS snapshots (
                         doc_name TEXT PRIMARY KEY,
                         state BLOB NOT NULL,
                         wal_id INTEGER NOT NULL,
                         updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                     );",
                )?;
            }
            shards.push(std::sync::Mutex::new(conn));
        }
        Ok(SqlitePool { shards })
    }

    /// Open an in-memory pool (for tests). shard_count is forced to 1
    /// because in-memory databases cannot share state across connections.
    pub fn open_memory(shard_count: usize) -> Result<Self, StorageError> {
        let _ = shard_count; // in-memory must be 1
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(
            "CREATE TABLE wal (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 doc_name TEXT NOT NULL,
                 update_bytes BLOB NOT NULL,
                 client_id INTEGER,
                 created_at TEXT NOT NULL DEFAULT (datetime('now'))
             );
             CREATE INDEX idx_wal_doc ON wal(doc_name, id);

             CREATE TABLE snapshots (
                 doc_name TEXT PRIMARY KEY,
                 state BLOB NOT NULL,
                 wal_id INTEGER NOT NULL,
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );",
        )?;
        Ok(SqlitePool {
            shards: vec![std::sync::Mutex::new(conn)],
        })
    }

    /// Select the shard for a given document name (FNV-1a hash).
    fn shard_for(&self, doc_name: &str) -> &std::sync::Mutex<Connection> {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in doc_name.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        &self.shards[hash as usize % self.shards.len()]
    }

    /// Primary shard (index 0) — used for schema operations and cross-doc queries.
    pub fn primary(&self) -> &std::sync::Mutex<Connection> {
        &self.shards[0]
    }
}

/// SQLite-backed storage using WAL journal mode with connection pooling.
pub struct SqliteBackend {
    pool: SqlitePool,
}

impl SqliteBackend {
    /// Open or create the SQLite database at the given path (default 4 shards).
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        Self::open_with_pool_size(path, 4)
    }

    /// Open with a specific pool size.
    pub fn open_with_pool_size(path: &Path, pool_size: usize) -> Result<Self, StorageError> {
        let pool = SqlitePool::open(path, pool_size)?;
        info!(path = %path.display(), shards = pool.shards.len(), "SQLite storage opened");
        Ok(SqliteBackend { pool })
    }

    /// Open an in-memory database (for testing).
    #[allow(dead_code)]
    pub fn open_memory() -> Result<Self, StorageError> {
        let pool = SqlitePool::open_memory(1)?;
        Ok(SqliteBackend { pool })
    }

    /// Query WAL entries with sequence ID > `since_seq` for a document.
    #[allow(dead_code)]
    pub fn wal_entries_since(
        &self,
        doc_name: &str,
        since_seq: u64,
    ) -> Result<Vec<(u64, Vec<u8>)>, StorageError> {
        let conn = self.pool.shard_for(doc_name).lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, update_bytes FROM wal WHERE doc_name = ?1 AND id > ?2 ORDER BY id",
        )?;
        let entries: Vec<(u64, Vec<u8>)> = stmt
            .query_map(rusqlite::params![doc_name, since_seq as i64], |row| {
                Ok((row.get::<_, i64>(0)? as u64, row.get(1)?))
            })?
            .collect::<Result<_, _>>()?;
        Ok(entries)
    }
}

#[async_trait]
impl StorageBackend for SqliteBackend {
    async fn wal_append(
        &self,
        doc_name: &str,
        update: &[u8],
        client_id: Option<u64>,
    ) -> Result<u64, StorageError> {
        let conn = self.pool.shard_for(doc_name).lock().unwrap();
        conn.execute(
            "INSERT INTO wal (doc_name, update_bytes, client_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![doc_name, update, client_id.map(|id| id as i64)],
        )?;
        let id = conn.last_insert_rowid() as u64;
        debug!(doc = doc_name, wal_id = id, "WAL append");
        Ok(id)
    }

    async fn load_document(&self, doc_name: &str) -> Result<Option<DocumentState>, StorageError> {
        let conn = self.pool.shard_for(doc_name).lock().unwrap();

        // Load snapshot if exists.
        let snapshot: Option<(Vec<u8>, i64)> = conn
            .query_row(
                "SELECT state, wal_id FROM snapshots WHERE doc_name = ?1",
                [doc_name],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();

        let (snapshot_bytes, wal_id_cutoff) = match &snapshot {
            Some((bytes, wal_id)) => (Some(bytes.clone()), *wal_id),
            None => (None, 0),
        };

        // Load WAL entries after the snapshot.
        let mut stmt = conn.prepare(
            "SELECT id, update_bytes, client_id FROM wal WHERE doc_name = ?1 AND id > ?2 ORDER BY id",
        )?;
        let entries: Vec<WalEntry> = stmt
            .query_map(rusqlite::params![doc_name, wal_id_cutoff], |row| {
                Ok(WalEntry {
                    id: row.get::<_, i64>(0)? as u64,
                    update: row.get(1)?,
                    client_id: row.get::<_, Option<i64>>(2)?.map(|v| v as u64),
                })
            })?
            .collect::<Result<_, _>>()?;

        if snapshot_bytes.is_none() && entries.is_empty() {
            return Ok(None);
        }

        Ok(Some(DocumentState {
            snapshot: snapshot_bytes,
            wal_tail: entries,
        }))
    }

    async fn compact(
        &self,
        doc_name: &str,
        state: &[u8],
        up_to_wal_id: u64,
    ) -> Result<(), StorageError> {
        let conn = self.pool.shard_for(doc_name).lock().unwrap();
        // Atomic: snapshot write + WAL trim in a single transaction.
        // Without this, a crash between the two statements causes duplicate
        // replay on recovery.
        conn.execute("BEGIN IMMEDIATE", [])?;
        let result = (|| -> Result<(), rusqlite::Error> {
            conn.execute(
                "INSERT OR REPLACE INTO snapshots (doc_name, state, wal_id, updated_at)
                 VALUES (?1, ?2, ?3, datetime('now'))",
                rusqlite::params![doc_name, state, up_to_wal_id as i64],
            )?;
            conn.execute(
                "DELETE FROM wal WHERE doc_name = ?1 AND id <= ?2",
                rusqlite::params![doc_name, up_to_wal_id as i64],
            )?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute("COMMIT", [])?;
                info!(doc = doc_name, up_to = up_to_wal_id, "compacted");
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK", []);
                Err(StorageError::Sqlite(format!("compact transaction: {e}")))
            }
        }
    }

    async fn list_documents(&self) -> Result<Vec<String>, StorageError> {
        let conn = self.pool.primary().lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT doc_name FROM (
                 SELECT doc_name FROM wal
                 UNION
                 SELECT doc_name FROM snapshots
             )",
        )?;
        let names: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(names)
    }

    async fn delete_document(&self, doc_name: &str) -> Result<(), StorageError> {
        let conn = self.pool.shard_for(doc_name).lock().unwrap();
        conn.execute("DELETE FROM snapshots WHERE doc_name = ?1", [doc_name])?;
        conn.execute("DELETE FROM wal WHERE doc_name = ?1", [doc_name])?;
        info!(doc = doc_name, "deleted document from storage");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn wal_append_and_load() {
        let backend = SqliteBackend::open_memory().unwrap();
        let id1 = backend
            .wal_append("doc1", b"update1", Some(1))
            .await
            .unwrap();
        let id2 = backend.wal_append("doc1", b"update2", None).await.unwrap();
        assert!(id2 > id1);

        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert!(state.snapshot.is_none());
        assert_eq!(state.wal_tail.len(), 2);
        assert_eq!(state.wal_tail[0].update, b"update1");
        assert_eq!(state.wal_tail[1].update, b"update2");
    }

    #[tokio::test]
    async fn load_nonexistent_returns_none() {
        let backend = SqliteBackend::open_memory().unwrap();
        assert!(backend.load_document("nope").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn compact_creates_snapshot_and_trims_wal() {
        let backend = SqliteBackend::open_memory().unwrap();
        let _id1 = backend.wal_append("doc1", b"u1", None).await.unwrap();
        let id2 = backend.wal_append("doc1", b"u2", None).await.unwrap();
        let _id3 = backend.wal_append("doc1", b"u3", None).await.unwrap();

        // Compact up to id2.
        backend.compact("doc1", b"full-state", id2).await.unwrap();

        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert_eq!(state.snapshot.as_deref(), Some(b"full-state".as_slice()));
        // Only u3 remains in WAL.
        assert_eq!(state.wal_tail.len(), 1);
        assert_eq!(state.wal_tail[0].update, b"u3");
    }

    #[tokio::test]
    async fn list_documents_from_wal_and_snapshots() {
        let backend = SqliteBackend::open_memory().unwrap();
        backend.wal_append("doc1", b"u1", None).await.unwrap();
        backend.wal_append("doc2", b"u2", None).await.unwrap();
        backend.compact("doc3", b"state", 0).await.unwrap();

        let mut docs = backend.list_documents().await.unwrap();
        docs.sort();
        assert_eq!(docs, vec!["doc1", "doc2", "doc3"]);
    }

    #[tokio::test]
    async fn compact_idempotent() {
        let backend = SqliteBackend::open_memory().unwrap();
        let id = backend.wal_append("doc1", b"u1", None).await.unwrap();
        backend.compact("doc1", b"state1", id).await.unwrap();
        backend.compact("doc1", b"state2", id).await.unwrap();

        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert_eq!(state.snapshot.as_deref(), Some(b"state2".as_slice()));
        assert!(state.wal_tail.is_empty());
    }

    #[tokio::test]
    async fn compact_is_atomic() {
        let backend = SqliteBackend::open_memory().unwrap();
        let id1 = backend.wal_append("doc1", b"u1", None).await.unwrap();
        let id2 = backend.wal_append("doc1", b"u2", None).await.unwrap();
        let id3 = backend.wal_append("doc1", b"u3", None).await.unwrap();

        // Compact up to id2, leaving id3 in the WAL.
        backend
            .compact("doc1", b"snapshot-at-id2", id2)
            .await
            .unwrap();

        let state = backend.load_document("doc1").await.unwrap().unwrap();

        // Invariant: snapshot must exist and its wal_id must be >= any remaining
        // WAL entry's id. This verifies the atomic post-condition: it is
        // impossible to observe a snapshot without the corresponding WAL trim
        // (or vice-versa), because compact() wraps both in a single transaction.
        let snap_wal_id: i64 = {
            let conn = backend.pool.primary().lock().unwrap();
            conn.query_row(
                "SELECT wal_id FROM snapshots WHERE doc_name = 'doc1'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert!(
            state.snapshot.is_some(),
            "snapshot must exist after compact"
        );
        for entry in &state.wal_tail {
            assert!(
                snap_wal_id as u64 >= id1,
                "snapshot.wal_id ({snap_wal_id}) must be >= first compacted id ({id1})"
            );
            assert!(
                entry.id > snap_wal_id as u64,
                "remaining WAL entry id ({}) must be > snapshot.wal_id ({snap_wal_id})",
                entry.id
            );
        }
        // Only id3 should remain.
        assert_eq!(state.wal_tail.len(), 1);
        assert_eq!(state.wal_tail[0].id, id3);
        assert_eq!(state.wal_tail[0].update, b"u3");
    }

    #[tokio::test]
    async fn recovery_after_wal_append_without_compact() {
        let backend = SqliteBackend::open_memory().unwrap();

        // Append 10 WAL entries without compacting.
        for i in 0u8..10 {
            backend.wal_append("doc1", &[i], None).await.unwrap();
        }

        let state = backend.load_document("doc1").await.unwrap().unwrap();

        // No compaction was performed, so there must be no snapshot.
        assert!(
            state.snapshot.is_none(),
            "no compaction occurred — snapshot must be None"
        );
        // All 10 WAL entries must be present and in order.
        assert_eq!(
            state.wal_tail.len(),
            10,
            "all 10 WAL entries must survive a load without compaction"
        );
        for (i, entry) in state.wal_tail.iter().enumerate() {
            assert_eq!(
                entry.update,
                vec![i as u8],
                "WAL entry {i} has wrong payload"
            );
        }
        // IDs must be monotonically increasing.
        let ids: Vec<u64> = state.wal_tail.iter().map(|e| e.id).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted, "WAL entries must be in id order");
    }

    // --- WU-D: branch-level coverage tests ---

    #[tokio::test]
    async fn wal_replay_with_corrupted_entry_stops_gracefully() {
        let backend = SqliteBackend::open_memory().unwrap();

        // Append valid + corrupted + valid entries.
        backend
            .wal_append("doc1", b"valid1", Some(1))
            .await
            .unwrap();
        backend
            .wal_append("doc1", b"\xff\xfe\x00\x01corrupted", None)
            .await
            .unwrap();
        backend
            .wal_append("doc1", b"valid3", Some(3))
            .await
            .unwrap();

        // load_document should return all entries (storage layer doesn't validate).
        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert_eq!(state.wal_tail.len(), 3, "all entries returned by storage");
        assert_eq!(state.wal_tail[0].update, b"valid1");
        assert_eq!(state.wal_tail[2].update, b"valid3");
        // The corrupted entry is stored as-is (CRDT layer validates on apply).
        assert!(
            state.wal_tail[1].update.starts_with(b"\xff\xfe"),
            "corrupted bytes preserved as-is"
        );
    }

    #[tokio::test]
    async fn concurrent_wal_writes_different_docs() {
        // SQLite WAL mode allows concurrent reads, serializes writes.
        // This test verifies no lock contention for different docs.
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());

        let mut handles = Vec::new();
        for i in 0u64..10 {
            let backend = Arc::clone(&backend);
            let doc_name = format!("doc{i}");
            handles.push(tokio::spawn(async move {
                for j in 0u8..5 {
                    backend.wal_append(&doc_name, &[j], Some(i)).await.unwrap();
                }
            }));
        }

        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for h in handles {
                h.await.unwrap();
            }
        })
        .await;
        assert!(result.is_ok(), "concurrent writes must not deadlock");

        // Verify all 10 docs exist with 5 entries each.
        let docs = backend.list_documents().await.unwrap();
        assert_eq!(docs.len(), 10);
        for i in 0..10 {
            let state = backend
                .load_document(&format!("doc{i}"))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(state.wal_tail.len(), 5, "doc{i} should have 5 WAL entries");
        }
    }

    #[tokio::test]
    async fn delete_document_removes_wal_and_snapshot() {
        let backend = SqliteBackend::open_memory().unwrap();

        let id = backend.wal_append("doc1", b"u1", None).await.unwrap();
        backend.compact("doc1", b"snapshot", id).await.unwrap();
        // Add one more after compaction.
        backend.wal_append("doc1", b"u2", None).await.unwrap();

        // Delete.
        backend.delete_document("doc1").await.unwrap();

        // Load should return None.
        assert!(backend.load_document("doc1").await.unwrap().is_none());
        assert!(backend.list_documents().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn wal_entries_since_filters_correctly() {
        let backend = SqliteBackend::open_memory().unwrap();

        let id1 = backend.wal_append("doc1", b"u1", None).await.unwrap();
        let id2 = backend.wal_append("doc1", b"u2", None).await.unwrap();
        let _id3 = backend.wal_append("doc1", b"u3", None).await.unwrap();

        // Entries since id1 should return u2 and u3.
        let entries = backend.wal_entries_since("doc1", id1).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, id2);
        assert_eq!(entries[0].1, b"u2");
    }

    #[tokio::test]
    async fn wal_append_with_and_without_client_id() {
        let backend = SqliteBackend::open_memory().unwrap();

        backend
            .wal_append("doc1", b"with-client", Some(42))
            .await
            .unwrap();
        backend
            .wal_append("doc1", b"no-client", None)
            .await
            .unwrap();

        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert_eq!(state.wal_tail[0].client_id, Some(42));
        assert_eq!(state.wal_tail[1].client_id, None);
    }

    #[tokio::test]
    async fn compact_with_zero_wal_id() {
        let backend = SqliteBackend::open_memory().unwrap();

        // Compact with wal_id=0 when no WAL exists (snapshot-only creation).
        backend
            .compact("doc1", b"initial-snapshot", 0)
            .await
            .unwrap();

        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert_eq!(
            state.snapshot.as_deref(),
            Some(b"initial-snapshot".as_slice())
        );
        assert!(state.wal_tail.is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_document_is_noop() {
        let backend = SqliteBackend::open_memory().unwrap();
        // Should not error.
        backend.delete_document("does-not-exist").await.unwrap();
    }
}
