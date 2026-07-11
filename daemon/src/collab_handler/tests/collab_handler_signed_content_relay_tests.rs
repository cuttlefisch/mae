use super::*;

#[tokio::test]
async fn signed_content_op_verified_or_rejected_on_anchored_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_ops::{ContentOp, SignedContentOp};

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_pk = owner.public().to_bytes();
    store.set_signer(Arc::new(owner));

    // bob is a real editor identity (he must SIGN, so a synthetic fp won't do).
    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();
    let bob_secret = bob.secret_bytes();
    let bob_pub = bob.public().to_bytes();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbc",
        "owner",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbc", &bob_fp, Some("editor")),
        &mut docs,
    )
    .await;
    // Anchor ⇒ the daemon derives membership from the signed op-log (the mesh path).
    store.set_kb_anchor("kbc", owner_pk).await;

    // A yrs edit authored under bob's epoch-0 KB client_id (so the ADR-023 fence
    // also passes for the valid case — signing is an ADDITIONAL gate, not a bypass).
    let cid = mae_sync::kb::derive_kb_client_id(&bob_fp, 0);
    let mut ts = TextSync::with_client_id("", cid);
    let upd = ts.insert(0, "hello mesh");

    // (1) VALID: bob signs his own edit → verified + applied.
    let op = ContentOp {
        kb_id: "kbc".to_string(),
        node_id: "concept:n".to_string(),
        base_sv: vec![],
        author: bob_fp.clone(),
        epoch: 0,
        issued_at: 1_700_000_000,
    };
    let sig = op.sign(&bob_secret, &upd);
    let signed = SignedContentOp {
        op,
        payload: upd.clone(),
        sig,
        author_pubkey: bob_pub,
    };
    let ok = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        signed_node_update_msg("kbc", "concept:n", &upd, &signed),
        &mut docs,
    )
    .await;
    assert!(
        ok.error.is_none(),
        "valid signed edit applies: {:?}",
        ok.error
    );

    // (2) FORGED SIGNATURE: flip a signature byte → BadSignature, rejected.
    let mut tampered = signed.clone();
    tampered.sig[0] ^= 0xff;
    let bad = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        signed_node_update_msg("kbc", "concept:n", &upd, &tampered),
        &mut docs,
    )
    .await;
    assert!(
        bad.error
            .as_ref()
            .map(|e| e.message.contains("signed content op rejected"))
            .unwrap_or(false),
        "tampered signature rejected, got {:?}",
        bad.error
    );

    // (3) MIS-ATTRIBUTION: a member (bob) relays an op attributed to a NON-member
    // (mallory), validly signed by mallory's own key. The signature + fingerprint
    // bind, but mallory ∉ members ⇒ NotAMember, rejected.
    let mallory = Identity::generate("mallory");
    let m_op = ContentOp {
        kb_id: "kbc".to_string(),
        node_id: "concept:n".to_string(),
        base_sv: vec![],
        author: mallory.fingerprint(),
        epoch: 0,
        issued_at: 1_700_000_000,
    };
    let m_sig = m_op.sign(&mallory.secret_bytes(), &upd);
    let m_signed = SignedContentOp {
        op: m_op,
        payload: upd.clone(),
        sig: m_sig,
        author_pubkey: mallory.public().to_bytes(),
    };
    let injected = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        signed_node_update_msg("kbc", "concept:n", &upd, &m_signed),
        &mut docs,
    )
    .await;
    assert!(
        injected
            .error
            .as_ref()
            .map(|e| e.message.contains("signed content op rejected"))
            .unwrap_or(false),
        "non-member-attributed op rejected, got {:?}",
        injected.error
    );
}

#[tokio::test]
async fn owned_kb_signed_op_broadcast_carries_content_header_for_mesh_relay() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_ops::{ContentOp, SignedContentOp};

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_pub = owner.public().to_bytes();
    let owner_secret = owner.secret_bytes();
    store.set_signer(Arc::new(owner));

    // Owner OWNS the KB. Crucially we do NOT set_kb_anchor — an owner never "joins"
    // its own KB, so before the fix `kb_anchor()` was None and the header was never
    // attached. `resolve_content_anchor` falls back to the owner's own signer key.
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbc",
        "owner",
        &mut docs,
    )
    .await;

    // Add a member — this seeds the signed owner-self-admit GENESIS (owner becomes a
    // derived Owner member) plus admits bob, exactly as a real E2E-owner flow does
    // before editing. Without a genesis the owner isn't a *derived* member and the
    // trusted-local edit falls through to the legacy gate (still applies, just no
    // header) — this test exercises the header-attach SUCCESS path.
    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbc", &bob_fp, Some("editor")),
        &mut docs,
    )
    .await;

    // A peer session subscribes so we can capture the relayed broadcast (dispatch_as
    // broadcasts under session 0 → everyone else receives it).
    let peer_sid = 999u64;
    let mut rx = {
        let mut b = bc.lock().unwrap();
        b.subscribe_doc(peer_sid, "kb:concept:n");
        b.subscribe(peer_sid, vec!["sync_update".to_string()])
    };

    // Owner authors a SIGNED edit under its own epoch-0 KB client_id (so the ADR-023
    // fence also passes — signing is an ADDITIONAL gate, not a bypass).
    let cid = mae_sync::kb::derive_kb_client_id(&owner_fp, 0);
    let mut ts = TextSync::with_client_id("", cid);
    let upd = ts.insert(0, "owner edit destined for the mesh");
    let op = ContentOp {
        kb_id: "kbc".to_string(),
        node_id: "concept:n".to_string(),
        base_sv: vec![],
        author: owner_fp.clone(),
        epoch: 0,
        issued_at: 1_700_000_000,
    };
    let sig = op.sign(&owner_secret, &upd);
    let signed = SignedContentOp {
        op,
        payload: upd.clone(),
        sig,
        author_pubkey: owner_pub,
    };

    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        signed_node_update_msg("kbc", "concept:n", &upd, &signed),
        &mut docs,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "owner's signed edit on its own KB applies: {:?}",
        resp.error
    );

    // THE FIX: the broadcast for kb:concept:n MUST carry a content_header (Some), so
    // the dialer relays a re-verifiable op to mesh peers. Before the fix it was None
    // and the mesh joiner's require-signed gate rejected it (#255).
    let mut saw = false;
    while let Ok(ev) = rx.try_recv() {
        if let EditorEvent::SyncUpdate {
            buffer_name,
            content_header,
            ..
        } = ev
        {
            if buffer_name == "kb:concept:n" {
                assert!(
                    content_header.is_some(),
                    "owned-KB signed op MUST broadcast WITH a content_header for mesh \
                     relay (#255) — got None, which a mesh member rejects as unsigned"
                );
                saw = true;
            }
        }
    }
    assert!(saw, "expected a SyncUpdate broadcast for kb:concept:n");
}

