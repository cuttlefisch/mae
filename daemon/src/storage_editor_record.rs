//! SQL for the per-document editor record (ADR-113 D3; see `doc_editors`).
//!
//! Two tables:
//! - `doc_editors`: one row per `(document, kind, editor)`, coalesced over all
//!   that editor's updates. `pending` marks a submitter whose content yrs is
//!   still holding back (see `doc_editors::PendingChange`).
//! - `doc_editor_origin`: per document, the WAL id after which every update is
//!   recorded. A cutoff before it cannot be answered (`EditorsSince::Incomplete`).
//!
//! Neither is touched by `delete_document`: a share or reseal deletes and
//! recreates a node's content, and its editors must survive that. `forget` is
//! the explicit purge, for ephemeral documents only.

use super::StorageError;
use crate::doc_editors::{EditStamp, EditorKind, EditorRecord, EditorsSince};

pub(super) const DOC_EDITORS_SCHEMA: &str = "CREATE TABLE IF NOT EXISTS doc_editors (
     doc_name TEXT NOT NULL,
     kind TEXT NOT NULL,
     editor TEXT NOT NULL,
     first_at INTEGER NOT NULL,
     last_at INTEGER NOT NULL,
     last_wal_id INTEGER NOT NULL,
     updates INTEGER NOT NULL,
     pending INTEGER NOT NULL DEFAULT 0,
     PRIMARY KEY (doc_name, kind, editor)
 );";

const ORIGIN_SCHEMA: &str = "CREATE TABLE doc_editor_origin (
     doc_name TEXT PRIMARY KEY,
     recorded_after_wal_id INTEGER NOT NULL
 );";

/// A document's WAL high-water mark: the largest WAL id it has ever been
/// written at, whether still in the WAL or folded into its snapshot. 0 for a
/// document storage has never seen.
const HIGH_WATER: &str = "SELECT COALESCE(MAX(w), 0) FROM (
     SELECT MAX(id) AS w FROM wal WHERE doc_name = ?1
     UNION ALL SELECT wal_id FROM snapshots WHERE doc_name = ?1
 )";

/// Create the record's tables, once per database, on its first connection.
///
/// **Upgrade:** the first time the origin table is created, every document
/// that already exists gets an origin at its current high-water mark — its
/// existing content predates the record, so who wrote it is unknown, and a
/// cutoff before that point must read as incomplete rather than "nobody".
pub(super) fn ensure_schema(conn: &sqlite::Connection) -> Result<(), sqlite::Error> {
    conn.execute(DOC_EDITORS_SCHEMA)?;
    // A database from before the `pending` column. Errors (a duplicate column
    // on a current table) are expected and ignored, as for `snapshots.hash`.
    let _ = conn.execute("ALTER TABLE doc_editors ADD COLUMN pending INTEGER NOT NULL DEFAULT 0");
    let mut probe = conn.prepare(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'doc_editor_origin'",
    )?;
    if let sqlite::State::Row = probe.next()? {
        return Ok(());
    }
    conn.execute("BEGIN IMMEDIATE")?;
    let made = conn.execute(ORIGIN_SCHEMA).and_then(|()| {
        conn.execute(
            "INSERT INTO doc_editor_origin (doc_name, recorded_after_wal_id)
             SELECT doc_name, MAX(w) FROM (
                 SELECT doc_name, id AS w FROM wal
                 UNION ALL SELECT doc_name, wal_id FROM snapshots
             ) GROUP BY doc_name",
        )
    });
    commit_or_rollback(conn, made)
}

/// COMMIT `result`'s transaction, or ROLLBACK it. A failed COMMIT leaves the
/// transaction open in SQLite, which would wedge the connection (every later
/// `BEGIN` fails), so it is rolled back too.
pub(super) fn commit_or_rollback<T>(
    conn: &sqlite::Connection,
    result: Result<T, sqlite::Error>,
) -> Result<T, sqlite::Error> {
    match result.and_then(|v| conn.execute("COMMIT").map(|()| v)) {
        Ok(v) => Ok(v),
        Err(e) => {
            let _ = conn.execute("ROLLBACK");
            Err(e)
        }
    }
}

