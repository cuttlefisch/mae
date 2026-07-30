use super::*;

// ADR-067 Phase B: `kb_access` Join/Read split + `kb_join` enforcement.

/// Sets up an anchored KB with 4 real principals -- Owner, Editor, a Full-policy
/// Viewer, and a QueryOnly-policy Viewer -- all via the real signed op-log (not a
/// synthetic fixture), returning their fingerprints plus the owner's signing material
/// for the caller's own further mutations.
async fn setup_four_principal_kb(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    kb_id: &str,
    docs: &mut HashSet<String>,
) -> (String, [u8; 32], [u8; 32], String, String, String) {
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

    let editor = fp("editor");
    dispatch_as(
        store,
        bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", kb_id, &editor, Some("editor")),
        docs,
    )
    .await;

    let full_viewer = fp("full-viewer");
    dispatch_as(
        store,
        bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", kb_id, &full_viewer, Some("viewer")),
        docs,
    )
    .await;

    let query_only_viewer = fp("query-only-viewer");
    dispatch_as(
        store,
        bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", kb_id, &query_only_viewer, Some("viewer")),
        docs,
    )
    .await;
    set_replication_query_only(
        store,
        kb_id,
        &owner_secret,
        &owner_pubkey,
        &owner_fp,
        &query_only_viewer,
    )
    .await;

    // Anchor so kb_access derives from the signed op-log (ReplicationPolicy has no
    // meaning on the legacy member_roles path at all).
    store.set_kb_anchor(kb_id, owner_pubkey).await;

    (
        owner_fp,
        owner_pubkey,
        owner_secret,
        editor,
        full_viewer,
        query_only_viewer,
    )
}

#[tokio::test]
async fn query_only_viewer_denied_join_others_allowed_with_distinguishable_message() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (owner_fp, _owner_pk, _owner_sk, editor, full_viewer, query_only_viewer) =
        setup_four_principal_kb(&store, &bc, "kb4p", &mut docs).await;

    let access = |p: String| {
        let store = Arc::clone(&store);
        async move { kb_access(&store, "kb4p", Some(&p), KbOp::Join, Transport::Hub).await }
    };

    assert!(
        matches!(access(owner_fp.clone()).await, Ok(AccessDecision::Allow)),
        "owner may always join/replicate their own KB"
    );
    assert!(
        matches!(access(editor).await, Ok(AccessDecision::Allow)),
        "a plain editor (Full replication, the default) may join"
    );
    assert!(
        matches!(access(full_viewer).await, Ok(AccessDecision::Allow)),
        "a Full-policy viewer may join"
    );

    let denied = access(query_only_viewer).await;
    match denied {
        Ok(AccessDecision::Deny(msg)) => {
            assert!(
                msg.contains("live-query-only") || msg.contains("ADR-067"),
                "must name the actual reason (replication-restricted), not a generic denial: {msg}"
            );
            assert!(
                !msg.contains("not a member"),
                "a genuine, restricted MEMBER must never be told they're not a member \
                 at all -- that would be actively misleading: {msg}"
            );
        }
        other => panic!("expected a QueryOnly-specific Deny, got {other:?}"),
    }

    // The real non-member case, for contrast -- confirms the two denial messages are
    // genuinely distinguishable, not the same string coincidentally passing both
    // `contains` checks above. The KB's default join policy is Invite (a stranger's
    // join is Pending, not Deny), so switch to Restrictive first to force the actual
    // Deny path this test wants to contrast against.
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_policy_msg("kb4p", "restrictive"),
        &mut docs,
    )
    .await;
    let stranger_denied = access(fp("total-stranger")).await;
    assert!(
        matches!(stranger_denied, Ok(AccessDecision::Deny(ref m)) if m.contains("not a member")),
        "a genuine non-member (restrictive join policy) IS told they're not a member: {stranger_denied:?}"
    );
}

