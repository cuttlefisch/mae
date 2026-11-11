//! SQL for a seeded KB's provenance (see `seed::apply`).
//!
//! Two tables, local to this daemon and never synced (like the editor record
//! and ADR-106's version history: a mesh peer that receives the KB receives its
//! CRDT documents, not how they were first written here):
//! - `kb_seed`: one row per seeded KB: the commit, revision, plan hash and
//!   contract the seed was made from, when the apply started, and when it
//!   finished (`applied_at` is NULL while an apply is in progress or was cut
//!   short, which is how a re-run tells its own unfinished KB from another).
//! - `kb_seed_nodes`: one row per seeded node: its source path (where export
//!   writes it back), its blob and N(F) digest, the git times and their
//!   sources, and the WAL id of the write that installed it (edits after it are
//!   the node's working-copy history).
//!
//! A later ADR-113 build records the imported, in-force act from these rows
//! (Amendment 1, A4).

use super::StorageError;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kb_seed (
     kb_id TEXT PRIMARY KEY,
     plan_hash TEXT NOT NULL,
     contract TEXT NOT NULL,
     repo TEXT NOT NULL,
     rev TEXT NOT NULL,
     commit_sha TEXT NOT NULL,
     tree_sha TEXT NOT NULL,
     excluded_json TEXT NOT NULL,
     started_at INTEGER NOT NULL,
     applied_at INTEGER
 );
 CREATE TABLE IF NOT EXISTS kb_seed_nodes (
     kb_id TEXT NOT NULL,
     node_id TEXT NOT NULL,
     path TEXT NOT NULL,
     blob TEXT NOT NULL,
     normalised_sha256 TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     created_source TEXT NOT NULL,
     modified_at INTEGER NOT NULL,
     modified_source TEXT NOT NULL,
     seed_wal_id INTEGER,
     PRIMARY KEY (kb_id, node_id),
     UNIQUE (kb_id, path)
 );";

/// What a KB was seeded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedRecord {
    pub kb_id: String,
    pub plan_hash: String,
    pub contract: String,
    pub repo: String,
    pub rev: String,
    pub commit: String,
    pub tree: String,
    /// The `--exclude` paths, which a seed never leaves out silently.
    pub excluded: Vec<String>,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds; `None` until every document of the KB is written.
    pub applied_at: Option<i64>,
}

/// One seeded node's source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedNodeRow {
    pub node_id: String,
    /// Repo-relative path of the file at the seeded commit.
    pub path: String,
    pub blob: String,
    /// SHA-256 of N(F), the bytes an export of the unedited node writes.
    pub normalised_sha256: String,
    pub created_at: i64,
    /// `git-id`, `git` or `received` (`seed::TimeSource`).
    pub created_source: String,
    pub modified_at: i64,
    pub modified_source: String,
    /// The WAL id of the write that installed the node; `None` before it.
    pub seed_wal_id: Option<u64>,
}

pub(super) fn ensure_schema(conn: &sqlite::Connection) -> Result<(), sqlite::Error> {
    conn.execute(SCHEMA)
}

/// The record and node rows of `kb_id`, if it was seeded here.
pub(super) fn load(
    conn: &sqlite::Connection,
    kb_id: &str,
) -> Result<Option<(SeedRecord, Vec<SeedNodeRow>)>, StorageError> {
    let mut st = conn.prepare(
        "SELECT plan_hash, contract, repo, rev, commit_sha, tree_sha, excluded_json, \
         started_at, applied_at FROM kb_seed WHERE kb_id = ?1",
    )?;
    st.bind((1, kb_id))?;
    if st.next()? != sqlite::State::Row {
        return Ok(None);
    }
    let excluded: Vec<String> = serde_json::from_str(&st.read::<String, _>("excluded_json")?)
        .map_err(|e| StorageError::Sqlite(format!("kb_seed.excluded_json: {e}")))?;
    let record = SeedRecord {
        kb_id: kb_id.to_string(),
        plan_hash: st.read("plan_hash")?,
        contract: st.read("contract")?,
        repo: st.read("repo")?,
        rev: st.read("rev")?,
        commit: st.read("commit_sha")?,
        tree: st.read("tree_sha")?,
        excluded,
        started_at: st.read("started_at")?,
        applied_at: st.read::<Option<i64>, _>("applied_at")?,
    };
    Ok(Some((record, load_nodes(conn, kb_id, None)?)))
}

