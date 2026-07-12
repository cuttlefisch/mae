//! Split from the monolithic `collab_bridge_tests.rs`: owner/member key rotation and reactive member rewrap.

use super::*;

/// ADR-040 PR2b: `plan_owner_rotation` produces, for every KB this peer OWNS, the deltas that
/// — applied to a fresh peer replica — retire the old owner, make the new key Owner, keep the
/// KB e2e, and let the new key decrypt. A KB owned by SOMEONE ELSE is skipped (that needs the
/// PR2c member-authored path). Selective oracles on a fresh replica, not the authoring doc.
#[test]
fn plan_owner_rotation_rotates_owned_e2e_kb_and_skips_unowned() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::{Encryption, KbCollectionDoc, Role};
    use mae_sync::membership::{derive_content_key, derive_encryption, derive_valid_members};
    use std::collections::HashMap;

    let old = Identity::from_seed(&[11u8; 32], "old");
    let new = Identity::from_seed(&[22u8; 32], "new");
    let stranger = Identity::from_seed(&[33u8; 32], "stranger");
    let (ofp, opk, osec) = (
        old.fingerprint(),
        old.public().to_bytes(),
        old.secret_bytes(),
    );

    // KB-A: an E2e KB I OWN (genesis owner == old).
    let mut a = KbCollectionDoc::new_owned("A", &ofp, "old");
    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    a.author_e2e_genesis("kb-a", &ofp, &osec, &opk, self_wrap, 1000);
    // KB-B: owned by SOMEONE ELSE — must be skipped (no path for a non-owner self-rebind yet).
    let b = KbCollectionDoc::new_owned("B", &stranger.fingerprint(), "stranger");

    let mut kb_collections = HashMap::new();
    kb_collections.insert("kb-a".to_string(), a.encode_state());
    kb_collections.insert("kb-b".to_string(), b.encode_state());
    let mut content_keys = HashMap::new();
    content_keys.insert("kb-a".to_string(), k.clone());

    let plans = plan_owner_rotation(&kb_collections, &content_keys, &old, &new, 2000);

    // Only the owned KB is planned, with two deltas (Rebind + e2e re-wrap).
    assert_eq!(
        plans.len(),
        1,
        "only the OWNED KB is rotated; the unowned one is skipped"
    );
    let plan = &plans[0];
    assert_eq!(plan.kb_id, "kb-a");
    assert_eq!(plan.deltas.len(), 2, "Rebind + E2e content-key re-wrap");

    // Apply the shipped deltas to a FRESH peer replica seeded from the pre-rotation state.
    let mut peer = KbCollectionDoc::from_bytes(&kb_collections["kb-a"]).unwrap();
    for d in &plan.deltas {
        peer.apply_update(d).unwrap();
    }
    let nfp = new.fingerprint();
    let m = derive_valid_members(&peer.oplog_ops(), &opk, 3000);
    assert!(!m.contains_key(&ofp), "predecessor owner retired");
    assert_eq!(
        m.get(&nfp).map(|x| x.role),
        Some(Role::Owner),
        "the new key is Owner"
    );
    assert_eq!(
        derive_encryption(&peer.oplog_ops(), &opk),
        Encryption::E2e,
        "still e2e after rotation (owner chain)"
    );
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &opk, &nfp, &new.secret_bytes())
            .map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the rotated owner decrypts with the NEW key on a peer replica"
    );
    // The `new_replica` the caller stores back agrees, anchored on the unchanged genesis owner.
    let stored = KbCollectionDoc::from_bytes(&plan.new_replica).unwrap();
    assert_eq!(
        stored.owner(),
        ofp,
        "the genesis owner (the immutable derive anchor) is unchanged in the manifest"
    );
}
/// ADR-040 PR2c — `plan_member_rotation`: a NON-owner member rotates their own identity on
/// the KBs they belong to, and ONLY those. Owned KBs (handled by the owner planner) and KBs
/// where I am not a member are skipped. The shipped Rebind aliases the successor on a peer
/// replica with NO new authority.
#[test]
fn plan_member_rotation_rebinds_member_kbs_only() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::derive_valid_members;
    use std::collections::HashMap;

    let owner = Identity::from_seed(&[1u8; 32], "owner");
    let me = Identity::from_seed(&[2u8; 32], "me"); // a non-owner member
    let new = Identity::from_seed(&[3u8; 32], "new"); // my successor
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let mfp = me.fingerprint();

    // KB-M: E2e, owned by `owner`, with `me` admitted as an Editor (oplog + roster).
    let mut m_kb = KbCollectionDoc::new_owned("M", &ofp, "owner");
    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    m_kb.author_e2e_genesis("kb-m", &ofp, &osec, &opk, self_wrap, 1000);
    let my_wrap = wrap_to_member(&k, &wrap_public_for(&me.secret_bytes())).unwrap();
    m_kb.author_member_admit(
        "kb-m",
        &mfp,
        &me.public().to_bytes(),
        &wrap_public_for(&me.secret_bytes()),
        Role::Editor,
        "me",
        my_wrap,
        &ofp,
        &osec,
        &opk,
        1001,
    );
    // KB-OWN: owned by ME — must be skipped (owner path handles it, not the member path).
    let own = KbCollectionDoc::new_owned("OWN", &mfp, "me");
    // KB-X: owned by `owner`, I am NOT a member — skipped.
    let x = KbCollectionDoc::new_owned("X", &ofp, "owner");

    let mut kb_collections = HashMap::new();
    kb_collections.insert("kb-m".to_string(), m_kb.encode_state());
    kb_collections.insert("kb-own".to_string(), own.encode_state());
    kb_collections.insert("kb-x".to_string(), x.encode_state());

    let plans = plan_member_rotation(&kb_collections, &me, &new, 2000);

    assert_eq!(
        plans.len(),
        1,
        "only the KB where I am a non-owner member is rotated"
    );
    let plan = &plans[0];
    assert_eq!(plan.kb_id, "kb-m");
    assert_eq!(
        plan.deltas.len(),
        1,
        "member rotation ships ONLY the Rebind (the owner re-wraps the key reactively)"
    );

    // A fresh peer applying the Rebind aliases the successor to my Editor seat; I retire.
    let mut peer = KbCollectionDoc::from_bytes(&kb_collections["kb-m"]).unwrap();
    peer.apply_update(&plan.deltas[0]).unwrap();
    let nfp = new.fingerprint();
    let derived = derive_valid_members(&peer.oplog_ops(), &opk, 3000);
    assert!(!derived.contains_key(&mfp), "predecessor member retired");
    assert_eq!(
        derived.get(&nfp).map(|x| x.role),
        Some(Role::Editor),
        "the successor inherits my exact role — no elevation"
    );
}
/// ADR-040 PR2c — `plan_reactive_member_rewraps`: the OWNER, on receiving a member's
/// `Rebind`, delivers the content key to that member's successor so the successor can
/// decrypt — the member can't author the re-wrap (the daemon's owner gate). A FRESH peer
/// holding only the successor's key derives the content key after applying both ops.
#[test]
fn plan_reactive_member_rewraps_delivers_key_to_a_members_successor() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::derive_content_key;

    let owner = Identity::from_seed(&[1u8; 32], "owner");
    let member = Identity::from_seed(&[2u8; 32], "member");
    let succ = Identity::from_seed(&[3u8; 32], "succ"); // the member's successor
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let mfp = member.fingerprint();

    // E2e KB owned by `owner`, with `member` admitted (key wrapped to the member).
    let mut kb = KbCollectionDoc::new_owned("M", &ofp, "owner");
    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    kb.author_e2e_genesis("kb", &ofp, &osec, &opk, self_wrap, 1000);
    let member_wrap = wrap_to_member(&k, &wrap_public_for(&member.secret_bytes())).unwrap();
    kb.author_member_admit(
        "kb",
        &mfp,
        &member.public().to_bytes(),
        &wrap_public_for(&member.secret_bytes()),
        Role::Editor,
        "member",
        member_wrap,
        &ofp,
        &osec,
        &opk,
        1001,
    );
    let base = kb.encode_state(); // the owner's replica BEFORE the member rotates

    // The member rotates: author their self-Rebind (the inbound delta the owner receives).
    let sfp = succ.fingerprint();
    let mut member_view = KbCollectionDoc::from_bytes(&base).unwrap();
    let rebind = member_view.author_rebind(
        "kb",
        &mfp,
        &sfp,
        &succ.public().to_bytes(),
        &wrap_public_for(&succ.secret_bytes()),
        &member.secret_bytes(),
        &member.public().to_bytes(),
        2000,
    );

    // The OWNER reacts: produce the re-wrap that delivers the key to the successor.
    let rewraps = plan_reactive_member_rewraps("kb", &base, &rebind, &k, &owner, 2001);
    assert_eq!(
        rewraps.len(),
        1,
        "the owner re-wraps for the one rotated member"
    );

    // A FRESH peer applies the member's Rebind + the owner's re-wrap; the SUCCESSOR's key
    // now opens the content key (it could not before — the key was wrapped to the old key).
    let mut peer = KbCollectionDoc::from_bytes(&base).unwrap();
    peer.apply_update(&rebind).unwrap();
    peer.apply_update(&rewraps[0]).unwrap();
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &opk, &sfp, &succ.secret_bytes())
            .map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the rotated member's successor decrypts with its NEW key"
    );

    // Adversarial: a NON-owner running the same planner produces nothing (owner gate).
    let stranger = Identity::from_seed(&[9u8; 32], "stranger");
    assert!(
        plan_reactive_member_rewraps("kb", &base, &rebind, &k, &stranger, 2002).is_empty(),
        "only the owner authors the reactive re-wrap"
    );

    // Adversarial: the owner's OWN rotation is not re-wrapped here (subject == owner is
    // handled by the rotation command, not the reactive member path).
    let owner_new = Identity::from_seed(&[8u8; 32], "owner2");
    let mut ov = KbCollectionDoc::from_bytes(&base).unwrap();
    let owner_rebind = ov.author_rebind(
        "kb",
        &ofp,
        &owner_new.fingerprint(),
        &owner_new.public().to_bytes(),
        &wrap_public_for(&owner_new.secret_bytes()),
        &osec,
        &opk,
        2003,
    );
    assert!(
        plan_reactive_member_rewraps("kb", &base, &owner_rebind, &k, &owner, 2004).is_empty(),
        "the owner's own rotation is not handled by the reactive MEMBER re-wrap path"
    );
}
/// ADR-040 (confidence-review #237) — the reactive member re-wrap must still fire AFTER the
/// OWNER has itself rotated. The collection's meta `owner()` field still points at the GENESIS
/// fingerprint, so the old `owner() == owner_fp` guard wrongly skipped a rotated owner reacting
/// to a member's rebind (the member's successor could never decrypt). The fix resolves owner
/// authority through the cross-signed rotation chain (`is_owner_principal`). This is the compound
/// sequence: owner→owner2, THEN member→succ, and owner2 must deliver the key to succ.
#[test]
fn plan_reactive_member_rewraps_works_after_the_owner_has_itself_rotated() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::derive_content_key;

    let owner = Identity::from_seed(&[1u8; 32], "owner");
    let owner2 = Identity::from_seed(&[7u8; 32], "owner2"); // the owner's successor
    let member = Identity::from_seed(&[2u8; 32], "member");
    let succ = Identity::from_seed(&[3u8; 32], "succ"); // the member's successor
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let mfp = member.fingerprint();

    // E2e KB owned by `owner`, `member` admitted.
    let mut kb = KbCollectionDoc::new_owned("M", &ofp, "owner");
    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    kb.author_e2e_genesis("kb", &ofp, &osec, &opk, self_wrap, 1000);
    let member_wrap = wrap_to_member(&k, &wrap_public_for(&member.secret_bytes())).unwrap();
    kb.author_member_admit(
        "kb",
        &mfp,
        &member.public().to_bytes(),
        &wrap_public_for(&member.secret_bytes()),
        Role::Editor,
        "member",
        member_wrap,
        &ofp,
        &osec,
        &opk,
        1001,
    );

    // The OWNER rotates first: owner → owner2 (old owner key signs the Rebind), then re-wraps
    // the content key to its own new key — exactly as `plan_owner_rotation` does. The meta
    // owner() field still resolves to the genesis owner fp — the condition the old guard botched.
    kb.author_rebind(
        "kb",
        &ofp,
        &owner2.fingerprint(),
        &owner2.public().to_bytes(),
        &wrap_public_for(&owner2.secret_bytes()),
        &osec,
        &opk,
        1002,
    );
    let owner2_wrap = wrap_to_member(&k, &wrap_public_for(&owner2.secret_bytes())).unwrap();
    let _ = kb.author_rebind_rewrap(
        "kb",
        &owner2.fingerprint(),
        &owner2.public().to_bytes(),
        owner2_wrap,
        &opk, // anchor = genesis owner pubkey
        &ofp, // signer = current owner (the old key authors the rewrap at rotation time)
        &osec,
        &opk,
        1003,
    );
    let base = kb.encode_state(); // owner2 is now the acting owner; meta owner() == genesis ofp

    // A member rotates AFTER the owner rotation.
    let sfp = succ.fingerprint();
    let mut member_view = KbCollectionDoc::from_bytes(&base).unwrap();
    let rebind = member_view.author_rebind(
        "kb",
        &mfp,
        &sfp,
        &succ.public().to_bytes(),
        &wrap_public_for(&succ.secret_bytes()),
        &member.secret_bytes(),
        &member.public().to_bytes(),
        2000,
    );

    // The ROTATED owner (owner2) reacts — pre-fix this returned empty (owner()!=owner2.fp).
    let rewraps = plan_reactive_member_rewraps("kb", &base, &rebind, &k, &owner2, 2001);
    assert_eq!(
        rewraps.len(),
        1,
        "a rotated owner must still re-wrap for a member who rotates after it"
    );

    // The member's successor decrypts with its NEW key, deriving under the ORIGINAL genesis
    // anchor (owner2's authored re-wrap is accepted via the owner principal chain).
    let mut peer = KbCollectionDoc::from_bytes(&base).unwrap();
    peer.apply_update(&rebind).unwrap();
    peer.apply_update(&rewraps[0]).unwrap();
    assert_eq!(
        derive_content_key(&peer.oplog_ops(), &opk, &sfp, &succ.secret_bytes())
            .map(|c| *c.as_bytes()),
        Some(*k.as_bytes()),
        "the member's successor decrypts even though the owner had itself rotated first"
    );

    // Authority negative still holds: a stranger (not in the owner chain) produces nothing.
    let stranger = Identity::from_seed(&[9u8; 32], "stranger");
    assert!(
        plan_reactive_member_rewraps("kb", &base, &rebind, &k, &stranger, 2002).is_empty(),
        "a non-owner (not in the owner principal chain) never authors a reactive re-wrap"
    );
}
