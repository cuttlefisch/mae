//! ADVERSARIAL: the method families that reach the doc store without asking
//! `kb_access`.
//!
//! Third sibling to `collab_handler_cross_kb_node_isolation_tests` and
//! `collab_handler_cross_kb_role_isolation_tests`. Those two pin the property
//! for `kb/node_fetch` and for roles. This file pins it for the two surfaces
//! that were never swept: the `docs/*` family, and `sync/update`.
//!
//! The pattern is a repeat, and the repetition is the point. The node-isolation
//! file's own header records the previous instance: `deny_kb_doc_read` "existed
//! and was correct — it was called from exactly TWO of the paths that needed
//! it". That audit fixed `sync/state_vector`, `sync/full_state`, `sync/resync`
//! and `sync/diff`, and stopped. `docs/content` returns the same bytes under a
//! different method name, with no principal passed to the handler at all.
//!
//! So these tests are deliberately written against the *behaviour*, not against
//! any particular guard, because the fix is a dispatcher-level classification
//! rather than seven more call sites. If a future refactor moves the check, the
//! tests should still hold.

use super::*;

/// Seed a KB owned by `owner`, containing one node with real content.
async fn seed_kb_with_node(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    owner: &str,
    kb_id: &str,
    node_id: &str,
    body: &str,
) -> HashSet<String> {
    let mut docs = HashSet::new();
    kb_share_as(
        store,
        bc,
        Some(owner),
        Some(&fp(owner)),
        kb_id,
        owner,
        &mut docs,
    )
    .await;

    let added = dispatch_as(
        store,
        bc,
        Some(owner),
        Some(&fp(owner)),
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
            "params":{"kb_id":kb_id,"node_id":node_id,"title":"secret"}}),
        &mut docs,
    )
    .await;
    assert!(added.error.is_none(), "seed failed: {:?}", added.error);

    // Put real content in the node, as the legitimate owner, so the leak test
    // below is reading genuine plaintext rather than an empty document.
    let doc_name = format!("kbn:{kb_id}:{node_id}");
    let update = make_test_node(node_id, "secret", body, &["confidential"]);
    let wrote = dispatch_as(
        store,
        bc,
        Some(owner),
        Some(&fp(owner)),
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"sync/update",
            "params":{"doc":doc_name,"kb_id":kb_id,"update":update_to_base64(&update)}}),
        &mut docs,
    )
    .await;
    assert!(
        wrote.error.is_none(),
        "seeding content failed: {:?}",
        wrote.error
    );
    docs
}

/// S1: `docs/content` must not serve a KB document.
///
/// **Scoped to the property MAE actually provides.** A KB *node body* does not
/// come back through this call today even without a guard, because
/// `DocStore::content` delegates to `TextSync::content()`, which reads the
/// top-level `TEXT_NAME` root, while `KbNodeDoc` nests its body as a
/// `TextPrelim` inside a root `Y.Map`. That is an accident of schema, not a
/// control — it would evaporate the moment either shape changed — so the
/// property is pinned here explicitly rather than left to coincidence.
///
/// What is deliberately NOT asserted: that a plain collaborative buffer is
/// protected. Plain buffers have no membership model, and `sync/full_state` on
/// one is equally ungated, so any check here is a step rather than a boundary.
/// See the note on `method_authz::authorize_named_doc`.
#[tokio::test]
async fn docs_content_cannot_read_a_kb_node_document() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    const SECRET: &str = "SALARY-BANDS-Q3-DO-NOT-DISTRIBUTE";
    let _victim_docs =
        seed_kb_with_node(&store, &bc, "victim", "kb-b", "concept:b-secret", SECRET).await;

    // Mallory owns kb-a. Deliberately the highest role available: if an owner
    // of one KB cannot reach another KB's node this way, no lesser role can.
    let mut mallory_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;

    let attack = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({
            "jsonrpc":"2.0","id":9,"method":"docs/content",
            "params":{"doc":"kbn:kb-b:concept:b-secret"}}),
        &mut mallory_docs,
    )
    .await;

    let err = attack
        .error
        .as_ref()
        .expect("docs/content on a KB node must be refused, not merely empty");
    // The denial must not distinguish "exists, not yours" from "does not exist",
    // or it becomes an existence oracle over every tenant's node ids.
    assert!(
        !err.message.contains("kb-b") || err.message.contains("is not available"),
        "refusal text leaks more than it should: {}",
        err.message
    );
    let leaked = serde_json::to_string(&attack.result).unwrap_or_default();
    assert!(
        !leaked.contains(SECRET),
        "content came back anyway: {leaked}"
    );

    // Same refusal for a collection manifest, which carries membership.
    let coll_attack = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({
            "jsonrpc":"2.0","id":10,"method":"docs/content","params":{"doc":"kbc:kb-b"}}),
        &mut mallory_docs,
    )
    .await;
    assert!(
        coll_attack.error.is_some(),
        "docs/content served a collection manifest: {:?}",
        coll_attack.result
    );

    // POSITIVE CONTROL: an ordinary buffer still works, so this is a KB-address
    // refusal and not a blanket break of `docs/content`.
    let sync = TextSync::new("ordinary shared buffer");
    let mut victim_buf = HashSet::new();
    dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        serde_json::json!({
            "jsonrpc":"2.0","id":11,"method":"sync/update",
            "params":{"doc":"notes.org","update":update_to_base64(&sync.encode_state())}}),
        &mut victim_buf,
    )
    .await;
    let ok = dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        serde_json::json!({
            "jsonrpc":"2.0","id":12,"method":"docs/content","params":{"doc":"notes.org"}}),
        &mut victim_buf,
    )
    .await;
    assert_eq!(
        ok.result.as_ref().and_then(|r| r["content"].as_str()),
        Some("ordinary shared buffer"),
        "plain-buffer collaboration must be unaffected: {:?}",
        ok.error
    );
}