#[tokio::test]
async fn verify_relayed_content_op_owned_kb_branches() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_ops::{ContentOp, SignedContentOp};

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    store.set_signer(Arc::new(owner));
    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbo",
        "owner",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbo", &bob_fp, Some("editor")),
        &mut docs,
    )
    .await;
    // Deliberately NOT anchored: an OWNED KB, so `resolve_content_anchor` must fall
    // back to the daemon's own signer key (owner == signer).

    let upd = {
        let mut ts = TextSync::with_client_id("", 5);
        ts.insert(0, "hi")
    };
    let header = |id: &Identity, epoch: u64| {
        let op = ContentOp {
            kb_id: "kbo".to_string(),
            node_id: "concept:n".to_string(),
            base_sv: vec![],
            author: id.fingerprint(),
            epoch,
            issued_at: 0,
        };
        let sig = op.sign(&id.secret_bytes(), &upd);
        SignedContentOp {
            op,
            payload: upd.clone(),
            sig,
            author_pubkey: id.public().to_bytes(),
        }
        .header_params()
    };
    let doc = "kb:concept:n";

    // A valid member's signed op verifies (owned-KB anchor resolved from the signer).
    let h = header(&bob, 0);
    assert!(
        matches!(
            verify_relayed_content_op(&store, "kbo", doc, &upd, Some(&h), true).await,
            Ok(Some(_))
        ),
        "valid member op on an owned KB verifies"
    );
    // Unsigned: rejected under require-signed (mesh), accepted without it (hub).
    assert!(
        verify_relayed_content_op(&store, "kbo", doc, &upd, None, true)
            .await
            .is_err(),
        "unsigned rejected when require_signed (mesh)"
    );
    assert!(
        matches!(
            verify_relayed_content_op(&store, "kbo", doc, &upd, None, false).await,
            Ok(None)
        ),
        "unsigned accepted on the hub (migration)"
    );
    // A non-member's validly-signed op is rejected (NotAMember).
    let stranger = header(&Identity::generate("stranger"), 0);
    assert!(
        verify_relayed_content_op(&store, "kbo", doc, &upd, Some(&stranger), true)
            .await
            .is_err(),
        "non-member op rejected even with a valid signature"
    );
    // A non-KB doc is not a content op → passes through.
    assert!(
        matches!(
            verify_relayed_content_op(&store, "kbo", "buffer:foo.txt", &upd, None, true).await,
            Ok(None)
        ),
        "non-KB doc passes through"
    );
}

#[tokio::test]
async fn kb_member_epoch_reads_oplog_not_legacy_for_anchored_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::MembershipAction;

    let store = test_doc_store();
    let owner = Identity::generate("owner");
    let ofp = owner.fingerprint();
    let opk = owner.public().to_bytes();
    let osec = owner.secret_bytes();
    let mfp = fp("member"); // op-log-only (never written to member_roles, as on a mesh join)

    let mut coll = KbCollectionDoc::new_owned("kbe", &ofp, "owner");
    let g = coll.build_membership_op(
        "kbe",
        MembershipAction::Admit,
        &ofp,
        Some(Role::Owner),
        true,
        &ofp,
        0,
        None,
        0,
    );
    let gsig = g.sign(&osec);
    coll.append_signed_op(&g, &gsig, &opk);
    // Admit the member at a NON-ZERO epoch (a re-grant carries a fresh epoch in reality).
    let a = coll.build_membership_op(
        "kbe",
        MembershipAction::Admit,
        &mfp,
        Some(Role::Editor),
        false,
        &ofp,
        0,
        None,
        7,
    );
    let asig = a.sign(&osec);
    coll.append_signed_op(&a, &asig, &opk);

    // The bug's precondition: legacy member_roles has no entry ⇒ epoch_of → 0.
    assert_eq!(
        coll.epoch_of(&mfp),
        0,
        "member_roles is empty for an op-log-only member"
    );

    // Anchored ⇒ kb_member_epoch derives the NON-ZERO epoch from the signed op-log (the fix).
    store.set_kb_anchor("kbe", opk).await;
    assert_eq!(
        kb_member_epoch(&store, "kbe", &coll, &mfp).await,
        7,
        "epoch comes from the signed op-log, not the frozen member_roles"
    );
    // Un-anchored (owned KB) ⇒ falls back to the legacy member_roles epoch (0 here).
    let store2 = test_doc_store();
    assert_eq!(
        kb_member_epoch(&store2, "kbe", &coll, &mfp).await,
        0,
        "an un-anchored KB keeps the legacy member_roles epoch"
    );
}
