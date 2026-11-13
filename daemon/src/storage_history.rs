//! SQL for node version history (ADR-116 D1): one row per version, in the
//! same database as the WAL and the editor record. Only the record format and
//! its rules live in `mae_sync::kb::history`; this module stores and loads
//! what that module seals.
//!
//! A node's history is written only on its document's shard connection, the
//! one its WAL appends use, so a version cut for an update commits in that
//! update's transaction (see `cut_in_txn`).
//!
//! Like the editor record, history is not touched by `delete_document`: a
//! share or reseal recreates a node's content, and its history must survive.

use mae_sync::kb::history::{
    seal, Draft, Editor, EditorKind, Hash, HistoryEntry, HistoryKey, VersionKind,
};
use mae_sync::kb::MaterializedNode;

use super::editor_record;
use super::StorageError;
use crate::doc_editors::EditorsSince;

pub(super) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kb_node_history (
     kb_id TEXT NOT NULL,
     node_id TEXT NOT NULL,
     seq INTEGER NOT NULL,
     kind TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     editors TEXT NOT NULL,
     wal_from INTEGER NOT NULL,
     wal_to INTEGER NOT NULL,
     summary TEXT,
     name TEXT,
     restored_from INTEGER,
     content BLOB,
     content_hash BLOB NOT NULL,
     prev_hash BLOB NOT NULL,
     entry_hash BLOB NOT NULL,
     PRIMARY KEY (kb_id, node_id, seq)
 );
 CREATE TABLE IF NOT EXISTS kb_history_keys (
     kb_id TEXT PRIMARY KEY,
     key BLOB,
     released_at INTEGER
 );
 CREATE TABLE IF NOT EXISTS kb_node_history_session (
     kb_id TEXT NOT NULL,
     node_id TEXT NOT NULL,
     since_wal INTEGER NOT NULL,
     PRIMARY KEY (kb_id, node_id)
 );";

const COLUMNS: &str = "kb_id, node_id, seq, kind, created_at, editors, wal_from, wal_to, \
     summary, name, restored_from, content, content_hash, prev_hash, entry_hash";

/// A version to cut: which node, why, and the content it records (the node
/// as it stands at the cut). The editors and WAL range come from the record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    pub kb_id: String,
    pub node_id: String,
    pub kind: VersionKind,
    pub content: MaterializedNode,
    pub name: Option<String>,
    pub summary: Option<String>,
    pub restored_from: Option<u64>,
}

pub(super) fn ensure_schema(conn: &sqlite::Connection) -> Result<(), sqlite::Error> {
    conn.execute(SCHEMA)
}

/// The node's latest entry, if it has any.
pub(super) fn tail(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
) -> Result<Option<HistoryEntry>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {COLUMNS} FROM kb_node_history WHERE kb_id = ?1 AND node_id = ?2
         ORDER BY seq DESC LIMIT 1"
    ))?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, node_id))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(Some(read_row(&stmt)?)),
        sqlite::State::Done => Ok(None),
    }
}

/// Whether any version of any node of `kb_id` still holds content (has not
/// been pruned): what a switch to E2E would leave readable.
pub(super) fn holds_content(conn: &sqlite::Connection, kb_id: &str) -> Result<bool, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT 1 FROM kb_node_history WHERE kb_id = ?1 AND content IS NOT NULL LIMIT 1",
    )?;
    stmt.bind((1, kb_id))?;
    Ok(matches!(stmt.next()?, sqlite::State::Row))
}

/// Every entry of the node, in `seq` order.
pub(super) fn list(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
) -> Result<Vec<HistoryEntry>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {COLUMNS} FROM kb_node_history WHERE kb_id = ?1 AND node_id = ?2 ORDER BY seq"
    ))?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, node_id))?;
    let mut out = Vec::new();
    while let sqlite::State::Row = stmt.next()? {
        out.push(read_row(&stmt)?);
    }
    Ok(out)
}

/// The editors of the node's current session: those recorded since its last
/// cut (D2), the WAL point every cut moves the session marker to, whether it
/// wrote a version or found the content unchanged. Without the marker, a cut
/// skipped as unchanged would leave the old session open, and the next
/// handoff back to one of its editors would go unseen.
pub(super) fn session(
    conn: &sqlite::Connection,
    doc_name: &str,
    kb_id: &str,
    node_id: &str,
) -> Result<Vec<(EditorKind, String)>, StorageError> {
    let after = session_since(conn, kb_id, node_id)?;
    Ok(editors_after(conn, doc_name, after)?
        .into_iter()
        .map(|e| (e.kind, e.principal))
        .collect())
}

