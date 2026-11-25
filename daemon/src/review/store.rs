//! SQL for publication review (ADR-113 D1): the per-KB policy, requests,
//! revisions, their frozen sources, document codes and the event chain, in the
//! daemon's `state.db` beside history and the editor record, so the existing
//! backup covers them. Hub-local, never replicated.
//!
//! Every function takes a connection the caller holds inside a transaction
//! (`DocStore::review_write` / `review_read`), so a method's reads and writes
//! are one atomic step. Nothing here decides who may act: that is `methods`.
//!
//! @ai-caution: [append-only] The event log and the frozen sources are never
//! updated or deleted: triggers refuse it, so a bug cannot rewrite the record.
//! They do not stop someone holding the database file (they can drop a
//! trigger); the chain's hashes and the daemon's signature are what show that.

use mae_sync::encoding::{base64_to_update as b64_decode, update_to_base64 as b64_encode};
use serde_json::Value;

use super::chain::{self, EventBody, Sealed, ZERO_HASH};
use crate::storage::StorageError;
use mae_mcp::identity::Identity;

pub(crate) const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kb_review_policy (
     kb_id TEXT PRIMARY KEY,
     review_enabled INTEGER NOT NULL,
     in_force_on_approval INTEGER NOT NULL,
     required_acts TEXT NOT NULL,
     approval_record TEXT NOT NULL,
     code_prefixes TEXT NOT NULL
 );
 CREATE TABLE IF NOT EXISTS kb_review_requests (
     ord INTEGER PRIMARY KEY AUTOINCREMENT,
     request_id TEXT NOT NULL UNIQUE,
     kb_id TEXT NOT NULL,
     set_name TEXT,
     in_force_on_approval INTEGER NOT NULL,
     reason TEXT NOT NULL,
     requester TEXT NOT NULL,
     requester_kind TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     ended_at INTEGER,
     end_reason TEXT
 );
 CREATE INDEX IF NOT EXISTS idx_review_requests_kb ON kb_review_requests(kb_id, ord);
 CREATE TABLE IF NOT EXISTS kb_review_revisions (
     ord INTEGER PRIMARY KEY AUTOINCREMENT,
     revision_id TEXT NOT NULL UNIQUE,
     kb_id TEXT NOT NULL,
     node_id TEXT NOT NULL,
     request_id TEXT NOT NULL,
     doc_code TEXT,
     rev_no INTEGER,
     source_sha256 TEXT NOT NULL,
     version TEXT NOT NULL,
     cutoff_wal INTEGER NOT NULL,
     history_seq INTEGER NOT NULL,
     state TEXT NOT NULL,
     doc_type TEXT,
     template_id TEXT,
     template_sha256 TEXT,
     approval_record TEXT,
     replaces TEXT,
     created_by TEXT NOT NULL,
     created_by_kind TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     approved_at INTEGER,
     in_force_since INTEGER,
     ended_at INTEGER
 );
 CREATE INDEX IF NOT EXISTS idx_review_revisions_node ON kb_review_revisions(kb_id, node_id, ord);
 CREATE INDEX IF NOT EXISTS idx_review_revisions_request ON kb_review_revisions(request_id);
 CREATE TABLE IF NOT EXISTS kb_review_sources (
     kb_id TEXT NOT NULL,
     source_sha256 TEXT NOT NULL,
     content BLOB NOT NULL,
     PRIMARY KEY (kb_id, source_sha256)
 );
 CREATE TABLE IF NOT EXISTS kb_review_codes (
     kb_id TEXT NOT NULL,
     prefix TEXT NOT NULL,
     number INTEGER NOT NULL,
     node_id TEXT NOT NULL,
     assigned_seq INTEGER NOT NULL,
     PRIMARY KEY (kb_id, prefix, number)
 );
 CREATE INDEX IF NOT EXISTS idx_review_codes_node ON kb_review_codes(kb_id, node_id, assigned_seq);
 CREATE TABLE IF NOT EXISTS kb_review_events (
     kb_id TEXT NOT NULL,
     seq INTEGER NOT NULL,
     kind TEXT NOT NULL,
     at TEXT NOT NULL,
     actor TEXT NOT NULL,
     payload TEXT NOT NULL,
     prev_hash TEXT NOT NULL,
     entry_hash TEXT NOT NULL,
     sig TEXT NOT NULL,
     member_sig TEXT,
     PRIMARY KEY (kb_id, seq)
 );
 CREATE TRIGGER IF NOT EXISTS kb_review_events_no_update BEFORE UPDATE ON kb_review_events
 BEGIN SELECT RAISE(ABORT, 'the review event log is append-only'); END;
 CREATE TRIGGER IF NOT EXISTS kb_review_events_no_delete BEFORE DELETE ON kb_review_events
 BEGIN SELECT RAISE(ABORT, 'the review event log is append-only'); END;
 CREATE TRIGGER IF NOT EXISTS kb_review_sources_no_update BEFORE UPDATE ON kb_review_sources
 BEGIN SELECT RAISE(ABORT, 'a frozen source is immutable'); END;
 CREATE TRIGGER IF NOT EXISTS kb_review_sources_no_delete BEFORE DELETE ON kb_review_sources
 BEGIN SELECT RAISE(ABORT, 'a frozen source is immutable'); END;";

