//! Split from the monolithic `collab_bridge_tests.rs`: recovery-key registration and recovery-key-honored rotation.

use super::*;

/// ADR-040 §Recovery-key — `plan_register_recovery_key` authors a `RegisterRecoveryKey` on
/// every KB I am a member of (owner OR member role) and NOTHING where I hold no role. After
/// applying the delta, a fresh peer's recovery registry resolves my registered recovery key —
/// the precondition for a later recovery rotation.
#[test]
fn plan_register_recovery_key_targets_member_kbs_and_publishes_the_key() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::recovery_registry;
    use std::collections::HashMap;

    let owner = Identity::from_seed(&[1u8; 32], "owner");
    let me = Identity::from_seed(&[2u8; 32], "me");
    let recovery = Identity::from_seed(&[7u8; 32], "recovery");
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let mfp = me.fingerprint();

    // KB-M: owner-owned, I'm an Editor member.
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
    // KB-OWN: I own it (role_of(me) == Owner) — register here too.
    let own = KbCollectionDoc::new_owned("OWN", &mfp, "me");
    // KB-X: owner-owned, I am NOT a member — skipped.
    let x = KbCollectionDoc::new_owned("X", &ofp, "owner");

    let mut kb_collections = HashMap::new();
    kb_collections.insert("kb-m".to_string(), m_kb.encode_state());
    kb_collections.insert("kb-own".to_string(), own.encode_state());
    kb_collections.insert("kb-x".to_string(), x.encode_state());

    let rec_pubkey = recovery.public().to_bytes();
    let plans = plan_register_recovery_key(&kb_collections, &me, &rec_pubkey, 2000);

    assert_eq!(
        plans.len(),
        2,
        "register on the KB I'm a member of AND the KB I own — never the KB I'm not in"
    );
    assert!(
        plans
            .iter()
            .all(|p| p.kb_id == "kb-m" || p.kb_id == "kb-own"),
        "only my KBs are targeted, never kb-x"
    );

    // A fresh peer applying the KB-M registration resolves my recovery key in the registry.
    let m_plan = plans.iter().find(|p| p.kb_id == "kb-m").unwrap();
    let mut peer = KbCollectionDoc::from_bytes(&kb_collections["kb-m"]).unwrap();
    peer.apply_update(&m_plan.deltas[0]).unwrap();
    let registry = recovery_registry(&peer.oplog_ops());
    assert_eq!(
        registry.get(&mfp),
        Some(&rec_pubkey),
        "my registered recovery key is published for a future recovery rotation"
    );
}
/// ADR-040 §Recovery-key — the full round-trip: after I register a recovery key and LOSE my
/// primary, `plan_recovery_rotation` (signed by the offline recovery key) rotates me to a fresh
/// successor, and a peer that NEVER saw my old key honors it via the recovery registry —
/// aliasing the successor to my exact role and retiring the lost key. The adversarial twin:
/// the same rebind WITHOUT a registration (or signed by a different key) is rejected by derive.
#[test]
fn plan_recovery_rotation_is_honored_by_derive_only_with_a_registered_key() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::derive_valid_members;
    use std::collections::HashMap;

    let owner = Identity::from_seed(&[1u8; 32], "owner");
    let me = Identity::from_seed(&[2u8; 32], "me"); // the lost primary
    let new = Identity::from_seed(&[3u8; 32], "new"); // the recovered successor
    let recovery = Identity::from_seed(&[7u8; 32], "recovery"); // pre-registered offline key
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let mfp = me.fingerprint();
    let nfp = new.fingerprint();

    // Owner-owned E2e KB with `me` admitted as Editor.
    let mut kb = KbCollectionDoc::new_owned("M", &ofp, "owner");
    let k = ContentKey::generate();
    let self_wrap = wrap_to_member(&k, &wrap_public_for(&osec)).unwrap();
    kb.author_e2e_genesis("kb", &ofp, &osec, &opk, self_wrap, 1000);
    let my_wrap = wrap_to_member(&k, &wrap_public_for(&me.secret_bytes())).unwrap();
    kb.author_member_admit(
        "kb",
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

    // (1) While my primary is intact, I register my recovery key.
    let rec_pubkey = recovery.public().to_bytes();
    let mut kc = HashMap::new();
    kc.insert("kb".to_string(), kb.encode_state());
    let reg_plans = plan_register_recovery_key(&kc, &me, &rec_pubkey, 1500);
    assert_eq!(reg_plans.len(), 1);
    kc.insert("kb".to_string(), reg_plans[0].new_replica.clone());

    // (2) I lose my primary. Run AS the new key: the recovery key signs old→new.
    let rec_plans = plan_recovery_rotation(&kc, &mfp, &new, &recovery, 2000);
    assert_eq!(rec_plans.len(), 1, "recover the one KB I belonged to");

    // A peer that never saw my old key: applies registration + recovery rebind, and derive
    // HONORS the recovery (registry binds the signer) — successor inherits Editor, old retired.
    let mut peer = KbCollectionDoc::from_bytes(&reg_plans[0].new_replica).unwrap();
    peer.apply_update(&rec_plans[0].deltas[0]).unwrap();
    let derived = derive_valid_members(&peer.oplog_ops(), &opk, 3000);
    assert_eq!(
        derived.get(&nfp).map(|x| x.role),
        Some(Role::Editor),
        "the recovered successor inherits my exact role — no elevation"
    );
    assert!(
        !derived.contains_key(&mfp),
        "the lost primary is retired after recovery"
    );

    // Adversarial twin: the SAME recovery rebind applied WITHOUT the registration present is
    // NOT honored — the successor never becomes a member (no registered key authorizes it).
    let mut bare = KbCollectionDoc::from_bytes(&kb.encode_state()).unwrap(); // no registration
    bare.apply_update(&rec_plans[0].deltas[0]).unwrap();
    let bare_derived = derive_valid_members(&bare.oplog_ops(), &opk, 3000);
    assert!(
        !bare_derived.contains_key(&nfp),
        "a recovery rebind with no registered recovery key is rejected by derive"
    );
    assert_eq!(
        bare_derived.get(&mfp).map(|x| x.role),
        Some(Role::Editor),
        "and the original member is untouched"
    );
}
