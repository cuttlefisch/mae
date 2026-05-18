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
        conn.execute(
            "INSERT OR REPLACE INTO snapshots (doc_name, state, wal_id, updated_at)
             VALUES (?1, ?2, ?3, datetime('now'))",
            rusqlite::params![doc_name, state, up_to_wal_id as i64],
        )?;
        conn.execute(
            "DELETE FROM wal WHERE doc_name = ?1 AND id <= ?2",
            rusqlite::params![doc_name, up_to_wal_id as i64],
        )?;
        info!(doc = doc_name, up_to = up_to_wal_id, "compacted");
        Ok(())
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
