//! Low-level DB/query-execution helpers shared by every other cozo_store
//! submodule: script execution (mutable/immutable, with SQLite BUSY retry),
//! epoch time, node-row/param encoding, and bulk import.
//!
//! @ai-caution: [architecture-debt] These `pub(super)` helpers (run_mut*,
//! run_immut*, now_epoch, node_put_params, ...) are called from ~12 sites
//! across every sibling module in this directory. Keep signatures stable.
//! Cross-referenced in ROADMAP.md's "Architecture Debt" section.

use super::util::{btree_params, cozo_err, dv_str, kind_to_str};
use super::*;

impl CozoKbStore {
    /// Upsert one row into `nodes`. Shared by `insert_node` (single) and
    /// `bulk_import` (many rows, one transaction). Touches NO links.
    pub(super) const NODE_PUT_SCRIPT: &'static str = r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
                aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee, due_date, sprint,
                created_at, updated_at] <- [[
                $id, $title, $kind, $body, $tags_json, $todo_state, $priority, $source, $source_version,
                $aliases_json, $properties_json, $crdt_doc, $has_crdt,
                $origin_instance, $assignee, $due_date, $sprint,
                $created_at, $now
            ]]
            :put nodes {
                id => title, kind, body, tags_json, todo_state, priority, source, source_version,
                aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee, due_date, sprint,
                created_at, updated_at
            }"#;
    /// Bulk upsert into `nodes` from a `$rows` list (one script = one transaction =
    /// one fsync). Column order MUST match [`Self::node_row`].
    const NODE_BULK_SCRIPT: &'static str = r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee, due_date, sprint, created_at, updated_at] <- $rows
            :put nodes {id => title, kind, body, tags_json, todo_state, priority, source, source_version, aliases_json, properties_json, crdt_doc, has_crdt, origin_instance, assignee, due_date, sprint, created_at, updated_at}"#;
    /// Bulk upsert into `links` from a `$rows` list, preserving ALL fields
    /// (rel_type/display/weight/confidence) — links are migrated verbatim (unlike
    /// `update_links_for_node`, which re-derives only body links as `related_to`).
    const LINK_BULK_SCRIPT: &'static str = r#"?[src, dst, rel_type, display, weight, confidence, created_at] <- $rows
            :put links {src, dst, rel_type => display, weight, confidence, created_at}"#;
    /// The four `nodes` columns that are **derived from CRDT truth**, not stored
    /// independently.
    ///
    /// All four shipped hardcoded to `""`/`0` (C3): declared in the schema, read
    /// by the seeded stored views, and unreachable by any write path. So
    /// `view:sprint` — which filters `sprint != ""` — returned the empty set for
    /// every user, always, and `due_date`/`origin_instance` were unwritten *and*
    /// unread.
    ///
    /// **Derived rather than four new CRDT keys, deliberately.** A derived column
    /// is a pure function of CRDT state, so it survives `rebuild_kb` *by
    /// construction* — which is exactly the field-authority rule ("a field not in
    /// the CRDT does not survive") without four separate ADR-093 tolerant-reader
    /// treatments, wire-payload changes and convergence tests to earn it.
    ///
    /// This depends on #655: deriving from `properties` is only sound now that
    /// `props` is canonical and the drawer is a rendering of it, rather than the
    /// two disagreeing.
    fn derived_columns(&self, node: &Node) -> (String, String, i64, String) {
        // Property keys are lowercased by the org parser; look up defensively so
        // a node built in code (`Node::new(..).with_properties(..)`) behaves the
        // same as one ingested from text.
        let prop = |key: &str| -> String {
            node.properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v.trim().to_string())
                .unwrap_or_default()
        };
        let origin_instance = self.instance_id().unwrap_or_default();
        (
            origin_instance,
            prop("assignee"),
            parse_org_due_date(&prop("deadline")),
            prop("sprint"),
        )
    }

    /// Positional column values for one `nodes` row, matching [`Self::NODE_BULK_SCRIPT`].
    fn node_row(&self, node: &Node, now: i64) -> Result<Vec<DataValue>, KbStoreError> {
        let (origin_instance, assignee, due_date, sprint) = self.derived_columns(node);
        let tags_json =
            serde_json::to_string(&node.tags).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let aliases_json = serde_json::to_string(&node.aliases)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let properties_json = serde_json::to_string(&node.properties)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let pri_str = node.priority.map(|c| c.to_string()).unwrap_or_default();
        // Single source of truth for the serialized form (#710) — this used to be
        // a second inline copy of the mapping that now lives on `NodeSource`.
        let source_str = node.source.map(|s| s.as_str()).unwrap_or("");
        let (crdt_bytes, has_crdt) = match &node.crdt_doc {
            Some(doc) => (doc.clone(), true),
            None => (vec![], false),
        };
        Ok(vec![
            dv_str(&node.id),
            dv_str(&node.title),
            dv_str(kind_to_str(node.kind)),
            dv_str(&node.body),
            dv_str(&tags_json),
            dv_str(node.todo_state.as_deref().unwrap_or("")),
            dv_str(&pri_str),
            dv_str(source_str),
            DataValue::from(node.source_version.unwrap_or(0) as i64),
            dv_str(&aliases_json),
            dv_str(&properties_json),
            DataValue::Bytes(crdt_bytes),
            DataValue::Bool(has_crdt),
            dv_str(&origin_instance),
            dv_str(&assignee),
            DataValue::from(due_date),
            dv_str(&sprint),
            // Node age is a fact about the node. Using `now` here meant every
            // write reset it, so `created_at` recorded "last written" and a
            // re-ingest destroyed age outright — `view:backlog` has been
            // ordering by the wrong thing. Prefer the CRDT's immutable stamp;
            // fall back to `now` only for a node that has never carried one.
            DataValue::from(node.created_at.unwrap_or(now)), // created_at
            DataValue::from(now),                            // updated_at
        ])
    }
    /// Build the parameter map for [`Self::NODE_PUT_SCRIPT`] from a node.
    pub(super) fn node_put_params(
        &self,
        node: &Node,
    ) -> Result<BTreeMap<String, DataValue>, KbStoreError> {
        let now = self.now_epoch();
        let (origin_instance, assignee, due_date, sprint) = self.derived_columns(node);
        let tags_json =
            serde_json::to_string(&node.tags).map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let aliases_json = serde_json::to_string(&node.aliases)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let properties_json = serde_json::to_string(&node.properties)
            .map_err(|e| KbStoreError::Storage(e.to_string()))?;
        let pri_str = node.priority.map(|c| c.to_string()).unwrap_or_default();
        // `NodeSource::as_str` is the single source of truth for this mapping
        // (#710). This was a second inline copy of it -- the exact drift #710's
        // fix removed from `node_row` and left here (principle #8).
        let source_str = node.source.map(|s| s.as_str()).unwrap_or("");
        let (crdt_bytes, has_crdt) = match &node.crdt_doc {
            Some(doc) => (doc.clone(), true),
            None => (vec![], false),
        };
        Ok(btree_params([
            ("id", dv_str(&node.id)),
            ("title", dv_str(&node.title)),
            ("kind", dv_str(kind_to_str(node.kind))),
            ("body", dv_str(&node.body)),
            ("tags_json", dv_str(&tags_json)),
            (
                "todo_state",
                dv_str(node.todo_state.as_deref().unwrap_or("")),
            ),
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
            ("origin_instance", dv_str(&origin_instance)),
            ("assignee", dv_str(&assignee)),
            ("due_date", DataValue::from(due_date)),
            ("sprint", dv_str(&sprint)),
            // Node age is a fact about the node, so a write must not reset it.
            // `node_row` already did this; this path did not, so `insert_node`
            // destroyed age while `bulk_import` preserved it -- the same field
            // meaning two different things depending on which door you came
            // through.
            (
                "created_at",
                DataValue::from(node.created_at.unwrap_or(now)),
            ),
            ("now", DataValue::from(now)),
        ]))
    }
    /// Bulk-import `nodes` + `links` into this (fresh) store — nodes in one `:put`
    /// and links in another (two transactions, two fsyncs total) — for FAST
    /// migration. Unlike repeated `insert_node`, it does NOT re-derive links from
    /// node bodies: it writes the exact `links` given, so AI-authored /
    /// non-`related_to` edges survive verbatim.
    pub fn bulk_import(
        &self,
        nodes: &[Node],
        links: &[Link],
    ) -> Result<(usize, usize), KbStoreError> {
        let now = self.now_epoch();
        if !nodes.is_empty() {
            let mut rows = Vec::with_capacity(nodes.len());
            for node in nodes {
                rows.push(DataValue::List(self.node_row(node, now)?));
            }
            self.run_mut_params(
                Self::NODE_BULK_SCRIPT,
                btree_params([("rows", DataValue::List(rows))]),
            )
            .map_err(cozo_err)?;
        }
        if !links.is_empty() {
            let rows: Vec<DataValue> = links
                .iter()
                .map(|l| {
                    DataValue::List(vec![
                        dv_str(&l.src),
                        dv_str(&l.dst),
                        dv_str(&l.rel_type),
                        dv_str(l.display.as_deref().unwrap_or("")),
                        DataValue::from(l.weight),
                        DataValue::from(l.confidence),
                        DataValue::from(now),
                    ])
                })
                .collect();
            self.run_mut_params(
                Self::LINK_BULK_SCRIPT,
                btree_params([("rows", DataValue::List(rows))]),
            )
            .map_err(cozo_err)?;
        }
        Ok((nodes.len(), links.len()))
    }
    /// Run a mutable CozoScript, retrying on SQLite BUSY/locked contention.
    pub(super) fn run_mut(&self, script: &str) -> Result<NamedRows, cozo::Error> {
        self.run_with_busy_retry(|| {
            self.db
                .run_script(script, BTreeMap::new(), ScriptMutability::Mutable)
        })
    }
    /// Run a mutable CozoScript with parameters, retrying on BUSY/locked contention.
    pub(super) fn run_mut_params(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, cozo::Error> {
        self.run_with_busy_retry(|| {
            self.db
                .run_script(script, params.clone(), ScriptMutability::Mutable)
        })
    }
    /// Retry a cozo op on SQLite BUSY / "database is locked" contention.
    ///
    /// cozo 0.7's sqlite backend sets no `busy_timeout` — confirmed by direct
    /// inspection of the vendored `cozo-0.7.6` source (`storage/sqlite.rs`):
    /// `new_cozo_sqlite`/`SqliteStorage::transact` open their own `sqlite` crate
    /// `Connection` internally with no `pragma busy_timeout`/`journal_mode=WAL`
    /// call anywhere, and the connection itself is never exposed via any public
    /// API.
    ///
    /// **CORRECTION.** This used to conclude "there is no hook this crate could
    /// use to set the pragma even if it wanted to". The premise above is right;
    /// that conclusion was wrong. **WAL is a property of the database file's
    /// header, not of the connection** — so MAE sets it out of band before cozo
    /// opens the file (`wal::ensure_wal`), and every cozo connection thereafter
    /// inherits it. Demonstrated in `wal_tests`, not assumed.
    ///
    /// This retry therefore still exists, but for a narrower case than it was
    /// written for: a store on a filesystem that cannot support WAL (network
    /// filesystems cannot), or one created before `ensure_wal` shipped and not
    /// yet reopened. So a concurrent cross-process writer transiently fails with
    /// "database is locked" (an experiment showed ~14% raw write-failure under
    /// two-writer contention, 0% with this backoff), and the only lever
    /// available is an application-level retry loop. Multi-instance
    /// daemon-less sharing depends on it. On the sled backend the predicate
    /// never matches, so this is a zero-cost pass-through.
    ///
    /// **Bounded by wall-clock time, not attempt count** (issue #484): a fixed
    /// `MAX_ATTEMPTS` was tried first (raised 100 → 400 after an earlier CI
    /// flake), but a per-attempt count is an indirect, hardware-dependent proxy
    /// for "how long can I wait" — it silently under-budgets on a slower/more
    /// contended CI runner than whatever machine last tuned the number, which is
    /// exactly what happened again (`sqlite_multi_instance_concurrent_writes_
    /// converge` still exhausted 400 attempts under heavier load). A deadline
    /// bounds the thing this retry actually cares about directly, and adapts to
    /// however slow the CI runner happens to be without needing a manual re-tune.
    ///
    /// `MAX_RETRY_DURATION` was 20s initially, then raised to 45s after a SECOND
    /// real miss specifically on `stable / test (windows)`: that leg runs
    /// `sqlite_multi_instance_concurrent_writes_converge` via plain `cargo test`
    /// (not nextest's per-test PROCESS isolation the Linux/macOS legs get), so
    /// its two writer threads compete for scheduling not just with each other
    /// but with every OTHER concurrently-running mae-core test in the SAME
    /// process — a genuinely worse contention pattern than this backoff was
    /// stress-verified against (16 cores fully CPU-saturated, 8 fully-isolated
    /// PROCESSES each running only this test, completed in 2-4s every time). 45s
    /// gives real headroom above that already-adversarial baseline for Windows'
    /// additionally slower SQLite I/O (NTFS overhead vs. Linux ext4, well
    /// documented) stacked with in-process contention, while staying finite —
    /// a genuinely stuck/deadlocked writer still surfaces as a real failure,
    /// just after exhausting a reasonable budget instead of an arbitrary count.
    fn run_with_busy_retry<F>(&self, mut op: F) -> Result<NamedRows, cozo::Error>
    where
        F: FnMut() -> Result<NamedRows, cozo::Error>,
    {
        const MAX_RETRY_DURATION: std::time::Duration = std::time::Duration::from_secs(45);
        // Per-instance seed so two competing writers jitter differently. Without
        // jitter, identical backoff keeps them in lockstep and they collide forever.
        let seed = self as *const Self as u64;
        let start = std::time::Instant::now();
        let mut attempt: u32 = 0;
        loop {
            match op() {
                Err(e) if start.elapsed() < MAX_RETRY_DURATION && Self::is_busy(&e) => {
                    attempt += 1;
                    // Exponential cap (~0.25ms → 8ms) with FULL jitter: sleep a random
                    // 0..cap so the two writers desynchronize and both make progress
                    // (application-level equivalent of SQLite's busy_timeout, which
                    // cozo 0.7 does not expose).
                    let cap = (250u64 << attempt.min(5)).min(8_000);
                    let jitter = seed
                        .wrapping_mul(attempt as u64 + 1)
                        .wrapping_add(attempt as u64)
                        % (cap + 1);
                    std::thread::sleep(std::time::Duration::from_micros(jitter));
                }
                other => return other,
            }
        }
    }
    /// True if a cozo error is a transient SQLite lock/BUSY that a retry can clear.
    ///
    /// cozo 0.7 hides the underlying SQLite BUSY behind an opaque wrapper — the raw
    /// `cozo::Error` displays only as "CozoDB: when executing against relation '…'"
    /// (the words "locked"/"busy" never surface, and the "storage error:" prefix is
    /// added later by `KbStoreError`). So on the sqlite backend we treat that generic
    /// storage-op wrapper as retryable contention. A genuinely fatal write (disk full,
    /// corruption) still returns after the bounded retries. On sled the write path
    /// does not produce this wrapper, so retries never fire there.
    pub(super) fn is_busy(e: &cozo::Error) -> bool {
        let s = e.to_string().to_ascii_lowercase();
        s.contains("locked") || s.contains("busy") || s.contains("executing against relation")
    }
    /// Run an immutable CozoScript, retrying on SQLite BUSY/locked contention
    /// exactly like `run_mut` above.
    ///
    /// **Found via a real CI failure, not assumed**: a read was originally
    /// unprotected here on the theory that only writers need retry — true
    /// under SQLite's WAL mode, where readers never block on a writer, but
    /// FALSE under the rollback-journal mode cozo 0.7.6 leaves a store in by
    /// default. MAE now sets WAL out of band at open (`wal::ensure_wal`), so on
    /// a filesystem that supports it this contention no longer arises — but the
    /// retry stays for the filesystems that do not, and for stores not yet
    /// reopened since. In rollback-journal
    /// mode a writer's exclusive lock blocks readers too, so an unprotected
    /// `run_immut` call CAN legitimately hit "database is locked" under
    /// real concurrent write contention — exactly what surfaced as a CI-only
    /// flake in `concurrent_first_time_sqlite_open_and_import_does_not_panic`
    /// (`ensure_instance_id`'s existence-check read, `schema.rs`, racing a
    /// different concurrent opener's `insert_node` write) even though the
    /// store-creation window itself is lock-protected — the race is between
    /// one opener's POST-open application writes and a DIFFERENT opener's
    /// still-in-progress `ensure_schema` reads, which the advisory lock
    /// deliberately does not (and should not) serialize against, since
    /// ordinary concurrent read/write after creation is supposed to be
    /// safe and usually is once contention clears within the retry budget.
    pub(super) fn run_immut(&self, script: &str) -> Result<NamedRows, cozo::Error> {
        self.run_with_busy_retry(|| {
            self.db
                .run_script(script, BTreeMap::new(), ScriptMutability::Immutable)
        })
    }
    /// Run an immutable CozoScript with parameters, retrying on BUSY/locked
    /// contention — see `run_immut`'s doc for why reads need this too.
    pub(super) fn run_immut_params(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, cozo::Error> {
        self.run_with_busy_retry(|| {
            self.db
                .run_script(script, params.clone(), ScriptMutability::Immutable)
        })
    }
    pub(super) fn now_epoch(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
    /// Get the next auto-increment ID for pending_updates.
    pub(super) fn next_pending_id(&self) -> Result<i64, KbStoreError> {
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
    /// Return (id, title) pairs for all nodes, optionally filtered by prefix.
    pub fn id_title_pairs(
        &self,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, KbStoreError> {
        // Bound, not interpolated. `p.replace('\'', "")` stripped single quotes
        // only, and silently CHANGED the caller's prefix while doing so -- an id
        // legitimately containing a quote would have matched the wrong set.
        // See `mae_kb::ident` for the identifier-position counterpart.
        let result = if let Some(p) = prefix {
            self.run_immut_params(
                "?[id, title] := *nodes{id, title}, title != '', starts_with(id, $prefix)",
                btree_params([("prefix", dv_str(p))]),
            )
        } else {
            self.run_immut("?[id, title] := *nodes{id, title}, title != ''")
        }
        .map_err(cozo_err)?;
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
        // Same binding as `id_title_pairs` above, for the same reason.
        let result = match (body_limit, prefix) {
            // No body needed — same shape as id_title_pairs.
            (0, Some(p)) => self.run_immut_params(
                "?[id, title, body] := *nodes{id, title}, title != '', starts_with(id, $prefix), body = ''",
                btree_params([("prefix", dv_str(p))]),
            ),
            (0, None) => {
                self.run_immut("?[id, title, body] := *nodes{id, title}, title != '', body = ''")
            }
            (_, Some(p)) => self.run_immut_params(
                "?[id, title, body] := *nodes{id, title, body}, title != '', starts_with(id, $prefix)",
                btree_params([("prefix", dv_str(p))]),
            ),
            (_, None) => self.run_immut("?[id, title, body] := *nodes{id, title, body}, title != ''"),
        }
        .map_err(cozo_err)?;
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
/// covering, not just the one first observed. See `schema.rs`'s
/// `open_with_engine` `@ai-caution` note for why this is needed at all (cozo
/// 0.7.6 never configures `busy_timeout`). Any OTHER panic message or `Err`
/// (a genuinely corrupt/inaccessible store, matching the sibling sled
/// `@ai-caution` in `schema.rs`) is NOT retried — returned on the first
/// occurrence, for the caller's existing error mapping to handle exactly as
/// if this wrapper weren't here.
///
/// Deliberately mirrors [`CozoKbStore::run_with_busy_retry`]'s
/// already-battle-tested backoff shape immediately above in this same file
/// (exponential cap with FULL jitter, not the two-instance-lockstep-prone
/// linear/no-jitter backoff an earlier draft of this function used) rather
/// than reinventing a worse one (principle #8) — the two can't literally
/// share code (that one only ever sees a `Result`, never a panic; this one
/// must handle both shapes from the same underlying condition) — but there
/// is no reason for this backoff's *quality* to regress from established
/// precedent just because the call shape differs. `run_with_busy_retry`'s
/// own doc comment explains why jitter specifically matters here: "Without
/// jitter, identical backoff keeps them in lockstep and they collide
/// forever."
///
/// Bounded by wall-clock time (issue #484), same reasoning and same fix as
/// `run_with_busy_retry`'s own doc comment: a fixed attempt count is an
/// indirect, hardware-dependent proxy for "how long can I wait," and this
/// function had the IDENTICAL `MAX_ATTEMPTS: u32 = 400` vulnerability its
/// sibling did, just never observed failing in CI yet — fixed here too for
/// consistency rather than leaving a second copy of the same latent bug
/// (principle #15: fix drift for the whole feature area, not just the one
/// symptom that happened to be reported first).
///
/// Originally lived in `schema.rs` (the only call site, `open_with_engine`,
/// was there too) — moved here (#535/#536-adjacent cleanup, schema.rs was
/// over the 800-line source-file ceiling) to sit alongside its sibling
/// `run_with_busy_retry`, the module that already owns "retry a cozo op on
/// SQLite BUSY contention" as a concern. `schema.rs`'s `open_with_engine`
/// calls this via `use super::db::retry_on_transient_sqlite_busy;`.
pub(crate) fn retry_on_transient_sqlite_busy<T, E: std::fmt::Display>(
    f: impl Fn() -> Result<T, E>,
) -> Result<T, E> {
    retry_on_transient_sqlite_busy_with_deadline(f, DEFAULT_BUSY_RETRY_DEADLINE)
}

/// Production default — matches [`CozoKbStore::run_with_busy_retry`]'s own
/// budget (principle #8: one tuned constant, not two that can drift apart).
/// That budget was raised from 20s to 45s after a real Windows CI miss (see
/// `run_with_busy_retry`'s doc comment above for the full story); this
/// constant was not updated to match at the time (cuttlefisch/mae#518 item
/// 5) — kept in sync here.
const DEFAULT_BUSY_RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

/// sled's exclusive-lock retry budget — deliberately much shorter than the
/// sqlite one above. See `transient_retry_budget` for why the two differ.
const SLED_LOCK_RETRY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

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
    /// How long a given transient error is worth retrying for.
    ///
    /// `None` = not transient, fail immediately.
    ///
    /// @ai-caution: [store-contention] sled was previously absent here, and the
    /// asymmetry was invisible: sqlite contention got the full 45s budget while
    /// sled's `could not acquire lock … WouldBlock … Resource temporarily
    /// unavailable` matched none of the sqlite wording, so it was classified
    /// permanent and failed on the FIRST attempt. That surfaced as a flaky
    /// test rather than a bug report — sled releases its exclusive directory
    /// lock during drop, and that release is not synchronous, so every
    /// sled->sqlite migration path (seed a store, drop it, reopen the same
    /// path: `migrate.rs`'s `seed_sled`, `migrate_sled_to_sqlite`,
    /// `kb_open_instance_store`, ~14 sites) loses the race on a loaded runner.
    /// `migrate_sled_to_sqlite` does the same reopen in PRODUCTION, so this was
    /// never test-only.
    ///
    /// sled gets a deliberately SHORTER budget than sqlite. sqlite's is a
    /// many-writer busy signal worth waiting out; sled's lock is *exclusive*,
    /// so a lock still held after a few seconds means another live handle
    /// exists — a real error the caller needs to see, not something to block on
    /// for 45s. Long enough to absorb a drop-release race, short enough that a
    /// genuinely-held lock still fails fast with its own clear message.
    fn transient_retry_budget(
        s: &str,
        deadline: std::time::Duration,
    ) -> Option<std::time::Duration> {
        let s = s.to_ascii_lowercase();
        if s.contains("database is locked") || s.contains("sqlite_busy") || s.contains("busy") {
            return Some(deadline);
        }
        // sled 0.34's exclusive-dir-lock contention, in the spellings it
        // actually produces (the io::Error kind, its message, and the errno
        // text) plus sled's own wording.
        if s.contains("wouldblock")
            || s.contains("would block")
            || s.contains("resource temporarily unavailable")
            || s.contains("could not acquire lock")
        {
            return Some(std::cmp::min(deadline, SLED_LOCK_RETRY_DEADLINE));
        }
        None
    }
    // Poor-man's per-call entropy (a stack address, like `run_with_busy_retry`'s
    // `self as *const Self as u64`) -- no need for a real RNG crate just to
    // desynchronize two competing retriers.
    let seed = &f as *const _ as u64;
    let start = std::time::Instant::now();
    let mut attempt: u32 = 0;
    loop {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&f));
        // The budget is per-ERROR, not per-call: sqlite contention is worth the
        // full deadline, sled's exclusive lock is not (see
        // `transient_retry_budget`). Recomputed each attempt because a retry can
        // legitimately surface a different error than the one that started it.
        let retry_budget = match &outcome {
            Ok(Err(e)) => transient_retry_budget(&e.to_string(), deadline),
            Err(payload) => payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .and_then(|m| transient_retry_budget(m, deadline)),
            Ok(Ok(_)) => None,
        };
        let expired = retry_budget.is_none_or(|budget| start.elapsed() >= budget);
        if expired {
            return match outcome {
                Ok(result) => result,
                Err(payload) => std::panic::resume_unwind(payload),
            };
        }
        attempt += 1;
        // Exponential cap (~0.25ms -> 8ms) with full jitter, same shape as
        // `run_with_busy_retry` above.
        let cap = (250u64 << attempt.min(5)).min(8_000);
        let jitter = seed
            .wrapping_mul(attempt as u64 + 1)
            .wrapping_add(attempt as u64)
            % (cap + 1);
        std::thread::sleep(std::time::Duration::from_micros(jitter));
    }
}

/// Parse an org `DEADLINE:`-style timestamp into epoch seconds at UTC midnight.
///
/// Accepts the shapes org actually writes -- `<2026-08-25 Tue>`, `[2026-08-25]`,
/// `<2026-08-25 Tue 14:30>` -- plus a bare `2026-08-25`, by locating the first
/// `YYYY-MM-DD` and ignoring the decoration. A time-of-day is deliberately
/// discarded: the column records a *day*, and pretending to more precision than
/// the field carries is worse than rounding to it.
///
/// **0 means "no deadline"**, which is also what the column held for every node
/// before this. Unparseable input is treated as absent rather than as an error:
/// a malformed timestamp in one node must not fail the write of the whole node.
///
/// Scope (#466): this reads the `:DEADLINE:` *property*. Org's planning LINES
/// (`DEADLINE: <...>` on the line after a heading) are a separate syntax with no
/// parser in this crate, and are follow-on work rather than cutover scope.
fn parse_org_due_date(raw: &str) -> i64 {
    let bytes = raw.as_bytes();
    // Scan for the first `dddd-dd-dd`.
    for start in 0..bytes.len().saturating_sub(9) {
        let window = &raw[start..start + 10.min(raw.len() - start)];
        if window.len() == 10 {
            if let Some((y, m, d)) = crate::activity::parse_date(window) {
                return crate::activity::epoch_seconds_utc_midnight(y, m, d);
            }
        }
    }
    0
}
