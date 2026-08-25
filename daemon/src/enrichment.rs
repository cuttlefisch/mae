//! ADR-061 Phase C: the daemon's own async half of a KB enrichment sweep —
//! orchestrates `mae_kb::enrichment`'s pure, synchronous plan/apply functions
//! around the daemon's own async `reqwest::Client`, dispatched from
//! `scheduler.rs`'s `maintenance_tick` (mirroring the sibling `health_tick`
//! hygiene-scan pattern, `daemon/src/maintenance.rs`).
//!
//! The store I/O (`plan_enrichment_scan`/`apply_enrichment_results`) runs
//! inside `tokio::task::spawn_blocking` (ADR-054 — a synchronous CozoDB scan
//! must never run inline on the async executor); the embedding HTTP call
//! runs directly on the async executor between those two blocking passes,
//! using the daemon's own already-async `reqwest::Client` — no `Handle::
//! block_on` bridging needed, since nothing here is itself called from
//! inside a `spawn_blocking` closure.

use std::sync::Arc;

use async_trait::async_trait;
use mae_kb::enrichment::{apply_enrichment_results, plan_enrichment_scan, EnrichmentTarget};
use mae_kb::federation::AiResidency;
use mae_kb::CozoKbStore;

use crate::config::EnrichmentConfig;

/// Injectable embedding backend — production wires a real Ollama HTTP call;
/// tests inject a fake that counts calls and returns canned vectors, so the
/// sweep's resumption/error-isolation properties are verified without a live
/// Ollama server in CI (matching this session's own `retry_on_etxtbsy`
/// dependency-injection precedent, `crates/babel/src/backend/compiled.rs`).
#[async_trait]
pub trait EmbedBackend: Send + Sync {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String>;
}

// ADR-061 Phase D2: `LeaseFence`/`NoFence` live in `mae_daemon::lease_fence`
// (the library crate), not here — `collab_handler::kb_lease::DaemonLeaseFence`
// (the production implementation) is library-crate code and cannot implement
// a trait defined in this binary-only module. See that module's doc comment
// for the full crate-boundary rationale.
use mae_daemon::lease_fence::LeaseFence;

/// The real Ollama backend, using the daemon's own async `reqwest::Client`
/// and the shared, dependency-light request/response shaping
/// (`mae_kb::embedding_client`, ADR-061 Phase C) — the same shaping
/// `crates/ai::OllamaProvider::embed` uses, so the wire format can never
/// silently drift between the editor's chat-adjacent embed path and this
/// one.
pub struct OllamaEmbedBackend {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl OllamaEmbedBackend {
    pub fn new(base_url: String, api_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            api_key,
        }
    }
}

#[async_trait]
impl EmbedBackend for OllamaEmbedBackend {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let url = format!("{}/api/embed", self.base_url);
        let body = mae_kb::embedding_client::build_ollama_embed_request(model, inputs);

        let mut request = self
            .client
            .post(&url)
            .header("content-type", "application/json");
        if let Some(key) = self.api_key.as_deref().filter(|k| !k.is_empty()) {
            request = request.header("Authorization", format!("Bearer {key}"));
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("HTTP error: {e}"))?;

        let status = response.status();
        let resp_body: serde_json::Value = response
            .json()
            .await
            .map_err(|e| format!("failed to read/parse response body: {e}"))?;

        if !status.is_success() {
            return Err(format!("API error {status}: {resp_body}"));
        }

        mae_kb::embedding_client::parse_ollama_embed_response(&resp_body).map_err(|e| e.message)
    }
}

/// Result of one sweep of one store — returned to the caller for logging.
#[derive(Debug, Default)]
pub struct SweepResult {
    pub nodes_scanned: usize,
    pub cache_hits: usize,
    pub newly_embedded: usize,
    pub residency_blocked: bool,
    pub errors: Vec<String>,
}

