use super::*;

#[tokio::test]
async fn share_ignores_claimed_creator_and_binds_owner_to_principal() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    // Authenticated principal = alice's key; claims creator "mallory" → SUCCEEDS,
    // owner bound to the principal (the I-7 reject is gone; the claim is ignored).
    let resp = kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb1",
        "mallory",
        &mut docs,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "claimed creator must be ignored, not rejected: {:?}",
        resp.error
    );
    let coll = load_coll(&store, "kb1").await;
    assert_eq!(coll.owner(), fp("alice"), "owner = verified principal");
    assert_eq!(coll.role_of(&fp("alice")), Some(SyncRole::Owner));
    assert_eq!(
        coll.role_of(&fp("mallory")),
        None,
        "spoofed name is not a member"
    );
}

#[tokio::test]
async fn anonymous_share_succeeds() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let resp = kb_share_as(&store, &bc, None, None, "kb3", "whoever", &mut docs).await;
    assert!(
        resp.error.is_none(),
        "anonymous (none) share must succeed: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn restrictive_nonmember_join_denied() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbr",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_policy_msg("kbr", "restrictive"),
        &mut docs,
    )
    .await;
    let denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbr"),
        &mut docs,
    )
    .await;
    assert!(
        denied.error.is_some(),
        "restrictive: non-member join denied"
    );
    assert!(denied.error.unwrap().message.contains("not a member"));
}

#[tokio::test]
async fn invite_nonmember_join_pending() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    // default policy = invite
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbi",
        "alice",
        &mut docs,
    )
    .await;
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbi"),
        &mut docs,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "invite join returns success+pending, not error"
    );
    assert_eq!(
        resp.result.as_ref().and_then(|r| r["status"].as_str()),
        Some("pending")
    );
    let coll = load_coll(&store, "kbi").await;
    assert_eq!(coll.pending().len(), 1, "join recorded as pending");
    assert_eq!(
        coll.role_of(&fp("bob")),
        None,
        "pending peer is not yet a member"
    );
}

#[tokio::test]
async fn permissive_nonmember_join_autoadds_viewer() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbp",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_policy_msg("kbp", "permissive"),
        &mut docs,
    )
    .await;
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbp"),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "permissive join succeeds");
    let coll = load_coll(&store, "kbp").await;
    assert_eq!(
        coll.role_of(&fp("bob")),
        Some(SyncRole::Viewer),
        "auto-granted least privilege"
    );
}

#[tokio::test]
async fn owner_add_member_then_join_and_edit() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbm2",
        "alice",
        &mut docs,
    )
    .await;
    // bob denied edit before being added.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg("kbm2"),
        &mut docs
    )
    .await
    .error
    .is_some());
    // owner adds bob (default editor).
    assert!(dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbm2", &fp("bob"), None),
        &mut docs
    )
    .await
    .error
    .is_none());
    // bob now joins (member) + edits.
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kbm2"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "member joins directly"
    );
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            // bob is a freshly-added editor ⇒ epoch 0; he authors under his
            // current-epoch client_id, which the ADR-023 fence accepts.
            kb_node_update_msg_as("kbm2", &fp("bob"), 0, "x"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "editor may edit"
    );
    // owner removes bob → next edit denied.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/remove_member", "kbm2", &fp("bob"), None),
        &mut docs
    )
    .await
    .error
    .is_none());
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg("kbm2"),
            &mut docs
        )
        .await
        .error
        .is_some(),
        "removed member denied"
    );
}
