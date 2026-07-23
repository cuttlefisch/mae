//! ADR-053/Phase G (#382) adversarial + positive tests for `kb/query.*`.
//!
//! Tests at the `kb_query::dispatch` level (real `DocStore`, real crypto, real
//! principal strings) rather than driving a full live HTTPS+bearer-token round
//! trip end to end: the HTTP/TLS/JWT transport layer (`validate_bearer_token`,
//! `load_tls_config`, PRM metadata) is already covered by Phase F's own
//! adversarial suite (`daemon/src/oauth.rs`'s `#[cfg(test)] mod tests`) — this
//! file is scoped to the NEW security-relevant logic Phase G actually
//! introduces (encryption-aware branching, the Read-only access gate applied
//! to it, and the per-call caps), not re-proving the same transport mechanism
//! again. `handle_request` just JSON-serializes `dispatch`'s returned `Value`
//! into the HTTP body with no further transformation, so scanning that
//! `Value`'s serialized JSON for a secret substring is a faithful proxy for
//! "the raw HTTP response body" — the literal ADR-053 requirement.

use std::sync::Arc;

use mae_daemon::doc_store::DocStore;
use mae_daemon::storage::SqliteBackend;
use mae_mcp::identity::Identity;
use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};
use mae_sync::op_set;
use serde_json::json;

use crate::kb_query::{self, KbQueryLimits};

fn generous_limits() -> KbQueryLimits {
    KbQueryLimits {
        max_body_bytes: 65_536,
        max_scan_nodes: 500,
        max_search_results: 20,
    }
}

async fn fresh_doc_store() -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500))
}

/// Build + share an UNENCRYPTED KB (`kb_id`) owned by `owner`, with `member`
/// admitted at `role` (unsigned `member_roles` field — sufficient for an
/// owned, un-anchored KB's `kb_access` role derivation), transport-exposed
/// over `Transport::Hub`, seeded with one real node (`node_id`/`title`/`body`
/// and any `links`).
#[allow(clippy::too_many_arguments)]
async fn seed_unencrypted_kb(
    doc_store: &DocStore,
    owner: &Arc<Identity>,
    kb_id: &str,
    member: Option<(&str, Role)>,
    node_id: &str,
    title: &str,
    body: &str,
    links: &[String],
) {
    doc_store.set_signer(Arc::clone(owner));
    let mut coll = KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    if let Some((principal, role)) = member {
        coll.upsert_member(principal, "member", role);
    }
    let _ = coll.add_node(node_id, title); // manifest entry -- list_nodes()/search()/graph() read this
    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state())
        .await
        .unwrap();

    let mut node = KbNodeDoc::new(node_id, title, body, &[]);
    for link in links {
        let _ = node.add_link(link); // building via encode_state() below, not incrementally
    }
    doc_store
        .share_doc(&format!("kb:{node_id}"), &node.encode_state())
        .await
        .unwrap();
}

