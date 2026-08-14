//! ADR-067 Phase D2 adversarial coverage: `kb/query.*` wired into the mTLS
//! collab handler (`handle_doc_request_inner`'s new dispatch arm,
//! `collab_handler/mod.rs`), reusing `crate::kb_query::dispatch` unchanged.
//! Closes the gap where ADR-053's own Decision-1 prose claimed mTLS
//! reachability that was never actually implemented.
//!
//! Per the ADR's own Verification bar (`docs/adr/
//! 067-admin-enforced-live-query-only-kb-access.md`), this file proves,
//! specifically OVER THIS TRANSPORT (not assumed inherited from the OAuth
//! HTTPS listener's own `kb_query_tests.rs` suite):
//! (a) a QueryOnly member's `kb/query.search` succeeds and reflects LIVE
//!     content (the ADR-062 Hard Rule -- never a mirrored/cached copy);
//! (b) a principal absent from the KB's signed op-log is denied, never
//!     silently treated as a permissive non-member case; and
//! (c) `KbOp::Edit`-shaped methods stay unreachable via the `kb/query.*`
//!     prefix -- wiring it in did not broaden what a Read-only principal can
//!     reach over this same dispatch match.

use super::*;

/// Real owner Identity (signing material needed for the signed op-log) +
/// one QueryOnly-policy viewer (a synthetic fingerprint is sufficient for a
/// non-signing member -- the OWNER signs admission/role ops on their
/// behalf, matching `collab_handler_replication_policy_tests.rs`'s own
/// `setup_four_principal_kb` pattern).
async fn setup_owner_and_query_only_member(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    kb_id: &str,
    docs: &mut HashSet<String>,
) -> (String, [u8; 32], [u8; 32], String) {
    use mae_mcp::identity::Identity;

    let id = Identity::generate("owner");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    let owner_secret = id.secret_bytes();
    store.set_signer(Arc::new(id));

    kb_share_as(
        store,
        bc,
        Some("owner"),
        Some(&owner_fp),
        kb_id,
        "owner",
        docs,
    )
    .await;

    let query_only = fp("query-only-viewer");
    dispatch_as(
        store,
        bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", kb_id, &query_only, Some("viewer")),
        docs,
    )
    .await;
    set_replication_query_only(
        store,
        kb_id,
        &owner_secret,
        &owner_pubkey,
        &owner_fp,
        &query_only,
    )
    .await;

    // Anchor so `kb_access` derives roles (and ReplicationPolicy) from the
    // signed op-log -- ReplicationPolicy has no meaning on the legacy
    // `member_roles` path.
    store.set_kb_anchor(kb_id, owner_pubkey).await;

    (owner_fp, owner_pubkey, owner_secret, query_only)
}

fn kb_query_search_msg(kb_id: &str, query: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/query.search",
            "params":{"kb_id":kb_id,"query":query}})
}

/// A real, schema-conformant `KbNodeDoc` (title/body/tags/links), authored
/// under the owner's own epoch-derived client id -- `kb/query.search`/`get`
/// parse via `KbNodeDoc::from_bytes`, so a bare `TextSync` insert (which
/// produces a plain-text CRDT doc with none of that schema) would silently
/// fail to parse and be skipped, not found. `KbNodeDoc::encode()` is a full
/// `encode_state_as_update_v1` -- the same wire format `apply_update`
/// expects, so it applies cleanly as this node's very first update.
async fn owner_create_node_msg(
    kb_id: &str,
    owner_fp: &str,
    node_id: &str,
    body: &str,
) -> serde_json::Value {
    use mae_sync::kb::KbNodeDoc;
    let cid = derive_kb_client_id(owner_fp, 0);
    let node = KbNodeDoc::new_with_client_id(node_id, node_id, body, &[], cid);
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":node_id,"update":update_to_base64(&node.encode())}})
}

