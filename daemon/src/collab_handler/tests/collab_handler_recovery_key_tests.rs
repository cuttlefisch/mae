use super::*;

// ============================================================================
// ADR-040 PR3 — the member-authored RECOVERY surface on the daemon write gate.
// `verify_member_self_service_update` now also accepts (b) a member registering its
// OWN recovery key and (c) a recovery-key-signed Rebind submitted by the successor
// (the lost-primary path). The attacker tests pin every way that authority could be
// abused: a stranger registering, cross-registering onto another seat, recovering
// with no/forged/unregistered recovery key, or a relay submitting on the successor's
// behalf. Principle #14: the negative cases are the point.
// ============================================================================

#[tokio::test]
async fn member_register_recovery_key_is_accepted_and_grants_no_access() {
    use mae_sync::membership::MembershipAction;
    let (store, bc, m, mut docs) = kb_with_member("kbrk1", 40).await;
    let rec = rotor_keys(41); // the offline recovery key R

    // M registers R, SIGNED BY M's own primary.
    let mut coll = load_coll(&store, "kbrk1").await;
    let update = coll.author_register_recovery_key("kbrk1", &m.2, &rec.1, &m.0, &m.1, 1000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbrk1", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "a member registering its OWN recovery key must be accepted: {:?}",
        r.error
    );

    let stored = load_coll(&store, "kbrk1").await;
    assert!(
        stored
            .oplog_ops()
            .iter()
            .any(|o| o.op.action == MembershipAction::RegisterRecoveryKey
                && o.op.author == m.2
                && o.op.recovery_pubkey == Some(rec.1)),
        "the RegisterRecoveryKey op is durably written to the op-log"
    );
    // Registration grants NO roster access — the recovery key is not a member; it only
    // becomes able to AUTHORIZE a future rotation, never to read on its own.
    assert_eq!(
        stored.role_of(&rec.2),
        None,
        "registering a recovery key does not add it to the member roster"
    );
}

#[tokio::test]
async fn non_member_cannot_register_a_recovery_key() {
    let (store, bc, _m, mut docs) = kb_with_member("kbrk2", 42).await;
    let s = rotor_keys(93); // a stranger, never admitted
    let rec = rotor_keys(43);
    let mut coll = load_coll(&store, "kbrk2").await;
    let update = coll.author_register_recovery_key("kbrk2", &s.2, &rec.1, &s.0, &s.1, 1000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("s"),
        Some(&s.2),
        kb_collection_op_msg("kbrk2", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a non-member cannot register a recovery key for this KB"
    );
}

#[tokio::test]
async fn member_cannot_register_a_recovery_key_for_another_seat() {
    use mae_sync::membership::MembershipAction;
    let (store, bc, m, mut docs) = kb_with_member("kbrk3", 44).await;
    let victim = rotor_keys(45); // another principal's fingerprint
    let rec = rotor_keys(46);

    // A crypto-valid op SIGNED BY M (author = M, so verify_signed passes), but whose
    // SUBJECT is someone else's seat — an attempt to plant a recovery key on `victim`
    // so M could later "recover" (hijack) it.
    let mut coll = load_coll(&store, "kbrk3").await;
    let mut op = coll.build_membership_op(
        "kbrk3",
        MembershipAction::RegisterRecoveryKey,
        &victim.2, // subject ≠ author
        None,
        false,
        &m.2, // author = M
        1000,
        None,
        0,
    );
    op.recovery_pubkey = Some(rec.1);
    let sig = op.sign(&m.0);
    coll.append_signed_op(&op, &sig, &m.1);
    let update = coll.encode_state();

    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbrk3", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a member may register a recovery key only for its OWN seat (subject must equal the principal)"
    );
}

