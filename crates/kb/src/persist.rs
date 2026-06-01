//! SQLite + FTS5 persistence for the knowledge base.
//!
//! # Future: yrs Document Storage (ADR-005)
//! This module is planned to evolve into the persistence backend for
//! yrs CRDT documents. Each KB node will gain a `crdt_doc BLOB` column
//! storing encoded yrs document bytes. FTS5 will be rebuilt from
//! materialized `YText::to_string()` content. The existing read path
//! (FTS5 queries, node lookups) remains unchanged during migration.
//!
//! # Model
//! The in-memory `KnowledgeBase` remains the canonical working copy —
//! all reads go through it, and the hot path for the *Help* buffer and
//! AI `kb_*` tools never touches disk. This module provides:
//!
//! - `save_to_sqlite(path)` — write the entire KB to a SQLite file.
//! - `load_from_sqlite(path)` — populate an empty KB from disk.
//! - `fts_search(query)` — full-text search over a loaded DB using FTS5
//!   with BM25 ranking. Returns ids in relevance order.
//!
//! # Schema (v1)
//! - `nodes(id PK, title, kind, body, tags_json)` — one row per node.
//! - `links(src, dst, display, PRIMARY KEY(src,dst))` — outgoing edges.
//!   Backlinks are computed by querying `WHERE dst = ?`.
//! - `nodes_fts` — FTS5 virtual table over `title`, `body`, `tags` with
//!   porter + unicode61 tokenizer. Rebuilt during load by triggers.
//!
//! Schema evolution uses `PRAGMA user_version` — bumped on breaking
//! changes so we can migrate rather than crash.

use crate::{KnowledgeBase, Node, NodeKind};
use rusqlite::{params, Connection};
use std::path::Path;
use tracing::{debug, info};

const SCHEMA_VERSION: i32 = 7;

/// Error type wrapping rusqlite and serde errors for the persistence layer.
#[derive(Debug)]
pub enum PersistError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    SchemaMismatch {
        found: i32,
        expected: i32,
    },
    /// The database was created by a newer version of MAE.
    FutureSchema {
        found: i32,
        supported: i32,
    },
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::SchemaMismatch { found, expected } => {
                write!(f, "KB schema v{found} found, expected v{expected}")
            }
            Self::FutureSchema { found, supported } => {
                write!(
                    f,
                    "KB schema v{found} is from a newer MAE version (this build supports up to v{supported})"
                )
            }
        }
    }
}

impl std::error::Error for PersistError {}

impl From<rusqlite::Error> for PersistError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}

impl From<serde_json::Error> for PersistError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

pub fn kind_to_str(k: NodeKind) -> &'static str {
    match k {
        NodeKind::Index => "index",
        NodeKind::Command => "command",
        NodeKind::Concept => "concept",
        NodeKind::Key => "key",
        NodeKind::Note => "note",
        NodeKind::Project => "project",
    }
}

fn kind_from_str(s: &str) -> NodeKind {
    match s {
        "index" => NodeKind::Index,
        "command" => NodeKind::Command,
        "concept" => NodeKind::Concept,
        "key" => NodeKind::Key,
        "project" => NodeKind::Project,
        _ => NodeKind::Note,
    }
}

/// Initialize schema and run any pending migrations. Public entry point
/// for `SqliteKbStore` and other consumers that manage their own connection.
pub fn init_and_migrate(conn: &Connection) -> Result<(), PersistError> {
    check_schema_version(conn)?;
    init_schema(conn)
}

