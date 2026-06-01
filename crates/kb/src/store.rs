//! KbStore trait — database-agnostic KB persistence interface.
//!
//! Implementations:
//! - `SqliteKbStore` (v0.11.1, bridge period + fallback)
//! - `CozoKbStore` (v0.12.0+, behind `cozo` feature flag)
//!
//! The in-memory `KnowledgeBase` remains the hot cache; `KbStore` is the
//! durable persistence layer that backs it.

use crate::{Node, NodeKind};
use std::path::PathBuf;

/// A link between two KB nodes.
#[derive(Debug, Clone)]
pub struct Link {
    pub src: String,
    pub dst: String,
    pub rel_type: String,
    pub display: Option<String>,
}

/// A search hit from FTS.
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub id: String,
    pub score: f64,
}

/// A pending CRDT update queued for sync.
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub rowid: i64,
    pub kb_id: String,
    pub node_id: String,
    pub update_bytes: Vec<u8>,
}

/// Subgraph result from neighborhood queries.
#[derive(Debug, Clone)]
pub struct SubGraph {
    pub nodes: Vec<(String, String)>,         // (id, title)
    pub edges: Vec<(String, String, String)>, // (src, dst, rel_type)
}

/// Error type for KbStore operations.
#[derive(Debug)]
pub enum KbStoreError {
    /// Operation not supported by this backend (e.g., graph algorithms on SQLite).
    NotSupported(String),
    /// Underlying storage error.
    Storage(String),
    /// Node not found.
    NotFound(String),
}

impl std::fmt::Display for KbStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotSupported(msg) => write!(f, "not supported: {msg}"),
            Self::Storage(msg) => write!(f, "storage error: {msg}"),
            Self::NotFound(id) => write!(f, "node not found: {id}"),
        }
    }
}

impl std::error::Error for KbStoreError {}

impl From<crate::PersistError> for KbStoreError {
    fn from(e: crate::PersistError) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<rusqlite::Error> for KbStoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.to_string())
    }
}

/// Database-agnostic KB persistence interface.
///
/// Implementations: `SqliteKbStore` (v0.11.1), `CozoKbStore` (v0.12.0+).
///
/// The trait is designed to be object-safe (`dyn KbStore`) so backends can
/// be swapped at runtime based on configuration.
pub trait KbStore: Send + Sync {
    // --- Node CRUD ---

    fn insert_node(&self, node: &Node) -> Result<(), KbStoreError>;
    fn update_node(&self, node: &Node) -> Result<(), KbStoreError>;
    fn delete_node(&self, id: &str) -> Result<(), KbStoreError>;
    fn get_node(&self, id: &str) -> Result<Option<Node>, KbStoreError>;
    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError>;

    // --- Search ---

    fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError>;

    // --- Links ---

    fn add_link(&self, src: &str, dst: &str, display: Option<&str>) -> Result<(), KbStoreError>;
    fn remove_link(&self, src: &str, dst: &str) -> Result<(), KbStoreError>;
    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError>;
    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError>;

    // --- CRDT ---

    fn get_crdt_doc(&self, id: &str) -> Result<Option<Vec<u8>>, KbStoreError>;
    fn update_crdt_doc(&self, id: &str, doc: &[u8]) -> Result<(), KbStoreError>;

    // --- Offline queue ---

    fn push_pending_update(
        &self,
        kb_id: &str,
        node_id: &str,
        update: &[u8],
    ) -> Result<(), KbStoreError>;
    fn drain_pending_updates(&self) -> Result<Vec<PendingUpdate>, KbStoreError>;
    fn ack_pending_update(&self, id: i64) -> Result<(), KbStoreError>;

    // --- Bulk operations ---

    fn load_all(&self) -> Result<Vec<Node>, KbStoreError>;
    fn save_all(&self, nodes: &[&Node]) -> Result<(), KbStoreError>;

    // --- Graph queries (optional, default NotSupported) ---

