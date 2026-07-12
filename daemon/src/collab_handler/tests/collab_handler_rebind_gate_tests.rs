use super::*;

// ============================================================================
// ADR-040 PR2c — member-authored `Rebind` write gate on `kb/collection_op`.
//
// `kb/collection_op` is otherwise owner-only (`KbOp::Manage`, ADR-018). PR2c adds
// ONE narrow exception: a non-owner member may write a self-`Rebind` (rotate their
// own identity) without owner mediation. These tests encode the ATTACKER model for
// that new write surface (principle #14): the gate must accept a clean member
// self-rotation and reject every way a member could abuse it to widen privilege.
//
// Scope note: PR2c-1 is the WRITE gate (the op is accepted + stored + broadcast so
// it converges; anchored-derive peers alias the successor via the PR2a post-pass).
// Successor ACCESS on an owned/hub daemon (the legacy `member_roles` roster) and
// content-key delivery are completed by the owner-side reactive path (PR2c-2).
// ============================================================================

#[tokio::test]
async fn member_self_rebind_is_accepted_and_stored() {
    use mae_sync::membership::MembershipAction;
    let (store, bc, m, mut docs) = kb_with_member("kbr1", 21).await;
    let m2 = rotor_keys(22);

    // M rotates into the FRESH successor key m2 (the OLD key signs the Rebind).
    let mut coll = load_coll(&store, "kbr1").await;
    let update = coll.author_rebind("kbr1", &m.2, &m2.2, &m2.1, &m2.3, &m.0, &m.1, 1000);

    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbr1", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "a clean member self-rotation must be accepted: {:?}",
        r.error
    );

    // The rebind is durably written to the collection op-log (it converges to peers).
    let stored = load_coll(&store, "kbr1").await;
    assert!(
        stored
            .oplog_ops()
            .iter()
            .any(|o| o.op.action == MembershipAction::Rebind
                && o.op.author == m.2
                && o.op.subject == m2.2),
        "the member's Rebind op is stored in the collection op-log"
    );
    // The successor is mirrored into the roster with the PREDECESSOR's role (Editor),
    // so it gains access on this roster-model daemon. The predecessor is left in place.
    assert_eq!(
        stored.role_of(&m2.2),
        Some(mae_sync::kb::Role::Editor),
        "the rotated successor inherits the predecessor's role in the roster"
    );
    assert_eq!(
        stored.role_of(&m.2),
        Some(mae_sync::kb::Role::Editor),
        "the predecessor is left in the roster (additive alias, no lockout)"
    );
}