pub(crate) fn ensure_schema(conn: &sqlite::Connection) -> Result<(), sqlite::Error> {
    conn.execute(SCHEMA)
}

fn bad(what: &str) -> StorageError {
    StorageError::Sqlite(format!("review row: bad {what}"))
}

fn opt_text(v: Option<&str>) -> sqlite::Value {
    v.map_or(sqlite::Value::Null, |s| s.into())
}

fn opt_int(v: Option<i64>) -> sqlite::Value {
    v.map_or(sqlite::Value::Null, sqlite::Value::Integer)
}

// ---------------------------------------------------------------- policy

/// A KB's review policy (D3, E3, E9). The fields later PRs add (lists,
/// second factor, retention) are not stored until they are built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub review_enabled: bool,
    pub in_force_on_approval: bool,
    pub required_acts: Vec<String>,
    pub approval_record: String,
    pub code_prefixes: Vec<String>,
}

impl Default for Policy {
    /// Review off (D3: what a personal KB wants), one `approved` act, the
    /// in-system act as the record of approval (B7), no code prefixes.
    fn default() -> Self {
        Policy {
            review_enabled: false,
            in_force_on_approval: false,
            required_acts: vec!["approved".into()],
            approval_record: "act".into(),
            code_prefixes: Vec::new(),
        }
    }
}

pub(crate) fn policy(conn: &sqlite::Connection, kb_id: &str) -> Result<Policy, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT review_enabled, in_force_on_approval, required_acts, approval_record,
                code_prefixes FROM kb_review_policy WHERE kb_id = ?1",
    )?;
    stmt.bind((1, kb_id))?;
    if stmt.next()? != sqlite::State::Row {
        return Ok(Policy::default());
    }
    let list = |col: &str| -> Result<Vec<String>, StorageError> {
        serde_json::from_str(&stmt.read::<String, _>(col)?).map_err(|_| bad(col))
    };
    Ok(Policy {
        review_enabled: stmt.read::<i64, _>("review_enabled")? != 0,
        in_force_on_approval: stmt.read::<i64, _>("in_force_on_approval")? != 0,
        required_acts: list("required_acts")?,
        approval_record: stmt.read::<String, _>("approval_record")?,
        code_prefixes: list("code_prefixes")?,
    })
}

pub(crate) fn put_policy(
    conn: &sqlite::Connection,
    kb_id: &str,
    p: &Policy,
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "INSERT OR REPLACE INTO kb_review_policy
         (kb_id, review_enabled, in_force_on_approval, required_acts, approval_record, code_prefixes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, i64::from(p.review_enabled)))?;
    stmt.bind((3, i64::from(p.in_force_on_approval)))?;
    stmt.bind((
        4,
        serde_json::to_string(&p.required_acts)
            .expect("strings")
            .as_str(),
    ))?;
    stmt.bind((5, p.approval_record.as_str()))?;
    stmt.bind((
        6,
        serde_json::to_string(&p.code_prefixes)
            .expect("strings")
            .as_str(),
    ))?;
    stmt.next()?;
    Ok(())
}

// ---------------------------------------------------------------- revisions

