//! `KbCollectionDoc` signed op-log tests (mirrors `collection_oplog.rs`).

use super::*;
use crate::membership::MembershipAction;

#[test]
fn oplog_append_read_roundtrips_and_verifies() {
    let (secret, pubkey, owner_fp) = oplog_keypair(1);
    let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
    assert!(coll.oplog_head().is_none(), "empty log has no head");

    // Genesis: the owner admits themselves (self-signed).
    let op = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &owner_fp,
        Some(Role::Owner),
        true,
        &owner_fp,
        1000,
        None,
        0,
    );
    assert_eq!(op.prev_hash, "", "genesis op has empty prev_hash");
    let sig = op.sign(&secret);
    coll.append_signed_op(&op, &sig, &pubkey);

    let ops = coll.oplog_ops();
    assert_eq!(ops.len(), 1);
    let rec = &ops[0];
    assert!(rec.verify_signed(), "round-tripped record verifies");
    assert_eq!(rec.op.subject, owner_fp);
    assert_eq!(rec.op.role, Some(Role::Owner));
    assert!(rec.op.can_invite);
    assert_eq!(rec.op.kb_id, "KB");
    assert_eq!(
        coll.oplog_head(),
        Some(rec.chain_hash()),
        "head is the lone op"
    );
    // Re-appending the identical signed op is idempotent (keyed by chain_hash).
    coll.append_signed_op(&op, &sig, &pubkey);
    assert_eq!(
        coll.oplog_len(),
        1,
        "same op re-append is a no-op set insert"
    );
}

#[test]
fn oplog_head_advances_along_the_chain() {
    let (osec, opub, owner_fp) = oplog_keypair(1);
    let (_bsec, _bpub, bob_fp) = oplog_keypair(2);
    let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");

    let g = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &owner_fp,
        Some(Role::Owner),
        true,
        &owner_fp,
        1,
        None,
        0,
    );
    let gsig = g.sign(&osec);
    coll.append_signed_op(&g, &gsig, &opub);
    let ghash = g.chain_hash(&gsig);
    assert_eq!(coll.oplog_head(), Some(ghash.clone()));

    // Owner admits bob; the new op chains off the genesis head.
    let a = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &bob_fp,
        Some(Role::Editor),
        false,
        &owner_fp,
        2,
        None,
        0,
    );
    assert_eq!(a.prev_hash, ghash, "second op chains off genesis");
    let asig = a.sign(&osec);
    coll.append_signed_op(&a, &asig, &opub);
    assert_eq!(coll.oplog_len(), 2);
    assert_eq!(
        coll.oplog_head(),
        Some(a.chain_hash(&asig)),
        "head advanced to the admit"
    );
}

#[test]
fn oplog_concurrent_appends_converge_as_a_set() {
    let (osec, opub, owner_fp) = oplog_keypair(1);
    let (_sx, _px, x_fp) = oplog_keypair(2);
    let (_sy, _py, y_fp) = oplog_keypair(3);

    let mut base = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");
    let g = base.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &owner_fp,
        Some(Role::Owner),
        true,
        &owner_fp,
        1,
        None,
        0,
    );
    let gsig = g.sign(&osec);
    base.append_signed_op(&g, &gsig, &opub);
    let state = base.encode_state();

    // Two replicas concurrently admit DIFFERENT subjects, both off genesis.
    let mut a = KbCollectionDoc::from_bytes(&state).unwrap();
    let mut b = KbCollectionDoc::from_bytes(&state).unwrap();
    let opx = a.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &x_fp,
        Some(Role::Editor),
        false,
        &owner_fp,
        2,
        None,
        0,
    );
    let sx = opx.sign(&osec);
    let ux = a.append_signed_op(&opx, &sx, &opub);
    let opy = b.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &y_fp,
        Some(Role::Editor),
        false,
        &owner_fp,
        3,
        None,
        0,
    );
    let sy = opy.sign(&osec);
    let uy = b.append_signed_op(&opy, &sy, &opub);

    // Cross-apply the concurrent updates.
    a.apply_update(&uy).unwrap();
    b.apply_update(&ux).unwrap();

    // Both replicas hold all three ops (set union; no lost append).
    assert_eq!(a.oplog_len(), 3);
    assert_eq!(b.oplog_len(), 3);
    // The deterministic head pick (highest-hash tip) agrees on both peers.
    assert_eq!(a.oplog_head(), b.oplog_head());
}

#[test]
fn oplog_record_with_mismatched_pubkey_fails_verify() {
    let (osec, _opub, owner_fp) = oplog_keypair(1);
    let (_msec, mpub, _mfp) = oplog_keypair(9);
    let mut coll = KbCollectionDoc::new_owned("KB", &owner_fp, "alice");

    // The op names + is signed by the owner, but the record stores a DIFFERENT
    // author_pubkey (a relay swapping the key). Decode succeeds; verify fails.
    let op = coll.build_membership_op(
        "KB",
        MembershipAction::Admit,
        &owner_fp,
        Some(Role::Owner),
        true,
        &owner_fp,
        1,
        None,
        0,
    );
    let sig = op.sign(&osec);
    coll.append_signed_op(&op, &sig, &mpub); // wrong pubkey stored

    let ops = coll.oplog_ops();
    assert_eq!(ops.len(), 1);
    assert!(
        !ops[0].verify_signed(),
        "fingerprint(author_pubkey) != author ⇒ record rejected"
    );
}
