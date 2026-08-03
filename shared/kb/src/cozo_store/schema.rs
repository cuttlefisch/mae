//! Schema DDL + one-time setup: relation creation (`ensure_schema`),
//! opening a store (sled/sqlite/mem engines), instance-id bootstrap, the
//! FTS index, and seeding the node/rel-type + view metadata relations.

use super::util::{btree_params, cozo_err, dv_str, generate_uuid_v4};
use super::*;

/// DDL for the `nodes:fts` full-text index — the SINGLE source of truth, shared
/// by initial schema creation and [`CozoKbStore::rebuild_fts`]. Previously these
/// two sites each carried their own copy of the script, so a fix applied to one
/// silently left the other building a differently-shaped index (principle #8).
///
/// @ai-caution: [kb-search] The `'.'` separator between `title` and `body` is
/// load-bearing and MUST NOT be "tidied" back to a space (`' '`), which is what
/// it used to be and what silently broke KB search for every node.
///
/// CozoDB does not persist the extractor as written. `parse/sys.rs` parses it,
/// partial-evaluates it, and stores `expr.to_string()` — and `Display for
/// DataValue` renders a string constant with Rust's `{:?}`, i.e. always DOUBLE
/// quotes. The stored text is re-parsed by `cozoscript.pest` on every indexed
/// row, where `quoted_string_inner = { char* }` is a NON-atomic rule: pest skips
/// implicit `WHITESPACE` between `char` repetitions, so a double-quoted,
/// ALL-whitespace literal matches zero chars and parses back as the EMPTY
/// string. `title ++ ' ' ++ body` therefore persisted as
/// `concat(concat(title, " "), body)` and evaluated as `title ++ body`, welding
/// the last title token onto the first body token: a node titled
/// `"Quantum Physics"` with body `"Entanglement is spooky."` indexed the tokens
/// `quantum`, `physicsentanglement`, `is`, `spooky` — so `quantum` and `spooky`
/// found it while `physics` and `entanglement` returned nothing at all.
///
/// Any non-whitespace separator survives the round trip. `.` is used because the
/// `Simple` tokenizer splits on `!c.is_alphanumeric()`, making it a hard token
/// boundary, and because it needs no escaping in `{:?}` (an escape would NOT
/// survive: cozoscript does not unescape `\n`, so `'\n'` would come back as a
/// literal backslash + `n`, and `n` IS alphanumeric — it would just glue a
/// stray `n` onto the body's first token instead).
///
/// [`FTS_EXTRACTOR_VERSION`] must be bumped whenever this DDL changes, so that
/// stores carrying an index built by the previous definition are rebuilt on open.
const NODES_FTS_DDL: &str = r#"::fts create nodes:fts {
                extractor: title ++ '.' ++ body,
                tokenizer: Simple,
                filters: [Lowercase]
            }"#;

/// Version stamp for [`NODES_FTS_DDL`], persisted in `instance_meta`.
///
/// Bump on any change to the FTS index definition. On open, a store whose stamp
/// differs (or is absent, i.e. predates this mechanism) has its FTS index
/// rebuilt exactly once. Version-stamping rather than comparing the stored
/// manifest text keeps the migration bounded: matching against CozoDB's
/// `Display` output would silently re-trigger a full reindex on every open if a
/// CozoDB upgrade ever changed that formatting.
const FTS_EXTRACTOR_VERSION: &str = "2";