/// One revision row (D2, PR-4's data list).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub revision_id: String,
    pub kb_id: String,
    pub node_id: String,
    pub request_id: String,
    pub doc_code: Option<String>,
    pub rev_no: Option<i64>,
    pub source_sha256: String,
    pub version: String,
    /// A2: the document's WAL high-water mark read with the frozen content.
    pub cutoff_wal: u64,
    /// O8: the ADR-116 `review_freeze` version cut at the freeze.
    pub history_seq: u64,
    pub state: String,
    /// C1 and C7, filled by later PRs; part of the row from the start.
    pub doc_type: Option<String>,
    pub template_id: Option<String>,
    pub template_sha256: Option<String>,
    /// E3: the policy in force when the revision was approved (PR-5).
    pub approval_record: Option<String>,
    pub replaces: Option<String>,
    pub created_by: String,
    pub created_by_kind: String,
    /// Unix milliseconds, daemon clock.
    pub created_at: i64,
    pub approved_at: Option<i64>,
    pub in_force_since: Option<i64>,
    pub ended_at: Option<i64>,
}

const REV_COLUMNS: &str = "revision_id, kb_id, node_id, request_id, doc_code, rev_no, \
     source_sha256, version, cutoff_wal, history_seq, state, doc_type, template_id, \
     template_sha256, approval_record, replaces, created_by, created_by_kind, created_at, \
     approved_at, in_force_since, ended_at";

fn read_revision(stmt: &sqlite::Statement<'_>) -> Result<Revision, StorageError> {
    let s = |c: &str| stmt.read::<String, _>(c).map_err(StorageError::from);
    let os = |c: &str| {
        stmt.read::<Option<String>, _>(c)
            .map_err(StorageError::from)
    };
    let oi = |c: &str| stmt.read::<Option<i64>, _>(c).map_err(StorageError::from);
    let u = |c: &str| -> Result<u64, StorageError> {
        u64::try_from(stmt.read::<i64, _>(c)?).map_err(|_| bad(c))
    };
    Ok(Revision {
        revision_id: s("revision_id")?,
        kb_id: s("kb_id")?,
        node_id: s("node_id")?,
        request_id: s("request_id")?,
        doc_code: os("doc_code")?,
        rev_no: oi("rev_no")?,
        source_sha256: s("source_sha256")?,
        version: s("version")?,
        cutoff_wal: u("cutoff_wal")?,
        history_seq: u("history_seq")?,
        state: s("state")?,
        doc_type: os("doc_type")?,
        template_id: os("template_id")?,
        template_sha256: os("template_sha256")?,
        approval_record: os("approval_record")?,
        replaces: os("replaces")?,
        created_by: s("created_by")?,
        created_by_kind: s("created_by_kind")?,
        created_at: stmt.read::<i64, _>("created_at")?,
        approved_at: oi("approved_at")?,
        in_force_since: oi("in_force_since")?,
        ended_at: oi("ended_at")?,
    })
}

pub(crate) fn insert_revision(conn: &sqlite::Connection, r: &Revision) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(format!(
        "INSERT INTO kb_review_revisions ({REV_COLUMNS})
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20, ?21, ?22)"
    ))?;
    let values: [sqlite::Value; 22] = [
        r.revision_id.as_str().into(),
        r.kb_id.as_str().into(),
        r.node_id.as_str().into(),
        r.request_id.as_str().into(),
        opt_text(r.doc_code.as_deref()),
        opt_int(r.rev_no),
        r.source_sha256.as_str().into(),
        r.version.as_str().into(),
        sqlite::Value::Integer(r.cutoff_wal as i64),
        sqlite::Value::Integer(r.history_seq as i64),
        r.state.as_str().into(),
        opt_text(r.doc_type.as_deref()),
        opt_text(r.template_id.as_deref()),
        opt_text(r.template_sha256.as_deref()),
        opt_text(r.approval_record.as_deref()),
        opt_text(r.replaces.as_deref()),
        r.created_by.as_str().into(),
        r.created_by_kind.as_str().into(),
        sqlite::Value::Integer(r.created_at),
        opt_int(r.approved_at),
        opt_int(r.in_force_since),
        opt_int(r.ended_at),
    ];
    for (i, v) in values.into_iter().enumerate() {
        stmt.bind((i + 1, v))?;
    }
    stmt.next()?;
    Ok(())
}

/// A node's revisions in the order they were made.
pub(crate) fn node_revisions(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
) -> Result<Vec<Revision>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {REV_COLUMNS} FROM kb_review_revisions WHERE kb_id = ?1 AND node_id = ?2 ORDER BY ord"
    ))?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, node_id))?;
    collect(&mut stmt)
}