/// As [`seed_unencrypted_kb`], but the KB is E2E-encrypted (a real signed
/// genesis + owner self-wrap via `author_e2e_genesis`, a real member admit +
/// wrap via `author_member_admit`) and the seeded node's content is REAL
/// sealed op-set ciphertext (`op_set::seal_op`/`merge`), not a plaintext
/// `KbNodeDoc`. Returns the `ContentKey` so a test can decrypt as the member
/// would.
#[allow(clippy::too_many_arguments)]
async fn seed_e2e_kb(
    doc_store: &DocStore,
    owner: &Arc<Identity>,
    kb_id: &str,
    member_principal: &str,
    member: &Identity,
    node_id: &str,
    title: &str,
    body: &str,
) -> ContentKey {
    doc_store.set_signer(Arc::clone(owner));
    let mut coll = KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);

    let owner_secret = owner.secret_bytes();
    let owner_pub = owner.public().to_bytes();
    let key = ContentKey::generate();
    let self_wrap = wrap_to_member(&key, &wrap_public_for(&owner_secret)).unwrap();
    coll.author_e2e_genesis(
        kb_id,
        &owner.fingerprint(),
        &owner_secret,
        &owner_pub,
        self_wrap,
        1000,
    );

    let member_secret = member.secret_bytes();
    let member_pub = member.public().to_bytes();
    let member_wrap_pub = wrap_public_for(&member_secret);
    let member_wrap = wrap_to_member(&key, &member_wrap_pub).unwrap();
    coll.author_member_admit(
        kb_id,
        member_principal,
        &member_pub,
        &member_wrap_pub,
        Role::Viewer,
        "member",
        member_wrap,
        &owner.fingerprint(),
        &owner_secret,
        &owner_pub,
        1001,
    );
    // A real E2E KB's manifest title is blanked (`blank_node_titles_delta`) --
    // the daemon never legitimately holds a plaintext title to put there.
    // Only the node_id is real, matching production behavior.
    let _ = coll.add_node(node_id, "");

    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state())
        .await
        .unwrap();

    // Seal real content into a real op-set, exactly as an editing member would.
    let mut node = KbNodeDoc::new_with_client_id(node_id, "", "", &[], 7);
    let mut state: Vec<u8> = Vec::new();
    for plaintext in [
        node.encode_state(),
        node.set_title(title),
        node.set_body(body),
    ] {
        let (_op_id, outer) = op_set::seal_op(&state, &key, &plaintext, 7).unwrap();
        state = op_set::merge(&state, &outer).unwrap();
    }
    doc_store
        .share_doc(&format!("kb:{node_id}"), &state)
        .await
        .unwrap();

    key
}

fn extract_result(v: Result<serde_json::Value, mae_mcp::protocol::McpError>) -> serde_json::Value {
    v.unwrap_or_else(|e| panic!("expected Ok, got error: {e:?}"))
}

// ---------------------------------------------------------------------------
// The required hostile-hub-operator adversarial test.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hostile_hub_operator_cannot_search_an_e2e_kb_for_plaintext() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let member = Identity::generate("member");
    let member_principal = "oauth:alice@example.com";
    let secret_title = "TOP SECRET PROJECT CODENAME NIGHTINGALE";
    let secret_body = "the launch date is classified and must never leak to the daemon operator";
    seed_e2e_kb(
        &doc_store,
        &owner,
        "secret-kb",
        member_principal,
        &member,
        "n1",
        secret_title,
        secret_body,
    )
    .await;

    // 1. kb/query.search must structurally refuse -- never silently return
    //    empty (which would be indistinguishable from "no matches").
    let search_result = kb_query::dispatch(
        "kb/query.search",
        &json!({"kb_id": "secret-kb", "query": "nightingale"}),
        &doc_store,
        Some(member_principal),
        generous_limits(),
    )
    .await;
    assert!(
        search_result.is_err(),
        "server-side search on an E2E KB must be refused, not silently empty"
    );

    // 2. kb/query.get's raw response body must not contain the secret,
    //    byte-for-byte, anywhere -- the daemon literally cannot decrypt it.
    let get_result = extract_result(
        kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": "secret-kb", "node_id": "n1"}),
            &doc_store,
            Some(member_principal),
            generous_limits(),
        )
        .await,
    );
    let wire_bytes = serde_json::to_string(&get_result).unwrap();
    assert!(
        !wire_bytes.contains(secret_title),
        "the wire response must never contain the plaintext title: {wire_bytes}"
    );
    assert!(
        !wire_bytes.contains(secret_body),
        "the wire response must never contain the plaintext body: {wire_bytes}"
    );
    assert_eq!(get_result["encryption"], "e2e");
    assert!(
        get_result.get("ciphertext_b64").is_some(),
        "an E2E get must return opaque ciphertext, not plaintext fields"
    );

    // 3. kb/query.graph must report node existence only, no edges (links are
    //    encrypted alongside title/body -- the daemon has no separate
    //    plaintext link registry to leak instead).
    let graph_result = extract_result(
        kb_query::dispatch(
            "kb/query.graph",
            &json!({"kb_id": "secret-kb"}),
            &doc_store,
            Some(member_principal),
            generous_limits(),
        )
        .await,
    );
    assert_eq!(graph_result["edges"].as_array().unwrap().len(), 0);
    assert_eq!(graph_result["nodes"].as_array().unwrap().len(), 1);
}

