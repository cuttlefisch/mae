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

const SCHEMA_VERSION: i32 = 1;

/// Error type wrapping rusqlite and serde errors for the persistence layer.
#[derive(Debug)]
pub enum PersistError {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    SchemaMismatch { found: i32, expected: i32 },
}

impl std::fmt::Display for PersistError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Json(e) => write!(f, "json: {e}"),
            Self::SchemaMismatch { found, expected } => {
                write!(f, "KB schema v{found} found, expected v{expected}")
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
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS nodes (
            id        TEXT PRIMARY KEY,
            title     TEXT NOT NULL,
            kind      TEXT NOT NULL,
            body      TEXT NOT NULL,
            tags_json TEXT NOT NULL DEFAULT '[]'
        );
        CREATE TABLE IF NOT EXISTS links (
            src     TEXT NOT NULL,
            dst     TEXT NOT NULL,
            display TEXT,
            PRIMARY KEY (src, dst)
        );
        CREATE INDEX IF NOT EXISTS idx_links_dst ON links(dst);
        CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
            id UNINDEXED,
            title,
            body,
            tags,
            tokenize='porter unicode61'
        );
        "#,
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn check_schema_version(conn: &Connection) -> Result<(), PersistError> {
    let found: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if found != 0 && found != SCHEMA_VERSION {
        return Err(PersistError::SchemaMismatch {
            found,
            expected: SCHEMA_VERSION,
        });
    }
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
        {
            let mut ins_node = tx.prepare(
                "INSERT INTO nodes (id, title, kind, body, tags_json) VALUES (?, ?, ?, ?, ?)",
            )?;
            let mut ins_link =
                tx.prepare("INSERT OR IGNORE INTO links (src, dst, display) VALUES (?, ?, ?)")?;
            let mut ins_fts =
                tx.prepare("INSERT INTO nodes_fts (id, title, body, tags) VALUES (?, ?, ?, ?)")?;
            for node in self.nodes_values() {
                let tags_json = serde_json::to_string(&node.tags)?;
                ins_node.execute(params![
                    &node.id,
                    &node.title,
                    kind_to_str(node.kind),
                    &node.body,
                    &tags_json,
                ])?;
                ins_fts.execute(params![
                    &node.id,
                    &node.title,
                    &node.body,
                    node.tags.join(" "),
                ])?;
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
        let mut stmt =
            conn.prepare("SELECT id, title, kind, body, tags_json FROM nodes ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let body: String = row.get(3)?;
            let tags_json: String = row.get(4)?;
            Ok((id, title, kind, body, tags_json))
        })?;
        let mut count = 0;
        for row in rows {
            let (id, title, kind, body, tags_json) = row?;
            let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
            self.insert(Node::new(id, title, kind_from_str(&kind), body).with_tags(tags));
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
}
