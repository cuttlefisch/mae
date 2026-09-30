//! Dispatch-level tests for the live HTML KB view (ADR-073/Phase E, #547).
//!
//! Drives `render_webview_response` directly rather than a live HTTP
//! connection — the transport layer (real TLS, the refusal of a query-string
//! token, the shell being byte-identical with and without a credential, and the
//! page's first request being where access is refused) is covered over the real
//! wire by `daemon/tests/oauth_e2e.rs`.
//!
//! ADR-111 P1 made the page a token-free SHELL: its credential travels in the
//! URL fragment, so the server never sees one when serving it and the shell is
//! the same bytes for every caller. What is left to pin here is that the shell
//! carries no KB content (its own or any other KB's) and that a daemon with no
//! `DocStore` says so plainly.

use std::sync::Arc;

use hyper::StatusCode;
use mae_daemon::doc_store::DocStore;
use mae_daemon::storage::SqliteBackend;
use mae_mcp::identity::Identity;
use mae_sync::kb::Role;

use crate::oauth::render_webview_response;
use crate::tests::kb_query_tests::seed_unencrypted_kb;

async fn fresh_doc_store() -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500))
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

/// The shell is a real, non-JSON HTML page that names its KB, states that it
/// polls (gate G1), and carries none of the KB's node content -- that arrives
/// only through the gated `kb/query.*` requests the page makes.
#[tokio::test]
async fn the_shell_is_html_that_names_its_kb_and_carries_none_of_its_content() {
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

    let resp = render_webview_response("kb-alice", Some(&doc_store));

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
    for content in ["ALICE_SECRET_BODY_MARKER", "Alice's Node", "n1\""] {
        assert!(
            !body_str.contains(content),
            "the unauthenticated shell must carry no node content ({content})"
        );
    }
    // Gate G1: v1 must state plainly that it polls, never imply push.
    assert!(body_str.to_lowercase().contains("poll"));
}

/// Adversarial (gate G5, the literal ADR-073 requirement): KB A's shell never
/// carries KB B's content, even though both KBs live in the same `DocStore`.
/// Two DISTINCT KBs and node bodies (principle #14) -- not a single KB where
/// "no leak" would be vacuously true.
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

    let resp = render_webview_response("kb-a", Some(&doc_store));
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
/// exists (`collab.enabled=false`) is a distinct condition and must get its own
/// clear error, not a panic or a page whose every request then fails.
#[tokio::test]
async fn webview_with_no_doc_store_gets_a_clean_service_unavailable() {
    let resp = render_webview_response("kb-anything", None);
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
}
