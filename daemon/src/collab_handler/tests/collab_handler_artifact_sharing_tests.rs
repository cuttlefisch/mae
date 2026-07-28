//! ADR-034 / ADR-061 Phase D3 (#420): `kb/fetch_artifact` coverage —
//! membership gating (the attacker case), the `share_derived_artifacts`
//! opt-in toggle, and a real cache-hit response through the real RPC
//! dispatch.

use super::*;
use crate::artifact_store::ArtifactStore;

/// A fake `ArtifactStore` pre-seeded with exactly the entries a test wants to
/// assert are (or are not) served — mirrors `enrichment.rs`'s own
/// `CountingBackend` dependency-injection precedent.
struct FakeArtifactStore {
    entries: std::collections::HashMap<(String, String, String, i64), Vec<f32>>,
}

impl FakeArtifactStore {
    fn new() -> Self {
        Self {
            entries: std::collections::HashMap::new(),
        }
    }
    fn seed(
        mut self,
        kb_id: &str,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
        vector: Vec<f32>,
    ) -> Self {
        self.entries.insert(
            (
                kb_id.to_string(),
                content_hash.to_string(),
                model.to_string(),
                chunk_version,
            ),
            vector,
        );
        self
    }
}

#[async_trait::async_trait]
impl ArtifactStore for FakeArtifactStore {
    async fn get_cached_embedding(
        &self,
        kb_id: &str,
        content_hash: &str,
        model: &str,
        chunk_version: i64,
    ) -> Result<Option<Vec<f32>>, String> {
        Ok(self
            .entries
            .get(&(
                kb_id.to_string(),
                content_hash.to_string(),
                model.to_string(),
                chunk_version,
            ))
            .cloned())
    }
}

fn kb_fetch_artifact_msg(
    kb_id: &str,
    content_hash: &str,
    model: &str,
    chunk_version: i64,
) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/fetch_artifact",
        "params":{"kb_id":kb_id,"content_hash":content_hash,"model":model,"chunk_version":chunk_version}})
}

/// A non-member must be denied outright — the attacker case (ADR-034: "an
/// artifact offered by a non-member is ignored"), gated at the SAME
/// `kb_access` every other read path uses, not a second check.
#[tokio::test]
async fn a_non_member_cannot_fetch_an_artifact() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-artifact-outsider",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_policy_msg("kb-artifact-outsider", "invite"), // no-op, just exercises a real op first
        &mut docs,
    )
    .await;

    let fake = FakeArtifactStore::new().seed(
        "kb-artifact-outsider",
        "hash1",
        "model-a",
        1,
        vec![1.0, 2.0],
    );
    let r = dispatch_as_with_artifacts(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        kb_fetch_artifact_msg("kb-artifact-outsider", "hash1", "model-a", 1),
        &mut docs,
        &fake,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a non-member must be denied, not served (even if the artifact exists)"
    );
}

/// `share_derived_artifacts` defaults to false (ADR-034/opt-in) — even a
/// MEMBER with a genuinely cached artifact gets `has_artifact: false` until
/// the KB owner opts in.
#[tokio::test]
async fn a_member_gets_nothing_while_sharing_is_disabled() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-artifact-disabled",
        "alice",
        &mut docs,
    )
    .await;

    let fake = FakeArtifactStore::new().seed(
        "kb-artifact-disabled",
        "hash1",
        "model-a",
        1,
        vec![1.0, 2.0],
    );
    let r = dispatch_as_with_artifacts(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_fetch_artifact_msg("kb-artifact-disabled", "hash1", "model-a", 1),
        &mut docs,
        &fake,
    )
    .await;
    assert!(
        r.error.is_none(),
        "a member's request itself must succeed: {:?}",
        r.error
    );
    let has_artifact = r.result.as_ref().and_then(|v| v["has_artifact"].as_bool());
    assert_eq!(
        has_artifact,
        Some(false),
        "share_derived_artifacts defaults to false -- must not serve even a genuinely cached vector"
    );
}

/// The real, positive path: sharing enabled, a member fetches a genuinely
/// cached vector and gets it back byte-for-byte.
#[tokio::test]
async fn a_member_fetches_a_cached_vector_once_sharing_is_enabled() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-artifact-enabled",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg(
            "kb/add_member",
            "kb-artifact-enabled",
            &fp("bob"),
            Some("editor"),
        ),
        &mut docs,
    )
    .await;

    // Enable sharing directly on the collection doc (no dedicated RPC/toggle
    // command in this phase -- named scope limit, see the PR's own note).
    {
        let mut coll = load_coll(&store, "kb-artifact-enabled").await;
        let update = coll.set_share_derived_artifacts(true);
        store
            .apply_update("kbc:kb-artifact-enabled", &update, None)
            .await
            .unwrap();
    }

    let fake = FakeArtifactStore::new().seed(
        "kb-artifact-enabled",
        "hash1",
        "model-a",
        1,
        vec![1.0, 2.0, 3.0],
    );
    let r = dispatch_as_with_artifacts(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_fetch_artifact_msg("kb-artifact-enabled", "hash1", "model-a", 1),
        &mut docs,
        &fake,
    )
    .await;
    assert!(
        r.error.is_none(),
        "a member's fetch must succeed: {:?}",
        r.error
    );
    let result = r.result.expect("a result");
    assert_eq!(result["has_artifact"], serde_json::json!(true));
    assert_eq!(result["vector"], serde_json::json!([1.0, 2.0, 3.0]));
}

/// A cache miss (no such key) must be a clean, distinct "not found" response
/// -- not conflated with the sharing-disabled or non-member cases.
#[tokio::test]
async fn a_cache_miss_is_reported_distinctly() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-artifact-miss",
        "alice",
        &mut docs,
    )
    .await;
    {
        let mut coll = load_coll(&store, "kb-artifact-miss").await;
        let update = coll.set_share_derived_artifacts(true);
        store
            .apply_update("kbc:kb-artifact-miss", &update, None)
            .await
            .unwrap();
    }

    let fake = FakeArtifactStore::new(); // nothing seeded
    let r = dispatch_as_with_artifacts(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_fetch_artifact_msg("kb-artifact-miss", "hash1", "model-a", 1),
        &mut docs,
        &fake,
    )
    .await;
    assert!(r.error.is_none());
    let result = r.result.expect("a result");
    assert_eq!(result["has_artifact"], serde_json::json!(false));
    assert!(
        result["reason"]
            .as_str()
            .unwrap_or("")
            .contains("no cached artifact"),
        "a genuine cache miss must be distinguishable from the sharing-disabled case, got: {result:?}"
    );
}
