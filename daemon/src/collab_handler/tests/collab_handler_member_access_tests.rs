use super::*;

#[tokio::test]
async fn kb_node_fetch_serves_members_denies_nonmembers() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbnf",
        "alice",
        &mut docs,
    )
    .await;
    let fetch = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/node_fetch",
            "params":{"kb_id":"kbnf","node_id":"concept:n"}});

    // Owner (a member) gets state + sv.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        fetch.clone(),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "owner fetch ok: {:?}", resp.error);
    let result = resp.result.expect("result present");
    assert!(result.get("state").and_then(|v| v.as_str()).is_some());
    assert!(result.get("sv").and_then(|v| v.as_str()).is_some());

    // A non-member is denied.
    let denied = dispatch_as(
        &store,
        &bc,
        Some("carol"),
        Some(&fp("carol")),
        fetch,
        &mut docs,
    )
    .await;
    assert!(denied.error.is_some(), "non-member fetch must be denied");
}

#[tokio::test]
async fn only_owner_manages_members() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbm3",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbm3", &fp("bob"), None),
        &mut docs,
    )
    .await;
    // bob (editor, non-owner) cannot add carol.
    let denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_member_msg("kb/add_member", "kbm3", &fp("carol"), None),
        &mut docs,
    )
    .await;
    assert!(denied.error.is_some(), "non-owner must not manage members");

    // bob (editor, non-owner) likewise cannot change the join policy — the
    // same owner-only Manage gate (kb_access) covers set_policy.
    let policy_denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_policy_msg("kbm3", "permissive"),
        &mut docs,
    )
    .await;
    assert!(
        policy_denied.error.is_some(),
        "non-owner must not change the join policy"
    );
}

#[tokio::test]
async fn pending_then_approve_allows_join() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kba",
        "alice",
        &mut docs,
    )
    .await;
    // bob requests (invite default) → pending.
    dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kba"),
        &mut docs,
    )
    .await;
    // owner approves as editor.
    let ok = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_approve_msg("kba", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    assert!(ok.error.is_none(), "owner approve succeeds: {:?}", ok.error);
    let coll = load_coll(&store, "kba").await;
    assert!(coll.pending().is_empty(), "approval clears pending");
    assert_eq!(coll.role_of(&fp("bob")), Some(SyncRole::Editor));
    // bob now joins as a member.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kba"),
        &mut docs
    )
    .await
    .error
    .is_none());
}

#[tokio::test]
async fn label_collision_two_keys_distinct_principals() {
    // Two peers share the same display label but have distinct principals — the
    // member added by one is NOT the other (no label-based impersonation).
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbc",
        "alice",
        &mut docs,
    )
    .await;
    // owner adds principal A under label "dupe".
    let a = "SHA256:keyA";
    let b = "SHA256:keyB";
    dispatch_as(&store, &bc, Some("alice"), Some(&fp("alice")),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/add_member","params":{"kb_id":"kbc","member":a,"role":"editor","label":"dupe"}}), &mut docs).await;
    let coll = load_coll(&store, "kbc").await;
    assert_eq!(coll.role_of(a), Some(SyncRole::Editor));
    assert_eq!(
        coll.role_of(b),
        None,
        "a different key with the same label is NOT a member"
    );
}

#[tokio::test]
async fn raw_collection_write_smuggling_denied() {
    // A non-owner cannot escalate by sending a raw `kbc:` sync/update that
    // grants itself ownership — the membership-smuggling defense.
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbs",
        "alice",
        &mut docs,
    )
    .await;
    let mut coll = load_coll(&store, "kbs").await;
    let evil = coll.upsert_member(&fp("bob"), "bob", SyncRole::Owner);
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
            "params":{"doc":"kbc:kbs","update":update_to_base64(&evil)}});
    let denied = dispatch_as(&store, &bc, Some("bob"), Some(&fp("bob")), msg, &mut docs).await;
    assert!(
        denied.error.is_some(),
        "non-owner raw collection write must be denied"
    );
    let after = load_coll(&store, "kbs").await;
    assert_eq!(
        after.role_of(&fp("bob")),
        None,
        "smuggled membership must not apply"
    );
}

#[tokio::test]
async fn none_mode_not_gated() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(&store, &bc, None, None, "kbn", "alice", &mut docs).await;
    assert!(
        dispatch_as(&store, &bc, None, None, kb_join_msg("kbn"), &mut docs)
            .await
            .error
            .is_none(),
        "none/loopback sessions are connection-trusted (dev only)"
    );
}
