//! KB search/query helpers: federated search, node listing, activity scoring.

use super::*;

/// Canonicalize `path` for identity comparison against a registered
/// instance's own `org_dir` — `KbRegistry::register` (shared/kb/src/
/// federation.rs) already canonicalizes `org_dir` at registration time, so
/// comparing an un-canonicalized caller path against it via `starts_with`
/// silently fails wherever the two forms diverge (e.g. macOS's `/var` ->
/// `/private/var` symlink) — issue #496. Falls back to the path unchanged
/// if it doesn't exist / can't be resolved (same fallback idiom already
/// used independently by `resolve_kb_scope` below and by `mae_kb::watch::
/// normalize_path`), so a hypothetical/not-yet-saved path never fails
/// closed.
fn canonicalize_for_instance_match(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// A pre-embedded query vector for RRF-blending semantic search into
/// `kb_federated_search_scoped_with_vector` (ADR-061 Phase F2). Embedding the
/// query TEXT is a network call this crate has no async runtime/HTTP client
/// for — the caller (`crates/ai`'s `execute_kb_vector_search`) computes this
/// via the same blocking-embed path `execute_kb_enrich` already uses, then
/// hands the result in here. `model`/`chunk_version` are the pin the cached
/// embeddings being searched must match (ADR-034) — a mismatched pin just
/// means `search_cached_embeddings` finds no hits, not an error.
pub struct QueryVector<'a> {
    pub vec: &'a [f32],
    pub model: &'a str,
    pub chunk_version: i64,
}

