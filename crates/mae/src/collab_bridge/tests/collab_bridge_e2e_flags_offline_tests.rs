//! Split from the monolithic `collab_bridge_tests.rs`: e2e-mode flag reads, CRDT-bytes apply, content-key refresh, Phase 8 offline KB-update queue.

use super::*;

/// ADR-037 #168 — the editor-side E2e gate reads the AUTHORITATIVE SIGNED mode. A plain
/// KB is not e2e; after a signed enable it is; and a relay flipping the unsigned flag
/// can't downgrade the verdict (we read `derive_encryption`, not `coll.encryption()`).
#[test]
fn kb_collection_is_e2e_reads_signed_mode_not_the_flippable_flag() {
    use mae_mcp::identity::Identity;
    let owner = Identity::generate("owner");
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned("KB", &ofp, "owner");
    assert!(
        !kb_collection_is_e2e(&coll.encode_state()),
        "a plain (un-genesis'd) KB is not e2e"
    );
    let k = mae_sync::content_crypto::ContentKey::generate();
    let wrap = mae_sync::content_crypto::wrap_to_member(&k, &opk).unwrap();
    coll.author_e2e_genesis("KB", &ofp, &osec, &opk, wrap, 1000);
    assert!(
        kb_collection_is_e2e(&coll.encode_state()),
        "after a SIGNED enable the KB is e2e (read from the op-log)"
    );
    coll.set_encryption(mae_sync::kb::Encryption::None);
    assert!(
        kb_collection_is_e2e(&coll.encode_state()),
        "the unsigned flag is ignored — the SIGNED monotonic mode still says e2e"
    );
}
/// ADR-037 #170 — the share/re-share path must NEVER ship a plaintext node snapshot to
/// the key-blind daemon on an E2e KB (the attacker's test). It sends the already-sealed
/// op-set or SKIPS the node; a plaintext canary must appear in NO wire payload. The
/// selective control: the UNENCRYPTED path still ships every plaintext node (legacy).
#[test]
fn select_share_node_states_never_ships_plaintext_on_e2e() {
    use std::collections::HashMap;
    let canary = b"PLAINTEXT-SECRET-canary-170".to_vec();
    let node_states = vec![
        ("n-sealed".to_string(), canary.clone()),
        ("n-bare".to_string(), canary.clone()),
    ];
    let sealed = b"SEALED-OPSET-BYTES".to_vec();
    let mut op_sets: HashMap<String, Vec<u8>> = HashMap::new();
    op_sets.insert("n-sealed".to_string(), sealed.clone());

    let out = select_share_node_states("kb", true, &node_states, &op_sets);
    assert_eq!(out.len(), 1, "the no-op-set node is skipped, not leaked");
    assert_eq!(out[0].0, "n-sealed");
    assert_eq!(
        mae_sync::encoding::base64_to_update(&out[0].1).unwrap(),
        sealed,
        "ships the SEALED op-set, never the plaintext"
    );
    for (_, b64) in &out {
        let bytes = mae_sync::encoding::base64_to_update(b64).unwrap();
        assert!(
            !bytes.windows(canary.len()).any(|w| w == canary.as_slice()),
            "the plaintext canary must appear in NO wire payload"
        );
    }

    let out2 = select_share_node_states("kb", false, &node_states, &op_sets);
    assert_eq!(out2.len(), 2, "unencrypted ships every node");
    assert_eq!(
        mae_sync::encoding::base64_to_update(&out2[0].1).unwrap(),
        canary,
        "unencrypted ships the plaintext (legacy)"
    );
}
/// #173 — the rotation-delivery oracle (the attacker's test). After the owner rotates a
/// member out, a REMAINING member receiving the rotation's `kbc:` collection delta MUST
/// re-derive the NEW key k'; the REMOVED member stays stranded on the OLD key k.
#[test]
fn refresh_kb_content_key_re_derives_on_rotation_remaining_yes_removed_no() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::{wrap_public_for, wrap_to_member, ContentKey};
    use mae_sync::kb::{KbCollectionDoc, Role};
    use std::collections::HashMap;
    use std::sync::Arc;

    let owner = Identity::generate("owner");
    let b = Arc::new(Identity::generate("member-b"));
    let c = Arc::new(Identity::generate("member-c"));
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let (bfp, bpk, bsec) = (b.fingerprint(), b.public().to_bytes(), b.secret_bytes());
    let (cfp, cpk, csec) = (c.fingerprint(), c.public().to_bytes(), c.secret_bytes());

    let k = ContentKey::generate();
    let mut coll = KbCollectionDoc::new_owned("KB", &ofp, "owner");
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
    let pre_rotation = coll.encode_state();

    let k2 = ContentKey::generate();
    assert_ne!(k.as_bytes(), k2.as_bytes());
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

    let run = |id: &Arc<Identity>| -> Option<[u8; 32]> {
        let mut content_keys: HashMap<String, ContentKey> = HashMap::new();
        content_keys.insert("KB".to_string(), ContentKey::from_bytes(*k.as_bytes()));
        let mut collections: HashMap<String, Vec<u8>> = HashMap::new();
        collections.insert("KB".to_string(), pre_rotation.clone());
        let (mut os, mut n2k, mut so) = (HashMap::new(), HashMap::new(), HashMap::new());
        let mut pco: Vec<(String, Vec<u8>)> = Vec::new();
        {
            let mut ctx = KbCryptoCtx {
                content_keys: &mut content_keys,
                op_sets: &mut os,
                node_to_kb: &mut n2k,
                seen_ops: &mut so,
                kb_collections: &mut collections,
                signing_identity: Some(id),
                pending_collection_ops: &mut pco,
            };
            refresh_kb_content_key_on_collection_delta(&mut ctx, "kbc:KB", &delta);
        }
        content_keys.get("KB").map(|k| *k.as_bytes())
    };

    assert_eq!(
        run(&c),
        Some(*k2.as_bytes()),
        "remaining member C re-derives the rotated key k' on the collection delta"
    );
    assert_eq!(
        run(&b),
        Some(*k.as_bytes()),
        "removed member B is stranded on the OLD k (cannot derive k')"
    );
}
#[test]
fn collab_kb_update_crdt_bytes_apply_to_fresh_doc() {
    // Verify that the CRDT update bytes generated by upsert_with_crdt
    // can actually be applied to reconstruct the node content.
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "crdt-test".to_string(),
        "Original".to_string(),
        mae_kb::NodeKind::Note,
        "original body with café and 日本語".to_string(),
    ));
    // ADR-019: durable primary-share marker gates the broadcast.
    editor.kb.registry.primary_shared = true;
    editor.kb.registry.primary_collab_id = Some("test-kb".to_string());
    editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();

    editor
        .kb_update_node(
            "crdt-test",
            Some("Updated Title"),
            Some("new body — naïve résumé"),
            None,
        )
        .unwrap();

    let (_, _, update_bytes) = &editor.collab.pending_kb_updates[0];

    // Apply the update bytes to a fresh KbNodeDoc.
    let doc = mae_sync::kb::KbNodeDoc::from_bytes(update_bytes)
        .expect("CRDT bytes should decode to valid KbNodeDoc");
    let mat = doc.materialize();
    assert_eq!(
        mat.title, "Updated Title",
        "title should match after CRDT round-trip"
    );
    assert_eq!(
        mat.body, "new body — naïve résumé",
        "body should preserve UTF-8 after CRDT round-trip"
    );
}