/// Where the node's current session began: its marker, or 0 for a node never
/// cut.
fn session_since(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
) -> Result<u64, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT since_wal FROM kb_node_history_session WHERE kb_id = ?1 AND node_id = ?2",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, node_id))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(stmt.read::<i64, _>(0)? as u64),
        sqlite::State::Done => Ok(0),
    }
}

fn mark_session(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
    since_wal: u64,
) -> Result<(), sqlite::Error> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO kb_node_history_session (kb_id, node_id, since_wal)
         VALUES (?1, ?2, ?3)",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, node_id))?;
    stmt.bind((3, since_wal as i64))?;
    stmt.next()?;
    Ok(())
}

/// The KB's history key for a cut, made in the cut's own transaction if the KB
/// has none: `INSERT OR IGNORE` under the transaction's write lock, then read
/// back, so concurrent first cuts on different nodes (different shards) all
/// get the one key the first of them made. `None` once the key is released
/// (the KB switched to E2E): the row stays as a marker, so no later cut
/// makes a new key, and the caller skips the cut rather than fail the write.
fn key_for_cut(conn: &sqlite::Connection, kb_id: &str) -> Result<Option<HistoryKey>, StorageError> {
    let mut make =
        conn.prepare("INSERT OR IGNORE INTO kb_history_keys (kb_id, key) VALUES (?1, ?2)")?;
    make.bind((1, kb_id))?;
    make.bind((2, &HistoryKey::generate().as_bytes()[..]))?;
    make.next()?;
    key(conn, kb_id)
}

/// The KB's history key, if it has one that is not released.
pub(super) fn key(
    conn: &sqlite::Connection,
    kb_id: &str,
) -> Result<Option<HistoryKey>, StorageError> {
    let mut stmt = conn.prepare("SELECT key FROM kb_history_keys WHERE kb_id = ?1")?;
    stmt.bind((1, kb_id))?;
    if stmt.next()? != sqlite::State::Row {
        return Ok(None);
    }
    match stmt.read::<Option<Vec<u8>>, _>(0)? {
        None => Ok(None),
        Some(bytes) => {
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| StorageError::Sqlite("history key: bad length".into()))?;
            Ok(Some(HistoryKey::from_bytes(bytes)))
        }
    }
}

/// The editors recorded after WAL id `after`. Where the record begins later
/// (`Incomplete`), the editors it does know: an unattributed stretch adds no
/// one, rather than failing the write that cuts the version.
fn editors_after(
    conn: &sqlite::Connection,
    doc_name: &str,
    after: u64,
) -> Result<Vec<Editor>, StorageError> {
    let rows = match editor_record::editors_since_wal(conn, doc_name, after)? {
        EditorsSince::Known(rows) => rows,
        EditorsSince::Incomplete {
            recorded_after_wal_id,
        } => match editor_record::editors_since_wal(conn, doc_name, recorded_after_wal_id)? {
            EditorsSince::Known(rows) => rows,
            _ => Vec::new(),
        },
        EditorsSince::CutoffAhead { .. } => Vec::new(),
    };
    Ok(rows
        .into_iter()
        .map(|r| Editor {
            kind: r.kind,
            principal: r.editor,
        })
        .collect())
}

/// Cut `cp` as the node's next version, inside the caller's transaction, at
/// time `now` (unix seconds). It covers the WAL after the previous version up
/// to the document's current high-water mark (`wal_from` exclusive, `wal_to`
/// inclusive), and the editors recorded there. Either way it starts a new
/// session.
/// Returns `None` without writing when the content equals the previous
/// version's and the cut is not a named one (a no-op save adds nothing, D2),
/// and when the KB's history key is released: an E2E KB is not versioned.
pub(super) fn cut_in_txn(
    conn: &sqlite::Connection,
    doc_name: &str,
    cp: Checkpoint,
    now: i64,
) -> Result<Option<HistoryEntry>, StorageError> {
    let Some(key) = key_for_cut(conn, &cp.kb_id)? else {
        return Ok(None);
    };
    let prev = tail(conn, &cp.kb_id, &cp.node_id)?;
    let wal_from = prev.as_ref().map_or(0, |p| p.wal_to);
    let wal_to = editor_record::high_water(conn, doc_name)?.max(wal_from);
    mark_session(conn, &cp.kb_id, &cp.node_id, wal_to)?;
    if cp.kind != VersionKind::Named
        && prev
            .as_ref()
            .is_some_and(|p| p.content_hash == key.content_hash(&cp.content))
    {
        return Ok(None);
    }
    let draft = Draft {
        kind: cp.kind,
        created_at: now,
        editors: editors_after(conn, doc_name, wal_from)?,
        wal_from,
        wal_to,
        summary: cp.summary,
        name: cp.name,
        restored_from: cp.restored_from,
        content: Some(cp.content),
    };
    let entry = seal(&cp.kb_id, &cp.node_id, prev.as_ref(), draft, &key)
        .map_err(|e| StorageError::Sqlite(format!("history: {e}")))?;
    insert(conn, &entry)?;
    Ok(Some(entry))
}

