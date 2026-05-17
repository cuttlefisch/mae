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

/// SQLite-backed storage using WAL journal mode.
pub struct SqliteBackend {
    /// We use a std::sync::Mutex because rusqlite::Connection is !Send.
    /// All operations are synchronous and fast (sub-ms for WAL append).
    conn: std::sync::Mutex<Connection>,
}

impl SqliteBackend {
    /// Open or create the SQLite database at the given path.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=5000;

             CREATE TABLE IF NOT EXISTS wal (
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

        info!(path = %path.display(), "SQLite storage opened");
        Ok(SqliteBackend {
            conn: std::sync::Mutex::new(conn),
        })
    }

    /// Open an in-memory database (for testing).
    #[allow(dead_code)]
    pub fn open_memory() -> Result<Self, StorageError> {
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
        Ok(SqliteBackend {
            conn: std::sync::Mutex::new(conn),
        })
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO wal (doc_name, update_bytes, client_id) VALUES (?1, ?2, ?3)",
            rusqlite::params![doc_name, update, client_id.map(|id| id as i64)],
        )?;
        let id = conn.last_insert_rowid() as u64;
        debug!(doc = doc_name, wal_id = id, "WAL append");
        Ok(id)
    }

    async fn load_document(&self, doc_name: &str) -> Result<Option<DocumentState>, StorageError> {
        let conn = self.conn.lock().unwrap();

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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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