#[tokio::test]
async fn non_member_rebind_is_rejected() {
    let (store, bc, _m, mut docs) = kb_with_member("kbr2", 23).await;
    // A stranger key S — never admitted — authors a self-rotation S → S'.
    let s = rotor_keys(90);
    let s2 = rotor_keys(91);
    let mut coll = load_coll(&store, "kbr2").await;
    let update = coll.author_rebind("kbr2", &s.2, &s2.2, &s2.1, &s2.3, &s.0, &s.1, 1000);

    let r = dispatch_as(
        &store,
        &bc,
        Some("s"),
        Some(&s.2),
        kb_collection_op_msg("kbr2", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a non-member must not be able to write a Rebind"
    );
}

#[tokio::test]
async fn member_cannot_rotate_someone_elses_identity() {
    let (store, bc, m, mut docs) = kb_with_member("kbr3", 24).await;
    let m2 = rotor_keys(25);
    // A VALID rebind authored by M (M signs it) …
    let mut coll = load_coll(&store, "kbr3").await;
    let update = coll.author_rebind("kbr3", &m.2, &m2.2, &m2.1, &m2.3, &m.0, &m.1, 1000);
    // … but submitted by a DIFFERENT principal. The op.author (M) ≠ connection principal.
    let r = dispatch_as(
        &store,
        &bc,
        Some("evil"),
        Some(&fp("evil")),
        kb_collection_op_msg("kbr3", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a member may only rotate their OWN identity (op.author must equal the principal)"
    );
}

#[tokio::test]
async fn member_cannot_smuggle_a_non_rebind_op() {
    use mae_sync::kb::Role;
    use mae_sync::membership::MembershipAction;
    let (store, bc, m, mut docs) = kb_with_member("kbr4", 26).await;

    // M signs an `Admit` self-elevation to Owner (crypto-valid, but NOT a Rebind) and
    // tries to inject it through the member path.
    let mut coll = load_coll(&store, "kbr4").await;
    let op = coll.build_membership_op(
        "kbr4",
        MembershipAction::Admit,
        &m.2,
        Some(Role::Owner),
        true,
        &m.2,
        1000,
        None,
        0,
    );
    let sig = op.sign(&m.0);
    coll.append_signed_op(&op, &sig, &m.1);
    let update = coll.encode_state();

    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbr4", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a member may author ONLY a Rebind on this path, never an Admit/SetRole"
    );
}

#[tokio::test]
async fn owner_self_rebind_is_mirrored_into_the_roster_no_lockout() {
    use mae_sync::kb::Role;
    use mae_sync::membership::MembershipAction;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let o = rotor_keys(31); // REAL-keyed owner (must sign its own Rebind)
    let o2 = rotor_keys(32); // the owner's fresh successor key

    // Owner shares an un-anchored (roster-model / hub) KB — no set_kb_anchor.
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&o.2),
        "kbo1",
        "owner",
        &mut docs,
    )
    .await;
    assert_eq!(
        load_coll(&store, "kbo1").await.role_of(&o.2),
        Some(Role::Owner),
        "sanity: the owner holds Owner in the roster on an un-anchored KB"
    );

    // The owner rotates into o2 (the OLD owner key signs the Rebind).
    let mut coll = load_coll(&store, "kbo1").await;
    let update = coll.author_rebind("kbo1", &o.2, &o2.2, &o2.1, &o2.3, &o.0, &o.1, 1000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&o.2),
        kb_collection_op_msg("kbo1", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "the owner's own self-rotation must be accepted: {:?}",
        r.error
    );

    let stored = load_coll(&store, "kbo1").await;
    assert_eq!(
        stored.role_of(&o2.2),
        Some(Role::Owner),
        "the rotated owner successor MUST inherit Owner in the roster — no self-lockout (#265)"
    );
    assert!(
        stored
            .oplog_ops()
            .iter()
            .any(|op| op.op.action == MembershipAction::Rebind
                && op.op.author == o.2
                && op.op.subject == o2.2),
        "the owner's Rebind is durably in the op-log"
    );
}

#[tokio::test]
async fn owner_path_does_not_mirror_a_rebind_authored_by_another_principal() {
    let (store, _bc, m, _docs) = kb_with_member("kbo2", 41).await;
    let m2 = rotor_keys(42);
    // The MEMBER authors their own valid self-rotation (author = m).
    let mut coll = load_coll(&store, "kbo2").await;
    let update = coll.author_rebind("kbo2", &m.2, &m2.2, &m2.1, &m2.3, &m.0, &m.1, 1000);

    // The owner-path extractor, invoked with the OWNER as principal, must NOT surface the
    // member's rebind (author m ≠ principal) — so the owner can't mirror someone else's key.
    let pairs = owner_self_rebind_pairs(&store, "kbo2", &fp("owner"), &update).await;
    assert!(
        pairs.is_empty(),
        "the owner path must not mirror a Rebind authored by another principal, got {pairs:?}"
    );
    // But the member's OWN principal does surface it (sanity that the filter is author-based).
    let self_pairs = owner_self_rebind_pairs(&store, "kbo2", &m.2, &update).await;
    assert_eq!(
        self_pairs,
        vec![(m2.2.clone(), m.2.clone())],
        "author-matched self-rebind is surfaced"
    );
}

