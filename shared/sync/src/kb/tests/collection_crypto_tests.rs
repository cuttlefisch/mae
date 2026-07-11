//! `KbCollectionDoc` E2E crypto authoring tests, part 1 (mirrors
//! `collection_crypto.rs`): legacy-schema migration, E2E genesis, member
//! admit, and identity rebind + recovery-key-signed rebind.

use super::*;
use crate::membership::MembershipAction;

#[test]
fn migrate_v1_resolves_labels_to_principals() {
    // Build a legacy v1 collection (label creator + members YArray).
    let mut coll = KbCollectionDoc::new("KB", "alice");
    coll.add_member("bob");
    assert_eq!(coll.schema_version(), 0, "legacy = no schema key");
    // resolver maps known labels to fingerprints.
    let resolver = |label: &str| match label {
        "alice" => Some(("SHA256:alice".to_string(), "alice".to_string())),
        "bob" => Some(("SHA256:bob".to_string(), "bob".to_string())),
        _ => None,
    };
    let update = coll.migrate_if_legacy(resolver).expect("migrated");
    assert!(!update.is_empty());
    assert_eq!(coll.schema_version(), 2);
    assert_eq!(coll.owner(), "SHA256:alice");
    assert_eq!(coll.role_of("SHA256:alice"), Some(Role::Owner));
    assert_eq!(coll.role_of("SHA256:bob"), Some(Role::Editor));
    assert_eq!(coll.join_policy(), JoinPolicy::Invite);
    // idempotent
    assert!(coll.migrate_if_legacy(resolver).is_none());
}

#[test]
fn migrate_v1_unresolved_label_falls_back_to_legacy_principal() {
    let mut coll = KbCollectionDoc::new("KB", "alice");
    coll.add_member("ghost");
    // resolver knows nobody → legacy:<label> principals.
    coll.migrate_if_legacy(|_| None).expect("migrated");
    assert_eq!(coll.schema_version(), 2);
    assert_eq!(coll.owner(), "legacy:alice");
    assert_eq!(coll.role_of("legacy:alice"), Some(Role::Owner));
    assert_eq!(coll.role_of("legacy:ghost"), Some(Role::Editor));
}

#[test]
fn author_e2e_genesis_signs_encryption_self_wraps_and_relays_to_peers() {
    use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use crate::membership::{derive_content_key, derive_encryption};
    let (secret, pubkey, owner_fp) = oplog_keypair(1);
    let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
    assert_eq!(
        derive_encryption(&coll.oplog_ops(), &pubkey),
        Encryption::None,
        "unencrypted before enable"
    );

    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&secret)).unwrap();
    let delta = coll.author_e2e_genesis("KB", &owner_fp, &secret, &pubkey, self_wrap, 1000);

    // Authoritative mode is the SIGNED op-log; the unsigned flag mirrors it; and the
    // owner recovers its OWN self-wrapped content key from the log.
    assert_eq!(
        derive_encryption(&coll.oplog_ops(), &pubkey),
        Encryption::E2e,
        "signed SetEncryption(e2e) latched"
    );
    assert_eq!(
        coll.encryption(),
        Encryption::E2e,
        "unsigned flag set for display"
    );
    assert_eq!(
        derive_content_key(&coll.oplog_ops(), &pubkey, &owner_fp, &secret).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "owner recovers its self-wrapped key"
    );

    // A peer applying the shipped delta to a fresh replica derives the SAME signed
    // state (the daemon relays this delta key-blind).
    let mut peer = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
    peer.apply_update(&delta).unwrap();
    assert_eq!(
        derive_encryption(&peer.oplog_ops(), &pubkey),
        Encryption::E2e,
        "peer derives e2e from the relayed delta"
    );
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &pubkey, &owner_fp, &secret).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the owner on a fresh replica still recovers the key"
    );

    // A different identity (a non-member) recovers nothing — no wrap targets them.
    let (other_secret, _other_pubkey, other_fp) = oplog_keypair(2);
    assert!(
        derive_content_key(&coll.oplog_ops(), &pubkey, &other_fp, &other_secret).is_none(),
        "a non-member recovers no content key"
    );

    // Idempotent: a second enable on the already-genesis'd KB adds no second genesis.
    let len_before = coll.oplog_len();
    let k2_wrap = wrap_to_member(&k, &wrap_public_for(&secret)).unwrap();
    coll.author_e2e_genesis("KB", &owner_fp, &secret, &pubkey, k2_wrap, 1001);
    assert!(coll.oplog_len() >= len_before, "re-enable never DROPS ops");
    assert_eq!(
        derive_encryption(&coll.oplog_ops(), &pubkey),
        Encryption::E2e,
        "still e2e after re-enable"
    );
}

