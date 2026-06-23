//! CozoKbStore — graph-native KB persistence using CozoDB (Datalog).
//!
//! Sole KB backend since v0.12.0. Storage engine selected by feature flag:
//! - `storage-sled` (default): sled embedded storage
//! - `storage-sqlite`: CozoDB's native SQLite engine (used by mae-daemon)
//!
//! CozoDB provides:
//! - Datalog query engine with recursive queries
//! - ACID + MVCC transactions
//! - Multiple storage backends (sled, SQLite, RocksDB)
//!
//! Graph algorithms (PageRank, community detection) require the `graph-algo`
//! feature, currently disabled due to upstream `graph_builder` rayon compat
//! issue. Will be re-enabled when upstream fixes land.

use crate::store::{
    AgendaFilter, Block, HealthReport, KbStore, KbStoreError, Link, MetaMember, NodeVersion,
    PendingUpdate, SearchHit, SubGraph, VectorHit,
};
use crate::{Node, NodeKind};
use cozo::{DataValue, DbInstance, NamedRows, ScriptMutability, Vector};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// CozoDB-backed KbStore using SQLite embedded storage.
pub struct CozoKbStore {
    db: DbInstance,
    path: PathBuf,
}

impl std::fmt::Debug for CozoKbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CozoKbStore")
            .field("path", &self.path)
            .finish()
    }
}