#[tokio::test]
async fn forged_rebind_signature_is_rejected() {
    use mae_sync::membership::MembershipAction;
    let (store, bc, m, mut docs) = kb_with_member("kbr5", 27).await;
    let m2 = rotor_keys(28);
    let evil = rotor_keys(92);

    // A Rebind op for M's seat, but SIGNED BY A DIFFERENT key (forgery): the record
    // claims author_pubkey = M's pubkey while the signature came from `evil`.
    let mut coll = load_coll(&store, "kbr5").await;
    let mut op = coll.build_membership_op(
        "kbr5",
        MembershipAction::Rebind,
        &m2.2,
        None,
        false,
        &m.2,
        1000,
        None,
        0,
    );
    op.new_pubkey = Some(m2.1);
    op.new_wrap_pubkey = Some(m2.3);
    let forged = op.sign(&evil.0); // wrong key
    coll.append_signed_op(&op, &forged, &m.1);
    let update = coll.encode_state();

    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbr5", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a Rebind whose signature does not verify against its author key is rejected"
    );
}

#[tokio::test]
async fn member_cannot_smuggle_a_roster_promotion_with_a_rebind() {
    use mae_sync::kb::Role;
    let (store, bc, m, mut docs) = kb_with_member("kbr6", 29).await;
    let m2 = rotor_keys(30);

    // The privilege-escalation attempt: a VALID self-Rebind PLUS a roster self-promotion
    // to Owner carried in the SAME collection update.
    let mut coll = load_coll(&store, "kbr6").await;
    coll.upsert_member(&m.2, "m", Role::Owner); // smuggled roster mutation
    coll.author_rebind("kbr6", &m.2, &m2.2, &m2.1, &m2.3, &m.0, &m.1, 1000);
    let update = coll.encode_state(); // full state carries BOTH changes

    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbr6", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a member rotation that ALSO mutates the roster/owner must be rejected wholesale"
    );

    // And the roster is unchanged — M is still an Editor, not the smuggled Owner.
    let stored = load_coll(&store, "kbr6").await;
    let m_role = stored
        .member_roles()
        .into_iter()
        .find(|mm| mm.fingerprint == m.2)
        .map(|mm| mm.role);
    assert_eq!(
        m_role,
        Some(Role::Editor),
        "the rejected op left no trace — M did not gain Owner"
    );
}

#[tokio::test]
async fn member_cannot_rebind_onto_an_existing_member() {
    let (store, bc, m, mut docs) = kb_with_member("kbr7", 31).await;
    // Admit a SECOND real-keyed member m2 (so its fingerprint is already a member).
    let m2 = rotor_keys(32);
    let owner = fp("owner");
    let r = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        kb_member_msg("kb/add_member", "kbr7", &m2.2, Some("editor")),
        &mut docs,
    )
    .await;
    assert!(r.error.is_none(), "owner admits the second member");

    // M tries to rotate ONTO m2's existing seat (clobber/downgrade attempt). The
    // successor is fingerprint-bound to m2's key but m2 is ALREADY a member.
    let mut coll = load_coll(&store, "kbr7").await;
    let update = coll.author_rebind("kbr7", &m.2, &m2.2, &m2.1, &m2.3, &m.0, &m.1, 1000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbr7", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "you rotate INTO a fresh key, never ONTO an existing member's seat"
    );
}

#[tokio::test]
async fn owner_collection_op_still_works_after_pr2c() {
    // Regression: the member-Rebind exception must not break the normal owner path —
    // the owner still manages members through the Manage gate.
    let (store, bc, _m, mut docs) = kb_with_member("kbr8", 33).await;
    let owner = fp("owner");
    let r = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        kb_member_msg("kb/add_member", "kbr8", &fp("carol"), Some("viewer")),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "owner still manages the KB after the PR2c exception: {:?}",
        r.error
    );
}