// ---------------------------------------------------------------------------
// The genuine positive case a lazy-fetch client would exercise.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_member_can_decrypt_kb_query_get_and_cache_it_via_the_lazy_fetch_client() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let member = Identity::generate("member");
    let member_principal = "oauth:bob@example.com";
    let key = seed_e2e_kb(
        &doc_store,
        &owner,
        "team-kb",
        member_principal,
        &member,
        "n1",
        "Real Title",
        "real body content",
    )
    .await;

    let get_result = extract_result(
        kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": "team-kb", "node_id": "n1"}),
            &doc_store,
            Some(member_principal),
            generous_limits(),
        )
        .await,
    );
    let ciphertext_b64 = get_result["ciphertext_b64"].as_str().unwrap();

    let client = crate::lazy_fetch_client::LazyFetchClient::new(50);
    let decrypted = client
        .decrypt_and_cache(member_principal, "n1", ciphertext_b64, &key)
        .expect("the real member key must open the fetched ciphertext");
    assert_eq!(decrypted.title, "Real Title");
    assert_eq!(decrypted.body, "real body content");

    let cached = client
        .get_cached(member_principal, "n1")
        .expect("must be a cache hit after decrypt_and_cache");
    assert_eq!(cached.title, "Real Title");
}

// ---------------------------------------------------------------------------
// Unencrypted-KB positive case + the search-cap adversarial test.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn thin_client_with_no_replica_reads_an_unencrypted_kb_it_has_viewer_access_to() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let viewer_principal = "oauth:viewer@example.com";
    seed_unencrypted_kb(
        &doc_store,
        &owner,
        "public-kb",
        Some((viewer_principal, Role::Viewer)),
        "n1",
        "Public Node",
        "public content mentioning rust",
        &["n2".to_string()],
    )
    .await;

    let caps = extract_result(
        kb_query::dispatch(
            "kb/query.capabilities",
            &json!({"kb_id": "public-kb"}),
            &doc_store,
            Some(viewer_principal),
            generous_limits(),
        )
        .await,
    );
    assert_eq!(caps["encryption"], "none");
    assert_eq!(caps["searchable"], true);

    let get_result = extract_result(
        kb_query::dispatch(
            "kb/query.get",
            &json!({"kb_id": "public-kb", "node_id": "n1"}),
            &doc_store,
            Some(viewer_principal),
            generous_limits(),
        )
        .await,
    );
    assert_eq!(get_result["title"], "Public Node");
    assert_eq!(get_result["body"], "public content mentioning rust");

    let search_result = extract_result(
        kb_query::dispatch(
            "kb/query.search",
            &json!({"kb_id": "public-kb", "query": "rust"}),
            &doc_store,
            Some(viewer_principal),
            generous_limits(),
        )
        .await,
    );
    let results = search_result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["id"], "n1");

    let graph_result = extract_result(
        kb_query::dispatch(
            "kb/query.graph",
            &json!({"kb_id": "public-kb"}),
            &doc_store,
            Some(viewer_principal),
            generous_limits(),
        )
        .await,
    );
    assert_eq!(graph_result["edges"][0], json!(["n1", "n2"]));
}

#[tokio::test]
async fn unencrypted_kb_search_is_capped_and_cannot_full_dump() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let viewer_principal = "oauth:viewer@example.com";

    // Seed the collection with far more nodes than the configured cap.
    doc_store.set_signer(Arc::clone(&owner));
    let mut coll = KbCollectionDoc::new_owned("big-kb", &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(viewer_principal, "viewer", Role::Viewer);
    doc_store
        .share_doc("kbc:big-kb", &coll.encode_state())
        .await
        .unwrap();
    const N: usize = 40;
    for i in 0..N {
        let node = KbNodeDoc::new(&format!("n{i}"), "Matching Title", "matches the query", &[]);
        doc_store
            .share_doc(&format!("kb:n{i}"), &node.encode_state())
            .await
            .unwrap();
    }

    let tight_limits = KbQueryLimits {
        max_body_bytes: 65_536,
        max_scan_nodes: 10, // « N
        max_search_results: 5,
    };
    let search_result = extract_result(
        kb_query::dispatch(
            "kb/query.search",
            &json!({"kb_id": "big-kb", "query": "matches", "limit": 100}),
            &doc_store,
            Some(viewer_principal),
            tight_limits,
        )
        .await,
    );
    let results = search_result["results"].as_array().unwrap();
    assert!(
        results.len() <= 5,
        "result count must respect max_search_results even though every node matches: {results:?}"
    );
    let scanned = search_result["scanned"].as_u64().unwrap();
    assert!(
        scanned <= 10,
        "the server-side SCAN itself must be capped at max_scan_nodes, not just the \
         returned result count (a cap on results alone would still let 'search' full-dump \
         scan the whole KB) — got scanned={scanned}"
    );
}