#[tokio::test]
async fn query_only_member_kb_join_never_subscribes_the_session() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (_owner_fp, _owner_pk, _owner_sk, _editor, full_viewer, query_only_viewer) =
        setup_four_principal_kb(&store, &bc, "kbsub", &mut docs).await;

    // Register two real sessions up front (subscribe() must be called before
    // add_event_sub/subscribe_doc can have any observable effect at all -- an
    // unregistered session's calls are silent no-ops, which would make this test
    // vacuously pass for the wrong reason if skipped).
    let denied_session = 100u64;
    let allowed_session = 200u64;
    let mut denied_rx = bc
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .subscribe(denied_session, vec![]);
    let mut allowed_rx = bc
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .subscribe(allowed_session, vec![]);

    // The QueryOnly member's kb_join must be denied and must never reach the
    // subscription step.
    let mut denied_docs = HashSet::new();
    let denied_resp = kb_membership::handle_kb_join(
        &store,
        &bc,
        denied_session,
        Some("query-only-viewer"),
        Some(&query_only_viewer),
        None,
        &mut denied_docs,
        Transport::Hub,
        serde_json::json!(1),
        &kb_join_msg("kbsub")["params"],
    )
    .await;
    assert!(denied_resp.error.is_some(), "QueryOnly join must be denied");

    // The Full-policy member's kb_join, by contrast, must succeed and subscribe.
    let mut allowed_docs = HashSet::new();
    let allowed_resp = kb_membership::handle_kb_join(
        &store,
        &bc,
        allowed_session,
        Some("full-viewer"),
        Some(&full_viewer),
        None,
        &mut allowed_docs,
        Transport::Hub,
        serde_json::json!(2),
        &kb_join_msg("kbsub")["params"],
    )
    .await;
    assert!(
        allowed_resp.error.is_none(),
        "Full-policy join must succeed: {:?}",
        allowed_resp.error
    );

    // Broadcast a real sync_update for the collection doc and confirm only the
    // ALLOWED session's channel actually received it.
    bc.lock()
        .unwrap_or_else(|e| e.into_inner())
        .broadcast(&EditorEvent::SyncUpdate {
            buffer_name: "kbc:kbsub".to_string(),
            update_base64: String::new(),
            wal_seq: 1,
            content_header: None,
        });

    assert!(
        denied_rx.try_recv().is_err(),
        "a denied QueryOnly join must never leave the session subscribed -- \
         it must not receive events for a KB it was refused replication access to"
    );
    assert!(
        allowed_rx.try_recv().is_ok(),
        "sanity check on the test technique itself: the ALLOWED session must \
         actually receive the broadcast, proving try_recv() would have caught \
         the denied session receiving one too if the gate were broken"
    );
}

#[tokio::test]
async fn mid_session_restriction_does_not_tear_down_an_already_live_session_but_blocks_future_joins(
) {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    use mae_mcp::identity::Identity;

    let id = Identity::generate("owner");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    let owner_secret = id.secret_bytes();
    store.set_signer(Arc::new(id));

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbmid",
        "owner",
        &mut docs,
    )
    .await;

    let member = fp("dana");
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbmid", &member, Some("viewer")),
        &mut docs,
    )
    .await;
    store.set_kb_anchor("kbmid", owner_pubkey).await;

    // Dana joins while still Full-policy -- a real, live, subscribed session.
    let session_id = 42u64;
    let mut rx = bc
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .subscribe(session_id, vec![]);
    let mut session_docs = HashSet::new();
    let join_resp = kb_membership::handle_kb_join(
        &store,
        &bc,
        session_id,
        Some("dana"),
        Some(&member),
        None,
        &mut session_docs,
        Transport::Hub,
        serde_json::json!(1),
        &kb_join_msg("kbmid")["params"],
    )
    .await;
    assert!(
        join_resp.error.is_none(),
        "initial Full-era join must succeed"
    );

    // The owner now restricts Dana to QueryOnly, AFTER the session above is already
    // live and subscribed.
    set_replication_query_only(
        &store,
        "kbmid",
        &owner_secret,
        &owner_pubkey,
        &owner_fp,
        &member,
    )
    .await;

    // Explicit, named limitation (not silently assumed): the ALREADY-established
    // session is not retroactively torn down. Session revocation is a distinct
    // mechanism this phase does not build.
    bc.lock()
        .unwrap_or_else(|e| e.into_inner())
        .broadcast(&EditorEvent::SyncUpdate {
            buffer_name: "kbc:kbmid".to_string(),
            update_base64: String::new(),
            wal_seq: 2,
            content_header: None,
        });
    assert!(
        rx.try_recv().is_ok(),
        "a live, already-subscribed session must NOT be retroactively torn down \
         by a later restriction -- this phase governs future kb_join calls only"
    );

    // But a FRESH kb_join attempt (e.g. a reconnect) by the same principal, after
    // the restriction, correctly hits the new policy.
    let mut reconnect_docs = HashSet::new();
    let reconnect_resp = kb_membership::handle_kb_join(
        &store,
        &bc,
        session_id + 1,
        Some("dana"),
        Some(&member),
        None,
        &mut reconnect_docs,
        Transport::Hub,
        serde_json::json!(2),
        &kb_join_msg("kbmid")["params"],
    )
    .await;
    assert!(
        reconnect_resp.error.is_some(),
        "a fresh kb_join AFTER the restriction must be denied, even for the same \
         principal whose earlier live session survives"
    );
}
