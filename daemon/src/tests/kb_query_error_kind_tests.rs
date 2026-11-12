//! `kb/query.*` refusal kinds on the HTTPS listener's own path
//! (`oauth::route_authenticated_request`, the function `handle_request` calls)
//! and the ones only an E2E KB reaches. The collab-socket path and the
//! membership/node kinds are covered by `collab_handler_error_kind_tests`.
//!
//! Also holds `docs/DAEMON_ADMIN.md`'s error-kind table to
//! `mae_mcp::protocol::ERROR_KINDS`, both ways, so the documented contract
//! cannot drift from what the daemon can return.

use std::sync::Arc;

use mae_mcp::identity::Identity;
use mae_mcp::protocol::JsonRpcRequest;
use serde_json::{json, Value};

use mae_daemon::kb_query;

use super::kb_query_tests::{fresh_doc_store, generous_limits, seed_e2e_kb, seed_unencrypted_kb};

fn principal(p: &str) -> crate::oauth::ValidatedPrincipal {
    crate::oauth::ValidatedPrincipal {
        principal: p.to_string(),
        audience: vec!["https://mae.example.com/mcp".to_string()],
        expires_at: 9_999_999_999,
        issuer: String::new(),
        subject: String::new(),
    }
}

fn config(kb_query_enabled: bool) -> crate::oauth::ResourceServerConfig {
    crate::oauth::ResourceServerConfig {
        canonical_resource_uri: "https://mae.example.com/mcp".to_string(),
        principal_claim: "sub".to_string(),
        jwks_url: "https://unused.example.com/jwks".to_string(),
        issuer: None,
        kb_query_enabled,
        max_request_body_bytes: 1_048_576,
        kb_query_max_body_bytes: 65_536,
        kb_query_max_scan_nodes: 500,
        kb_query_max_search_results: 20,
        webview_enabled: false,
    }
}

fn get_rpc() -> JsonRpcRequest {
    JsonRpcRequest {
        jsonrpc: "2.0".to_string(),
        id: json!(1),
        method: "kb/query.get".to_string(),
        params: Some(json!({"kb_id": "kb1", "node_id": "n1"})),
    }
}

struct Refuses;

impl mae_daemon::quota::QuotaCharger for Refuses {
    fn charge(&self, _: Option<&str>, _: &str) -> Result<mae_daemon::quota::QuotaLease, String> {
        Err("tenant quota exceeded for 'someone'".to_string())
    }
}

fn kind(response: &Value) -> &Value {
    &response["error"]["data"]["kind"]
}

#[tokio::test]
async fn the_https_listener_refusals_carry_their_kinds() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    seed_unencrypted_kb(&doc_store, &owner, "kb1", None, "n1", "T", "B", &[]).await;
    let who = principal("oauth:someone@example.com");
    let no_quota = &mae_daemon::quota::NoQuota;
    let route = crate::oauth::route_authenticated_request;

    let off = route(
        Some(get_rpc()),
        &config(false),
        Some(&doc_store),
        &who,
        no_quota,
    )
    .await;
    assert_eq!(kind(&off), "kb_query_unavailable", "{off}");
    let no_store = route(Some(get_rpc()), &config(true), None, &who, no_quota).await;
    assert_eq!(kind(&no_store), "kb_query_unavailable", "{no_store}");
    let throttled = route(
        Some(get_rpc()),
        &config(true),
        Some(&doc_store),
        &who,
        &Refuses,
    )
    .await;
    assert_eq!(kind(&throttled), "tenant_quota_exceeded", "{throttled}");
    // A principal with no membership, through the listener's whole path.
    let stranger = route(
        Some(get_rpc()),
        &config(true),
        Some(&doc_store),
        &who,
        no_quota,
    )
    .await;
    assert_eq!(kind(&stranger), "not_member", "{stranger}");
}

#[tokio::test]
async fn e2e_and_unauthenticated_refusals_carry_their_kinds() {
    let doc_store = fresh_doc_store().await;
    let owner = Arc::new(Identity::generate("owner"));
    let member = Identity::generate("member");
    let who = "oauth:alice@example.com";
    seed_e2e_kb(&doc_store, &owner, "e2e-kb", who, &member, "n1", "t", "b").await;

    let search = json!({"kb_id": "e2e-kb", "query": "t"});
    let err = kb_query::dispatch(
        "kb/query.search",
        &search,
        &doc_store,
        Some(who),
        generous_limits(),
    )
    .await
    .expect_err("search on an E2E KB is refused");
    assert_eq!(err.data.as_deref().unwrap()["kind"], "e2e_unsupported");
    assert_eq!(err.code, -32600, "the code it always had");

    // With no principal the read gate admits (loopback trust) but there is no
    // one to look a wrapped key up for.
    let params = json!({"kb_id": "e2e-kb"});
    let err = kb_query::dispatch(
        "kb/query.my_wrapped_key",
        &params,
        &doc_store,
        None,
        generous_limits(),
    )
    .await
    .expect_err("no principal");
    assert_eq!(err.data.as_deref().unwrap()["kind"], "not_authenticated");
}

/// The kinds named in the first column of the "Error kinds" table.
fn documented_kinds() -> Vec<String> {
    let doc = include_str!("../../../docs/DAEMON_ADMIN.md");
    let table = doc
        .split("#### Error kinds")
        .nth(1)
        .expect("DAEMON_ADMIN.md has an \"Error kinds\" section");
    table
        .lines()
        .skip_while(|l| !l.starts_with("|--"))
        .skip(1) // the header row and its separator
        .take_while(|l| l.starts_with('|'))
        .filter_map(|l| l.split('`').nth(1).map(str::to_string))
        .collect()
}

#[test]
fn the_documented_error_kinds_are_exactly_the_protocol_list() {
    let documented = documented_kinds();
    let listed: Vec<String> = mae_mcp::protocol::ERROR_KINDS
        .iter()
        .map(|k| k.to_string())
        .collect();
    let (mut d, mut l) = (documented.clone(), listed.clone());
    d.sort();
    l.sort();
    assert_eq!(
        d, l,
        "docs/DAEMON_ADMIN.md's table and ERROR_KINDS disagree"
    );
    assert_eq!(documented.len(), listed.len(), "a kind is documented twice");
}
