//! `KbCollectionDoc` E2E crypto authoring tests, part 2 (mirrors
//! `collection_crypto.rs`): recovery-key registration/rebind attack
//! lifecycle — unregistered key, non-primary registration, key rotation,
//! and post-removal recovery.

use super::*;
use crate::membership::MembershipAction;

/// ADR-040 §Recovery-key: with NO registration, a Rebind for a principal signed by any
/// non-primary key is not honored — recovery requires a pre-registered key.
#[test]
fn recovery_rebind_without_registration_is_rejected() {
    use crate::content_crypto::wrap_public_for;
    use crate::membership::derive_valid_members;
    let (osec, opk, ofp) = oplog_keypair(1);
    let (_msec, _mpk, mfp) = oplog_keypair(2);
    let (rsec, rpk, _rfp) = oplog_keypair(3); // an UNREGISTERED key
    let (s2sec, s2pk, s2fp) = oplog_keypair(4);
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let g = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &ofp,
        Some(Role::Owner),
        true,
        &ofp,
        1000,
        None,
        0,
    );
    let gs = g.sign(&osec);
    coll.append_signed_op(&g, &gs, &opk);
    let a = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &mfp,
        Some(Role::Editor),
        false,
        &ofp,
        1001,
        None,
        0,
    );
    let as_ = a.sign(&osec);
    coll.append_signed_op(&a, &as_, &opk);
    // No author_register_recovery_key. Try to recover m → s2 with an unregistered key.
    coll.author_recovery_rebind(
        "KB",
        &mfp,
        &s2fp,
        &s2pk,
        &wrap_public_for(&s2sec),
        &rsec,
        &rpk,
        1002,
    );
    let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
    assert!(
        !m.contains_key(&s2fp),
        "no registration ⇒ recovery rebind ignored"
    );
    assert_eq!(
        m.get(&mfp).map(|x| x.role),
        Some(Role::Editor),
        "the member is unchanged"
    );
}

/// ADR-040 §Recovery-key: a registration is only honored when signed by the PRINCIPAL'S
/// PRIMARY (verify_signed). A registration "for m" signed by an attacker's key never enters
/// the registry, so a Rebind signed by that attacker key is not honored.
#[test]
fn recovery_registration_requires_the_primary() {
    use crate::content_crypto::wrap_public_for;
    use crate::membership::derive_valid_members;
    let (osec, opk, ofp) = oplog_keypair(1);
    let (_msec, _mpk, mfp) = oplog_keypair(2);
    let (atksec, atkpk, _atkfp) = oplog_keypair(8); // attacker key
    let (s2sec, s2pk, s2fp) = oplog_keypair(4);
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let g = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &ofp,
        Some(Role::Owner),
        true,
        &ofp,
        1000,
        None,
        0,
    );
    let gs = g.sign(&osec);
    coll.append_signed_op(&g, &gs, &opk);
    let a = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &mfp,
        Some(Role::Editor),
        false,
        &ofp,
        1001,
        None,
        0,
    );
    let as_ = a.sign(&osec);
    coll.append_signed_op(&a, &as_, &opk);
    // Attacker forges a RegisterRecoveryKey for m, authorizing ITS OWN key as recovery —
    // but signs with the attacker key (not m's primary). subject=m, author=m, signed by atk.
    let mut reg = coll.build_membership_op(
        "KB",
        MembershipAction::RegisterRecoveryKey,
        &mfp,
        None,
        false,
        &mfp,
        1002,
        None,
        0,
    );
    reg.recovery_pubkey = Some(atkpk);
    let regsig = reg.sign(&atksec); // WRONG signer (not m's primary)
    coll.append_signed_op(&reg, &regsig, &atkpk);
    // Attacker now tries to recover m → s2 with its key.
    coll.author_recovery_rebind(
        "KB",
        &mfp,
        &s2fp,
        &s2pk,
        &wrap_public_for(&s2sec),
        &atksec,
        &atkpk,
        1003,
    );
    let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
    assert!(
        !m.contains_key(&s2fp),
        "a registration not signed by the primary is ignored"
    );
}

