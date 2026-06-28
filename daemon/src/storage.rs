//! Storage backend trait + SQLite implementation.
//!
//! WAL-first persistence: every sync update is appended to the WAL before
//! being applied in memory. Periodic compaction writes a full snapshot and
//! trims the WAL.
//!
//! Uses the `sqlite` crate (same native library as CozoDB) to avoid
//! `links = "sqlite3"` conflicts with rusqlite.

use std::path::Path;

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

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

impl From<sqlite::Error> for StorageError {
    fn from(e: sqlite::Error) -> Self {
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

    /// Load the entire LOCAL per-KB blocklist as `(kb_id, principal)` pairs
    /// (ADR-039 A2, #162). Local self-protection only — never synced to peers.
    /// Default: no durable blocklist (an in-memory-only backend has nothing to load).
    async fn load_blocklist(&self) -> Result<Vec<(String, String)>, StorageError> {
        Ok(Vec::new())
    }

    /// Add `principal` to `kb_id`'s local blocklist (idempotent). Default no-op.
    async fn add_block(&self, _kb_id: &str, _principal: &str) -> Result<(), StorageError> {
        Ok(())
    }

    /// Remove `principal` from `kb_id`'s local blocklist (idempotent). Default no-op.
    async fn remove_block(&self, _kb_id: &str, _principal: &str) -> Result<(), StorageError> {
        Ok(())
    }
}

/// Sharded SQLite connection pool.
///
/// Multiple connections in WAL mode to the same file allow concurrent reads
/// across different documents. Documents are assigned to shards via FNV-1a hash.
pub struct SqlitePool {
    shards: Vec<std::sync::Mutex<sqlite::Connection>>,
}

impl SqlitePool {
    /// Open `shard_count` connections in WAL mode to the same file.
    pub fn open(path: &Path, shard_count: usize) -> Result<Self, StorageError> {
        let count = shard_count.max(1);
        let mut shards = Vec::with_capacity(count);
        for i in 0..count {
            let conn = sqlite::Connection::open(path)?;
            conn.execute(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA busy_timeout=5000;
                 PRAGMA secure_delete=ON;",
            )?;
            // ADR-037 (#171): a key-blind daemon must not retain RECOVERABLE plaintext
            // at rest. `secure_delete=ON` zeroes freed pages on DELETE, so when E2E is
            // enabled on a previously-plaintext KB and the node docs are reseal-replaced
            // (delete + recreate as ciphertext, see `delete_document` + `share_doc`), the
            // pre-enable plaintext bytes are overwritten in the main DB file rather than
            // lingering in free pages. The WAL sidecar is TRUNCATE-checkpointed after the
            // delete so an uncheckpointed pre-image can't linger there either.
            // Only the first connection creates tables (idempotent via IF NOT EXISTS).
            if i == 0 {
                conn.execute(
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
                         hash TEXT NOT NULL DEFAULT '',
                         updated_at TEXT NOT NULL DEFAULT (datetime('now'))
                     );

                     -- ADR-039 A2 (#162): the per-KB LOCAL self-protection blocklist.
                     -- A purely local deny-list of principals this daemon refuses to
                     -- trust — NEVER propagated to peers (distinct from a signed op-log
                     -- Remove/Revoke), so it lives in local storage and NOT in the synced
                     -- `kbc:` collection doc. Durable so a self-protection block survives
                     -- restart (an evaporating deny-list is a weak defense).
                     CREATE TABLE IF NOT EXISTS kb_blocklist (
                         kb_id TEXT NOT NULL,
                         principal TEXT NOT NULL,
                         created_at TEXT NOT NULL DEFAULT (datetime('now')),
                         PRIMARY KEY (kb_id, principal)
                     );",
                )?;
                // ADR-032 A5: migrate snapshot tables created before the integrity
                // hash column. Errors (duplicate column on a fresh table) are ignored.
                let _ =
                    conn.execute("ALTER TABLE snapshots ADD COLUMN hash TEXT NOT NULL DEFAULT ''");
            }
            shards.push(std::sync::Mutex::new(conn));
        }
        Ok(SqlitePool { shards })
    }

