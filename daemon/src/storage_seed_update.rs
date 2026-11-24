//! SQL for a seed UPDATE in progress (see `seed::update`).
//!
//! An update moves a seeded KB from the plan in `kb_seed` to a new one. Its
//! intent is recorded first, in ONE transaction, in two tables of its own:
//! - `kb_seed_update`: the new plan's hash and source, one row per KB with an
//!   update in progress (its presence is "not yet applied");
//! - `kb_seed_update_nodes`: one row per node the update touches or keeps:
//!   every node of the new plan with its new source row, and every node it
//!   removes (`remove = 1`, with its old row). `base_version` is the node's
//!   version when the update began, which a resumed update checks again;
//!   `done` is set, with the WAL id and version the node was left at, once
//!   its document is written (or found already written).
//!
//! `kb_seed` and `kb_seed_nodes` keep describing the LAST APPLIED plan until
//! [`finish`], which replaces them from these rows and deletes these rows, in
//! one transaction. So the record of what the seed wrote is never half one
//! plan and half the other, and the pending rows need no unique path: a
//! removed node and a new one may share a path within one update.

use super::seed_record::SeedNodeRow;
use super::StorageError;

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS kb_seed_update (
     kb_id TEXT PRIMARY KEY,
     plan_hash TEXT NOT NULL,
     repo TEXT NOT NULL,
     rev TEXT NOT NULL,
     commit_sha TEXT NOT NULL,
     tree_sha TEXT NOT NULL,
     excluded_json TEXT NOT NULL,
     started_at INTEGER NOT NULL
 );
 CREATE TABLE IF NOT EXISTS kb_seed_update_nodes (
     kb_id TEXT NOT NULL,
     node_id TEXT NOT NULL,
     remove INTEGER NOT NULL,
     base_version TEXT,
     path TEXT NOT NULL,
     blob TEXT NOT NULL,
     normalised_sha256 TEXT NOT NULL,
     created_at INTEGER NOT NULL,
     created_source TEXT NOT NULL,
     modified_at INTEGER NOT NULL,
     modified_source TEXT NOT NULL,
     done INTEGER NOT NULL DEFAULT 0,
     seed_wal_id INTEGER,
     seed_version TEXT,
     PRIMARY KEY (kb_id, node_id)
 );";

/// An update in progress: the plan a seeded KB is being moved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedUpdate {
    pub kb_id: String,
    pub plan_hash: String,
    pub repo: String,
    pub rev: String,
    pub commit: String,
    pub tree: String,
    pub excluded: Vec<String>,
    /// Unix seconds.
    pub started_at: i64,
}

/// One node of an update in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedUpdateNode {
    /// The node's row once the update is applied (for a removal: its old
    /// row). `seed_wal_id`/`seed_version` are what the update left it at, set
    /// when `done`.
    pub row: SeedNodeRow,
    /// The update removes this node from the KB.
    pub remove: bool,
    /// The node document's `version()` when the update began; `None` when
    /// there was no document.
    pub base_version: Option<String>,
    pub done: bool,
}

pub(super) fn ensure_schema(conn: &sqlite::Connection) -> Result<(), sqlite::Error> {
    conn.execute(SCHEMA)
}

/// Record an update's intent, not yet applied, in one transaction. Fails,
/// writing nothing, if the KB has no seed record or already has an update in
/// progress.
pub(super) fn begin(
    conn: &sqlite::Connection,
    u: &SeedUpdate,
    nodes: &[SeedUpdateNode],
) -> Result<(), StorageError> {
    conn.execute("BEGIN IMMEDIATE")?;
    let result = begin_rows(conn, u, nodes);
    super::editor_record::commit_or_rollback(conn, result)
        .map_err(|e| StorageError::Sqlite(format!("kb_seed_update begin: {e}")))
}

