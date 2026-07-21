//! Schema DDL + one-time setup: relation creation (`ensure_schema`),
//! opening a store (sled/sqlite/mem engines), instance-id bootstrap, the
//! FTS index, and seeding the node/rel-type + view metadata relations.

use super::util::{btree_params, cozo_err, dv_str, generate_uuid_v4};
use super::*;

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
        let path_str = path.to_str().unwrap_or("").to_string();
        let engine_owned = engine.to_string();
        // @ai-caution: [architecture-debt] sled 0.34's PageCache::start can panic
        // (not just Err) — "tried to serialize Uninitialized" in
        // sled::pagecache::snapshot — when opening a directory it can't use as a
        // valid store (permission-denied, corrupt/partial). Caught here so a broken
        // on-disk KB store degrades to a normal open failure instead of crashing the
        // editor at startup. Reproduced via `cargo test --release` under full-suite
        // load; see the two adversarial tests in crates/mae/src/bootstrap.rs that hit
        // this path (`init_kb_federation_notifies_on_a_real_store_open_failure`,
        // `init_kb_federation_notifies_on_a_real_migration_failure`).
        let db = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            DbInstance::new(&engine_owned, &path_str, "")
        }))
        .map_err(|_| {
            KbStoreError::Storage(format!(
                "CozoDB open ({engine}) panicked internally (store directory likely \
                 inaccessible or corrupt)"
            ))
        })?
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
}

impl CozoKbStore {
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
}