/// A request's member revisions, in request order.
pub(crate) fn request_revisions(
    conn: &sqlite::Connection,
    request_id: &str,
) -> Result<Vec<Revision>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {REV_COLUMNS} FROM kb_review_revisions WHERE request_id = ?1 ORDER BY ord"
    ))?;
    stmt.bind((1, request_id))?;
    collect(&mut stmt)
}

/// One revision of `kb_id`, by id. Another KB's revision is `None`.
pub(crate) fn revision(
    conn: &sqlite::Connection,
    kb_id: &str,
    revision_id: &str,
) -> Result<Option<Revision>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {REV_COLUMNS} FROM kb_review_revisions WHERE kb_id = ?1 AND revision_id = ?2"
    ))?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, revision_id))?;
    Ok(collect(&mut stmt)?.into_iter().next())
}

fn collect(stmt: &mut sqlite::Statement<'_>) -> Result<Vec<Revision>, StorageError> {
    let mut out = Vec::new();
    while let sqlite::State::Row = stmt.next()? {
        out.push(read_revision(stmt)?);
    }
    Ok(out)
}

/// Every revision of `kb_id` whose node is `node_id` and whose state is one
/// of `states`, oldest first.
pub(crate) fn node_revisions_in(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
    states: &[&str],
) -> Result<Vec<Revision>, StorageError> {
    Ok(node_revisions(conn, kb_id, node_id)?
        .into_iter()
        .filter(|r| states.contains(&r.state.as_str()))
        .collect())
}

/// Set every revision of a request to `state`, ending it at `at`.
pub(crate) fn end_request_revisions(
    conn: &sqlite::Connection,
    request_id: &str,
    state: &str,
    at: i64,
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "UPDATE kb_review_revisions SET state = ?1, ended_at = ?2 WHERE request_id = ?3",
    )?;
    stmt.bind((1, state))?;
    stmt.bind((2, at))?;
    stmt.bind((3, request_id))?;
    stmt.next()?;
    Ok(())
}

// ---------------------------------------------------------------- sources

/// Store a frozen source under its sha256, once per KB (content-addressed).
pub(crate) fn put_source(
    conn: &sqlite::Connection,
    kb_id: &str,
    sha256: &str,
    content: &[u8],
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "INSERT OR IGNORE INTO kb_review_sources (kb_id, source_sha256, content) VALUES (?1, ?2, ?3)",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, sha256))?;
    stmt.bind((3, content))?;
    stmt.next()?;
    Ok(())
}

pub(crate) fn source(
    conn: &sqlite::Connection,
    kb_id: &str,
    sha256: &str,
) -> Result<Option<Vec<u8>>, StorageError> {
    let mut stmt = conn
        .prepare("SELECT content FROM kb_review_sources WHERE kb_id = ?1 AND source_sha256 = ?2")?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, sha256))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(Some(stmt.read::<Vec<u8>, _>(0)?)),
        sqlite::State::Done => Ok(None),
    }
}

/// Every frozen source `kb_id` holds, as `(sha256, content)`: the sweep's
/// revision holders, whatever their revisions' states.
pub(crate) fn sources(
    conn: &sqlite::Connection,
    kb_id: &str,
) -> Result<Vec<(String, Vec<u8>)>, StorageError> {
    let mut stmt = conn
        .prepare("SELECT source_sha256, content FROM kb_review_sources WHERE kb_id = ?1 ORDER BY source_sha256")?;
    stmt.bind((1, kb_id))?;
    let mut out = Vec::new();
    while let sqlite::State::Row = stmt.next()? {
        out.push((stmt.read::<String, _>(0)?, stmt.read::<Vec<u8>, _>(1)?));
    }
    Ok(out)
}

/// Whether `kb_id` holds any revision, in any state.
pub(crate) fn has_revisions(conn: &sqlite::Connection, kb_id: &str) -> Result<bool, StorageError> {
    let mut stmt = conn.prepare("SELECT 1 FROM kb_review_revisions WHERE kb_id = ?1 LIMIT 1")?;
    stmt.bind((1, kb_id))?;
    Ok(stmt.next()? == sqlite::State::Row)
}

