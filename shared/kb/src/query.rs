//! KbQueryLayer — CozoDB-first query abstraction for knowledge base reads.
//!
//! All runtime KB reads go through `KbQueryLayer`. The trait has implementations
//! for `CozoKbStore` (direct Datalog queries), `FederatedQuery` (multi-store
//! fan-out), and `CachedQueryLayer` (LRU cache wrapper).
//!
//! ## Error contract (ADR-086 read-side twin)
//!
//! Every method whose data comes from a fallible backing store (Datalog query, daemon
//! RPC, filesystem) returns `Result<_, KbStoreError>`. This is deliberate: a storage
//! failure and a genuinely empty/healthy result MUST be distinguishable by callers,
//! all the way up to the MCP tool boundary an AI agent reads from. Before this
//! contract existed, `CozoQueryLayer` converted every `Err` from `CozoKbStore` into an
//! empty `Vec`/`None` via `unwrap_or_default()`/`.ok()` — a caller (and the AI agent
//! reading its output) could not tell "the KB has nothing matching this query" from
//! "the KB failed to answer this query," and would confidently report the wrong thing
//! ("there is nothing in the KB") when the truth was "the database errored." See
//! `docs/adr/086-tool-outcome-contract.md` for the write-side analogue this closes the
//! gap with.
//!
//! `get`/`contains` remain infallible (`Option`/`bool`): a single-node lookup already
//! treats "not found" and "lookup failed" as the same caller-visible outcome elsewhere
//! in this codebase (and `CozoQueryLayer::get` already logs failures via
//! `tracing::warn!`), and threading `Result` through `contains`'s many purely-internal
//! ownership-routing call sites (`FederatedQuery`'s `if inst.contains(id) { … }`
//! pattern) would not improve caller-visible error surfacing.
//!
//! Layers with no failure mode of their own (`InMemoryQueryLayer`, `CachedQueryLayer`'s
//! pass-through) simply return `Ok(...)`. `RemoteHubQueryLayer` keeps its existing,
//! separately-designed and separately-tested "timeout-and-continue" graceful
//! degradation contract (ADR-062 Phase E, surfaced via `degraded()`) rather than
//! turning every network hiccup into an `Err` — that would regress an already-correct,
//! deliberately different failure-handling strategy for an inherently flaky transport.

use crate::store::{HealthReport, KbStore, KbStoreError, Link, SearchHit, SubGraph};
use crate::{CozoKbStore, Node};
use std::sync::Arc;

/// Read-only query interface for knowledge base operations.
///
/// All runtime reads (help buffers, AI tools, search, link navigation)
/// go through this trait instead of the in-memory `KnowledgeBase`.
pub trait KbQueryLayer: Send + Sync {
    /// Get a node by ID.
    fn get(&self, id: &str) -> Option<Node>;

    /// Check if a node exists.
    fn contains(&self, id: &str) -> bool;

    /// Full-text search across node titles and bodies.
    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError>;

    /// Outgoing links from a node (typed, with rel_type).
    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError>;

    /// Incoming links to a node (typed, with rel_type).
    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError>;

    /// List all node IDs, optionally filtered by prefix.
    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError>;

    /// Return (id, title) pairs for all nodes, optionally filtered by prefix.
    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError>;

    /// Return (id, title, body) triples for all nodes.
    /// Body is truncated to `body_limit` chars (0 = no body).
    /// Default implementation calls `id_title_pairs` + `get` per node (slow).
    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Result<Vec<(String, String, String)>, KbStoreError> {
        Ok(self
            .id_title_pairs(prefix)?
            .into_iter()
            .map(|(id, title)| {
                let body = if body_limit > 0 {
                    self.get(&id)
                        .map(|n| n.body.chars().take(body_limit).collect())
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                (id, title, body)
            })
            .collect())
    }

    /// Compute a structured health report. `Ok(None)` means this backend has no
    /// concept of a health report (e.g. `InMemoryQueryLayer`); `Err` means the
    /// backing store failed to produce one.
    fn health_report(&self) -> Result<Option<HealthReport>, KbStoreError>;

    /// BFS neighborhood subgraph around a node. `Ok(None)` means the root wasn't
    /// found; `Err` means the backing store failed.
    fn neighborhood(&self, id: &str, depth: u32) -> Result<Option<SubGraph>, KbStoreError>;

    /// Graph-relatedness: `(id, score)` for nodes structurally related to
    /// `id` (co-citation / bibliographic coupling / shared tags), distinct
    /// from lexical `search`. Default returns empty so RPC/daemon layers that
    /// don't implement it degrade gracefully; `CozoQueryLayer` overrides.
    fn related(&self, _id: &str, _limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        Ok(Vec::new())
    }

    /// Full per-instance node-id -> incoming-link-count map (NOT truncated to any top-N —
    /// see CozoKbStore::compute_in_degree_map). Used by FederatedQuery::health_report to
    /// reconcile orphan/hub detection across instances without adding new query cost (issue
    /// #474) — this is the same data health_report's own hub computation already builds
    /// before truncating it away. Default empty, mirroring related()/neighborhood()'s
    /// existing graceful-degrade contract (e.g. RemoteHubQueryLayer, which already can't
    /// participate in health_report at all).
    fn linked_in_degree(&self) -> Result<std::collections::HashMap<String, usize>, KbStoreError> {
        Ok(std::collections::HashMap::new())
    }

    /// Evict cached entries for node `id` (Phase D3b). A no-op for layers without a
    /// cache (`CozoQueryLayer`, `FederatedQuery`); `LruQueryLayer` overrides it. The
    /// editor calls this when a KB node changes remotely (a `sync_update` from the
    /// daemon) so the next daemon-routed read returns fresh content, not a stale hit.
    fn invalidate(&self, _id: &str) {}

    /// Fetch a node's authoritative CRDT doc state from the daemon (Phase D3b), for
    /// lazy edit hydration on a thin client: the editor applies this to its in-memory
    /// mirror to obtain the node WITH its real lineage before editing. Default `None`
    /// (no daemon / non-RPC layers); `LruQueryLayer` overrides via `kb/node_crdt`.
    ///
    /// ADR-105: takes `kb_id` because a node document is addressed per-KB. Without it
    /// the daemon cannot name the document — and resolving the KB server-side from
    /// the node id alone is precisely the ambiguity this ADR removes.
    fn node_crdt_state(&self, _kb_id: &str, _id: &str) -> Option<Vec<u8>> {
        None
    }

    /// All nodes carrying a TODO state, for the agenda buffer (Phase D thin-client:
    /// the agenda was mirror-only). Default empty (non-cozo layers); `CozoQueryLayer`
    /// + `LruQueryLayer` implement it. The editor applies state/priority/tag filters.
    fn todo_nodes(&self) -> Result<Vec<Node>, KbStoreError> {
        Ok(Vec::new())
    }

    /// Agenda query (todo/priority/tag/orphan/stale/dead-end/custom) resolved via the
    /// store's Datalog. Phase 3: routes `:kb-agenda` uniformly through the query layer
    /// instead of reaching into the primary store directly. Default empty so non-cozo
    /// layers degrade gracefully; `CozoQueryLayer` delegates to the store.
    fn agenda(&self, _filter: &crate::AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        Ok(Vec::new())
    }

    /// Version history (snapshots) for a node, newest first. Phase 3: routes
    /// `:kb-history` through the query layer. Default empty; `CozoQueryLayer` delegates
    /// to the store.
    fn history(&self, _id: &str, _limit: usize) -> Result<Vec<crate::NodeVersion>, KbStoreError> {
        Ok(Vec::new())
    }

    /// Whether the MOST RECENT call on this layer was degraded (e.g. a `RemoteHubQueryLayer`
    /// whose last HTTP call timed out, hit an auth failure, or hit an unreachable host —
    /// ADR-062 Phase E). Default `false` for every layer with nothing to degrade (local
    /// Cozo reads don't have a network failure mode). `FederatedQuery` polls this after a
    /// fan-out round to set its own aggregate `last_query_was_partial()` flag — the
    /// "timeout-and-continue degradation contract" Phase E's Decision text calls for,
    /// without widening every `KbQueryLayer` method's return type to carry a `Result`.
    fn degraded(&self) -> bool {
        false
    }

    /// Return all known namespace prefixes (e.g., "cmd:", "concept:").
    fn namespace_prefixes(&self) -> Result<Vec<String>, KbStoreError> {
        let mut prefixes = std::collections::HashSet::new();
        for id in self.list_ids(None)? {
            if let Some(colon) = id.find(':') {
                prefixes.insert(format!("{}:", &id[..colon]));
            }
        }
        let mut result: Vec<String> = prefixes.into_iter().collect();
        result.sort();
        Ok(result)
    }
}

/// `KbQueryLayer` implementation backed by a `CozoKbStore`.
pub struct CozoQueryLayer {
    store: Arc<CozoKbStore>,
}

impl CozoQueryLayer {
    pub fn new(store: Arc<CozoKbStore>) -> Self {
        Self { store }
    }
}

impl KbQueryLayer for CozoQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        match self.store.get_node(id) {
            Ok(node) => node,
            Err(e) => {
                tracing::warn!(error = %e, id, "CozoQueryLayer::get failed");
                None
            }
        }
    }

    fn contains(&self, id: &str) -> bool {
        matches!(self.store.get_node_light(id), Ok(Some(_)))
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        self.store.fts_search(query, limit)
    }

    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        self.store.links_from(id)
    }

    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        self.store.links_to(id)
    }

    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        self.store.list_ids(prefix)
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        self.store.id_title_pairs(prefix)
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Result<Vec<(String, String, String)>, KbStoreError> {
        self.store.id_title_body_triples(prefix, body_limit)
    }

    fn health_report(&self) -> Result<Option<HealthReport>, KbStoreError> {
        self.store.health_report().map(Some)
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Result<Option<SubGraph>, KbStoreError> {
        self.store.neighborhood(id, depth).map(Some)
    }

    fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        self.store.related(id, limit)
    }

    fn linked_in_degree(&self) -> Result<std::collections::HashMap<String, usize>, KbStoreError> {
        self.store.compute_in_degree_map()
    }

    fn todo_nodes(&self) -> Result<Vec<Node>, KbStoreError> {
        self.store.agenda_query(&crate::AgendaFilter::Todo(None))
    }

    fn agenda(&self, filter: &crate::AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        self.store.agenda_query(filter)
    }

    fn history(&self, id: &str, limit: usize) -> Result<Vec<crate::NodeVersion>, KbStoreError> {
        self.store.node_history(id, limit)
    }
}

