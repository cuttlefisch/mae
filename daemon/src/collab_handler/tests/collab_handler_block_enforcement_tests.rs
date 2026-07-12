use super::*;

#[tokio::test]
async fn local_block_fences_principal_at_every_derive_site() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_ops::{ContentOp, SignedContentOp};
    use mae_sync::membership::derive_valid_members;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_pk = owner.public().to_bytes();
    store.set_signer(Arc::new(owner));

    // bob + carol are REAL editor identities (they must sign their own content ops).
    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();
    let carol = Identity::generate("carol");
    let carol_fp = carol.fingerprint();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbb",
        "owner",
        &mut docs,
    )
    .await;
    for member_fp in [&bob_fp, &carol_fp] {
        dispatch_as(
            &store,
            &bc,
            Some("owner"),
            Some(&owner_fp),
            kb_member_msg("kb/add_member", "kbb", member_fp, Some("editor")),
            &mut docs,
        )
        .await;
    }
    // Anchor ⇒ membership derives from the signed op-log (the path the blocklist guards).
    store.set_kb_anchor("kbb", owner_pk).await;

    let access = |p: String, op: KbOp| {
        let store = Arc::clone(&store);
        async move { kb_access(&store, "kbb", Some(&p), op, Transport::Hub).await }
    };
    // Build a validly-signed epoch-0 content op for an identity (no store borrow).
    let signed_op = |idy: &Identity, text: &str| -> (Vec<u8>, SignedContentOp) {
        let fp = idy.fingerprint();
        let cid = mae_sync::kb::derive_kb_client_id(&fp, 0);
        let mut ts = TextSync::with_client_id("", cid);
        let upd = ts.insert(0, text);
        let op = ContentOp {
            kb_id: "kbb".to_string(),
            node_id: "concept:n".to_string(),
            base_sv: vec![],
            author: fp,
            epoch: 0,
            issued_at: 1_700_000_000,
        };
        let sig = op.sign(&idy.secret_bytes(), &upd);
        let signed = SignedContentOp {
            op,
            payload: upd.clone(),
            sig,
            author_pubkey: idy.public().to_bytes(),
        };
        (upd, signed)
    };

    // --- baseline: both editors derive Edit + their signed ops apply ---
    assert!(matches!(
        access(bob_fp.clone(), KbOp::Edit).await,
        Ok(AccessDecision::Allow)
    ));
    assert!(matches!(
        access(carol_fp.clone(), KbOp::Edit).await,
        Ok(AccessDecision::Allow)
    ));
    let (upd, signed) = signed_op(&bob, "bob-pre");
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&bob_fp),
            signed_node_update_msg("kbb", "concept:n", &upd, &signed),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "baseline: bob's signed op verifies"
    );

    // --- block bob (local self-protection) ---
    let blocked = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_block_msg("kb/block_principal", "kbb", &bob_fp),
        &mut docs,
    )
    .await;
    assert!(
        blocked.error.is_none(),
        "block accepted: {:?}",
        blocked.error
    );

    // (a) GATE: bob is now denied at kb_access...
    assert!(
        matches!(
            access(bob_fp.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Deny(_))
        ),
        "blocked principal fenced at the access gate"
    );
    // (b) CONTENT PATH: ...AND his validly-signed op is rejected by `verify_content_op`.
    //     The access gate (a) short-circuits the full dispatch, so call the content check
    //     directly to prove it ALSO fences the blocked author independently (complete
    //     mediation — the block holds even if a future path reaches content verification
    //     without the gate). The signature itself is valid; only the blocklist rejects it.
    let (_upd_b, signed_b) = signed_op(&bob, "bob-post");
    let verdict = verify_content_op(&store, "kbb", &owner_pk, &signed_b).await;
    assert!(
        verdict.is_err(),
        "verify_content_op rejects a blocked author's (validly-signed) op, got {verdict:?}"
    );
    // (c) SELECTIVE: carol (NOT blocked) is unaffected at BOTH sites.
    assert!(
        matches!(
            access(carol_fp.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "the block is targeted — a non-blocked member still derives Edit"
    );
    let (upd_c, signed_c) = signed_op(&carol, "carol-post");
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("carol"),
            Some(&carol_fp),
            signed_node_update_msg("kbb", "concept:n", &upd_c, &signed_c),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "a non-blocked member's signed op still applies (not a blanket failure)"
    );
    // (d) LOCAL-ONLY: the synced op-log still lists bob as a member — the block lives
    //     only in this daemon's derived view, never as a propagated op-log Remove.
    let coll = load_coll(&store, "kbb").await;
    let global = derive_valid_members(&coll.oplog_ops(), &owner_pk, 0);
    assert!(
        global.contains_key(&bob_fp),
        "the shared op-log still admits bob — the block was NOT propagated as a removal"
    );
    // (e) INTROSPECTION: the kb/blocklist query reports the block (the ONLY way a client
    //     learns the local-only blocklist; backs the editor's *KB Sharing* Blocked view).
    let bl = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/blocklist","params":{"kb_id":"kbb"}}),
        &mut docs,
    )
    .await;
    let listed = bl
        .result
        .as_ref()
        .and_then(|r| r.get("blocklist"))
        .and_then(|b| b.get("kbb"))
        .and_then(|a| a.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec![bob_fp.as_str()],
        "kb/blocklist reports exactly the locally-blocked principal"
    );

    // --- fence even the OWNER (self-lock) — then unblock to restore ---
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_block_msg("kb/block_principal", "kbb", &owner_fp),
        &mut docs,
    )
    .await;
    assert!(
        matches!(
            access(owner_fp.clone(), KbOp::Manage).await,
            Ok(AccessDecision::Deny(_))
        ),
        "a local block fences even the owner"
    );
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_block_msg("kb/unblock_principal", "kbb", &owner_fp),
        &mut docs,
    )
    .await;
    assert!(matches!(
        access(owner_fp.clone(), KbOp::Manage).await,
        Ok(AccessDecision::Allow)
    ));

    // --- unblock bob → access + content restored ---
    let unblocked = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_block_msg("kb/unblock_principal", "kbb", &bob_fp),
        &mut docs,
    )
    .await;
    assert!(unblocked.error.is_none());
    assert!(
        matches!(
            access(bob_fp.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "unblock restores access"
    );
    let (upd_r, signed_r) = signed_op(&bob, "bob-restored");
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&bob_fp),
            signed_node_update_msg("kbb", "concept:n", &upd_r, &signed_r),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "unblock restores the content path too"
    );

    // --- blocking a NON-member is a harmless no-op ---
    let noop = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_block_msg("kb/block_principal", "kbb", &fp("stranger")),
        &mut docs,
    )
    .await;
    assert!(noop.error.is_none(), "blocking a non-member does not error");
    assert!(
        matches!(
            access(carol_fp.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "blocking a stranger leaves real members untouched"
    );
}

#[tokio::test]
async fn local_block_survives_docstore_reload() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("collab.sqlite");
    let backend = Arc::new(SqliteBackend::open(&db).unwrap());
    {
        let store = Arc::new(DocStore::new(backend.clone(), 500));
        store.add_kb_block("kbx", "SHA256:ghost").await.unwrap();
        assert!(
            store.kb_blocklist("kbx").await.contains("SHA256:ghost"),
            "the cache reflects a write immediately"
        );
    }
    // A fresh DocStore on the SAME durable backend — a genuine restart.
    let store2 = Arc::new(DocStore::new(backend.clone(), 500));
    assert!(
        store2.kb_blocklist("kbx").await.is_empty(),
        "the in-memory cache starts empty before hydration"
    );
    store2.load_blocklists().await;
    assert!(
        store2
            .membership_view_for("kbx")
            .await
            .blocklist
            .contains("SHA256:ghost"),
        "load_blocklists rehydrates the durable block → still fenced after restart"
    );
}
