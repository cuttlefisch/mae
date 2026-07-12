use super::*;

#[tokio::test]
async fn transport_policy_gate() {
    use mae_sync::kb::TransportPolicy;

    // Share a fresh "kbx" collection with the given transport policy + bob as a
    // (non-owner) Editor member. `share_doc` replaces, so each call resets it.
    async fn share(store: &DocStore, owner: &str, member: &str, policy: TransportPolicy) {
        let mut coll = KbCollectionDoc::new_owned("KB", owner, "owner");
        coll.set_transport_policy(policy);
        coll.add_pending(member, "bob", "t0", None, None);
        coll.approve(member, SyncRole::Editor);
        store
            .share_doc("kbc:kbx", &coll.encode_state())
            .await
            .unwrap();
    }

    let store = test_doc_store();
    let owner = fp("owner");
    let bob = fp("bob");

    // --- p2p-only KB ---
    share(&store, &owner, &bob, TransportPolicy::P2p).await;
    // Owner over the hub: ALLOWED (owner bypass — the local editor reaches its own KB).
    assert_eq!(
        kb_access(&store, "kbx", Some(&owner), KbOp::Read, Transport::Hub)
            .await
            .unwrap(),
        AccessDecision::Allow
    );
    // Non-owner member over the hub: DENIED (not exposed on the hub).
    assert!(matches!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::Hub)
            .await
            .unwrap(),
        AccessDecision::Deny(_)
    ));
    // Non-owner member over the mesh: ALLOWED.
    assert_eq!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Allow
    );

    // --- hub-only KB ---
    share(&store, &owner, &bob, TransportPolicy::Hub).await;
    // Member over the mesh: DENIED (hub-only is not on the mesh).
    assert!(matches!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Edit, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Deny(_)
    ));
    // A non-member's JOIN over the mesh is transport-gated too (before join policy).
    let stranger = fp("stranger");
    assert!(matches!(
        kb_access(&store, "kbx", Some(&stranger), KbOp::Join, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Deny(_)
    ));

    // --- both ---
    share(&store, &owner, &bob, TransportPolicy::Both).await;
    assert_eq!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::Hub)
            .await
            .unwrap(),
        AccessDecision::Allow
    );
    assert_eq!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Allow
    );
}

#[tokio::test]
async fn kb_share_sets_and_widens_transport_policy() {
    use mae_sync::kb::TransportPolicy;

    fn share_msg(kb_id: &str, owner_label: &str, transport: &str) -> serde_json::Value {
        let coll = KbCollectionDoc::new_owned(kb_id, "", owner_label);
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": kb_id, "name": kb_id, "creator": owner_label,
                "collection_state": update_to_base64(&coll.encode_state()),
                "nodes": [], "transport": transport,
            }
        })
    }

    let store = test_doc_store();
    let bc = test_broadcaster();
    let owner = fp("owner");

    // First share over p2p ⇒ P2p-only (NOT widened with the conservative Hub default).
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        share_msg("kbx", "owner", "p2p"),
        &mut HashSet::new(),
    )
    .await;
    assert!(resp.error.is_none(), "kb/share p2p: {:?}", resp.error);
    assert_eq!(
        load_coll(&store, "kbx").await.transport_policy(),
        TransportPolicy::P2p
    );

    // The owner re-shares over hub ⇒ exposure WIDENS to Both (preserving p2p).
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        share_msg("kbx", "owner", "hub"),
        &mut HashSet::new(),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "kb/share hub re-share: {:?}",
        resp.error
    );
    assert_eq!(
        load_coll(&store, "kbx").await.transport_policy(),
        TransportPolicy::Both
    );

    // A KB shared with no transport param defaults to Hub-only.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": "kbhub", "name": "kbhub", "creator": "owner",
                "collection_state": update_to_base64(&KbCollectionDoc::new_owned("kbhub", "", "owner").encode_state()),
                "nodes": [],
            }
        }),
        &mut HashSet::new(),
    )
    .await;
    assert!(resp.error.is_none(), "kb/share default: {:?}", resp.error);
    assert_eq!(
        load_coll(&store, "kbhub").await.transport_policy(),
        TransportPolicy::Hub
    );
}