/// Default ceiling on how many federated instances participate in a single query's
/// fan-out (ADR-062 Phase B) when no caller-supplied override applies. Every
/// participating instance costs a full store query, so fan-out work scales with instance
/// count, not with the caller's `limit` — unlike the registry lookup itself (ADR-062
/// Phase A found that lookup to already be cheap at realistic scale; the org-roam-style
/// scaling risk this ADR is actually guarding against lives here, in per-query fan-out
/// cost). 128 comfortably covers "trusted-org scale" federation (see ADR-060's own
/// scoping language) while still bounding worst-case per-query cost for anyone who
/// registers far more instances than that.
const DEFAULT_MAX_FANOUT_INSTANCES: usize = 128;

/// Multi-store query layer that fans out reads across primary + instances.
/// Primary is checked first; search results are merged by score.
pub struct FederatedQuery {
    primary: Arc<dyn KbQueryLayer>,
    /// `(name, priority, layer)`. Priority (ADR-062 Phase B) decides which instance's
    /// copy wins when two instances' results collide on the same node id — see
    /// `priority_ordered_instances`. Higher priority wins; equal priority (the default —
    /// every instance is `priority: 0` unless a user raises it) falls back to
    /// registration order, matching pre-062 behavior exactly for anyone who never touches
    /// the new field.
    instances: Vec<(String, u32, Arc<dyn KbQueryLayer>)>,
    max_fanout_instances: usize,
    /// Whether the most recent `search()` call included a degraded source (ADR-062 Phase
    /// E's "timeout-and-continue" contract). Set fresh on every `search()` call — never
    /// sticky across calls, so a subsequent successful query correctly clears it (no
    /// stuck-degraded state).
    last_query_partial: std::sync::atomic::AtomicBool,
}