// ---------------------------------------------------------------- requests

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub ord: i64,
    pub request_id: String,
    pub kb_id: String,
    pub set_name: Option<String>,
    pub in_force_on_approval: bool,
    pub reason: String,
    pub requester: String,
    pub requester_kind: String,
    pub created_at: i64,
    pub ended_at: Option<i64>,
    pub end_reason: Option<String>,
}

const REQ_COLUMNS: &str = "ord, request_id, kb_id, set_name, in_force_on_approval, reason, \
     requester, requester_kind, created_at, ended_at, end_reason";

fn read_request(stmt: &sqlite::Statement<'_>) -> Result<Request, StorageError> {
    Ok(Request {
        ord: stmt.read::<i64, _>("ord")?,
        request_id: stmt.read::<String, _>("request_id")?,
        kb_id: stmt.read::<String, _>("kb_id")?,
        set_name: stmt.read::<Option<String>, _>("set_name")?,
        in_force_on_approval: stmt.read::<i64, _>("in_force_on_approval")? != 0,
        reason: stmt.read::<String, _>("reason")?,
        requester: stmt.read::<String, _>("requester")?,
        requester_kind: stmt.read::<String, _>("requester_kind")?,
        created_at: stmt.read::<i64, _>("created_at")?,
        ended_at: stmt.read::<Option<i64>, _>("ended_at")?,
        end_reason: stmt.read::<Option<String>, _>("end_reason")?,
    })
}

/// Insert a request; `r.ord` is ignored (the table assigns it).
pub(crate) fn insert_request(conn: &sqlite::Connection, r: &Request) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "INSERT INTO kb_review_requests (request_id, kb_id, set_name, in_force_on_approval,
             reason, requester, requester_kind, created_at, ended_at, end_reason)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, NULL)",
    )?;
    stmt.bind((1, r.request_id.as_str()))?;
    stmt.bind((2, r.kb_id.as_str()))?;
    stmt.bind((3, opt_text(r.set_name.as_deref())))?;
    stmt.bind((4, i64::from(r.in_force_on_approval)))?;
    stmt.bind((5, r.reason.as_str()))?;
    stmt.bind((6, r.requester.as_str()))?;
    stmt.bind((7, r.requester_kind.as_str()))?;
    stmt.bind((8, r.created_at))?;
    stmt.next()?;
    Ok(())
}

/// One request of `kb_id`, by id. Another KB's request is `None`.
pub(crate) fn request(
    conn: &sqlite::Connection,
    kb_id: &str,
    request_id: &str,
) -> Result<Option<Request>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {REQ_COLUMNS} FROM kb_review_requests WHERE kb_id = ?1 AND request_id = ?2"
    ))?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, request_id))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(Some(read_request(&stmt)?)),
        sqlite::State::Done => Ok(None),
    }
}

/// Whether `ord` is a request of `kb_id` (a cursor this KB issued).
pub(crate) fn request_ord_exists(
    conn: &sqlite::Connection,
    kb_id: &str,
    ord: i64,
) -> Result<bool, StorageError> {
    let mut stmt =
        conn.prepare("SELECT 1 FROM kb_review_requests WHERE kb_id = ?1 AND ord = ?2")?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, ord))?;
    Ok(stmt.next()? == sqlite::State::Row)
}

/// Up to `limit` open requests of `kb_id` after `after` (ord), oldest first.
pub(crate) fn open_requests(
    conn: &sqlite::Connection,
    kb_id: &str,
    after: i64,
    limit: usize,
) -> Result<Vec<Request>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {REQ_COLUMNS} FROM kb_review_requests
         WHERE kb_id = ?1 AND ended_at IS NULL AND ord > ?2 ORDER BY ord LIMIT ?3"
    ))?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, after))?;
    stmt.bind((3, limit as i64))?;
    let mut out = Vec::new();
    while let sqlite::State::Row = stmt.next()? {
        out.push(read_request(&stmt)?);
    }
    Ok(out)
}

pub(crate) fn end_request(
    conn: &sqlite::Connection,
    request_id: &str,
    at: i64,
    reason: &str,
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "UPDATE kb_review_requests SET ended_at = ?1, end_reason = ?2 WHERE request_id = ?3",
    )?;
    stmt.bind((1, at))?;
    stmt.bind((2, reason))?;
    stmt.bind((3, request_id))?;
    stmt.next()?;
    Ok(())
}

// ---------------------------------------------------------------- codes