#[tokio::test]
async fn recovery_signed_rebind_is_accepted_and_aliased() {
    use mae_sync::membership::MembershipAction;
    let (store, bc, m, mut docs) = kb_with_member("kbrk4", 47).await;
    let rec = rotor_keys(48); // recovery key R
    let m2 = rotor_keys(49); // the fresh successor M'

    // (1) While the primary is intact, M registers R.
    let mut coll = load_coll(&store, "kbrk4").await;
    let reg = coll.author_register_recovery_key("kbrk4", &m.2, &rec.1, &m.0, &m.1, 1000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbrk4", &reg),
        &mut docs,
    )
    .await;
    assert!(r.error.is_none(), "register R: {:?}", r.error);

    // (2) M lost its primary. R signs a Rebind M→M', and the SUCCESSOR M' submits it
    // (connecting with its newly-authorized key, ADR-040 §4).
    let mut coll = load_coll(&store, "kbrk4").await;
    let update =
        coll.author_recovery_rebind("kbrk4", &m.2, &m2.2, &m2.1, &m2.3, &rec.0, &rec.1, 2000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m2"),
        Some(&m2.2),
        kb_collection_op_msg("kbrk4", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_none(),
        "a recovery-signed rebind submitted by the successor must be accepted: {:?}",
        r.error
    );

    let stored = load_coll(&store, "kbrk4").await;
    assert!(
        stored
            .oplog_ops()
            .iter()
            .any(|o| o.op.action == MembershipAction::Rebind
                && o.op.author == m.2
                && o.op.subject == m2.2),
        "the recovery rebind is durably written"
    );
    assert_eq!(
        stored.role_of(&m2.2),
        Some(mae_sync::kb::Role::Editor),
        "the recovered successor inherits the predecessor's role (no elevation)"
    );
}

#[tokio::test]
async fn recovery_rebind_without_a_registered_key_is_rejected() {
    let (store, bc, m, mut docs) = kb_with_member("kbrk5", 50).await;
    let rec = rotor_keys(51);
    let m2 = rotor_keys(52);
    // No registration was ever made — there is no recovery key to authorize this.
    let mut coll = load_coll(&store, "kbrk5").await;
    let update =
        coll.author_recovery_rebind("kbrk5", &m.2, &m2.2, &m2.1, &m2.3, &rec.0, &rec.1, 2000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m2"),
        Some(&m2.2),
        kb_collection_op_msg("kbrk5", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a recovery rebind with no pre-registered recovery key must be rejected"
    );
}

#[tokio::test]
async fn recovery_rebind_with_an_unregistered_key_is_rejected() {
    let (store, bc, m, mut docs) = kb_with_member("kbrk6", 53).await;
    let rec = rotor_keys(54); // the key M actually registered
    let evil = rotor_keys(94); // attacker's key — NEVER registered
    let m2 = rotor_keys(55);

    // M registers the legitimate R.
    let mut coll = load_coll(&store, "kbrk6").await;
    let reg = coll.author_register_recovery_key("kbrk6", &m.2, &rec.1, &m.0, &m.1, 1000);
    let _ = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbrk6", &reg),
        &mut docs,
    )
    .await;

    // The attacker signs a recovery rebind with a DIFFERENT key than the registered one.
    let mut coll = load_coll(&store, "kbrk6").await;
    let update =
        coll.author_recovery_rebind("kbrk6", &m.2, &m2.2, &m2.1, &m2.3, &evil.0, &evil.1, 2000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("m2"),
        Some(&m2.2),
        kb_collection_op_msg("kbrk6", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "only the REGISTERED recovery key may sign a recovery rebind — a forged/other key is rejected"
    );
}

#[tokio::test]
async fn recovery_rebind_must_be_submitted_by_the_successor_key() {
    let (store, bc, m, mut docs) = kb_with_member("kbrk7", 56).await;
    let rec = rotor_keys(57);
    let m2 = rotor_keys(58);

    let mut coll = load_coll(&store, "kbrk7").await;
    let reg = coll.author_register_recovery_key("kbrk7", &m.2, &rec.1, &m.0, &m.1, 1000);
    let _ = dispatch_as(
        &store,
        &bc,
        Some("m"),
        Some(&m.2),
        kb_collection_op_msg("kbrk7", &reg),
        &mut docs,
    )
    .await;

    // A valid recovery rebind (R-signed) but RELAYED by an unrelated principal rather
    // than the successor M' whose key it rotates into.
    let mut coll = load_coll(&store, "kbrk7").await;
    let update =
        coll.author_recovery_rebind("kbrk7", &m.2, &m2.2, &m2.1, &m2.3, &rec.0, &rec.1, 2000);
    let r = dispatch_as(
        &store,
        &bc,
        Some("evil"),
        Some(&fp("evil")),
        kb_collection_op_msg("kbrk7", &update),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a recovery rebind must be submitted by the successor key it rotates into (subject == principal)"
    );
}
