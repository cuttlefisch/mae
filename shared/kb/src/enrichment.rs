//! ADR-061 Phases C/E: the store-facing half of a KB enrichment sweep —
//! deciding which nodes need embedding (a cache-aware scan) and recording
//! freshly-computed vectors back into the cache. Deliberately does NOT call
//! an embedding provider itself: the actual network call is owned by
//! whichever caller already has an HTTP client (the daemon's own async
//! `reqwest::Client` for Phase C's scheduler sweep, `crates/ai`'s
//! `OllamaProvider` for Phase E's editor-side enrich-now path) — this module
//! is the pure, synchronous, easily-unit-tested "what needs doing" /
//! "record what got done" pair each caller wraps its own async embed loop
//! around.
//!
//! Split this way (plan → caller-owned embed → apply) rather than as a single
//! function taking an injected embed callback, so this crate never needs an
//! async runtime or an async-trait dependency at all — every caller already
//! owns the right execution context for its own network call, and the store
//! I/O here is synchronous CozoDB access exactly like every other `mae-kb`
//! entry point.

use crate::federation::{residency_permits_provider, AiResidency};
use crate::store::{KbStore, VectorHit};

/// One node whose current content has no cached embedding under
/// `(content_hash, model, chunk_version)` — a genuine cache miss the caller
/// should embed.
#[derive(Debug, Clone, PartialEq)]
pub struct EnrichmentTarget {
    pub node_id: String,
    pub content_hash: String,
    pub body: String,
}

/// The result of scanning a store for content needing enrichment.
#[derive(Debug, Clone, Default)]
pub struct ScanPlan {
    /// Nodes whose current content-hash has no cache entry yet — what the
    /// caller should actually embed.
    pub targets: Vec<EnrichmentTarget>,
    /// Nodes scanned in total (targets + hits + skipped-empty).
    pub nodes_scanned: usize,
    /// Nodes that already had a cached embedding under the current
    /// `(model, chunk_version)` — no provider call needed for these.
    pub cache_hits: usize,
    /// True iff the KB's AI residency policy forbids `provider_name` — in
    /// this case `targets` is always empty and NO node was read for content
    /// (checked once, before any node is touched, so a `LocalModelsOnly` KB
    /// never has its content passed to a scan at all, not merely "not
    /// embedded").
    pub residency_blocked: bool,
    /// Per-node read errors (a corrupt row, a store I/O failure) — recorded,
    /// not fatal to the rest of the scan, so one bad node never blocks
    /// enrichment of the rest of the KB.
    pub errors: Vec<String>,
}

/// Scan `store` for nodes needing enrichment under `(model, chunk_version)`,
/// gated by `residency` — mirrors the SAME `residency_permits_provider` check
/// ADR-048's chat/completion calls already use (`crates/mae/src/
/// ai_event_handler.rs:172`), not a second, independently-implemented one
/// that could drift out of sync (ADR-061 Phase A's own Verification
/// requirement, reused here for its second caller).
pub fn plan_enrichment_scan(
    store: &dyn KbStore,
    residency: AiResidency,
    provider_name: &str,
    model: &str,
    chunk_version: i64,
) -> ScanPlan {
    let mut plan = ScanPlan::default();

    if !residency_permits_provider(residency, provider_name) {
        plan.residency_blocked = true;
        return plan;
    }

    let ids = match store.list_ids(None) {
        Ok(ids) => ids,
        Err(e) => {
            plan.errors.push(format!("list_ids failed: {e}"));
            return plan;
        }
    };

    for id in ids {
        plan.nodes_scanned += 1;
        let node = match store.get_node(&id) {
            Ok(Some(n)) => n,
            Ok(None) => continue, // deleted between list and read -- not an error
            Err(e) => {
                plan.errors.push(format!("{id}: get_node failed: {e}"));
                continue;
            }
        };
        if node.body.trim().is_empty() {
            continue; // nothing to embed
        }
        let content_hash = crate::activity::body_hash(&node.body);
        match store.get_cached_embedding(&content_hash, model, chunk_version) {
            Ok(Some(_)) => {
                plan.cache_hits += 1;
            }
            Ok(None) => {
                plan.targets.push(EnrichmentTarget {
                    node_id: id,
                    content_hash,
                    body: node.body,
                });
            }
            Err(e) => {
                plan.errors.push(format!("{id}: cache lookup failed: {e}"));
            }
        }
    }

    plan
}