// ---------------------------------------------------------------------------
// Access-gate adversarial tests.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_member_is_denied_regardless_of_encryption() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let member = Identity::generate("member");
    let stranger_principal = "oauth:stranger@example.com";

    seed_unencrypted_kb(
        &doc_store,
        &owner,
        "unenc-kb",
        None, // no member admitted at all
        "n1",
        "T",
        "B",
        &[],
    )
    .await;
    let unenc_result = kb_query::dispatch(
        "kb/query.get",
        &json!({"kb_id": "unenc-kb", "node_id": "n1"}),
        &doc_store,
        Some(stranger_principal),
        generous_limits(),
    )
    .await;
    assert!(
        unenc_result.is_err(),
        "a non-member must be denied on an unencrypted KB"
    );

    seed_e2e_kb(
        &doc_store,
        &owner,
        "enc-kb",
        "oauth:legit-member@example.com",
        &member,
        "n1",
        "T",
        "B",
    )
    .await;
    let enc_result = kb_query::dispatch(
        "kb/query.get",
        &json!({"kb_id": "enc-kb", "node_id": "n1"}),
        &doc_store,
        Some(stranger_principal),
        generous_limits(),
    )
    .await;
    assert!(
        enc_result.is_err(),
        "a non-member must be denied on an E2E KB too -- the gate fires before the \
         encryption branch, not as a side effect of it"
    );
}

#[tokio::test]
async fn kb_query_unreachable_when_disabled() {
    // Drives the real `oauth::route_authenticated_request` gating function
    // directly (extracted from `handle_request` specifically so this is
    // testable without a live HTTP connection) -- a valid, well-formed
    // kb/query.* JSON-RPC request must get a clear "disabled" error, DISTINCT
    // from "method not found", when `kb_query_enabled = false`, even with an
    // otherwise-valid principal and a real DocStore available.
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_unencrypted_kb(&doc_store, &owner, "kb1", None, "n1", "T", "B", &[]).await;

    let config = crate::oauth::ResourceServerConfig {
        canonical_resource_uri: "https://mae.example.com/mcp".to_string(),
        principal_claim: "sub".to_string(),
        jwks_url: "https://unused.example.com/jwks".to_string(),
        issuer: None,
        kb_query_enabled: false,
        kb_query_max_body_bytes: 65_536,
        kb_query_max_scan_nodes: 500,
        kb_query_max_search_results: 20,
    };
    let principal = mae_mcp_test_principal("oauth:someone@example.com");
    let rpc = mae_mcp::protocol::JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "kb/query.capabilities".to_string(),
        params: Some(json!({"kb_id": "kb1"})),
    };

    let response =
        crate::oauth::route_authenticated_request(Some(rpc), &config, Some(&doc_store), &principal)
            .await;

    let error = response
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
        .expect("a disabled kb_query must return a JSON-RPC error, not a result");
    assert!(
        error.contains("disabled"),
        "expected a clear 'disabled' error distinct from 'method not found', got: {error}"
    );
    assert!(response.get("result").is_none());
}

/// Build a `ValidatedPrincipal` directly — this file tests below the JWT
/// layer (see the module doc), so a real signed token isn't needed to
/// exercise `route_authenticated_request`'s own gating logic.
fn mae_mcp_test_principal(principal: &str) -> crate::oauth::ValidatedPrincipal {
    crate::oauth::ValidatedPrincipal {
        principal: principal.to_string(),
        audience: vec!["https://mae.example.com/mcp".to_string()],
        expires_at: 9_999_999_999,
    }
}