/// A real, schema-preserving REPLACE of `node_id`'s body (deletes the old
/// text, inserts the new -- see `KbNodeDoc::set_body`), continuing the
/// node's actual current state under the owner's SAME epoch-derived client
/// id (loading via `from_bytes_with_client_id`, not a fresh `new_doc()`,
/// which would author under a different, unrelated client id and fail the
/// daemon's epoch fence).
async fn owner_replace_body_msg(
    store: &Arc<DocStore>,
    kb_id: &str,
    owner_fp: &str,
    node_id: &str,
    new_body: &str,
) -> serde_json::Value {
    use mae_sync::kb::KbNodeDoc;
    let node_doc = mae_sync::kb_node_doc_name(kb_id, node_id);
    let (state, _sv) = store.encode_state_and_sv(&node_doc).await.unwrap();
    let cid = derive_kb_client_id(owner_fp, 0);
    let mut node = KbNodeDoc::from_bytes_with_client_id(&state, cid).unwrap();
    let update = node.set_body(new_body);
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":node_id,"update":update_to_base64(&update)}})
}

#[tokio::test]
async fn mtls_query_only_member_search_reflects_live_content_changes() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (owner_fp, _owner_pk, _owner_sk, query_only_fp) =
        setup_owner_and_query_only_member(&store, &bc, "kbmtls1", &mut docs).await;

    // Owner creates the node the QueryOnly member will read.
    let create_msg =
        owner_create_node_msg("kbmtls1", &owner_fp, "concept:n", "marker-round-one").await;
    let create = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        create_msg,
        &mut docs,
    )
    .await;
    assert!(
        create.error.is_none(),
        "owner node create failed: {:?}",
        create.error
    );

    // `kb/query.search` scans the collection's manifest (`coll.list_nodes()`),
    // not the raw doc store -- register the node there too, exactly as a
    // real client does via `kb/collection_node_add` after creating content.
    let manifest = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
                "params":{"kb_id":"kbmtls1","node_id":"concept:n","title":"n"}}),
        &mut docs,
    )
    .await;
    assert!(
        manifest.error.is_none(),
        "owner manifest registration failed: {:?}",
        manifest.error
    );

    // The QueryOnly member's own mTLS-authenticated session dispatches
    // kb/query.search over the SAME `handle_doc_request_inner` path a real
    // collab connection would use.
    let mut viewer_docs = HashSet::new();
    let first = dispatch_as(
        &store,
        &bc,
        Some("query-only-viewer"),
        Some(&query_only_fp),
        kb_query_search_msg("kbmtls1", "marker-round-one"),
        &mut viewer_docs,
    )
    .await;
    assert!(
        first.error.is_none(),
        "QueryOnly member's kb/query.search must succeed over mTLS: {:?}",
        first.error
    );
    let results = first.result.unwrap()["results"].as_array().unwrap().clone();
    assert_eq!(
        results.len(),
        1,
        "expected exactly the seeded node to match: {results:?}"
    );

    // Before the live edit, round-two's marker must NOT be findable --
    // establishes the baseline a stale/mirrored result would fail to move
    // away from.
    let before = dispatch_as(
        &store,
        &bc,
        Some("query-only-viewer"),
        Some(&query_only_fp),
        kb_query_search_msg("kbmtls1", "marker-round-two"),
        &mut viewer_docs,
    )
    .await;
    let before_results = before.result.unwrap()["results"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        before_results.is_empty(),
        "round-two marker must not exist yet: {before_results:?}"
    );

    // Owner REPLACES the node's body -- a real CRDT delete-old/insert-new,
    // not an append (see `KbNodeDoc::set_body`), so the old marker genuinely
    // stops existing.
    let update_msg = owner_replace_body_msg(
        &store,
        "kbmtls1",
        &owner_fp,
        "concept:n",
        "marker-round-two",
    )
    .await;
    let update = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        update_msg,
        &mut docs,
    )
    .await;
    assert!(
        update.error.is_none(),
        "owner live edit failed: {:?}",
        update.error
    );

    // The Hard Rule (ADR-062): a THIRD search through this exact mTLS
    // transport must reflect the change immediately -- never a cached
    // snapshot taken at the first search above.
    let after = dispatch_as(
        &store,
        &bc,
        Some("query-only-viewer"),
        Some(&query_only_fp),
        kb_query_search_msg("kbmtls1", "marker-round-two"),
        &mut viewer_docs,
    )
    .await;
    assert!(
        after.error.is_none(),
        "QueryOnly member's second search must succeed: {:?}",
        after.error
    );
    let after_results = after.result.unwrap()["results"].as_array().unwrap().clone();
    assert_eq!(
        after_results.len(),
        1,
        "the second search must reflect the NEW live content -- proving the \
         self-pointing/mTLS kb/query.search path inherits ADR-062's 'never \
         mirror, always query live' guarantee: {after_results:?}"
    );

    // The OLD marker must now be genuinely gone (not just superseded in
    // ranking) -- a mirrored/cached copy of the first search's result set
    // would still return it.
    let stale = dispatch_as(
        &store,
        &bc,
        Some("query-only-viewer"),
        Some(&query_only_fp),
        kb_query_search_msg("kbmtls1", "marker-round-one"),
        &mut viewer_docs,
    )
    .await;
    let stale_results = stale.result.unwrap()["results"].as_array().unwrap().clone();
    assert!(
        stale_results.is_empty(),
        "the OLD content must no longer be findable once its node's body was \
         replaced: {stale_results:?}"
    );
}