/// Cut the version `by` carries, if any, inside the caller's transaction:
/// the version an update cuts commits with that update (ADR-116 D2).
pub(super) fn cut_for(
    conn: &sqlite::Connection,
    doc_name: &str,
    by: &crate::doc_editors::EditStamp,
) -> Result<(), StorageError> {
    let Some(cp) = &by.checkpoint else {
        return Ok(());
    };
    if mae_sync::kb_node_doc_name(&cp.kb_id, &cp.node_id) != doc_name {
        return Err(StorageError::Sqlite(
            "history: a checkpoint names another document".into(),
        ));
    }
    cut_in_txn(conn, doc_name, cp.clone(), (by.at_ms / 1000) as i64)?;
    Ok(())
}

fn insert(conn: &sqlite::Connection, e: &HistoryEntry) -> Result<(), sqlite::Error> {
    let mut stmt = conn.prepare(format!(
        "INSERT INTO kb_node_history ({COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
    ))?;
    let editors: Vec<[&str; 2]> = e
        .editors
        .iter()
        .map(|ed| [ed.kind.as_str(), ed.principal.as_str()])
        .collect();
    let opt_text = |v: Option<&str>| v.map_or(sqlite::Value::Null, |s| s.into());
    stmt.bind((1, e.kb_id.as_str()))?;
    stmt.bind((2, e.node_id.as_str()))?;
    stmt.bind((3, e.seq as i64))?;
    stmt.bind((4, e.kind.as_str()))?;
    stmt.bind((5, e.created_at))?;
    stmt.bind((
        6,
        serde_json::to_string(&editors).expect("strings").as_str(),
    ))?;
    stmt.bind((7, e.wal_from as i64))?;
    stmt.bind((8, e.wal_to as i64))?;
    stmt.bind((9, opt_text(e.summary.as_deref())))?;
    stmt.bind((10, opt_text(e.name.as_deref())))?;
    stmt.bind((
        11,
        e.restored_from
            .map_or(sqlite::Value::Null, |s| (s as i64).into()),
    ))?;
    stmt.bind((
        12,
        e.content
            .as_ref()
            .map_or(sqlite::Value::Null, |c| c.clone().into()),
    ))?;
    stmt.bind((13, &e.content_hash[..]))?;
    stmt.bind((14, &e.prev_hash[..]))?;
    stmt.bind((15, &e.entry_hash[..]))?;
    stmt.next()?;
    Ok(())
}

/// One row as stored. A value this build cannot read (an unknown kind, a
/// malformed hash) is an error, never a guess; verification then reports the
/// chain as it stands.
fn read_row(stmt: &sqlite::Statement<'_>) -> Result<HistoryEntry, StorageError> {
    let bad = |what: &str| StorageError::Sqlite(format!("history row: bad {what}"));
    let hash = |col: &str| -> Result<Hash, StorageError> {
        stmt.read::<Vec<u8>, _>(col)?
            .try_into()
            .map_err(|_| bad(col))
    };
    let kind_s = stmt.read::<String, _>("kind")?;
    let editors_s = stmt.read::<String, _>("editors")?;
    let pairs: Vec<[String; 2]> = serde_json::from_str(&editors_s).map_err(|_| bad("editors"))?;
    let editors = pairs
        .into_iter()
        .map(|[k, p]| {
            EditorKind::parse(&k)
                .map(|kind| Editor { kind, principal: p })
                .ok_or_else(|| bad("editor kind"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(HistoryEntry {
        kb_id: stmt.read::<String, _>("kb_id")?,
        node_id: stmt.read::<String, _>("node_id")?,
        seq: stmt.read::<i64, _>("seq")? as u64,
        kind: VersionKind::parse(&kind_s).ok_or_else(|| bad("kind"))?,
        created_at: stmt.read::<i64, _>("created_at")?,
        editors,
        wal_from: stmt.read::<i64, _>("wal_from")? as u64,
        wal_to: stmt.read::<i64, _>("wal_to")? as u64,
        summary: stmt.read::<Option<String>, _>("summary")?,
        name: stmt.read::<Option<String>, _>("name")?,
        restored_from: stmt
            .read::<Option<i64>, _>("restored_from")?
            .map(|s| s as u64),
        content: stmt.read::<Option<Vec<u8>>, _>("content")?,
        content_hash: hash("content_hash")?,
        prev_hash: hash("prev_hash")?,
        entry_hash: hash("entry_hash")?,
    })
}