#[test]
fn author_member_admit_delivers_the_key_stores_pubkey_and_keeps_epoch_consistent() {
    use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use crate::membership::derive_content_key;
    let (osec, opk, ofp) = oplog_keypair(1);
    let (msec, mpk, mfp) = oplog_keypair(2);
    let (_xsec, _xpk, xfp) = oplog_keypair(3); // a non-member
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let k = ContentKey::generate();
    // Owner enables (genesis + self-wrap).
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    coll.author_e2e_genesis("KB", &ofp, &osec, &opk, self_wrap, 1000);

    // Owner admits a member, wrapping the content key to THEM.
    let member_wrap = wrap_to_member(&k, &wrap_public_for(&msec)).unwrap();
    let delta = coll.author_member_admit(
        "KB",
        &mfp,
        &mpk,
        &wrap_public_for(&msec),
        Role::Editor,
        "m",
        member_wrap,
        &ofp,
        &osec,
        &opk,
        1001,
    );

    // The member is now Editor, recovers the SAME content key, and their pubkey is
    // stored (for re-wrap on rotation, 3c).
    assert_eq!(coll.role_of(&mfp), Some(Role::Editor));
    assert_eq!(
        derive_content_key(&coll.oplog_ops(), &opk, &mfp, &msec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the admitted member recovers the content key"
    );
    assert_eq!(
        coll.member_pubkey(&mfp),
        Some(mpk),
        "member pubkey stored for re-wrap"
    );

    // Epoch consistency (the dual-write): the member's signed Admit op carries the SAME
    // epoch as the member_roles entry the daemon's fence reads.
    let admit = coll
        .oplog_ops()
        .into_iter()
        .find(|o| o.op.subject == mfp && o.op.action == MembershipAction::Admit)
        .unwrap();
    assert_eq!(
        admit.op.epoch,
        coll.epoch_of(&mfp),
        "op epoch == member_roles epoch"
    );

    // A peer with the relayed collection agrees (the admit delta is incremental on
    // top of the genesis a peer already holds; a member with the full collection
    // derives the key). `delta` is non-empty (it carries the admit).
    assert!(
        !delta.is_empty(),
        "the admit produces a non-empty delta to ship"
    );
    let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &opk, &mfp, &msec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the member recovers the key on a peer replica too"
    );
    assert!(
        derive_content_key(&coll.oplog_ops(), &opk, &xfp, &_xsec).is_none(),
        "a non-member recovers no key"
    );
}

/// ADR-040 §1-3 end-to-end through the yrs doc: a member rotates (`author_rebind`,
/// the v3 op survives serialization via `op_from_map`), the owner re-wraps the content
/// key to the successor (`author_rebind_rewrap`), and a FRESH PEER replica derives the
/// rotated membership + delivers the key to the new identity. Adversarial oracles:
/// the predecessor is retired from membership; a non-member still recovers nothing.
#[test]
fn rebind_rotates_identity_and_owner_rewrap_delivers_key_through_a_peer_replica() {
    use crate::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use crate::membership::{derive_content_key, derive_valid_members};
    let (osec, opk, ofp) = oplog_keypair(1);
    let (bsec, bpk, bfp) = oplog_keypair(2); // bob's OLD identity
    let (b2sec, b2pk, b2fp) = oplog_keypair(3); // bob's NEW identity
    let (_xsec, _xpk, xfp) = oplog_keypair(4); // a non-member
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let k = ContentKey::generate();

    // Owner enables e2e + admits bob (Editor), wrapping the key to bob's OLD wrap key.
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    coll.author_e2e_genesis("KB", &ofp, &osec, &opk, self_wrap, 1000);
    let bob_wrap = wrap_to_member(&k, &wrap_public_for(&bsec)).unwrap();
    coll.author_member_admit(
        "KB",
        &bfp,
        &bpk,
        &wrap_public_for(&bsec),
        Role::Editor,
        "bob",
        bob_wrap,
        &ofp,
        &osec,
        &opk,
        1001,
    );
    assert_eq!(coll.role_of(&bfp), Some(Role::Editor));

    // Bob rotates his identity: the OLD key cross-signs the NEW key.
    coll.author_rebind(
        "KB",
        &bfp,
        &b2fp,
        &b2pk,
        &wrap_public_for(&b2sec),
        &bsec,
        &bpk,
        1002,
    );
    // Membership transfers to the successor; the predecessor is retired.
    let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
    assert!(!m.contains_key(&bfp), "predecessor retired");
    assert_eq!(
        m.get(&b2fp).map(|x| x.role),
        Some(Role::Editor),
        "successor inherits Editor"
    );
    // ...but the successor can't read yet (no wrap targets the new key).
    assert!(
        derive_content_key(&coll.oplog_ops(), &opk, &b2fp, &b2sec).is_none(),
        "successor cannot decrypt until the owner re-wraps"
    );

    // Owner observes the rebind and re-wraps the CURRENT key to the successor's wrap key.
    let succ_wrap = wrap_to_member(&k, &wrap_public_for(&b2sec)).unwrap();
    // Member rotation re-wrapped by the STABLE owner: anchor == signer == owner.
    coll.author_rebind_rewrap("KB", &b2fp, &b2pk, succ_wrap, &opk, &ofp, &osec, &opk, 1003);

    // The successor now recovers the SAME content key — proven on a FRESH PEER replica
    // (the v3 Rebind op + the re-wrap Admit both survived yrs serialization).
    let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
    let pm = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
    assert!(!pm.contains_key(&bfp), "peer agrees: predecessor retired");
    assert!(pm.contains_key(&b2fp), "peer agrees: successor present");
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &opk, &b2fp, &b2sec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the rotated member recovers the content key on a peer replica"
    );
    // A non-member still recovers nothing (selective oracle).
    assert!(
        derive_content_key(&peer.oplog_ops(), &opk, &xfp, &_xsec).is_none(),
        "a non-member recovers no key after rotation"
    );
    // Planned-rotation property (ADR-040 threat model): the OLD key, still held by the
    // user during a planned rotation, retains read access to pre-rotation content (its
    // original wrap is untouched — only a §D3 rotation revokes that).
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &opk, &bfp, &bsec).map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "old key retains read access to history (planned rotation, not §D3 revocation)"
    );
}

