use super::*;

#[tokio::test]
async fn viewer_cannot_node_update() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbv",
        "alice",
        &mut docs,
    )
    .await;
    // add bob as VIEWER
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbv", &fp("bob"), Some("viewer")),
        &mut docs,
    )
    .await;
    // viewer may join/read but not edit.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbv"),
        &mut docs
    )
    .await
    .error
    .is_none());
    let denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg("kbv"),
        &mut docs,
    )
    .await;
    assert!(
        denied.error.is_some(),
        "viewer must not edit (least privilege)"
    );
}

#[tokio::test]
async fn viewer_era_edits_do_not_cascade_on_grant() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbx",
        "alice",
        &mut docs,
    )
    .await;

    // bob is added as a VIEWER — a fresh grant ⇒ epoch 0.
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("viewer")),
        &mut docs,
    )
    .await;

    // bob (viewer) authors an edit under his epoch-0 client_id and pushes it —
    // DENIED at the role gate. He keeps it in his local lineage (the cascade seed).
    let viewer_era = kb_node_update_msg_as("kbx", &fp("bob"), 0, "VIEWER-ERA");
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            viewer_era.clone(),
            &mut docs
        )
        .await
        .error
        .is_some(),
        "viewer edit denied at the role gate"
    );

    // Owner PROMOTES bob viewer→editor — a role change ⇒ bob's epoch bumps to 1.
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("editor")),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "owner promotes bob to editor"
    );

    // THE EXPLOIT: bob re-pushes his VIEWER-ERA op (still authored under epoch 0).
    // The role gate now passes (he is an editor), but the EPOCH FENCE must reject
    // it — the op is from his stale, pre-grant client_id.
    //
    // Strong no-cascade oracle: snapshot the canonical state BEFORE the fenced push
    // and assert it is BYTE-IDENTICAL after — a fenced op must perturb the
    // authoritative node by exactly zero bytes (stronger than a substring check).
    let (before, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        viewer_era.clone(),
        &mut docs,
    )
    .await;
    let msg = resp
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        resp.error.is_some() && msg.contains("rebase required"),
        "viewer-era lineage must be fenced on grant; got: {msg:?}"
    );

    // NO CASCADE: the canonical state is byte-identical (and, redundantly, never
    // contains the viewer-era edit).
    let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    assert_eq!(
        state, before,
        "a fenced op must leave the canonical node byte-identical (no cascade)"
    );
    let canonical = TextSync::from_state(&state).unwrap().content();
    assert!(
        !canonical.contains("VIEWER-ERA"),
        "pre-grant edit must not cascade; canonical = {canonical:?}"
    );

    // bob CAN make a fresh, current-epoch edit — that is accepted. Post-promotion
    // his epoch is an unpredictable token (#72), so read it from the collection
    // rather than assuming the old prev+1 value.
    let (kbc_state, _) = store.encode_state_and_sv("kbc:kbx").await.unwrap();
    let bob_epoch = KbCollectionDoc::from_bytes(&kbc_state)
        .unwrap()
        .epoch_of(&fp("bob"));
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg_as("kbx", &fp("bob"), bob_epoch, "FRESH"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "a fresh current-epoch edit is accepted"
    );
    let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    assert!(
        TextSync::from_state(&state)
            .unwrap()
            .content()
            .contains("FRESH"),
        "fresh current-epoch edit is applied"
    );

    // MALICIOUS-CLIENT VARIANT: re-sending the divergent op stays rejected (its
    // new ops are still from the stale-epoch client_id, never C_now).
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            viewer_era,
            &mut docs
        )
        .await
        .error
        .is_some(),
        "re-sent stale-epoch op stays fenced"
    );
}

#[tokio::test]
async fn stale_epoch_continuation_of_canonical_client_is_fenced() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbx",
        "alice",
        &mut docs,
    )
    .await;

    // bob added directly as EDITOR (fresh grant ⇒ epoch 0) and makes an ACCEPTED
    // edit, so his epoch-0 client becomes part of the node's canonical lineage.
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg_as("kbx", &fp("bob"), 0, "ACCEPTED-EDIT"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "bob's epoch-0 edit is accepted and becomes canonical"
    );

    // Owner DEMOTES bob → viewer (epoch 1) then RE-PROMOTES → editor (epoch 2).
    // bob's editor never rotated off the epoch-0 client (no rejoin), mirroring 9c.
    for role in ["viewer", "editor"] {
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("alice"),
                Some(&fp("alice")),
                kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some(role)),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "owner role change to {role} applies"
        );
    }

    // THE EXPLOIT: bob authors a CONTINUATION under his now-stale epoch-0 client,
    // chained onto the canonical state (not a fresh lineage). Role gate passes
    // (he is an editor); the epoch fence must still reject it.
    let (canonical_state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    let cid0 = derive_kb_client_id(&fp("bob"), 0);
    let mut ts = TextSync::from_state_with_client_id(&canonical_state, cid0).unwrap();
    let cont_update = ts.insert(0, "VIEWER-ERA-CONT ");
    let cont_msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":"kbx","node_id":"concept:n","update":update_to_base64(&cont_update)}});
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        cont_msg,
        &mut docs,
    )
    .await;
    let msg = resp
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        resp.error.is_some() && msg.contains("rebase required"),
        "stale-epoch continuation must be fenced (B-20); got: {msg:?}"
    );

    // NO CASCADE: the canonical state is byte-identical to before the fenced push
    // (`canonical_state` was captured just above to build the continuation), and
    // never gains the viewer-interval edit.
    let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    assert_eq!(
        state, canonical_state,
        "a fenced continuation must leave the canonical node byte-identical (no cascade)"
    );
    let canonical = TextSync::from_state(&state).unwrap().content();
    assert!(
        !canonical.contains("VIEWER-ERA-CONT"),
        "stale continuation must not cascade; canonical = {canonical:?}"
    );

    // bob CAN still converge by re-authoring under his CURRENT epoch — now an
    // unpredictable token (#72), read from the collection rather than assumed.
    let (kbc_state, _) = store.encode_state_and_sv("kbc:kbx").await.unwrap();
    let bob_epoch = KbCollectionDoc::from_bytes(&kbc_state)
        .unwrap()
        .epoch_of(&fp("bob"));
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg_as("kbx", &fp("bob"), bob_epoch, "REAUTHORED"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "a fresh current-epoch edit is accepted"
    );
}
