use super::*;

// ============================================================================
// Confidence-review finding A3 — RAW-sync read of a KB doc must be access-gated.
// `sync/full_state` / `sync/state_vector` return a doc's yrs state for any caller-supplied
// name; without a gate they bypass the `kb_access(Read)` check that `kb/node_fetch`/`kb/join`
// enforce, leaking `kb:<node>` plaintext and `kbc:<kb>` (roster + pending pubkeys) to a
// non-member. The attacker's test: a stranger must be DENIED.
// ============================================================================

#[tokio::test]
async fn raw_sync_read_of_a_kb_doc_is_access_gated() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let owner = fp("owner");
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        "kbsec",
        "owner",
        &mut docs,
    )
    .await;

    // A stranger (non-member) must be DENIED both the collection doc and any node doc, on
    // BOTH raw read methods.
    for doc in ["kbc:kbsec", "kb:kbsec:alpha"] {
        for method in ["sync/full_state", "sync/state_vector"] {
            let r = dispatch_as(
                &store,
                &bc,
                Some("evil"),
                Some(&fp("evil")),
                serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":{"doc":doc}}),
                &mut docs,
            )
            .await;
            assert!(
                r.error.is_some(),
                "a non-member must be DENIED {method} on {doc}"
            );
        }
    }

    // The owner (a member) MAY read its own collection doc via the raw path (members pass the
    // Read gate) — so the fix closes the hole without breaking legitimate members.
    let r = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/full_state","params":{"doc":"kbc:kbsec"}}),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "the owner (a member) may read its own collection doc"
    );

    // A non-KB doc (text buffer / session doc) is UNAFFECTED — no KB gating applied.
    let r = dispatch_as(
        &store,
        &bc,
        Some("evil"),
        Some(&fp("evil")),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/full_state","params":{"doc":"a-shared-buffer"}}),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "a non-KB doc keeps its existing (ungated) sync behavior"
    );
}

#[tokio::test]
async fn member_self_service_update_cannot_delete_an_existing_oplog_op() {
    // (1) Member M legitimately self-rebinds to m2 — this appends a `Rebind` op to the
    // op-log (the owned KB's log starts empty; the accepted rebind populates it).
    let (store, bc, m, mut docs) = kb_with_member("kbdel", 60).await;
    let m2 = rotor_keys(63);
    let mut coll = load_coll(&store, "kbdel").await;
    let update = coll.author_rebind("kbdel", &m.2, &m2.2, &m2.1, &m2.3, &m.0, &m.1, 1000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbdel", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "legit self-rebind accepted: {:?}",
        r.error
    );

    // (2) The op-log now has a record to attack.
    let mut coll = load_coll(&store, "kbdel").await;
    let ops = coll.oplog_ops();
    assert!(
        !ops.is_empty(),
        "precondition: the accepted rebind left an op-log record to attack"
    );
    let victim = ops[0].chain_hash();

    // (3) The now-current key m2 crafts an update that DELETES that op-log record. The
    // grow-only gate must reject it wholesale (before ⊄ after), even though it touches no
    // pinned manifest field.
    let delete_update = coll.remove_oplog_op_for_test(&victim);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m2"),
        Some(&m2.2),
        kb_collection_op_msg("kbdel", &delete_update),
        &mut docs,
    )
    .await;
    let msg = r
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        msg.contains("append-only"),
        "the delete must be rejected specifically by the append-only op-log gate, got: {msg}"
    );

    // (4) The rejected delete left the daemon's op-log intact.
    let after = load_coll(&store, "kbdel").await;
    assert!(
        after.oplog_ops().iter().any(|o| o.chain_hash() == victim),
        "the rejected delete must not have removed the op from the daemon's store"
    );
}