impl Editor {
    /// Collect all KB node (id, title) pairs from local + federated instances.
    pub fn kb_all_node_pairs(&self) -> Vec<(String, String)> {
        if let Some(q) = self.kb.query_layer() {
            let mut pairs = q.id_title_pairs(None).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "kb id_title_pairs failed");
                Vec::new()
            });
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            return pairs;
        }
        let mut pairs: Vec<(String, String)> = self.kb.primary.all_id_title_pairs();
        let mut seen: std::collections::HashSet<String> =
            pairs.iter().map(|(id, _)| id.clone()).collect();

        for kb in self.kb.instances.values() {
            for (id, title) in kb.all_id_title_pairs() {
                if seen.insert(id.clone()) {
                    pairs.push((id, title));
                }
            }
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }

    /// Collect all KB node (id, title, body) triples from local + federated instances.
    /// Used by KB palettes that need body content for search matching.
    /// Sorted according to `kb_search_sort` option: alphabetical (default/relevance),
    /// activity (recent first), or alphabetical.
    pub fn kb_all_node_triples(&self) -> Vec<(String, String, String)> {
        // Body truncated to 500 chars — only used for fuzzy search, not display.
        let mut triples: Vec<(String, String, String)> = if let Some(q) = self.kb.query_layer() {
            q.id_title_body_triples(None, 500).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "kb id_title_body_triples failed");
                Vec::new()
            })
        } else {
            self.kb.primary.all_id_title_body_triples()
        };
        let mut seen: std::collections::HashSet<String> =
            triples.iter().map(|(id, _, _)| id.clone()).collect();

        if self.kb.query_layer().is_none() {
            for kb in self.kb.instances.values() {
                for (id, title, body) in kb.all_id_title_body_triples() {
                    if seen.insert(id.clone()) {
                        triples.push((id, title, body));
                    }
                }
            }
        }

        if self.kb.search_sort == "activity" {
            self.sort_triples_by_activity(&mut triples);
        } else {
            triples.sort_by(|a, b| a.0.cmp(&b.0));
        }
        triples
    }

    /// Sort `triples` by activity score descending (most recently
    /// accessed/modified/linked first), id ascending as the tiebreak.
    /// Factored out of `kb_all_node_triples`'s `"activity"` branch so
    /// `kb_find_candidates`'s empty-query default (below) can reuse the
    /// exact same comparator instead of duplicating it.
    fn sort_triples_by_activity(&self, triples: &mut [(String, String, String)]) {
        let weights = mae_kb::activity::ActivityWeights {
            decay: self.kb.activity_decay,
            ..Default::default()
        };
        let today = today_ymd();
        triples.sort_by(|a, b| {
            let score_a = self.kb_activity_score_for_id(&a.0, &weights, today);
            let score_b = self.kb_activity_score_for_id(&b.0, &weights, today);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
    }

    /// Node-count signal for deciding the kb-find completion strategy. Uses the
    /// in-memory `primary` (+ instances) length — O(1), no allocation, safe to
    /// call per keystroke. (A Cozo-backed large KB with an empty `primary` falls
    /// back to the eager all-load path; the lazy window targets large in-memory
    /// KBs, which is the common at-scale case.)
    pub fn kb_loaded_node_count(&self) -> usize {
        self.kb.primary.len() + self.kb.instances.values().map(|k| k.len()).sum::<usize>()
    }

    /// Candidate triples (id, title, body≤500) for the kb-find/create palette.
    ///
    /// Small KBs (≤ `KB_FIND_LAZY_THRESHOLD`): return *all* nodes so the palette
    /// filters client-side (instant, no re-search). Large KBs: return a bounded,
    /// query-driven ranked window via `search_ranked` — full-KB-reachable (the
    /// ranker scans primary *and every federated instance*, mirroring
    /// `kb_federated_search_scoped`) yet capped, so per-keystroke work stays
    /// bounded instead of materializing every node. This is the lazy-at-scale
    /// path.
    pub fn kb_find_candidates(&self, query: &str) -> Vec<(String, String, String)> {
        if self.kb_loaded_node_count() <= Self::KB_FIND_LAZY_THRESHOLD {
            let mut triples = self.kb_all_node_triples();
            // Empty-query default: "relevance" has nothing to rank against
            // with zero query terms and silently degenerates to
            // alphabetical-by-id, which doesn't match how users actually
            // work -- mostly cycling between a handful of recent nodes.
            // Default to activity order instead, but only when the sort is
            // still at its default ("relevance"); an explicit
            // "alphabetical"/"activity"/"recency" choice is left untouched.
            // The moment a query is typed this branch no longer applies,
            // restoring today's behavior exactly (usability gap fix, no
            // tracked issue).
            if query.is_empty() && self.kb.search_sort == "relevance" {
                self.sort_triples_by_activity(&mut triples);
            }
            return triples;
        }
        let limit = Self::KB_FIND_LAZY_LIMIT;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut triples: Vec<(String, String, String)> = Vec::new();

        if self.kb.primary_thin() {
            if let Some(ql) = self.kb.query_layer() {
                let hits = ql.search(query, limit).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, query, "kb search failed in kb_find_candidates");
                    Vec::new()
                });
                for hit in hits {
                    if let Some(n) = ql.get(&hit.id) {
                        if seen.insert(n.id.clone()) {
                            let body: String = n.body.chars().take(500).collect();
                            triples.push((n.id.clone(), n.title.clone(), body));
                        }
                    }
                }
            }
        } else {
            for (id, _) in self.kb.primary.search_ranked(query, limit) {
                if let Some(n) = self.kb.primary.get(&id) {
                    if seen.insert(n.id.clone()) {
                        let body: String = n.body.chars().take(500).collect();
                        triples.push((n.id.clone(), n.title.clone(), body));
                    }
                }
            }
        }

        // Federated instances (kb-register'd directories) participate too —
        // this is the part `kb_find_candidates` used to skip entirely once a
        // large KB tipped it into the lazy branch, leaving federated content
        // permanently unreachable through kb-find regardless of query.
        if triples.len() < limit {
            for kb in self.kb.instances.values() {
                for (id, _) in kb.search_ranked(query, limit) {
                    if triples.len() >= limit {
                        break;
                    }
                    if let Some(n) = kb.get(&id) {
                        if seen.insert(n.id.clone()) {
                            let body: String = n.body.chars().take(500).collect();
                            triples.push((n.id.clone(), n.title.clone(), body));
                        }
                    }
                }
            }
        }

        triples
    }

    /// Re-derive the kb-find palette after its query changed: re-search a bounded
    /// ranked window for large KBs (lazy), else the standard client-side filter.
    /// A no-op for non-kb-find palettes beyond their usual `update_filter`.
    pub fn kb_find_palette_query_changed(&mut self) {
        use crate::command_palette::PalettePurpose;
        let (is_kb_find, query) = match self.command_palette.as_ref() {
            Some(p) => (p.purpose == PalettePurpose::KbFindOrCreate, p.query.clone()),
            None => return,
        };
        if is_kb_find && self.kb_loaded_node_count() > Self::KB_FIND_LAZY_THRESHOLD {
            let cands = self.kb_find_candidates(&query);
            if let Some(p) = self.command_palette.as_mut() {
                p.set_kb_find_entries(&cands);
            }
        } else if let Some(p) = self.command_palette.as_mut() {
            p.update_filter();
        }
    }

    /// Get activity score for a node by ID, searching local then federated KBs.
    pub fn kb_activity_score_for_id(
        &self,
        id: &str,
        weights: &mae_kb::activity::ActivityWeights,
        today: (i32, u32, u32),
    ) -> f64 {
        if let Some(q) = self.kb.query_layer() {
            if let Some(node) = q.get(id) {
                return mae_kb::activity::activity_score(&node.properties, weights, today);
            }
            return 0.0;
        }
        if let Some(node) = self.kb.primary.get(id) {
            return mae_kb::activity::activity_score(&node.properties, weights, today);
        }
        for kb in self.kb.instances.values() {
            if let Some(node) = kb.get(id) {
                return mae_kb::activity::activity_score(&node.properties, weights, today);
            }
        }
        0.0
    }

    /// Re-import a single file into the KB instance that covers its directory.
    /// Used after saving a file inside `kb_notes_dir` to keep the graph in sync.
    ///
    /// Issues #455/#498 (same root cause, two platforms): `KbRegistry::register`
    /// canonicalizes `org_dir` (#303's own established discipline) so a
    /// symlinked/relative/non-normalized registration path always resolves to the
    /// same stable location. A caller-supplied `path` that ISN'T already
    /// canonicalized then fails `path.starts_with(&inst)` even when it's the exact
    /// same file — on Windows because `std::fs::canonicalize` prepends the `\\?\`
    /// verbatim-path prefix (tracked in #455's skip list), and on macOS because
    /// `/var` (where `TempDir`'s default `env::temp_dir()` lives) is itself a
    /// symlink to `/private/var` (tracked in #498) — Linux temp dirs typically
    /// don't hit this since `/tmp` there is rarely a symlink requiring resolution,
    /// which is exactly why this surfaced as "Windows/macOS-only", not a Linux
    /// bug being separately introduced twice. Canonicalizing `path` HERE, once,
    /// before the comparison, fixes both platforms' manifestations of the same
    /// mismatch at its actual source instead of working around each symptom
    /// separately. Falls back to the given path if canonicalization fails (e.g.
    /// the file was deleted between the caller's own check and this call) rather
    /// than hard-failing the whole reimport.
    pub fn kb_reimport_file(&mut self, path: &std::path::Path) {
        // Canonicalize once, up front, and shadow `path` so EVERY downstream
        // use (the containment check below, `ids_for_path`, `ids_by_source_
        // file`, `ingest_org_file`, `record_ids`) consistently sees the same
        // canonical form the watcher's own cache keys and the registered
        // instance's `org_dir` both already use -- not just the initial
        // `starts_with` check.
        let path = &canonicalize_for_instance_match(path);
        for (uuid, inst) in self
            .kb
            .registry
            .instances
            .iter()
            .map(|i| (i.uuid.clone(), i.org_dir.clone()))
        {
            if path.starts_with(&inst) {
                // Issue #498/#502 (drift, principle #15): retraction used to depend
                // ENTIRELY on a live watcher's cached path->ids mapping, silently doing
                // nothing when no watcher was tracking this path -- true whenever
                // `kb_watcher_enabled` is off, or whenever `OrgDirWatcher::new` failed to
                // attach (an exhausted inotify-instance limit under heavy concurrent
                // process load, reproduced locally under parallel `cargo test`/nextest
                // execution). Fall back to the in-memory graph's own source_file
                // attribution, which is always available and always correct for "what did
                // this path currently produce" -- retraction on an in-place `:ID:` rename
                // must not silently depend on a watcher having happened to attach.
                let prev_ids = match self
                    .kb
                    .watchers
                    .get(&uuid)
                    .and_then(|w| w.ids_for_path(path))
                {
                    Some(ids) => ids,
                    None => self
                        .kb
                        .instances
                        .get(&uuid)
                        .map(|kb| kb.ids_by_source_file(path))
                        .unwrap_or_default(),
                };
                let ids = match self.kb.instances.get_mut(&uuid) {
                    Some(kb) => kb.ingest_org_file(path),
                    None => return,
                };
                // Retract ids this path no longer produces (e.g. an in-place `:ID:`
                // edit followed by a save) — same class of fix as the watcher path.
                for old_id in prev_ids.iter().filter(|id| !ids.contains(id)) {
                    if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                        kb.remove(old_id);
                    }
                    self.kb_persist_instance_delete(&uuid, old_id);
                }
                // Phase 0b: persist the reimported nodes to the durable instance
                // store — parity with the watcher drain (0a); otherwise a save-driven
                // reimport is lost on restart.
                self.kb_persist_instance_ids(&uuid, &ids);
                // Keep the watcher's own path->ids map in sync too, so a subsequent
                // watcher-driven event for this same path diffs against the truth
                // rather than a stale pre-save mapping.
                if let Some(w) = self.kb.watchers.get(&uuid) {
                    w.record_ids(path, ids);
                }
                return;
            }
        }
    }

    /// Check if a path is inside a registered KB instance directory.
    pub fn kb_path_in_instance(&self, path: &std::path::Path) -> bool {
        let canonical = canonicalize_for_instance_match(path);
        self.kb
            .registry
            .instances
            .iter()
            .any(|i| canonical.starts_with(&i.org_dir))
    }

    /// Resolve a `kb_search_scope` option value / AI-tool `scope` argument to a `KbScope`
    /// (ADR-058 Phase C). Handles the `"project"`/`"project-only"` token specially: resolves
    /// the *current* project root fresh via `active_project_root()` (itself built on
    /// `detect_project_root`, the same detector Phase B's provisioning flow uses to register
    /// a `Project`-kind instance's `project_root` — using the identical accessor on both sides
    /// is what keeps registration and resolution from silently drifting apart) and constructs
    /// `KbScope::Project(root)`. Falls back to `KbScope::parse` for every other token.
    ///
    /// **Canonicalized, matching `KbRegistry::register`'s own established discipline for
    /// `org_dir` (#303)** — falls back to the raw path if canonicalization fails (e.g. the
    /// directory was removed since detection) rather than hard-failing the whole scope
    /// resolution. Without this, a symlinked or non-normalized path could textually differ
    /// from the canonical form `KbInstance.project_root` was registered with (Phase B applies
    /// the identical canonicalization), silently failing to match a real, correctly-registered
    /// project instance — Phase D's negative test (`kb_scope_project_path_identity_tests`)
    /// exercises exactly this.
    ///
    /// Graceful degrade (Phase E): if `"project"` is requested but no project root can be
    /// detected (e.g. editing a scratch buffer with no file), this returns `KbScope::All`
    /// rather than an empty/unusable scope — the caller still gets a working search, just not
    /// narrowed, and nothing panics or silently returns zero results with no explanation.
    pub fn resolve_kb_scope(&self, token: &str) -> mae_kb::KbScope {
        let normalized = token.trim().to_ascii_lowercase();
        if normalized == "project" || normalized == "project-only" {
            return match self.active_project_root() {
                Some(root) => mae_kb::KbScope::Project(canonicalize_for_instance_match(root)),
                None => mae_kb::KbScope::All,
            };
        }
        mae_kb::KbScope::parse(token)
    }

    /// Search across local KB and all federated instances.
    /// Returns (instance_name_or_none, node) pairs, deduplicated by node ID.
    /// Local results take priority over federated.
    /// Respects `kb_search_sort` option: "relevance" (default), "activity", "alphabetical".
    pub fn kb_federated_search(&self, query: &str) -> Vec<(Option<String>, mae_kb::Node)> {
        self.kb_federated_search_scoped(query, &mae_kb::KbScope::All)
    }

    /// Search across the primary KB and federated instances, restricted to the
    /// given `scope` (plan decision D4). `KbScope::All` reproduces
    /// `kb_federated_search` exactly. Local results always win on duplicates.
    /// Respects `kb_search_sort` ("relevance" default / "activity" /
    /// "alphabetical" / "recency"). "recency" ranks by relevance first, then
    /// stably re-sorts so session-visited nodes float to the top (most-recent
    /// first; unvisited nodes keep their relevance order below them).
    pub fn kb_federated_search_scoped(
        &self,
        query: &str,
        scope: &mae_kb::KbScope,
    ) -> Vec<(Option<String>, mae_kb::Node)> {
        self.kb_federated_search_scoped_impl(query, scope, None)
    }

    /// ADR-061 Phase F2: same as `kb_federated_search_scoped`, additionally
    /// blending in semantic hits from the primary KB's cached embeddings via
    /// Reciprocal Rank Fusion. `query_vector` is an ALREADY-EMBEDDED query
    /// (embedding the query text is a network call this crate has no async
    /// runtime/HTTP client for — `crates/ai`'s `execute_kb_vector_search` owns
    /// computing it via the same blocking-embed path `execute_kb_enrich`
    /// already uses, then passes the result in here). `None` reproduces
    /// `kb_federated_search_scoped` exactly, so this is purely additive.
    pub fn kb_federated_search_scoped_with_vector(
        &self,
        query: &str,
        scope: &mae_kb::KbScope,
        query_vector: QueryVector<'_>,
    ) -> Vec<(Option<String>, mae_kb::Node)> {
        self.kb_federated_search_scoped_impl(query, scope, Some(query_vector))
    }

    fn kb_federated_search_scoped_impl(
        &self,
        query: &str,
        scope: &mae_kb::KbScope,
        query_vector: Option<QueryVector<'_>>,
    ) -> Vec<(Option<String>, mae_kb::Node)> {
        use mae_kb::KbScope;
        let use_activity = self.kb.search_sort == "activity";
        let use_alpha = self.kb.search_sort == "alphabetical";
        let use_recency = self.kb.search_sort == "recency";
        let weights = mae_kb::activity::ActivityWeights {
            decay: self.kb.activity_decay,
            ..Default::default()
        };
        let today = if use_activity { today_ymd() } else { (0, 0, 0) };

        // Per-instance ranking, shared by primary + federated members.
        let rank = |kb: &mae_kb::KnowledgeBase| -> Vec<String> {
            if use_activity {
                kb.search_sorted_by_activity(query, &weights, today)
            } else if use_alpha {
                kb.search(query)
            } else {
                kb.search_ranked(query, self.kb.search_max_results)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect()
            }
        };

        let mut results: Vec<(Option<String>, mae_kb::Node)> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // Does the primary participate under this scope? The primary's registry
        // name is matched for the Named case.
        let primary_name = self
            .kb
            .registry
            .instances
            .iter()
            .find(|i| i.primary)
            .map(|i| i.name.as_str());
        let include_primary = match scope {
            KbScope::All | KbScope::LocalOnly => true,
            KbScope::RemoteOnly => false,
            KbScope::Named(n) => primary_name == Some(n.as_str()),
            // Project scope excludes the primary (ADR-058 Phase C/D) — narrowing to "just
            // this project's registered instance(s)" is the point, mirroring RemoteOnly's
            // existing precedent for scopes orthogonal to the machine-global primary.
            KbScope::Project(_) => false,
        };

        if include_primary {
            if self.kb.primary_thin() {
                // Thin primary (Phase D): the in-memory mirror is empty; the daemon
                // holds the primary. Rank + fetch owned nodes via the query layer
                // (daemon LRU). Relevance order — the "activity" sort needs in-memory
                // scoring, so it degrades to relevance here (honest, not silent: the
                // daemon-hosted primary has no local activity log).
                if let Some(ql) = self.kb.query_layer() {
                    let hits = ql.search(query, self.kb.search_max_results).unwrap_or_else(|e| {
                        tracing::warn!(error = %e, query, "kb search failed in kb_federated_search");
                        Vec::new()
                    });
                    for hit in hits {
                        if let Some(node) = ql.get(&hit.id) {
                            if seen_ids.insert(node.id.clone()) {
                                results.push((None, node));
                            }
                        }
                    }
                }
            } else {
                for id in rank(&self.kb.primary) {
                    if let Some(node) = self.kb.primary.get(&id) {
                        if seen_ids.insert(node.id.clone()) {
                            results.push((None, node.clone()));
                        }
                    }
                }
            }
        }

        // Then each federated instance permitted by the scope (skip if seen).
        for (uuid, kb) in &self.kb.instances {
            let inst = self.kb.registry.find_by_uuid(uuid);
            let include = match scope {
                KbScope::All => true,
                KbScope::LocalOnly => false,
                KbScope::RemoteOnly => inst.is_some_and(|i| i.is_remote()),
                KbScope::Named(n) => inst.is_some_and(|i| &i.name == n),
                KbScope::Project(root) => inst.is_some_and(|i| i.matches_project_root(root)),
            };
            if !include {
                continue;
            }
            let inst_name = inst.map(|i| i.name.clone());
            for id in rank(kb) {
                if let Some(node) = kb.get(&id) {
                    if seen_ids.insert(node.id.clone()) {
                        results.push((inst_name.clone(), node.clone()));
                    }
                }
            }
        }

        // ADR-061 Phase F2: fuse in semantic hits BEFORE the alpha/recency
        // resort below, so relevance-mode RRF fusion composes with the same
        // sort-mode logic every other mode already goes through — a
        // "relevance" blended order can still be re-sorted alphabetically or
        // by recency same as a pure-lexical one could. Primary store only
        // (Phase F1's own scope limit): federated instances are in-memory
        // `KnowledgeBase` values with no `Arc<dyn KbStore>` handle to look up
        // cached embeddings against.
        if let Some(qv) = query_vector {
            if let Some(store) = self.kb.store.as_ref() {
                if let Ok(vector_hits) = mae_kb::enrichment::search_cached_embeddings(
                    store.as_ref(),
                    qv.model,
                    qv.chunk_version,
                    qv.vec,
                    results.len().max(self.kb.search_max_results),
                ) {
                    if !vector_hits.is_empty() {
                        results = self.rrf_blend_with_vector(results, vector_hits);
                    }
                }
            }
        }

        if use_alpha {
            results.sort_by(|a, b| a.1.id.cmp(&b.1.id));
        } else if use_recency {
            // Stable sort by descending visit ordinal: most-recently-visited
            // first; ties (incl. all unvisited at rank 0) keep relevance order.
            results.sort_by(|a, b| {
                self.kb
                    .visit_rank(&b.1.id)
                    .cmp(&self.kb.visit_rank(&a.1.id))
            });
        }

        results
    }

    /// Reciprocal Rank Fusion of the existing lexical `results` order with
    /// `vector_hits` (already ranked by ascending cosine distance). Score by
    /// RANK POSITION, not raw score — FTS relevance and cosine distance
    /// aren't on a comparable scale, but ranks from each list are directly
    /// combinable (`score(id) = Σ 1/(60 + rank)`, standard RRF constant). A
    /// node that's a vector hit but never appeared lexically (semantically
    /// related, lexically distant) is fetched fresh from the primary store
    /// and still included — the whole point of blending in a second
    /// modality — and silently dropped only if it's since been deleted.
    fn rrf_blend_with_vector(
        &self,
        lexical: Vec<(Option<String>, mae_kb::Node)>,
        vector_hits: Vec<mae_kb::VectorHit>,
    ) -> Vec<(Option<String>, mae_kb::Node)> {
        const RRF_K: f64 = 60.0;
        let mut score: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let mut entries: std::collections::HashMap<String, (Option<String>, mae_kb::Node)> =
            std::collections::HashMap::new();

        for (rank, (inst, node)) in lexical.into_iter().enumerate() {
            *score.entry(node.id.clone()).or_insert(0.0) += 1.0 / (RRF_K + (rank + 1) as f64);
            entries.insert(node.id.clone(), (inst, node));
        }

        for (rank, hit) in vector_hits.into_iter().enumerate() {
            *score.entry(hit.id.clone()).or_insert(0.0) += 1.0 / (RRF_K + (rank + 1) as f64);
            if !entries.contains_key(&hit.id) {
                if let Some(node) = self.kb.primary.get(&hit.id) {
                    entries.insert(hit.id.clone(), (None, node.clone()));
                } else if let Some(ql) = self.kb.query_layer() {
                    if let Some(node) = ql.get(&hit.id) {
                        entries.insert(hit.id.clone(), (None, node));
                    }
                }
            }
        }

        let mut fused: Vec<(f64, Option<String>, mae_kb::Node)> = entries
            .into_iter()
            .filter_map(|(id, (inst, node))| score.get(&id).map(|s| (*s, inst, node)))
            .collect();
        fused.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        fused
            .into_iter()
            .map(|(_, inst, node)| (inst, node))
            .collect()
    }

    /// Get a node by ID, searching local first then federated instances.
    pub fn kb_federated_get(&self, id: &str) -> Option<(Option<String>, &mae_kb::Node)> {
        if let Some(node) = self.kb.primary.get(id) {
            return Some((None, node));
        }
        for (uuid, kb) in &self.kb.instances {
            if let Some(node) = kb.get(id) {
                let name = self.kb.registry.find_by_uuid(uuid).map(|i| i.name.clone());
                return Some((name, node));
            }
        }
        None
    }
}