/// `instance_meta` key holding the [`FTS_EXTRACTOR_VERSION`] the on-disk index
/// was built with.
const FTS_VERSION_KEY: &str = "fts_extractor_version";

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

        // Serialize the whole "create from scratch" window (DB-file/directory
        // creation + schema DDL) across concurrent first-time opens of the SAME
        // path -- a real, reproduced panic (issue #447: "index out of bounds"
        // reading `source_files` mid-schema-creation) when 3+ processes race to
        // open/import into an identical not-yet-existing store. Reuses the
        // existing, already-adversarially-tested (ADR-058 Phase B's 3-way-race
        // test) atomic-exclusive-create advisory lock rather than a new
        // mechanism (principle #8) -- `acquire_lock`'s own doc comment names
        // exactly this scenario: "the very first write on a fresh machine...
        // multiple simultaneously-starting processes racing to create that same
        // [path]". Held only across store creation, not the store's whole
        // lifetime -- once schema exists, ordinary concurrent read/write is
        // safe under CozoDB's per-instance write lock for callers sharing one
        // `Db`/`CozoKbStore`. **Correction, found via a real CI failure, not
        // assumed:** this ADR-004/ADR-054 "WAL + busy-retry" framing does NOT
        // hold across *separate* `Db` instances opened against the same file
        // (as this function's own concurrent-open callers are, by
        // definition) -- direct inspection of `cozo-0.7.6/src/storage/
        // sqlite.rs` confirms `new_cozo_sqlite` never sets `journal_mode=WAL`
        // or `busy_timeout`, and each `open_with_engine` call constructs a
        // wholly separate `SqliteStorage` with its own private
        // `Arc<ShardedLock<()>>` -- cozo's own write-serialization is
        // per-instance, not per-file, and its own internal bootstrap
        // statement (`create table if not exists cozo`) `.unwrap()`s the
        // result rather than retrying `SQLITE_BUSY`/"database is locked",
        // so a *new* connection opening while a *different*, already-open
        // connection from an earlier opener is mid-write-transaction can
        // panic instead of waiting. See the retry loop immediately below,
        // which exists specifically to absorb that gap -- this ADR-004 claim
        // is corrected directly in `docs/adr/004-kb-scaling.md`'s own Tier 1
        // section (a real, disproven claim gets the doc fixed, not a second
        // unverified repetition of it here).
        //
        // The retry budget here is deliberately much wider than
        // `with_locked_update`'s (30 x 20ms = 600ms, sized for a small TOML
        // save): schema creation is a one-time, bounded event where N-1
        // waiting openers may each need to wait out a full turn of the
        // relation-creation sequence ahead of them, not a hot path where
        // latency matters. A real 5-way adversarial test caught the original
        // 600ms budget as too tight under genuine contention. Acquisition is
        // also load-bearing here, unlike `with_locked_update`'s "proceed
        // anyway" best-effort semantics -- proceeding unlocked after a
        // failed acquisition would silently reintroduce the exact race this
        // fix exists to close, so a still-unacquired guard after the full
        // budget is a hard error, not a warning.
        let _create_guard = if !path.as_os_str().is_empty() && path != Path::new(":memory:") {
            let guard = mae_mcp::file_lock::LockGuard::try_acquire_with_retry(
                &path,
                200,
                std::time::Duration::from_millis(25),
            );
            if !guard.acquired() {
                return Err(KbStoreError::Storage(format!(
                    "timed out waiting for the store-creation lock on {} (another process/\
                     thread held it for over 5s -- see issue #447)",
                    path.display()
                )));
            }
            Some(guard)
        } else {
            None
        };

        // See ROADMAP.md's "Architecture Debt" section for the cross-reference this
        // marker (and its sibling below) is tracked under.
        //
        // @ai-caution: [architecture-debt] sled 0.34's PageCache::start can panic
        // (not just Err) — "tried to serialize Uninitialized" in
        // sled::pagecache::snapshot — when opening a directory it can't use as a
        // valid store (permission-denied, corrupt/partial). Caught here so a broken
        // on-disk KB store degrades to a normal open failure instead of crashing the
        // editor at startup. Reproduced via `cargo test --release` under full-suite
        // load; see the two adversarial tests in crates/mae/src/bootstrap.rs that hit
        // this path (`init_kb_federation_notifies_on_a_real_store_open_failure`,
        // `init_kb_federation_notifies_on_a_real_migration_failure`).
        //
        // @ai-caution: [architecture-debt] a SECOND, distinct panic source lives in
        // the SAME `DbInstance::new` call for the sqlite engine specifically: a real
        // CI failure (nightly leg, not reproduced locally in dozens of runs --
        // genuinely timing-dependent) caught `new_cozo_sqlite`'s own bootstrap
        // `create table if not exists cozo` statement `.unwrap()`ing a
        // `SQLITE_BUSY`/"database is locked" `Err` while a DIFFERENT, already-open
        // `CozoKbStore` (a separate `Db` instance against the same file, opened by
        // an earlier concurrent caller of this same function) was mid-write-
        // transaction. This is NOT the same race the advisory lock above closes
        // (that one only serializes concurrent *entries into this function*) --
        // ADR-004's assumed "SQLite WAL + busy-retry" safety net does not
        // actually exist in cozo 0.7.6 (`journal_mode`/`busy_timeout` are never
        // configured, confirmed by direct source read; corrected directly in
        // `docs/adr/004-kb-scaling.md`'s Tier 1 section, not re-asserted here
        // a second time). Since that gap lives in an external crate, MAE
        // compensates here: a bounded
        // retry specifically for this transient, well-understood SQLite condition
        // (never for a genuinely corrupt store, which is a different panic
        // message and correctly still fails fast below).
        let db = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retry_on_transient_sqlite_busy(|| DbInstance::new(&engine_owned, &path_str, ""))
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

        // Tantivy FTS index on nodes (title + body combined) — see
        // `NODES_FTS_DDL` for the separator invariant.
        // NOTE: Post-query verification in fts_search() guards against stale FTS
        // entries (observed with sled backend; kept as defensive measure).
        self.run_mut(NODES_FTS_DDL)
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

        // ADR-061 Phase B: content-addressed embedding cache -- deliberately a
        // SEPARATE relation from `embeddings` above, not an extra key column on it.
        // `embeddings` is keyed by (id, model) and HNSW-indexed with a FIXED
        // `<F32; 384>` vector width (all-MiniLM-L6-v2's dimension) because it's the
        // relation `vector_search`/`graphrag_search` actually query for similarity.
        // This cache answers a different question -- "have we already paid to embed
        // this exact (content, model, chunking) combination" -- pure exact-key
        // lookup, never similarity-searched, so its `vec` column is a plain
        // variable-length list (`[Float]`), not the fixed-width HNSW type. That
        // also means this cache is NOT locked to any one embedding model's output
        // dimension, unlike `embeddings` -- a real, separately-tracked limitation
        // (see the Phase B implementation note) rather than something this schema
        // change needs to fix. `chunk_version` is a small integer versioning MAE's
        // own chunking algorithm, bumped only when that algorithm changes.
        self.create_if_absent(
            r#":create embedding_cache {
                content_hash: String,
                model: String,
                chunk_version: Int
                =>
                vec: [Float]
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

        // Rebuild the FTS index if it was built by an older extractor
        // definition. MUST come after `instance_meta` exists.
        self.ensure_fts_index_current()?;

        Ok(())
    }
    /// Rebuild `nodes:fts` if it was built by a superseded [`NODES_FTS_DDL`].
    ///
    /// A CozoDB FTS index is populated at `::fts create` time and incrementally
    /// maintained on write — changing the DDL therefore has NO effect on a store
    /// whose index already exists. Without this, the separator fix documented on
    /// [`NODES_FTS_DDL`] would only ever reach brand-new KBs, and every KB
    /// already on disk would keep silently dropping search terms forever.
    ///
    /// Runs at most once per version bump: the stamp is written after a
    /// successful rebuild, so an interrupted rebuild retries on the next open
    /// rather than being recorded as done.
    fn ensure_fts_index_current(&self) -> Result<(), KbStoreError> {
        let stamped = self
            .run_immut_params(
                "?[val] := *instance_meta{key: $key, val}",
                btree_params([("key", dv_str(FTS_VERSION_KEY))]),
            )
            .map_err(cozo_err)?
            .rows
            .first()
            .and_then(|r| r.first())
            .and_then(|v| v.get_str())
            .map(|s| s.to_string());

        if stamped.as_deref() == Some(FTS_EXTRACTOR_VERSION) {
            return Ok(());
        }

        // A failed rebuild must NOT stop the store from opening. `nodes` can be
        // on disk at an older/short arity (the artifact
        // `load_all_tolerates_query_bind_failure` pins), and `::fts create`
        // binds against it — propagating that error here would turn a store
        // that previously degraded gracefully into one that cannot be opened
        // at all, re-breaking the `kb_join` abort + main-thread-stall watchdog
        // that degradation was added for. Leave the old index in place and
        // skip the stamp, so the migration simply retries on the next open.
        if let Err(e) = self.rebuild_fts() {
            tracing::warn!(
                error = %e,
                "KB store: could not rebuild the FTS index; search may miss \
                 terms at the title/body boundary until this succeeds"
            );
            return Ok(());
        }
        self.run_mut_params(
            r#"?[key, val] <- [[$key, $ver]] :put instance_meta {key => val}"#,
            btree_params([
                ("key", dv_str(FTS_VERSION_KEY)),
                ("ver", dv_str(FTS_EXTRACTOR_VERSION)),
            ]),
        )
        .map_err(cozo_err)?;
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

/// Run `f` (a `DbInstance::new`-shaped call returning `Result<T, E>`),
/// retrying a bounded number of times on a transient SQLite
/// `SQLITE_BUSY`/"database is locked" condition — surfacing EITHER as a
/// PANIC (cozo 0.7.6's own bootstrap `create table if not exists cozo`
/// `.unwrap()`s this) OR as a normal `Err(E)` (cozo's post-open
/// `initialize()`/`load_last_ids()` step, which DOES propagate via `?`
/// rather than panicking) — both are the SAME underlying condition
/// (confirmed by direct source read of `cozo-0.7.6/src/runtime/db.rs`'s
/// `initialize()`/`load_last_ids()`), just surfaced two different ways
/// depending on which internal cozo code path hits it first. **Found via
/// two separate real CI failures, not assumed**: an earlier version of this
/// function only caught the panic shape, and a later CI run reproduced the
/// SAME race manifesting as the Err shape instead — proving both needed
/// covering, not just the one first observed. See `open_with_engine`'s
/// `@ai-caution` note for why this is needed at all (cozo 0.7.6 never
/// configures `busy_timeout`). Any OTHER panic message or `Err` (a
/// genuinely corrupt/inaccessible store, matching the sibling sled
/// `@ai-caution` above) is NOT retried — returned on the first occurrence,
/// for the caller's existing error mapping to handle exactly as if this
/// wrapper weren't here.
///
/// Deliberately mirrors `Db::run_with_busy_retry`'s already-battle-tested
/// backoff shape immediately below in this same crate (exponential cap with
/// FULL jitter, not the two-instance-lockstep-prone linear/no-jitter backoff
/// an earlier draft of this function used) rather than reinventing a worse
/// one (principle #8) — the two can't literally share code (that one only
/// ever sees a `Result`, never a panic; this one must handle both shapes
/// from the same underlying condition) — but there is no reason for this
/// backoff's *quality* to regress from established precedent just because
/// the call shape differs. `run_with_busy_retry`'s own doc comment explains
/// why jitter specifically matters here: "Without jitter, identical backoff
/// keeps them in lockstep and they collide forever."
///
/// Bounded by wall-clock time (issue #484), same reasoning and same fix as
/// `Db::run_with_busy_retry`'s own doc comment: a fixed attempt count is an
/// indirect, hardware-dependent proxy for "how long can I wait," and this
/// function had the IDENTICAL `MAX_ATTEMPTS: u32 = 400` vulnerability its
/// sibling did, just never observed failing in CI yet — fixed here too for
/// consistency rather than leaving a second copy of the same latent bug
/// (principle #15: fix drift for the whole feature area, not just the one
/// symptom that happened to be reported first).
pub(crate) fn retry_on_transient_sqlite_busy<T, E: std::fmt::Display>(
    f: impl Fn() -> Result<T, E>,
) -> Result<T, E> {
    retry_on_transient_sqlite_busy_with_deadline(f, DEFAULT_BUSY_RETRY_DEADLINE)
}

/// Production default — matches `Db::run_with_busy_retry`'s own budget
/// (principle #8: one tuned constant, not two that can drift apart). That
/// budget was raised from 20s to 45s after a real Windows CI miss (see
/// `Db::run_with_busy_retry`'s doc comment in `db.rs` for the full story);
/// this constant was not updated to match at the time (cuttlefisch/mae#518
/// item 5) — kept in sync here.
const DEFAULT_BUSY_RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

/// Test-only seam: the "gives up eventually" tests need a SHORT deadline to
/// stay fast (a persistent-contention closure genuinely runs for the entire
/// budget by construction — that's the property being tested), so the real
/// 20s production deadline isn't usable directly in a unit test without
/// making the suite slow. Not part of the public API surface (`pub(crate)`,
/// `#[cfg(test)]`-only caller) — production code always goes through
/// [`retry_on_transient_sqlite_busy`] with the real budget above.
#[cfg(test)]
pub(crate) fn retry_on_transient_sqlite_busy_for_test<T, E: std::fmt::Display>(
    f: impl Fn() -> Result<T, E>,
    deadline: std::time::Duration,
) -> Result<T, E> {
    retry_on_transient_sqlite_busy_with_deadline(f, deadline)
}

fn retry_on_transient_sqlite_busy_with_deadline<T, E: std::fmt::Display>(
    f: impl Fn() -> Result<T, E>,
    deadline: std::time::Duration,
) -> Result<T, E> {
    fn is_transient_busy_message(s: &str) -> bool {
        let s = s.to_ascii_lowercase();
        s.contains("database is locked") || s.contains("sqlite_busy") || s.contains("busy")
    }
    // Poor-man's per-call entropy (a stack address, like `Db::run_with_busy_retry`'s
    // `self as *const Self as u64`) -- no need for a real RNG crate just to
    // desynchronize two competing retriers.
    let seed = &f as *const _ as u64;
    let start = std::time::Instant::now();
    let mut attempt: u32 = 0;
    loop {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&f));
        let is_retryable = match &outcome {
            Ok(Err(e)) => is_transient_busy_message(&e.to_string()),
            Err(payload) => payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .map(is_transient_busy_message)
                .unwrap_or(false),
            Ok(Ok(_)) => false,
        };
        if !is_retryable || start.elapsed() >= deadline {
            return match outcome {
                Ok(result) => result,
                Err(payload) => std::panic::resume_unwind(payload),
            };
        }
        attempt += 1;
        // Exponential cap (~0.25ms -> 8ms) with full jitter, same shape as
        // `Db::run_with_busy_retry` below.
        let cap = (250u64 << attempt.min(5)).min(8_000);
        let jitter = seed
            .wrapping_mul(attempt as u64 + 1)
            .wrapping_add(attempt as u64)
            % (cap + 1);
        std::thread::sleep(std::time::Duration::from_micros(jitter));
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
        self.run_mut(NODES_FTS_DDL).map_err(cozo_err)?;
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