impl FederatedQuery {
    pub fn new(primary: Arc<dyn KbQueryLayer>) -> Self {
        Self {
            primary,
            instances: Vec::new(),
            max_fanout_instances: DEFAULT_MAX_FANOUT_INSTANCES,
            last_query_partial: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Whether the most recent `search()` call's result set is missing content from one
    /// or more degraded sources (a timed-out/unreachable/auth-failed `RemoteHub` instance
    /// — ADR-062 Phase E). Local-only federations (no `RemoteHub` instances registered)
    /// never set this; it exists specifically for the new failure mode a live network
    /// source introduces that a purely local fan-out never had.
    pub fn last_query_was_partial(&self) -> bool {
        self.last_query_partial
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn add_instance(&mut self, name: String, priority: u32, layer: Arc<dyn KbQueryLayer>) {
        self.instances.push((name, priority, layer));
    }

    /// Override the fan-out cap (ADR-062 Phase B). Callers with a config surface for it
    /// (the editor's `OptionRegistry`; the daemon may in future) can raise or lower this;
    /// unset, `DEFAULT_MAX_FANOUT_INSTANCES` applies.
    pub fn set_max_fanout_instances(&mut self, max: usize) {
        self.max_fanout_instances = max;
    }

    /// Instances ordered by priority descending, ties broken by registration order (a
    /// *stable* sort — `Vec::sort_by` never reorders equal elements — so two instances at
    /// the same priority always resolve the same way run over run, which is exactly what
    /// the "20 repeated identical queries must produce identical ordering" adversarial
    /// test (ADR-062 Phase B) requires), then truncated to `max_fanout_instances` — so if
    /// a registry genuinely exceeds the cap, it's always the *lowest*-priority instances
    /// that are dropped from a given query's fan-out, never an arbitrary subset. Ownership
    /// lookups (`get`/`links_from`/etc.) and merge-dedup lookups (`search`/`agenda`/etc.)
    /// both use this instead of raw registration order, so a higher-priority instance's
    /// copy of a node also wins whenever the same id exists in more than one federated
    /// instance (the org-roam #1480/#1496 duplicate-id failure class this ADR names).
    fn priority_ordered_instances(&self) -> Vec<&(String, u32, Arc<dyn KbQueryLayer>)> {
        let mut ordered: Vec<&(String, u32, Arc<dyn KbQueryLayer>)> =
            self.instances.iter().collect();
        ordered.sort_by_key(|(_, priority, _)| std::cmp::Reverse(*priority));
        if ordered.len() > self.max_fanout_instances {
            tracing::warn!(
                total = ordered.len(),
                cap = self.max_fanout_instances,
                "federated query fan-out capped; lowest-priority instances excluded from this query"
            );
            ordered.truncate(self.max_fanout_instances);
        }
        ordered
    }
}

impl KbQueryLayer for FederatedQuery {
    fn get(&self, id: &str) -> Option<Node> {
        if let Some(node) = self.primary.get(id) {
            return Some(node);
        }
        for (_, _, inst) in self.priority_ordered_instances() {
            if let Some(node) = inst.get(id) {
                return Some(node);
            }
        }
        None
    }

    fn contains(&self, id: &str) -> bool {
        self.primary.contains(id) || self.instances.iter().any(|(_, _, i)| i.contains(id))
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        // ADR-062 Phase E: every participating source (primary + each federated instance)
        // is queried on its OWN thread via `std::thread::scope`, not a sequential loop.
        // A sequential fan-out would let one slow/hung `RemoteHubQueryLayer` (bounded by
        // its own internal HTTP timeout, but still up to ~1.5s by default) serialize with
        // every other instance's latency, turning "the other 3 sources return within the
        // local-only latency budget" into a false promise — the whole call would wait for
        // the slow one regardless of where the fast local sources sit in iteration order.
        // Running each source's `search()` concurrently instead means total latency is
        // bounded by the SLOWEST single source, not the SUM of all of them — and that
        // slowest source is itself bounded by its own timeout, so the whole call has a
        // real, predictable worst case.
        // (priority, result, degraded) per participating instance.
        type InstanceSearchResult = (u32, Result<Vec<SearchHit>, KbStoreError>, bool);
        let ordered = self.priority_ordered_instances();
        let (primary_result, instance_results): (
            Result<Vec<SearchHit>, KbStoreError>,
            Vec<InstanceSearchResult>,
        ) = std::thread::scope(|scope| {
            let primary_handle = scope.spawn(|| self.primary.search(query, limit));
            let instance_handles: Vec<_> = ordered
                .iter()
                .map(|(_, priority, inst)| {
                    scope.spawn(move || {
                        let result = inst.search(query, limit);
                        let degraded = inst.degraded() || result.is_err();
                        (*priority, result, degraded)
                    })
                })
                .collect();
            let primary_result = primary_handle.join().unwrap_or_else(|_| {
                Err(KbStoreError::Storage(
                    "primary search worker thread panicked".to_string(),
                ))
            });
            let instance_results = instance_handles
                .into_iter()
                .map(|h| {
                    h.join().unwrap_or_else(|_| {
                        (
                            0,
                            Err(KbStoreError::Storage(
                                "federated instance search worker thread panicked".to_string(),
                            )),
                            true,
                        )
                    })
                })
                .collect();
            (primary_result, instance_results)
        });

        // A primary storage failure is not something a caller can safely fold into "no
        // results" — propagate it (ADR-086 read-side twin: never let a real storage error
        // render as an empty result set). A single federated *instance* failing, by
        // contrast, degrades the merge (that instance's contribution is missing, logged
        // below) rather than failing the whole call — the same "timeout-and-continue"
        // contract this type already applies to a slow/unreachable `RemoteHubQueryLayer`
        // (ADR-062 Phase E), just now also covering a local instance's genuine query error.
        let primary_hits = primary_result?;

        let mut any_degraded = false;
        let mut by_id: std::collections::HashMap<String, (i64, SearchHit)> =
            std::collections::HashMap::new();
        for hit in primary_hits {
            by_id.insert(hit.id.clone(), (i64::MAX, hit));
        }
        for (priority, result, degraded) in instance_results {
            any_degraded |= degraded;
            match result {
                Ok(hits) => {
                    for hit in hits {
                        let candidate_priority = priority as i64;
                        let replace = match by_id.get(&hit.id) {
                            Some((existing_priority, _)) => candidate_priority > *existing_priority,
                            None => true,
                        };
                        if replace {
                            by_id.insert(hit.id.clone(), (candidate_priority, hit));
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "federated search: instance query failed, excluded from merged results"
                    );
                }
            }
        }
        self.last_query_partial
            .store(any_degraded, std::sync::atomic::Ordering::Relaxed);

        let mut hits: Vec<SearchHit> = by_id.into_values().map(|(_, hit)| hit).collect();
        // Deterministic final order: score descending, id ascending as an explicit
        // tiebreak. `by_id` is a `HashMap`, whose iteration order is process-randomized —
        // without this explicit secondary key, two hits tied on score could come out in a
        // different order on different runs even for byte-identical input (the exact
        // nondeterminism the ADR-062 Phase B adversarial test — 20 repeated identical
        // queries must produce identical ordering — exists to catch). Thread completion
        // order (now that fan-out is concurrent) is an ADDITIONAL source of nondeterminism
        // this same explicit sort neutralizes — it was already required before
        // parallelizing, and remains sufficient after.
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.id.cmp(&b.id))
        });
        hits.truncate(limit);
        Ok(hits)
    }

    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        // Merge outgoing links from all stores — symmetric with `links_to`
        // below, and for the same reason it gives: an instance's edges are
        // real, distinct edges, not competing copies of one fact.
        //
        // This used to return early from whichever store "owned" the node
        // (issue #698), which made the two directions disagree: an edge
        // recorded in instance B about a node owned by the primary showed up
        // in `links_to(dst)` but never in `links_from(src)`. Any forward-BFS
        // consumer — `kb_graph`, `kb_neighborhood`, `kb_shortest_path`,
        // `related_enriched` — therefore walked a strictly smaller graph than
        // the reverse walk reported, with no error and no way to notice.
        //
        // Failure posture matches `links_to` exactly: primary propagates (it
        // is the caller's authoritative baseline), a sibling instance's
        // failure is logged and excluded rather than failing the whole query.
        let mut links = self.primary.links_from(id)?;
        for (name, _, inst) in &self.instances {
            match inst.links_from(id) {
                Ok(more) => links.extend(more),
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    "links_from failed for a federated instance; excluded from the merge"
                ),
            }
        }
        // Unlike `links_to`, merging outgoing edges CAN produce true duplicates:
        // a federated copy of the same node carries its own copy of the same
        // outgoing edge, and that is one fact, not two. Dedup on the identity
        // triple only — weight/confidence/display may legitimately differ
        // between copies, and first-wins keeps the primary's version.
        let mut seen = std::collections::HashSet::new();
        links.retain(|l| seen.insert((l.src.clone(), l.dst.clone(), l.rel_type.clone())));
        Ok(links)
    }

    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        // Merge incoming links from all stores — every instance's incoming edges are real,
        // distinct edges (not competing copies of the same fact), so there's nothing to
        // dedup-by-priority here; this deliberately stays priority-agnostic. Primary
        // failure propagates (it's the caller's authoritative baseline); a sibling
        // instance's failure is logged and excluded from the merge, matching `search`'s
        // graceful-degradation posture for non-primary sources.
        let mut links = self.primary.links_to(id)?;
        for (name, _, inst) in &self.instances {
            match inst.links_to(id) {
                Ok(more) => links.extend(more),
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    id,
                    "federated links_to: instance query failed, excluded from merged results"
                ),
            }
        }
        Ok(links)
    }

    fn agenda(&self, filter: &crate::AgendaFilter) -> Result<Vec<Node>, KbStoreError> {
        // Merge agenda matches across primary + instances, de-duped by id. Priority
        // decides which instance's copy of a colliding node is kept — same rule as
        // `search`, without imposing `search`'s score-based reordering (agenda order is
        // meaningful on its own and untouched here). Primary failure propagates; a
        // sibling instance's failure is logged and excluded.
        let mut nodes = self.primary.agenda(filter)?;
        let mut seen: std::collections::HashSet<String> =
            nodes.iter().map(|n| n.id.clone()).collect();
        for (name, _, inst) in self.priority_ordered_instances() {
            match inst.agenda(filter) {
                Ok(inst_nodes) => {
                    for n in inst_nodes {
                        if seen.insert(n.id.clone()) {
                            nodes.push(n);
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    "federated agenda: instance query failed, excluded from merged results"
                ),
            }
        }
        Ok(nodes)
    }

    fn history(&self, id: &str, limit: usize) -> Result<Vec<crate::NodeVersion>, KbStoreError> {
        // History lives in whichever store owns the node.
        if self.primary.contains(id) {
            return self.primary.history(id, limit);
        }
        for (_, _, inst) in self.priority_ordered_instances() {
            if inst.contains(id) {
                return inst.history(id, limit);
            }
        }
        Ok(Vec::new())
    }

    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        let mut ids = self.primary.list_ids(prefix)?;
        let mut seen: std::collections::HashSet<String> = ids.iter().cloned().collect();
        for (name, _, inst) in self.priority_ordered_instances() {
            match inst.list_ids(prefix) {
                Ok(inst_ids) => {
                    for id in inst_ids {
                        if seen.insert(id.clone()) {
                            ids.push(id);
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    "federated list_ids: instance query failed, excluded from merged results"
                ),
            }
        }
        Ok(ids)
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        let mut pairs = self.primary.id_title_pairs(prefix)?;
        let mut seen: std::collections::HashSet<String> =
            pairs.iter().map(|(id, _)| id.clone()).collect();
        for (name, _, inst) in self.priority_ordered_instances() {
            match inst.id_title_pairs(prefix) {
                Ok(inst_pairs) => {
                    for pair in inst_pairs {
                        if seen.insert(pair.0.clone()) {
                            pairs.push(pair);
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    "federated id_title_pairs: instance query failed, excluded from merged results"
                ),
            }
        }
        Ok(pairs)
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Result<Vec<(String, String, String)>, KbStoreError> {
        let mut triples = self.primary.id_title_body_triples(prefix, body_limit)?;
        let mut seen: std::collections::HashSet<String> =
            triples.iter().map(|(id, _, _)| id.clone()).collect();
        for (name, _, inst) in self.priority_ordered_instances() {
            match inst.id_title_body_triples(prefix, body_limit) {
                Ok(inst_triples) => {
                    for triple in inst_triples {
                        if seen.insert(triple.0.clone()) {
                            triples.push(triple);
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    "federated id_title_body_triples: instance query failed, excluded from merged results"
                ),
            }
        }
        Ok(triples)
    }

    fn health_report(&self) -> Result<Option<HealthReport>, KbStoreError> {
        // Mirrors `id_title_body_triples`'s aggregation pattern above: start from the
        // primary's report and merge every registered instance's report into it, rather
        // than silently returning only the primary (ADR-065 item 1). Unlike a plain
        // merge, each instance's contribution is also recorded in `by_instance` so a
        // corrupted/unreachable instance shows up as `reachable: false` instead of being
        // indistinguishable from "this instance is empty and healthy".
        //
        // Issue #474 (ADR-065 addendum): each instance's own orphan/broken-link/hub
        // detection is scoped ENTIRELY to that instance's own `*nodes`/`*links` — no
        // cross-engine Datalog query is possible (cozo 0.7.6 has no attach/union
        // primitive; each registered instance is a fully separate embedded engine). A
        // link whose target is a real node in a DIFFERENT instance was therefore wrongly
        // reported broken, and a node whose only real incoming link comes from a sibling
        // instance was wrongly reported orphaned. Fixed below by reconciling against a
        // federation-wide in-degree map + `contains` check, without adding
        // O(candidates × instances × total_links) cost (principle #9) — see the comments
        // at each fix site.
        let Some(mut merged) = self.primary.health_report()? else {
            return Ok(None);
        };
        let mut by_instance: std::collections::HashMap<String, crate::store::InstanceHealth> =
            std::collections::HashMap::new();
        by_instance.insert(
            "primary".to_string(),
            crate::store::InstanceHealth {
                reachable: true,
                total_nodes: merged.total_nodes,
                orphan_count: merged.orphan_ids.len(),
                broken_link_count: merged.broken_links.len(),
            },
        );

        // Federation-wide node-id -> incoming-link-count, summed across primary + every
        // participating instance (issue #474). Reuses `linked_in_degree` — the SAME
        // Datalog query `health_report`'s own hub computation already runs per instance
        // (CLAUDE.md principle #8: no new query cost, just no longer throwing the full
        // map away after truncating it to top-10). Built once, O(instances) calls each
        // O(nodes-with-incoming-links) — not multiplied by candidate count.
        let mut global_in_degree: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for (id, count) in self.primary.linked_in_degree()? {
            *global_in_degree.entry(id).or_default() += count;
        }

        // `priority_ordered_instances()` (respects `max_fanout_instances`), not the raw
        // `&self.instances` this loop used before — every OTHER aggregation method on this
        // type (search/agenda/list_ids/id_title_pairs/id_title_body_triples/todo_nodes)
        // already iterates instances this way; `health_report` iterating `&self.instances`
        // directly was an inconsistency (and meant health_report alone ignored the fan-out
        // cap) fixed here alongside the orphan/hub/broken-link reconciliation, not treated
        // as a separate PR.
        for (name, _, inst) in self.priority_ordered_instances() {
            match inst.health_report() {
                Ok(Some(report)) => {
                    by_instance.insert(
                        name.clone(),
                        crate::store::InstanceHealth {
                            reachable: true,
                            total_nodes: report.total_nodes,
                            orphan_count: report.orphan_ids.len(),
                            broken_link_count: report.broken_links.len(),
                        },
                    );
                    merged.total_nodes += report.total_nodes;
                    merged.total_links += report.total_links;
                    for (k, v) in report.namespace_counts {
                        *merged.namespace_counts.entry(k).or_default() += v;
                    }
                    for (k, v) in report.by_kind {
                        *merged.by_kind.entry(k).or_default() += v;
                    }
                    for (k, v) in report.by_rel_type {
                        *merged.by_rel_type.entry(k).or_default() += v;
                    }
                    merged.orphan_ids.extend(report.orphan_ids);
                    merged.broken_links.extend(report.broken_links);
                    // hub_nodes deliberately NOT extended here — see the fresh rebuild
                    // from `global_in_degree` below.
                }
                Ok(None) => {
                    // This backend has no health-report concept (e.g. an in-memory
                    // federated instance) — nothing to contribute, recorded the same way
                    // a genuine failure is below (both mean "did not contribute", the
                    // distinction between the two is now visible in the log for the
                    // Err case, which the pre-fix code could never emit at all).
                    by_instance.insert(
                        name.clone(),
                        crate::store::InstanceHealth {
                            reachable: false,
                            total_nodes: 0,
                            orphan_count: 0,
                            broken_link_count: 0,
                        },
                    );
                }
                Err(e) => {
                    // Instance failed to report — surface it as unreachable rather than
                    // silently dropping it from the aggregate (the exact bug this fix
                    // closes: a corrupted non-primary instance must not vanish), AND now
                    // log the real cause instead of discarding it (pre-fix, `.ok()`
                    // erased it entirely — there was no trace of *why* an instance was
                    // unreachable).
                    tracing::warn!(
                        error = %e,
                        instance = %name,
                        "federated health_report: instance query failed"
                    );
                    by_instance.insert(
                        name.clone(),
                        crate::store::InstanceHealth {
                            reachable: false,
                            total_nodes: 0,
                            orphan_count: 0,
                            broken_link_count: 0,
                        },
                    );
                }
            }
            match inst.linked_in_degree() {
                Ok(map) => {
                    for (id, count) in map {
                        *global_in_degree.entry(id).or_default() += count;
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    "federated health_report: instance linked_in_degree failed"
                ),
            }
        }

        // Orphan fix (issue #474): only the incoming-link half of each instance's local
        // orphan check needs federation-wide reconciliation. A node's own OUTGOING links
        // can only ever be recorded in its own owning instance — they're extracted from
        // that node's own content at ingest time, never from a sibling instance's data —
        // so each instance's local "no outgoing links" half is already trustworthy as-is.
        // The "no incoming links" half is the one that can be a false positive: the real
        // inbound link may live in a sibling instance's `*links` relation, invisible to
        // this instance's own Datalog query. `global_in_degree` is exactly the
        // federation-wide answer to "does ANY instance record an incoming link to this
        // id" — an id with an entry there has a real incoming link somewhere, so it's not
        // actually an orphan.
        merged
            .orphan_ids
            .retain(|id| !global_in_degree.contains_key(id));

        // Broken-link fix (issue #474): a link is only genuinely broken if its target
        // resolves NOWHERE in the federation, not just in its own owning instance.
        // `self.contains` already dispatches across primary + every instance and is a
        // cheap keyed lookup per store (`nodes` keyed by id) — O(candidates × instances),
        // not O(candidates × instances × total_links).
        merged
            .broken_links
            .retain(|link| !self.contains(&link.target));

        // Hub-node fix (issue #474): rebuild the top-10 fresh from the summed
        // federation-wide in-degree map, instead of re-sorting/truncating the union of
        // already-locally-truncated per-instance top-10 lists. That naive approach can
        // silently drop a node whose COMBINED global rank qualifies for the federated
        // top-10 but whose per-instance share never made any single instance's own local
        // top-10 ranking (see
        // `federated_health_report_hub_node_recovers_node_absent_from_every_instances_own_local_top_10`).
        // Single-instance case (`self.instances` empty): `global_in_degree` is exactly
        // primary's own full in-degree map (nothing summed in), so this reproduces
        // byte-identical top-10 output to before this fix (see
        // `federated_health_report_single_instance_unchanged_by_hub_fix`).
        let mut hubs: Vec<(String, usize)> = global_in_degree.into_iter().collect();
        hubs.sort_by_key(|h| std::cmp::Reverse(h.1));
        hubs.truncate(10);
        merged.hub_nodes = hubs;

        merged.by_instance = by_instance;
        Ok(Some(merged))
    }

    fn neighborhood(&self, id: &str, depth: u32) -> Result<Option<SubGraph>, KbStoreError> {
        if self.primary.contains(id) {
            return self.primary.neighborhood(id, depth);
        }
        for (_, _, inst) in self.priority_ordered_instances() {
            if inst.contains(id) {
                return inst.neighborhood(id, depth);
            }
        }
        Ok(None)
    }

    fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        // Per-instance, like `neighborhood`: relatedness is computed within the instance
        // that owns the node. KNOWN APPROXIMATION (issue #474 review): cross-instance links
        // ARE real — a node's true structural neighbors can live in a sibling federated
        // instance (see the `CrossInstanceLink`/`partition_boundary_links_by_instance` work
        // for issue #462's multi-KB chord graph view, which surfaces exactly these edges for
        // display) — so this stays scoped to the owning instance's own graph and undercounts
        // relatedness whenever a node's real co-citation/tag-sharing neighbors sit in another
        // instance. A full fix would blend the owning instance's own relatedness score with a
        // cross-instance signal; that's real design work (weighting, dedup, cost of fanning
        // out a `related()` call to every instance for every candidate), not attempted here —
        // out of scope for this change, unlike `health_report`'s orphan/hub/broken-link
        // reconciliation, which this change does fix.
        if self.primary.contains(id) {
            return self.primary.related(id, limit);
        }
        for (_, _, inst) in self.priority_ordered_instances() {
            if inst.contains(id) {
                return inst.related(id, limit);
            }
        }
        Ok(Vec::new())
    }

    /// Scoped specifically to the last `search()` call — `last_query_partial` is only ever
    /// set inside `search()` on this type, not by any other method (`health_report`,
    /// `links_to`, etc. never touch it). Do not read this as "was the last call of any kind
    /// degraded."
    fn degraded(&self) -> bool {
        self.last_query_was_partial()
    }

    fn todo_nodes(&self) -> Result<Vec<Node>, KbStoreError> {
        // Primary failure propagates; a sibling instance's failure is logged and
        // excluded, matching every other aggregation method above.
        let mut out = self.primary.todo_nodes()?;
        let mut seen: std::collections::HashSet<String> =
            out.iter().map(|n| n.id.clone()).collect();
        for (name, _, inst) in self.priority_ordered_instances() {
            match inst.todo_nodes() {
                Ok(inst_nodes) => {
                    for n in inst_nodes {
                        if seen.insert(n.id.clone()) {
                            out.push(n);
                        }
                    }
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    instance = %name,
                    "federated todo_nodes: instance query failed, excluded from merged results"
                ),
            }
        }
        Ok(out)
    }
}

/// Fallback query layer wrapping an in-memory `KnowledgeBase`.
/// Used when no CozoDB store is available.
pub struct InMemoryQueryLayer {
    kb: std::sync::Mutex<crate::KnowledgeBase>,
}

impl InMemoryQueryLayer {
    pub fn new(kb: crate::KnowledgeBase) -> Self {
        Self {
            kb: std::sync::Mutex::new(kb),
        }
    }

    /// Get a mutable reference to the underlying KB (for inserts/updates).
    pub fn kb_mut(&self) -> std::sync::MutexGuard<'_, crate::KnowledgeBase> {
        self.kb.lock().unwrap()
    }
}

impl KbQueryLayer for InMemoryQueryLayer {
    fn get(&self, id: &str) -> Option<Node> {
        self.kb.lock().unwrap().get(id).cloned()
    }

    fn contains(&self, id: &str) -> bool {
        self.kb.lock().unwrap().contains(id)
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
        let kb = self.kb.lock().unwrap();
        Ok(kb
            .search(query)
            .into_iter()
            .take(limit)
            .map(|id| SearchHit { id, score: 1.0 })
            .collect())
    }

    fn links_from(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let kb = self.kb.lock().unwrap();
        Ok(kb
            .links_from(id)
            .into_iter()
            .map(|dst| Link {
                src: id.to_string(),
                dst,
                rel_type: "references".to_string(),
                display: None,
                weight: 1.0,
                confidence: 1.0,
            })
            .collect())
    }

    fn links_to(&self, id: &str) -> Result<Vec<Link>, KbStoreError> {
        let kb = self.kb.lock().unwrap();
        Ok(kb
            .links_to(id)
            .into_iter()
            .map(|src| Link {
                src,
                dst: id.to_string(),
                rel_type: "references".to_string(),
                display: None,
                weight: 1.0,
                confidence: 1.0,
            })
            .collect())
    }

    fn list_ids(&self, prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
        let kb = self.kb.lock().unwrap();
        Ok(kb.list_ids(prefix))
    }

    fn id_title_pairs(&self, prefix: Option<&str>) -> Result<Vec<(String, String)>, KbStoreError> {
        let kb = self.kb.lock().unwrap();
        Ok(kb
            .list_ids(prefix)
            .into_iter()
            .filter_map(|id| {
                let title = kb.get(&id)?.title.clone();
                Some((id, title))
            })
            .collect())
    }

    fn id_title_body_triples(
        &self,
        prefix: Option<&str>,
        body_limit: usize,
    ) -> Result<Vec<(String, String, String)>, KbStoreError> {
        let kb = self.kb.lock().unwrap();
        Ok(kb
            .list_ids(prefix)
            .into_iter()
            .filter_map(|id| {
                let node = kb.get(&id)?;
                let body = if body_limit > 0 {
                    node.body.chars().take(body_limit).collect()
                } else {
                    String::new()
                };
                Some((id, node.title.clone(), body))
            })
            .collect())
    }

    fn health_report(&self) -> Result<Option<HealthReport>, KbStoreError> {
        Ok(None) // In-memory KB uses KbHealthReport, not store::HealthReport
    }

    fn neighborhood(&self, _id: &str, _depth: u32) -> Result<Option<SubGraph>, KbStoreError> {
        Ok(None)
    }

    fn related(&self, id: &str, limit: usize) -> Result<Vec<(String, f64)>, KbStoreError> {
        Ok(self.kb.lock().unwrap().related(id, limit))
    }

    fn linked_in_degree(&self) -> Result<std::collections::HashMap<String, usize>, KbStoreError> {
        // `KnowledgeBase` already maintains a `links_in` reverse index for `links_to` —
        // a real, correct implementation costs nothing beyond what's already tracked
        // (unlike `RemoteHubQueryLayer`, which has no such structure and falls through to
        // the trait default).
        Ok(self.kb.lock().unwrap().linked_in_degree())
    }

    fn todo_nodes(&self) -> Result<Vec<Node>, KbStoreError> {
        Ok(self
            .kb
            .lock()
            .unwrap()
            .todo_nodes()
            .into_iter()
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, NodeKind};

    #[test]
    fn cozo_query_layer_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("test.cozo")).unwrap());
        store
            .insert_node(&Node::new("test:a", "Alpha", NodeKind::Note, "body text"))
            .unwrap();

        let layer = CozoQueryLayer::new(store);
        assert!(layer.contains("test:a"));
        assert!(!layer.contains("test:b"));

        let node = layer.get("test:a").unwrap();
        assert_eq!(node.title, "Alpha");

        let ids = layer.list_ids(Some("test:")).unwrap();
        assert!(ids.contains(&"test:a".to_string()));

        let pairs = layer.id_title_pairs(None).unwrap();
        assert!(pairs.iter().any(|(id, _)| id == "test:a"));
    }

    #[test]
    fn federated_query_primary_first() {
        let tmp = tempfile::tempdir().unwrap();
        let store1 = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        let store2 = Arc::new(CozoKbStore::open(tmp.path().join("inst.cozo")).unwrap());

        store1
            .insert_node(&Node::new("shared", "Primary Version", NodeKind::Note, ""))
            .unwrap();
        store2
            .insert_node(&Node::new("shared", "Instance Version", NodeKind::Note, ""))
            .unwrap();
        store2
            .insert_node(&Node::new("only:inst", "Instance Only", NodeKind::Note, ""))
            .unwrap();

        let primary = Arc::new(CozoQueryLayer::new(store1));
        let inst = Arc::new(CozoQueryLayer::new(store2));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("test".into(), 0, inst);

        // Primary wins for shared IDs
        let node = federated.get("shared").unwrap();
        assert_eq!(node.title, "Primary Version");

        // Instance-only nodes are found
        assert!(federated.contains("only:inst"));
        let node = federated.get("only:inst").unwrap();
        assert_eq!(node.title, "Instance Only");
    }

    /// Issue #698: the two link directions must describe the **same graph**.
    ///
    /// `links_from` used to return early from whichever store "owned" the node,
    /// while `links_to` merged across all of them. So an edge recorded in a
    /// federated instance, about a node the primary owns, appeared in
    /// `links_to(dst)` and never in `links_from(src)` — and every forward-BFS
    /// consumer (`kb_graph`, `kb_neighborhood`, `kb_shortest_path`,
    /// `related_enriched`) silently walked a smaller graph than the reverse
    /// walk reported.
    ///
    /// Written as a round-trip property rather than a fixed expectation: for
    /// every edge either direction reports, the other must report it too.
    #[test]
    fn links_from_and_links_to_describe_the_same_edges() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Arc::new(CozoKbStore::open(tmp.path().join("p.cozo")).unwrap());
        let i = Arc::new(CozoKbStore::open(tmp.path().join("i.cozo")).unwrap());

        // The node is owned by the PRIMARY — which is what made the old
        // ownership routing return the primary's links and stop.
        p.insert_node(&Node::new("concept:tide", "Tide", NodeKind::Note, ""))
            .unwrap();
        p.insert_node(&Node::new("concept:moon", "Moon", NodeKind::Note, ""))
            .unwrap();
        p.add_link("concept:tide", "concept:moon", None).unwrap();

        // ...while a FEDERATED instance records a further outgoing edge about
        // that same node. Both are real edges; neither is a copy of the other.
        i.insert_node(&Node::new("concept:tide", "Tide", NodeKind::Note, ""))
            .unwrap();
        i.insert_node(&Node::new("note:survey", "Survey", NodeKind::Note, ""))
            .unwrap();
        i.add_link("concept:tide", "note:survey", None).unwrap();

        let mut fed = FederatedQuery::new(Arc::new(CozoQueryLayer::new(p)));
        fed.add_instance("field".into(), 0, Arc::new(CozoQueryLayer::new(i)));

        let from: std::collections::HashSet<(String, String)> = fed
            .links_from("concept:tide")
            .unwrap()
            .into_iter()
            .map(|l| (l.src, l.dst))
            .collect();

        // The instance-recorded edge is the one that used to vanish.
        assert!(
            from.contains(&("concept:tide".into(), "note:survey".into())),
            "an edge recorded in a federated instance about a primary-owned node \
             must still be reachable going forward; got {from:?}"
        );
        assert!(
            from.contains(&("concept:tide".into(), "concept:moon".into())),
            "the primary's own outgoing edge must survive the merge; got {from:?}"
        );

        // The symmetry property: every forward edge is reported backward too.
        for (src, dst) in &from {
            let back: std::collections::HashSet<(String, String)> = fed
                .links_to(dst)
                .unwrap()
                .into_iter()
                .map(|l| (l.src, l.dst))
                .collect();
            assert!(
                back.contains(&(src.clone(), dst.clone())),
                "{src} -> {dst} is reported by links_from but not by links_to({dst})"
            );
        }
    }

    /// The dedup half of the same change, and the reason `links_from` cannot
    /// simply concatenate the way `links_to` does: a federated **copy** of a
    /// node carries its own copy of the same outgoing edge. That is one fact,
    /// not two, and a naive merge would report it twice.
    #[test]
    fn a_federated_copy_of_the_same_edge_is_reported_once() {
        let tmp = tempfile::tempdir().unwrap();
        let p = Arc::new(CozoKbStore::open(tmp.path().join("p.cozo")).unwrap());
        let i = Arc::new(CozoKbStore::open(tmp.path().join("i.cozo")).unwrap());

        for s in [&p, &i] {
            s.insert_node(&Node::new("concept:tide", "Tide", NodeKind::Note, ""))
                .unwrap();
            s.insert_node(&Node::new("concept:moon", "Moon", NodeKind::Note, ""))
                .unwrap();
            s.add_link("concept:tide", "concept:moon", None).unwrap();
        }

        let mut fed = FederatedQuery::new(Arc::new(CozoQueryLayer::new(p)));
        fed.add_instance("mirror".into(), 0, Arc::new(CozoQueryLayer::new(i)));

        let links = fed.links_from("concept:tide").unwrap();
        assert_eq!(
            links.len(),
            1,
            "the same edge held by two stores is one edge, got {links:?}"
        );
    }

    /// ADR-062 Phase B: two *non-primary* instances that happen to register the same node
    /// id (the org-roam #1480/#1496 duplicate-id failure class — an uncoordinated,
    /// independently-populated pair of federated instances, not a hand-picked convenient
    /// case) must resolve the collision by priority, not by registration order. Proven
    /// both ways (A-then-B and B-then-A registration) so the result is provably
    /// priority-driven, not an accident of iteration order that priority happens to agree
    /// with in only one direction.
    #[test]
    fn federated_query_priority_decides_colliding_instance_ids_regardless_of_registration_order() {
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        let store_a = Arc::new(CozoKbStore::open(tmp.path().join("a.cozo")).unwrap());
        let store_b = Arc::new(CozoKbStore::open(tmp.path().join("b.cozo")).unwrap());
        store_a
            .insert_node(&Node::new(
                "dup:node",
                "From A (high priority)",
                NodeKind::Note,
                "",
            ))
            .unwrap();
        store_b
            .insert_node(&Node::new(
                "dup:node",
                "From B (low priority)",
                NodeKind::Note,
                "",
            ))
            .unwrap();

        // Order 1: A registered first.
        let primary = Arc::new(CozoQueryLayer::new(Arc::clone(&primary_store)));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance(
            "a".into(),
            5,
            Arc::new(CozoQueryLayer::new(Arc::clone(&store_a))),
        );
        federated.add_instance(
            "b".into(),
            1,
            Arc::new(CozoQueryLayer::new(Arc::clone(&store_b))),
        );
        assert_eq!(
            federated.get("dup:node").unwrap().title,
            "From A (high priority)"
        );

        // Order 2: B registered first — same outcome must hold; if it flipped, the
        // collision was being resolved by iteration order, not priority.
        let primary2 = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated2 = FederatedQuery::new(primary2);
        federated2.add_instance("b".into(), 1, Arc::new(CozoQueryLayer::new(store_b)));
        federated2.add_instance("a".into(), 5, Arc::new(CozoQueryLayer::new(store_a)));
        assert_eq!(
            federated2.get("dup:node").unwrap().title,
            "From A (high priority)"
        );
    }

    /// ADR-062 Phase B adversarial verification (the ADR's own named test): 20 repeated
    /// identical queries against a fixed fixture must produce byte-identical ordering
    /// every time. Uses several instances with genuinely tied scores (identical title +
    /// body content, so CozoDB's own BM25-style ranking has no basis to distinguish them)
    /// specifically to stress the tie-breaking path — a `HashMap`-based merge (Rust's
    /// `HashMap` iterates in a process-randomized order) without an explicit deterministic
    /// tiebreak would be the exact bug this test exists to catch, and it wouldn't
    /// necessarily show up on a single run, only across repeats.
    #[test]
    fn federated_search_ordering_is_stable_across_twenty_repeated_identical_queries() {
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .insert_node(&Node::new(
                "tie:primary",
                "Widget Report",
                NodeKind::Note,
                "widget widget widget",
            ))
            .unwrap();
        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);

        for i in 0..5 {
            let store =
                Arc::new(CozoKbStore::open(tmp.path().join(format!("tie-{i}.cozo"))).unwrap());
            store
                .insert_node(&Node::new(
                    format!("tie:inst-{i}"),
                    "Widget Report",
                    NodeKind::Note,
                    "widget widget widget",
                ))
                .unwrap();
            federated.add_instance(format!("inst-{i}"), 0, Arc::new(CozoQueryLayer::new(store)));
        }

        let baseline = federated.search("widget", 10).unwrap();
        assert!(
            baseline.len() >= 3,
            "fixture sanity: expected several tied-score hits, got {baseline:?}"
        );
        for _ in 0..20 {
            let repeat = federated.search("widget", 10).unwrap();
            assert_eq!(
                repeat, baseline,
                "search() must return byte-identical ordering across repeated identical \
                 queries — nondeterministic tie-breaking regressed"
            );
        }
    }

    /// ADR-062 Phase B: when a registry exceeds `max_fanout_instances`, the excluded
    /// instances must always be the *lowest*-priority ones, never an arbitrary subset
    /// (e.g. one determined by `HashMap`/registration-order accident). Registers 5
    /// instances with a cap of 3 and distinct, non-monotonic-with-registration-order
    /// priorities, then asserts exactly the 3 highest-priority instances' nodes are
    /// reachable and the 2 lowest-priority instances' nodes are not.
    #[test]
    fn federated_query_fanout_cap_excludes_only_the_lowest_priority_instances() {
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.set_max_fanout_instances(3);

        // (name, priority) — deliberately registered in an order that doesn't match
        // priority order, so a bug that used registration order instead of priority
        // would be caught.
        let specs = [("c", 10), ("a", 50), ("e", 1), ("b", 40), ("d", 5)];
        for (name, priority) in specs {
            let store =
                Arc::new(CozoKbStore::open(tmp.path().join(format!("{name}.cozo"))).unwrap());
            store
                .insert_node(&Node::new(
                    format!("only:{name}"),
                    format!("Node {name}"),
                    NodeKind::Note,
                    "",
                ))
                .unwrap();
            federated.add_instance(
                name.to_string(),
                priority,
                Arc::new(CozoQueryLayer::new(store)),
            );
        }

        // Top 3 by priority: a(50), b(40), c(10).
        assert!(
            federated.contains("only:a"),
            "highest-priority instance must survive the cap"
        );
        assert!(federated.get("only:a").is_some());
        assert!(federated.contains("only:b"));
        assert!(federated.get("only:b").is_some());
        assert!(federated.contains("only:c"));
        assert!(federated.get("only:c").is_some());

        // Bottom 2 by priority: d(5), e(1) — excluded from this query's fan-out.
        // `contains` deliberately stays uncapped (a correctness-sensitive membership
        // check must not silently say "no" for real data), so check via `get` through
        // the capped ownership-lookup path instead, which is what a caller doing
        // `federated.search(...)` would actually observe missing.
        let ordered = federated.priority_ordered_instances();
        let capped_names: Vec<&str> = ordered.iter().map(|(n, _, _)| n.as_str()).collect();
        assert_eq!(
            capped_names,
            vec!["a", "b", "c"],
            "cap must keep exactly the top-3 by priority"
        );
    }

    #[test]
    fn federated_health_report_aggregates_all_instances_and_surfaces_unreachable_ones() {
        // ADR-065 item 1: `health_report` must mirror `id_title_body_triples`'s
        // aggregation pattern (primary + every registered instance), not silently
        // return only the primary. This test is adversarial per CLAUDE.md principle
        // #14 — it injects a real instance whose `health_report()` genuinely returns
        // `None` (simulating a corrupted/unreadable instance) and asserts it is
        // surfaced in `by_instance` as unreachable, not silently dropped from the
        // aggregate as if it were healthy-but-empty.
        let tmp = tempfile::tempdir().unwrap();
        let store1 = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        let store2 = Arc::new(CozoKbStore::open(tmp.path().join("inst.cozo")).unwrap());

        store1
            .insert_node(&Node::new("primary:a", "Primary A", NodeKind::Note, ""))
            .unwrap();
        store2
            .insert_node(&Node::new("inst:a", "Instance A", NodeKind::Note, ""))
            .unwrap();
        store2
            .insert_node(&Node::new("inst:b", "Instance B", NodeKind::Note, ""))
            .unwrap();

        let primary = Arc::new(CozoQueryLayer::new(store1));
        let healthy_inst = Arc::new(CozoQueryLayer::new(store2));
        // A real `KbQueryLayer` implementor whose `health_report()` genuinely returns
        // `Ok(None)` (see `InMemoryQueryLayer::health_report` above — it has no
        // `store::HealthReport` concept at all) — stands in for an instance that
        // contributes nothing to the aggregate without needing a mock. This is
        // deliberately distinct from a genuine storage `Err`, covered separately by
        // `federated_health_report_propagates_a_genuine_instance_storage_error` below
        // (ADR-086 read-side twin: "no report available" and "report query failed"
        // must not be conflated).
        let corrupted_inst = Arc::new(InMemoryQueryLayer::new(crate::KnowledgeBase::new()));

        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("healthy".into(), 0, healthy_inst);
        federated.add_instance("corrupted".into(), 0, corrupted_inst);

        let report = federated
            .health_report()
            .unwrap()
            .expect("primary must report health");

        // The actual bug: total_nodes must reflect primary + the healthy instance
        // (1 + 2 = 3), not just the primary alone (which would silently be 1).
        assert_eq!(report.total_nodes, 3);

        let primary_health = report.by_instance.get("primary").unwrap();
        assert!(primary_health.reachable);
        assert_eq!(primary_health.total_nodes, 1);

        let healthy_health = report.by_instance.get("healthy").unwrap();
        assert!(healthy_health.reachable);
        assert_eq!(healthy_health.total_nodes, 2);

        let corrupted_health = report.by_instance.get("corrupted").unwrap();
        assert!(
            !corrupted_health.reachable,
            "a corrupted/unreadable instance must be surfaced as unreachable, \
             not silently omitted from the federated health report"
        );
    }

    // --- Issue #474: cross-instance orphan/broken-link/hub reconciliation ---
    //
    // `CozoKbStore::health_report`'s orphan/broken-link/hub detection is scoped ENTIRELY
    // to that one store's own `*nodes`/`*links` relations (no cross-engine Datalog query
    // is possible — cozo 0.7.6 has no attach/union primitive; each federated instance is a
    // fully separate embedded engine). `FederatedQuery::health_report` merged those
    // per-instance reports with zero reconciliation, so a link whose target is a real node
    // in a *different* instance was wrongly reported broken, and a node whose only real
    // incoming link comes from a sibling instance was wrongly reported orphaned. These
    // tests are adversarial per CLAUDE.md principle #14: real multi-instance topologies (up
    // to 3-way, not just 2-way), explicit negative/"still genuinely broken" cases alongside
    // the positive "false positive resolved" cases, and a regression guard proving the
    // single-instance case is byte-identical to before.

    #[test]
    fn federated_health_report_orphan_false_positive_resolved_via_cross_instance_incoming_link() {
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .insert_node(&Node::new(
                "primary:no-outgoing",
                "No local links",
                NodeKind::Note,
                "",
            ))
            .unwrap();

        let inst_store = Arc::new(CozoKbStore::open(tmp.path().join("inst.cozo")).unwrap());
        inst_store
            .insert_node(&Node::new("inst:linker", "Linker", NodeKind::Note, ""))
            .unwrap();
        // The only real incoming link to primary:no-outgoing lives in a SIBLING
        // instance's *links relation — invisible to primary's own local orphan query.
        inst_store
            .add_typed_link("inst:linker", "primary:no-outgoing", "references", 1.0)
            .unwrap();

        // Sanity: primary's own local health_report (the pre-federation-fix view) DOES
        // wrongly flag it as an orphan — proving this test would actually catch a
        // regression back to the old per-instance-only behavior.
        let local_report = primary_store.health_report().unwrap();
        assert!(
            local_report
                .orphan_ids
                .contains(&"primary:no-outgoing".to_string()),
            "sanity: primary's own local view must NOT see the cross-instance link"
        );

        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("inst".into(), 0, Arc::new(CozoQueryLayer::new(inst_store)));

        let report = federated.health_report().unwrap().unwrap();
        assert!(
            !report
                .orphan_ids
                .contains(&"primary:no-outgoing".to_string()),
            "a node with a real incoming link from a sibling federated instance must not \
             be reported as an orphan: {:?}",
            report.orphan_ids
        );
    }

    #[test]
    fn federated_health_report_broken_link_false_positive_resolved_via_cross_instance_target() {
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .insert_node(&Node::new("primary:seed", "Seed", NodeKind::Note, ""))
            .unwrap();

        let store_a = Arc::new(CozoKbStore::open(tmp.path().join("a.cozo")).unwrap());
        store_a
            .insert_node(&Node::new("a:linker", "Linker", NodeKind::Note, ""))
            .unwrap();
        // Targets a node that only exists in instance B — locally, instance A's own
        // health_report has no way to know it resolves elsewhere.
        store_a
            .add_typed_link("a:linker", "b:target", "references", 1.0)
            .unwrap();

        let store_b = Arc::new(CozoKbStore::open(tmp.path().join("b.cozo")).unwrap());
        store_b
            .insert_node(&Node::new("b:target", "Target", NodeKind::Note, ""))
            .unwrap();

        let local_a_report = store_a.health_report().unwrap();
        assert!(
            local_a_report
                .broken_links
                .iter()
                .any(|l| l.target == "b:target"),
            "sanity: instance A's own local view must see the link as broken"
        );

        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("a".into(), 0, Arc::new(CozoQueryLayer::new(store_a)));
        federated.add_instance("b".into(), 0, Arc::new(CozoQueryLayer::new(store_b)));

        let report = federated.health_report().unwrap().unwrap();
        assert!(
            !report.broken_links.iter().any(|l| l.target == "b:target"),
            "a link whose target exists in a sibling federated instance must not be \
             reported as broken: {:?}",
            report.broken_links
        );
    }

    #[test]
    fn federated_health_report_orphan_still_reported_when_genuinely_unlinked_across_all_instances()
    {
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .insert_node(&Node::new(
                "primary:truly-orphan",
                "Truly alone",
                NodeKind::Note,
                "",
            ))
            .unwrap();

        // Two sibling instances with real content and links of their own, but NOTHING
        // touching primary:truly-orphan — the negative case that must survive the fix.
        let store_b1 = Arc::new(CozoKbStore::open(tmp.path().join("b1.cozo")).unwrap());
        store_b1
            .insert_node(&Node::new("b1:x", "X", NodeKind::Note, ""))
            .unwrap();
        store_b1
            .insert_node(&Node::new("b1:y", "Y", NodeKind::Note, ""))
            .unwrap();
        store_b1
            .add_typed_link("b1:x", "b1:y", "references", 1.0)
            .unwrap();

        let store_b2 = Arc::new(CozoKbStore::open(tmp.path().join("b2.cozo")).unwrap());
        store_b2
            .insert_node(&Node::new("b2:x", "X", NodeKind::Note, ""))
            .unwrap();
        store_b2
            .insert_node(&Node::new("b2:y", "Y", NodeKind::Note, ""))
            .unwrap();
        store_b2
            .add_typed_link("b2:x", "b2:y", "references", 1.0)
            .unwrap();

        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("b1".into(), 0, Arc::new(CozoQueryLayer::new(store_b1)));
        federated.add_instance("b2".into(), 0, Arc::new(CozoQueryLayer::new(store_b2)));

        let report = federated.health_report().unwrap().unwrap();
        assert!(
            report
                .orphan_ids
                .contains(&"primary:truly-orphan".to_string()),
            "a node with genuinely zero links anywhere across the whole federation must \
             still be reported as an orphan — the fix must not become overly lenient: {:?}",
            report.orphan_ids
        );
    }

    #[test]
    fn federated_health_report_broken_link_still_reported_when_target_missing_everywhere() {
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .insert_node(&Node::new("primary:linker", "Linker", NodeKind::Note, ""))
            .unwrap();
        primary_store
            .add_typed_link("primary:linker", "nowhere:missing", "references", 1.0)
            .unwrap();

        let store_b1 = Arc::new(CozoKbStore::open(tmp.path().join("b1.cozo")).unwrap());
        store_b1
            .insert_node(&Node::new("b1:x", "X", NodeKind::Note, ""))
            .unwrap();
        let store_b2 = Arc::new(CozoKbStore::open(tmp.path().join("b2.cozo")).unwrap());
        store_b2
            .insert_node(&Node::new("b2:x", "X", NodeKind::Note, ""))
            .unwrap();

        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("b1".into(), 0, Arc::new(CozoQueryLayer::new(store_b1)));
        federated.add_instance("b2".into(), 0, Arc::new(CozoQueryLayer::new(store_b2)));

        let report = federated.health_report().unwrap().unwrap();
        assert!(
            report
                .broken_links
                .iter()
                .any(|l| l.target == "nowhere:missing"),
            "a link whose target is missing from EVERY instance in the federation must \
             still be reported as broken: {:?}",
            report.broken_links
        );
    }

    #[test]
    fn federated_health_report_broken_link_resolves_against_third_instance_not_first_checked() {
        // 3 non-primary instances (A holds the link, B is an unrelated sibling that does
        // NOT have the target, C is where the target actually lives) — proves the fix
        // doesn't short-circuit its resolution check after the first sibling it looks at.
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .insert_node(&Node::new("primary:seed", "Seed", NodeKind::Note, ""))
            .unwrap();

        let store_a = Arc::new(CozoKbStore::open(tmp.path().join("a.cozo")).unwrap());
        store_a
            .insert_node(&Node::new("a:linker", "Linker", NodeKind::Note, ""))
            .unwrap();
        store_a
            .add_typed_link("a:linker", "c:target", "references", 1.0)
            .unwrap();

        let store_b = Arc::new(CozoKbStore::open(tmp.path().join("b.cozo")).unwrap());
        store_b
            .insert_node(&Node::new("b:unrelated", "Unrelated", NodeKind::Note, ""))
            .unwrap();

        let store_c = Arc::new(CozoKbStore::open(tmp.path().join("c.cozo")).unwrap());
        store_c
            .insert_node(&Node::new("c:target", "Target", NodeKind::Note, ""))
            .unwrap();

        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("a".into(), 0, Arc::new(CozoQueryLayer::new(store_a)));
        federated.add_instance("b".into(), 0, Arc::new(CozoQueryLayer::new(store_b)));
        federated.add_instance("c".into(), 0, Arc::new(CozoQueryLayer::new(store_c)));

        let report = federated.health_report().unwrap().unwrap();
        assert!(
            !report.broken_links.iter().any(|l| l.target == "c:target"),
            "a link resolving against the THIRD instance checked must still be recognized \
             as resolved, not just one that happens to resolve against the first sibling: \
             {:?}",
            report.broken_links
        );
    }

    #[test]
    fn federated_health_report_hub_node_reflects_true_global_in_degree_spanning_multiple_instances()
    {
        // A node with SOME incoming links recorded in each of 3 different instances
        // (primary + 2 federated instances) — the merged hub count must be the TRUE SUM
        // (3), not any single instance's local count (1 each).
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .add_typed_link("primary:src", "shared:hub", "references", 1.0)
            .unwrap();

        let store_a = Arc::new(CozoKbStore::open(tmp.path().join("a.cozo")).unwrap());
        store_a
            .add_typed_link("a:src", "shared:hub", "references", 1.0)
            .unwrap();

        let store_b = Arc::new(CozoKbStore::open(tmp.path().join("b.cozo")).unwrap());
        store_b
            .add_typed_link("b:src", "shared:hub", "references", 1.0)
            .unwrap();

        // Sanity: no SINGLE instance sees more than its own local contribution.
        assert_eq!(
            primary_store
                .health_report()
                .unwrap()
                .hub_nodes
                .iter()
                .find(|(id, _)| id == "shared:hub")
                .map(|(_, c)| *c),
            Some(1)
        );

        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("a".into(), 0, Arc::new(CozoQueryLayer::new(store_a)));
        federated.add_instance("b".into(), 0, Arc::new(CozoQueryLayer::new(store_b)));

        let report = federated.health_report().unwrap().unwrap();
        let hub_count = report
            .hub_nodes
            .iter()
            .find(|(id, _)| id == "shared:hub")
            .map(|(_, c)| *c);
        assert_eq!(
            hub_count,
            Some(3),
            "merged hub-node in-degree must be the TRUE SUM across all 3 instances, not \
             any single instance's local count: {:?}",
            report.hub_nodes
        );
    }

    #[test]
    fn federated_health_report_hub_node_recovers_node_absent_from_every_instances_own_local_top_10()
    {
        // Proves the "sum full maps, then truncate once" design over a naive "union of
        // each instance's own already-truncated local top-10" design. `shared:sneaky-hub`
        // gets in-degree 2 in EACH of instances A and B (combined global in-degree 4), but
        // in EACH instance individually it's beaten by 10 "noise" nodes each with a HIGHER
        // local-only in-degree of 3 — so `shared:sneaky-hub` never makes either instance's
        // own local top-10 and is invisible to a naive union-of-locally-truncated-lists
        // merge. Its true combined score (4) is still higher than any single noise node's
        // global score (3, since noise nodes are per-instance-only), so a correct
        // federation-wide ranking must surface it.
        let tmp = tempfile::tempdir().unwrap();
        let primary_store = Arc::new(CozoKbStore::open(tmp.path().join("primary.cozo")).unwrap());
        primary_store
            .insert_node(&Node::new("primary:seed", "Seed", NodeKind::Note, ""))
            .unwrap();

        let store_a = Arc::new(CozoKbStore::open(tmp.path().join("a.cozo")).unwrap());
        for i in 0..10 {
            for src in ["src0", "src1", "src2"] {
                store_a
                    .add_typed_link(
                        &format!("a:{src}"),
                        &format!("a:noise-{i}"),
                        "references",
                        1.0,
                    )
                    .unwrap();
            }
        }
        store_a
            .add_typed_link("a:sneaky-src1", "shared:sneaky-hub", "references", 1.0)
            .unwrap();
        store_a
            .add_typed_link("a:sneaky-src2", "shared:sneaky-hub", "references", 1.0)
            .unwrap();

        let store_b = Arc::new(CozoKbStore::open(tmp.path().join("b.cozo")).unwrap());
        for i in 0..10 {
            for src in ["src0", "src1", "src2"] {
                store_b
                    .add_typed_link(
                        &format!("b:{src}"),
                        &format!("b:noise-{i}"),
                        "references",
                        1.0,
                    )
                    .unwrap();
            }
        }
        store_b
            .add_typed_link("b:sneaky-src1", "shared:sneaky-hub", "references", 1.0)
            .unwrap();
        store_b
            .add_typed_link("b:sneaky-src2", "shared:sneaky-hub", "references", 1.0)
            .unwrap();

        // Sanity: shared:sneaky-hub is NOT in either instance's own local top-10.
        for store in [&store_a, &store_b] {
            let local_hubs = store.health_report().unwrap().hub_nodes;
            assert!(
                !local_hubs.iter().any(|(id, _)| id == "shared:sneaky-hub"),
                "sanity: shared:sneaky-hub must be excluded from this instance's own \
                 local top-10: {local_hubs:?}"
            );
            assert_eq!(
                local_hubs.len(),
                10,
                "sanity: local top-10 is full of noise nodes"
            );
        }

        let primary = Arc::new(CozoQueryLayer::new(primary_store));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("a".into(), 0, Arc::new(CozoQueryLayer::new(store_a)));
        federated.add_instance("b".into(), 0, Arc::new(CozoQueryLayer::new(store_b)));

        let report = federated.health_report().unwrap().unwrap();
        let hub_count = report
            .hub_nodes
            .iter()
            .find(|(id, _)| id == "shared:sneaky-hub")
            .map(|(_, c)| *c);
        assert_eq!(
            hub_count,
            Some(4),
            "a node absent from every instance's own local top-10 must still surface in \
             the federation-wide top-10 once its true combined in-degree qualifies: {:?}",
            report.hub_nodes
        );
    }

    #[test]
    fn federated_health_report_single_instance_unchanged_by_hub_fix() {
        // Regression guard: with zero registered instances, hub_nodes must be
        // byte-identical to calling the underlying store's own health_report directly
        // (which is exactly what pre-fix FederatedQuery::health_report reduced to for the
        // single-instance case). Distinct in-degrees (no ties) so ordering is unambiguous.
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("solo.cozo")).unwrap());
        for src in ["s0", "s1", "s2", "s3", "s4"] {
            store
                .add_typed_link(src, "solo:hub-5", "references", 1.0)
                .unwrap();
        }
        for src in ["t0", "t1", "t2"] {
            store
                .add_typed_link(src, "solo:hub-3", "references", 1.0)
                .unwrap();
        }
        store
            .add_typed_link("u0", "solo:hub-1", "references", 1.0)
            .unwrap();

        let direct_report = store.health_report().unwrap();

        let primary = Arc::new(CozoQueryLayer::new(store));
        let federated = FederatedQuery::new(primary);
        let fed_report = federated.health_report().unwrap().unwrap();

        assert_eq!(
            fed_report.hub_nodes, direct_report.hub_nodes,
            "a federation with zero registered instances must produce byte-identical \
             hub_nodes output to the underlying store's own health_report"
        );
    }

    /// A test-only `KbQueryLayer` whose `degraded()` unconditionally reports `true` —
    /// stands in for a real degraded source (e.g. a timed-out `RemoteHubQueryLayer`,
    /// ADR-062 Phase E) without needing to spawn a mock network server, since the only
    /// thing under test here is that `FederatedQuery::degraded()` (issue #474) actually
    /// reads `last_query_was_partial()` instead of falling through to the trait's
    /// unconditional `false` default.
    struct AlwaysDegradedLayer;
    impl KbQueryLayer for AlwaysDegradedLayer {
        fn get(&self, _id: &str) -> Option<Node> {
            None
        }
        fn contains(&self, _id: &str) -> bool {
            false
        }
        fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SearchHit>, KbStoreError> {
            Ok(Vec::new())
        }
        fn links_from(&self, _id: &str) -> Result<Vec<Link>, KbStoreError> {
            Ok(Vec::new())
        }
        fn links_to(&self, _id: &str) -> Result<Vec<Link>, KbStoreError> {
            Ok(Vec::new())
        }
        fn list_ids(&self, _prefix: Option<&str>) -> Result<Vec<String>, KbStoreError> {
            Ok(Vec::new())
        }
        fn id_title_pairs(
            &self,
            _prefix: Option<&str>,
        ) -> Result<Vec<(String, String)>, KbStoreError> {
            Ok(Vec::new())
        }
        fn health_report(&self) -> Result<Option<HealthReport>, KbStoreError> {
            Ok(None)
        }
        fn neighborhood(&self, _id: &str, _depth: u32) -> Result<Option<SubGraph>, KbStoreError> {
            Ok(None)
        }
        fn degraded(&self) -> bool {
            true
        }
    }

    #[test]
    fn federated_query_layer_degraded_reflects_last_search_partial() {
        let primary = Arc::new(InMemoryQueryLayer::new(crate::KnowledgeBase::new()));
        let mut federated = FederatedQuery::new(primary);
        federated.add_instance("flaky".into(), 0, Arc::new(AlwaysDegradedLayer));

        // Trigger a search — this is what actually sets last_query_partial (see
        // FederatedQuery::search's own doc comment above).
        let _ = federated.search("anything", 5);

        // Called via `&dyn KbQueryLayer`, not any inherent method — proves the trait
        // override itself works, not just the underlying `last_query_was_partial()`.
        let layer: &dyn KbQueryLayer = &federated;
        assert!(
            layer.degraded(),
            "FederatedQuery::degraded() must reflect a degraded fan-out source from the \
             most recent search() call"
        );
    }

    #[test]
    fn todo_nodes_via_query_layers() {
        // Cozo layer: only TODO-bearing nodes come back.
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(CozoKbStore::open(tmp.path().join("todo.cozo")).unwrap());
        store
            .insert_node(&Node::new("task:a", "Do A", NodeKind::Task, "").with_todo_state("TODO"))
            .unwrap();
        store
            .insert_node(&Node::new("task:b", "Do B", NodeKind::Task, "").with_todo_state("DONE"))
            .unwrap();
        store
            .insert_node(&Node::new("note:c", "Plain note", NodeKind::Note, ""))
            .unwrap();

        let cozo = Arc::new(CozoQueryLayer::new(store));
        let todos = cozo.todo_nodes().unwrap();
        let ids: Vec<_> = todos.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"task:a"));
        assert!(ids.contains(&"task:b"));
        assert!(!ids.contains(&"note:c"));

        // In-memory layer mirrors the same TODO set.
        let mut kb = crate::KnowledgeBase::new();
        kb.insert(Node::new("task:x", "X", NodeKind::Task, "").with_todo_state("TODO"));
        kb.insert(Node::new("note:y", "Y", NodeKind::Note, ""));
        let mem = InMemoryQueryLayer::new(kb);
        let mem_todos = mem.todo_nodes().unwrap();
        assert_eq!(mem_todos.len(), 1);
        assert_eq!(mem_todos[0].id, "task:x");

        // Federated layer dedups primary over instance and unions instance-only.
        let mut federated = FederatedQuery::new(cozo);
        let tmp2 = tempfile::tempdir().unwrap();
        let store2 = Arc::new(CozoKbStore::open(tmp2.path().join("inst.cozo")).unwrap());
        store2
            .insert_node(&Node::new("task:a", "Dup", NodeKind::Task, "").with_todo_state("TODO"))
            .unwrap();
        store2
            .insert_node(
                &Node::new("task:z", "Inst only", NodeKind::Task, "").with_todo_state("TODO"),
            )
            .unwrap();
        federated.add_instance("inst".into(), 0, Arc::new(CozoQueryLayer::new(store2)));
        let fed_ids: Vec<_> = federated
            .todo_nodes()
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        assert!(fed_ids.contains(&"task:a".to_string()));
        assert!(fed_ids.contains(&"task:z".to_string()));
        // Deduped: task:a appears once.
        assert_eq!(
            federated
                .todo_nodes()
                .unwrap()
                .iter()
                .filter(|n| n.id == "task:a")
                .count(),
            1
        );
    }

    #[test]
    fn in_memory_query_layer() {
        let mut kb = crate::KnowledgeBase::new();
        kb.insert(Node::new(
            "note:a",
            "Alpha",
            NodeKind::Note,
            "body [[note:b]]",
        ));
        kb.insert(Node::new("note:b", "Beta", NodeKind::Note, ""));

        let layer = InMemoryQueryLayer::new(kb);
        assert!(layer.contains("note:a"));
        assert!(!layer.contains("note:c"));

        let links = layer.links_from("note:a").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].dst, "note:b");

        let backlinks = layer.links_to("note:b").unwrap();
        assert_eq!(backlinks.len(), 1);
        assert_eq!(backlinks[0].src, "note:a");
    }
}
