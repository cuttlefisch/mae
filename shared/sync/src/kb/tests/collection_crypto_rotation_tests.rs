//! `KbCollectionDoc` E2E crypto authoring tests, part 3 (mirrors
//! `collection_crypto.rs`): owner recovery via recovery key, owner
//! self-rotation, and ADR-037 §D3 rotate-on-remove re-keying.

use super::*;
use crate::membership::MembershipAction;

/// ADR-040 §Recovery-key (owner recovery): the OWNER recovers via its recovery key; the
/// owner chain + governance/encryption readers honor the recovery-signed Rebind, so the
/// successor is resolved as Owner and the KB stays E2e.
#[test]
fn owner_recovery_via_recovery_key_preserves_owner_chain() {
    use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use crate::membership::{derive_encryption, derive_valid_members};
    let (osec, opk, ofp) = oplog_keypair(1);
    let (rsec, rpk, _rfp) = oplog_keypair(3); // owner's offline recovery key
    let (o2sec, o2pk, o2fp) = oplog_keypair(2); // recovered owner identity
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let k = ContentKey::generate();
    coll.author_e2e_genesis(
        "KB",
        &ofp,
        &osec,
        &opk,
        wrap_to_member(&k, &wrap_public_for(&osec)).unwrap(),
        1000,
    );
    coll.author_register_recovery_key("KB", &ofp, &rpk, &osec, &opk, 1001);
    // Owner primary LOST → recover owner → owner2 via R.
    coll.author_recovery_rebind(
        "KB",
        &ofp,
        &o2fp,
        &o2pk,
        &wrap_public_for(&o2sec),
        &rsec,
        &rpk,
        1002,
    );
    let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
    let m = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
    assert_eq!(
        m.get(&o2fp).map(|x| x.role),
        Some(Role::Owner),
        "recovered owner identity is Owner"
    );
    assert!(!m.contains_key(&ofp), "the lost owner key is retired");
    assert_eq!(
        derive_encryption(&peer.oplog_ops(), &opk),
        Encryption::E2e,
        "the KB stays E2e across owner recovery (owner chain honors the recovery Rebind)"
    );
}

/// ADR-040 PR2b (owner self-rotation): the OWNER rotates their own identity on an E2e KB
/// they own. The Rebind is signed by the OLD owner key (still valid at the rebind's causal
/// point); the content-key re-wrap to the NEW owner key must be signed by the NEW key (the
/// old is retired the instant the Rebind lands) while derivation still anchors on the
/// ORIGINAL genesis owner pubkey — the `anchor != signer` case. Proven on a fresh peer
/// replica: owner2 is Owner, the predecessor owner is retired, owner2 decrypts, and the
/// KB is still latched E2e via the owner chain.
#[test]
fn owner_self_rotation_rewraps_to_new_key_anchored_on_genesis() {
    use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use crate::membership::{derive_content_key, derive_encryption, derive_valid_members};
    let (osec, opk, ofp) = oplog_keypair(1); // genesis owner (the anchor, forever)
    let (o2sec, o2pk, o2fp) = oplog_keypair(2); // owner's NEW identity
    let (_xsec, _xpk, xfp) = oplog_keypair(3); // a non-member
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let k = ContentKey::generate();
    // Owner enables e2e (genesis self-wrap to the owner's OLD wrap key).
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    coll.author_e2e_genesis("KB", &ofp, &osec, &opk, self_wrap, 1000);

    // Owner rotates: the OLD owner key signs Rebind(owner → owner2).
    coll.author_rebind(
        "KB",
        &ofp,
        &o2fp,
        &o2pk,
        &wrap_public_for(&o2sec),
        &osec,
        &opk,
        1001,
    );
    // Owner re-wraps the content key to the NEW owner key — signed by owner2, anchored on
    // the OLD genesis pubkey (anchor != signer).
    let new_wrap = wrap_to_member(&k, &wrap_public_for(&o2sec)).unwrap();
    coll.author_rebind_rewrap(
        "KB", &o2fp, &o2pk, new_wrap, /*anchor*/ &opk, /*signer*/ &o2fp, &o2sec, &o2pk,
        1002,
    );

    // Fresh peer replica: derive everything from the anchored (OLD) genesis pubkey.
    let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
    let m = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
    assert!(!m.contains_key(&ofp), "predecessor owner retired");
    assert_eq!(
        m.get(&o2fp).map(|x| x.role),
        Some(Role::Owner),
        "successor inherits Owner"
    );
    assert_eq!(
        derive_encryption(&peer.oplog_ops(), &opk),
        Encryption::E2e,
        "still e2e via the owner chain after rotation"
    );
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &opk, &o2fp, &o2sec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "rotated owner recovers the content key with the new key"
    );
    assert!(
        derive_content_key(&peer.oplog_ops(), &opk, &xfp, &_xsec).is_none(),
        "a non-member recovers nothing"
    );
}