/// ADR-040 §Recovery-key (the attacker's test): a member registers an offline recovery
/// key R (signed by its primary), LOSES the primary, and rotates using R — honored. A
/// FORGER who lacks R cannot rotate the member, even authoring the identical Rebind shape.
#[test]
fn recovery_key_signed_rebind_is_honored_and_forgery_is_rejected() {
    use crate::content_crypto::wrap_public_for;
    use crate::membership::derive_valid_members;
    let (osec, opk, ofp) = oplog_keypair(1);
    let (msec, mpk, mfp) = oplog_keypair(2); // member's primary
    let (rsec, rpk, _rfp) = oplog_keypair(3); // member's OFFLINE recovery key
    let (s2sec, s2pk, s2fp) = oplog_keypair(4); // the recovered successor
    let (zsec, zpk, _zfp) = oplog_keypair(9); // a forger's key (NOT R)
    let (g2sec, g2pk, g2fp) = oplog_keypair(10); // the forger's would-be successor

    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    coll.author_e2e_genesis(
        "KB",
        &ofp,
        &osec,
        &opk,
        crate::content_crypto::wrap_to_member(
            &crate::content_crypto::ContentKey::generate(),
            &wrap_public_for(&osec),
        )
        .unwrap(),
        1000,
    );
    // Owner admits the member (Editor), no content wrap needed for this membership test.
    coll.author_member_admit(
        "KB",
        &mfp,
        &mpk,
        &wrap_public_for(&msec),
        Role::Editor,
        "m",
        crate::content_crypto::wrap_to_member(
            &crate::content_crypto::ContentKey::generate(),
            &wrap_public_for(&msec),
        )
        .unwrap(),
        &ofp,
        &osec,
        &opk,
        1001,
    );
    // The member registers its recovery key R (signed by its PRIMARY).
    coll.author_register_recovery_key("KB", &mfp, &rpk, &msec, &mpk, 1002);

    // PRIMARY LOST. The member rotates m → s2 using the RECOVERY key R.
    coll.author_recovery_rebind(
        "KB",
        &mfp,
        &s2fp,
        &s2pk,
        &wrap_public_for(&s2sec),
        &rsec,
        &rpk,
        1003,
    );

    // A FORGER, lacking R, authors the same-shaped recovery Rebind m → g2 with key Z.
    coll.author_recovery_rebind(
        "KB",
        &mfp,
        &g2fp,
        &g2pk,
        &wrap_public_for(&g2sec),
        &zsec,
        &zpk,
        1004,
    );

    // On a FRESH peer: the recovery-key rotation is honored (s2 inherits Editor, m retired);
    // the forged one is NOT (g2 is not a member — the registry binds recovery to R alone).
    let peer = KbCollectionDoc::from_bytes(&coll.encode_state()).unwrap();
    let m = derive_valid_members(&peer.oplog_ops(), &opk, 2000);
    assert_eq!(
        m.get(&s2fp).map(|x| x.role),
        Some(Role::Editor),
        "the recovery-key-signed rotation is honored; successor inherits Editor"
    );
    assert!(!m.contains_key(&mfp), "the recovered primary is retired");
    assert!(
        !m.contains_key(&g2fp),
        "a forger without the recovery key cannot rotate the member"
    );
}