    /// Open an in-memory pool (for tests). shard_count is forced to 1
    /// because in-memory databases cannot share state across connections.
    pub fn open_memory(_shard_count: usize) -> Result<Self, StorageError> {
        let conn = sqlite::Connection::open(":memory:")?;
        conn.execute(
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
                 hash TEXT NOT NULL DEFAULT '',
                 updated_at TEXT NOT NULL DEFAULT (datetime('now'))
             );

             CREATE TABLE kb_blocklist (
                 kb_id TEXT NOT NULL,
                 principal TEXT NOT NULL,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')),
                 PRIMARY KEY (kb_id, principal)
             );",
        )?;
        Ok(SqlitePool {
            shards: vec![std::sync::Mutex::new(conn)],
        })
    }

    /// Select the shard for a given document name (FNV-1a hash).
    fn shard_for(&self, doc_name: &str) -> &std::sync::Mutex<sqlite::Connection> {
        let mut hash: u64 = 0xcbf29ce484222325;
        for byte in doc_name.as_bytes() {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x100000001b3);
        }
        &self.shards[hash as usize % self.shards.len()]
    }

    /// Primary shard (index 0) — used for schema operations and cross-doc queries.
    pub fn primary(&self) -> &std::sync::Mutex<sqlite::Connection> {
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
        stmt.bind((1, doc_name))?;
        stmt.bind((2, since_seq as i64))?;

        let mut entries = Vec::new();
        while let Ok(sqlite::State::Row) = stmt.next() {
            let id = stmt.read::<i64, _>("id")? as u64;
            let update = stmt.read::<Vec<u8>, _>("update_bytes")?;
            entries.push((id, update));
        }
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
        let mut stmt = conn
            .prepare("INSERT INTO wal (doc_name, update_bytes, client_id) VALUES (?1, ?2, ?3)")?;
        stmt.bind((1, doc_name))?;
        stmt.bind((2, update))?;
        match client_id {
            Some(id) => stmt.bind((3, id as i64))?,
            None => stmt.bind((3, sqlite::Value::Null))?,
        }
        stmt.next()?;

        // Get last insert rowid
        let mut id_stmt = conn.prepare("SELECT last_insert_rowid()")?;
        id_stmt.next()?;
        let id = id_stmt.read::<i64, _>(0)? as u64;

        debug!(doc = doc_name, wal_id = id, "WAL append");
        Ok(id)
    }

    async fn load_document(&self, doc_name: &str) -> Result<Option<DocumentState>, StorageError> {
        let conn = self.pool.shard_for(doc_name).lock().unwrap();

        // Load snapshot if exists.
        let mut snap_stmt =
            conn.prepare("SELECT state, wal_id, hash FROM snapshots WHERE doc_name = ?1")?;
        snap_stmt.bind((1, doc_name))?;

        let (snapshot_bytes, wal_id_cutoff) = if let Ok(sqlite::State::Row) = snap_stmt.next() {
            let state = snap_stmt.read::<Vec<u8>, _>("state")?;
            let wal_id = snap_stmt.read::<i64, _>("wal_id")?;
            let stored_hash = snap_stmt.read::<String, _>("hash")?;
            // ADR-032 A5: verify snapshot integrity. An empty hash is a legacy snapshot
            // (pre-A5), left unverified for back-compat. A non-empty hash that does not
            // match the stored bytes means corruption — DISCARD the snapshot and degrade
            // to a WAL-only load (cutoff 0) so the doc stays loadable and can self-heal
            // via mesh re-sync / projection rebuild / backup restore, rather than
            // silently serving corrupt state.
            if !stored_hash.is_empty() && hex::encode(Sha256::digest(&state)) != stored_hash {
                warn!(
                    doc = doc_name,
                    "snapshot integrity check FAILED — discarding corrupt snapshot; doc reloads \
                     from WAL and heals via re-sync/rebuild (ADR-032 A5)"
                );
                (None, 0)
            } else {
                (Some(state), wal_id)
            }
        } else {
            (None, 0)
        };

        // Load WAL entries after the snapshot.
        let mut wal_stmt = conn.prepare(
            "SELECT id, update_bytes, client_id FROM wal WHERE doc_name = ?1 AND id > ?2 ORDER BY id",
        )?;
        wal_stmt.bind((1, doc_name))?;
        wal_stmt.bind((2, wal_id_cutoff))?;

        let mut entries = Vec::new();
        while let Ok(sqlite::State::Row) = wal_stmt.next() {
            let id = wal_stmt.read::<i64, _>("id")? as u64;
            let update = wal_stmt.read::<Vec<u8>, _>("update_bytes")?;
            let client_id = match wal_stmt.read::<sqlite::Value, _>("client_id")? {
                sqlite::Value::Integer(v) => Some(v as u64),
                _ => None,
            };
            entries.push(WalEntry {
                id,
                update,
                client_id,
            });
        }

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
        // ADR-032 A5: a content hash committed with the snapshot, verified on load.
        let hash = hex::encode(Sha256::digest(state));
        let conn = self.pool.shard_for(doc_name).lock().unwrap();
        // Atomic: snapshot write + WAL trim in a single transaction.
        conn.execute("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<(), sqlite::Error> {
            let mut snap_stmt = conn.prepare(
                "INSERT OR REPLACE INTO snapshots (doc_name, state, wal_id, hash, updated_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))",
            )?;
            snap_stmt.bind((1, doc_name))?;
            snap_stmt.bind((2, state))?;
            snap_stmt.bind((3, up_to_wal_id as i64))?;
            snap_stmt.bind((4, hash.as_str()))?;
            snap_stmt.next()?;

            let mut del_stmt = conn.prepare("DELETE FROM wal WHERE doc_name = ?1 AND id <= ?2")?;
            del_stmt.bind((1, doc_name))?;
            del_stmt.bind((2, up_to_wal_id as i64))?;
            del_stmt.next()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute("COMMIT")?;
                info!(doc = doc_name, up_to = up_to_wal_id, "compacted");
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute("ROLLBACK");
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
        let mut names = Vec::new();
        while let Ok(sqlite::State::Row) = stmt.next() {
            names.push(stmt.read::<String, _>("doc_name")?);
        }
        Ok(names)
    }

    async fn delete_document(&self, doc_name: &str) -> Result<(), StorageError> {
        let conn = self.pool.shard_for(doc_name).lock().unwrap();

        let mut stmt1 = conn.prepare("DELETE FROM snapshots WHERE doc_name = ?1")?;
        stmt1.bind((1, doc_name))?;
        stmt1.next()?;

        let mut stmt2 = conn.prepare("DELETE FROM wal WHERE doc_name = ?1")?;
        stmt2.bind((1, doc_name))?;
        stmt2.next()?;

        // ADR-037 (#171): with `secure_delete=ON` the rows above are zeroed in the main
        // DB, but the zeroing is journalled through the WAL sidecar. TRUNCATE-checkpoint
        // so the sidecar can't retain a pre-image of the just-purged plaintext at rest.
        // Best-effort: downgrades to PASSIVE if another shard holds a read mark — the
        // zeroed pages still land on the next checkpoint; we never block the delete on it.
        let _ = conn.execute("PRAGMA wal_checkpoint(TRUNCATE);");

        info!(doc = doc_name, "deleted document from storage");
        Ok(())
    }

    async fn load_blocklist(&self) -> Result<Vec<(String, String)>, StorageError> {
        let conn = self.pool.primary().lock().unwrap();
        let mut stmt = conn.prepare("SELECT kb_id, principal FROM kb_blocklist")?;
        let mut pairs = Vec::new();
        while let Ok(sqlite::State::Row) = stmt.next() {
            let kb_id = stmt.read::<String, _>("kb_id")?;
            let principal = stmt.read::<String, _>("principal")?;
            pairs.push((kb_id, principal));
        }
        Ok(pairs)
    }

    async fn add_block(&self, kb_id: &str, principal: &str) -> Result<(), StorageError> {
        let conn = self.pool.primary().lock().unwrap();
        // Idempotent: re-blocking an already-blocked principal is a no-op.
        let mut stmt =
            conn.prepare("INSERT OR IGNORE INTO kb_blocklist (kb_id, principal) VALUES (?1, ?2)")?;
        stmt.bind((1, kb_id))?;
        stmt.bind((2, principal))?;
        stmt.next()?;
        Ok(())
    }

    async fn remove_block(&self, kb_id: &str, principal: &str) -> Result<(), StorageError> {
        let conn = self.pool.primary().lock().unwrap();
        let mut stmt =
            conn.prepare("DELETE FROM kb_blocklist WHERE kb_id = ?1 AND principal = ?2")?;
        stmt.bind((1, kb_id))?;
        stmt.bind((2, principal))?;
        stmt.next()?;
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

        backend.compact("doc1", b"full-state", id2).await.unwrap();

        let state = backend.load_document("doc1").await.unwrap().unwrap();
        assert_eq!(state.snapshot.as_deref(), Some(b"full-state".as_slice()));
        assert_eq!(state.wal_tail.len(), 1);
        assert_eq!(state.wal_tail[0].update, b"u3");
    }

    #[tokio::test]
    async fn corrupt_snapshot_is_detected_and_discarded() {
        // ADR-032 A5: a snapshot carries a content hash; on load, a mismatch (disk
        // corruption / tampering) is detected and the corrupt snapshot is NOT served.
        let backend = SqliteBackend::open_memory().unwrap();
        backend.compact("d", b"valid-state", 0).await.unwrap();

        // A valid (hash-matching) snapshot loads normally.
        let ok = backend.load_document("d").await.unwrap().unwrap();
        assert_eq!(ok.snapshot.as_deref(), Some(b"valid-state".as_slice()));

        // Corrupt the snapshot bytes WITHOUT updating its hash.
        {
            let conn = backend.pool.shard_for("d").lock().unwrap();
            let mut s = conn
                .prepare("UPDATE snapshots SET state = ?1 WHERE doc_name = 'd'")
                .unwrap();
            s.bind((1, &b"tampered"[..])).unwrap();
            s.next().unwrap();
        }

        // The corrupt snapshot is discarded (degraded to WAL-only, here empty).
        let loaded = backend.load_document("d").await.unwrap();
        assert!(
            loaded.as_ref().is_none_or(|ds| ds.snapshot.is_none()),
            "a corrupt snapshot must be discarded, not served"
        );
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
    async fn delete_document_removes_wal_and_snapshot() {
        let backend = SqliteBackend::open_memory().unwrap();
        let id = backend.wal_append("doc1", b"u1", None).await.unwrap();
        backend.compact("doc1", b"snapshot", id).await.unwrap();
        backend.wal_append("doc1", b"u2", None).await.unwrap();

        backend.delete_document("doc1").await.unwrap();

        assert!(backend.load_document("doc1").await.unwrap().is_none());
        assert!(backend.list_documents().await.unwrap().is_empty());
    }

    /// ADR-037 (#171) — the AT-REST confidentiality oracle (the attacker's test). A
    /// key-blind daemon must not retain RECOVERABLE plaintext after a doc is purged. With
    /// `secure_delete=ON` + a TRUNCATE checkpoint, `delete_document` must scrub the bytes
    /// from the DB FILE itself — not merely unlink the rows (SQLite leaves deleted content
    /// legible in free pages / the WAL sidecar otherwise). This is what makes reseal-as-
    /// replace (delete plaintext lineage → recreate as ciphertext) an actual purge.
    #[tokio::test]
    async fn delete_document_scrubs_plaintext_from_the_db_file_at_rest() {
        use std::io::Read;
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("collab.sqlite");
        // A unique, recognizable plaintext "canary" — what a pre-enable plaintext share
        // would have written to the daemon before E2E was turned on.
        let canary = b"PLAINTEXT-CANARY-7f3a9e21-must-not-survive-purge";

        let raw_contains_canary = |p: &std::path::Path| -> bool {
            // Scan the DB file AND any -wal/-shm sidecars in the dir (an attacker reads
            // the whole on-disk footprint, not just the main file).
            let mut hit = false;
            for entry in std::fs::read_dir(p.parent().unwrap()).unwrap() {
                let path = entry.unwrap().path();
                let mut buf = Vec::new();
                if std::fs::File::open(&path)
                    .and_then(|mut f| f.read_to_end(&mut buf))
                    .is_ok()
                    && buf.windows(canary.len()).any(|w| w == canary.as_slice())
                {
                    hit = true;
                }
            }
            hit
        };

        {
            let backend = SqliteBackend::open(&db_path).unwrap();
            // Write the plaintext canary as a node's content, then compact it into the
            // snapshot (mirrors a plaintext share that has been checkpointed to disk).
            let id = backend.wal_append("kb:n1", canary, None).await.unwrap();
            backend.compact("kb:n1", canary, id).await.unwrap();
            // Also leave a fresh WAL-tail copy (mirrors a not-yet-compacted plaintext edit).
            backend.wal_append("kb:n1", canary, None).await.unwrap();
            assert!(
                raw_contains_canary(&db_path),
                "precondition: the plaintext canary is on disk before purge (else the test is vacuous)"
            );

            // The purge (what reseal-as-replace's share_doc triggers).
            backend.delete_document("kb:n1").await.unwrap();

            assert!(
                !raw_contains_canary(&db_path),
                "the plaintext canary MUST be scrubbed from the DB file at rest after delete_document (secure_delete + checkpoint)"
            );
        }
    }

    /// ADR-039 A2 (#162): the local blocklist round-trips through storage and — the
    /// security-relevant property — SURVIVES a reopen (a self-protection deny-list that
    /// evaporates on restart is a weak defense). Uses a file-backed DB so the second
    /// `open` is a genuine restart, not a shared in-memory handle.
    #[tokio::test]
    async fn blocklist_round_trips_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("collab.sqlite");
        {
            let backend = SqliteBackend::open(&db_path).unwrap();
            backend.add_block("kbA", "SHA256:evil").await.unwrap();
            backend.add_block("kbA", "SHA256:alsobad").await.unwrap();
            backend.add_block("kbB", "SHA256:evil").await.unwrap();
            // Idempotent re-block is a no-op (PRIMARY KEY + INSERT OR IGNORE).
            backend.add_block("kbA", "SHA256:evil").await.unwrap();
            // Unblock drops exactly one (kb_id, principal) pair.
            backend.remove_block("kbA", "SHA256:alsobad").await.unwrap();
        }
        // Reopen — a fresh process would see the durable table.
        let backend = SqliteBackend::open(&db_path).unwrap();
        let mut pairs = backend.load_blocklist().await.unwrap();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("kbA".to_string(), "SHA256:evil".to_string()),
                ("kbB".to_string(), "SHA256:evil".to_string()),
            ],
            "blocks persist across restart; the unblocked pair is gone; the block is per-KB"
        );
    }

    #[tokio::test]
    async fn wal_entries_since_filters_correctly() {
        let backend = SqliteBackend::open_memory().unwrap();
        let id1 = backend.wal_append("doc1", b"u1", None).await.unwrap();
        let id2 = backend.wal_append("doc1", b"u2", None).await.unwrap();
        let _id3 = backend.wal_append("doc1", b"u3", None).await.unwrap();

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
        backend.delete_document("does-not-exist").await.unwrap();
    }

    #[tokio::test]
    async fn concurrent_wal_writes_different_docs() {
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
}