    /// Add a typed relationship between two nodes.
    fn add_typed_link(
        &self,
        _src: &str,
        _dst: &str,
        _rel_type: &str,
        _weight: f64,
    ) -> Result<(), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "typed links require CozoDB backend".into(),
        ))
    }

    /// Query links by relationship type.
    fn links_typed(&self, _id: &str, _rel_type: &str) -> Result<Vec<Link>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "typed link queries require CozoDB backend".into(),
        ))
    }

    /// Find shortest path between two nodes (BFS).
    fn shortest_path(&self, _from: &str, _to: &str) -> Result<Vec<String>, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "shortest path requires CozoDB backend".into(),
        ))
    }

    /// BFS neighborhood subgraph around a node up to `depth` hops.
    fn neighborhood(&self, _id: &str, _depth: u32) -> Result<SubGraph, KbStoreError> {
        Err(KbStoreError::NotSupported(
            "neighborhood queries require CozoDB backend".into(),
        ))
    }

    /// Execute a raw backend query (Datalog for CozoDB, SQL for SQLite).
    fn raw_query(&self, _script: &str) -> Result<(Vec<String>, Vec<Vec<String>>), KbStoreError> {
        Err(KbStoreError::NotSupported(
            "raw queries require CozoDB backend".into(),
        ))
    }

    // --- Lifecycle ---

    fn backend_name(&self) -> &str;
    fn db_path(&self) -> &std::path::Path;
}

// ---------------------------------------------------------------------------
// SqliteKbStore implementation
// ---------------------------------------------------------------------------

use crate::persist::kind_to_str;
use rusqlite::{params, Connection};
use std::sync::Mutex;

/// SQLite-backed KbStore. Wraps a single connection in a Mutex for thread safety.
pub struct SqliteKbStore {
    conn: Mutex<Connection>,
    path: PathBuf,
}

impl SqliteKbStore {
    /// Open (or create) a SQLite KB at the given path.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, KbStoreError> {
        let path = path.into();
        let conn = Connection::open(&path).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        // Run schema init + migrations
        crate::persist::init_and_migrate(&conn)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        // Create pending_updates table if not exists
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS pending_updates (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                kb_id       TEXT NOT NULL,
                node_id     TEXT NOT NULL,
                update_bytes BLOB NOT NULL,
                created_at  TEXT NOT NULL DEFAULT (datetime('now'))
            );
            "#,
        )
        .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        Ok(Self {
            conn: Mutex::new(conn),
            path,
        })
    }

    fn now_epoch(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

impl std::fmt::Debug for SqliteKbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteKbStore")
            .field("path", &self.path)
            .finish()
    }
}

fn source_to_str(s: crate::NodeSource) -> &'static str {
    match s {
        crate::NodeSource::Seed => "seed",
        crate::NodeSource::UserOrg => "user_org",
        crate::NodeSource::Manual => "manual",
        crate::NodeSource::Federation => "federation",
    }
}

fn str_to_source(s: &str) -> crate::NodeSource {
    match s {
        "seed" => crate::NodeSource::Seed,
        "user_org" => crate::NodeSource::UserOrg,
        "manual" => crate::NodeSource::Manual,
        "federation" => crate::NodeSource::Federation,
        _ => crate::NodeSource::Manual,
    }
}

fn str_to_kind(s: &str) -> NodeKind {
    match s {
        "index" => NodeKind::Index,
        "command" => NodeKind::Command,
        "concept" => NodeKind::Concept,
        "key" => NodeKind::Key,
        "project" => NodeKind::Project,
        _ => NodeKind::Note,
    }
}

impl KbStore for SqliteKbStore {
    fn insert_node(&self, node: &Node) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let now = self.now_epoch();
        let tags_json =
            serde_json::to_string(&node.tags).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let aliases_json = serde_json::to_string(&node.aliases)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let properties_json = serde_json::to_string(&node.properties)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let pri_str = node.priority.map(|c| c.to_string());
        let source_str = node.source.map(source_to_str);