/// Append one WAL row and record its editors. Runs inside the caller's
/// transaction; returns the WAL id.
pub(super) fn write_attributed(
    conn: &sqlite::Connection,
    doc_name: &str,
    update: &[u8],
    client_id: Option<u64>,
    by: &EditStamp,
) -> Result<u64, sqlite::Error> {
    if !by.resets_origin {
        // First write since the record began: everything before it is unattributed.
        let mut stmt = conn.prepare(format!(
            "INSERT OR IGNORE INTO doc_editor_origin (doc_name, recorded_after_wal_id)
             VALUES (?1, ({HIGH_WATER}))"
        ))?;
        stmt.bind((1, doc_name))?;
        stmt.next()?;
    }
    let id = insert_wal_row(conn, doc_name, update, client_id)?;
    if by.resets_origin {
        let mut stmt = conn.prepare(
            "INSERT OR REPLACE INTO doc_editor_origin (doc_name, recorded_after_wal_id)
             VALUES (?1, ?2)",
        )?;
        stmt.bind((1, doc_name))?;
        stmt.bind((2, id as i64))?;
        stmt.next()?;
    }
    if by.pending.integrated {
        stamp_pending(conn, doc_name, by.at_ms, id)?;
    }
    for (kind, editor) in &by.editors {
        upsert_editor_row(conn, doc_name, (*kind, editor), by, id)?;
    }
    if by.pending.cleared {
        let mut stmt = conn.prepare("UPDATE doc_editors SET pending = 0 WHERE doc_name = ?1")?;
        stmt.bind((1, doc_name))?;
        stmt.next()?;
    }
    Ok(id)
}

fn insert_wal_row(
    conn: &sqlite::Connection,
    doc_name: &str,
    update: &[u8],
    client_id: Option<u64>,
) -> Result<u64, sqlite::Error> {
    let mut stmt =
        conn.prepare("INSERT INTO wal (doc_name, update_bytes, client_id) VALUES (?1, ?2, ?3)")?;
    stmt.bind((1, doc_name))?;
    stmt.bind((2, update))?;
    match client_id {
        Some(id) => stmt.bind((3, id as i64))?,
        None => stmt.bind((3, sqlite::Value::Null))?,
    }
    stmt.next()?;
    let mut id_stmt = conn.prepare("SELECT last_insert_rowid()")?;
    id_stmt.next()?;
    Ok(id_stmt.read::<i64, _>(0)? as u64)
}

/// Content held back from earlier submitters may have integrated at `wal_id`:
/// stamp each of them there, so a cutoff before this update still sees them.
fn stamp_pending(
    conn: &sqlite::Connection,
    doc_name: &str,
    at_ms: u64,
    wal_id: u64,
) -> Result<(), sqlite::Error> {
    let mut stmt = conn.prepare(
        "UPDATE doc_editors SET last_at = MAX(last_at, ?2), last_wal_id = MAX(last_wal_id, ?3)
         WHERE doc_name = ?1 AND pending = 1",
    )?;
    stmt.bind((1, doc_name))?;
    stmt.bind((2, at_ms as i64))?;
    stmt.bind((3, wal_id as i64))?;
    stmt.next()?;
    Ok(())
}

