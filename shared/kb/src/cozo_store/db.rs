//! Low-level DB/query-execution helpers shared by every other cozo_store
//! submodule: script execution (mutable/immutable, with SQLite BUSY retry),
//! epoch time, node-row/param encoding, and bulk import.
//!
//! @ai-caution: [architecture-debt] These `pub(super)` helpers (run_mut*,
//! run_immut*, now_epoch, node_put_params, ...) are called from ~12 sites
//! across every sibling module in this directory. Keep signatures stable.

use super::util::{btree_params, cozo_err, dv_str, kind_to_str};
use super::*;

impl CozoKbStore {
    /// Upsert one row into `nodes`. Shared by `insert_node` (single) and
    /// `bulk_import` (many rows, one transaction). Touches NO links.
    pub(super) const NODE_PUT_SCRIPT: &'static str = r#"?[id, title, kind, body, tags_json, todo_state, priority, source, source_version,
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
    /// Positional column values for one `nodes` row, matching [`Self::NODE_BULK_SCRIPT`].
    fn node_row(&self, node: &Node, now: i64) -> Result<Vec<DataValue>, KbStoreError> {
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
            dv_str(""),            // origin_instance
            dv_str(""),            // assignee
            DataValue::from(0i64), // due_date
            dv_str(""),            // sprint
            DataValue::from(now),  // created_at
            DataValue::from(now),  // updated_at
        ])
    }
    /// Build the parameter map for [`Self::NODE_PUT_SCRIPT`] from a node.
    pub(super) fn node_put_params(
        &self,
        node: &Node,
    ) -> Result<BTreeMap<String, DataValue>, KbStoreError> {
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
    /// cozo 0.7's sqlite backend sets no `busy_timeout`, so a concurrent
    /// cross-process writer transiently fails with "database is locked" — an
    /// experiment showed ~14% raw write-failure under two-writer contention, and 0%
    /// with this backoff. Multi-instance daemon-less sharing depends on it. On the
    /// sled backend the predicate never matches, so this is a zero-cost pass-through.
    ///
    /// MAX_ATTEMPTS was raised from 100 to 400 after
    /// `sqlite_multi_instance_concurrent_writes_converge` (the adversarial test this
    /// backoff exists for) flaked on CI with a genuine "database is locked (code 5)"
    /// after exhausting retries — CI runners contend for disk/CPU more than the dev
    /// machine the original 14%/0% figures were measured on, so the retry budget
    /// needs headroom for slower/more-loaded hardware, not just a fast local box.
    /// Worst-case added latency stays bounded (~8ms/attempt cap × 400 ≈ 3.2s ceiling,
    /// only ever paid under sustained contention — a successful write still returns
    /// on the first attempt with zero added latency).
    fn run_with_busy_retry<F>(&self, mut op: F) -> Result<NamedRows, cozo::Error>
    where
        F: FnMut() -> Result<NamedRows, cozo::Error>,
    {
        const MAX_ATTEMPTS: u32 = 400;
        // Per-instance seed so two competing writers jitter differently. Without
        // jitter, identical backoff keeps them in lockstep and they collide forever.
        let seed = self as *const Self as u64;
        let mut attempt: u32 = 0;
        loop {
            match op() {
                Err(e) if attempt < MAX_ATTEMPTS && Self::is_busy(&e) => {
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
    fn is_busy(e: &cozo::Error) -> bool {
        let s = e.to_string().to_ascii_lowercase();
        s.contains("locked") || s.contains("busy") || s.contains("executing against relation")
    }
    /// Run an immutable CozoScript.
    pub(super) fn run_immut(&self, script: &str) -> Result<NamedRows, cozo::Error> {
        self.db
            .run_script(script, BTreeMap::new(), ScriptMutability::Immutable)
    }
    /// Run an immutable CozoScript with parameters.
    pub(super) fn run_immut_params(
        &self,
        script: &str,
        params: BTreeMap<String, DataValue>,
    ) -> Result<NamedRows, cozo::Error> {
        self.db
            .run_script(script, params, ScriptMutability::Immutable)
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
}
