//! Split from the monolithic `collab_bridge_tests.rs`: e2e content-key derivation and encrypted node send/receive round-trips.

use super::*;

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}
/// Hand-build an encoded KB collection op-log: owner self-admit (genesis) + an admit per
/// member, with `key` wrapped to each member when `encrypted`. Mirrors what the Phase 3
/// daemon lifecycle will emit; here it's the fixture the network task derives from.
fn build_e2e_collection(
    kb_id: &str,
    owner: &mae_mcp::identity::Identity,
    members: &[&mae_mcp::identity::Identity],
    key: &mae_sync::content_crypto::ContentKey,
    encrypted: bool,
) -> Vec<u8> {
    use mae_sync::kb::Role;
    use mae_sync::membership::MembershipAction;
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned(kb_id, &owner.fingerprint(), "owner");
    if encrypted {
        coll.set_encryption(mae_sync::kb::Encryption::E2e);
    }
    // Genesis: owner self-admit (first op ⇒ prev_hash empty) — the trust anchor.
    let g = coll.build_membership_op(
        kb_id,
        MembershipAction::Admit,
        &owner.fingerprint(),
        Some(Role::Owner),
        true,
        &owner.fingerprint(),
        0,
        None,
        0,
    );
    let gsig = g.sign(&owner.secret_bytes());
    coll.append_signed_op(&g, &gsig, &owner.public().to_bytes());
    // Signed SetEncryption(e2e) — the authoritative, anti-downgrade mode source (F2). The
    // unsigned flag alone is no longer enough for `derive_kb_content_key`.
    if encrypted {
        let se = coll.build_membership_op(
            kb_id,
            MembershipAction::SetEncryption,
            "e2e",
            None,
            false,
            &owner.fingerprint(),
            0,
            None,
            0,
        );
        let sesig = se.sign(&owner.secret_bytes());
        coll.append_signed_op(&se, &sesig, &owner.public().to_bytes());
    }
    // Owner admits each member, wrapping the content key to them (encrypted only).
    for (i, m) in members.iter().enumerate() {
        let mut a = coll.build_membership_op(
            kb_id,
            MembershipAction::Admit,
            &m.fingerprint(),
            Some(Role::Editor),
            false,
            &owner.fingerprint(),
            (i + 1) as u64,
            None,
            0,
        );
        if encrypted {
            // ADR-041 (#158 I1): wrap to the member's PUBLISHED X25519 wrap key.
            a.wrapped_key = Some(
                mae_sync::content_crypto::wrap_to_member(
                    key,
                    &mae_sync::content_crypto::wrap_public_for(&m.secret_bytes()),
                )
                .unwrap(),
            );
        }
        let asig = a.sign(&owner.secret_bytes());
        coll.append_signed_op(&a, &asig, &owner.public().to_bytes());
    }
    coll.encode_state()
}
/// Derive recovers the per-KB content key for EVERY member (two distinct identities, not
/// a single-identity tautology), excludes a non-member, and returns nothing for an
/// unencrypted KB (the `Encryption::E2e` gate).
#[test]
fn derive_kb_content_key_recovers_for_members_excludes_others() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::ContentKey;

    let owner = Identity::generate("owner");
    let m1 = Identity::generate("m1");
    let m2 = Identity::generate("m2");
    let stranger = Identity::generate("stranger");
    let key = ContentKey::generate();

    let coll = build_e2e_collection("kb1", &owner, &[&m1, &m2], &key, true);
    assert_eq!(
        derive_kb_content_key(&coll, &m1)
            .expect("m1 recovers")
            .as_bytes(),
        key.as_bytes(),
        "member m1 derives the exact content key",
    );
    assert_eq!(
        derive_kb_content_key(&coll, &m2)
            .expect("m2 recovers")
            .as_bytes(),
        key.as_bytes(),
        "a SECOND distinct member also derives the exact key",
    );
    assert!(
        derive_kb_content_key(&coll, &stranger).is_none(),
        "a non-member's secret opens no wrap ⇒ no key",
    );
    let plain = build_e2e_collection("kb1", &owner, &[&m1], &key, false);
    assert!(
        derive_kb_content_key(&plain, &m1).is_none(),
        "unencrypted KB ⇒ no content key, even for an admitted member (the E2e gate)",
    );
}
/// ADR-039 F1+F2: `derive_kb_content_key` refuses (a) a KB whose E2e is asserted ONLY by
/// the unsigned collection flag (no signed `SetEncryption` op — a relay could set the flag)
/// and (b) a genesis NOT authored by the daemon-attested owner (`COLL_OWNER_KEY`) — a relay
/// substituting a forged genesis to inject an attacker-known key.
#[test]
fn derive_kb_content_key_requires_signed_mode_and_owner_anchor() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_to_member, ContentKey};
    use mae_sync::kb::{Encryption, KbCollectionDoc, Role};
    use mae_sync::membership::MembershipAction;

    let owner = Identity::generate("owner");
    let key = ContentKey::generate();

    // (F2) Unsigned flag ONLY: genesis + self-wrap, but NO signed SetEncryption op.
    let mut flag_only = KbCollectionDoc::new_owned("kb", &owner.fingerprint(), "owner");
    flag_only.set_encryption(Encryption::E2e); // unsigned flag a relay could also set
    let mut g = flag_only.build_membership_op(
        "kb",
        MembershipAction::Admit,
        &owner.fingerprint(),
        Some(Role::Owner),
        true,
        &owner.fingerprint(),
        0,
        None,
        0,
    );
    g.wrapped_key = Some(wrap_to_member(&key, &owner.public().to_bytes()).unwrap());
    let gsig = g.sign(&owner.secret_bytes());
    flag_only.append_signed_op(&g, &gsig, &owner.public().to_bytes());
    assert!(
        derive_kb_content_key(&flag_only.encode_state(), &owner).is_none(),
        "the unsigned flag alone must NOT enable derivation — the signed op-log is authoritative (F2)"
    );

    // (F1) Substituted genesis: COLL_OWNER_KEY is the legit owner, but the genesis (+ the
    // signed SetEncryption + the wrap to the victim) is authored by an ATTACKER's key.
    let attacker = Identity::generate("attacker");
    let mut forged = KbCollectionDoc::new_owned("kb", &owner.fingerprint(), "owner");
    let mut ag = forged.build_membership_op(
        "kb",
        MembershipAction::Admit,
        &attacker.fingerprint(),
        Some(Role::Owner),
        true,
        &attacker.fingerprint(),
        0,
        None,
        0,
    );
    ag.wrapped_key = Some(wrap_to_member(&key, &owner.public().to_bytes()).unwrap());
    let agsig = ag.sign(&attacker.secret_bytes());
    forged.append_signed_op(&ag, &agsig, &attacker.public().to_bytes());
    let ase = forged.build_membership_op(
        "kb",
        MembershipAction::SetEncryption,
        "e2e",
        None,
        false,
        &attacker.fingerprint(),
        0,
        None,
        0,
    );
    let asesig = ase.sign(&attacker.secret_bytes());
    forged.append_signed_op(&ase, &asesig, &attacker.public().to_bytes());
    assert!(
        derive_kb_content_key(&forged.encode_state(), &owner).is_none(),
        "a genesis NOT authored by the attested owner is refused, even with a wrap to the victim (F1)"
    );
}
/// Full seam round-trip: the SEND side seals two CAUSALLY DEPENDENT edits into the op-set
/// (no plaintext on the wire), and the RECEIVE seam opens + emits them so they materialize
/// to the correct final text — exercising the causal-order apply the op-set guarantees.
#[test]
fn encrypted_node_seals_on_send_and_materializes_on_receive() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::ContentKey;
    use mae_sync::kb::KbNodeDoc;
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    let sender = Arc::new(Identity::generate("sender"));
    let key = ContentKey::generate();

    // Two successive title edits are causally dependent — the exact case yrs does NOT
    // converge if applied out of order (the bug class the op-set layer caught).
    let mut node = KbNodeDoc::new_with_client_id("n1", "", "", &[], 7);
    let init = node.encode_state();
    let e1 = node.set_title("Secret");
    let e2 = node.set_title("SecretFinal");

    let mut op_set: Vec<u8> = Vec::new();
    let mut payloads: Vec<Vec<u8>> = Vec::new();
    for upd in [&init, &e1, &e2] {
        let (req, new_state, op_id) = build_kb_node_update_request(
            1,
            "kb1",
            "n1",
            upd,
            0,
            Some(&sender),
            Some(&key),
            true,
            &op_set,
        )
        .expect("e2e seal ⇒ Some");
        assert!(op_id.is_some(), "each push seals an op");
        op_set = new_state;
        payloads.push(
            mae_sync::encoding::base64_to_update(req["params"]["update"].as_str().unwrap())
                .unwrap(),
        );
    }
    // Nothing on the wire leaks the plaintext title.
    for p in &payloads {
        assert!(
            !contains_subslice(p, b"SecretFinal") && !contains_subslice(p, b"Secret"),
            "a sealed wire payload must not contain plaintext",
        );
    }

    // RECEIVER holds the key for kb1 and knows n1 → kb1.
    let (tx, mut rx) = mpsc::channel(100);
    let mut content_keys = HashMap::new();
    content_keys.insert("kb1".to_string(), key.clone());
    let mut node_to_kb = HashMap::new();
    node_to_kb.insert("n1".to_string(), "kb1".to_string());
    let mut op_sets: HashMap<String, Vec<u8>> = HashMap::new();
    let mut seen: HashMap<String, BTreeSet<String>> = HashMap::new();
    for p in &payloads {
        route_kb_node_update(
            "n1",
            p.clone(),
            &content_keys,
            &node_to_kb,
            &mut op_sets,
            &mut seen,
            &tx,
        );
    }
    drop(tx);

    let mut updates = Vec::new();
    while let Ok(evt) = rx.try_recv() {
        if let CollabEvent::KbNodeUpdate {
            update_bytes,
            kb_id,
            node_id,
        } = evt
        {
            assert_eq!(kb_id, "kb1", "decrypted update is attributed to its KB");
            assert_eq!(node_id, "n1");
            updates.push(update_bytes);
        }
    }
    assert_eq!(updates.len(), 3, "all three sealed ops open as plaintext");
    // Materialize in the emitted (causal) order → the correct final title.
    let mut materialized = KbNodeDoc::from_bytes(&updates[0]).expect("first op is the node state");
    materialized.apply_update(&updates[1]).unwrap();
    materialized.apply_update(&updates[2]).unwrap();
    assert_eq!(
        materialized.title(),
        "SecretFinal",
        "causal-ordered apply of the decrypted ops yields the final edit",
    );

    // A FRESH JOINER receives the ENTIRE accumulated op-set in ONE update (the join
    // snapshot) — so the seam must open all three ops in causal order from a single
    // call, not rely on them arriving pre-ordered. This is the real ordering stress.
    let (tx2, mut rx2) = mpsc::channel(100);
    let mut op_sets2: HashMap<String, Vec<u8>> = HashMap::new();
    let mut seen2: HashMap<String, BTreeSet<String>> = HashMap::new();
    route_kb_node_update(
        "n1",
        op_set.clone(),
        &content_keys,
        &node_to_kb,
        &mut op_sets2,
        &mut seen2,
        &tx2,
    );
    drop(tx2);
    let mut joiner = Vec::new();
    while let Ok(evt) = rx2.try_recv() {
        if let CollabEvent::KbNodeUpdate { update_bytes, .. } = evt {
            joiner.push(update_bytes);
        }
    }
    assert_eq!(
        joiner.len(),
        3,
        "a fresh joiner opens the whole op-set at once"
    );
    let mut joined = KbNodeDoc::from_bytes(&joiner[0]).expect("first opened op is the node state");
    joined.apply_update(&joiner[1]).unwrap();
    joined.apply_update(&joiner[2]).unwrap();
    assert_eq!(
        joined.title(),
        "SecretFinal",
        "the single-shot op-set opens in causal order (node-create before each edit)",
    );
}
/// Without the key the receive seam passes ciphertext THROUGH verbatim (never plaintext);
/// with the key, replaying the same payload is idempotent (`seen_ops` emits each op once).
#[test]
fn receive_passthrough_without_key_and_idempotent_with_key() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::ContentKey;
    use mae_sync::kb::KbNodeDoc;
    use std::collections::{BTreeSet, HashMap};
    use std::sync::Arc;

    let sender = Arc::new(Identity::generate("sender"));
    let key = ContentKey::generate();
    let mut node = KbNodeDoc::new_with_client_id("n1", "", "", &[], 7);
    let edit = node.set_title("Secret");
    let (req, _state, _id) = build_kb_node_update_request(
        1,
        "kb1",
        "n1",
        &edit,
        0,
        Some(&sender),
        Some(&key),
        true,
        &[],
    )
    .expect("e2e seal ⇒ Some");
    let payload =
        mae_sync::encoding::base64_to_update(req["params"]["update"].as_str().unwrap()).unwrap();

    // (a) NON-MEMBER receiver: knows the node→kb mapping but holds no key ⇒ passthrough
    // of the exact ciphertext bytes (this is the relay/non-member's view — opaque).
    let (tx_a, mut rx_a) = mpsc::channel(100);
    let empty_keys: HashMap<String, ContentKey> = HashMap::new();
    let mut node_to_kb = HashMap::new();
    node_to_kb.insert("n1".to_string(), "kb1".to_string());
    let mut op_sets_a: HashMap<String, Vec<u8>> = HashMap::new();
    let mut seen_a: HashMap<String, BTreeSet<String>> = HashMap::new();
    route_kb_node_update(
        "n1",
        payload.clone(),
        &empty_keys,
        &node_to_kb,
        &mut op_sets_a,
        &mut seen_a,
        &tx_a,
    );
    drop(tx_a);
    let mut a_events = Vec::new();
    while let Ok(evt) = rx_a.try_recv() {
        if let CollabEvent::KbNodeUpdate { update_bytes, .. } = evt {
            a_events.push(update_bytes);
        }
    }
    assert_eq!(a_events.len(), 1, "passthrough emits the update once");
    assert_eq!(
        a_events[0], payload,
        "no key ⇒ the ciphertext passes through verbatim"
    );
    assert!(
        !contains_subslice(&a_events[0], b"Secret"),
        "the relay's view is ciphertext, never plaintext",
    );

    // (b) MEMBER receiver: opens once; replaying the SAME payload emits NOTHING the
    // second time (seen_ops dedupe — guards against re-applying the daemon's echoes).
    let (tx_b, mut rx_b) = mpsc::channel(100);
    let mut keys_b = HashMap::new();
    keys_b.insert("kb1".to_string(), key.clone());
    let mut op_sets_b: HashMap<String, Vec<u8>> = HashMap::new();
    let mut seen_b: HashMap<String, BTreeSet<String>> = HashMap::new();
    route_kb_node_update(
        "n1",
        payload.clone(),
        &keys_b,
        &node_to_kb,
        &mut op_sets_b,
        &mut seen_b,
        &tx_b,
    );
    route_kb_node_update(
        "n1",
        payload.clone(),
        &keys_b,
        &node_to_kb,
        &mut op_sets_b,
        &mut seen_b,
        &tx_b,
    );
    drop(tx_b);
    let mut b_count = 0;
    while let Ok(evt) = rx_b.try_recv() {
        if matches!(evt, CollabEvent::KbNodeUpdate { .. }) {
            b_count += 1;
        }
    }
    assert_eq!(
        b_count, 1,
        "a duplicated op-set update materializes its op exactly once"
    );
}
