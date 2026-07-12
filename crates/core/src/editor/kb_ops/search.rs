//! KB search/query helpers: federated search, node listing, activity scoring.

use super::*;

impl Editor {
    /// Collect all KB node (id, title) pairs from local + federated instances.
    pub fn kb_all_node_pairs(&self) -> Vec<(String, String)> {
        if let Some(q) = self.kb.query_layer() {
            let mut pairs = q.id_title_pairs(None);
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
            q.id_title_body_triples(None, 500)
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
        } else {
            triples.sort_by(|a, b| a.0.cmp(&b.0));
        }
        triples
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
            return self.kb_all_node_triples();
        }
        let limit = Self::KB_FIND_LAZY_LIMIT;
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut triples: Vec<(String, String, String)> = Vec::new();

        if self.kb.primary_thin() {
            if let Some(ql) = self.kb.query_layer() {
                for hit in ql.search(query, limit) {
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
    pub fn kb_reimport_file(&mut self, path: &std::path::Path) {
        for (uuid, inst) in self
            .kb
            .registry
            .instances
            .iter()
            .map(|i| (i.uuid.clone(), i.org_dir.clone()))
        {
            if path.starts_with(&inst) {
                let prev_ids = self
                    .kb
                    .watchers
                    .get(&uuid)
                    .and_then(|w| w.ids_for_path(path));
                let ids = match self.kb.instances.get_mut(&uuid) {
                    Some(kb) => kb.ingest_org_file(path),
                    None => return,
                };
                // Retract ids this path no longer produces (e.g. an in-place `:ID:`
                // edit followed by a save) — same class of fix as the watcher path.
                if let Some(prev_ids) = prev_ids {
                    for old_id in prev_ids.iter().filter(|id| !ids.contains(id)) {
                        if let Some(kb) = self.kb.instances.get_mut(&uuid) {
                            kb.remove(old_id);
                        }
                        self.kb_persist_instance_delete(&uuid, old_id);
                    }
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
        self.kb
            .registry
            .instances
            .iter()
            .any(|i| path.starts_with(&i.org_dir))
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
        };

        if include_primary {
            if self.kb.primary_thin() {
                // Thin primary (Phase D): the in-memory mirror is empty; the daemon
                // holds the primary. Rank + fetch owned nodes via the query layer
                // (daemon LRU). Relevance order — the "activity" sort needs in-memory
                // scoring, so it degrades to relevance here (honest, not silent: the
                // daemon-hosted primary has no local activity log).
                if let Some(ql) = self.kb.query_layer() {
                    for hit in ql.search(query, self.kb.search_max_results) {
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