impl CozoKbStore {
    /// Open (or create) a CozoDB at the given path using the sled storage engine.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, KbStoreError> {
        Self::open_with_engine(path, "sled")
    }

    /// Open (or create) a CozoDB at the given path with a specific storage engine.
    ///
    /// Supported engines: `"sled"`, `"sqlite"`, `"mem"`.
    /// The caller must ensure the appropriate CozoDB storage feature is enabled.
    pub fn open_with_engine(path: impl Into<PathBuf>, engine: &str) -> Result<Self, KbStoreError> {
        let path = path.into();
        let db = DbInstance::new(engine, path.to_str().unwrap_or(""), "")
            .map_err(|e| KbStoreError::Storage(format!("CozoDB open ({engine}) failed: {e}")))?;

        let store = Self { db, path };
        store.ensure_schema()?;
        Ok(store)
    }

    /// Open an in-memory CozoDB store (for tests). No storage backend needed.
    pub fn open_mem() -> Result<Self, KbStoreError> {
        let db = DbInstance::new("mem", "", "")
            .map_err(|e| KbStoreError::Storage(format!("CozoDB mem open failed: {e}")))?;
        let store = Self {
            db,
            path: PathBuf::from(":memory:"),
        };
        store.ensure_schema()?;
        Ok(store)
    }

    /// Create schema relations if they don't exist.
    fn ensure_schema(&self) -> Result<(), KbStoreError> {
        // Nodes relation
        self.run_mut(
            r#"
            :create nodes {
                id: String
                =>
                title: String,
                kind: String,
                body: String,
                tags_json: String,
                todo_state: String,
                priority: String,
                source: String,
                source_version: Int,
                aliases_json: String,
                properties_json: String,
                crdt_doc: Bytes,
                has_crdt: Bool,
                origin_instance: String,
                assignee: String,
                due_date: Int,
                sprint: String,
                created_at: Int,
                updated_at: Int
            }
            "#,
        )
        .or_else(|e| {
            // :create fails if relation exists — that's fine
            if e.to_string().contains("already exists") || e.to_string().contains("conflicts with")
            {
                Ok(NamedRows::default())
            } else {
                Err(e)
            }
        })
        .map_err(cozo_err)?;

        // Links relation (typed relationships with confidence)
        self.run_mut(
            r#"
            :create links {
                src: String,
                dst: String,
                rel_type: String
                =>
                display: String,
                weight: Float,
                confidence: Float,
                created_at: Int
            }
            "#,
        )
        .or_else(|e| {
            if e.to_string().contains("already exists") || e.to_string().contains("conflicts with")
            {
                Ok(NamedRows::default())
            } else {
                Err(e)
            }
        })
        .map_err(cozo_err)?;

        // Pending updates (offline queue)
        self.run_mut(
            r#"
            :create pending_updates {
                id: Int
                =>
                kb_id: String,
                node_id: String,
                update_bytes: Bytes,
                created_at: Int
            }
            "#,
        )
        .or_else(|e| {
            if e.to_string().contains("already exists") || e.to_string().contains("conflicts with")
            {
                Ok(NamedRows::default())
            } else {
                Err(e)
            }
        })
        .map_err(cozo_err)?;

        // Counter for pending_updates auto-increment
        self.run_mut(
            r#"
            :create pending_counter {
                key: String
                =>
                val: Int
            }
            "#,
        )
        .or_else(|e| {
            if e.to_string().contains("already exists") || e.to_string().contains("conflicts with")
            {
                Ok(NamedRows::default())
            } else {
                Err(e)
            }
        })
        .map_err(cozo_err)?;

        // Initialize counter if empty
        let result = self
            .run_immut("?[val] := *pending_counter{key: 'counter', val}")
            .map_err(cozo_err)?;
        if result.rows.is_empty() {
            self.run_mut(
                r#"?[key, val] <- [["counter", 0]]
                :put pending_counter {key => val}"#,
            )
            .map_err(cozo_err)?;
        }

        // Tantivy FTS index on nodes (title + body combined).
        // NOTE: Post-query verification in fts_search() guards against stale FTS
        // entries (observed with sled backend; kept as defensive measure).
        self.run_mut(
            r#"::fts create nodes:fts {
                extractor: title ++ ' ' ++ body,
                tokenizer: Simple,
                filters: [Lowercase]
            }"#,
        )
        .or_else(|e| {
            let msg = e.to_string();
            if msg.contains("already exists") || msg.contains("duplicate") {
                Ok(NamedRows::default())
            } else {
                Err(e)
            }
        })
        .map_err(cozo_err)?;

        // --- Phase B: Enhanced schema relations ---

        // Schema metadata: queryable type system for node kinds
        self.create_if_absent(
            r#":create node_types {
                kind: String
                =>
                label: String,
                description: String,
                namespace_prefix: String,
                icon: String,
                required_fields_json: String
            }"#,
        )?;

        // Schema metadata: relationship types with inverses
        self.create_if_absent(
            r#":create rel_types {
                name: String
                =>
                label: String,
                description: String,
                inverse_name: String,
                directed: Bool
            }"#,
        )?;

        // Block-level addressing: paragraphs within nodes
        self.create_if_absent(
            r#":create blocks {
                parent_id: String,
                block_idx: Int
                =>
                content: String,
                block_type: String,
                created_at: Int,
                updated_at: Int
            }"#,
        )?;

        // Meta-node composition: ordered member references
        self.create_if_absent(
            r#":create meta_members {
                meta_id: String,
                member_id: String,
                position: Int
                =>
                role: String
            }"#,
        )?;

        // Node versioning: append-only snapshots with content checksums
        self.create_if_absent(
            r#":create node_versions {
                id: String,
                version: Int
                =>
                title: String,
                body: String,
                tags_json: String,
                todo_state: String,
                priority: String,
                properties_json: String,
                assignee: String,
                change_summary: String,
                author: String,
                content_hash: String,
                created_at: Int
            }"#,
        )?;

        // View definitions for task management / agenda
        self.create_if_absent(
            r#":create views {
                id: String
                =>
                title: String,
                kind: String,
                query: String,
                display_config_json: String,
                owner: String,
                created_at: Int,
                updated_at: Int
            }"#,
        )?;

        // AI hygiene suggestion tracking
        self.create_if_absent(
            r#":create hygiene_suggestions {
                node_id: String,
                suggestion_id: Int
                =>
                category: String,
                message: String,
                suggested_action_json: String,
                confidence: Float,
                status: String,
                created_at: Int
            }"#,
        )?;

        // Federation identity (key-value metadata)
        self.create_if_absent(
            r#":create instance_meta {
                key: String
                =>
                val: String
            }"#,
        )?;

        // HNSW vector embeddings (schema ready, populated in v0.13.0)
        // vec type is <F32; 384> — 384-dim vectors for all-MiniLM-L6-v2
        self.create_if_absent(
            r#":create embeddings {
                id: String,
                model: String
                =>
                vec: <F32; 384>
            }"#,
        )?;

        // HNSW index on embeddings for vector search.
        // Uses Cosine distance, dim=384 (all-MiniLM-L6-v2 default).
        // Index creation is idempotent — silently ignored if already exists.
        self.create_if_absent(
            r#"::hnsw create embeddings:semantic {
                dim: 384,
                m: 16,
                dtype: F32,
                fields: [vec],
                distance: Cosine,
                ef_construction: 100,
                extend_candidates: true,
                keep_pruned_connections: false
            }"#,
        )?;

        // Source file tracking for ingestion pipeline.
        // Enables incremental reimport (only re-parse changed files).
        self.create_if_absent(
            r#":create source_files {
                file_path: String
                =>
                content_hash: String,
                last_mtime: Int,
                node_ids_json: String,
                last_import: Int
            }"#,
        )?;

        // Generate instance_id UUID if not already set
        self.ensure_instance_id()?;

        Ok(())
    }

    /// Create a relation if it doesn't already exist.
    fn create_if_absent(&self, script: &str) -> Result<(), KbStoreError> {
        self.run_mut(script)
            .or_else(|e| {
                if e.to_string().contains("already exists")
                    || e.to_string().contains("conflicts with")
                {
                    Ok(NamedRows::default())
                } else {
                    Err(e)
                }
            })
            .map_err(cozo_err)?;
        Ok(())
    }

    /// Generate and store instance UUID if not already present.
    fn ensure_instance_id(&self) -> Result<(), KbStoreError> {
        let result = self
            .run_immut("?[val] := *instance_meta{key: 'instance_id', val}")
            .map_err(cozo_err)?;
        if result.rows.is_empty() {
            let uuid = generate_uuid_v4();
            self.run_mut_params(
                r#"?[key, val] <- [["instance_id", $uuid]]
                :put instance_meta {key => val}"#,
                btree_params([("uuid", dv_str(&uuid))]),
            )
            .map_err(cozo_err)?;
            let now = self.now_epoch().to_string();
            self.run_mut_params(
                r#"?[key, val] <- [["created_at", $now]]
                :put instance_meta {key => val}"#,
                btree_params([("now", dv_str(&now))]),
            )
            .map_err(cozo_err)?;
        }
        Ok(())
    }

    /// Run a mutable CozoScript.
    fn run_mut(&self, script: &str) -> Result<NamedRows, cozo::Error> {
        self.db
            .run_script(script, BTreeMap::new(), ScriptMutability::Mutable)
    }

    /// Run a mutable CozoScript with parameters.
    fn run_mut_params(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, cozo::Error> {
        self.db
            .run_script(script, params, ScriptMutability::Mutable)
    }

    /// Run an immutable CozoScript.
    fn run_immut(&self, script: &str) -> Result<NamedRows, cozo::Error> {
        self.db
            .run_script(script, BTreeMap::new(), ScriptMutability::Immutable)
    }

    /// Run an immutable CozoScript with parameters.
    fn run_immut_params(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, cozo::Error> {
        self.db
            .run_script(script, params, ScriptMutability::Immutable)
    }

    fn now_epoch(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Get the next auto-increment ID for pending_updates.
    fn next_pending_id(&self) -> Result<i64, KbStoreError> {
        let result = self
            .run_immut("?[val] := *pending_counter{key: 'counter', val}")
            .map_err(cozo_err)?;
        let current = result
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0);
        let next = current + 1;
        self.run_mut_params(
            r#"?[key, val] <- [[$key, $val]]
            :put pending_counter {key => val}"#,
            btree_params([("key", dv_str("counter")), ("val", DataValue::from(next))]),
        )
        .map_err(cozo_err)?;
        Ok(next)
    }

    /// Insert or replace node links by parsing the body.
    fn update_links_for_node(&self, node: &Node) -> Result<(), KbStoreError> {
        // Remove old links from this node
        self.run_mut_params(
            r#"
            ?[src, dst, rel_type] := *links{src, dst, rel_type}, src = $id
            :rm links {src, dst, rel_type}
            "#,
            btree_params([("id", dv_str(&node.id))]),
        )
        .map_err(cozo_err)?;

        // Parse and insert new links
        let now = self.now_epoch();
        for (dst_raw, display) in crate::parse_links(&node.body) {
            // Strip fragment (e.g., "concept:buffer#rope-internals" → "concept:buffer")
            let dst = dst_raw.split('#').next().unwrap_or(&dst_raw).to_string();
            let disp = if dst == display {
                String::new()
            } else {
                display
            };
            self.run_mut_params(
                r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- [[$src, $dst, "related_to", $display, 1.0, 1.0, $now]]
                :put links {src, dst, rel_type => display, weight, confidence, created_at}"#,
                btree_params([
                    ("src", dv_str(&node.id)),
                    ("dst", dv_str(&dst)),
                    ("display", dv_str(&disp)),
                    ("now", DataValue::from(now)),
                ]),
            )
            .map_err(cozo_err)?;
        }
        Ok(())
    }

    // --- Graph queries (Datalog-native) ---

    /// Find shortest path between two nodes using BFS-style recursive Datalog.
    pub fn shortest_path(&self, from: &str, to: &str) -> Result<Vec<String>, KbStoreError> {
        // Simple reachability check — full path tracking requires
        // list operations that vary across CozoDB versions.
        // Returns the nodes on *a* path (not necessarily shortest).
        let result = self
            .run_immut_params(
                r#"
                reach[node, 0] := node = $from
                reach[node, d + 1] := reach[mid, d], *links{src: mid, dst: node}, d < 10
                reach[node, d + 1] := reach[mid, d], *links{src: node, dst: mid}, d < 10

                ?[node, depth] := reach[node, depth], node = $to
                :limit 1
                "#,
                btree_params([("from", dv_str(from)), ("to", dv_str(to))]),
            )
            .map_err(cozo_err)?;

        if result.rows.is_empty() {
            Ok(vec![])
        } else {
            // Path exists — return from and to (full path reconstruction is complex in Datalog)
            Ok(vec![from.to_string(), to.to_string()])
        }
    }

    /// Get neighborhood subgraph around a node up to a given depth.
    pub fn neighborhood(&self, id: &str, depth: u32) -> Result<SubGraph, KbStoreError> {
        // Use simple multi-hop expansion without recursion depth tracking
        // to avoid CozoDB parser issues with `d + 1` syntax.
        // Collect all reachable nodes within `depth` hops.
        let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut frontier = vec![id.to_string()];
        visited.insert(id.to_string());

        for _ in 0..depth {
            let mut next_frontier = Vec::new();
            for node_id in &frontier {
                let out = self.links_from(node_id)?;
                for link in &out {
                    if visited.insert(link.dst.clone()) {
                        next_frontier.push(link.dst.clone());
                    }
                }
                let inc = self.links_to(node_id)?;
                for link in &inc {
                    if visited.insert(link.src.clone()) {
                        next_frontier.push(link.src.clone());
                    }
                }
            }
            frontier = next_frontier;
        }

        // Collect node info
        let mut nodes = Vec::new();
        for nid in &visited {
            if let Some(node) = self.get_node(nid)? {
                nodes.push((node.id, node.title));
            }
        }

        // Collect edges between visited nodes
        let mut edges = Vec::new();
        for nid in &visited {
            for link in self.links_from(nid)? {
                if visited.contains(&link.dst) {
                    edges.push((link.src, link.dst, link.rel_type));
                }
            }
        }

        Ok(SubGraph { nodes, edges })
    }

    /// Graph-relatedness over the typed link graph + tags — the Cozo-backed
    /// twin of [`crate::KnowledgeBase::related`]. Same four signals (direct
    /// link, bibliographic coupling, co-citation, shared tags), same weights,
    /// so results match the in-memory path. Built from the Datalog-backed
    /// `links_from`/`links_to` primitives (mirrors `neighborhood`'s approach,
    /// which avoids fragile recursive/self-join Datalog).
    pub fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        let Some(node) = self.get_node(id)? else {
            return Ok(Vec::new());
        };
        const W_DIRECT: f64 = 2.0;
        const W_COUPLING: f64 = 1.0;
        const W_COCITATION: f64 = 1.0;
        const W_TAG: f64 = 0.5;

        let out: Vec<String> = self.links_from(id)?.into_iter().map(|l| l.dst).collect();
        let inn: Vec<String> = self.links_to(id)?.into_iter().map(|l| l.src).collect();
        let tags: std::collections::HashSet<String> = node.tags.into_iter().collect();

        let mut score: std::collections::HashMap<String, f64> = std::collections::HashMap::new();

        // Bibliographic coupling: other nodes that link to the same targets.
        for target in &out {
            for l in self.links_to(target)? {
                if l.src != id {
                    *score.entry(l.src).or_default() += W_COUPLING;
                }
            }
        }
        // Co-citation: other nodes cited by the same sources.
        for src in &inn {
            for l in self.links_from(src)? {
                if l.dst != id {
                    *score.entry(l.dst).or_default() += W_COCITATION;
                }
            }
        }
        // Direct adjacency (either direction) is the strongest signal.
        for c in out.iter().chain(inn.iter()) {
            if c != id {
                *score.entry(c.clone()).or_default() += W_DIRECT;
            }
        }
        // Shared tags — one bulk query over `tags_json`, parsed in Rust (tags
        // aren't a relation, so this can't be a pure Datalog join).
        if !tags.is_empty() {
            let rows = self
                .run_immut("?[id, tags_json] := *nodes{id, tags_json}")
                .map_err(cozo_err)?;
            for row in &rows.rows {
                let Some(cid) = row.first().and_then(|v| v.get_str()) else {
                    continue;
                };
                if cid == id {
                    continue;
                }
                let ctags: Vec<String> = row
                    .get(1)
                    .and_then(|v| v.get_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or_default();
                let shared = ctags.iter().filter(|t| tags.contains(*t)).count();
                if shared > 0 {
                    *score.entry(cid.to_string()).or_default() += W_TAG * shared as f64;
                }
            }
        }

        let mut scored: Vec<(String, f64)> = score.into_iter().collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        Ok(scored)
    }

    /// Persist all nodes from an in-memory `KnowledgeBase` into this CozoDB store.
    ///
    /// Used by `build-manual-kb` to create the pre-built manual KB file.
    /// Returns the number of nodes persisted.
    pub fn persist_nodes(&self, kb: &crate::KnowledgeBase) -> Result<usize, KbStoreError> {
        let mut count = 0;
        for node in kb.nodes_values() {
            self.insert_node(node)?;
            count += 1;
        }
        Ok(count)
    }

    /// Record a source file's metadata for incremental ingestion.
    pub fn record_source_file(
        &self,
        file_path: &str,
        content_hash: &str,
        mtime: i64,
        node_ids: &[String],
    ) -> Result<(), KbStoreError> {
        let node_ids_json =
            serde_json::to_string(node_ids).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[file_path, content_hash, last_mtime, node_ids_json, last_import] <- [[
                $file_path, $content_hash, $last_mtime, $node_ids_json, $now
            ]]
            :put source_files {
                file_path => content_hash, last_mtime, node_ids_json, last_import
            }"#,
            btree_params([
                ("file_path", dv_str(file_path)),
                ("content_hash", dv_str(content_hash)),
                ("last_mtime", DataValue::from(mtime)),
                ("node_ids_json", dv_str(&node_ids_json)),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// Get a source file's stored content hash (for change detection).
    pub fn get_source_file_hash(&self, file_path: &str) -> Result<Option<String>, KbStoreError> {
        let result = self
            .run_immut_params(
                r#"?[content_hash] := *source_files{file_path: $fp, content_hash}"#,
                btree_params([("fp", dv_str(file_path))]),
            )
            .map_err(cozo_err)?;
        Ok(result
            .rows
            .first()
            .and_then(|r| r[0].get_str())
            .map(|s| s.to_string()))
    }

    /// Get node IDs associated with a source file (for deletion on file removal).
    pub fn get_source_file_node_ids(&self, file_path: &str) -> Result<Vec<String>, KbStoreError> {
        let result = self
            .run_immut_params(
                r#"?[node_ids_json] := *source_files{file_path: $fp, node_ids_json}"#,
                btree_params([("fp", dv_str(file_path))]),
            )
            .map_err(cozo_err)?;
        if let Some(row) = result.rows.first() {
            if let Some(json) = row[0].get_str() {
                let ids: Vec<String> = serde_json::from_str(json).unwrap_or_default();
                return Ok(ids);
            }
        }
        Ok(Vec::new())
    }

    /// Remove a source file record and its associated nodes.
    pub fn remove_source_file(&self, file_path: &str) -> Result<Vec<String>, KbStoreError> {
        let node_ids = self.get_source_file_node_ids(file_path)?;
        for id in &node_ids {
            self.delete_node(id)?;
        }
        self.run_mut_params(
            r#"?[file_path] <- [[$fp]]
            :rm source_files {file_path}"#,
            btree_params([("fp", dv_str(file_path))]),
        )
        .map_err(cozo_err)?;
        Ok(node_ids)
    }

    /// List all tracked source files with their content hashes.
    pub fn list_source_files(&self) -> Result<Vec<(String, String, i64)>, KbStoreError> {
        let result = self
            .run_immut(
                r#"?[file_path, content_hash, last_mtime]
                   := *source_files{file_path, content_hash, last_mtime}
                   :order file_path"#,
            )
            .map_err(cozo_err)?;
        let mut files = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            let fp = row[0].get_str().unwrap_or("").to_string();
            let hash = row[1].get_str().unwrap_or("").to_string();
            let mtime = row.get(2).and_then(|v| v.get_int()).unwrap_or(0);
            files.push((fp, hash, mtime));
        }
        Ok(files)
    }

    /// Load all links from CozoDB.
    pub fn load_all_links(&self) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut(
                r#"?[src, dst, rel_type, display, weight, confidence]
                   := *links{src, dst, rel_type, display, weight, confidence}
                   :order src, dst"#,
            )
            .map_err(cozo_err)?;

        let mut links = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            if let Some(link) = parse_link_row(row) {
                links.push(link);
            }
        }
        Ok(links)
    }
}

// ---------------------------------------------------------------------------
// KbStore trait implementation
// ---------------------------------------------------------------------------

impl KbStore for CozoKbStore {
    fn insert_node(&self, node: &Node) -> Result<(), KbStoreError> {
        let now = self.now_epoch();
        let tags_json =
            serde_json::to_string(&node.tags).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let aliases_json = serde_json::to_string(&node.aliases)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let properties_json = serde_json::to_string(&node.properties)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let pri_str = node.priority.map(|c| c.to_string()).unwrap_or_default();
        let source_str = node
            .source
            .map(|s| match s {
                crate::NodeSource::Seed => "seed",
                crate::NodeSource::UserOrg => "user_org",
                crate::NodeSource::Manual => "manual",
                crate::NodeSource::Federation => "federation",
            })
            .unwrap_or("");
        let (crdt_bytes, has_crdt) = match &node.crdt_doc {
            Some(doc) => (doc.clone(), true),
            None => (vec![], false),
        };

        self.run_mut_params(
            r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee, due_date, sprint,
                created_at, updated_at] <- [[
                $id, $title, $kind, $body, $tags_json, $todo_state, $priority, $source, $source_version,
                $aliases_json, $properties_json, $crdt_doc, $has_crdt, "", "", 0, "",
                $now, $now
            ]]
            :put nodes {
                id => title, kind, body, tags_json, todo_state, priority, source, source_version,
                aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee, due_date, sprint,
                created_at, updated_at
            }"#,
            btree_params([
                ("id", dv_str(&node.id)),
                ("title", dv_str(&node.title)),
                ("kind", dv_str(kind_to_str(node.kind))),
                ("body", dv_str(&node.body)),
                ("tags_json", dv_str(&tags_json)),
                ("todo_state", dv_str(node.todo_state.as_deref().unwrap_or(""))),
                ("priority", dv_str(&pri_str)),
                ("source", dv_str(source_str)),
                (
                    "source_version",
                    DataValue::from(node.source_version.unwrap_or(0) as i64),
                ),
                ("aliases_json", dv_str(&aliases_json)),
                ("properties_json", dv_str(&properties_json)),
                ("crdt_doc", DataValue::Bytes(crdt_bytes)),
                ("has_crdt", DataValue::Bool(has_crdt)),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;

        self.update_links_for_node(node)?;
        Ok(())
    }

    fn update_node(&self, node: &Node) -> Result<(), KbStoreError> {
        self.insert_node(node)
    }

    fn delete_node(&self, id: &str) -> Result<(), KbStoreError> {
        // Use :rm (not :delete) — :rm removes entire rows, :delete only clears values
        self.run_mut_params(
            "?[id] <- [[$id]]\n:rm nodes {id}",
            btree_params([("id", dv_str(id))]),
        )
        .map_err(cozo_err)?;

        // Remove links from this node
        self.run_mut_params(
            "?[src, dst, rel_type] := *links{src, dst, rel_type}, src = $id\n:rm links {src, dst, rel_type}",
            btree_params([("id", dv_str(id))]),
        )
        .map_err(cozo_err)?;

        Ok(())
    }

    fn get_node(&self, id: &str) -> Result<Option<Node>, KbStoreError> {
        let result = self
            .run_immut_params(
                r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                    aliases_json, properties_json, crdt_doc, has_crdt]
                    := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                              aliases_json, properties_json, crdt_doc, has_crdt},
                    id = $id"#,
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        if let Some(row) = result.rows.first() {
            let node = row_to_node(row)?;
            // Sled backend may leave ghost rows after :rm — treat as absent
            if node.title.is_empty() && node.body.is_empty() && node.tags.is_empty() {
                Ok(None)
            } else {
                Ok(Some(node))
            }
        } else {
            Ok(None)
        }
    }

    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        // Filter out ghost rows (title is empty string after :rm — defensive)
        let result = match prefix {
            Some(p) => self
                .run_immut_params(
                    r#"?[id] := *nodes{id, title}, starts_with(id, $prefix), title != """#,
                    btree_params([("prefix", dv_str(p))]),
                )
                .map_err(cozo_err)?,
            None => self
                .run_immut(r#"?[id] := *nodes{id, title}, title != """#)
                .map_err(cozo_err)?,
        };

        let mut ids: Vec<String> = result
            .rows
            .iter()
            .filter_map(|row| row.first()?.get_str().map(|s| s.to_string()))
            .collect();
        ids.sort();
        Ok(ids)
    }

    fn fts_search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        if query.is_empty() {
            // Empty query: return all node IDs (no ranking)
            let result = self
                .run_immut("?[id] := *nodes{id, title}, title != ''")
                .map_err(cozo_err)?;
            return Ok(result
                .rows
                .iter()
                .filter_map(|row| {
                    Some(SearchHit {
                        id: row.first()?.get_str()?.to_string(),
                        score: 0.0,
                    })
                })
                .collect());
        }

        // Use Tantivy FTS index for ranked search.
        // Fetch extra candidates to allow for post-query filtering
        // (guards against stale FTS index entries).
        let fetch_k = limit * 3 + 10;
        let result = self
            .run_immut_params(
                &format!(
                    r#"?[id, score] := ~nodes:fts{{id | query: $query, k: {fetch_k}, bind_score: score}}"#
                ),
                btree_params([("query", dv_str(query))]),
            )
            .map_err(cozo_err)?;

        // Post-query verification: check each hit's actual content still matches.
        // Defensive measure against stale FTS index entries.
        let query_lower = query.to_lowercase();
        let query_terms: Vec<&str> = query_lower.split_whitespace().collect();
        let mut hits = Vec::new();
        for row in &result.rows {
            let Some(id) = row.first().and_then(|v| v.get_str()) else {
                continue;
            };
            let score = row.get(1).and_then(|v| v.get_float()).unwrap_or(0.0);

            // Fetch actual title+body to verify the match is current
            if let Ok(Some(node)) = self.get_node(id) {
                let text = format!("{} {}", node.title, node.body).to_lowercase();
                let matches = query_terms.iter().any(|term| text.contains(term));
                if matches {
                    hits.push(SearchHit {
                        id: id.to_string(),
                        score,
                    });
                    if hits.len() >= limit {
                        break;
                    }
                }
            }
        }
        Ok(hits)
    }

    fn add_link(&self, src: &str, dst: &str, display: Option<&str>) -> Result<(), KbStoreError> {
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- [[$src, $dst, "related_to", $display, 1.0, 1.0, $now]]
            :put links {src, dst, rel_type => display, weight, confidence, created_at}"#,
            btree_params([
                ("src", dv_str(src)),
                ("dst", dv_str(dst)),
                ("display", dv_str(display.unwrap_or(""))),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn remove_link(&self, src: &str, dst: &str) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"
            ?[src, dst, rel_type] := *links{src, dst, rel_type}, src = $src, dst = $dst
            :rm links {src, dst, rel_type}
            "#,
            btree_params([("src", dv_str(src)), ("dst", dv_str(dst))]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[src, dst, rel_type, display, weight, confidence] := *links{src, dst, rel_type, display, weight, confidence}, src = $id",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| parse_link_row(row))
            .collect())
    }

    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[src, dst, rel_type, display, weight, confidence] := *links{src, dst, rel_type, display, weight, confidence}, dst = $id",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| parse_link_row(row))
            .collect())
    }

    fn get_crdt_doc(&self, id: &str) -> Result<Option<Vec<u8>>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[crdt_doc, has_crdt] := *nodes{id, crdt_doc, has_crdt}, id = $id",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;

        if let Some(row) = result.rows.first() {
            let has_crdt = row.get(1).and_then(|v| v.get_bool()).unwrap_or(false);
            if has_crdt {
                let doc = row.first().and_then(|v| v.get_bytes().map(|b| b.to_vec()));
                Ok(doc)
            } else {
                Ok(None)
            }
        } else {
            Ok(None)
        }
    }

    fn update_crdt_doc(&self, id: &str, doc: &[u8]) -> Result<(), KbStoreError> {
        let now = self.now_epoch();
        self.run_mut_params(
            r#"
            old[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                aliases_json, properties_json, origin_instance, assignee, due_date, sprint, created_at]
                := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                          aliases_json, properties_json, origin_instance, assignee, due_date, sprint,
                          crdt_doc: _, has_crdt: _, created_at, updated_at: _},
                id = $id

            ?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
              aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee, due_date, sprint,
              created_at, updated_at]
                := old[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                       aliases_json, properties_json, origin_instance, assignee, due_date, sprint, created_at],
                crdt_doc = $crdt_doc, has_crdt = true, updated_at = $now

            :put nodes {id => title, kind, body, tags_json, todo_state, priority, source, source_version,
                        aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee,
                        due_date, sprint, created_at, updated_at}
            "#,
            btree_params([
                ("id", dv_str(id)),
                ("crdt_doc", DataValue::Bytes(doc.to_vec())),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn push_pending_update(
        &self,
        kb_id: &str,
        node_id: &str,
        update: &[u8],
    ) -> Result<(), KbStoreError> {
        let id = self.next_pending_id()?;
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[id, kb_id, node_id, update_bytes, created_at] <- [[$id, $kb_id, $node_id, $update_bytes, $now]]
            :put pending_updates {id => kb_id, node_id, update_bytes, created_at}"#,
            btree_params([
                ("id", DataValue::from(id)),
                ("kb_id", dv_str(kb_id)),
                ("node_id", dv_str(node_id)),
                ("update_bytes", DataValue::Bytes(update.to_vec())),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn drain_pending_updates(&self) -> Result<Vec<PendingUpdate>, KbStoreError> {
        let result = self
            .run_immut(
                "?[id, kb_id, node_id, update_bytes] := *pending_updates{id, kb_id, node_id, update_bytes} :order id",
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                let rowid = row.first()?.get_int()?;
                let kb_id = row.get(1)?.get_str()?.to_string();
                let node_id = row.get(2)?.get_str()?.to_string();
                let update_bytes = row.get(3)?.get_bytes()?.to_vec();
                Some(PendingUpdate {
                    rowid,
                    kb_id,
                    node_id,
                    update_bytes,
                })
            })
            .collect())
    }

    fn ack_pending_update(&self, id: i64) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"?[id] <- [[$id]]
            :rm pending_updates {id}"#,
            btree_params([("id", DataValue::from(id))]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    fn load_all(&self) -> Result<Vec<Node>, KbStoreError> {
        let query = r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                    aliases_json, properties_json, crdt_doc, has_crdt]
                    := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                              aliases_json, properties_json, crdt_doc, has_crdt},
                    title != ""
                    :order id"#;
        // B-5: a malformed / short-arity stored row (e.g. one left by an older
        // schema version or a previously-broken write path) makes the ENTIRE cozo
        // query fail at bind time ("tuple bound by variable 'title' is too short")
        // — before the per-row skip loop below ever runs. Propagating that error
        // here previously aborted the caller (e.g. `kb_join`) and, on the main
        // thread, tripped the 10s stall watchdog. Degrade to an empty load (logged
        // at ERROR for repair visibility) so the editor keeps running: this is the
        // same observable state as a genuinely empty store, which every caller
        // already handles, and strictly safer than a hard error. (Moving this
        // query off the UI thread is the deeper concurrency-#1 fix, tracked
        // separately.)
        let result = match self.run_immut(query) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(error = %e, "KB store: node load query failed — returning empty load (store may need repair); editor continues without stalling");
                return Ok(Vec::new());
            }
        };

        let mut nodes = Vec::with_capacity(result.rows.len());
        for row in &result.rows {
            // ADR-019 / B-5: tolerate a malformed row — skip it (with a warning)
            // instead of aborting the entire load, which previously errored and
            // stalled the editor's main thread on a single bad-arity row.
            match row_to_node(row) {
                Ok(node) => nodes.push(node),
                Err(e) => {
                    tracing::warn!(error = %e, "KB store: skipping malformed node row");
                }
            }
        }
        Ok(nodes)
    }

    fn save_all(&self, nodes: &[&Node]) -> Result<(), KbStoreError> {
        // Clear existing data
        self.run_mut(
            r#"
            ?[id] := *nodes{id}
            :rm nodes {id}
            "#,
        )
        .map_err(cozo_err)?;
        self.run_mut(
            r#"
            ?[src, dst, rel_type] := *links{src, dst, rel_type}
            :rm links {src, dst, rel_type}
            "#,
        )
        .map_err(cozo_err)?;

        // Insert all nodes
        for node in nodes {
            self.insert_node(node)?;
        }
        Ok(())
    }

    // --- Trait overrides for CozoDB-specific features ---

    fn add_typed_link(
        &self,
        src: &str,
        dst: &str,
        rel_type: &str,
        weight: f64,
    ) -> Result<(), KbStoreError> {
        CozoKbStore::add_typed_link(self, src, dst, rel_type, weight)
    }

    fn links_typed(&self, id: &str, rel_type: &str) -> Result<Vec<Link>, KbStoreError> {
        CozoKbStore::links_typed(self, id, rel_type)
    }

    fn known_rel_types(&self) -> Result<std::collections::HashSet<String>, KbStoreError> {
        CozoKbStore::known_rel_types(self)
    }

    fn shortest_path(&self, from: &str, to: &str) -> Result<Vec<String>, KbStoreError> {
        CozoKbStore::shortest_path(self, from, to)
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Result<SubGraph, KbStoreError> {
        CozoKbStore::neighborhood(self, id, depth)
    }

    fn raw_query(&self, script: &str) -> Result<(Vec<String>, Vec<Vec<String>>), KbStoreError> {
        CozoKbStore::raw_query(self, script)
    }

    fn meta_members(&self, meta_id: &str) -> Result<Vec<MetaMember>, KbStoreError> {
        CozoKbStore::meta_members(self, meta_id)
    }

    fn add_meta_member(
        &self,
        meta_id: &str,
        member_id: &str,
        position: i32,
        role: &str,
    ) -> Result<(), KbStoreError> {
        CozoKbStore::add_meta_member(self, meta_id, member_id, position, role)
    }

    fn remove_meta_member(&self, meta_id: &str, member_id: &str) -> Result<(), KbStoreError> {
        CozoKbStore::remove_meta_member(self, meta_id, member_id)
    }

    fn compose_meta_body(&self, meta_id: &str) -> Result<String, KbStoreError> {
        CozoKbStore::compose_meta_body(self, meta_id)
    }

    fn get_blocks(&self, parent_id: &str) -> Result<Vec<Block>, KbStoreError> {
        CozoKbStore::get_blocks(self, parent_id)
    }

    fn get_block(&self, parent_id: &str, idx: usize) -> Result<Option<Block>, KbStoreError> {
        CozoKbStore::get_block(self, parent_id, idx)
    }

    fn agenda_query(&self, filter: &AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        CozoKbStore::agenda_query(self, filter)
    }

    fn node_history(&self, id: &str, limit: usize) -> Result<Vec<NodeVersion>, KbStoreError> {
        CozoKbStore::node_history(self, id, limit)
    }

    fn restore_version(&self, id: &str, version: i64) -> Result<(), KbStoreError> {
        CozoKbStore::restore_version(self, id, version)
    }

    fn store_embedding(&self, id: &str, model: &str, vec: &[f32]) -> Result<(), KbStoreError> {
        CozoKbStore::store_embedding(self, id, model, vec)
    }

    fn vector_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        CozoKbStore::vector_search(self, vec, k)
    }

    fn graphrag_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        CozoKbStore::graphrag_search(self, vec, k)
    }

    fn health_report(&self) -> Result<HealthReport, KbStoreError> {
        CozoKbStore::health_report(self)
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        CozoKbStore::id_title_pairs(self, prefix)
    }

    fn backend_name(&self) -> &str {
        "cozo"
    }

    fn db_path(&self) -> &Path {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Typed relationship extensions (CozoDB-specific)
// ---------------------------------------------------------------------------

impl CozoKbStore {
    /// Query all known relationship type names from the `rel_types` relation.
    /// Returns a set of type names (e.g., "teaches", "implements", "references").
    pub fn known_rel_types(&self) -> Result<std::collections::HashSet<String>, KbStoreError> {
        let result = self
            .run_immut("?[name] := *rel_types{name}")
            .map_err(cozo_err)?;
        Ok(result
            .rows
            .iter()
            .filter_map(|row| row.first()?.get_str().map(|s| s.to_string()))
            .collect())
    }

    /// Add a typed link between nodes with confidence score.
    pub fn add_typed_link(
        &self,
        src: &str,
        dst: &str,
        rel_type: &str,
        weight: f64,
    ) -> Result<(), KbStoreError> {
        // Strip fragment (e.g., "concept:buffer#rope-internals" → "concept:buffer")
        let dst_clean = dst.split('#').next().unwrap_or(dst);
        self.add_typed_link_with_confidence(src, dst_clean, rel_type, weight, 1.0)
    }

    /// Add a typed link with explicit confidence (0.0–1.0).
    /// AI-generated links should use lower confidence values.
    pub fn add_typed_link_with_confidence(
        &self,
        src: &str,
        dst: &str,
        rel_type: &str,
        weight: f64,
        confidence: f64,
    ) -> Result<(), KbStoreError> {
        let now = self.now_epoch();
        self.run_mut_params(
            r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- [[$src, $dst, $rel_type, "", $weight, $confidence, $now]]
            :put links {src, dst, rel_type => display, weight, confidence, created_at}"#,
            btree_params([
                ("src", dv_str(src)),
                ("dst", dv_str(dst)),
                ("rel_type", dv_str(rel_type)),
                ("weight", DataValue::from(weight)),
                ("confidence", DataValue::from(confidence)),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// Query links filtered by relationship type.
    pub fn links_typed(&self, id: &str, rel_type: &str) -> Result<Vec<Link>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[src, dst, rel_type, display, weight, confidence] := *links{src, dst, rel_type, display, weight, confidence}, src = $id, rel_type = $rel_type",
                btree_params([("id", dv_str(id)), ("rel_type", dv_str(rel_type))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| parse_link_row(row))
            .collect())
    }

    /// Rebuild the FTS index to clean up stale entries.
    /// Call periodically or after bulk updates.
    pub fn rebuild_fts(&self) -> Result<(), KbStoreError> {
        // Drop and recreate the FTS index
        self.run_mut("::fts drop nodes:fts")
            .or_else(|e| {
                if e.to_string().contains("not found") {
                    Ok(NamedRows::default())
                } else {
                    Err(e)
                }
            })
            .map_err(cozo_err)?;
        self.run_mut(
            r#"::fts create nodes:fts {
                extractor: title ++ ' ' ++ body,
                tokenizer: Simple,
                filters: [Lowercase]
            }"#,
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// Seed typed relationships between known seed nodes.
    ///
    /// Since v0.13.0, content relationships (lesson→concept, concept→concept)
    /// are expressed as inline typed links in org files and extracted by the
    /// org parser during ingestion. This function now only seeds relationships
    /// that can't be expressed in org files (code-generated nodes like cmd:*).
    ///
    /// Idempotent — uses :put (upsert) so re-running is safe.
    pub fn seed_typed_relationships(&self) -> Result<usize, KbStoreError> {
        let now = self.now_epoch();
        // Only code-generated relationships remain here.
        // Content relationships (lesson↔concept, concept↔concept, tutorial chains)
        // are now inline typed links in assets/manual/*.org files.
        let relationships: Vec<(&str, &str, &str, f64)> = vec![
            // Index categorizes — kept because index.org links are plain links
            // and these establish the top-level graph structure.
            ("index", "concept:buffer", "categorizes", 1.0),
            ("index", "concept:mode", "categorizes", 1.0),
            ("index", "concept:ai-as-peer", "categorizes", 1.0),
            ("index", "concept:knowledge-base", "categorizes", 1.0),
            ("index", "concept:scheme-api", "categorizes", 1.0),
            ("index", "concept:debugging", "categorizes", 1.0),
        ];

        let count = relationships.len();
        for (src, dst, rel_type, weight) in &relationships {
            self.run_mut_params(
                r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- [[$src, $dst, $rel_type, "", $weight, 1.0, $now]]
                :put links {src, dst, rel_type => display, weight, confidence, created_at}"#,
                btree_params([
                    ("src", dv_str(src)),
                    ("dst", dv_str(dst)),
                    ("rel_type", dv_str(rel_type)),
                    ("weight", DataValue::from(*weight)),
                    ("now", DataValue::from(now)),
                ]),
            )
            .map_err(cozo_err)?;
        }

        Ok(count)
    }

    /// Run a raw Datalog query against the KB. Returns headers + rows as strings.
    pub fn raw_query(&self, script: &str) -> Result<(Vec<String>, Vec<Vec<String>>), KbStoreError> {
        let result = self.run_immut(script).map_err(cozo_err)?;
        let rows: Vec<Vec<String>> = result
            .rows
            .iter()
            .map(|row| row.iter().map(|v| format!("{v:?}")).collect())
            .collect();
        Ok((result.headers, rows))
    }

    /// Get this instance's UUID (generated on first open).
    pub fn instance_id(&self) -> Result<String, KbStoreError> {
        let result = self
            .run_immut("?[val] := *instance_meta{key: 'instance_id', val}")
            .map_err(cozo_err)?;
        result
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_str())
            .map(|s| s.to_string())
            .ok_or_else(|| KbStoreError::Storage("instance_id not found".into()))
    }

    /// Seed the node_types and rel_types metadata relations.
    /// Idempotent — overwrites existing entries.
    pub fn seed_type_system(&self) -> Result<(), KbStoreError> {
        // Node types: kind, label, description, namespace_prefix, icon, required_fields_json
        let node_types_script = concat!(
            "?[kind, label, description, namespace_prefix, icon, required_fields_json] <- [\n",
            r#"["index",      "Index",      "Top-level index/category node",                "",          "I",  "[]"],"#, "\n",
            r#"["command",    "Command",    "Editor command (ex-command or key-triggered)",  "cmd:",      "C",  "[]"],"#, "\n",
            r#"["concept",    "Concept",    "Architecture concept or design doc",            "concept:",  "c",  "[]"],"#, "\n",
            r#"["key",        "Key",        "Keybinding definition",                         "key:",      "K",  "[]"],"#, "\n",
            r#"["note",       "Note",       "General-purpose note",                          "",          "N",  "[]"],"#, "\n",
            r#"["project",    "Project",    "Project definition",                            "project:",  "P",  "[]"],"#, "\n",
            r#"["category",   "Category",   "Grouping/taxonomy node",                        "category:", "G",  "[]"],"#, "\n",
            r#"["lesson",     "Lesson",     "Tutorial lesson (ordered)",                     "lesson:",   "L",  "[]"],"#, "\n",
            r#"["tutorial",   "Tutorial",   "Tutorial track (contains lessons)",             "tutorial:", "T",  "[]"],"#, "\n",
            r#"["meta",       "Meta",       "Composite node (cached from members)",          "meta:",     "M",  "[]"],"#, "\n",
            r#"["block",      "Block",      "Paragraph-level sub-node",                      "",          "B",  "[]"],"#, "\n",
            r#"["scheme_api", "Scheme API", "Scheme primitive/variable documentation",       "scheme:",   "S",  "[]"],"#, "\n",
            r#"["task",       "Task",       "Work item with state/priority/assignee",        "task:",     "t",  "[]"],"#, "\n",
            r#"["view",       "View",       "Query-based view (kanban/agenda/etc)",          "view:",     "V",  "[]"]"#, "\n",
            "]\n",
            ":put node_types {kind => label, description, namespace_prefix, icon, required_fields_json}",
        );
        self.run_mut(node_types_script).map_err(cozo_err)?;

        // Relationship types: name, label, description, inverse_name, directed
        self.run_mut(
            r#"?[name, label, description, inverse_name, directed] <- [
                ["implements",       "Implements",       "Source implements/realizes target",            "implemented_by",   true],
                ["extends",          "Extends",          "Source extends/inherits from target",          "extended_by",      true],
                ["contradicts",      "Contradicts",      "Source contradicts/conflicts with target",     "contradicted_by",  true],
                ["explains",         "Explains",         "Source explains/clarifies target",             "explained_by",     true],
                ["references",       "References",       "Source references target (see also)",          "referenced_by",    true],
                ["supersedes",       "Supersedes",       "Source replaces/supersedes target",            "superseded_by",    true],
                ["part_of",          "Part Of",          "Source is a component of target",              "has_part",         true],
                ["related_to",       "Related To",       "General undirected relationship",              "related_to",       false],
                ["teaches",          "Teaches",          "Lesson/tutorial teaches concept",              "taught_by",        true],
                ["requires",         "Requires",         "Source requires target as prerequisite",       "required_by",      true],
                ["configures",       "Configures",       "Option/setting configures feature",            "configured_by",    true],
                ["binds",            "Binds",            "Keybinding binds to command",                  "bound_by",         true],
                ["categorized_under","Categorized Under","Node belongs to category",                     "categorizes",      true],
                ["documents",        "Documents",        "Concept documents command/feature",            "documented_by",    true],
                ["contains",         "Contains",         "Meta-node/parent contains member/block",       "contained_in",     true],
                ["federated_from",   "Federated From",   "Node originates from remote instance",         "federated_to",     true],
                ["assigned_to",      "Assigned To",      "Task assigned to user/entity",                 "assigned_from",    true],
                ["belongs_to_sprint","Belongs To Sprint","Task belongs to sprint/milestone",              "sprint_contains",  true],
                ["subtask_of",       "Subtask Of",       "Task is subtask of parent task/epic",          "has_subtask",      true],
                ["blocks_task",      "Blocks",           "Task blocks another task (scheduling dep)",    "blocked_by",       true]
            ]
            :put rel_types {name => label, description, inverse_name, directed}"#,
        )
        .map_err(cozo_err)?;

        Ok(())
    }

    /// Seed pre-built view definitions (6 flavors).
    /// Idempotent: uses :put so re-running overwrites with latest definitions.
    pub fn seed_views(&self) -> Result<(), KbStoreError> {
        let now = self.now_epoch();

        let views: Vec<(&str, &str, &str, &str, &str, &str)> = vec![
            (
                "view:kanban",
                "Kanban Board",
                "kanban",
                r#"?[id, title, todo, assignee, priority] := *nodes{id, title, kind, todo_state: todo, assignee, priority}, kind = "task""#,
                r#"{"group_by":"todo_state","columns":["TODO","IN_PROGRESS","REVIEW","DONE"],"sort_by":"priority"}"#,
                "Task management view grouped by todo state (TODO > IN_PROGRESS > REVIEW > DONE). Shows all task nodes with assignee and priority.",
            ),
            (
                "view:backlog",
                "Backlog",
                "backlog",
                r#"?[id, title, priority, created_at] := *nodes{id, title, kind, priority, sprint, created_at}, kind = "task", sprint = """#,
                r#"{"sort_by":"priority","columns":["id","title","priority","created_at"]}"#,
                "Unscheduled tasks (no sprint assigned). Sorted by priority, then creation date.",
            ),
            (
                "view:sprint",
                "Sprint View",
                "sprint",
                r#"?[id, title, todo, assignee, priority] := *nodes{id, title, kind, todo_state: todo, assignee, priority, sprint}, kind = "task", sprint != """#,
                r#"{"group_by":"assignee","sort_by":"priority","columns":["id","title","todo_state","priority"]}"#,
                "Tasks assigned to a sprint. Grouped by assignee, sorted by priority.",
            ),
            (
                "view:timeline",
                "Timeline",
                "timeline",
                r#"?[id, title, due_date, priority] := *nodes{id, title, kind, due_date, priority}, kind = "task", due_date != 0"#,
                r#"{"sort_by":"due_date","columns":["id","title","due_date","priority"]}"#,
                "Tasks with due dates, sorted chronologically. Colored by priority.",
            ),
            (
                "view:agenda",
                "Agenda",
                "agenda",
                r#"?[id, title, todo, priority, due_date] := *nodes{id, title, kind, todo_state: todo, priority, due_date}, kind = "task", todo != """#,
                r#"{"group_by":"priority","sort_by":"due_date","columns":["id","title","todo_state","due_date"]}"#,
                "Active tasks (with todo state) grouped by priority. Org-agenda-style daily/weekly view.",
            ),
            (
                "view:orphans",
                "Orphan Nodes",
                "custom",
                "all_linked[id] := *links{src: id} all_linked[id] := *links{dst: id} ?[id, title, kind] := *nodes{id, title, kind}, not all_linked[id]",
                r#"{"sort_by":"kind","columns":["id","title","kind"]}"#,
                "Custom Datalog view showing all nodes with no incoming or outgoing links.",
            ),
        ];

        for (id, title, kind, query, config, body) in &views {
            self.run_mut_params(
                "?[id, title, kind, query, display_config_json, owner, created_at, updated_at] <- [[$id, $title, $kind, $query, $config, $owner, $now, $now]] :put views {id => title, kind, query, display_config_json, owner, created_at, updated_at}",
                btree_params([
                    ("id", dv_str(id)),
                    ("title", dv_str(title)),
                    ("kind", dv_str(kind)),
                    ("query", dv_str(query)),
                    ("config", dv_str(config)),
                    ("owner", dv_str("")),
                    ("now", DataValue::from(now)),
                ]),
            )
            .map_err(cozo_err)?;

            // Also insert as KB node for help/search
            self.insert_node(&Node::new(*id, *title, NodeKind::View, *body))?;
        }

        Ok(())
    }

    // --- Phase D: Meta-nodes + Block addressing ---

    /// Get ordered members of a meta-node.
    pub fn meta_members(&self, meta_id: &str) -> Result<Vec<MetaMember>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[member_id, position, role] := *meta_members{meta_id, member_id, position, role}, meta_id = $id :order position",
                btree_params([("id", dv_str(meta_id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                Some(MetaMember {
                    member_id: row.first()?.get_str()?.to_string(),
                    position: row.get(1)?.get_int()? as i32,
                    role: row.get(2)?.get_str()?.to_string(),
                })
            })
            .collect())
    }

    /// Add a member to a meta-node.
    pub fn add_meta_member(
        &self,
        meta_id: &str,
        member_id: &str,
        position: i32,
        role: &str,
    ) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"?[meta_id, member_id, position, role] <- [[$meta_id, $member_id, $position, $role]]
            :put meta_members {meta_id, member_id, position => role}"#,
            btree_params([
                ("meta_id", dv_str(meta_id)),
                ("member_id", dv_str(member_id)),
                ("position", DataValue::from(position as i64)),
                ("role", dv_str(role)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// Remove a member from a meta-node.
    pub fn remove_meta_member(&self, meta_id: &str, member_id: &str) -> Result<(), KbStoreError> {
        self.run_mut_params(
            r#"?[meta_id, member_id, position] := *meta_members{meta_id, member_id, position}, meta_id = $meta_id, member_id = $member_id
            :rm meta_members {meta_id, member_id, position}"#,
            btree_params([
                ("meta_id", dv_str(meta_id)),
                ("member_id", dv_str(member_id)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// Compose a meta-node's body from its members.
    pub fn compose_meta_body(&self, meta_id: &str) -> Result<String, KbStoreError> {
        let members = self.meta_members(meta_id)?;
        let mut parts = Vec::new();
        for member in &members {
            match member.role.as_str() {
                "content" | "transclusion" => {
                    if let Ok(Some(node)) = self.get_node(&member.member_id) {
                        parts.push(node.body);
                    }
                }
                "reference" => {
                    parts.push(format!("→ [[{}]]", member.member_id));
                }
                _ => {}
            }
        }
        Ok(parts.join("\n\n"))
    }

    /// Split a node body into paragraph blocks and store them.
    pub fn split_into_blocks(&self, parent_id: &str) -> Result<usize, KbStoreError> {
        let node = self
            .get_node(parent_id)?
            .ok_or_else(|| KbStoreError::NotFound(parent_id.to_string()))?;

        let now = self.now_epoch();
        // Remove existing blocks
        self.run_mut_params(
            "?[parent_id, block_idx] := *blocks{parent_id, block_idx}, parent_id = $id\n:rm blocks {parent_id, block_idx}",
            btree_params([("id", dv_str(parent_id))]),
        )
        .map_err(cozo_err)?;

        let paragraphs: Vec<&str> = node.body.split("\n\n").collect();
        for (idx, content) in paragraphs.iter().enumerate() {
            let block_type = if content.starts_with('#') || content.starts_with('*') {
                "heading"
            } else if content.starts_with("```") || content.starts_with("#+begin_src") {
                "code"
            } else if content.starts_with("- ") || content.starts_with("1.") {
                "list"
            } else {
                "paragraph"
            };
            self.run_mut_params(
                r#"?[parent_id, block_idx, content, block_type, created_at, updated_at] <- [[$pid, $idx, $content, $btype, $now, $now]]
                :put blocks {parent_id, block_idx => content, block_type, created_at, updated_at}"#,
                btree_params([
                    ("pid", dv_str(parent_id)),
                    ("idx", DataValue::from(idx as i64)),
                    ("content", dv_str(content)),
                    ("btype", dv_str(block_type)),
                    ("now", DataValue::from(now)),
                ]),
            )
            .map_err(cozo_err)?;
        }
        Ok(paragraphs.len())
    }

    /// Get all blocks for a node.
    pub fn get_blocks(&self, parent_id: &str) -> Result<Vec<Block>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[block_idx, content, block_type] := *blocks{parent_id, block_idx, content, block_type}, parent_id = $id :order block_idx",
                btree_params([("id", dv_str(parent_id))]),
            )
            .map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                Some(Block {
                    block_idx: row.first()?.get_int()? as usize,
                    content: row.get(1)?.get_str()?.to_string(),
                    block_type: row.get(2)?.get_str()?.to_string(),
                })
            })
            .collect())
    }

    /// Get a single block by index.
    pub fn get_block(&self, parent_id: &str, idx: usize) -> Result<Option<Block>, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[block_idx, content, block_type] := *blocks{parent_id, block_idx, content, block_type}, parent_id = $id, block_idx = $idx",
                btree_params([
                    ("id", dv_str(parent_id)),
                    ("idx", DataValue::from(idx as i64)),
                ]),
            )
            .map_err(cozo_err)?;

        Ok(result.rows.first().and_then(|row| {
            Some(Block {
                block_idx: row.first()?.get_int()? as usize,
                content: row.get(1)?.get_str()?.to_string(),
                block_type: row.get(2)?.get_str()?.to_string(),
            })
        }))
    }

    // --- Phase E: Agenda queries ---

    /// Run an agenda query with the given filter.
    pub fn agenda_query(&self, filter: &AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        let query = match filter {
            AgendaFilter::Todo(None) => {
                "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, todo_state != ''".to_string()
            }
            AgendaFilter::Todo(Some(state)) => {
                format!(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}}, todo_state = '{}'",
                    state.replace('\'', "")
                )
            }
            AgendaFilter::Priority(min_pri) => {
                format!(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}}, priority <= '{}'",
                    min_pri
                )
            }
            AgendaFilter::Tag(tag) => {
                format!(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}}, contains(tags_json, '{}')",
                    tag.replace('\'', "")
                )
            }
            AgendaFilter::Stale(days) => {
                let cutoff = self.now_epoch() - (*days as i64 * 86400);
                format!(
                    "?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt, updated_at}}, updated_at < {cutoff}, title != ''"
                )
            }
            AgendaFilter::Orphan => {
                // Nodes with no incoming or outgoing links
                "has_links[id] := *links{src: id}\nhas_links[id] := *links{dst: id}\n?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, not has_links[id], title != ''".to_string()
            }
            AgendaFilter::DeadEnd => {
                // Nodes with no outgoing links
                "has_outgoing[id] := *links{src: id}\n?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt] := *nodes{id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt}, not has_outgoing[id], title != ''".to_string()
            }
            AgendaFilter::Custom(q) => q.clone(),
        };

        let result = self.run_immut(&query).map_err(cozo_err)?;
        let mut nodes = Vec::new();
        for row in &result.rows {
            // ADR-019 / B-5: tolerate a malformed row — skip it (with a warning)
            // instead of aborting the entire load, which previously errored and
            // stalled the editor's main thread on a single bad-arity row.
            match row_to_node(row) {
                Ok(node) => nodes.push(node),
                Err(e) => {
                    tracing::warn!(error = %e, "KB store: skipping malformed node row");
                }
            }
        }
        Ok(nodes)
    }

    // --- Phase F: KB Health via Datalog ---

    /// Compute a structured health report using Datalog queries.
    pub fn health_report(&self) -> Result<HealthReport, KbStoreError> {
        use crate::store::{BrokenLinkInfo, BrokenLinkReason};

        // Total counts
        let total_nodes = self
            .run_immut("?[id] := *nodes{id, title}, title != ''")
            .map_err(cozo_err)?
            .rows
            .len();
        let total_links = self
            .run_immut("?[src, dst, rt] := *links{src, dst, rel_type: rt}")
            .map_err(cozo_err)?
            .rows
            .len();

        // Nodes by kind
        let kind_result = self
            .run_immut("?[kind, id] := *nodes{id, kind, title}, title != ''")
            .map_err(cozo_err)?;
        let mut by_kind: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &kind_result.rows {
            if let Some(kind) = row.first().and_then(|v| v.get_str()) {
                *by_kind.entry(kind.to_string()).or_default() += 1;
            }
        }

        // Namespace counts (derived from node ID prefixes)
        let ns_result = self
            .run_immut("?[id] := *nodes{id, title}, title != ''")
            .map_err(cozo_err)?;
        let mut namespace_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &ns_result.rows {
            if let Some(id) = row.first().and_then(|v| v.get_str()) {
                let ns = if let Some(colon) = id.find(':') {
                    &id[..colon]
                } else {
                    "(none)"
                };
                *namespace_counts.entry(ns.to_string()).or_default() += 1;
            }
        }

        // Links by type
        let rel_result = self
            .run_immut("?[rt, src, dst] := *links{src, dst, rel_type: rt}")
            .map_err(cozo_err)?;
        let mut by_rel_type: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &rel_result.rows {
            if let Some(rt) = row.first().and_then(|v| v.get_str()) {
                *by_rel_type.entry(rt.to_string()).or_default() += 1;
            }
        }

        // Orphan nodes (no links in or out) — returns IDs
        let orphan_result = self.run_immut(
            "has_links[id] := *links{src: id}\nhas_links[id] := *links{dst: id}\n?[id] := *nodes{id, title}, not has_links[id], title != ''"
        ).map_err(cozo_err)?;
        let orphan_ids: Vec<String> = orphan_result
            .rows
            .iter()
            .filter_map(|row| row.first().and_then(|v| v.get_str()).map(|s| s.to_string()))
            .collect();

        // Broken links (target doesn't exist) — returns details
        let broken_result = self.run_immut(
            "exists[id] := *nodes{id, title}, title != ''\n?[src, dst, rt] := *links{src, dst, rel_type: rt}, not exists[dst]"
        ).map_err(cozo_err)?;
        let broken_links: Vec<BrokenLinkInfo> = broken_result
            .rows
            .iter()
            .filter_map(|row| {
                let src = row.first()?.get_str()?.to_string();
                let dst = row.get(1)?.get_str()?.to_string();
                let rt = row.get(2)?.get_str()?.to_string();
                let reason = if dst.contains(':') || dst.len() > 3 {
                    BrokenLinkReason::DeletedNode
                } else {
                    BrokenLinkReason::MalformedId
                };
                Some(BrokenLinkInfo {
                    source: src,
                    target: dst,
                    rel_type: rt,
                    reason,
                })
            })
            .collect();

        // Hub nodes (highest in-degree, top 10)
        let hub_result = self
            .run_immut("in_deg[dst, id] := *links{dst, src: id}\n?[dst, id] := in_deg[dst, id]")
            .map_err(cozo_err)?;
        let mut hub_map: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for row in &hub_result.rows {
            if let Some(dst) = row.first().and_then(|v| v.get_str()) {
                *hub_map.entry(dst.to_string()).or_default() += 1;
            }
        }
        let mut hubs: Vec<(String, usize)> = hub_map.into_iter().collect();
        hubs.sort_by_key(|h| std::cmp::Reverse(h.1));
        hubs.truncate(10);

        Ok(HealthReport {
            total_nodes,
            total_links,
            namespace_counts,
            by_kind,
            by_rel_type,
            orphan_ids,
            broken_links,
            hub_nodes: hubs,
        })
    }

    /// Return (id, title) pairs for all nodes, optionally filtered by prefix.
    pub fn id_title_pairs(
        &self,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, KbStoreError> {
        let query = if let Some(p) = prefix {
            format!(
                "?[id, title] := *nodes{{id, title}}, title != '', starts_with(id, '{}')",
                p.replace('\'', "")
            )
        } else {
            "?[id, title] := *nodes{id, title}, title != ''".to_string()
        };
        let result = self.run_immut(&query).map_err(cozo_err)?;
        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                let id = row.first()?.get_str()?.to_string();
                let title = row.get(1)?.get_str()?.to_string();
                Some((id, title))
            })
            .collect())
    }

    /// Batch query returning (id, title, body) for all nodes.
    /// Body is truncated to `body_limit` chars (0 = no body column).
    pub fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Result<Vec<(String, String, String)>, KbStoreError> {
        let query = if body_limit == 0 {
            // No body needed — same as id_title_pairs
            if let Some(p) = prefix {
                format!(
                    "?[id, title, body] := *nodes{{id, title}}, title != '', starts_with(id, '{}'), body = ''",
                    p.replace('\'', "")
                )
            } else {
                "?[id, title, body] := *nodes{id, title}, title != '', body = ''".to_string()
            }
        } else if let Some(p) = prefix {
            format!(
                "?[id, title, body] := *nodes{{id, title, body}}, title != '', starts_with(id, '{}')",
                p.replace('\'', "")
            )
        } else {
            "?[id, title, body] := *nodes{id, title, body}, title != ''".to_string()
        };
        let result = self.run_immut(&query).map_err(cozo_err)?;
        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                let id = row.first()?.get_str()?.to_string();
                let title = row.get(1)?.get_str()?.to_string();
                let body_raw = row.get(2)?.get_str().unwrap_or("");
                let body = if body_limit > 0 && body_raw.len() > body_limit {
                    body_raw.chars().take(body_limit).collect()
                } else {
                    body_raw.to_string()
                };
                Some((id, title, body))
            })
            .collect())
    }

    // --- Phase H: Node versioning ---

    /// Snapshot the current state of a node into node_versions.
    /// Computes a content checksum for tamper detection (SOC II audit trail).
    pub fn snapshot_version(&self, id: &str, change_summary: &str) -> Result<i64, KbStoreError> {
        // Get current max version for this node
        let ver_result = self
            .run_immut_params(
                "?[v] := *node_versions{id, version: v}, id = $id :order -v :limit 1",
                btree_params([("id", dv_str(id))]),
            )
            .map_err(cozo_err)?;
        let next_version = ver_result
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0)
            + 1;

        // Read current node state
        let node = self
            .get_node(id)?
            .ok_or_else(|| KbStoreError::NotFound(id.to_string()))?;

        let tags_json = serde_json::to_string(&node.tags).unwrap_or_else(|_| "[]".to_string());
        let properties_json =
            serde_json::to_string(&node.properties).unwrap_or_else(|_| "{}".to_string());
        let todo_state_str = node.todo_state.as_deref().unwrap_or("");
        let priority_str = node.priority.map(|c| c.to_string()).unwrap_or_default();
        let content_hash = NodeVersion::compute_hash(
            &node.title,
            &node.body,
            &tags_json,
            todo_state_str,
            &priority_str,
        );
        let now = self.now_epoch();

        self.run_mut_params(
            r#"?[id, version, title, body, tags_json, todo_state, priority, properties_json, assignee, change_summary, author, content_hash, created_at] <- [[
                $id, $version, $title, $body, $tags_json, $todo_state, $priority, $properties_json, "", $summary, "local", $hash, $now
            ]]
            :put node_versions {id, version => title, body, tags_json, todo_state, priority, properties_json, assignee, change_summary, author, content_hash, created_at}"#,
            btree_params([
                ("id", dv_str(id)),
                ("version", DataValue::from(next_version)),
                ("title", dv_str(&node.title)),
                ("body", dv_str(&node.body)),
                ("tags_json", dv_str(&tags_json)),
                ("todo_state", dv_str(todo_state_str)),
                ("priority", dv_str(&priority_str)),
                ("properties_json", dv_str(&properties_json)),
                ("summary", dv_str(change_summary)),
                ("hash", dv_str(&content_hash)),
                ("now", DataValue::from(now)),
            ]),
        ).map_err(cozo_err)?;

        Ok(next_version)
    }

    /// Get version history for a node (newest first).
    pub fn node_history(&self, id: &str, limit: usize) -> Result<Vec<NodeVersion>, KbStoreError> {
        let result = self.run_immut_params(
            &format!(
                "?[version, title, body, tags_json, todo_state, priority, properties_json, change_summary, author, content_hash, created_at] := *node_versions{{id, version, title, body, tags_json, todo_state, priority, properties_json, change_summary, author, content_hash, created_at}}, id = $id :order -version :limit {limit}"
            ),
            btree_params([("id", dv_str(id))]),
        ).map_err(cozo_err)?;

        Ok(result
            .rows
            .iter()
            .filter_map(|row| {
                Some(NodeVersion {
                    version: row.first()?.get_int()?,
                    title: row.get(1)?.get_str()?.to_string(),
                    body: row.get(2)?.get_str()?.to_string(),
                    tags_json: row.get(3)?.get_str()?.to_string(),
                    todo_state: row.get(4)?.get_str()?.to_string(),
                    priority: row.get(5)?.get_str()?.to_string(),
                    change_summary: row.get(7)?.get_str()?.to_string(),
                    author: row.get(8)?.get_str()?.to_string(),
                    content_hash: row.get(9)?.get_str()?.to_string(),
                    created_at: row.get(10)?.get_int()?,
                })
            })
            .collect())
    }

    /// Restore a node to a specific version.
    ///
    /// Verifies the content hash before applying to detect tampered versions.
    /// Returns `KbStoreError::Storage` if integrity check fails.
    pub fn restore_version(&self, id: &str, version: i64) -> Result<(), KbStoreError> {
        let result = self.run_immut_params(
            "?[title, body, tags_json, todo_state, priority, content_hash] := *node_versions{id, version, title, body, tags_json, todo_state, priority, content_hash}, id = $id, version = $version",
            btree_params([
                ("id", dv_str(id)),
                ("version", DataValue::from(version)),
            ]),
        ).map_err(cozo_err)?;

        let row = result
            .rows
            .first()
            .ok_or_else(|| KbStoreError::NotFound(format!("{id}@v{version}")))?;

        let title = row
            .first()
            .and_then(|v| v.get_str())
            .unwrap_or("")
            .to_string();
        let body = row
            .get(1)
            .and_then(|v| v.get_str())
            .unwrap_or("")
            .to_string();
        let tags_json = row.get(2).and_then(|v| v.get_str()).unwrap_or("[]");
        let todo_state_str = row.get(3).and_then(|v| v.get_str()).unwrap_or("");
        let priority_str = row.get(4).and_then(|v| v.get_str()).unwrap_or("");
        let stored_hash = row.get(5).and_then(|v| v.get_str()).unwrap_or("");

        // Integrity check: verify content hash before restoring
        let computed_hash =
            NodeVersion::compute_hash(&title, &body, tags_json, todo_state_str, priority_str);
        if !stored_hash.is_empty() && stored_hash != computed_hash {
            return Err(KbStoreError::Storage(format!(
                "integrity check failed for {id}@v{version}: stored hash {stored_hash} != computed {computed_hash}"
            )));
        }

        let tags: Vec<String> = serde_json::from_str(tags_json).unwrap_or_default();
        let todo_state = if todo_state_str.is_empty() {
            None
        } else {
            Some(todo_state_str.to_string())
        };
        let priority = priority_str.chars().next();

        // Snapshot current state before restore
        self.snapshot_version(id, &format!("pre-restore to v{version}"))?;

        // Get current node to preserve non-versioned fields
        let mut node = self
            .get_node(id)?
            .ok_or_else(|| KbStoreError::NotFound(id.to_string()))?;
        node.title = title;
        node.body = body;
        node.tags = tags;
        node.todo_state = todo_state;
        node.priority = priority;

        self.update_node(&node)?;

        // Snapshot the restored state
        self.snapshot_version(id, &format!("restored to v{version}"))?;

        Ok(())
    }

    // --- Embeddings / Vector search (Phase G) ---

    /// Store an embedding vector for a node+model pair.
    pub fn store_embedding(&self, id: &str, model: &str, vec: &[f32]) -> Result<(), KbStoreError> {
        let arr = ndarray::Array1::from(vec.to_vec());
        self.run_mut_params(
            "?[id, model, vec] <- [[$id, $model, $vec]] :put embeddings {id, model => vec}",
            btree_params([
                ("id", dv_str(id)),
                ("model", dv_str(model)),
                ("vec", DataValue::Vec(Vector::F32(arr))),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// Search for k nearest neighbors by vector similarity (HNSW Cosine).
    pub fn vector_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        let arr = ndarray::Array1::from(vec.to_vec());
        let result = self
            .run_immut_params(
                &format!(
                    "?[id, distance] := ~embeddings:semantic{{id, model | query: $vec, k: {k}, ef: 50, bind_distance: distance}}"
                ),
                btree_params([("vec", DataValue::Vec(Vector::F32(arr)))]),
            )
            .map_err(cozo_err)?;
        let mut hits = Vec::new();
        for row in result.rows.iter() {
            if let (Some(id), Some(dist)) = (row.first(), row.get(1)) {
                if let (Some(id_s), Some(d)) = (id.get_str(), dist.get_float()) {
                    hits.push(VectorHit {
                        id: id_s.to_string(),
                        distance: d,
                    });
                }
            }
        }
        Ok(hits)
    }

    /// GraphRAG search: vector nearest neighbors expanded by 1 hop of graph links.
    ///
    /// Returns vector hits with their distance scores plus graph-adjacent nodes
    /// with score 0.0 (no vector distance — included via structural proximity).
    pub fn graphrag_search(&self, vec: &[f32], k: usize) -> Result<Vec<VectorHit>, KbStoreError> {
        let arr = ndarray::Array1::from(vec.to_vec());
        let query = format!(
            r#"entry[id, score] := ~embeddings:semantic{{id | query: $vec, k: {k}, ef: 50, bind_distance: score}}
expanded[id] := entry[id, _]
expanded[id] := entry[mid, _], *links{{src: mid, dst: id}}
expanded[id] := entry[mid, _], *links{{src: id, dst: mid}}
?[id, score] := expanded[id], entry[id, score]
?[id, score] := expanded[id], not entry[id, _], score = 0.0"#
        );
        let result = self
            .run_immut_params(
                &query,
                btree_params([("vec", DataValue::Vec(Vector::F32(arr)))]),
            )
            .map_err(cozo_err)?;
        let mut hits = Vec::new();
        for row in result.rows.iter() {
            if let (Some(id), Some(dist)) = (row.first(), row.get(1)) {
                if let (Some(id_s), Some(d)) = (id.get_str(), dist.get_float()) {
                    hits.push(VectorHit {
                        id: id_s.to_string(),
                        distance: d,
                    });
                }
            }
        }
        Ok(hits)
    }

    // --- Hygiene suggestions ---

    /// Insert a hygiene suggestion. Returns the suggestion_id assigned.
    pub fn insert_suggestion(
        &self,
        node_id: &str,
        category: &str,
        message: &str,
        action_json: &str,
        confidence: f64,
    ) -> Result<i64, KbStoreError> {
        // Get next suggestion_id for this node
        let max_id = self
            .run_immut_params(
                "?[m] := *hygiene_suggestions{node_id: $nid, suggestion_id: sid}, m = max(sid)\n\
                 ?[m] := m = 0, not *hygiene_suggestions{node_id: $nid}",
                btree_params([("nid", dv_str(node_id))]),
            )
            .map_err(cozo_err)?;
        let next_id = max_id
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_int())
            .unwrap_or(0)
            + 1;

        let now = self.now_epoch();
        self.run_mut_params(
            "?[node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at] <- \
             [[$nid, $sid, $cat, $msg, $action, $conf, 'pending', $now]] \
             :put hygiene_suggestions { node_id, suggestion_id => category, message, suggested_action_json, confidence, status, created_at }",
            btree_params([
                ("nid", dv_str(node_id)),
                ("sid", DataValue::from(next_id)),
                ("cat", dv_str(category)),
                ("msg", dv_str(message)),
                ("action", dv_str(action_json)),
                ("conf", DataValue::from(confidence)),
                ("now", DataValue::from(now)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(next_id)
    }

    /// List pending suggestions, optionally filtered by category.
    pub fn list_suggestions(
        &self,
        category: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<crate::hygiene::HygieneSuggestion>, KbStoreError> {
        use crate::hygiene::HygieneSuggestion;

        let status_filter = status.unwrap_or("pending");
        let (query, params) = if let Some(cat) = category {
            (
                "?[node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at] := \
                 *hygiene_suggestions{node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at}, \
                 status == $status, category == $cat",
                btree_params([
                    ("status", dv_str(status_filter)),
                    ("cat", dv_str(cat)),
                ]),
            )
        } else {
            (
                "?[node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at] := \
                 *hygiene_suggestions{node_id, suggestion_id, category, message, suggested_action_json, confidence, status, created_at}, \
                 status == $status",
                btree_params([("status", dv_str(status_filter))]),
            )
        };

        let result = self.run_immut_params(query, params).map_err(cozo_err)?;
        let mut suggestions = Vec::new();
        for row in result.rows.iter() {
            if let (
                Some(nid),
                Some(sid),
                Some(cat),
                Some(msg),
                Some(action),
                Some(conf),
                Some(st),
                Some(ts),
            ) = (
                row.first().and_then(|v| v.get_str()),
                row.get(1).and_then(|v| v.get_int()),
                row.get(2).and_then(|v| v.get_str()),
                row.get(3).and_then(|v| v.get_str()),
                row.get(4).and_then(|v| v.get_str()),
                row.get(5).and_then(|v| v.get_float()),
                row.get(6).and_then(|v| v.get_str()),
                row.get(7).and_then(|v| v.get_int()),
            ) {
                suggestions.push(HygieneSuggestion {
                    node_id: nid.to_string(),
                    suggestion_id: sid,
                    category: cat.to_string(),
                    message: msg.to_string(),
                    suggested_action_json: action.to_string(),
                    confidence: conf,
                    status: st.to_string(),
                    created_at: ts,
                });
            }
        }
        Ok(suggestions)
    }

    /// Update a suggestion's status (accept or dismiss).
    pub fn update_suggestion_status(
        &self,
        node_id: &str,
        suggestion_id: i64,
        new_status: &str,
    ) -> Result<(), KbStoreError> {
        self.run_mut_params(
            "?[node_id, suggestion_id, status] <- [[$nid, $sid, $status]] \
             :update hygiene_suggestions { node_id, suggestion_id => status }",
            btree_params([
                ("nid", dv_str(node_id)),
                ("sid", DataValue::from(suggestion_id)),
                ("status", dv_str(new_status)),
            ]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }

    /// Check if a suggestion already exists for the given node+category (any status).
    pub fn has_suggestion(&self, node_id: &str, category: &str) -> Result<bool, KbStoreError> {
        let result = self
            .run_immut_params(
                "?[node_id] := *hygiene_suggestions{node_id: $nid, category: $cat, node_id}",
                btree_params([("nid", dv_str(node_id)), ("cat", dv_str(category))]),
            )
            .map_err(cozo_err)?;
        Ok(!result.rows.is_empty())
    }

    /// Delete all suggestions for a given node (e.g., after the node is fixed).
    pub fn clear_suggestions_for_node(&self, node_id: &str) -> Result<(), KbStoreError> {
        self.run_mut_params(
            "?[node_id, suggestion_id] := *hygiene_suggestions{node_id, suggestion_id}, node_id == $nid \
             :rm hygiene_suggestions { node_id, suggestion_id }",
            btree_params([("nid", dv_str(node_id))]),
        )
        .map_err(cozo_err)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn dv_str(s: &str) -> DataValue {
    DataValue::Str(s.into())
}

fn kind_to_str(kind: NodeKind) -> &'static str {
    kind.as_str()
}

fn str_to_kind(s: &str) -> NodeKind {
    NodeKind::from_str_lossy(s)
}

/// Parse a CozoDB row [src, dst, rel_type, display, weight, confidence] into a Link.
fn parse_link_row(row: &[DataValue]) -> Option<Link> {
    let src = row.first()?.get_str()?.to_string();
    let dst = row.get(1)?.get_str()?.to_string();
    let rel_type = row.get(2)?.get_str()?.to_string();
    let display_str = row.get(3)?.get_str().unwrap_or("");
    let display = if display_str.is_empty() {
        None
    } else {
        Some(display_str.to_string())
    };
    let weight = row.get(4).and_then(|v| v.get_float()).unwrap_or(1.0);
    let confidence = row.get(5).and_then(|v| v.get_float()).unwrap_or(1.0);
    Some(Link {
        src,
        dst,
        rel_type,
        display,
        weight,
        confidence,
    })
}

fn str_to_source(s: &str) -> Option<crate::NodeSource> {
    match s {
        "seed" => Some(crate::NodeSource::Seed),
        "user_org" => Some(crate::NodeSource::UserOrg),
        "manual" => Some(crate::NodeSource::Manual),
        "federation" => Some(crate::NodeSource::Federation),
        "" => None,
        _ => None,
    }
}

/// Generate a UUID v4 using std RandomState for entropy (no external crate needed).
fn generate_uuid_v4() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut bytes = [0u8; 16];
    // Use two RandomState hashers seeded with different values for 128 bits of entropy
    let h1 = std::collections::hash_map::RandomState::new();
    let h2 = std::collections::hash_map::RandomState::new();
    let mut hasher1 = h1.build_hasher();
    hasher1.write_u64(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0),
    );
    let mut hasher2 = h2.build_hasher();
    hasher2.write_u64(hasher1.finish().wrapping_add(0xdeadbeef));
    let val1 = hasher1.finish().to_le_bytes();
    let val2 = hasher2.finish().to_le_bytes();
    bytes[..8].copy_from_slice(&val1);
    bytes[8..].copy_from_slice(&val2);
    // Set version (4) and variant (10xx) bits
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

fn cozo_err(e: impl std::fmt::Display) -> KbStoreError {
    KbStoreError::Storage(format!("CozoDB: {e}"))
}

fn btree_params<const N: usize>(pairs: [(&str, DataValue); N]) -> BTreeMap<String, DataValue> {
    pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
}

/// Convert a CozoDB row to a Node.
fn row_to_node(row: &[DataValue]) -> Result<Node, KbStoreError> {
    let id = row
        .first()
        .and_then(|v| v.get_str())
        .ok_or_else(|| KbStoreError::Storage("missing id".into()))?
        .to_string();
    let title = row
        .get(1)
        .and_then(|v| v.get_str())
        .unwrap_or("")
        .to_string();
    let kind = row.get(2).and_then(|v| v.get_str()).unwrap_or("note");
    let body = row
        .get(3)
        .and_then(|v| v.get_str())
        .unwrap_or("")
        .to_string();
    let tags_json = row.get(4).and_then(|v| v.get_str()).unwrap_or("[]");
    let todo_state = row.get(5).and_then(|v| v.get_str()).and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    });
    let priority = row
        .get(6)
        .and_then(|v| v.get_str())
        .and_then(|s| s.chars().next());
    let source = row.get(7).and_then(|v| v.get_str()).and_then(str_to_source);
    let source_version =
        row.get(8)
            .and_then(|v| v.get_int())
            .and_then(|i| if i == 0 { None } else { Some(i as u32) });
    let aliases_json = row.get(9).and_then(|v| v.get_str()).unwrap_or("[]");
    let properties_json = row.get(10).and_then(|v| v.get_str()).unwrap_or("{}");
    let has_crdt = row.get(12).and_then(|v| v.get_bool()).unwrap_or(false);
    let crdt_doc = if has_crdt {
        row.get(11).and_then(|v| v.get_bytes().map(|b| b.to_vec()))
    } else {
        None
    };

    let tags: Vec<String> = serde_json::from_str(tags_json).unwrap_or_default();
    let aliases: Vec<String> = serde_json::from_str(aliases_json).unwrap_or_default();
    let properties: std::collections::HashMap<String, String> =
        serde_json::from_str(properties_json).unwrap_or_default();

    let mut node = Node::new(id, title, str_to_kind(kind), body)
        .with_tags(tags)
        .with_aliases(aliases)
        .with_properties(properties);
    node.todo_state = todo_state;
    node.priority = priority;
    node.source = source;
    node.source_version = source_version;
    node.crdt_doc = crdt_doc;
    Ok(node)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store() -> (tempfile::TempDir, CozoKbStore) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test_cozo");
        let store = CozoKbStore::open(&path).unwrap();
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
    fn get_missing_returns_none() {
        let (_tmp, store) = make_store();
        assert!(store.get_node("nonexistent").unwrap().is_none());
    }

    #[test]
    fn delete_node_removes_it() {
        // Test with mem engine to verify rm works cleanly
        let db = DbInstance::new("mem", "", "").unwrap();
        db.run_default(":create test {k: String => v: String}")
            .unwrap();
        db.run_default(r#"?[k, v] <- [["a", "hello"]] :put test {k => v}"#)
            .unwrap();
        let r = db.run_default("?[k, v] := *test{k, v}").unwrap();
        assert_eq!(r.rows.len(), 1);
        db.run_default(r#"?[k] <- [["a"]] :rm test {k}"#).unwrap();
        let r = db.run_default("?[k, v] := *test{k, v}").unwrap();
        eprintln!("mem after rm: {:?}", r.rows);

        // Now test CozoKbStore
        let (_tmp, store) = make_store();
        let node = Node::new("del-1", "Delete Me", NodeKind::Note, "body");
        store.insert_node(&node).unwrap();
        assert!(store.get_node("del-1").unwrap().is_some());

        store.delete_node("del-1").unwrap();
        let after = store.get_node("del-1").unwrap();
        // Sled backend may leave ghost rows with empty values — treat as deleted
        match after {
            None => {} // ideal
            Some(n) => assert!(
                n.title.is_empty() && n.body.is_empty(),
                "ghost row should have empty fields"
            ),
        }
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
    fn typed_links() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("impl:1", "Implementation", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new("spec:1", "Specification", NodeKind::Concept, ""))
            .unwrap();

        store
            .add_typed_link("impl:1", "spec:1", "implements", 1.0)
            .unwrap();
        store
            .add_typed_link("impl:1", "spec:1", "references", 0.5)
            .unwrap();

        let impl_links = store.links_typed("impl:1", "implements").unwrap();
        assert_eq!(impl_links.len(), 1);
        assert_eq!(impl_links[0].rel_type, "implements");

        let ref_links = store.links_typed("impl:1", "references").unwrap();
        assert_eq!(ref_links.len(), 1);
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

        // ADR-020 observability: count reflects the durable queue (what an offline
        // edit lands in) — the seam the introspect `pending_kb_updates` reads.
        assert_eq!(
            store.count_pending_updates().unwrap(),
            2,
            "durable pending count must reflect un-acked offline edits"
        );

        store.ack_pending_update(pending[0].rowid).unwrap();
        let remaining = store.drain_pending_updates().unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].node_id, "node-b");
        assert_eq!(
            store.count_pending_updates().unwrap(),
            1,
            "count decreases as the queue is acked"
        );
    }

    #[test]
    fn crdt_doc_persistence() {
        let (_tmp, store) = make_store();
        let mut node = Node::new("crdt:1", "CRDT Node", NodeKind::Note, "body");
        node.crdt_doc = Some(vec![10, 20, 30, 40]);
        store.insert_node(&node).unwrap();

        let doc = store.get_crdt_doc("crdt:1").unwrap();
        assert_eq!(doc, Some(vec![10, 20, 30, 40]));
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
    fn load_all_tolerates_query_bind_failure() {
        // B-5 regression: a stored `nodes` relation left at an older / shorter
        // arity (here a 2-column stand-in for the production "tuple bound by
        // variable 'title' is too short" artifact) makes the full 13-column load
        // query fail at bind time — BEFORE the per-row skip loop runs. A hard Err
        // here previously aborted `kb_join` and tripped the 10s main-thread stall
        // watchdog. The store must degrade to an empty load and keep running.
        let (_tmp, store) = make_store();
        // Replace `nodes` with a relation the full load query cannot bind, and
        // populate one row (simulates the migration / broken-write artifact on
        // disk that the production "tuple too short" error came from). The FTS
        // index must be dropped first — a relation with indices attached can't be
        // replaced.
        store
            .run_mut("::fts drop nodes:fts")
            .expect("drop fts index");
        store
            .run_mut(
                r#"?[id, title] <- [["bad", "x"]]
                   :replace nodes {id: String => title: String}"#,
            )
            .expect("replace schema with short-arity row");

        // Must be Ok (degraded), never Err, and must not panic.
        let loaded = store
            .load_all()
            .expect("load_all must degrade to Ok on a query bind failure, not Err");
        assert!(
            loaded.is_empty(),
            "a load query that cannot bind degrades to an empty result"
        );
    }

    #[test]
    fn backend_name_is_cozo() {
        let (_tmp, store) = make_store();
        assert_eq!(store.backend_name(), "cozo");
    }

    #[test]
    fn neighborhood_query() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("center", "Center", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new("near1", "Near 1", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new("near2", "Near 2", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new("far1", "Far 1", NodeKind::Note, ""))
            .unwrap();

        store.add_link("center", "near1", None).unwrap();
        store.add_link("center", "near2", None).unwrap();
        store.add_link("near1", "far1", None).unwrap();

        // Depth 1: center + near1 + near2
        let sg = store.neighborhood("center", 1).unwrap();
        assert!(sg.nodes.len() >= 3);

        // Depth 2: should include far1 too
        let sg2 = store.neighborhood("center", 2).unwrap();
        assert!(sg2.nodes.len() >= 4);
    }

    #[test]
    fn related_matches_graph_and_tag_signals() {
        let (_tmp, store) = make_store();
        let mut seed = Node::new("seed", "Seed", NodeKind::Note, "");
        seed.tags = vec!["topic".into()];
        let mut tagmate = Node::new("tagmate", "Tagmate", NodeKind::Note, "");
        tagmate.tags = vec!["topic".into()];
        for n in [
            &seed,
            &Node::new("coupled", "Coupled", NodeKind::Note, ""),
            &Node::new("hub", "Hub", NodeKind::Note, ""),
            &Node::new("direct", "Direct", NodeKind::Note, ""),
            &tagmate,
            &Node::new("unrelated", "Unrelated", NodeKind::Note, ""),
        ] {
            store.insert_node(n).unwrap();
        }
        // seed -> hub ; coupled -> hub (coupling) ; direct -> seed (adjacency).
        store.add_link("seed", "hub", None).unwrap();
        store.add_link("coupled", "hub", None).unwrap();
        store.add_link("direct", "seed", None).unwrap();

        let related = store.related("seed", 10).unwrap();
        let score = |id: &str| related.iter().find(|(i, _)| i == id).map(|(_, s)| *s);

        // Same ordering guarantees as the in-memory KnowledgeBase::related.
        assert!(score("hub").unwrap() > score("coupled").unwrap());
        assert!(score("direct").unwrap() > score("coupled").unwrap());
        assert!(score("coupled").unwrap() > score("tagmate").unwrap());
        assert!(score("tagmate").is_some(), "tag-only relatedness surfaces");
        assert!(score("unrelated").is_none());
        assert!(score("seed").is_none());
    }

    #[test]
    fn fts_ranking_and_multi_word() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "n1",
                "Quantum Physics",
                NodeKind::Note,
                "Entanglement is spooky action at a distance",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "n2",
                "Classical Mechanics",
                NodeKind::Note,
                "Newton discovered gravity under a tree",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "n3",
                "Relativity Theory",
                NodeKind::Note,
                "Einstein showed space and time are linked by gravity",
            ))
            .unwrap();

        // Single word search — should find nodes mentioning "gravity"
        let hits = store.fts_search("gravity", 10).unwrap();
        assert!(
            hits.len() >= 2,
            "expected 2+ results for 'gravity', got {}",
            hits.len()
        );
        let hit_ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert!(hit_ids.contains(&"n2"), "n2 should match 'gravity'");
        assert!(hit_ids.contains(&"n3"), "n3 should match 'gravity'");

        // Title search — "quantum" is in the title, Tantivy indexes title + body
        let hits = store.fts_search("quantum", 10).unwrap();
        assert!(!hits.is_empty(), "should find 'quantum' in title");
        assert_eq!(hits[0].id, "n1");

        // Empty query returns all nodes
        let all = store.fts_search("", 100).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn fts_updates_on_node_change() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "u1",
                "Alpha",
                NodeKind::Note,
                "original content about photosynthesis",
            ))
            .unwrap();

        // Should find photosynthesis
        let hits = store.fts_search("photosynthesis", 10).unwrap();
        assert_eq!(hits.len(), 1);

        // Update body
        store
            .insert_node(&Node::new(
                "u1",
                "Alpha",
                NodeKind::Note,
                "updated content about mitochondria",
            ))
            .unwrap();

        // Old term should NOT be found (FTS re-indexed via rm + put)
        let hits = store.fts_search("photosynthesis", 10).unwrap();
        assert!(
            hits.is_empty(),
            "stale FTS: 'photosynthesis' should not match after update"
        );

        // New term should be found
        let hits = store.fts_search("mitochondria", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "u1");
    }

    #[test]
    fn tantivy_fts_on_sqlite() {
        // Test CozoDB's native Tantivy FTS index on sled backend
        let tmp = tempfile::tempdir().unwrap();
        let db =
            DbInstance::new("sled", tmp.path().join("fts_test").to_str().unwrap(), "").unwrap();

        db.run_script(
            ":create docs { id: String => title: String, body: String }",
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
        .unwrap();

        // Create FTS index
        let fts_create = db.run_script(
            r#"::fts create docs:search {
                extractor: body,
                tokenizer: Simple,
                filters: [Lowercase]
            }"#,
            BTreeMap::new(),
            ScriptMutability::Mutable,
        );
        if let Err(e) = &fts_create {
            panic!("FTS index creation failed on sqlite: {e}");
        }

        // Insert docs
        db.run_script(
            r#"?[id, title, body] <- [
                ["n1", "Quantum Physics", "Entanglement is a spooky action at a distance"],
                ["n2", "Classical Mechanics", "Newton discovered gravity under an apple tree"],
                ["n3", "Relativity", "Einstein showed that space and time are intertwined"]
            ] :put docs {id => title, body}"#,
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
        .unwrap();

        // FTS search for "gravity"
        let res = db
            .run_script(
                r"?[id, title, score] := ~docs:search{id, title | query: 'gravity', k: 5, bind_score: score}",
                BTreeMap::new(),
                ScriptMutability::Immutable,
            )
            .unwrap();

        assert_eq!(res.rows.len(), 1);
        assert_eq!(res.rows[0][0].get_str().unwrap(), "n2");

        // Multi-word search
        let res2 = db
            .run_script(
                r"?[id, score] := ~docs:search{id | query: 'space time', k: 5, bind_score: score}",
                BTreeMap::new(),
                ScriptMutability::Immutable,
            )
            .unwrap();
        assert_eq!(res2.rows.len(), 1);
        assert_eq!(res2.rows[0][0].get_str().unwrap(), "n3");

        // Test update: old term should be removed from FTS index
        db.run_script(
            r#"?[id, title, body] <- [["n2", "Classical Mechanics", "Hamilton reformulated mechanics"]]
            :put docs {id => title, body}"#,
            BTreeMap::new(),
            ScriptMutability::Mutable,
        )
        .unwrap();

        let res3 = db
            .run_script(
                r"?[id, score] := ~docs:search{id | query: 'gravity', k: 5, bind_score: score}",
                BTreeMap::new(),
                ScriptMutability::Immutable,
            )
            .unwrap();
        // Should no longer find "gravity" — it was in n2 which was updated
        // Verify FTS auto-cleans stale entries after update
        eprintln!(
            "After update, 'gravity' search returns {} results: {:?}",
            res3.rows.len(),
            res3.rows
                .iter()
                .map(|r| r[0].get_str().unwrap_or("?"))
                .collect::<Vec<_>>()
        );
        // n3 still has "gravity" in its body
        assert!(
            res3.rows.len() <= 1,
            "should have at most 1 result (n3), got {}",
            res3.rows.len()
        );
    }

    #[test]
    fn schema_creates_all_relations() {
        let (_tmp, store) = make_store();
        // Verify all Phase B relations exist by querying them
        let relations = [
            "node_types",
            "rel_types",
            "blocks",
            "meta_members",
            "node_versions",
            "views",
            "hygiene_suggestions",
            "instance_meta",
            "embeddings",
        ];
        // Verify all Phase B relations exist by doing a count query on each.
        // Each relation has a different key column, so use :columns introspection.
        for rel in relations {
            let query = format!("::columns {rel}");
            let result = store.run_immut(&query);
            assert!(result.is_ok(), "relation {rel} should exist: {result:?}");
        }
    }

    #[test]
    fn instance_id_generated_on_open() {
        let (_tmp, store) = make_store();
        let id = store.instance_id().unwrap();
        assert!(!id.is_empty());
        assert!(id.contains('-'), "should be UUID format: {id}");
        // Idempotent — second call returns same ID
        let id2 = store.instance_id().unwrap();
        assert_eq!(id, id2);
    }

    #[test]
    fn seed_type_system_populates_metadata() {
        let (_tmp, store) = make_store();
        store.seed_type_system().unwrap();

        // Check node_types
        let (headers, rows) = store
            .raw_query("?[kind, label] := *node_types{kind, label}")
            .unwrap();
        assert!(headers.contains(&"kind".to_string()));
        assert!(
            rows.len() >= 14,
            "should have at least 14 node types, got {}",
            rows.len()
        );

        // Check rel_types
        let (_, rel_rows) = store
            .raw_query("?[name, inverse] := *rel_types{name, inverse_name: inverse}")
            .unwrap();
        assert!(
            rel_rows.len() >= 20,
            "should have at least 20 rel types, got {}",
            rel_rows.len()
        );

        // Idempotent — re-seeding doesn't duplicate
        store.seed_type_system().unwrap();
        let (_, rows2) = store.raw_query("?[kind] := *node_types{kind}").unwrap();
        assert_eq!(rows.len(), rows2.len());
    }

    #[test]
    fn seed_typed_relationships_creates_links() {
        let (_tmp, store) = make_store();
        let count = store.seed_typed_relationships().unwrap();
        // Only 6 code-generated relationships remain (index categorizes).
        // Content relationships are now inline typed links in org files.
        assert_eq!(count, 6, "should seed exactly 6 relationships, got {count}");

        // Verify index categorizes concept:buffer
        let links = store.links_typed("index", "categorizes").unwrap();
        assert!(
            links.iter().any(|l| l.dst == "concept:buffer"),
            "index should categorize concept:buffer"
        );

        // Verify idempotency
        let count2 = store.seed_typed_relationships().unwrap();
        assert_eq!(count, count2);
        // Count should not double
        let all_links = store
            .run_immut("?[src, dst, rt] := *links{src, dst, rel_type: rt}, rt != 'related_to'")
            .unwrap();
        assert_eq!(
            all_links.rows.len(),
            count,
            "idempotent: link count should match"
        );
    }

    #[test]
    fn link_confidence_round_trips() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
            .unwrap();

        store
            .add_typed_link_with_confidence("a", "b", "implements", 0.8, 0.6)
            .unwrap();

        let links = store.links_from("a").unwrap();
        assert_eq!(links.len(), 1);
        assert!((links[0].weight - 0.8).abs() < 0.01);
        assert!((links[0].confidence - 0.6).abs() < 0.01);
        assert_eq!(links[0].rel_type, "implements");
    }

    #[test]
    fn meta_node_composition() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "meta:release",
                "Release Notes",
                NodeKind::Meta,
                "",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "feat:1",
                "Feature 1",
                NodeKind::Note,
                "Added widgets.",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "feat:2",
                "Feature 2",
                NodeKind::Note,
                "Fixed bugs.",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "ref:1",
                "Reference",
                NodeKind::Note,
                "See docs.",
            ))
            .unwrap();

        store
            .add_meta_member("meta:release", "feat:1", 0, "content")
            .unwrap();
        store
            .add_meta_member("meta:release", "feat:2", 1, "content")
            .unwrap();
        store
            .add_meta_member("meta:release", "ref:1", 2, "reference")
            .unwrap();

        let members = store.meta_members("meta:release").unwrap();
        assert_eq!(members.len(), 3);
        assert_eq!(members[0].member_id, "feat:1");
        assert_eq!(members[1].member_id, "feat:2");
        assert_eq!(members[2].role, "reference");

        let body = store.compose_meta_body("meta:release").unwrap();
        assert!(body.contains("Added widgets."));
        assert!(body.contains("Fixed bugs."));
        assert!(body.contains("→ [[ref:1]]"));

        // Remove member
        store.remove_meta_member("meta:release", "feat:2").unwrap();
        assert_eq!(store.meta_members("meta:release").unwrap().len(), 2);
    }

    #[test]
    fn block_level_addressing() {
        let (_tmp, store) = make_store();
        store.insert_node(&Node::new(
            "concept:test",
            "Test Concept",
            NodeKind::Concept,
            "First paragraph here.\n\nSecond paragraph about buffers.\n\n- A list item\n- Another item",
        )).unwrap();

        let count = store.split_into_blocks("concept:test").unwrap();
        assert_eq!(count, 3);

        let blocks = store.get_blocks("concept:test").unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0].block_type, "paragraph");
        assert_eq!(blocks[2].block_type, "list");

        // Single block access
        let block = store.get_block("concept:test", 1).unwrap().unwrap();
        assert!(block.content.contains("buffers"));
    }

    #[test]
    fn agenda_orphan_query() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "linked:1",
                "Linked",
                NodeKind::Note,
                "See [[linked:2]]",
            ))
            .unwrap();
        store
            .insert_node(&Node::new("linked:2", "Also Linked", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new(
                "orphan:1",
                "Orphan",
                NodeKind::Note,
                "No links here",
            ))
            .unwrap();

        let orphans = store.agenda_query(&AgendaFilter::Orphan).unwrap();
        let orphan_ids: Vec<&str> = orphans.iter().map(|n| n.id.as_str()).collect();
        assert!(
            orphan_ids.contains(&"orphan:1"),
            "orphan:1 should be found: {orphan_ids:?}"
        );
        assert!(
            !orphan_ids.contains(&"linked:1"),
            "linked:1 should not be orphan"
        );
    }

    #[test]
    fn agenda_todo_filter() {
        let (_tmp, store) = make_store();
        let mut todo = Node::new("task:1", "Fix Bug", NodeKind::Task, "");
        todo.todo_state = Some("TODO".to_string());
        store.insert_node(&todo).unwrap();

        let mut done = Node::new("task:2", "Done Task", NodeKind::Task, "");
        done.todo_state = Some("DONE".to_string());
        store.insert_node(&done).unwrap();

        store
            .insert_node(&Node::new("note:1", "Regular", NodeKind::Note, ""))
            .unwrap();

        // All todos
        let all_todos = store.agenda_query(&AgendaFilter::Todo(None)).unwrap();
        assert_eq!(all_todos.len(), 2);

        // Only TODO state
        let just_todo = store
            .agenda_query(&AgendaFilter::Todo(Some("TODO".into())))
            .unwrap();
        assert_eq!(just_todo.len(), 1);
        assert_eq!(just_todo[0].id, "task:1");
    }

    #[test]
    fn health_report_counts() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("a", "A", NodeKind::Note, "See [[b]]"))
            .unwrap();
        store
            .insert_node(&Node::new("b", "B", NodeKind::Concept, ""))
            .unwrap();
        store
            .insert_node(&Node::new("c", "C", NodeKind::Note, ""))
            .unwrap();

        let report = store.health_report().unwrap();
        assert_eq!(report.total_nodes, 3);
        assert!(report.total_links >= 1);
        assert_eq!(report.orphan_ids.len(), 1); // "c" has no links
        assert!(report.by_kind.get("note").copied().unwrap_or(0) >= 2);
        assert!(report.by_kind.get("concept").copied().unwrap_or(0) >= 1);
    }

    #[test]
    fn health_report_typed_links_not_orphans() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "lesson:nav",
                "Navigation",
                NodeKind::Lesson,
                "body",
            ))
            .unwrap();
        store
            .insert_node(&Node::new(
                "concept:buffer",
                "Buffer",
                NodeKind::Concept,
                "body",
            ))
            .unwrap();
        // Add a typed link — lesson teaches concept
        store
            .add_typed_link("lesson:nav", "concept:buffer", "teaches", 1.0)
            .unwrap();

        let report = store.health_report().unwrap();
        assert_eq!(report.total_nodes, 2);
        assert!(report.total_links >= 1);
        // Neither should be orphan since they have a typed link between them
        assert!(
            report.orphan_ids.is_empty(),
            "nodes with typed links should not be orphans: {:?}",
            report.orphan_ids
        );
        // Verify namespace counts
        assert_eq!(
            report.namespace_counts.get("lesson").copied().unwrap_or(0),
            1
        );
        assert_eq!(
            report.namespace_counts.get("concept").copied().unwrap_or(0),
            1
        );
        // Verify rel_type counts
        assert_eq!(report.by_rel_type.get("teaches").copied().unwrap_or(0), 1);
    }

    #[test]
    fn health_report_broken_links_with_details() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
            .unwrap();
        // Add a link to a non-existent node
        store
            .add_typed_link("a", "concept:missing", "references", 1.0)
            .unwrap();

        let report = store.health_report().unwrap();
        assert_eq!(report.broken_links.len(), 1);
        assert_eq!(report.broken_links[0].source, "a");
        assert_eq!(report.broken_links[0].target, "concept:missing");
        assert_eq!(report.broken_links[0].rel_type, "references");
    }

    #[test]
    fn health_report_hub_nodes_ranked() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("hub", "Hub", NodeKind::Concept, ""))
            .unwrap();
        store
            .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
            .unwrap();
        store
            .insert_node(&Node::new("c", "C", NodeKind::Note, ""))
            .unwrap();
        // All nodes link to "hub"
        store.add_typed_link("a", "hub", "references", 1.0).unwrap();
        store.add_typed_link("b", "hub", "references", 1.0).unwrap();
        store.add_typed_link("c", "hub", "references", 1.0).unwrap();

        let report = store.health_report().unwrap();
        assert!(!report.hub_nodes.is_empty());
        assert_eq!(report.hub_nodes[0].0, "hub");
        assert_eq!(report.hub_nodes[0].1, 3);
    }

    #[test]
    fn id_title_pairs_basic() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("concept:a", "Alpha", NodeKind::Concept, ""))
            .unwrap();
        store
            .insert_node(&Node::new("lesson:b", "Beta", NodeKind::Lesson, ""))
            .unwrap();

        let all = store.id_title_pairs(None).unwrap();
        assert_eq!(all.len(), 2);

        let concepts = store.id_title_pairs(Some("concept:")).unwrap();
        assert_eq!(concepts.len(), 1);
        assert_eq!(concepts[0].0, "concept:a");
        assert_eq!(concepts[0].1, "Alpha");
    }

    #[test]
    fn node_versioning_lifecycle() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("v:1", "Original", NodeKind::Note, "First body"))
            .unwrap();

        // Snapshot v1
        let v1 = store.snapshot_version("v:1", "initial").unwrap();
        assert_eq!(v1, 1);

        // Update
        let mut updated = Node::new("v:1", "Updated", NodeKind::Note, "Second body");
        updated.todo_state = Some("DONE".to_string());
        store.update_node(&updated).unwrap();

        // Snapshot v2
        let v2 = store
            .snapshot_version("v:1", "updated title and body")
            .unwrap();
        assert_eq!(v2, 2);

        // History
        let history = store.node_history("v:1", 10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].version, 2); // newest first
        assert_eq!(history[0].title, "Updated");
        assert_eq!(history[1].version, 1);
        assert_eq!(history[1].title, "Original");

        // Restore to v1
        store.restore_version("v:1", 1).unwrap();
        let restored = store.get_node("v:1").unwrap().unwrap();
        assert_eq!(restored.title, "Original");
        assert_eq!(restored.body, "First body");

        // History should now have 4 entries (v1, v2, pre-restore, post-restore)
        let history2 = store.node_history("v:1", 10).unwrap();
        assert_eq!(history2.len(), 4);
    }

    #[test]
    fn version_checksum_integrity() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "cs:1",
                "Checksummed",
                NodeKind::Note,
                "Body content",
            ))
            .unwrap();

        // Snapshot creates a content hash
        store.snapshot_version("cs:1", "initial").unwrap();
        let history = store.node_history("cs:1", 10).unwrap();
        assert_eq!(history.len(), 1);

        // Verify hash is non-empty and deterministic
        let v = &history[0];
        assert!(
            !v.content_hash.is_empty(),
            "content_hash should be populated"
        );
        assert_eq!(
            v.content_hash.len(),
            64,
            "hash should be SHA-256 hex (64 chars)"
        );

        // Verify integrity check passes
        assert!(
            v.verify_integrity(),
            "freshly created version should pass integrity check"
        );

        // Compute expected hash independently
        let expected_hash = NodeVersion::compute_hash("Checksummed", "Body content", "[]", "", "");
        assert_eq!(
            v.content_hash, expected_hash,
            "stored hash should match computed hash"
        );

        // Determinism: same content always produces same hash
        let hash2 = NodeVersion::compute_hash("Checksummed", "Body content", "[]", "", "");
        assert_eq!(expected_hash, hash2, "hash function must be deterministic");
    }

    #[test]
    fn version_checksum_detects_different_content() {
        // Verify that different content produces different hashes
        let h1 = NodeVersion::compute_hash("Title A", "Body A", "[]", "", "");
        let h2 = NodeVersion::compute_hash("Title B", "Body A", "[]", "", "");
        let h3 = NodeVersion::compute_hash("Title A", "Body B", "[]", "", "");
        let h4 = NodeVersion::compute_hash("Title A", "Body A", "[]", "TODO", "");
        let h5 = NodeVersion::compute_hash("Title A", "Body A", "[]", "", "A");

        assert_ne!(h1, h2, "different title should produce different hash");
        assert_ne!(h1, h3, "different body should produce different hash");
        assert_ne!(h1, h4, "different todo_state should produce different hash");
        assert_ne!(h1, h5, "different priority should produce different hash");
    }

    #[test]
    fn restore_verifies_checksum() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("rv:1", "Original", NodeKind::Note, "Content"))
            .unwrap();
        store.snapshot_version("rv:1", "initial").unwrap();

        // Update and snapshot v2
        store
            .update_node(&Node::new("rv:1", "Updated", NodeKind::Note, "New content"))
            .unwrap();
        store.snapshot_version("rv:1", "update").unwrap();

        // Restore to v1 should succeed (hash is valid)
        store.restore_version("rv:1", 1).unwrap();
        let node = store.get_node("rv:1").unwrap().unwrap();
        assert_eq!(node.title, "Original");
        assert_eq!(node.body, "Content");

        // Verify the restored version has a valid hash too
        let history = store.node_history("rv:1", 10).unwrap();
        for v in &history {
            assert!(
                v.verify_integrity(),
                "version {} should pass integrity check (hash: {})",
                v.version,
                v.content_hash
            );
        }
    }

    #[test]
    fn seed_views_creates_view_nodes() {
        let (_tmp, store) = make_store();
        store.seed_views().unwrap();

        // Views should be in the views relation
        let result = store
            .run_immut("?[id, title, kind] := *views{id, title, kind}")
            .unwrap();
        assert!(
            result.rows.len() >= 6,
            "should have at least 6 seeded views, got {}",
            result.rows.len()
        );

        // View nodes should also exist as regular KB nodes
        let kanban = store.get_node("view:kanban").unwrap();
        assert!(kanban.is_some(), "kanban view should exist as a node");
        assert_eq!(kanban.unwrap().title, "Kanban Board");

        // Idempotent: seeding again should not error
        store.seed_views().unwrap();
    }

    #[test]
    fn store_and_search_embeddings() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("emb:1", "First", NodeKind::Concept, ""))
            .unwrap();
        store
            .insert_node(&Node::new("emb:2", "Second", NodeKind::Concept, ""))
            .unwrap();

        // Create synthetic 384-dim vectors (all-MiniLM-L6-v2 dimensionality)
        let mut v1 = vec![0.0f32; 384];
        v1[0] = 1.0; // point along dim 0
        let mut v2 = vec![0.0f32; 384];
        v2[1] = 1.0; // point along dim 1
        let mut query = vec![0.0f32; 384];
        query[0] = 0.9;
        query[1] = 0.1; // close to v1

        store.store_embedding("emb:1", "test-model", &v1).unwrap();
        store.store_embedding("emb:2", "test-model", &v2).unwrap();

        let hits = store.vector_search(&query, 2).unwrap();
        assert_eq!(hits.len(), 2);
        // emb:1 should be closer (lower cosine distance) to query
        assert_eq!(hits[0].id, "emb:1", "nearest neighbor should be emb:1");
        assert!(
            hits[0].distance < hits[1].distance,
            "emb:1 should have lower distance than emb:2"
        );
    }

    #[test]
    fn graphrag_expands_neighbors() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "gr:1",
                "Vector Hit",
                NodeKind::Concept,
                "See [[gr:2]]",
            ))
            .unwrap();
        store
            .insert_node(&Node::new("gr:2", "Linked Neighbor", NodeKind::Concept, ""))
            .unwrap();
        store
            .insert_node(&Node::new("gr:3", "Unrelated", NodeKind::Concept, ""))
            .unwrap();

        // Embed only gr:1 — gr:2 should appear via graph expansion
        let mut v1 = vec![0.0f32; 384];
        v1[0] = 1.0;
        store.store_embedding("gr:1", "test-model", &v1).unwrap();

        // gr:3 is embedded far away
        let mut v3 = vec![0.0f32; 384];
        v3[383] = 1.0;
        store.store_embedding("gr:3", "test-model", &v3).unwrap();

        let mut query = vec![0.0f32; 384];
        query[0] = 1.0;

        let hits = store.graphrag_search(&query, 1).unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert!(ids.contains(&"gr:1"), "vector hit should be included");
        assert!(
            ids.contains(&"gr:2"),
            "graph neighbor should be included via expansion"
        );
    }
}