#[tokio::test]
async fn derive_cache_hits_but_never_serves_stale_membership() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::KbCollectionDoc;

    let store = test_doc_store();
    let owner = Identity::from_seed(&[1u8; 32], "owner");
    let owner_fp = owner.fingerprint();
    let opk = owner.public().to_bytes();
    let osec = owner.secret_bytes();
    let anchor = opk;
    let member = Identity::from_seed(&[2u8; 32], "member");
    let mfp = member.fingerprint();
    let now = 2_000;

    // An E2e KB with a signed owner genesis — the anchor the derive resolves membership from.
    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    let mut coll = KbCollectionDoc::new_owned("kbc", &owner_fp, "owner");
    coll.author_e2e_genesis("kbc", &owner_fp, &osec, &opk, self_wrap, 1000);

    // (1) Two derives on an UNCHANGED op-log return the same cached Arc — the O(1) hit.
    let dm1 = store.derived_membership("kbc", &coll, &anchor, now).await;
    let dm2 = store.derived_membership("kbc", &coll, &anchor, now).await;
    assert!(
        std::sync::Arc::ptr_eq(&dm1, &dm2),
        "an unchanged op-log must be a cache hit (same Arc)"
    );
    assert_eq!(
        dm1.members.get(&owner_fp).map(|m| m.role),
        Some(SyncRole::Owner),
        "owner derived from the genesis"
    );

    // (2) Advancing the op-log (admit a member) changes the collection SV ⇒ the cache must
    // recompute, not serve the stale Arc.
    let member_wrap = wrap_to_member(&k, &wrap_public_for(&member.secret_bytes())).unwrap();
    coll.author_member_admit(
        "kbc",
        &mfp,
        &member.public().to_bytes(),
        &wrap_public_for(&member.secret_bytes()),
        SyncRole::Editor,
        "member",
        member_wrap,
        &owner_fp,
        &osec,
        &opk,
        1001,
    );
    let dm3 = store.derived_membership("kbc", &coll, &anchor, now).await;
    assert!(
        !std::sync::Arc::ptr_eq(&dm1, &dm3),
        "an op-log advance (SV change) must invalidate the cache"
    );
    assert!(
        dm3.members.contains_key(&mfp),
        "the newly admitted member is present after the op-log advance"
    );

    // (3) A local BLOCK is NOT in the op-log SV, so it must invalidate explicitly. A blocked
    // owner's authority (incl. the genesis it signed) is ignored ⇒ the derived set must drop it.
    // A stale cache would wrongly keep serving the blocked owner as authoritative.
    store.add_kb_block("kbc", &owner_fp).await.unwrap();
    let dm4 = store.derived_membership("kbc", &coll, &anchor, now).await;
    assert!(
        !std::sync::Arc::ptr_eq(&dm3, &dm4),
        "a block (same SV) must still invalidate the derive cache"
    );
    assert!(
        !dm4.members.contains_key(&owner_fp),
        "a locally-blocked owner must NOT be served stale as authoritative (ADR-039)"
    );

    // (4) Unblocking invalidates again and restores the owner.
    store.remove_kb_block("kbc", &owner_fp).await.unwrap();
    let dm5 = store.derived_membership("kbc", &coll, &anchor, now).await;
    assert!(
        dm5.members.contains_key(&owner_fp),
        "unblock invalidates + restores the owner"
    );
}

#[tokio::test]
async fn derive_cache_drops_a_timeboxed_out_member_when_wallclock_passes_expiry() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::KbCollectionDoc;
    use mae_sync::membership::MembershipAction;

    let store = test_doc_store();
    let owner = Identity::from_seed(&[1u8; 32], "owner");
    let owner_fp = owner.fingerprint();
    let opk = owner.public().to_bytes();
    let osec = owner.secret_bytes();
    let anchor = opk;
    let member = Identity::from_seed(&[2u8; 32], "member");
    let mfp = member.fingerprint();
    let expiry = 5_000u64;

    // E2e KB with a signed owner genesis (the anchor the derive resolves from).
    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    let mut coll = KbCollectionDoc::new_owned("kbc", &owner_fp, "owner");
    coll.author_e2e_genesis("kbc", &owner_fp, &osec, &opk, self_wrap, 1_000);

    // Admit the member with a TIMEBOX (expires_at = 5000), owner-signed.
    coll.upsert_member(&mfp, "member", SyncRole::Editor);
    let epoch = coll.epoch_of(&mfp);
    let mut op = coll.build_membership_op(
        "kbc",
        MembershipAction::Admit,
        &mfp,
        Some(SyncRole::Editor),
        false,
        &owner_fp,
        1_001,
        Some(expiry),
        epoch,
    );
    op.wrapped_key = Some(wrap_to_member(&k, &wrap_public_for(&member.secret_bytes())).unwrap());
    let sig = op.sign(&osec);
    coll.append_signed_op(&op, &sig, &opk);

    // Derive BEFORE expiry — member present; this WARMS the cache (valid_until = 5000).
    let before = store
        .derived_membership("kbc", &coll, &anchor, expiry - 1)
        .await;
    assert!(
        before.members.contains_key(&mfp),
        "timeboxed member is present before their expiry"
    );

    // Derive AFTER expiry with the SAME op-log (SV unchanged, no block) — the cache must
    // NOT serve the stale 'present' set; the timebox drops the member.
    let after = store
        .derived_membership("kbc", &coll, &anchor, expiry + 1)
        .await;
    assert!(
        !after.members.contains_key(&mfp),
        "a timeboxed-out member MUST be dropped once wall-clock passes expires_at, \
         even when the op-log is unchanged and the cache is warm (fail-open guard)"
    );
    assert!(
        after.members.contains_key(&owner_fp),
        "the owner (no timebox) remains a member across the expiry boundary"
    );
}