#[tokio::test]
async fn add_member_signs_oplog_for_owned_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::derive_valid_members;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    // The daemon's signing identity == the KB owner (the seam where the daemon
    // signs membership ops). Use a REAL key so fingerprints + signatures verify.
    let id = Identity::generate("daemon");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    // Owner shares the KB (authenticated as the signer's principal), then adds bob.
    let resp = kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbsig",
        "owner",
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "share: {:?}", resp.error);

    let bob = fp("bob");
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbsig", &bob, Some("editor")),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "add_member: {:?}", resp.error);

    // The op-log carries the genesis owner self-admit + the signed admit of bob;
    // every record verifies, and a peer derives owner+bob anchored on the owner key
    // — without trusting the relay (ADR-026).
    let coll = load_coll(&store, "kbsig").await;
    let ops = coll.oplog_ops();
    assert_eq!(ops.len(), 2, "genesis + admit");
    assert!(ops.iter().all(|o| o.verify_signed()), "all records verify");

    let members = derive_valid_members(&ops, &owner_pubkey, 0);
    assert_eq!(members.len(), 2);
    assert_eq!(members[&owner_fp].role, SyncRole::Owner);
    assert_eq!(members[&bob].role, SyncRole::Editor);
    assert_eq!(members[&bob].invited_by, owner_fp, "owner admitted bob");

    // A different anchor (a relay's forged collection) derives nothing.
    let stranger = Identity::generate("stranger").public().to_bytes();
    assert!(derive_valid_members(&ops, &stranger, 0).is_empty());

    // Removing bob appends a signed Remove; the peer no longer derives bob.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/remove_member", "kbsig", &bob, None),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "remove_member: {:?}", resp.error);
    let coll = load_coll(&store, "kbsig").await;
    let members = derive_valid_members(&coll.oplog_ops(), &owner_pubkey, 0);
    assert!(
        !members.contains_key(&bob),
        "bob removed in the derived set"
    );
    assert!(members.contains_key(&owner_fp));
}

#[tokio::test]
async fn add_member_unsigned_without_a_signer() {
    // No signer (psk/none mode) ⇒ the legacy member_roles path only; no op-log.
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let owner = fp("alice");
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&owner),
        "kbu",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&owner),
        kb_member_msg("kb/add_member", "kbu", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    let coll = load_coll(&store, "kbu").await;
    assert_eq!(coll.oplog_len(), 0, "no signer ⇒ no signed op-log");
    assert_eq!(
        coll.role_of(&fp("bob")),
        Some(SyncRole::Editor),
        "legacy path"
    );
}

#[tokio::test]
async fn approve_member_signs_oplog_for_owned_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::derive_valid_members;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let id = Identity::generate("daemon");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbap",
        "owner",
        &mut docs,
    )
    .await;
    // bob requests to join (invite default ⇒ pending), owner approves as editor.
    let bob = fp("bob");
    dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob),
        kb_join_msg("kbap"),
        &mut docs,
    )
    .await;
    let ok = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_approve_msg("kbap", &bob, Some("editor")),
        &mut docs,
    )
    .await;
    assert!(ok.error.is_none(), "approve: {:?}", ok.error);

    // The approval is a signed Admit: a peer derives owner + the approved member.
    let coll = load_coll(&store, "kbap").await;
    let members = derive_valid_members(&coll.oplog_ops(), &owner_pubkey, 0);
    assert_eq!(members.len(), 2);
    assert_eq!(members[&bob].role, SyncRole::Editor);
    assert_eq!(members[&bob].invited_by, owner_fp);
}

#[tokio::test]
async fn kb_access_derives_from_oplog_when_anchored() {
    use mae_mcp::identity::Identity;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let id = Identity::generate("daemon");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    // Owner shares + adds bob (editor) ⇒ a signed op-log.
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbanc",
        "owner",
        &mut docs,
    )
    .await;
    let bob = fp("bob");
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbanc", &bob, Some("editor")),
        &mut docs,
    )
    .await;

    // Register the external anchor (what the dialer does for a JOINED KB), so
    // kb_access derives membership from the signed op-log, not member_roles.
    store.set_kb_anchor("kbanc", owner_pubkey).await;

    let access = |p: String, op: KbOp| {
        let store = Arc::clone(&store);
        async move { kb_access(&store, "kbanc", Some(&p), op, Transport::Hub).await }
    };
    assert!(
        matches!(
            access(owner_fp.clone(), KbOp::Manage).await,
            Ok(AccessDecision::Allow)
        ),
        "owner derives Manage from the op-log"
    );
    assert!(
        matches!(
            access(bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "bob (editor) derives Edit"
    );
    assert!(
        matches!(
            access(bob.clone(), KbOp::Manage).await,
            Ok(AccessDecision::Deny(_))
        ),
        "editor may not Manage"
    );
    assert!(
        matches!(
            access(fp("carol"), KbOp::Edit).await,
            Ok(AccessDecision::Deny(_))
        ),
        "non-member denied"
    );

    // Remove bob (signed Remove); the derived gate now denies him.
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/remove_member", "kbanc", &bob, None),
        &mut docs,
    )
    .await;
    assert!(
        matches!(
            access(bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Deny(_))
        ),
        "removed member denied via derived membership"
    );
}
