use super::*;

#[tokio::test]
async fn set_governance_signs_oplog_and_derive_reads_quorum() {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::{derive_governance, Governance};

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let id = Identity::generate("owner");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbg",
        "owner",
        &mut docs,
    )
    .await;

    // The default (no SetGovernance op) is single-owner.
    let coll = load_coll(&store, "kbg").await;
    assert_eq!(
        derive_governance(&coll.oplog_ops(), &owner_pubkey),
        Governance::SingleOwner
    );

    // Owner sets quorum:2; the signed log now derives Quorum{2}.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_set_governance_msg("kbg", "quorum:2"),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "set_governance: {:?}", resp.error);
    let coll = load_coll(&store, "kbg").await;
    assert_eq!(
        derive_governance(&coll.oplog_ops(), &owner_pubkey),
        Governance::Quorum { threshold: 2 }
    );

    // A meaningless threshold is rejected at the RPC boundary.
    let bad = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_set_governance_msg("kbg", "quorum:0"),
        &mut docs,
    )
    .await;
    assert!(bad.error.is_some(), "quorum:0 is rejected");
}

#[tokio::test]
async fn quorum_requires_two_distinct_owners_to_revoke() {
    use mae_mcp::identity::Identity;

    // Daemon A (owner1, genesis) and daemon B (owner2). bob never signs, so a
    // synthetic fingerprint suffices for the subject.
    let store_a = test_doc_store();
    let store_b = test_doc_store();
    let bc = test_broadcaster();
    let mut docs_a = HashSet::new();
    let mut docs_b = HashSet::new();

    let owner1 = Identity::generate("owner1");
    let owner1_fp = owner1.fingerprint();
    let owner1_pk = owner1.public().to_bytes();
    let owner2 = Identity::generate("owner2");
    let owner2_fp = owner2.fingerprint();
    let bob = fp("bob");

    store_a.set_signer(Arc::new(owner1));
    store_b.set_signer(Arc::new(owner2));

    // Daemon A: owner1 shares, promotes owner2 to Owner, admits bob (editor),
    // sets quorum:2, then authors the FIRST revoke of bob.
    kb_share_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        "kbq",
        "owner1",
        &mut docs_a,
    )
    .await;
    dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_member_msg("kb/add_member", "kbq", &owner2_fp, Some("owner")),
        &mut docs_a,
    )
    .await;
    dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_member_msg("kb/add_member", "kbq", &bob, Some("editor")),
        &mut docs_a,
    )
    .await;
    let g = dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_set_governance_msg("kbq", "quorum:2"),
        &mut docs_a,
    )
    .await;
    assert!(g.error.is_none(), "set_governance: {:?}", g.error);
    store_a.set_kb_anchor("kbq", owner1_pk).await;
    let r1 = dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_revoke_msg("kbq", &bob),
        &mut docs_a,
    )
    .await;
    assert!(r1.error.is_none(), "owner1 revoke: {:?}", r1.error);

    // One owner's revoke is below quorum ⇒ bob still has access on daemon A.
    let access = |store: Arc<DocStore>, p: String, op: KbOp| async move {
        kb_access(&store, "kbq", Some(&p), op, Transport::Hub).await
    };
    assert!(
        matches!(
            access(Arc::clone(&store_a), bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "one owner's revoke is below quorum ⇒ bob retains access on daemon A"
    );

    // Sync A's signed collection state to daemon B (what the mesh transport carries).
    let (state, _) = store_a
        .encode_state_and_sv("kbc:kbq")
        .await
        .expect("collection exists on A");
    store_b
        .apply_update("kbc:kbq", &state, None)
        .await
        .expect("apply A's collection state onto B");
    store_b.set_kb_anchor("kbq", owner1_pk).await;

    // B derives the same anchored membership: owner2 is an Owner, and bob still has
    // access there too (still one revoke).
    assert!(
        matches!(
            access(Arc::clone(&store_b), owner2_fp.clone(), KbOp::Manage).await,
            Ok(AccessDecision::Allow)
        ),
        "owner2 derives Owner on its own daemon ⇒ may co-sign"
    );
    assert!(
        matches!(
            access(Arc::clone(&store_b), bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "bob still below quorum on daemon B before owner2 co-signs"
    );

    // Daemon B: owner2 co-signs the revoke ⇒ TWO distinct owners ⇒ bob removed.
    let r2 = dispatch_as(
        &store_b,
        &bc,
        Some("owner2"),
        Some(&owner2_fp),
        kb_revoke_msg("kbq", &bob),
        &mut docs_b,
    )
    .await;
    assert!(r2.error.is_none(), "owner2 revoke: {:?}", r2.error);
    assert!(
        matches!(
            access(Arc::clone(&store_b), bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Deny(_))
        ),
        "two distinct owners co-signed ⇒ bob removed by quorum"
    );
}

#[tokio::test]
async fn non_owner_cannot_revoke_or_set_governance() {
    use mae_mcp::identity::Identity;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_pk = owner.public().to_bytes();
    store.set_signer(Arc::new(owner));

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbn",
        "owner",
        &mut docs,
    )
    .await;
    let editor = fp("editor");
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbn", &editor, Some("editor")),
        &mut docs,
    )
    .await;
    store.set_kb_anchor("kbn", owner_pk).await;

    // An editor principal may not revoke (Manage denied at the derived gate) ...
    let r = dispatch_as(
        &store,
        &bc,
        Some("editor"),
        Some(&editor),
        kb_revoke_msg("kbn", &fp("someone")),
        &mut docs,
    )
    .await;
    assert!(r.error.is_some(), "editor may not revoke");

    // ... nor set governance.
    let gov = dispatch_as(
        &store,
        &bc,
        Some("editor"),
        Some(&editor),
        kb_set_governance_msg("kbn", "quorum:2"),
        &mut docs,
    )
    .await;
    assert!(gov.error.is_some(), "editor may not set governance");
}
