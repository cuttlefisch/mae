//! SQLite + FTS5 persistence for the knowledge base.
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

const SCHEMA_VERSION: i32 = 5;

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
            properties_json TEXT NOT NULL DEFAULT '{}'
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
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM nodes", [])?;
        tx.execute("DELETE FROM links", [])?;
        tx.execute("DELETE FROM nodes_fts", [])?;
        tx.execute("DELETE FROM node_tags", [])?;
        {
            let mut ins_node = tx.prepare(
                "INSERT INTO nodes (id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
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
            }
        }
        tx.commit()?;
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
        // Check if optional columns exist (pre-v4/v5 databases may not have them).
        let has_aliases = has_column(&conn, "nodes", "aliases_json")?;
        let has_properties = has_column(&conn, "nodes", "properties_json")?;
        let query_str = match (has_aliases, has_properties) {
            (true, true) => "SELECT id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json FROM nodes ORDER BY id",
            (true, false) => "SELECT id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json FROM nodes ORDER BY id",
            _ => "SELECT id, title, kind, body, tags_json, todo_state, priority, source, source_version FROM nodes ORDER BY id",
        };
        let mut stmt = conn.prepare(query_str)?;
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
            let aliases_json: String = if has_aliases {
                row.get(9)?
            } else {
                "[]".to_string()
            };
            let properties_json: String = if has_properties {
                row.get(10)?
            } else {
                "{}".to_string()
            };
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
            node.source_version = source_version;
            self.insert(node);
            count += 1;
        }
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
}
