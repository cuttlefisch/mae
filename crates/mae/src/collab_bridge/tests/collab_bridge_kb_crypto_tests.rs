//! Split from the monolithic `collab_bridge_tests.rs`: node-update signing/sealing (manifest-title blanking, pending-update draining, signed/sealed/fail-closed node-update requests).

use super::*;

/// #156 F5 — the manifest-title leak oracle (the attacker's test). A node added to an
/// E2e KB must NOT carry its cleartext title into the manifest the key-blind daemon
/// stores; the title is blanked at the editor before the `kb/collection_node_add` is
/// sent (the real title is encrypted in the node op-set). Selective control: on an
/// UNENCRYPTED KB the title rides along as before.
#[test]
fn manifest_title_blanked_on_e2e_kb_only() {
    use mae_mcp::identity::Identity;
    let title = "Secret Project Roadmap";

    // Build a SIGNED-e2e collection replica (downgrade-resistant detection) for kb-e2e,
    // and a plain replica for kb-plain.
    let owner = Identity::generate("owner");
    let (ofp, opk, osec) = (
        owner.fingerprint(),
        owner.public().to_bytes(),
        owner.secret_bytes(),
    );
    let mut e2e = mae_sync::kb::KbCollectionDoc::new_owned("E2E", &ofp, "owner");
    let k = mae_sync::content_crypto::ContentKey::generate();
    let wrap = mae_sync::content_crypto::wrap_to_member(&k, &opk).unwrap();
    e2e.author_e2e_genesis("kb-e2e", &ofp, &osec, &opk, wrap, 1000);
    let plain = mae_sync::kb::KbCollectionDoc::new_owned("PLAIN", &ofp, "owner");

    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    editor
        .collab
        .kb_collection_state
        .insert("kb-e2e".to_string(), e2e.encode_state());
    editor
        .collab
        .kb_collection_state
        .insert("kb-plain".to_string(), plain.encode_state());
    // Same node+title queued for BOTH KBs (an add op carries the title).
    editor.collab.pending_kb_manifest.push((
        "kb-e2e".to_string(),
        "concept:n".to_string(),
        title.to_string(),
        true,
    ));
    editor.collab.pending_kb_manifest.push((
        "kb-plain".to_string(),
        "concept:n".to_string(),
        title.to_string(),
        true,
    ));

    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    let mut e2e_title = None;
    let mut plain_title = None;
    while let Ok(cmd) = rx.try_recv() {
        if let CollabCommand::KbCollectionNode {
            kb_id, title, add, ..
        } = cmd
        {
            assert!(add);
            match kb_id.as_str() {
                "kb-e2e" => e2e_title = Some(title),
                "kb-plain" => plain_title = Some(title),
                _ => {}
            }
        }
    }
    assert_eq!(
        e2e_title.as_deref(),
        Some(""),
        "E2e manifest add must blank the cleartext title (no leak to the key-blind daemon)"
    );
    assert_eq!(
        plain_title.as_deref(),
        Some(title),
        "the unencrypted manifest still carries the title (selective control)"
    );
}
#[test]
fn collab_kb_drain_pending_updates_sends_commands() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    editor.collab.pending_kb_updates.push((
        "kb-1".to_string(),
        "node-a".to_string(),
        vec![1, 2, 3],
    ));
    editor.collab.pending_kb_updates.push((
        "kb-1".to_string(),
        "node-b".to_string(),
        vec![4, 5, 6],
    ));

    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    // Should have sent 2 KbNodeUpdate commands.
    let cmd1 = rx.try_recv().unwrap();
    let cmd2 = rx.try_recv().unwrap();
    match cmd1 {
        CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            epoch: _,
            e2e: _,
            pending_rowid,
        } => {
            assert_eq!(kb_id, "kb-1");
            assert_eq!(node_id, "node-a");
            assert_eq!(update, vec![1, 2, 3]);
            assert_eq!(
                pending_rowid, None,
                "in-memory updates carry no durable rowid"
            );
        }
        other => panic!(
            "expected KbNodeUpdate, got: {:?}",
            collab_command_name(&other)
        ),
    }
    match cmd2 {
        CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            epoch: _,
            e2e: _,
            pending_rowid,
        } => {
            assert_eq!(kb_id, "kb-1");
            assert_eq!(node_id, "node-b");
            assert_eq!(update, vec![4, 5, 6]);
            assert_eq!(
                pending_rowid, None,
                "in-memory updates carry no durable rowid"
            );
        }
        other => panic!(
            "expected KbNodeUpdate, got: {:?}",
            collab_command_name(&other)
        ),
    }

    // Pending list should be drained.
    assert!(editor.collab.pending_kb_updates.is_empty());
}
/// ADR-036 §D2: the editor's sign-on-push builder signs with a key-mode identity
/// (the request parses back + verifies, carrying this editor's author + the KB
/// epoch), and falls back to a legacy unsigned op when there is no identity
/// (psk/none mode).
#[test]
fn build_kb_node_update_request_signs_with_identity_else_unsigned() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_ops::SignedContentOp;
    use std::sync::Arc;

    let update = vec![1u8, 0, 2, 0, 9];
    let id = Arc::new(Identity::generate("editor"));

    // Signed: the daemon's parser reconstructs the op, the signature verifies, and
    // the author + epoch + node_id are exactly what the editor stamped. (Plaintext KB:
    // no content key ⇒ the payload is the plaintext update, op-set state unchanged.)
    let (req, op_set, op_id) = build_kb_node_update_request(
        7,
        "kb1",
        "concept:n",
        &update,
        3,
        Some(&id),
        None,
        false,
        &[],
    )
    .expect("unencrypted KB ⇒ Some (plaintext path)");
    assert!(op_set.is_empty(), "no content key ⇒ no op-set");
    assert!(op_id.is_none(), "no content key ⇒ nothing sealed");
    assert_eq!(req["id"], 7, "still a request (carries an id)");
    let parsed = SignedContentOp::from_params(&req["params"], update.clone())
        .expect("signed request parses");
    assert!(parsed.verify_signed(), "the editor's signature verifies");
    assert_eq!(
        parsed.op.author,
        id.fingerprint(),
        "authored by this editor"
    );
    assert_eq!(
        parsed.op.epoch, 3,
        "the KB epoch is carried into the header"
    );
    assert_eq!(parsed.op.node_id, "concept:n");

    // Unsigned (psk/none): no authorship header ⇒ the legacy path.
    let (unsigned, _, _) =
        build_kb_node_update_request(8, "kb1", "concept:n", &update, 3, None, None, false, &[])
            .expect("unencrypted KB ⇒ Some (plaintext path)");
    assert!(
        SignedContentOp::from_params(&unsigned["params"], update).is_none(),
        "no identity ⇒ legacy unsigned op"
    );
}
/// ADR-037 §2a (#146): on an ENCRYPTED KB, `build_kb_node_update_request` seals the
/// plaintext update into the op-set — the wire payload is the outer op-set op (NOT the
/// plaintext), signed over the ciphertext (encrypt-then-sign), and the op-set state
/// advances. The op-set carries no plaintext, yet a member opens + materializes it.
#[test]
fn build_kb_node_update_request_seals_on_encrypted_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_crypto::ContentKey;
    use mae_sync::content_ops::SignedContentOp;
    use mae_sync::kb::KbNodeDoc;
    use std::sync::Arc;

    let id = Arc::new(Identity::generate("editor"));
    let key = ContentKey::generate();
    let mut node = KbNodeDoc::new_with_client_id("n1", "", "", &[], 5);

    // Seal op 0 (the node structure), then a title edit — accumulating the op-set.
    let (_r0, op_set0, id0) = build_kb_node_update_request(
        1,
        "kb1",
        "n1",
        &node.encode_state(),
        0,
        Some(&id),
        Some(&key),
        true,
        &[],
    )
    .expect("e2e seal ⇒ Some");
    assert!(id0.is_some(), "sealing yields an op_id");
    let edit = node.set_title("Secret");
    let (req, op_set, id1) = build_kb_node_update_request(
        2,
        "kb1",
        "n1",
        &edit,
        0,
        Some(&id),
        Some(&key),
        true,
        &op_set0,
    )
    .expect("e2e seal ⇒ Some");
    assert_ne!(id0, id1, "distinct ops get distinct ids");

    // The wire payload is the OUTER op-set op, signed + verifying — and is NOT the
    // plaintext edit. The op-set bytes carry no plaintext.
    let payload =
        mae_sync::encoding::base64_to_update(req["params"]["update"].as_str().unwrap()).unwrap();
    let parsed = SignedContentOp::from_params(&req["params"], payload.clone()).expect("signed");
    assert!(
        parsed.verify_signed(),
        "signature over the sealed op verifies"
    );
    assert_ne!(
        payload, edit,
        "the wire payload is sealed, not the plaintext edit"
    );
    assert!(!op_set.is_empty(), "the op-set state advanced");
    assert!(
        !op_set.windows(6).any(|w| w == b"Secret"),
        "the op-set carries no plaintext"
    );

    // A member opens the op-set (causal order) + materializes the title.
    let opened = mae_sync::op_set::open_new_ops(&op_set, &key, &std::collections::BTreeSet::new());
    let mut reader = KbNodeDoc::from_bytes(&opened[0].1).unwrap();
    for (_oid, pt) in &opened[1..] {
        reader.apply_update(pt).unwrap();
    }
    assert_eq!(
        reader.title(),
        "Secret",
        "a member materializes the sealed edit"
    );
}
/// ADR-037 #168 — the FAIL-CLOSED oracle (the attacker's test). On an E2e KB with no
/// content key (or where sealing fails), `build_kb_node_update_request` MUST return
/// `None` (refuse) — it must NEVER fall back to emitting the plaintext update to the
/// key-blind daemon. The unencrypted KB still ships plaintext (legacy). This is the
/// regression guard for the confidentiality breach: a missing key must drop to "refuse",
/// not "leak".
#[test]
fn build_kb_node_update_request_fails_closed_on_e2e_without_key() {
    use mae_mcp::identity::Identity;
    use std::sync::Arc;
    let id = Arc::new(Identity::generate("editor"));
    let update = vec![1u8, 0, 2, 0, 9];

    assert!(
        build_kb_node_update_request(1, "kb1", "n1", &update, 0, Some(&id), None, true, &[])
            .is_none(),
        "E2e KB with no content key MUST NOT emit plaintext (fail-closed #168)"
    );
    assert!(
        build_kb_node_update_request(2, "kb1", "n1", &update, 0, None, None, true, &[]).is_none(),
        "E2e + no key refuses even on the unsigned path"
    );
    assert!(
        build_kb_node_update_request(3, "kb1", "n1", &update, 0, Some(&id), None, false, &[])
            .is_some(),
        "unencrypted KB still ships (legacy plaintext path)"
    );
    let key = mae_sync::content_crypto::ContentKey::generate();
    let node = mae_sync::kb::KbNodeDoc::new_with_client_id("n1", "", "", &[], 5);
    assert!(
        build_kb_node_update_request(
            4,
            "kb1",
            "n1",
            &node.encode_state(),
            0,
            Some(&id),
            Some(&key),
            true,
            &[]
        )
        .is_some(),
        "E2e KB WITH a key seals + ships"
    );
}