fn begin_rows(
    conn: &sqlite::Connection,
    u: &SeedUpdate,
    nodes: &[SeedUpdateNode],
) -> Result<(), sqlite::Error> {
    let mut seeded = conn.prepare("SELECT 1 FROM kb_seed WHERE kb_id = ?1")?;
    seeded.bind((1, u.kb_id.as_str()))?;
    if seeded.next()? != sqlite::State::Row {
        return Err(sql_error("the KB has no seed record"));
    }
    let excluded = serde_json::to_string(&u.excluded).unwrap_or_else(|_| "[]".into());
    let mut st = conn.prepare(
        "INSERT INTO kb_seed_update (kb_id, plan_hash, repo, rev, commit_sha, tree_sha, \
         excluded_json, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )?;
    let texts = [
        &u.kb_id,
        &u.plan_hash,
        &u.repo,
        &u.rev,
        &u.commit,
        &u.tree,
        &excluded,
    ];
    for (i, v) in texts.iter().enumerate() {
        st.bind((i + 1, v.as_str()))?;
    }
    st.bind((8, u.started_at))?;
    st.next()?;
    for n in nodes {
        insert_node(conn, &u.kb_id, n)?;
    }
    Ok(())
}

fn sql_error(message: &str) -> sqlite::Error {
    sqlite::Error {
        code: None,
        message: Some(message.to_string()),
    }
}

fn insert_node(
    conn: &sqlite::Connection,
    kb_id: &str,
    n: &SeedUpdateNode,
) -> Result<(), sqlite::Error> {
    let r = &n.row;
    let mut st = conn.prepare(
        "INSERT INTO kb_seed_update_nodes (kb_id, node_id, remove, base_version, path, blob, \
         normalised_sha256, created_at, created_source, modified_at, modified_source, done) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0)",
    )?;
    st.bind((1, kb_id))?;
    st.bind((2, r.node_id.as_str()))?;
    st.bind((3, i64::from(n.remove)))?;
    st.bind((4, n.base_version.as_deref()))?;
    for (i, v) in [&r.path, &r.blob, &r.normalised_sha256].iter().enumerate() {
        st.bind((i + 5, v.as_str()))?;
    }
    st.bind((8, r.created_at))?;
    st.bind((9, r.created_source.as_str()))?;
    st.bind((10, r.modified_at))?;
    st.bind((11, r.modified_source.as_str()))?;
    st.next()?;
    Ok(())
}

/// The update in progress for `kb_id`, if any, with its nodes in path order.
pub(super) fn load(
    conn: &sqlite::Connection,
    kb_id: &str,
) -> Result<Option<(SeedUpdate, Vec<SeedUpdateNode>)>, StorageError> {
    let mut st = conn.prepare(
        "SELECT plan_hash, repo, rev, commit_sha, tree_sha, excluded_json, started_at \
         FROM kb_seed_update WHERE kb_id = ?1",
    )?;
    st.bind((1, kb_id))?;
    if st.next()? != sqlite::State::Row {
        return Ok(None);
    }
    let excluded: Vec<String> = serde_json::from_str(&st.read::<String, _>("excluded_json")?)
        .map_err(|e| StorageError::Sqlite(format!("kb_seed_update.excluded_json: {e}")))?;
    let u = SeedUpdate {
        kb_id: kb_id.to_string(),
        plan_hash: st.read("plan_hash")?,
        repo: st.read("repo")?,
        rev: st.read("rev")?,
        commit: st.read("commit_sha")?,
        tree: st.read("tree_sha")?,
        excluded,
        started_at: st.read("started_at")?,
    };
    Ok(Some((u, load_nodes(conn, kb_id)?)))
}

fn load_nodes(conn: &sqlite::Connection, kb_id: &str) -> Result<Vec<SeedUpdateNode>, StorageError> {
    let mut st = conn.prepare(
        "SELECT node_id, remove, base_version, path, blob, normalised_sha256, created_at, \
         created_source, modified_at, modified_source, done, seed_wal_id, seed_version \
         FROM kb_seed_update_nodes WHERE kb_id = ?1 ORDER BY remove, path, node_id",
    )?;
    st.bind((1, kb_id))?;
    let mut out = Vec::new();
    while st.next()? == sqlite::State::Row {
        out.push(SeedUpdateNode {
            row: SeedNodeRow {
                node_id: st.read("node_id")?,
                path: st.read("path")?,
                blob: st.read("blob")?,
                normalised_sha256: st.read("normalised_sha256")?,
                created_at: st.read("created_at")?,
                created_source: st.read("created_source")?,
                modified_at: st.read("modified_at")?,
                modified_source: st.read("modified_source")?,
                seed_wal_id: st.read::<Option<i64>, _>("seed_wal_id")?.map(|w| w as u64),
                seed_version: st.read::<Option<String>, _>("seed_version")?,
            },
            remove: st.read::<i64, _>("remove")? != 0,
            base_version: st.read::<Option<String>, _>("base_version")?,
            done: st.read::<i64, _>("done")? != 0,
        });
    }
    Ok(out)
}

/// Mark one node of the update done, with the WAL id and version it was left
/// at (both `None` for a removal).
pub(super) fn node_done(
    conn: &sqlite::Connection,
    kb_id: &str,
    node_id: &str,
    wal: Option<u64>,
    version: Option<&str>,
) -> Result<(), StorageError> {
    let mut st = conn.prepare(
        "UPDATE kb_seed_update_nodes SET done = 1, seed_wal_id = ?3, seed_version = ?4 \
         WHERE kb_id = ?1 AND node_id = ?2",
    )?;
    st.bind((1, kb_id))?;
    st.bind((2, node_id))?;
    st.bind((3, wal.map(|w| w as i64)))?;
    st.bind((4, version))?;
    st.next()?;
    if conn.change_count() != 1 {
        return Err(StorageError::Sqlite(format!(
            "kb_seed_update_nodes: no row for {kb_id}/{node_id}"
        )));
    }
    Ok(())
}

/// Apply the update to the seed record, in one transaction: the KB's node
/// rows become the update's (removals dropped), `kb_seed` takes the new plan
/// and `applied_at`, and the update's own rows go. Refused, changing nothing,
/// while any node is not done.
pub(super) fn finish(
    conn: &sqlite::Connection,
    kb_id: &str,
    applied_at: i64,
) -> Result<(), StorageError> {
    conn.execute("BEGIN IMMEDIATE")?;
    let result = finish_rows(conn, kb_id, applied_at);
    super::editor_record::commit_or_rollback(conn, result)
        .map_err(|e| StorageError::Sqlite(format!("kb_seed_update finish: {e}")))
}

fn finish_rows(
    conn: &sqlite::Connection,
    kb_id: &str,
    applied_at: i64,
) -> Result<(), sqlite::Error> {
    let mut open =
        conn.prepare("SELECT COUNT(*) FROM kb_seed_update_nodes WHERE kb_id = ?1 AND done = 0")?;
    open.bind((1, kb_id))?;
    open.next()?;
    if open.read::<i64, _>(0)? != 0 {
        return Err(sql_error("a node of the update is not written yet"));
    }
    let steps = [
        "DELETE FROM kb_seed_nodes WHERE kb_id = ?1",
        "INSERT INTO kb_seed_nodes (kb_id, node_id, path, blob, normalised_sha256, created_at, \
         created_source, modified_at, modified_source, seed_wal_id, seed_version) \
         SELECT kb_id, node_id, path, blob, normalised_sha256, created_at, created_source, \
         modified_at, modified_source, seed_wal_id, seed_version FROM kb_seed_update_nodes \
         WHERE kb_id = ?1 AND remove = 0",
    ];
    for sql in steps {
        let mut st = conn.prepare(sql)?;
        st.bind((1, kb_id))?;
        st.next()?;
    }
    let mut st = conn.prepare(
        "UPDATE kb_seed SET (plan_hash, repo, rev, commit_sha, tree_sha, excluded_json, \
         started_at, applied_at) = (SELECT plan_hash, repo, rev, commit_sha, tree_sha, \
         excluded_json, started_at, ?2 FROM kb_seed_update WHERE kb_id = ?1) \
         WHERE kb_id = ?1 AND EXISTS (SELECT 1 FROM kb_seed_update WHERE kb_id = ?1)",
    )?;
    st.bind((1, kb_id))?;
    st.bind((2, applied_at))?;
    st.next()?;
    if conn.change_count() != 1 {
        return Err(sql_error("no update in progress for the KB"));
    }
    for sql in [
        "DELETE FROM kb_seed_update_nodes WHERE kb_id = ?1",
        "DELETE FROM kb_seed_update WHERE kb_id = ?1",
    ] {
        let mut st = conn.prepare(sql)?;
        st.bind((1, kb_id))?;
        st.next()?;
    }
    Ok(())
}