/// The node holding `(prefix, number)` in `kb_id`, if any.
pub(crate) fn code_holder(
    conn: &sqlite::Connection,
    kb_id: &str,
    prefix: &str,
    number: i64,
) -> Result<Option<String>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT node_id FROM kb_review_codes WHERE kb_id = ?1 AND prefix = ?2 AND number = ?3",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, prefix))?;
    stmt.bind((3, number))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(Some(stmt.read::<String, _>(0)?)),
        sqlite::State::Done => Ok(None),
    }
}

/// The node's current code: the one assigned to it last.
pub(crate) fn node_code(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
) -> Result<Option<String>, StorageError> {
    let mut stmt = conn.prepare(
        "SELECT prefix, number FROM kb_review_codes WHERE kb_id = ?1 AND node_id = ?2
         ORDER BY assigned_seq DESC LIMIT 1",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, node_id))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(Some(format!(
            "{}-{}",
            stmt.read::<String, _>(0)?,
            stmt.read::<i64, _>(1)?
        ))),
        sqlite::State::Done => Ok(None),
    }
}

/// Assign `(prefix, number)` to `node_id`, at the event `seq` that records it.
/// A code once assigned is never freed: a number names one document for good.
/// Assigning the node the number it already holds again moves it back to
/// being the node's current code.
pub(crate) fn assign_code(
    conn: &sqlite::Connection,
    kb_id: &str,
    prefix: &str,
    number: i64,
    node_id: &str,
    seq: u64,
) -> Result<(), StorageError> {
    let mut stmt = conn.prepare(
        "INSERT INTO kb_review_codes (kb_id, prefix, number, node_id, assigned_seq)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (kb_id, prefix, number) DO UPDATE SET assigned_seq = excluded.assigned_seq
         WHERE kb_review_codes.node_id = excluded.node_id",
    )?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, prefix))?;
    stmt.bind((3, number))?;
    stmt.bind((4, node_id))?;
    stmt.bind((5, seq as i64))?;
    stmt.next()?;
    Ok(())
}

// ---------------------------------------------------------------- events

/// One stored event row: E10's event, column by column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub kb_id: String,
    pub seq: u64,
    pub kind: String,
    pub at: String,
    pub actor: String,
    pub payload: String,
    pub prev_hash: String,
    pub entry_hash: String,
    pub sig: String,
    pub member_sig: Option<String>,
}

impl EventRow {
    /// The event as its columns say it is. The canonical bytes an export
    /// carries are rebuilt from these, never stored beside them, so a changed
    /// column changes what is hashed and the stored `entry_hash` no longer
    /// matches.
    pub fn body(&self) -> Result<EventBody, StorageError> {
        Ok(EventBody {
            kb_id: self.kb_id.clone(),
            seq: self.seq,
            prev_hash: self.prev_hash.clone(),
            kind: self.kind.clone(),
            at: self.at.clone(),
            actor: serde_json::from_str(&self.actor).map_err(|_| bad("actor"))?,
            payload: serde_json::from_str(&self.payload).map_err(|_| bad("payload"))?,
            member_sig: self.member_sig.clone(),
        })
    }

    /// Its export line (E10), from the rebuilt canonical bytes and the stored
    /// hash and signature.
    pub fn export_line(&self) -> Result<Value, StorageError> {
        let canonical = chain::canonical(&self.body()?.to_value())
            .map_err(|e| StorageError::Sqlite(format!("review event {}: {e}", self.seq)))?;
        let entry_hash: [u8; 32] = hex::decode(&self.entry_hash)
            .ok()
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| bad("entry_hash"))?;
        let sig = b64_decode(&self.sig).map_err(|_| bad("sig"))?;
        Ok(chain::event_line(
            self.seq,
            &self.prev_hash,
            &Sealed {
                canonical,
                entry_hash,
                sig,
            },
        ))
    }
}

const EVENT_COLUMNS: &str =
    "kb_id, seq, kind, at, actor, payload, prev_hash, entry_hash, sig, member_sig";