/// S1a: `docs/list` enumerates every document on the daemon, which for a
/// multi-tenant host is the full set of KB ids and node ids — the structure of
/// every tenant's corpus, even where the bodies are unreadable.
#[tokio::test]
async fn docs_list_cannot_enumerate_another_tenants_documents() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let _victim_docs =
        seed_kb_with_node(&store, &bc, "victim", "kb-b", "concept:b-secret", "content").await;

    let mut mallory_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;

    let attack = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({"jsonrpc":"2.0","id":9,"method":"docs/list","params":{}}),
        &mut mallory_docs,
    )
    .await;

    let listed = serde_json::to_string(&attack.result).unwrap_or_default();
    assert!(
        !listed.contains("kb-b"),
        "docs/list disclosed another tenant's KB and node ids: {listed}"
    );
}

/// S1b: `docs/delete` destroys any document by name, including a collection
/// manifest — which is the KB's membership and node index.
#[tokio::test]
async fn docs_delete_cannot_destroy_another_kbs_collection_manifest() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let _victim_docs =
        seed_kb_with_node(&store, &bc, "victim", "kb-b", "concept:b-secret", "content").await;

    let mut mallory_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;

    let attack = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({
            "jsonrpc":"2.0","id":9,"method":"docs/delete",
            "params":{"doc":"kbc:kb-b"}}),
        &mut mallory_docs,
    )
    .await;
    assert!(
        attack.error.is_some(),
        "a non-member deleted another KB's collection manifest: {:?}",
        attack.result
    );

    // The load-bearing assertion: the manifest must still resolve the node.
    // Asserting only on the response would pass against a handler that errored
    // *after* deleting.
    let coll = load_coll(&store, "kb-b").await;
    assert!(
        coll.has_node("concept:b-secret"),
        "the collection manifest was destroyed even though the call reported an error"
    );
}

/// S2: on the Hub transport an UNSIGNED `sync/update` to a KB node skips
/// `verify_relayed_content_op` (which returns `Ok(None)` when `require_signed`
/// is false), which leaves `sync_content_header` `None`, which means the
/// `if let Some(author)` epoch-fence block never runs — and `kb_access` is not
/// called on this path at all.
///
/// `require_signed` is `matches!(transport, Transport::P2p)`, and `dispatch_as`
/// uses `Transport::Hub`, so this is the default deployment, not an edge case.
#[tokio::test]
async fn unsigned_sync_update_from_a_non_member_cannot_write_a_kb_node() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    const LEGIT: &str = "the owner's real content";
    let _victim_docs =
        seed_kb_with_node(&store, &bc, "victim", "kb-b", "concept:b-secret", LEGIT).await;

    // Mallory is not a member of kb-b in any role. She owns an unrelated KB
    // purely so she is an authenticated principal rather than an anonymous one.
    let mut mallory_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;

    const TAMPERED: &str = "MALLORY-WAS-HERE";
    let evil = make_test_node("concept:b-secret", "owned", TAMPERED, &["pwned"]);
    let attack = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({
            "jsonrpc":"2.0","id":9,"method":"sync/update",
            "params":{"doc":"kbn:kb-b:concept:b-secret","kb_id":"kb-b",
                      "update":update_to_base64(&evil)}}),
        &mut mallory_docs,
    )
    .await;
    assert!(
        attack.error.is_some(),
        "a non-member wrote another KB's node via an unsigned sync/update: {:?}",
        attack.result
    );

    // The selective oracle: the STORE, not the response. A denial that still
    // applied the update is the failure this test exists to catch.
    let after = store
        .content("kbn:kb-b:concept:b-secret")
        .await
        .expect("node doc readable");
    assert!(
        !after.contains(TAMPERED),
        "the update was applied despite the error response — content is now: {after}"
    );
}