// --- Phase 8: Offline KB sync tests ---
#[test]
fn offline_kb_updates_accumulate_when_disconnected() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Disconnected;
    editor.collab.pending_kb_updates.push((
        "kb-1".to_string(),
        "node-a".to_string(),
        vec![1, 2, 3],
    ));

    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    // Updates should NOT be sent when disconnected.
    assert!(
        rx.try_recv().is_err(),
        "pending KB updates should not be drained while disconnected"
    );
    // They should remain in the queue.
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "pending KB updates should be preserved while offline"
    );
}
#[test]
fn offline_kb_updates_drain_on_reconnect() {
    let mut editor = Editor::new();
    // Start disconnected, accumulate updates.
    editor.collab.status = CollabStatus::Disconnected;
    editor
        .collab
        .pending_kb_updates
        .push(("kb-1".to_string(), "node-a".to_string(), vec![10, 20]));

    let (tx, mut rx) = mpsc::channel(8);

    // First drain while disconnected — nothing sent.
    drain_collab_intents(&mut editor, &tx);
    assert!(rx.try_recv().is_err());
    assert_eq!(editor.collab.pending_kb_updates.len(), 1);

    // Simulate reconnect.
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    drain_collab_intents(&mut editor, &tx);

    // Now the update should be sent.
    let cmd = rx
        .try_recv()
        .expect("KB update should be sent after reconnect");
    match cmd {
        CollabCommand::KbNodeUpdate { kb_id, node_id, .. } => {
            assert_eq!(kb_id, "kb-1");
            assert_eq!(node_id, "node-a");
        }
        other => panic!(
            "expected KbNodeUpdate, got: {:?}",
            collab_command_name(&other)
        ),
    }
    assert!(editor.collab.pending_kb_updates.is_empty());
}
#[test]
fn offline_kb_multiple_edits_all_sent_on_reconnect() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Disconnected;

    // Accumulate 3 offline edits.
    for i in 0..3 {
        editor.collab.pending_kb_updates.push((
            "kb-1".to_string(),
            format!("node-{}", i),
            vec![i as u8],
        ));
    }

    let (tx, mut rx) = mpsc::channel(8);

    // Reconnect and drain.
    editor.collab.status = CollabStatus::Connected { peer_count: 2 };
    drain_collab_intents(&mut editor, &tx);

    // All 3 should be sent.
    for _ in 0..3 {
        assert!(
            rx.try_recv().is_ok(),
            "all offline KB updates should be sent on reconnect"
        );
    }
    assert!(rx.try_recv().is_err(), "no extra commands should be sent");
    assert!(editor.collab.pending_kb_updates.is_empty());
}

// -----------------------------------------------------------------------
// PSK wiring tests — CI-runnable (no network required)
// -----------------------------------------------------------------------