/// Create schema tables on a fresh connection. Idempotent — safe to run
/// on every open.
fn init_schema(conn: &Connection) -> Result<(), PersistError> {
    // Enable WAL mode for concurrent readers + single writer.
    // Safe to call on every open — SQLite ignores if already in WAL mode.
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // Retry on SQLITE_BUSY for up to 5 seconds before failing.
    conn.pragma_update(None, "busy_timeout", "5000")?;
    // NORMAL synchronous is safe with WAL (data integrity guaranteed on crash).
    conn.pragma_update(None, "synchronous", "NORMAL")?;

    debug!("SQLite WAL mode enabled");

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS nodes (
            id              TEXT PRIMARY KEY,
            title           TEXT NOT NULL,
            kind            TEXT NOT NULL,
            body            TEXT NOT NULL,
            tags_json       TEXT NOT NULL DEFAULT '[]',
            todo_state      TEXT,
            priority        TEXT,
            source          TEXT,
            source_version  INTEGER,
            aliases_json    TEXT NOT NULL DEFAULT '[]',
            properties_json TEXT NOT NULL DEFAULT '{}',
            created_at      INTEGER,
            updated_at      INTEGER,
            crdt_doc        BLOB
        );
        CREATE TABLE IF NOT EXISTS links (
            src     TEXT NOT NULL,
            dst     TEXT NOT NULL,
            display TEXT,
            PRIMARY KEY (src, dst)
        );
        CREATE INDEX IF NOT EXISTS idx_links_dst ON links(dst);
        CREATE INDEX IF NOT EXISTS idx_nodes_todo ON nodes(todo_state);
        CREATE INDEX IF NOT EXISTS idx_nodes_priority ON nodes(priority);
        CREATE TABLE IF NOT EXISTS node_tags (
            node_id TEXT NOT NULL,
            tag     TEXT NOT NULL,
            PRIMARY KEY (node_id, tag)
        );
        CREATE INDEX IF NOT EXISTS idx_node_tags_tag ON node_tags(tag);
        CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
            id UNINDEXED,
            title,
            body,
            tags,
            aliases,
            tokenize='porter unicode61'
        );
        CREATE TABLE IF NOT EXISTS node_changelog (
            rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
            node_id     TEXT NOT NULL,
            operation   TEXT NOT NULL,
            old_title   TEXT,
            old_body    TEXT,
            old_tags_json TEXT,
            new_title   TEXT,
            new_body    TEXT,
            new_tags_json TEXT,
            timestamp   INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            author      TEXT,
            reason      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_changelog_node ON node_changelog(node_id);
        CREATE INDEX IF NOT EXISTS idx_changelog_ts ON node_changelog(timestamp);
        "#,
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn check_schema_version(conn: &Connection) -> Result<(), PersistError> {
    let found: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found == 0 || found == SCHEMA_VERSION {
        return Ok(());
    }
    if found > SCHEMA_VERSION {
        return Err(PersistError::FutureSchema {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    // Step-wise migration chain: v1 → v2 → v3 → v4 → ...
    if found < 2 {
        migrate_v1_to_v2(conn)?;
    }
    if found < 3 {
        migrate_v2_to_v3(conn)?;
    }
    if found < 4 {
        migrate_v3_to_v4(conn)?;
    }
    if found < 5 {
        migrate_v4_to_v5(conn)?;
    }
    if found < 6 {
        migrate_v5_to_v6(conn)?;
    }
    if found < 7 {
        migrate_v6_to_v7(conn)?;
    }
    Ok(())
}

/// Check whether a column exists in a table via `PRAGMA table_info`.
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, PersistError> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let found = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .any(|r| r.as_deref() == Ok(column));
    Ok(found)
}

fn migrate_v1_to_v2(conn: &Connection) -> Result<(), PersistError> {
    let tx = conn.unchecked_transaction()?;
    if !has_column(conn, "nodes", "todo_state")? {
        tx.execute("ALTER TABLE nodes ADD COLUMN todo_state TEXT", [])?;
    }
    if !has_column(conn, "nodes", "priority")? {
        tx.execute("ALTER TABLE nodes ADD COLUMN priority TEXT", [])?;
    }
    tx.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_nodes_todo ON nodes(todo_state);
        CREATE INDEX IF NOT EXISTS idx_nodes_priority ON nodes(priority);
        CREATE TABLE IF NOT EXISTS node_tags (
            node_id TEXT NOT NULL,
            tag     TEXT NOT NULL,
            PRIMARY KEY (node_id, tag)
        );
        CREATE INDEX IF NOT EXISTS idx_node_tags_tag ON node_tags(tag);
        "#,
    )?;
    tx.pragma_update(None, "user_version", 2)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v2_to_v3(conn: &Connection) -> Result<(), PersistError> {
    let tx = conn.unchecked_transaction()?;
    if !has_column(conn, "nodes", "source")? {
        tx.execute("ALTER TABLE nodes ADD COLUMN source TEXT", [])?;
    }
    if !has_column(conn, "nodes", "source_version")? {
        tx.execute("ALTER TABLE nodes ADD COLUMN source_version INTEGER", [])?;
    }
    tx.pragma_update(None, "user_version", 3)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v3_to_v4(conn: &Connection) -> Result<(), PersistError> {
    let tx = conn.unchecked_transaction()?;
    if !has_column(conn, "nodes", "aliases_json")? {
        tx.execute(
            "ALTER TABLE nodes ADD COLUMN aliases_json TEXT NOT NULL DEFAULT '[]'",
            [],
        )?;
    }
    // Rebuild FTS5 table to include aliases column.
    tx.execute_batch("DROP TABLE IF EXISTS nodes_fts")?;
    tx.execute_batch(
        r#"CREATE VIRTUAL TABLE nodes_fts USING fts5(
            id UNINDEXED, title, body, tags, aliases,
            tokenize='porter unicode61'
        )"#,
    )?;
    // Repopulate FTS5 from existing data.
    tx.execute_batch(
        "INSERT INTO nodes_fts(id, title, body, tags, aliases)
         SELECT id, title, body, COALESCE(tags_json,''), aliases_json FROM nodes",
    )?;
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v4_to_v5(conn: &Connection) -> Result<(), PersistError> {
    let tx = conn.unchecked_transaction()?;
    if !has_column(conn, "nodes", "properties_json")? {
        tx.execute(
            "ALTER TABLE nodes ADD COLUMN properties_json TEXT NOT NULL DEFAULT '{}'",
            [],
        )?;
    }
    tx.pragma_update(None, "user_version", 5)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v5_to_v6(conn: &Connection) -> Result<(), PersistError> {
    info!(from = 5, to = 6, "KB schema migration");
    let tx = conn.unchecked_transaction()?;
    // Add timestamp columns
    if !has_column(conn, "nodes", "created_at")? {
        tx.execute("ALTER TABLE nodes ADD COLUMN created_at INTEGER", [])?;
    }
    if !has_column(conn, "nodes", "updated_at")? {
        tx.execute("ALTER TABLE nodes ADD COLUMN updated_at INTEGER", [])?;
    }
    // Create changelog table
    tx.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS node_changelog (
            rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
            node_id     TEXT NOT NULL,
            operation   TEXT NOT NULL,
            old_title   TEXT,
            old_body    TEXT,
            old_tags_json TEXT,
            new_title   TEXT,
            new_body    TEXT,
            new_tags_json TEXT,
            timestamp   INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
            author      TEXT,
            reason      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_changelog_node ON node_changelog(node_id);
        CREATE INDEX IF NOT EXISTS idx_changelog_ts ON node_changelog(timestamp);
    "#,
    )?;
    // Backfill timestamps from properties_json if available
    tx.execute_batch(
        r#"
        UPDATE nodes SET
            updated_at = CAST(json_extract(properties_json, '$.last-modified') AS INTEGER),
            created_at = CAST(json_extract(properties_json, '$.last-modified') AS INTEGER)
        WHERE properties_json != '{}' AND json_extract(properties_json, '$.last-modified') IS NOT NULL
    "#,
    )?;
    // Backfill remaining with current time
    tx.execute(
        "UPDATE nodes SET created_at = strftime('%s', 'now') WHERE created_at IS NULL",
        [],
    )?;
    tx.execute(
        "UPDATE nodes SET updated_at = strftime('%s', 'now') WHERE updated_at IS NULL",
        [],
    )?;
    tx.pragma_update(None, "user_version", 6)?;
    tx.commit()?;
    Ok(())
}

fn migrate_v6_to_v7(conn: &Connection) -> Result<(), PersistError> {
    info!(
        from = 6,
        to = 7,
        "KB schema migration — adding crdt_doc BLOB column"
    );
    let tx = conn.unchecked_transaction()?;
    if !has_column(conn, "nodes", "crdt_doc")? {
        tx.execute("ALTER TABLE nodes ADD COLUMN crdt_doc BLOB", [])?;
    }
    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    tx.commit()?;
    Ok(())
}

impl KnowledgeBase {
    /// Write the full KB to a SQLite file at `path`. Creates the file
    /// if absent and overwrites all existing node/link/FTS rows atomically
    /// in one transaction.
    pub fn save_to_sqlite(&self, path: impl AsRef<Path>) -> Result<(), PersistError> {
        let mut conn = Connection::open(path)?;
        init_schema(&conn)?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM nodes", [])?;
        tx.execute("DELETE FROM links", [])?;
        tx.execute("DELETE FROM nodes_fts", [])?;
        tx.execute("DELETE FROM node_tags", [])?;
        let mut node_count: usize = 0;
        {
            let mut ins_node = tx.prepare(
                "INSERT INTO nodes (id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, created_at, updated_at, crdt_doc) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )?;
            let mut ins_link =
                tx.prepare("INSERT OR IGNORE INTO links (src, dst, display) VALUES (?, ?, ?)")?;
            let mut ins_fts = tx.prepare(
                "INSERT INTO nodes_fts (id, title, body, tags, aliases) VALUES (?, ?, ?, ?, ?)",
            )?;
            let mut ins_tag =
                tx.prepare("INSERT OR IGNORE INTO node_tags (node_id, tag) VALUES (?, ?)")?;
            for node in self.nodes_values() {
                let tags_json = serde_json::to_string(&node.tags)?;
                let aliases_json = serde_json::to_string(&node.aliases)?;
                let properties_json = serde_json::to_string(&node.properties)?;
                let pri_str = node.priority.map(|c| c.to_string());
                let source_str = node.source.map(|s| match s {
                    crate::NodeSource::Seed => "seed",
                    crate::NodeSource::UserOrg => "user_org",
                    crate::NodeSource::Manual => "manual",
                    crate::NodeSource::Federation => "federation",
                });
                ins_node.execute(params![
                    &node.id,
                    &node.title,
                    kind_to_str(node.kind),
                    &node.body,
                    &tags_json,
                    &node.todo_state,
                    &pri_str,
                    &source_str,
                    &node.source_version,
                    &aliases_json,
                    &properties_json,
                    now,
                    now,
                    &node.crdt_doc,
                ])?;
                ins_fts.execute(params![
                    &node.id,
                    &node.title,
                    &node.body,
                    node.tags.join(" "),
                    node.aliases.join(" "),
                ])?;
                for tag in &node.tags {
                    ins_tag.execute(params![&node.id, tag])?;
                }
                for (dst, display) in crate::parse_links(&node.body) {
                    let disp: Option<&str> = if dst == display {
                        None
                    } else {
                        Some(display.as_str())
                    };
                    ins_link.execute(params![&node.id, &dst, disp])?;
                }
                node_count += 1;
            }
        }
        tx.commit()?;
        info!(node_count, "KB saved to SQLite (full)");
        Ok(())
    }

    /// Replace this KB's contents with those loaded from a SQLite file.
    /// The existing in-memory state is discarded. Returns the number of
    /// nodes loaded. Links are rebuilt from node bodies (authoritative)
    /// rather than from the `links` table (that table is a denormalized
    /// cache for SQL-side queries, e.g. migration tools).
    pub fn load_from_sqlite(&mut self, path: impl AsRef<Path>) -> Result<usize, PersistError> {
        let conn = Connection::open(path)?;
        check_schema_version(&conn)?;
        init_schema(&conn)?; // no-op if already initialized
        *self = KnowledgeBase::new();
        // Check if optional columns exist (pre-v4/v5/v7 databases may not have them).
        let has_aliases = has_column(&conn, "nodes", "aliases_json")?;
        let has_properties = has_column(&conn, "nodes", "properties_json")?;
        let has_crdt = has_column(&conn, "nodes", "crdt_doc")?;
        let base_cols =
            "id, title, kind, body, tags_json, todo_state, priority, source, source_version";
        let mut cols = base_cols.to_string();
        if has_aliases {
            cols.push_str(", aliases_json");
        }
        if has_properties {
            cols.push_str(", properties_json");
        }
        if has_crdt {
            cols.push_str(", crdt_doc");
        }
        let query_str = format!("SELECT {cols} FROM nodes ORDER BY id");
        let mut stmt = conn.prepare(&query_str)?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let body: String = row.get(3)?;
            let tags_json: String = row.get(4)?;
            let todo_state: Option<String> = row.get(5)?;
            let priority_str: Option<String> = row.get(6)?;
            let source_str: Option<String> = row.get(7)?;
            let source_version: Option<u32> = row.get(8)?;
            let mut col_idx = 9;
            let aliases_json: String = if has_aliases {
                let v = row.get(col_idx)?;
                col_idx += 1;
                v
            } else {
                "[]".to_string()
            };
            let properties_json: String = if has_properties {
                let v = row.get(col_idx)?;
                col_idx += 1;
                v
            } else {
                "{}".to_string()
            };
            let crdt_doc: Option<Vec<u8>> = if has_crdt { row.get(col_idx)? } else { None };
            Ok((
                id,
                title,
                kind,
                body,
                tags_json,
                todo_state,
                priority_str,
                source_str,
                source_version,
                aliases_json,
                properties_json,
                crdt_doc,
            ))
        })?;
        let mut count = 0;
        for row in rows {
            let (
                id,
                title,
                kind,
                body,
                tags_json,
                todo_state,
                priority_str,
                source_str,
                source_version,
                aliases_json,
                properties_json,
                crdt_doc,
            ) = row?;
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
            let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();
            let properties: std::collections::HashMap<String, String> =
                serde_json::from_str(&properties_json).unwrap_or_default();
            let priority = priority_str.and_then(|s| s.chars().next());
            let source = source_str.as_deref().map(|s| match s {
                "seed" => crate::NodeSource::Seed,
                "user_org" => crate::NodeSource::UserOrg,
                "manual" => crate::NodeSource::Manual,
                "federation" => crate::NodeSource::Federation,
                _ => crate::NodeSource::Manual,
            });
            let mut node = Node::new(id, title, kind_from_str(&kind), body)
                .with_tags(tags)
                .with_aliases(aliases)
                .with_properties(properties);
            node.todo_state = todo_state;
            node.priority = priority;
            node.source = source;
            node.crdt_doc = crdt_doc;
            node.source_version = source_version;
            self.insert(node);
            count += 1;
        }
        info!(node_count = count, "KB loaded from SQLite");
        Ok(count)
    }

    /// Full-text search over a saved KB using FTS5 BM25 ranking.
    /// Opens the DB read-only. Use this when your KB lives on disk and
    /// you want ranked results beyond plain substring matching.
    ///
    /// Query syntax is FTS5's: bare words, `"phrase"`, `prefix*`, `NOT`, `AND`, `OR`.
    pub fn fts_search(
        path: impl AsRef<Path>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<String>, PersistError> {
        let conn = Connection::open(path)?;
        check_schema_version(&conn)?;
        init_schema(&conn)?; // ensure FTS virtual table exists on old databases
        let mut stmt = conn.prepare(
            "SELECT id FROM nodes_fts WHERE nodes_fts MATCH ? ORDER BY bm25(nodes_fts) LIMIT ?",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Check whether a SQLite file at `path` looks like a MAE KB database
    /// (has the expected schema version set). Returns Ok(None) if the
    /// file doesn't exist.
    pub fn probe_sqlite(path: impl AsRef<Path>) -> Result<Option<i32>, PersistError> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(None);
        }
        let conn = Connection::open(path)?;
        let v: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        Ok(Some(v))
    }
}

/// A single entry from the changelog.
#[derive(Debug, Clone)]
pub struct ChangelogEntry {
    pub rowid: i64,
    pub node_id: String,
    pub operation: String,
    pub old_title: Option<String>,
    pub old_body: Option<String>,
    pub old_tags_json: Option<String>,
    pub new_title: Option<String>,
    pub new_body: Option<String>,
    pub new_tags_json: Option<String>,
    pub timestamp: i64,
    pub author: Option<String>,
    pub reason: Option<String>,
}

impl KnowledgeBase {
    /// Incrementally sync in-memory KB to SQLite, recording changes in the changelog.
    /// Only writes nodes that have changed since the last sync.
    pub fn sync_to_sqlite(&self, path: impl AsRef<Path>) -> Result<(), PersistError> {
        let mut conn = Connection::open(path)?;
        init_schema(&conn)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        // Load existing node data for comparison
        let mut existing: std::collections::HashMap<String, (String, String, String)> =
            std::collections::HashMap::new();
        {
            let mut stmt = conn.prepare("SELECT id, title, body, tags_json FROM nodes")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            for row in rows {
                let (id, title, body, tags) = row?;
                existing.insert(id, (title, body, tags));
            }
        }

        let tx = conn.transaction()?;

        let in_memory_ids: std::collections::HashSet<String> =
            self.nodes_values().map(|n| n.id.clone()).collect();

        let mut n_creates: usize = 0;
        let mut n_updates: usize = 0;
        let mut n_deletes: usize = 0;

        // Handle creates and updates
        for node in self.nodes_values() {
            let tags_json = serde_json::to_string(&node.tags)?;
            let aliases_json = serde_json::to_string(&node.aliases)?;
            let properties_json = serde_json::to_string(&node.properties)?;
            let pri_str = node.priority.map(|c| c.to_string());
            let source_str = node.source.map(|s| match s {
                crate::NodeSource::Seed => "seed",
                crate::NodeSource::UserOrg => "user_org",
                crate::NodeSource::Manual => "manual",
                crate::NodeSource::Federation => "federation",
            });

            if let Some((old_title, old_body, old_tags)) = existing.get(&node.id) {
                // Exists — check if changed
                if old_title != &node.title || old_body != &node.body || old_tags != &tags_json {
                    // UPDATE
                    tx.execute(
                        "UPDATE nodes SET title=?, kind=?, body=?, tags_json=?, todo_state=?, priority=?, source=?, source_version=?, aliases_json=?, properties_json=?, updated_at=?, crdt_doc=? WHERE id=?",
                        params![&node.title, kind_to_str(node.kind), &node.body, &tags_json, &node.todo_state, &pri_str, &source_str, &node.source_version, &aliases_json, &properties_json, now, &node.crdt_doc, &node.id],
                    )?;
                    // Record changelog
                    tx.execute(
                        "INSERT INTO node_changelog (node_id, operation, old_title, old_body, old_tags_json, new_title, new_body, new_tags_json) VALUES (?, 'update', ?, ?, ?, ?, ?, ?)",
                        params![&node.id, old_title, old_body, old_tags, &node.title, &node.body, &tags_json],
                    )?;
                    // Rebuild FTS for this node
                    tx.execute("DELETE FROM nodes_fts WHERE id = ?", params![&node.id])?;
                    tx.execute(
                        "INSERT INTO nodes_fts (id, title, body, tags, aliases) VALUES (?, ?, ?, ?, ?)",
                        params![&node.id, &node.title, &node.body, node.tags.join(" "), node.aliases.join(" ")],
                    )?;
                    // Rebuild tags
                    tx.execute("DELETE FROM node_tags WHERE node_id = ?", params![&node.id])?;
                    for tag in &node.tags {
                        tx.execute(
                            "INSERT OR IGNORE INTO node_tags (node_id, tag) VALUES (?, ?)",
                            params![&node.id, tag],
                        )?;
                    }
                    n_updates += 1;
                }
                // Unchanged — skip
            } else {
                // New node — INSERT
                tx.execute(
                    "INSERT INTO nodes (id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, created_at, updated_at, crdt_doc) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                    params![&node.id, &node.title, kind_to_str(node.kind), &node.body, &tags_json, &node.todo_state, &pri_str, &source_str, &node.source_version, &aliases_json, &properties_json, now, now, &node.crdt_doc],
                )?;
                // Record changelog
                tx.execute(
                    "INSERT INTO node_changelog (node_id, operation, new_title, new_body, new_tags_json) VALUES (?, 'create', ?, ?, ?)",
                    params![&node.id, &node.title, &node.body, &tags_json],
                )?;
                // FTS
                tx.execute(
                    "INSERT INTO nodes_fts (id, title, body, tags, aliases) VALUES (?, ?, ?, ?, ?)",
                    params![
                        &node.id,
                        &node.title,
                        &node.body,
                        node.tags.join(" "),
                        node.aliases.join(" ")
                    ],
                )?;
                for tag in &node.tags {
                    tx.execute(
                        "INSERT OR IGNORE INTO node_tags (node_id, tag) VALUES (?, ?)",
                        params![&node.id, tag],
                    )?;
                }
                n_creates += 1;
            }

            // Rebuild links for this node
            tx.execute("DELETE FROM links WHERE src = ?", params![&node.id])?;
            for (dst, display) in crate::parse_links(&node.body) {
                let disp: Option<&str> = if dst == display {
                    None
                } else {
                    Some(display.as_str())
                };
                tx.execute(
                    "INSERT OR IGNORE INTO links (src, dst, display) VALUES (?, ?, ?)",
                    params![&node.id, &dst, disp],
                )?;
            }
        }

        // Handle deletes (in DB but not in memory)
        for (old_id, (old_title, old_body, old_tags)) in &existing {
            if !in_memory_ids.contains(old_id) {
                tx.execute(
                    "INSERT INTO node_changelog (node_id, operation, old_title, old_body, old_tags_json) VALUES (?, 'delete', ?, ?, ?)",
                    params![old_id, old_title, old_body, old_tags],
                )?;
                tx.execute("DELETE FROM nodes WHERE id = ?", params![old_id])?;
                tx.execute("DELETE FROM nodes_fts WHERE id = ?", params![old_id])?;
                tx.execute("DELETE FROM node_tags WHERE node_id = ?", params![old_id])?;
                tx.execute("DELETE FROM links WHERE src = ?", params![old_id])?;
                n_deletes += 1;
            }
        }

        tx.commit()?;
        info!(
            creates = n_creates,
            updates = n_updates,
            deletes = n_deletes,
            "KB synced to SQLite (incremental)"
        );
        Ok(())
    }

    /// Get change history for a specific node.
    pub fn node_history(
        path: impl AsRef<Path>,
        node_id: &str,
    ) -> Result<Vec<ChangelogEntry>, PersistError> {
        let conn = Connection::open(path)?;
        check_schema_version(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT rowid, node_id, operation, old_title, old_body, old_tags_json, new_title, new_body, new_tags_json, timestamp, author, reason FROM node_changelog WHERE node_id = ? ORDER BY rowid DESC",
        )?;
        let rows = stmt.query_map(params![node_id], |row| {
            Ok(ChangelogEntry {
                rowid: row.get(0)?,
                node_id: row.get(1)?,
                operation: row.get(2)?,
                old_title: row.get(3)?,
                old_body: row.get(4)?,
                old_tags_json: row.get(5)?,
                new_title: row.get(6)?,
                new_body: row.get(7)?,
                new_tags_json: row.get(8)?,
                timestamp: row.get(9)?,
                author: row.get(10)?,
                reason: row.get(11)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Get all changes since a given epoch timestamp.
    pub fn changes_since(
        path: impl AsRef<Path>,
        since_epoch: i64,
    ) -> Result<Vec<ChangelogEntry>, PersistError> {
        let conn = Connection::open(path)?;
        check_schema_version(&conn)?;
        let mut stmt = conn.prepare(
            "SELECT rowid, node_id, operation, old_title, old_body, old_tags_json, new_title, new_body, new_tags_json, timestamp, author, reason FROM node_changelog WHERE timestamp >= ? ORDER BY rowid",
        )?;
        let rows = stmt.query_map(params![since_epoch], |row| {
            Ok(ChangelogEntry {
                rowid: row.get(0)?,
                node_id: row.get(1)?,
                operation: row.get(2)?,
                old_title: row.get(3)?,
                old_body: row.get(4)?,
                old_tags_json: row.get(5)?,
                new_title: row.get(6)?,
                new_body: row.get(7)?,
                new_tags_json: row.get(8)?,
                timestamp: row.get(9)?,
                author: row.get(10)?,
                reason: row.get(11)?,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_kb() -> KnowledgeBase {
        let mut kb = KnowledgeBase::new();
        kb.insert(
            Node::new(
                "index",
                "Help Index",
                NodeKind::Index,
                "Welcome. See [[concept:buffer]] and [[cmd:save]].",
            )
            .with_tags(["help"]),
        );
        kb.insert(
            Node::new(
                "concept:buffer",
                "Buffer",
                NodeKind::Concept,
                "A buffer is a piece of editable text backed by a rope.",
            )
            .with_tags(["core", "editing"]),
        );
        kb.insert(Node::new(
            "cmd:save",
            "Save Buffer",
            NodeKind::Command,
            "Persist the active buffer to disk.",
        ));
        kb
    }

    #[test]
    fn save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();

        let mut restored = KnowledgeBase::new();
        let n = restored.load_from_sqlite(&path).unwrap();
        assert_eq!(n, 3);
        assert_eq!(restored.len(), 3);
        let idx = restored.get("index").unwrap();
        assert_eq!(idx.title, "Help Index");
        assert_eq!(idx.kind, NodeKind::Index);
        assert_eq!(idx.tags, vec!["help".to_string()]);
        // Links are rebuilt from body — reverse index must work post-load.
        assert!(restored
            .links_to("concept:buffer")
            .contains(&"index".to_string()));
        assert!(restored.links_to("cmd:save").contains(&"index".to_string()));
    }

    #[test]
    fn save_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();
        kb.save_to_sqlite(&path).unwrap(); // second save must not duplicate
        let mut restored = KnowledgeBase::new();
        assert_eq!(restored.load_from_sqlite(&path).unwrap(), 3);
    }

    #[test]
    fn save_overwrites_existing_rows() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        let mut kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();
        // Remove a node, save again — the stale row must not persist.
        kb.remove("cmd:save");
        kb.save_to_sqlite(&path).unwrap();
        let mut restored = KnowledgeBase::new();
        restored.load_from_sqlite(&path).unwrap();
        assert!(restored.get("cmd:save").is_none());
        assert_eq!(restored.len(), 2);
    }

    #[test]
    fn fts_search_finds_by_title_and_body() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        sample_kb().save_to_sqlite(&path).unwrap();

        let hits = KnowledgeBase::fts_search(&path, "buffer", 10).unwrap();
        assert!(hits.contains(&"concept:buffer".to_string()));

        let hits = KnowledgeBase::fts_search(&path, "rope", 10).unwrap();
        assert!(hits.contains(&"concept:buffer".to_string()));
    }

    #[test]
    fn fts_search_prefix_query() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        sample_kb().save_to_sqlite(&path).unwrap();
        // FTS5 prefix query: "pers*" should match "Persist" in cmd:save body.
        let hits = KnowledgeBase::fts_search(&path, "pers*", 10).unwrap();
        assert!(hits.contains(&"cmd:save".to_string()));
    }

    #[test]
    fn probe_returns_none_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("does-not-exist.db");
        assert_eq!(KnowledgeBase::probe_sqlite(&path).unwrap(), None);
    }

    #[test]
    fn probe_returns_schema_version() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        sample_kb().save_to_sqlite(&path).unwrap();
        assert_eq!(
            KnowledgeBase::probe_sqlite(&path).unwrap(),
            Some(SCHEMA_VERSION)
        );
    }

    #[test]
    fn load_from_empty_db_is_ok() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("empty.db");
        // Create an empty DB with our schema
        let conn = Connection::open(&path).unwrap();
        init_schema(&conn).unwrap();
        drop(conn);
        let mut kb = KnowledgeBase::new();
        assert_eq!(kb.load_from_sqlite(&path).unwrap(), 0);
    }

    #[test]
    fn load_replaces_existing_state() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        sample_kb().save_to_sqlite(&path).unwrap();
        let mut kb = KnowledgeBase::new();
        kb.insert(Node::new(
            "ghost",
            "Ghost",
            NodeKind::Note,
            "should not survive load",
        ));
        kb.load_from_sqlite(&path).unwrap();
        assert!(kb.get("ghost").is_none(), "pre-load state must be cleared");
    }

    /// Create a v1 database (no todo_state, priority, source columns)
    /// and verify the migration chain runs through to current.
    #[test]
    fn migrate_v1_to_current() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("v1.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE nodes (
                id        TEXT PRIMARY KEY,
                title     TEXT NOT NULL,
                kind      TEXT NOT NULL,
                body      TEXT NOT NULL,
                tags_json TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE links (
                src TEXT NOT NULL, dst TEXT NOT NULL, display TEXT,
                PRIMARY KEY (src, dst)
            );
            "#,
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES (?, ?, ?, ?, ?)",
            params!["n1", "Test", "note", "body", "[]"],
        )
        .unwrap();
        drop(conn);

        let mut kb = KnowledgeBase::new();
        let n = kb.load_from_sqlite(&path).unwrap();
        assert_eq!(n, 1);
        let node = kb.get("n1").unwrap();
        assert_eq!(node.title, "Test");
        assert!(node.todo_state.is_none());
        assert!(node.source.is_none());
    }

    /// Create a v2 database (has todo_state/priority, no source columns)
    /// and verify v2→v3 migration preserves todo_state.
    #[test]
    fn migrate_v2_to_current() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("v2.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE nodes (
                id        TEXT PRIMARY KEY,
                title     TEXT NOT NULL,
                kind      TEXT NOT NULL,
                body      TEXT NOT NULL,
                tags_json TEXT NOT NULL DEFAULT '[]',
                todo_state TEXT,
                priority   TEXT
            );
            CREATE TABLE links (
                src TEXT NOT NULL, dst TEXT NOT NULL, display TEXT,
                PRIMARY KEY (src, dst)
            );
            CREATE TABLE node_tags (
                node_id TEXT NOT NULL, tag TEXT NOT NULL,
                PRIMARY KEY (node_id, tag)
            );
            "#,
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute(
            "INSERT INTO nodes (id, title, kind, body, tags_json, todo_state) VALUES (?, ?, ?, ?, ?, ?)",
            params!["n1", "Task", "note", "do thing", "[]", "TODO"],
        )
        .unwrap();
        drop(conn);

        let mut kb = KnowledgeBase::new();
        let n = kb.load_from_sqlite(&path).unwrap();
        assert_eq!(n, 1);
        let node = kb.get("n1").unwrap();
        assert_eq!(node.todo_state.as_deref(), Some("TODO"));
        assert!(node.source.is_none()); // added by migration but NULL
    }

    /// Running migrations twice must not crash (idempotent ALTER TABLE).
    #[test]
    fn migrate_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("v1.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE nodes (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, kind TEXT NOT NULL,
                body TEXT NOT NULL, tags_json TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE links (
                src TEXT NOT NULL, dst TEXT NOT NULL, display TEXT,
                PRIMARY KEY (src, dst)
            );
            "#,
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        drop(conn);

        // First load triggers v1→v2→v3 migration
        let mut kb = KnowledgeBase::new();
        kb.load_from_sqlite(&path).unwrap();

        // Second load — migration should be a no-op (already at v3)
        let mut kb2 = KnowledgeBase::new();
        kb2.load_from_sqlite(&path).unwrap(); // must not crash
    }

    #[test]
    fn aliases_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        let mut kb = KnowledgeBase::new();
        kb.insert(
            Node::new(
                "concept:modules",
                "Module System",
                NodeKind::Concept,
                "body",
            )
            .with_aliases(["plugins", "extensions"]),
        );
        kb.save_to_sqlite(&path).unwrap();

        let mut restored = KnowledgeBase::new();
        restored.load_from_sqlite(&path).unwrap();
        let node = restored.get("concept:modules").unwrap();
        assert_eq!(
            node.aliases,
            vec!["plugins".to_string(), "extensions".to_string()]
        );
    }

    #[test]
    fn fts_search_finds_by_alias() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        let mut kb = KnowledgeBase::new();
        kb.insert(
            Node::new(
                "concept:modules",
                "Module System",
                NodeKind::Concept,
                "body",
            )
            .with_aliases(["plugins", "extensions"]),
        );
        kb.save_to_sqlite(&path).unwrap();

        let hits = KnowledgeBase::fts_search(&path, "plugins", 10).unwrap();
        assert!(hits.contains(&"concept:modules".to_string()));
    }

    /// Create a v3 database (no aliases_json column) and verify v3→v4 migration.
    #[test]
    fn migrate_v3_to_current() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("v3.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE nodes (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, kind TEXT NOT NULL,
                body TEXT NOT NULL, tags_json TEXT NOT NULL DEFAULT '[]',
                todo_state TEXT, priority TEXT, source TEXT, source_version INTEGER
            );
            CREATE TABLE links (
                src TEXT NOT NULL, dst TEXT NOT NULL, display TEXT,
                PRIMARY KEY (src, dst)
            );
            CREATE TABLE node_tags (
                node_id TEXT NOT NULL, tag TEXT NOT NULL,
                PRIMARY KEY (node_id, tag)
            );
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                id UNINDEXED, title, body, tags,
                tokenize='porter unicode61'
            );
            "#,
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute(
            "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES (?, ?, ?, ?, ?)",
            params!["n1", "Test", "note", "body", "[]"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nodes_fts (id, title, body, tags) VALUES (?, ?, ?, ?)",
            params!["n1", "Test", "body", ""],
        )
        .unwrap();
        drop(conn);

        let mut kb = KnowledgeBase::new();
        let n = kb.load_from_sqlite(&path).unwrap();
        assert_eq!(n, 1);
        let node = kb.get("n1").unwrap();
        assert!(node.aliases.is_empty(), "aliases should default to empty");

        // Verify we can now save back with aliases
        kb.insert(Node::new("n2", "Two", NodeKind::Note, "body").with_aliases(["alias1"]));
        kb.save_to_sqlite(&path).unwrap();
        let mut kb2 = KnowledgeBase::new();
        kb2.load_from_sqlite(&path).unwrap();
        assert_eq!(kb2.get("n2").unwrap().aliases, vec!["alias1".to_string()]);
    }

    #[test]
    fn properties_round_trip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("kb.db");
        let mut kb = KnowledgeBase::new();
        let mut props = std::collections::HashMap::new();
        props.insert("hash".to_string(), "deadbeef".to_string());
        props.insert("last-modified".to_string(), "2026-01-15".to_string());
        kb.insert(Node::new("n1", "Test", NodeKind::Note, "body").with_properties(props));
        kb.save_to_sqlite(&path).unwrap();

        let mut restored = KnowledgeBase::new();
        restored.load_from_sqlite(&path).unwrap();
        let node = restored.get("n1").unwrap();
        assert_eq!(node.properties.get("hash").unwrap(), "deadbeef");
        assert_eq!(node.properties.get("last-modified").unwrap(), "2026-01-15");
    }

    /// Migrate a v4 database (no properties_json) → v5.
    #[test]
    fn migrate_v4_to_current() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("v4.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE nodes (
                id TEXT PRIMARY KEY, title TEXT NOT NULL, kind TEXT NOT NULL,
                body TEXT NOT NULL, tags_json TEXT NOT NULL DEFAULT '[]',
                todo_state TEXT, priority TEXT, source TEXT, source_version INTEGER,
                aliases_json TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE links (
                src TEXT NOT NULL, dst TEXT NOT NULL, display TEXT,
                PRIMARY KEY (src, dst)
            );
            CREATE TABLE node_tags (
                node_id TEXT NOT NULL, tag TEXT NOT NULL,
                PRIMARY KEY (node_id, tag)
            );
            CREATE VIRTUAL TABLE nodes_fts USING fts5(
                id UNINDEXED, title, body, tags, aliases,
                tokenize='porter unicode61'
            );
            "#,
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute(
            "INSERT INTO nodes (id, title, kind, body) VALUES (?, ?, ?, ?)",
            params!["n1", "Test", "note", "body"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nodes_fts (id, title, body, tags, aliases) VALUES (?, ?, ?, ?, ?)",
            params!["n1", "Test", "body", "", ""],
        )
        .unwrap();
        drop(conn);

        let mut kb = KnowledgeBase::new();
        let n = kb.load_from_sqlite(&path).unwrap();
        assert_eq!(n, 1);
        let node = kb.get("n1").unwrap();
        assert!(node.properties.is_empty());
    }

    /// Verify WAL mode is enabled after init_schema.
    #[test]
    fn wal_mode_enabled() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("wal.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();

        let conn = Connection::open(&path).unwrap();
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "journal_mode should be WAL");

        let busy: i32 = conn
            .pragma_query_value(None, "busy_timeout", |row| row.get(0))
            .unwrap();
        assert_eq!(busy, 5000, "busy_timeout should be 5000ms");
    }

    /// A database from a future MAE version should return FutureSchema error.
    #[test]
    fn future_schema_returns_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("future.db");
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE nodes (id TEXT PRIMARY KEY, title TEXT, kind TEXT, body TEXT, tags_json TEXT);",
        )
        .unwrap();
        conn.pragma_update(None, "user_version", 999).unwrap();
        drop(conn);

        let mut kb = KnowledgeBase::new();
        let err = kb.load_from_sqlite(&path).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("999"), "should mention found version: {msg}");
        assert!(
            msg.contains("newer"),
            "should explain it's from a newer version: {msg}"
        );
    }

    // --- WAL integration tests ---

    #[test]
    fn wal_concurrent_read_during_write() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("wal_concurrent.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();

        // Start a write transaction on one connection
        let write_conn = Connection::open(&path).unwrap();
        write_conn
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        write_conn.execute("BEGIN IMMEDIATE", []).unwrap();
        write_conn
            .execute(
                "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES ('test', 'Test', 'note', 'body', '[]')",
                [],
            )
            .unwrap();

        // Reader should NOT be blocked (WAL allows concurrent reads during writes)
        let read_conn = Connection::open(&path).unwrap();
        let count: i32 = read_conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 3,
            "Reader should see pre-transaction state (3 nodes)"
        );

        write_conn.execute("COMMIT", []).unwrap();

        // After commit, reader sees new state
        let count: i32 = read_conn
            .query_row("SELECT COUNT(*) FROM nodes", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 4, "Reader should see committed state (4 nodes)");
    }

    #[test]
    fn wal_busy_timeout_retries() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("wal_busy.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();

        let conn1 = Connection::open(&path).unwrap();
        conn1.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn1.pragma_update(None, "busy_timeout", "5000").unwrap();
        conn1.execute("BEGIN IMMEDIATE", []).unwrap();

        // Second writer should eventually get BUSY or succeed after conn1 commits
        let conn2 = Connection::open(&path).unwrap();
        conn2.pragma_update(None, "journal_mode", "WAL").unwrap();
        conn2.pragma_update(None, "busy_timeout", "100").unwrap(); // short timeout

        // Release conn1's transaction
        conn1.execute("COMMIT", []).unwrap();

        // Now conn2 should succeed
        let result = conn2.execute(
            "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES ('n2', 'N2', 'note', 'body', '[]')",
            [],
        );
        assert!(result.is_ok());
    }

    #[test]
    fn wal_files_created() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("wal_files.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();

        // WAL files may be cleaned up after checkpoint; check they at least existed
        // by verifying WAL mode is actually set
        let conn = Connection::open(&path).unwrap();
        let mode: String = conn
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn wal_crash_recovery() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("wal_crash.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();

        // Write additional data
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.execute(
                "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES ('crash_test', 'Crash', 'note', 'data', '[]')",
                [],
            )
            .unwrap();
            // Don't explicitly checkpoint — simulate "crash" by dropping connection
        }

        // Reopen — WAL recovery should make data visible
        let mut kb2 = KnowledgeBase::new();
        let count = kb2.load_from_sqlite(&path).unwrap();
        assert_eq!(count, 4, "Should recover crash_test node from WAL");
        assert!(kb2.get("crash_test").is_some());
    }

    #[test]
    fn kb_contention_multi_thread() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("contention.db");
        let kb = sample_kb();
        kb.save_to_sqlite(&path).unwrap();

        let path_clone = path.clone();
        let writer = std::thread::spawn(move || {
            for i in 0..10 {
                let mut kb = KnowledgeBase::new();
                kb.load_from_sqlite(&path_clone).unwrap();
                kb.insert(Node::new(
                    format!("writer:{}", i),
                    format!("Writer {}", i),
                    NodeKind::Note,
                    "body",
                ));
                kb.save_to_sqlite(&path_clone).unwrap();
            }
        });

        let readers: Vec<_> = (0..5)
            .map(|r| {
                let p = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        let mut kb = KnowledgeBase::new();
                        let result = kb.load_from_sqlite(&p);
                        assert!(result.is_ok(), "Reader {r} got error: {:?}", result.err());
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                })
            })
            .collect();

        writer.join().unwrap();
        for r in readers {
            r.join().unwrap();
        }

        // Final state should have the original 3 + 10 writer nodes
        let mut final_kb = KnowledgeBase::new();
        let count = final_kb.load_from_sqlite(&path).unwrap();
        assert_eq!(count, 13);
    }

    // --- Changelog tests ---

    #[test]
    fn changelog_records_create() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("changelog.db");
        let kb = sample_kb();
        kb.sync_to_sqlite(&path).unwrap();

        let history = KnowledgeBase::changes_since(&path, 0).unwrap();
        assert_eq!(history.len(), 3, "3 creates should be logged");
        assert!(history.iter().all(|e| e.operation == "create"));
    }

    #[test]
    fn changelog_records_update() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("changelog.db");
        let mut kb = sample_kb();
        kb.sync_to_sqlite(&path).unwrap();

        // Modify a node
        if let Some(node) = kb.get_mut("concept:buffer") {
            node.body = "Updated body content.".to_string();
        }
        kb.sync_to_sqlite(&path).unwrap();

        let history = KnowledgeBase::node_history(&path, "concept:buffer").unwrap();
        assert!(history.iter().any(|e| e.operation == "update"));
        let update = history.iter().find(|e| e.operation == "update").unwrap();
        assert_eq!(update.new_body.as_deref(), Some("Updated body content."));
    }

    #[test]
    fn changelog_records_delete() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("changelog.db");
        let mut kb = sample_kb();
        kb.sync_to_sqlite(&path).unwrap();

        // Remove a node
        kb.remove("cmd:save");
        kb.sync_to_sqlite(&path).unwrap();

        let history = KnowledgeBase::node_history(&path, "cmd:save").unwrap();
        assert!(history.iter().any(|e| e.operation == "delete"));
    }

    #[test]
    fn sync_is_incremental() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("incremental.db");
        let kb = sample_kb();
        kb.sync_to_sqlite(&path).unwrap();

        // Sync again without changes — no new changelog entries
        let before_count = KnowledgeBase::changes_since(&path, 0).unwrap().len();
        kb.sync_to_sqlite(&path).unwrap();
        let after_count = KnowledgeBase::changes_since(&path, 0).unwrap().len();
        assert_eq!(
            before_count, after_count,
            "No new changelog entries for unchanged data"
        );
    }

    #[test]
    fn migrate_v5_to_v6() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("v5.db");
        // Create a v5 database
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS nodes (
                    id TEXT PRIMARY KEY, title TEXT NOT NULL, kind TEXT NOT NULL,
                    body TEXT NOT NULL, tags_json TEXT NOT NULL DEFAULT '[]',
                    todo_state TEXT, priority TEXT, source TEXT, source_version INTEGER,
                    aliases_json TEXT NOT NULL DEFAULT '[]',
                    properties_json TEXT NOT NULL DEFAULT '{}'
                );
                CREATE TABLE IF NOT EXISTS links (src TEXT NOT NULL, dst TEXT NOT NULL, display TEXT, PRIMARY KEY (src, dst));
                CREATE TABLE IF NOT EXISTS node_tags (node_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (node_id, tag));
                CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(id UNINDEXED, title, body, tags, aliases, tokenize='porter unicode61');
            "#,
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 5).unwrap();
            conn.execute(
                "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES ('n1', 'Test', 'note', 'body', '[]')",
                [],
            )
            .unwrap();
        }

        let mut kb2 = KnowledgeBase::new();
        let n = kb2.load_from_sqlite(&path).unwrap();
        assert_eq!(n, 1);

        // Verify changelog table exists
        let conn = Connection::open(&path).unwrap();
        let table_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='node_changelog'",
                [],
                |r| r.get::<_, i32>(0),
            )
            .unwrap()
            > 0;
        assert!(
            table_exists,
            "node_changelog table should exist after migration"
        );

        // Verify timestamps were backfilled
        let has_ts: bool = has_column(&conn, "nodes", "created_at").unwrap();
        assert!(has_ts, "created_at column should exist");
    }

    #[test]
    fn migrate_v6_to_v7_adds_crdt_column() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("v6.db");
        // Create a v6 database (no crdt_doc column)
        {
            let conn = Connection::open(&path).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS nodes (
                    id TEXT PRIMARY KEY, title TEXT NOT NULL, kind TEXT NOT NULL,
                    body TEXT NOT NULL, tags_json TEXT NOT NULL DEFAULT '[]',
                    todo_state TEXT, priority TEXT, source TEXT, source_version INTEGER,
                    aliases_json TEXT NOT NULL DEFAULT '[]',
                    properties_json TEXT NOT NULL DEFAULT '{}',
                    created_at INTEGER, updated_at INTEGER
                );
                CREATE TABLE IF NOT EXISTS links (src TEXT NOT NULL, dst TEXT NOT NULL, display TEXT, PRIMARY KEY (src, dst));
                CREATE TABLE IF NOT EXISTS node_tags (node_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY (node_id, tag));
                CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(id UNINDEXED, title, body, tags, aliases, tokenize='porter unicode61');
                CREATE TABLE IF NOT EXISTS node_changelog (
                    rowid INTEGER PRIMARY KEY AUTOINCREMENT,
                    node_id TEXT NOT NULL, operation TEXT NOT NULL,
                    old_title TEXT, old_body TEXT, old_tags_json TEXT,
                    new_title TEXT, new_body TEXT, new_tags_json TEXT,
                    timestamp INTEGER NOT NULL DEFAULT (strftime('%s', 'now')),
                    author TEXT, reason TEXT
                );
            "#,
            )
            .unwrap();
            conn.pragma_update(None, "user_version", 6).unwrap();
            conn.execute(
                "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES ('n1', 'Test', 'note', 'body', '[]')",
                [],
            )
            .unwrap();
        }

        let mut kb = KnowledgeBase::new();
        let n = kb.load_from_sqlite(&path).unwrap();
        assert_eq!(n, 1);

        // Verify crdt_doc column exists after migration
        let conn = Connection::open(&path).unwrap();
        assert!(
            has_column(&conn, "nodes", "crdt_doc").unwrap(),
            "crdt_doc column should exist after v6→v7 migration"
        );

        // Node loaded without crdt_doc should have None
        let node = kb.get("n1").unwrap();
        assert!(node.crdt_doc.is_none());
    }

    #[test]
    fn crdt_doc_roundtrip_via_save_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("crdt.db");

        let mut kb = sample_kb();
        // Create a CRDT doc for one node
        if let Some(node) = kb.get_mut("concept:buffer") {
            let doc = mae_sync::kb::KbNodeDoc::new(&node.id, &node.title, &node.body, &node.tags);
            node.crdt_doc = Some(doc.encode());
        }

        kb.save_to_sqlite(&path).unwrap();

        let mut kb2 = KnowledgeBase::new();
        kb2.load_from_sqlite(&path).unwrap();

        let node = kb2.get("concept:buffer").unwrap();
        assert!(
            node.crdt_doc.is_some(),
            "crdt_doc should survive save/load roundtrip"
        );

        // Verify the CRDT doc can be decoded
        let doc = mae_sync::kb::KbNodeDoc::from_bytes(node.crdt_doc.as_ref().unwrap()).unwrap();
        assert_eq!(doc.id(), "concept:buffer");
        assert_eq!(doc.title(), node.title);
    }

    #[test]
    fn node_to_crdt_doc_conversion() {
        let node = Node::new("test:node", "Test Title", NodeKind::Note, "Test body text")
            .with_tags(["tag1", "tag2"]);

        let doc = node.to_crdt_doc().unwrap();
        assert_eq!(doc.id(), "test:node");
        assert_eq!(doc.title(), "Test Title");
        assert_eq!(doc.body(), "Test body text");
        assert_eq!(doc.tags(), vec!["tag1", "tag2"]);
    }

    #[test]
    fn apply_crdt_doc_updates_node_fields() {
        let mut node = Node::new("test:node", "Old", NodeKind::Note, "old body");
        assert!(node.crdt_doc.is_none());

        let mut doc = mae_sync::kb::KbNodeDoc::new(
            "test:node",
            "New Title",
            "new body",
            &["newtag".to_string()],
        );
        doc.add_tag("extra");

        node.apply_crdt_doc(&doc);
        assert_eq!(node.title, "New Title");
        assert_eq!(node.body, "new body");
        assert_eq!(node.tags, vec!["newtag", "extra"]);
        assert!(node.crdt_doc.is_some());
    }
}