fn read_event(stmt: &sqlite::Statement<'_>) -> Result<EventRow, StorageError> {
    Ok(EventRow {
        kb_id: stmt.read::<String, _>("kb_id")?,
        seq: u64::try_from(stmt.read::<i64, _>("seq")?).map_err(|_| bad("seq"))?,
        kind: stmt.read::<String, _>("kind")?,
        at: stmt.read::<String, _>("at")?,
        actor: stmt.read::<String, _>("actor")?,
        payload: stmt.read::<String, _>("payload")?,
        prev_hash: stmt.read::<String, _>("prev_hash")?,
        entry_hash: stmt.read::<String, _>("entry_hash")?,
        sig: stmt.read::<String, _>("sig")?,
        member_sig: stmt.read::<Option<String>, _>("member_sig")?,
    })
}

/// The last event of `kb_id`, if any.
pub(crate) fn last_event(
    conn: &sqlite::Connection,
    kb_id: &str,
) -> Result<Option<EventRow>, StorageError> {
    let mut stmt = conn.prepare(format!(
        "SELECT {EVENT_COLUMNS} FROM kb_review_events WHERE kb_id = ?1 ORDER BY seq DESC LIMIT 1"
    ))?;
    stmt.bind((1, kb_id))?;
    match stmt.next()? {
        sqlite::State::Row => Ok(Some(read_event(&stmt)?)),
        sqlite::State::Done => Ok(None),
    }
}

/// Up to `limit` events of `kb_id` with `seq > after`, in order (`limit` 0:
/// every one).
pub(crate) fn events(
    conn: &sqlite::Connection,
    kb_id: &str,
    after: u64,
    limit: usize,
) -> Result<Vec<EventRow>, StorageError> {
    let limit = if limit == 0 { -1 } else { limit as i64 };
    let mut stmt = conn.prepare(format!(
        "SELECT {EVENT_COLUMNS} FROM kb_review_events WHERE kb_id = ?1 AND seq > ?2
         ORDER BY seq LIMIT ?3"
    ))?;
    stmt.bind((1, kb_id))?;
    stmt.bind((2, after as i64))?;
    stmt.bind((3, limit))?;
    let mut out = Vec::new();
    while let sqlite::State::Row = stmt.next()? {
        out.push(read_event(&stmt)?);
    }
    Ok(out)
}

/// What an appended event was given.
pub(crate) struct NewEvent<'a> {
    pub kb_id: &'a str,
    pub kind: &'a str,
    pub at: String,
    pub actor: Value,
    pub payload: Value,
}

/// Append the KB's next event: `seq` and `prev_hash` follow its last event,
/// and the daemon signs it. Inside the caller's write transaction, so the
/// event commits with the change it records, or neither does. Returns its
/// `seq`.
pub(crate) fn append_event(
    conn: &sqlite::Connection,
    signer: &Identity,
    e: NewEvent<'_>,
) -> Result<u64, StorageError> {
    let (seq, prev_hash) = match last_event(conn, e.kb_id)? {
        Some(last) => (last.seq + 1, last.entry_hash),
        None => (1, ZERO_HASH.to_string()),
    };
    let body = EventBody {
        kb_id: e.kb_id.to_string(),
        seq,
        prev_hash: prev_hash.clone(),
        kind: e.kind.to_string(),
        at: e.at,
        actor: e.actor,
        payload: e.payload,
        member_sig: None,
    };
    let canon = |v: &Value| -> Result<String, StorageError> {
        chain::canonical(v)
            .map(|b| String::from_utf8(b).expect("canonical JSON is UTF-8"))
            .map_err(|err| StorageError::Sqlite(format!("review event: {err}")))
    };
    let actor = canon(&body.actor)?;
    let payload = canon(&body.payload)?;
    let sealed = chain::seal(&body, signer)
        .map_err(|err| StorageError::Sqlite(format!("review event: {err}")))?;
    let mut stmt = conn.prepare(format!(
        "INSERT INTO kb_review_events ({EVENT_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)"
    ))?;
    stmt.bind((1, e.kb_id))?;
    stmt.bind((2, seq as i64))?;
    stmt.bind((3, body.kind.as_str()))?;
    stmt.bind((4, body.at.as_str()))?;
    stmt.bind((5, actor.as_str()))?;
    stmt.bind((6, payload.as_str()))?;
    stmt.bind((7, prev_hash.as_str()))?;
    stmt.bind((8, hex::encode(sealed.entry_hash).as_str()))?;
    stmt.bind((9, b64_encode(&sealed.sig).as_str()))?;
    stmt.next()?;
    Ok(seq)
}

/// The opaque id of event `seq` (E9).
pub fn event_id(seq: u64) -> String {
    format!("ev-{seq:010}")
}