/// ADR-040 §Recovery-key: latest registration wins (revoke a leaked recovery key). After
/// R1 is superseded by R2, a Rebind signed by R1 is rejected while one signed by R2 is
/// honored. Two independent collections (same op history up to the competing rebind) so the
/// rejected branch doesn't causally orphan the honored one.
#[test]
fn latest_recovery_key_registration_wins() {
    use crate::content_crypto::wrap_public_for;
    use crate::membership::derive_valid_members;
    let (osec, opk, ofp) = oplog_keypair(1);
    let (msec, mpk, mfp) = oplog_keypair(2);
    let (r1sec, r1pk, _r1fp) = oplog_keypair(3); // first (leaked) recovery key
    let (r2sec, r2pk, _r2fp) = oplog_keypair(5); // replacement recovery key
    let (ssec, spk, sfp) = oplog_keypair(4); // the would-be successor

    // Build the shared prefix: owner genesis + admit m + register R1 + supersede with R2.
    let seed = |label: &str| {
        let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
        let g = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &ofp,
            Some(Role::Owner),
            true,
            &ofp,
            1000,
            None,
            0,
        );
        let gs = g.sign(&osec);
        coll.append_signed_op(&g, &gs, &opk);
        let a = coll.build_membership_op(
            "KB",
            MembershipAction::Admit,
            &mfp,
            Some(Role::Editor),
            false,
            &ofp,
            1001,
            None,
            0,
        );
        let as_ = a.sign(&osec);
        coll.append_signed_op(&a, &as_, &opk);
        coll.author_register_recovery_key("KB", &mfp, &r1pk, &msec, &mpk, 1002);
        coll.author_register_recovery_key("KB", &mfp, &r2pk, &msec, &mpk, 1003); // supersedes R1
        let _ = label;
        coll
    };

    // Branch A: rotate with the SUPERSEDED R1 → rejected (m unchanged).
    let mut a = seed("a");
    a.author_recovery_rebind(
        "KB",
        &mfp,
        &sfp,
        &spk,
        &wrap_public_for(&ssec),
        &r1sec,
        &r1pk,
        1004,
    );
    let ma = derive_valid_members(&a.oplog_ops(), &opk, 2000);
    assert!(
        !ma.contains_key(&sfp),
        "a Rebind signed by the SUPERSEDED recovery key is rejected"
    );
    assert_eq!(
        ma.get(&mfp).map(|x| x.role),
        Some(Role::Editor),
        "the member is unchanged under the revoked key"
    );

    // Branch B: rotate with the CURRENT R2 → honored (m → successor).
    let mut b = seed("b");
    b.author_recovery_rebind(
        "KB",
        &mfp,
        &sfp,
        &spk,
        &wrap_public_for(&ssec),
        &r2sec,
        &r2pk,
        1004,
    );
    let mb = derive_valid_members(&b.oplog_ops(), &opk, 2000);
    assert_eq!(
        mb.get(&sfp).map(|x| x.role),
        Some(Role::Editor),
        "the CURRENT recovery key rotates the member"
    );
    assert!(
        !mb.contains_key(&mfp),
        "and the recovered member key is retired"
    );
}

/// ADR-040 §Recovery-key: a REMOVED member cannot recover — `authorized`'s Rebind arm
/// still requires the recovered principal be a current member, independent of who signed.
#[test]
fn removed_member_cannot_recover() {
    use crate::content_crypto::wrap_public_for;
    use crate::membership::derive_valid_members;
    let (osec, opk, ofp) = oplog_keypair(1);
    let (msec, mpk, mfp) = oplog_keypair(2);
    let (rsec, rpk, _rfp) = oplog_keypair(3);
    let (s2sec, s2pk, s2fp) = oplog_keypair(4);
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
    let g = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &ofp,
        Some(Role::Owner),
        true,
        &ofp,
        1000,
        None,
        0,
    );
    let gs = g.sign(&osec);
    coll.append_signed_op(&g, &gs, &opk);
    let a = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &mfp,
        Some(Role::Editor),
        false,
        &ofp,
        1001,
        None,
        0,
    );
    let as_ = a.sign(&osec);
    coll.append_signed_op(&a, &as_, &opk);
    coll.author_register_recovery_key("KB", &mfp, &rpk, &msec, &mpk, 1002);
    // Owner removes m.
    let rm = coll.build_membership_op(
        "KB",
        MembershipAction::Remove,
        &mfp,
        None,
        false,
        &ofp,
        1003,
        None,
        0,
    );
    let rmsig = rm.sign(&osec);
    coll.append_signed_op(&rm, &rmsig, &opk);
    // m tries to recover via R → rejected (not a current member).
    coll.author_recovery_rebind(
        "KB",
        &mfp,
        &s2fp,
        &s2pk,
        &wrap_public_for(&s2sec),
        &rsec,
        &rpk,
        1004,
    );
    let m = derive_valid_members(&coll.oplog_ops(), &opk, 2000);
    assert!(
        !m.contains_key(&s2fp),
        "a removed member cannot recover its seat via the recovery key"
    );
    assert!(
        !m.contains_key(&mfp),
        "and the removed member stays removed"
    );
}
