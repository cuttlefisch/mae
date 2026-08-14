//! ADVERSARIAL (#456): the collab dispatch must actually CONSULT the quota seam.
//!
//! ADR-060 Phase C's mechanism was already implemented and unit-tested inside
//! `tenant.rs` while requests sailed straight past it — "the mechanism works" was
//! true and the property below was false. That gap is what this file pins, using a
//! stub charger so it tests the wiring rather than re-testing the registry (the
//! real `TenantRegistry` lives in the binary crate and its own behaviour is covered
//! in `tenant::tests`).

use super::*;
use crate::quota::{QuotaCharger, QuotaLease};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Refuses every request and counts what it was asked about.
struct AlwaysThrottle {
    seen: AtomicUsize,
    last_method: std::sync::Mutex<String>,
}

impl QuotaCharger for AlwaysThrottle {
    fn charge(&self, _principal: Option<&str>, method: &str) -> Result<QuotaLease, String> {
        self.seen.fetch_add(1, Ordering::SeqCst);
        *self.last_method.lock().unwrap() = method.to_string();
        Err("tenant quota exceeded for 'alice'".to_string())
    }
}

async fn state_vector_as(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    quota: &dyn QuotaCharger,
    docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"sync/state_vector",
        "params":{"doc":"plain-doc-1"}})
    .to_string();
    handle_doc_request_inner(
        &msg,
        store,
        quota,
        bc,
        std::time::Instant::now(),
        0,
        Some("alice"),
        Some(&fp("alice")),
        None,
        docs,
        Transport::Hub,
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        None,
    )
    .await
}

#[tokio::test]
async fn a_throttled_request_is_refused_by_the_collab_dispatch() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let quota = AlwaysThrottle {
        seen: AtomicUsize::new(0),
        last_method: std::sync::Mutex::new(String::new()),
    };
    let mut docs = HashSet::new();

    let resp = state_vector_as(&store, &bc, &quota, &mut docs).await;

    let err = resp
        .error
        .expect("a throttled request must be refused, not served");
    assert!(
        err.message.contains("quota"),
        "the refusal must say why, got: {}",
        err.message
    );
    assert_eq!(
        quota.seen.load(Ordering::SeqCst),
        1,
        "dispatch must consult the charger exactly once per request"
    );
    assert_eq!(
        *quota.last_method.lock().unwrap(),
        "sync/state_vector",
        "the charger must be told which method it is pricing"
    );
}

/// The other half of the oracle: with an admitting charger the same request is
/// served. Without this, a fix that refused everything would pass the test above.
#[tokio::test]
async fn an_admitted_request_still_reaches_its_handler() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let resp = state_vector_as(&store, &bc, &crate::quota::NoQuota, &mut docs).await;

    assert!(
        resp.error.is_none(),
        "an admitted request must be served: {:?}",
        resp.error
    );
}