/// Fold one attributed update into its editor row.
fn upsert_editor_row(
    conn: &sqlite::Connection,
    doc_name: &str,
    (kind, editor): (EditorKind, &str),
    by: &EditStamp,
    wal_id: u64,
) -> Result<(), sqlite::Error> {
    let mut stmt = conn.prepare(
        "INSERT INTO doc_editors
             (doc_name, kind, editor, first_at, last_at, last_wal_id, updates, pending)
         VALUES (?1, ?2, ?3, ?4, ?4, ?5, 1, ?6)
         ON CONFLICT (doc_name, kind, editor) DO UPDATE SET
             first_at = MIN(first_at, excluded.first_at),
             last_at = MAX(last_at, excluded.last_at),
             last_wal_id = MAX(last_wal_id, excluded.last_wal_id),
             updates = updates + 1,
             pending = MAX(pending, excluded.pending)",
    )?;
    stmt.bind((1, doc_name))?;
    stmt.bind((2, kind.as_str()))?;
    stmt.bind((3, editor))?;
    stmt.bind((4, by.at_ms as i64))?;
    stmt.bind((5, wal_id as i64))?;
    stmt.bind((6, by.pending.leaves_pending as i64))?;
    stmt.next()?;
    Ok(())
}

pub(super) fn high_water(conn: &sqlite::Connection, doc_name: &str) -> Result<u64, sqlite::Error> {
    let mut stmt = conn.prepare(HIGH_WATER)?;
    stmt.bind((1, doc_name))?;
    stmt.next()?;
    Ok(stmt.read::<i64, _>(0)? as u64)
}

/// The record origin: updates after this WAL id are recorded. A document with
/// no origin row has either never been written (0) or holds content written
/// by no path that records (its high-water mark) — the fail-safe reading.
fn origin(conn: &sqlite::Connection, doc_name: &str) -> Result<u64, sqlite::Error> {
    let mut stmt =
        conn.prepare("SELECT recorded_after_wal_id FROM doc_editor_origin WHERE doc_name = ?1")?;
    stmt.bind((1, doc_name))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(stmt.read::<i64, _>(0)? as u64),
        sqlite::State::Done => high_water(conn, doc_name),
    }
}

pub(super) fn editors_since_wal(
    conn: &sqlite::Connection,
    doc_name: &str,
    after_wal_id: u64,
) -> Result<EditorsSince, StorageError> {
    let high_water = high_water(conn, doc_name)?;
    if after_wal_id > high_water {
        return Ok(EditorsSince::CutoffAhead { high_water });
    }
    let recorded_after_wal_id = origin(conn, doc_name)?;
    if after_wal_id < recorded_after_wal_id {
        return Ok(EditorsSince::Incomplete {
            recorded_after_wal_id,
        });
    }
    let mut stmt = conn.prepare(
        "SELECT kind, editor, first_at, last_at, last_wal_id, updates
         FROM doc_editors WHERE doc_name = ?1 AND last_wal_id > ?2
         ORDER BY last_wal_id, kind, editor",
    )?;
    stmt.bind((1, doc_name))?;
    stmt.bind((2, after_wal_id as i64))?;
    let mut out = Vec::new();
    while let sqlite::State::Row = stmt.next()? {
        out.push(read_editor_row(&stmt)?);
    }
    Ok(EditorsSince::Known(out))
}

/// Read one `doc_editors` row. An unknown `kind` is an error, never a guess.
fn read_editor_row(stmt: &sqlite::Statement<'_>) -> Result<EditorRecord, StorageError> {
    let kind_s = stmt.read::<String, _>("kind")?;
    let kind = EditorKind::parse(&kind_s)
        .ok_or_else(|| StorageError::Sqlite(format!("unknown editor kind '{kind_s}'")))?;
    Ok(EditorRecord {
        kind,
        editor: stmt.read::<String, _>("editor")?,
        first_at_ms: stmt.read::<i64, _>("first_at")? as u64,
        last_at_ms: stmt.read::<i64, _>("last_at")? as u64,
        last_wal_id: stmt.read::<i64, _>("last_wal_id")? as u64,
        updates: stmt.read::<i64, _>("updates")? as u64,
    })
}

/// Drop a document's record and origin.
pub(super) fn forget(conn: &sqlite::Connection, doc_name: &str) -> Result<(), sqlite::Error> {
    for table in ["doc_editors", "doc_editor_origin"] {
        let mut stmt = conn.prepare(format!("DELETE FROM {table} WHERE doc_name = ?1"))?;
        stmt.bind((1, doc_name))?;
        stmt.next()?;
    }
    Ok(())
}