        conn.execute(
            "INSERT OR REPLACE INTO nodes (id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, created_at, updated_at, crdt_doc) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
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
            ],
        )?;

        // Update FTS
        conn.execute("DELETE FROM nodes_fts WHERE id = ?1", params![&node.id])?;
        conn.execute(
            "INSERT INTO nodes_fts (id, title, body, tags, aliases) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                &node.id,
                &node.title,
                &node.body,
                node.tags.join(" "),
                node.aliases.join(" "),
            ],
        )?;

        // Update tags
        conn.execute(
            "DELETE FROM node_tags WHERE node_id = ?1",
            params![&node.id],
        )?;
        for tag in &node.tags {
            conn.execute(
                "INSERT OR IGNORE INTO node_tags (node_id, tag) VALUES (?1, ?2)",
                params![&node.id, tag],
            )?;
        }

        // Update links
        conn.execute("DELETE FROM links WHERE src = ?1", params![&node.id])?;
        for (dst, display) in crate::parse_links(&node.body) {
            let disp: Option<&str> = if dst == display {
                None
            } else {
                Some(display.as_str())
            };
            conn.execute(
                "INSERT OR IGNORE INTO links (src, dst, display) VALUES (?1, ?2, ?3)",
                params![&node.id, &dst, disp],
            )?;
        }

        Ok(())
    }

    fn update_node(&self, node: &Node) -> Result<(), KbStoreError> {
        // For SQLite, update is the same as insert (UPSERT via INSERT OR REPLACE).
        self.insert_node(node)
    }

    fn delete_node(&self, id: &str) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM nodes WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM nodes_fts WHERE id = ?1", params![id])?;
        conn.execute("DELETE FROM node_tags WHERE node_id = ?1", params![id])?;
        conn.execute("DELETE FROM links WHERE src = ?1", params![id])?;
        Ok(())
    }

    fn get_node(&self, id: &str) -> Result<Option<Node>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc FROM nodes WHERE id = ?1",
        )?;
        let result = stmt.query_row(params![id], |row| {
            let id: String = row.get(0)?;
            let title: String = row.get(1)?;
            let kind: String = row.get(2)?;
            let body: String = row.get(3)?;
            let tags_json: String = row.get(4)?;
            let todo_state: Option<String> = row.get(5)?;
            let priority_str: Option<String> = row.get(6)?;
            let source_str: Option<String> = row.get(7)?;
            let source_version: Option<u32> = row.get(8)?;
            let aliases_json: String = row.get(9)?;
            let properties_json: String = row.get(10)?;
            let crdt_doc: Option<Vec<u8>> = row.get(11)?;
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
        });

        match result {
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
            )) => {
                let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
                let aliases: Vec<String> = serde_json::from_str(&aliases_json).unwrap_or_default();
                let properties: std::collections::HashMap<String, String> =
                    serde_json::from_str(&properties_json).unwrap_or_default();
                let priority = priority_str.and_then(|s| s.chars().next());
                let source = source_str.as_deref().map(str_to_source);

                let mut node = Node::new(id, title, str_to_kind(&kind), body)
                    .with_tags(tags)
                    .with_aliases(aliases)
                    .with_properties(properties);
                node.todo_state = todo_state;
                node.priority = priority;
                node.source = source;
                node.source_version = source_version;
                node.crdt_doc = crdt_doc;
                Ok(Some(node))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(KbStoreError::from(e)),
        }
    }

    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut ids = Vec::new();
        match prefix {
            Some(p) => {
                let pattern = format!("{p}%");
                let mut stmt = conn.prepare("SELECT id FROM nodes WHERE id LIKE ?1 ORDER BY id")?;
                let rows = stmt.query_map(params![pattern], |row| row.get::<_, String>(0))?;
                for r in rows {
                    ids.push(r?);
                }
            }
            None => {
                let mut stmt = conn.prepare("SELECT id FROM nodes ORDER BY id")?;
                let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
                for r in rows {
                    ids.push(r?);
                }
            }
        }
        Ok(ids)
    }

    fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, bm25(nodes_fts) as score FROM nodes_fts WHERE nodes_fts MATCH ?1 ORDER BY score LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit as i64], |row| {
            Ok(SearchHit {
                id: row.get(0)?,
                score: row.get(1)?,
            })
        })?;
        let mut hits = Vec::new();
        for r in rows {
            hits.push(r?);
        }
        Ok(hits)
    }

    fn add_link(&self, src: &str, dst: &str, display: Option<&str>) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO links (src, dst, display) VALUES (?1, ?2, ?3)",
            params![src, dst, display],
        )?;
        Ok(())
    }

    fn remove_link(&self, src: &str, dst: &str) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM links WHERE src = ?1 AND dst = ?2",
            params![src, dst],
        )?;
        Ok(())
    }

    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT src, dst, display FROM links WHERE src = ?1 ORDER BY dst")?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(Link {
                src: row.get(0)?,
                dst: row.get(1)?,
                rel_type: "related_to".to_string(),
                display: row.get(2)?,
            })
        })?;
        let mut links = Vec::new();
        for r in rows {
            links.push(r?);
        }
        Ok(links)
    }

    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT src, dst, display FROM links WHERE dst = ?1 ORDER BY src")?;
        let rows = stmt.query_map(params![id], |row| {
            Ok(Link {
                src: row.get(0)?,
                dst: row.get(1)?,
                rel_type: "related_to".to_string(),
                display: row.get(2)?,
            })
        })?;
        let mut links = Vec::new();
        for r in rows {
            links.push(r?);
        }
        Ok(links)
    }

    fn get_crdt_doc(&self, id: &str) -> Result<Option<Vec<u8>>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let result: Result<Option<Vec<u8>>, _> = conn.query_row(
            "SELECT crdt_doc FROM nodes WHERE id = ?1",
            params![id],
            |r| r.get(0),
        );
        match result {
            Ok(doc) => Ok(doc),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(KbStoreError::from(e)),
        }
    }

    fn update_crdt_doc(&self, id: &str, doc: &[u8]) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let now = self.now_epoch();
        conn.execute(
            "UPDATE nodes SET crdt_doc = ?1, updated_at = ?2 WHERE id = ?3",
            params![doc, now, id],
        )?;
        Ok(())
    }

    fn push_pending_update(
        &self,
        kb_id: &str,
        node_id: &str,
        update: &[u8],
    ) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO pending_updates (kb_id, node_id, update_bytes) VALUES (?1, ?2, ?3)",
            params![kb_id, node_id, update],
        )?;
        Ok(())
    }

    fn drain_pending_updates(&self) -> Result<Vec<PendingUpdate>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT id, kb_id, node_id, update_bytes FROM pending_updates ORDER BY id")?;
        let rows = stmt.query_map([], |row| {
            Ok(PendingUpdate {
                rowid: row.get(0)?,
                kb_id: row.get(1)?,
                node_id: row.get(2)?,
                update_bytes: row.get(3)?,
            })
        })?;
        let mut updates = Vec::new();
        for r in rows {
            updates.push(r?);
        }
        Ok(updates)
    }

    fn ack_pending_update(&self, id: i64) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM pending_updates WHERE id = ?1", params![id])?;
        Ok(())
    }

    fn load_all(&self) -> Result<Vec<Node>, KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc FROM nodes ORDER BY id",
        )?;
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
            let aliases_json: String = row.get(9)?;
            let properties_json: String = row.get(10)?;
            let crdt_doc: Option<Vec<u8>> = row.get(11)?;
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

        let mut nodes = Vec::new();
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
            let source = source_str.as_deref().map(str_to_source);

            let mut node = Node::new(id, title, str_to_kind(&kind), body)
                .with_tags(tags)
                .with_aliases(aliases)
                .with_properties(properties);
            node.todo_state = todo_state;
            node.priority = priority;
            node.source = source;
            node.source_version = source_version;
            node.crdt_doc = crdt_doc;
            nodes.push(node);
        }
        Ok(nodes)
    }

    fn save_all(&self, nodes: &[&Node]) -> Result<(), KbStoreError> {
        let conn = self.conn.lock().unwrap();
        let now = self.now_epoch();
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;

        tx.execute("DELETE FROM nodes", [])
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        tx.execute("DELETE FROM links", [])
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        tx.execute("DELETE FROM nodes_fts", [])
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        tx.execute("DELETE FROM node_tags", [])
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;

        for node in nodes {
            let tags_json = serde_json::to_string(&node.tags)
                .map_err(|e| KbStoreError::Storage(e.to_string()))?;
            let aliases_json = serde_json::to_string(&node.aliases)
                .map_err(|e| KbStoreError::Storage(e.to_string()))?;
            let properties_json = serde_json::to_string(&node.properties)
                .map_err(|e| KbStoreError::Storage(e.to_string()))?;
            let pri_str = node.priority.map(|c| c.to_string());
            let source_str = node.source.map(source_to_str);

            tx.execute(
                "INSERT INTO nodes (id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, created_at, updated_at, crdt_doc) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                params![
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
                ],
            ).map_err(|e| KbStoreError::Storage(e.to_string()))?;

            tx.execute(
                "INSERT INTO nodes_fts (id, title, body, tags, aliases) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    &node.id,
                    &node.title,
                    &node.body,
                    node.tags.join(" "),
                    node.aliases.join(" "),
                ],
            ).map_err(|e| KbStoreError::Storage(e.to_string()))?;

            for tag in &node.tags {
                tx.execute(
                    "INSERT OR IGNORE INTO node_tags (node_id, tag) VALUES (?1, ?2)",
                    params![&node.id, tag],
                )
                .map_err(|e| KbStoreError::Storage(e.to_string()))?;
            }

            for (dst, display) in crate::parse_links(&node.body) {
                let disp: Option<&str> = if dst == display {
                    None
                } else {
                    Some(display.as_str())
                };
                tx.execute(
                    "INSERT OR IGNORE INTO links (src, dst, display) VALUES (?1, ?2, ?3)",
                    params![&node.id, &dst, disp],
                )
                .map_err(|e| KbStoreError::Storage(e.to_string()))?;
            }
        }

        tx.commit()
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        Ok(())
    }

    fn backend_name(&self) -> &str {
        "sqlite"
    }

    fn db_path(&self) -> &std::path::Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NodeKind;

    fn make_store() -> (tempfile::TempDir, SqliteKbStore) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test_kb.db");
        let store = SqliteKbStore::open(&path).unwrap();
        (tmp, store)
    }

    #[test]
    fn insert_and_get_node() {
        let (_tmp, store) = make_store();
        let node = Node::new("test:1", "Test Node", NodeKind::Note, "Hello world")
            .with_tags(["tag1", "tag2"]);
        store.insert_node(&node).unwrap();

        let loaded = store.get_node("test:1").unwrap().unwrap();
        assert_eq!(loaded.title, "Test Node");
        assert_eq!(loaded.body, "Hello world");
        assert_eq!(loaded.tags, vec!["tag1", "tag2"]);
    }

    #[test]
    fn get_missing_node_returns_none() {
        let (_tmp, store) = make_store();
        assert!(store.get_node("nonexistent").unwrap().is_none());
    }

    #[test]
    fn delete_node_removes_it() {
        let (_tmp, store) = make_store();
        let node = Node::new("del:1", "Delete Me", NodeKind::Note, "body");
        store.insert_node(&node).unwrap();
        store.delete_node("del:1").unwrap();
        assert!(store.get_node("del:1").unwrap().is_none());
    }

    #[test]
    fn fts_search_finds_nodes() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "n1",
                "Quantum Physics",
                NodeKind::Note,
                "Entanglement is spooky.",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "n2",
                "Classical Mechanics",
                NodeKind::Note,
                "Newton was right.",
            ))
            .unwrap();

        let hits = store.fts_search("quantum", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "n1");
    }

    #[test]
    fn list_ids_with_prefix() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("cmd:save", "Save", NodeKind::Command, ""))
            .unwrap();
        store
            .insert_node(&Node::new("cmd:quit", "Quit", NodeKind::Command, ""))
            .unwrap();
        store
            .insert_node(&Node::new(
                "concept:buffer",
                "Buffer",
                NodeKind::Concept,
                "",
            ))
            .unwrap();

        let cmd_ids = store.list_ids(Some("cmd:")).unwrap();
        assert_eq!(cmd_ids.len(), 2);
        let all_ids = store.list_ids(None).unwrap();
        assert_eq!(all_ids.len(), 3);
    }

    #[test]
    fn links_from_and_to() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "a",
                "A",
                NodeKind::Note,
                "See [[b]] for details.",
            ))
            .unwrap();
        store
            .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
            .unwrap();

        let from_a = store.links_from("a").unwrap();
        assert_eq!(from_a.len(), 1);
        assert_eq!(from_a[0].dst, "b");

        let to_b = store.links_to("b").unwrap();
        assert_eq!(to_b.len(), 1);
        assert_eq!(to_b[0].src, "a");
    }

    #[test]
    fn pending_updates_lifecycle() {
        let (_tmp, store) = make_store();

        store
            .push_pending_update("kb-1", "node-a", &[1, 2, 3])
            .unwrap();
        store
            .push_pending_update("kb-1", "node-b", &[4, 5, 6])
            .unwrap();

        let pending = store.drain_pending_updates().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].node_id, "node-a");
        assert_eq!(pending[1].node_id, "node-b");

        // Ack first
        store.ack_pending_update(pending[0].rowid).unwrap();
        let remaining = store.drain_pending_updates().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].node_id, "node-b");
    }

    #[test]
    fn load_all_and_save_all() {
        let (_tmp, store) = make_store();
        let n1 = Node::new("n1", "One", NodeKind::Note, "body1");
        let n2 = Node::new("n2", "Two", NodeKind::Note, "body2");

        store.save_all(&[&n1, &n2]).unwrap();
        let loaded = store.load_all().unwrap();
        assert_eq!(loaded.len(), 2);
    }

    #[test]
    fn crdt_doc_persistence() {
        let (_tmp, store) = make_store();
        let mut node = Node::new("crdt:1", "CRDT Node", NodeKind::Note, "body");
        node.crdt_doc = Some(vec![10, 20, 30, 40]);
        store.insert_node(&node).unwrap();

        let doc = store.get_crdt_doc("crdt:1").unwrap();
        assert_eq!(doc, Some(vec![10, 20, 30, 40]));

        store.update_crdt_doc("crdt:1", &[50, 60]).unwrap();
        let doc = store.get_crdt_doc("crdt:1").unwrap();
        assert_eq!(doc, Some(vec![50, 60]));
    }

    #[test]
    fn update_node_is_upsert() {
        let (_tmp, store) = make_store();
        let node = Node::new("up:1", "Original", NodeKind::Note, "old body");
        store.insert_node(&node).unwrap();

        let updated = Node::new("up:1", "Updated", NodeKind::Note, "new body");
        store.update_node(&updated).unwrap();

        let loaded = store.get_node("up:1").unwrap().unwrap();
        assert_eq!(loaded.title, "Updated");
        assert_eq!(loaded.body, "new body");
    }

    #[test]
    fn backend_name_is_sqlite() {
        let (_tmp, store) = make_store();
        assert_eq!(store.backend_name(), "sqlite");
    }
}