#[tokio::test]
async fn mtls_non_member_principal_is_denied_kb_query_search() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (_owner_fp, _pk, _sk, _query_only_fp) =
        setup_owner_and_query_only_member(&store, &bc, "kbmtls2", &mut docs).await;

    let stranger_fp = fp("total-stranger");
    let mut stranger_docs = HashSet::new();
    let resp = dispatch_as(
        &store,
        &bc,
        Some("total-stranger"),
        Some(&stranger_fp),
        kb_query_search_msg("kbmtls2", "anything"),
        &mut stranger_docs,
    )
    .await;
    assert!(
        resp.error.is_some(),
        "a principal absent from the KB's signed op-log must be denied \
         kb/query.search over the mTLS transport, never silently treated as \
         a permissive non-member case (ADR-067 Phase D's own Verification \
         bar #2, locked in specifically for this transport)"
    );
}

#[tokio::test]
async fn mtls_kb_query_prefix_never_reaches_an_edit_shaped_method() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (_owner_fp, _pk, _sk, query_only_fp) =
        setup_owner_and_query_only_member(&store, &bc, "kbmtls3", &mut docs).await;

    // A write-shaped method under the "kb/query." prefix does not exist in
    // `handle_doc_request_inner`'s dispatch match (only the five real
    // `kb/query.*` read methods are wired) -- it must be rejected as an
    // unknown method entirely, never silently routed anywhere that mutates.
    let mut viewer_docs = HashSet::new();
    let smuggled = dispatch_as(
        &store,
        &bc,
        Some("query-only-viewer"),
        Some(&query_only_fp),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/query.node_update",
                "params":{"kb_id":"kbmtls3","node_id":"concept:n","update":"AAAA"}}),
        &mut viewer_docs,
    )
    .await;
    assert!(
        smuggled.error.is_some(),
        "an unrecognized 'kb/query.*'-prefixed method must be rejected, not \
         silently accepted"
    );

    // The REAL edit method, `kb/node_update`, remains gated by its own
    // pre-existing Edit-role check (ADR-018) -- wiring `kb/query.*` in
    // alongside it in the same dispatch match must not have broadened what
    // a QueryOnly/Read-only principal can reach.
    let real_edit = dispatch_as(
        &store,
        &bc,
        Some("query-only-viewer"),
        Some(&query_only_fp),
        kb_node_update_msg_as("kbmtls3", &query_only_fp, 0, "SHOULD-NOT-APPLY"),
        &mut viewer_docs,
    )
    .await;
    assert!(
        real_edit.error.is_some(),
        "a QueryOnly/Read-only principal must still be denied the real \
         kb/node_update edit method over this same mTLS dispatch path: {:?}",
        real_edit.error
    );
}