/// The node rows of `kb_id` (all of them, or the one `node_id`).
pub(super) fn load_nodes(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: Option<&str>,
) -> Result<Vec<SeedNodeRow>, StorageError> {
    let mut st = conn.prepare(
        "SELECT node_id, path, blob, normalised_sha256, created_at, created_source, \
         modified_at, modified_source, seed_wal_id FROM kb_seed_nodes \
         WHERE kb_id = ?1 AND (?2 IS NULL OR node_id = ?2) ORDER BY path",
    )?;
    st.bind((1, kb_id))?;
    st.bind((2, node_id))?;
    let mut rows = Vec::new();
    while st.next()? == sqlite::State::Row {
        rows.push(SeedNodeRow {
            node_id: st.read("node_id")?,
            path: st.read("path")?,
            blob: st.read("blob")?,
            normalised_sha256: st.read("normalised_sha256")?,
            created_at: st.read("created_at")?,
            created_source: st.read("created_source")?,
            modified_at: st.read("modified_at")?,
            modified_source: st.read("modified_source")?,
            seed_wal_id: st.read::<Option<i64>, _>("seed_wal_id")?.map(|w| w as u64),
        });
    }
    Ok(rows)
}

/// Insert the record (not yet applied) and every node row, in one
/// transaction. Fails, writing nothing, if `kb_id` already has a record.
pub(super) fn insert(
    conn: &sqlite::Connection,
    record: &SeedRecord,
    nodes: &[SeedNodeRow],
) -> Result<(), StorageError> {
    conn.execute("BEGIN IMMEDIATE")?;
    let result = insert_rows(conn, record, nodes);
    super::editor_record::commit_or_rollback(conn, result)
        .map_err(|e| StorageError::Sqlite(format!("kb_seed insert: {e}")))
}

fn insert_rows(
    conn: &sqlite::Connection,
    r: &SeedRecord,
    nodes: &[SeedNodeRow],
) -> Result<(), sqlite::Error> {
    let excluded = serde_json::to_string(&r.excluded).unwrap_or_else(|_| "[]".into());
    let mut st = conn.prepare(
        "INSERT INTO kb_seed (kb_id, plan_hash, contract, repo, rev, commit_sha, tree_sha, \
         excluded_json, started_at, applied_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
    )?;
    let texts = [
        &r.kb_id,
        &r.plan_hash,
        &r.contract,
        &r.repo,
        &r.rev,
        &r.commit,
        &r.tree,
        &excluded,
    ];
    for (i, v) in texts.iter().enumerate() {
        st.bind((i + 1, v.as_str()))?;
    }
    st.bind((9, r.started_at))?;
    st.next()?;
    for n in nodes {
        insert_node(conn, &r.kb_id, n)?;
    }
    Ok(())
}

fn insert_node(
    conn: &sqlite::Connection,
    kb_id: &str,
    n: &SeedNodeRow,
) -> Result<(), sqlite::Error> {
    let mut st = conn.prepare(
        "INSERT INTO kb_seed_nodes (kb_id, node_id, path, blob, normalised_sha256, created_at, \
         created_source, modified_at, modified_source, seed_wal_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, NULL)",
    )?;
    for (i, v) in [kb_id, &n.node_id, &n.path, &n.blob, &n.normalised_sha256]
        .iter()
        .enumerate()
    {
        st.bind((i + 1, *v))?;
    }
    st.bind((6, n.created_at))?;
    st.bind((7, n.created_source.as_str()))?;
    st.bind((8, n.modified_at))?;
    st.bind((9, n.modified_source.as_str()))?;
    st.next()?;
    Ok(())
}

/// Record the WAL id of the write that installed a seeded node.
pub(super) fn set_node_wal(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
    wal_id: u64,
) -> Result<(), StorageError> {
    let mut st = conn
        .prepare("UPDATE kb_seed_nodes SET seed_wal_id = ?3 WHERE kb_id = ?1 AND node_id = ?2")?;
    st.bind((1, kb_id))?;
    st.bind((2, node_id))?;
    st.bind((3, wal_id as i64))?;
    st.next()?;
    Ok(())
}

/// Mark the seed of `kb_id` complete.
pub(super) fn complete(
    conn: &sqlite::Connection,
    kb_id: &str,
    applied_at: i64,
) -> Result<(), StorageError> {
    let mut st = conn.prepare("UPDATE kb_seed SET applied_at = ?2 WHERE kb_id = ?1")?;
    st.bind((1, kb_id))?;
    st.bind((2, applied_at))?;
    st.next()?;
    Ok(())
}