/// Record freshly-computed `(content_hash, vector)` pairs into the cache.
/// Each write is independent (one failure doesn't drop the rest), matching
/// `plan_enrichment_scan`'s own per-node error isolation. Returns the error
/// messages for any writes that failed (empty on full success).
pub fn apply_enrichment_results(
    store: &dyn KbStore,
    model: &str,
    chunk_version: i64,
    results: &[(String, Vec<f32>)],
) -> Vec<String> {
    let mut errors = Vec::new();
    for (content_hash, vec) in results {
        if let Err(e) = store.put_cached_embedding(content_hash, model, chunk_version, vec) {
            errors.push(format!("{content_hash}: cache write failed: {e}"));
        }
    }
    errors
}

/// ADR-061 Phase F1: brute-force cosine k-NN over the `embedding_cache`
/// relation `plan_enrichment_scan`/`apply_enrichment_results` already
/// populate — NOT the fixed `<F32; 384>`-typed HNSW `embeddings` relation
/// (`CozoKbStore::vector_search`), which is hardcoded to one dimensionality
/// and cannot be coupled to the user-configurable `ai_embedding_model`
/// option. KB sizes here are in the thousands of nodes at most, so a linear
/// scan is single-digit-millisecond — no index needed for this to be usable.
///
/// Reuses `body_hash`/`get_cached_embedding` exactly as `plan_enrichment_scan`
/// does, so a node is only a hit here if it was already embedded by a prior
/// `kb_enrich` sweep under the SAME `(model, chunk_version)` pin — a cache
/// miss is silently skipped (not an error), matching the cache's own
/// "absence just means recompute/rescan later" contract.
pub fn search_cached_embeddings(
    store: &dyn KbStore,
    model: &str,
    chunk_version: i64,
    query_vec: &[f32],
    k: usize,
) -> Result<Vec<VectorHit>, String> {
    let ids = store.list_ids(None).map_err(|e| e.to_string())?;

    let mut hits: Vec<VectorHit> = Vec::new();
    for id in ids {
        let node = match store.get_node(&id) {
            Ok(Some(n)) => n,
            Ok(None) => continue,
            Err(_) => continue, // one bad row must not abort the whole scan
        };
        if node.body.trim().is_empty() {
            continue;
        }
        let content_hash = crate::activity::body_hash(&node.body);
        let Ok(Some(vec)) = store.get_cached_embedding(&content_hash, model, chunk_version) else {
            continue; // no cached embedding under this pin -- not a hit
        };
        if let Some(distance) = cosine_distance(query_vec, &vec) {
            hits.push(VectorHit { id, distance });
        }
    }

    hits.sort_by(|a, b| {
        a.distance
            .partial_cmp(&b.distance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(k);
    Ok(hits)
}

/// Cosine distance (`1.0 - cosine_similarity`, lower = more similar) —
/// matches `VectorHit::distance`'s own documented convention ("lower = more
/// similar for cosine"), the same sense `CozoKbStore::vector_search`'s HNSW
/// index returns. `None` on a dimension mismatch (should not happen under a
/// correctly pinned model, but a stale/corrupt cache entry must not panic or
/// silently return a meaningless score) or a zero-magnitude vector.
fn cosine_distance(a: &[f32], b: &[f32]) -> Option<f64> {
    if a.len() != b.len() || a.is_empty() {
        return None;
    }
    let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
    let mag_a: f64 = a.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    let mag_b: f64 = b.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 {
        return None;
    }
    Some(1.0 - (dot / (mag_a * mag_b)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CozoKbStore, Node, NodeKind};

    fn make_store() -> (tempfile::TempDir, CozoKbStore) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test_cozo");
        let store = CozoKbStore::open(&path).unwrap();
        (tmp, store)
    }

    #[test]
    fn residency_blocks_the_whole_scan_before_any_node_is_read() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "n:1",
                "Secret",
                NodeKind::Note,
                "sensitive body",
            ))
            .unwrap();

        let plan = plan_enrichment_scan(
            &store,
            AiResidency::LocalModelsOnly,
            "claude", // a hosted, non-local provider
            "some-model",
            1,
        );
        assert!(plan.residency_blocked);
        assert!(
            plan.targets.is_empty(),
            "no node content may be selected for embedding when residency forbids the provider"
        );
        assert_eq!(
            plan.nodes_scanned, 0,
            "residency is checked BEFORE any node is touched, not per-node after the fact"
        );
    }

    #[test]
    fn local_provider_is_permitted_under_local_models_only_residency() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "Note", NodeKind::Note, "local body"))
            .unwrap();

        let plan = plan_enrichment_scan(
            &store,
            AiResidency::LocalModelsOnly,
            "ollama",
            "some-model",
            1,
        );
        assert!(!plan.residency_blocked);
        assert_eq!(plan.targets.len(), 1);
    }

    #[test]
    fn a_node_with_a_cached_embedding_is_a_hit_not_a_target() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "Note", NodeKind::Note, "cached body"))
            .unwrap();
        let content_hash = crate::activity::body_hash("cached body");
        store
            .put_cached_embedding(&content_hash, "m", 1, &[0.1, 0.2])
            .unwrap();

        let plan = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "m", 1);
        assert_eq!(plan.cache_hits, 1);
        assert!(plan.targets.is_empty());
        assert_eq!(plan.nodes_scanned, 1);
    }

    #[test]
    fn a_model_bump_makes_a_previously_cached_node_a_target_again() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "Note", NodeKind::Note, "body"))
            .unwrap();
        let content_hash = crate::activity::body_hash("body");
        store
            .put_cached_embedding(&content_hash, "model-a", 1, &[0.1])
            .unwrap();

        let plan = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "model-b", 1);
        assert_eq!(
            plan.targets.len(),
            1,
            "a different model_id must be treated as needing its own embedding"
        );
        assert_eq!(plan.cache_hits, 0);
    }

    #[test]
    fn empty_body_nodes_are_scanned_but_never_targeted() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "Empty", NodeKind::Note, ""))
            .unwrap();

        let plan = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "m", 1);
        assert_eq!(plan.nodes_scanned, 1);
        assert!(plan.targets.is_empty());
        assert_eq!(plan.cache_hits, 0);
    }

    // ADR-061 Phase C Verification: "kill mid-sweep and restart; resumption
    // must not double-process nodes already completed before the kill, and
    // must not silently lose nodes still pending."
    #[test]
    fn resuming_after_a_partial_prior_run_only_targets_the_still_unembedded_nodes() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "One", NodeKind::Note, "body one"))
            .unwrap();
        store
            .insert_node(&Node::new("n:2", "Two", NodeKind::Note, "body two"))
            .unwrap();
        store
            .insert_node(&Node::new("n:3", "Three", NodeKind::Note, "body three"))
            .unwrap();

        // Simulate a sweep that got through n:1 before being killed: only
        // n:1's cache entry exists on disk when the process restarts.
        let hash1 = crate::activity::body_hash("body one");
        store.put_cached_embedding(&hash1, "m", 1, &[1.0]).unwrap();

        let plan = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "m", 1);
        let target_ids: Vec<&str> = plan.targets.iter().map(|t| t.node_id.as_str()).collect();

        assert_eq!(
            plan.cache_hits, 1,
            "n:1 must be recognized as already done -- not re-selected for embedding"
        );
        assert!(
            !target_ids.contains(&"n:1"),
            "already-embedded n:1 must never be re-targeted (no duplicate provider call)"
        );
        assert!(
            target_ids.contains(&"n:2") && target_ids.contains(&"n:3"),
            "the still-pending nodes must all be picked up on resume, not silently dropped: {target_ids:?}"
        );
        assert_eq!(target_ids.len(), 2);
    }

    #[test]
    fn apply_results_writes_every_entry_and_a_later_scan_sees_them_all_as_hits() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "One", NodeKind::Note, "alpha"))
            .unwrap();
        store
            .insert_node(&Node::new("n:2", "Two", NodeKind::Note, "beta"))
            .unwrap();

        let plan = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "m", 1);
        assert_eq!(plan.targets.len(), 2);

        let results: Vec<(String, Vec<f32>)> = plan
            .targets
            .iter()
            .map(|t| (t.content_hash.clone(), vec![0.42]))
            .collect();
        let errors = apply_enrichment_results(&store, "m", 1, &results);
        assert!(errors.is_empty());

        let rescanned = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "m", 1);
        assert_eq!(rescanned.cache_hits, 2);
        assert!(rescanned.targets.is_empty());
    }

    // A provider failure for one node must not corrupt the cache or prevent
    // OTHER nodes' results from being recorded -- verified by only supplying
    // a "successful" result for one of two targets (simulating the other
    // having failed embedding upstream and never reaching apply at all).
    #[test]
    fn a_partial_results_batch_only_records_what_was_actually_supplied() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "One", NodeKind::Note, "alpha"))
            .unwrap();
        store
            .insert_node(&Node::new("n:2", "Two", NodeKind::Note, "beta"))
            .unwrap();

        let plan = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "m", 1);
        let succeeded = &plan.targets[0]; // only the first "succeeds"
        apply_enrichment_results(
            &store,
            "m",
            1,
            &[(succeeded.content_hash.clone(), vec![9.9])],
        );

        let rescanned = plan_enrichment_scan(&store, AiResidency::Open, "ollama", "m", 1);
        assert_eq!(
            rescanned.cache_hits, 1,
            "only the node whose result was actually applied is now a hit"
        );
        assert_eq!(
            rescanned.targets.len(),
            1,
            "the node whose embedding failed upstream remains a target -- picked up on the next sweep, never lost"
        );
    }

    #[test]
    fn search_cached_embeddings_ranks_the_closest_vector_first() {
        let (_tmp, store) = make_store();
        for (id, body, vec) in [
            ("n:close", "close", vec![1.0_f32, 0.0, 0.0]),
            ("n:mid", "mid", vec![0.7_f32, 0.7, 0.0]),
            ("n:far", "far", vec![0.0_f32, 1.0, 0.0]),
        ] {
            store
                .insert_node(&Node::new(id, id, NodeKind::Note, body))
                .unwrap();
            let hash = crate::activity::body_hash(body);
            store.put_cached_embedding(&hash, "m", 1, &vec).unwrap();
        }

        let hits = search_cached_embeddings(&store, "m", 1, &[1.0, 0.0, 0.0], 10).unwrap();
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["n:close", "n:mid", "n:far"],
            "must rank by ascending distance to the query vector, not insertion order"
        );
        assert!(hits[0].distance < hits[1].distance && hits[1].distance < hits[2].distance);
    }

    #[test]
    fn search_cached_embeddings_ignores_nodes_with_no_cached_embedding() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new(
                "n:uncached",
                "Uncached",
                NodeKind::Note,
                "no vector yet",
            ))
            .unwrap();

        let hits = search_cached_embeddings(&store, "m", 1, &[1.0, 0.0], 10).unwrap();
        assert!(
            hits.is_empty(),
            "a node never embedded under this pin must not appear as a hit: {hits:?}"
        );
    }

    #[test]
    fn search_cached_embeddings_ignores_a_different_model_pin() {
        let (_tmp, store) = make_store();
        store
            .insert_node(&Node::new("n:1", "One", NodeKind::Note, "body"))
            .unwrap();
        let hash = crate::activity::body_hash("body");
        store
            .put_cached_embedding(&hash, "model-a", 1, &[1.0, 0.0])
            .unwrap();

        let hits = search_cached_embeddings(&store, "model-b", 1, &[1.0, 0.0], 10).unwrap();
        assert!(
            hits.is_empty(),
            "a cache entry under a different model pin must not be treated as interchangeable"
        );
    }

    #[test]
    fn search_cached_embeddings_respects_k() {
        let (_tmp, store) = make_store();
        for (id, body, vec) in [
            ("n:1", "one", vec![1.0_f32, 0.0]),
            ("n:2", "two", vec![0.9_f32, 0.1]),
            ("n:3", "three", vec![0.8_f32, 0.2]),
        ] {
            store
                .insert_node(&Node::new(id, id, NodeKind::Note, body))
                .unwrap();
            let hash = crate::activity::body_hash(body);
            store.put_cached_embedding(&hash, "m", 1, &vec).unwrap();
        }

        let hits = search_cached_embeddings(&store, "m", 1, &[1.0, 0.0], 2).unwrap();
        assert_eq!(hits.len(), 2, "k must cap the returned hit count");
    }
}
