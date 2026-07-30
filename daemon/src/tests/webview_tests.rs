//! Dispatch-level tests for the live HTML KB view (ADR-073/Phase E, #547).
//!
//! Mirrors `kb_query_tests.rs`'s own convention exactly: real `DocStore` +
//! real crypto + real principal strings, driving `render_webview_response`
//! directly (split out from `handle_request` for exactly this reason) rather
//! than a live HTTP connection — the transport layer (real TLS, real bearer
//! header/query-param parsing over the wire) is covered separately by
//! `daemon/tests/oauth_e2e.rs`. Scoped to the NEW logic this phase
//! introduces: the access gate applied to `GET /kb/{kb_id}/view` and the
//! non-JSON response shape, not re-proving `kb/query.*`'s own business logic
//! a second time.

use std::sync::Arc;

use hyper::StatusCode;
use mae_daemon::doc_store::DocStore;
use mae_daemon::storage::SqliteBackend;
use mae_mcp::identity::Identity;
use mae_sync::kb::Role;

use crate::oauth::{render_webview_response, ResourceServerConfig, ValidatedPrincipal};
use crate::tests::kb_query_tests::seed_unencrypted_kb;

async fn fresh_doc_store() -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500))
}

fn webview_config() -> ResourceServerConfig {
    ResourceServerConfig {
        canonical_resource_uri: "https://mae.example.com/mcp".to_string(),
        principal_claim: "sub".to_string(),
        jwks_url: "https://unused.example.com/jwks".to_string(),
        issuer: None,
        kb_query_enabled: true,
        max_request_body_bytes: 1_048_576,
        kb_query_max_body_bytes: 65_536,
        kb_query_max_scan_nodes: 500,
        kb_query_max_search_results: 20,
        webview_enabled: true,
    }
}

fn principal(name: &str) -> ValidatedPrincipal {
    ValidatedPrincipal {
        principal: name.to_string(),
        audience: vec!["https://mae.example.com/mcp".to_string()],
        expires_at: 0,
    }
}

async fn response_bytes(resp: hyper::Response<http_body_util::Full<bytes::Bytes>>) -> Vec<u8> {
    use http_body_util::BodyExt;
    resp.into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec()
}

/// Positive case: a Viewer-role member gets a real, non-JSON HTML page for
/// the KB they belong to.
#[tokio::test]
async fn a_member_with_access_gets_a_real_html_page() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_unencrypted_kb(
        &doc_store,
        &owner,
        "kb-alice",
        Some(("oauth:alice@example.com", Role::Viewer)),
        "n1",
        "Alice's Node",
        "ALICE_SECRET_BODY_MARKER",
        &[],
    )
    .await;

    let resp = render_webview_response(
        "kb-alice",
        "test-bearer-token",
        &webview_config(),
        Some(&doc_store),
        &principal("oauth:alice@example.com"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::OK);
    let content_type = resp
        .headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        content_type.starts_with("text/html"),
        "expected a real non-JSON Content-Type, got: {content_type}"
    );
    let body = response_bytes(resp).await;
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        body_str.contains("kb-alice"),
        "page must embed its own kb_id"
    );
    assert!(
        body_str.contains("test-bearer-token"),
        "page must embed the bearer token for its own polling JS to reuse"
    );
    // Gate G1: v1 must state plainly that it polls, never imply push.
    assert!(body_str.to_lowercase().contains("poll"));
}

/// Adversarial (gate G5): a principal with NO access to a KB must be denied
/// the view entirely -- never a page that renders and only fails on its
/// first background poll.
#[tokio::test]
async fn a_non_member_is_denied_the_view_entirely() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_unencrypted_kb(
        &doc_store,
        &owner,
        "kb-private",
        Some(("oauth:alice@example.com", Role::Viewer)),
        "n1",
        "Private Node",
        "PRIVATE_SECRET_BODY_MARKER",
        &[],
    )
    .await;

    let resp = render_webview_response(
        "kb-private",
        "test-bearer-token",
        &webview_config(),
        Some(&doc_store),
        &principal("oauth:mallory@example.com"),
    )
    .await;

    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let content_type = resp
        .headers()
        .get(hyper::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert_eq!(content_type, "application/json");
    let body = response_bytes(resp).await;
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        !body_str.contains("PRIVATE_SECRET_BODY_MARKER"),
        "a denied request must never leak the KB's node content: {body_str}"
    );
}

/// Adversarial (gate G5, the literal ADR-073 requirement): a member of KB A
/// requesting KB A's view must never have KB B's content appear anywhere in
/// the raw response bytes, even though both KBs exist in the same
/// `DocStore`. Two DISTINCT, non-cherry-picked KBs and node bodies
/// (principle #14) -- not a single KB where "no leak" would be vacuously
/// true.
#[tokio::test]
async fn a_kb_view_never_leaks_a_different_kbs_content_in_the_raw_response() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_unencrypted_kb(
        &doc_store,
        &owner,
        "kb-a",
        Some(("oauth:alice@example.com", Role::Viewer)),
        "node-a",
        "Node A Title",
        "SECRET_MARKER_BELONGING_TO_KB_A",
        &[],
    )
    .await;
    seed_unencrypted_kb(
        &doc_store,
        &owner,
        "kb-b",
        Some(("oauth:bob@example.com", Role::Viewer)),
        "node-b",
        "Node B Title",
        "SECRET_MARKER_BELONGING_TO_KB_B",
        &[],
    )
    .await;

    let resp = render_webview_response(
        "kb-a",
        "alice-token",
        &webview_config(),
        Some(&doc_store),
        &principal("oauth:alice@example.com"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = response_bytes(resp).await;
    let body_str = String::from_utf8_lossy(&body);
    assert!(
        !body_str.contains("SECRET_MARKER_BELONGING_TO_KB_B"),
        "kb-a's view must never contain kb-b's content: {body_str}"
    );
    assert!(
        !body_str.contains("kb-b"),
        "kb-a's view must not reference kb-b at all"
    );
}

/// Adversarial (QA-pass-style, mirroring `kb_query_enabled_but_no_doc_store_
/// gets_a_distinct_jsonrpc_error`): `webview_enabled=true` but no `DocStore`
/// exists (`collab.enabled=false`) is a distinct condition from "denied" and
/// must get its own clear error, not a panic or a misleading 200.
#[tokio::test]
async fn webview_with_no_doc_store_gets_a_clean_service_unavailable() {
    let resp = render_webview_response(
        "kb-anything",
        "tok",
        &webview_config(),
        None,
        &principal("oauth:alice@example.com"),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