/// Run one full enrichment sweep of `store`: plan (blocking) → embed in
/// batches of `cfg.batch_size` (async, on the caller's executor) → **re-check
/// `fence`** → apply (blocking). A batch's embed failure is recorded and does
/// NOT abort the sweep — every other batch still gets its chance, matching
/// `mae_kb::enrichment`'s own per-node error-isolation discipline one level
/// up. `fence` is ADR-061 Phase D2's write-time lease re-check — pass
/// [`NoFence`] for a KB that isn't collab-shared (nothing to coordinate).
pub async fn run_enrichment_sweep(
    store: Arc<CozoKbStore>,
    residency: AiResidency,
    backend: &dyn EmbedBackend,
    cfg: &EnrichmentConfig,
    fence: &dyn LeaseFence,
) -> SweepResult {
    let mut result = SweepResult::default();

    let plan = {
        let store = Arc::clone(&store);
        let provider = cfg.provider.clone();
        let model = cfg.model.clone();
        let chunk_version = cfg.chunk_version;
        match tokio::task::spawn_blocking(move || {
            plan_enrichment_scan(store.as_ref(), residency, &provider, &model, chunk_version)
        })
        .await
        {
            Ok(plan) => plan,
            Err(e) => {
                result.errors.push(format!("plan task panicked: {e}"));
                return result;
            }
        }
    };

    result.nodes_scanned = plan.nodes_scanned;
    result.cache_hits = plan.cache_hits;
    result.residency_blocked = plan.residency_blocked;
    result.errors.extend(plan.errors);

    if plan.targets.is_empty() {
        return result;
    }

    let mut embedded: Vec<(String, String, Vec<f32>)> = Vec::with_capacity(plan.targets.len());
    for batch in plan.targets.chunks(cfg.batch_size.max(1)) {
        let inputs: Vec<String> = batch.iter().map(|t| t.body.clone()).collect();
        match backend.embed(&cfg.model, &inputs).await {
            Ok(vecs) => {
                for (target, vec) in batch.iter().zip(vecs) {
                    embedded.push((target.node_id.clone(), target.content_hash.clone(), vec));
                }
            }
            Err(e) => {
                let ids: Vec<&str> = batch
                    .iter()
                    .map(|t: &EnrichmentTarget| t.node_id.as_str())
                    .collect();
                result
                    .errors
                    .push(format!("embed batch failed for {ids:?}: {e}"));
                // This batch's targets remain un-cached -- picked up again on
                // the next sweep (the SAME resumption guarantee proven for a
                // killed process, just triggered by a batch-level failure
                // instead).
            }
        }
    }

    result.newly_embedded = embedded.len();

    if !embedded.is_empty() {
        // ADR-061 Phase D2 / ADR-033: re-check the lease immediately before
        // committing — the embed loop above can take real wall-clock time
        // (network calls), so the lease this sweep started under may have
        // since expired and been granted to another daemon. Discard rather
        // than commit a stale batch; the next tick's `plan_enrichment_scan`
        // naturally re-targets whatever this daemon didn't finish (the same
        // resumption guarantee already proven for a killed process).
        if let Err(e) = fence.check().await {
            result.newly_embedded = 0;
            result.errors.push(format!(
                "lease fence rejected commit, discarding batch: {e}"
            ));
            return result;
        }
        let model = cfg.model.clone();
        let chunk_version = cfg.chunk_version;
        let apply_errors = match tokio::task::spawn_blocking(move || {
            apply_enrichment_results(store.as_ref(), &model, chunk_version, &embedded)
        })
        .await
        {
            Ok(errs) => errs,
            Err(e) => vec![format!("apply task panicked: {e}")],
        };
        result.errors.extend(apply_errors);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mae_daemon::lease_fence::NoFence;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn make_store() -> Arc<CozoKbStore> {
        Arc::new(CozoKbStore::open_mem().unwrap())
    }

    fn insert(store: &CozoKbStore, id: &str, body: &str) {
        use mae_kb::store::KbStore;
        use mae_kb::{Node, NodeKind};
        store
            .insert_node(&Node::new(id, id, NodeKind::Note, body))
            .unwrap();
    }

    /// Counts calls and echoes back a deterministic vector per input so tests
    /// can assert exactly which content was (and was not) embedded.
    struct CountingBackend {
        calls: AtomicUsize,
        embedded_inputs: Mutex<Vec<String>>,
        fail_containing: Option<&'static str>,
    }

    impl CountingBackend {
        fn new() -> Self {
            Self {
                calls: AtomicUsize::new(0),
                embedded_inputs: Mutex::new(Vec::new()),
                fail_containing: None,
            }
        }
        fn failing(fail_containing: &'static str) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                embedded_inputs: Mutex::new(Vec::new()),
                fail_containing: Some(fail_containing),
            }
        }
    }

    #[async_trait]
    impl EmbedBackend for CountingBackend {
        async fn embed(&self, _model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(needle) = self.fail_containing {
                if inputs.iter().any(|i| i.contains(needle)) {
                    return Err("simulated provider failure".to_string());
                }
            }
            self.embedded_inputs
                .lock()
                .unwrap()
                .extend(inputs.iter().cloned());
            Ok(inputs.iter().map(|s| vec![s.len() as f32]).collect())
        }
    }

    fn test_cfg() -> EnrichmentConfig {
        EnrichmentConfig {
            enabled: true,
            provider: "ollama".to_string(),
            base_url: "http://unused.invalid".to_string(),
            api_key: None,
            model: "test-model".to_string(),
            chunk_version: 1,
            batch_size: 16,
            lease_ttl_secs: 300,
        }
    }

    #[tokio::test]
    async fn a_fresh_sweep_embeds_every_node_and_caches_the_results() {
        let store = make_store();
        insert(&store, "n:1", "alpha content");
        insert(&store, "n:2", "beta content");

        let backend = CountingBackend::new();
        let result = run_enrichment_sweep(
            Arc::clone(&store),
            AiResidency::Open,
            &backend,
            &test_cfg(),
            &NoFence,
        )
        .await;

        assert_eq!(result.nodes_scanned, 2);
        assert_eq!(result.newly_embedded, 2);
        assert!(result.errors.is_empty());
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "one batch call for both nodes"
        );

        // A second sweep must find both nodes already cached -- zero new
        // provider calls.
        let backend2 = CountingBackend::new();
        let result2 = run_enrichment_sweep(
            Arc::clone(&store),
            AiResidency::Open,
            &backend2,
            &test_cfg(),
            &NoFence,
        )
        .await;
        assert_eq!(result2.cache_hits, 2);
        assert_eq!(result2.newly_embedded, 0);
        assert_eq!(
            backend2.calls.load(Ordering::SeqCst),
            0,
            "a fully-cached KB must make zero provider calls on the next sweep"
        );
    }

    // ADR-061 Phase C Verification: "kill mid-sweep and restart; resumption
    // must not double-process nodes already completed, must not lose nodes
    // still pending" -- exercised at THIS orchestration layer (batches +
    // async embed), not just the pure plan/apply layer in mae-kb.
    #[tokio::test]
    async fn a_batch_failure_does_not_lose_or_duplicate_other_batches_work() {
        let store = make_store();
        insert(&store, "n:1", "good content one");
        insert(&store, "n:2", "BOOM content two"); // triggers the fake failure
        insert(&store, "n:3", "good content three");

        let mut cfg = test_cfg();
        cfg.batch_size = 1; // force one node per batch so the failure is isolated

        let backend = CountingBackend::failing("BOOM");
        let result = run_enrichment_sweep(
            Arc::clone(&store),
            AiResidency::Open,
            &backend,
            &cfg,
            &NoFence,
        )
        .await;

        assert_eq!(result.nodes_scanned, 3);
        assert_eq!(
            result.newly_embedded, 2,
            "the two good batches must still be embedded despite the third failing"
        );
        assert_eq!(
            result.errors.len(),
            1,
            "exactly one error, for the failed batch"
        );

        // Resume: the failed node must still be a target (not lost), the two
        // successful ones must NOT be re-embedded (not duplicated).
        let backend2 = CountingBackend::new(); // no longer fails -- "the transient issue cleared"
        let result2 = run_enrichment_sweep(
            Arc::clone(&store),
            AiResidency::Open,
            &backend2,
            &cfg,
            &NoFence,
        )
        .await;
        assert_eq!(result2.cache_hits, 2, "n:1 and n:3 must not be re-embedded");
        assert_eq!(
            result2.newly_embedded, 1,
            "only n:2 (previously failed) is embedded on resume"
        );
    }

    #[tokio::test]
    async fn residency_blocks_the_sweep_before_any_embed_call() {
        let store = make_store();
        insert(&store, "n:1", "sensitive content");

        let backend = CountingBackend::new();
        let result = run_enrichment_sweep(
            Arc::clone(&store),
            AiResidency::LocalModelsOnly,
            &backend,
            &EnrichmentConfig {
                provider: "claude".to_string(), // hosted, non-local
                ..test_cfg()
            },
            &NoFence,
        )
        .await;

        assert!(result.residency_blocked);
        assert_eq!(result.newly_embedded, 0);
        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            0,
            "residency must be enforced before any node's content ever reaches embed()"
        );
    }

    #[tokio::test]
    async fn an_empty_kb_is_a_clean_no_op() {
        let store = make_store();
        let backend = CountingBackend::new();
        let result = run_enrichment_sweep(
            Arc::clone(&store),
            AiResidency::Open,
            &backend,
            &test_cfg(),
            &NoFence,
        )
        .await;
        assert_eq!(result.nodes_scanned, 0);
        assert_eq!(backend.calls.load(Ordering::SeqCst), 0);
        assert!(result.errors.is_empty());
    }

    /// A fence that always rejects — simulates "another daemon was granted the
    /// lease before this sweep could commit."
    struct RejectingFence;

    #[async_trait]
    impl LeaseFence for RejectingFence {
        async fn check(&self) -> Result<(), String> {
            Err("simulated: lease moved on".to_string())
        }
    }

    // ADR-061 Phase D2 / ADR-033 Verification D (the fence half — the N-way
    // claim race itself is exercised at the daemon/collab_handler layer,
    // `collab_handler_lease_race_tests.rs`): a sweep that computes real
    // embeddings but then finds the lease has moved on must discard the
    // batch, not commit it — asserted via the store's own cache staying
    // empty (a real observation, not just checking the returned error
    // string).
    #[tokio::test]
    async fn a_fence_rejection_discards_the_batch_instead_of_committing() {
        let store = make_store();
        insert(&store, "n:1", "alpha content");
        insert(&store, "n:2", "beta content");

        let backend = CountingBackend::new();
        let result = run_enrichment_sweep(
            Arc::clone(&store),
            AiResidency::Open,
            &backend,
            &test_cfg(),
            &RejectingFence,
        )
        .await;

        assert_eq!(
            backend.calls.load(Ordering::SeqCst),
            1,
            "the embed call itself still happens -- the fence is checked at COMMIT time, \
             not before embedding starts (matches ADR-033's own framing: a paused/slow \
             holder's late WRITE is rejected, not its compute prevented outright)"
        );
        assert_eq!(
            result.newly_embedded, 0,
            "a rejected batch must not be reported as committed"
        );
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.contains("lease fence rejected")),
            "the rejection must be visible in the sweep result, got: {:?}",
            result.errors
        );

        // The real, load-bearing assertion: the cache must actually be empty
        // afterward, not just that the result struct says so.
        use mae_kb::activity::body_hash;
        use mae_kb::store::KbStore;
        let n1 = store.get_node("n:1").unwrap().unwrap();
        let hash = body_hash(&n1.body);
        assert!(
            store
                .get_cached_embedding(&hash, &test_cfg().model, test_cfg().chunk_version)
                .unwrap()
                .is_none(),
            "a fence-rejected batch must not have written anything to the cache"
        );
    }
}