// Regression for the join-decrypt bug (branch `fix/joiner-content-sync`): the OWNER must
// author the member `Admit` against the CURRENT collection lineage (the network task's fresh
// replica that already holds the genesis + SetEncryption it authored at enable) — NOT a STALE
// pre-enable snapshot. Authoring against a stale base (which has no oplog map) re-creates the
// oplog `MapPrelim` root; when the key-blind daemon merges that delta it TOMBSTONES the live
// genesis/SetEncryption ops, the admit becomes a phantom second-genesis (empty `prev_hash`),
// and the joiner gets a corrupt op-log it can't derive a key from. This pins the FIX (chain
// cleanly, converge, member derives) and the precise root-cause property (the admit CHAINS
// onto the SetEncryption head rather than masquerading as a genesis).
#[test]
fn member_admit_must_chain_on_the_current_collection_not_a_stale_pre_enable_base() {
    use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use crate::membership::{derive_content_key, derive_encryption};
    let (osec, opk, ofp) = oplog_keypair(11);
    let (msec, mpk, mfp) = oplog_keypair(22);

    // 1) Owner shares: owner set, NO membership op-log yet (mirror of the share path).
    let shared = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let shared_state = shared.encode_state();
    // The key-blind daemon relays the owner-signed collection bytes verbatim.
    let mut daemon = KbCollectionDoc::from_bytes(&shared_state).unwrap();

    // 2) Owner ENABLES e2e against a reconstruction of the shared collection.
    let k = ContentKey::generate();
    let mut live = KbCollectionDoc::from_bytes(&shared_state).unwrap();
    let enable_delta = live.author_e2e_genesis(
        "KB",
        &ofp,
        &osec,
        &opk,
        wrap_to_member(&k, &wrap_public_for(&osec)).unwrap(),
        1000,
    );
    daemon.apply_update(&enable_delta).unwrap();
    assert_eq!(
        daemon.oplog_ops().len(),
        2,
        "daemon holds genesis + SetEncryption after enable"
    );
    let head_before_admit = {
        // The op the admit MUST chain onto (the SetEncryption, latest in causal order).
        let ops = daemon.oplog_ops();
        ops.iter()
            .find(|o| o.op.action == MembershipAction::SetEncryption)
            .map(|o| o.chain_hash())
            .expect("SetEncryption present")
    };

    // 3a) ROOT-CAUSE GUARD — authoring against the STALE pre-enable base produces a phantom
    //     genesis (empty prev_hash): it has NO knowledge of the genesis/SetEncryption head.
    {
        let mut stale = KbCollectionDoc::from_bytes(&shared_state).unwrap(); // pre-enable!
        stale.author_member_admit(
            "KB",
            &mfp,
            &mpk,
            &wrap_public_for(&msec),
            Role::Editor,
            "m",
            wrap_to_member(&k, &wrap_public_for(&msec)).unwrap(),
            &ofp,
            &osec,
            &opk,
            1001,
        );
        let admit = stale
            .oplog_ops()
            .into_iter()
            .find(|o| o.op.subject == mfp)
            .expect("admit authored");
        assert!(
            admit.op.prev_hash.is_empty(),
            "stale-base admit masquerades as a genesis (the bug) — empty prev_hash"
        );
    }

    // 3b) THE FIX — author against the CURRENT collection (the daemon's lineage). The admit
    //     CHAINS onto the SetEncryption head; merging it is purely additive (no tombstone).
    let mut current = KbCollectionDoc::from_bytes(&daemon.encode_state()).unwrap();
    let good_delta = current.author_member_admit(
        "KB",
        &mfp,
        &mpk,
        &wrap_public_for(&msec),
        Role::Editor,
        "m",
        wrap_to_member(&k, &wrap_public_for(&msec)).unwrap(),
        &ofp,
        &osec,
        &opk,
        1001,
    );
    let admit = current
        .oplog_ops()
        .into_iter()
        .find(|o| o.op.subject == mfp)
        .expect("admit authored");
    assert_eq!(
        admit.op.prev_hash, head_before_admit,
        "current-base admit chains onto the SetEncryption head (the fix)"
    );

    daemon.apply_update(&good_delta).unwrap();
    // Adversarial: a duplicate echo of our own op must be idempotent (the racy re-apply path).
    daemon.apply_update(&good_delta).unwrap();
    assert_eq!(
        daemon.oplog_ops().len(),
        3,
        "genesis + SetEncryption + admit all survive the merge (and the echo)"
    );

    // The joiner, with ONLY the daemon-relayed collection, derives the SAME content key AND
    // sees E2e mode intact — the user-visible success the bug denied.
    let joiner = KbCollectionDoc::from_bytes(&daemon.encode_state()).unwrap();
    assert_eq!(
        derive_encryption(&joiner.oplog_ops(), &opk),
        Encryption::E2e,
        "E2e mode survives (genesis anchor + SetEncryption intact)"
    );
    assert_eq!(
        derive_content_key(&joiner.oplog_ops(), &opk, &mfp, &msec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the approved member derives the content key from the relayed collection"
    );
    // A non-member still derives nothing.
    let (xsec, _xpk, xfp) = oplog_keypair(33);
    assert!(
        derive_content_key(&joiner.oplog_ops(), &opk, &xfp, &xsec).is_none(),
        "a non-member recovers no key"
    );
}

#[test]
fn rotate_on_remove_rekeys_remaining_members_and_strands_the_removed_one() {
    // ADR-037 §D3 — the SELECTIVE security oracle. 3 members (owner + B + C) share key k.
    // Remove B with a fresh k'. The remaining two must CONVERGE on k' and the removed B
    // must keep ONLY the old k (reads nothing new), not break entirely — proving the
    // rotation denies k' specifically, rather than just severing B's pipeline.
    use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use crate::membership::{
        derive_content_key, derive_governance, derive_valid_members_governed, MembershipView,
    };
    let (osec, opk, ofp) = oplog_keypair(1);
    let (bsec, bpk, bfp) = oplog_keypair(2);
    let (csec, cpk, cfp) = oplog_keypair(3);

    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let k = ContentKey::generate();
    coll.author_e2e_genesis(
        "KB",
        &ofp,
        &osec,
        &opk,
        wrap_to_member(&k, &wrap_public_for(&osec)).unwrap(),
        1000,
    );
    coll.author_member_admit(
        "KB",
        &bfp,
        &bpk,
        &wrap_public_for(&bsec),
        Role::Editor,
        "b",
        wrap_to_member(&k, &wrap_public_for(&bsec)).unwrap(),
        &ofp,
        &osec,
        &opk,
        1001,
    );
    coll.author_member_admit(
        "KB",
        &cfp,
        &cpk,
        &wrap_public_for(&csec),
        Role::Editor,
        "c",
        wrap_to_member(&k, &wrap_public_for(&csec)).unwrap(),
        &ofp,
        &osec,
        &opk,
        1002,
    );
    // Everyone holds k before rotation.
    for (fp, sec) in [(&ofp, &osec), (&bfp, &bsec), (&cfp, &csec)] {
        assert_eq!(
            derive_content_key(&coll.oplog_ops(), &opk, fp, sec).map(|c| *c.as_bytes()),
            Some(*k.as_bytes()),
            "every member holds k before rotation"
        );
    }
    let c_epoch_before = coll.epoch_of(&cfp);
    // Exact pre-rotation replica (matching chain hashes) so the rotation DELTA grafts.
    let pre_rotation_state = coll.encode_state();

    // Rotate: remove B, re-wrap a FRESH k' to the remaining members (owner + C).
    let k2 = ContentKey::generate();
    assert_ne!(k.as_bytes(), k2.as_bytes(), "fresh rotation key");
    let rewraps = vec![
        (
            ofp.clone(),
            wrap_to_member(&k2, &wrap_public_for(&osec)).unwrap(),
        ),
        (
            cfp.clone(),
            wrap_to_member(&k2, &wrap_public_for(&csec)).unwrap(),
        ),
    ];
    let delta = coll.author_rotate_on_remove("KB", &bfp, &rewraps, &ofp, &osec, &opk, 2000);

    let ops = coll.oplog_ops();
    // (1) The two remaining members converge on k'.
    assert_eq!(
        derive_content_key(&ops, &opk, &ofp, &osec).map(|c| *c.as_bytes()),
        Some(*k2.as_bytes()),
        "owner re-keys to k'"
    );
    assert_eq!(
        derive_content_key(&ops, &opk, &cfp, &csec).map(|c| *c.as_bytes()),
        Some(*k2.as_bytes()),
        "remaining member C re-keys to k'"
    );
    // (2) THE ORACLE: the removed B still derives the OLD k (its last wrap) — NOT k', and
    // NOT nothing. It can decrypt pre-rotation content but no post-rotation ciphertext.
    assert_eq!(
        derive_content_key(&ops, &opk, &bfp, &bsec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "removed B keeps ONLY the old k — stranded from k'"
    );
    // (3) B is gone from derived membership; owner + C remain with UNCHANGED attributes
    // (the re-key Admit must not silently downgrade role/can_invite or bump epoch).
    let gov = derive_governance(&ops, &opk);
    let members = derive_valid_members_governed(&ops, &opk, 2000, gov, &MembershipView::default());
    assert!(!members.contains_key(&bfp), "B removed from membership");
    assert_eq!(members.len(), 2, "only owner + C remain");
    assert_eq!(members[&ofp].role, Role::Owner, "owner role preserved");
    assert!(
        members[&ofp].can_invite,
        "owner can_invite preserved (genesis)"
    );
    assert_eq!(members[&cfp].role, Role::Editor, "C role preserved");
    assert_eq!(
        members[&cfp].epoch, c_epoch_before,
        "C epoch NOT bumped by re-key"
    );

    // (4) Convergence: a replica at the pre-rotation state applies ONLY the relayed delta
    // (as the key-blind daemon ships it) and agrees on every point.
    let mut peer = KbCollectionDoc::from_bytes(&pre_rotation_state).unwrap();
    peer.apply_update(&delta).unwrap();
    let pops = peer.oplog_ops();
    assert_eq!(
        derive_content_key(&pops, &opk, &cfp, &csec).map(|c| *c.as_bytes()),
        Some(*k2.as_bytes()),
        "peer: C converges on k'"
    );
    assert_eq!(
        derive_content_key(&pops, &opk, &bfp, &bsec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "peer: removed B still stranded on old k"
    );
    assert!(
        !derive_valid_members_governed(
            &pops,
            &opk,
            2000,
            derive_governance(&pops, &opk),
            &MembershipView::default()
        )
        .contains_key(&bfp),
        "peer agrees B is removed"
    );

    // (5) The Remove op is genuinely owner-signed (not a daemon-forged membership change).
    let remove = ops
        .iter()
        .find(|o| o.op.action == MembershipAction::Remove && o.op.subject == bfp)
        .expect("a Remove op for B exists");
    assert!(
        remove.verify_signed(),
        "Remove is a verifiable owner signature"
    );
    assert_eq!(
        remove.author_pubkey, opk,
        "Remove authored by the owner key"
    );
}
