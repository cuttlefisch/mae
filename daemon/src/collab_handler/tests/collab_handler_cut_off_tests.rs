//! Every path that takes a principal's read access away also cuts off the
//! sessions they already have open: a block, and a quorum revoke once it
//! reaches its threshold (removal is in `collab_handler_manifest_op_tests`).

use super::*;
use mae_mcp::identity::Identity;
use tokio::sync::mpsc::Receiver;

/// Does a session subscribed to `doc` still receive an update to it?
fn still_receives(bc: &SharedBroadcaster, rx: &mut Receiver<EditorEvent>, doc: &str) -> bool {
    while rx.try_recv().is_ok() {}
    bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
        buffer_name: doc.to_string(),
        update_base64: "AA==".into(),
        wal_seq: 1,
        content_header: None,
    });
    rx.try_recv().is_ok()
}

fn open_session(bc: &SharedBroadcaster, sid: u64, fp: &str, doc: &str) -> Receiver<EditorEvent> {
    let rx = register_session(bc, sid, Some(fp));
    let mut b = bc.lock().unwrap();
    b.add_event_sub(sid, "sync_update");
    b.subscribe_doc(sid, doc);
    drop(b);
    rx
}

/// Kills the block not cutting the blocked principal's sessions off.
#[tokio::test]
async fn a_blocked_principals_open_sessions_stop_receiving_the_kb() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    store.set_signer(Arc::new(owner));
    let (bob, carol) = (fp("bob"), fp("carol"));
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbx",
        "owner",
        &mut docs,
    )
    .await;
    for m in [&bob, &carol] {
        let msg = kb_member_msg("kb/add_member", "kbx", m, Some("editor"));
        dispatch_as(&store, &bc, Some("owner"), Some(&owner_fp), msg, &mut docs).await;
    }
    let node = mae_sync::kb_node_doc_name("kbx", "concept:n");
    let mut bob_rx = open_session(&bc, 701, &bob, &node);
    let mut carol_rx = open_session(&bc, 702, &carol, &node);
    assert!(still_receives(&bc, &mut bob_rx, &node), "baseline");

    let msg = kb_block_msg("kb/block_principal", "kbx", &bob);
    let r = dispatch_as(&store, &bc, Some("owner"), Some(&owner_fp), msg, &mut docs).await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(
        !still_receives(&bc, &mut bob_rx, &node),
        "the blocked principal is cut off"
    );
    assert!(
        still_receives(&bc, &mut carol_rx, &node),
        "another member still receives"
    );

    // Unblocking cuts nobody off.
    let msg = kb_block_msg("kb/unblock_principal", "kbx", &carol);
    dispatch_as(&store, &bc, Some("owner"), Some(&owner_fp), msg, &mut docs).await;
    assert!(
        still_receives(&bc, &mut carol_rx, &node),
        "an unblock cuts nobody off"
    );
}

/// One co-signature below the quorum leaves the subject's sessions open; the
/// one that reaches it cuts them off. Kills both the revoke not cutting off
/// and the revoke cutting off before the quorum is reached.
#[tokio::test]
async fn a_revoke_cuts_off_only_once_it_reaches_its_quorum() {
    let (store_a, store_b) = (test_doc_store(), test_doc_store());
    let bc = test_broadcaster();
    let (mut docs_a, mut docs_b) = (HashSet::new(), HashSet::new());
    let (owner1, owner2) = (Identity::generate("owner1"), Identity::generate("owner2"));
    let (o1, o2) = (owner1.fingerprint(), owner2.fingerprint());
    let o1_pk = owner1.public().to_bytes();
    store_a.set_signer(Arc::new(owner1));
    store_b.set_signer(Arc::new(owner2));
    let bob = fp("bob");
    kb_share_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&o1),
        "kbq",
        "owner1",
        &mut docs_a,
    )
    .await;
    for msg in [
        kb_member_msg("kb/add_member", "kbq", &o2, Some("owner")),
        kb_member_msg("kb/add_member", "kbq", &bob, Some("editor")),
        kb_set_governance_msg("kbq", "quorum:2"),
    ] {
        let r = dispatch_as(&store_a, &bc, Some("owner1"), Some(&o1), msg, &mut docs_a).await;
        assert!(r.error.is_none(), "{:?}", r.error);
    }
    store_a.set_kb_anchor("kbq", o1_pk).await.unwrap();
    let node = mae_sync::kb_node_doc_name("kbq", "concept:n");
    let mut bob_rx = open_session(&bc, 801, &bob, &node);

    let r = dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&o1),
        kb_revoke_msg("kbq", &bob),
        &mut docs_a,
    )
    .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(
        still_receives(&bc, &mut bob_rx, &node),
        "below quorum: not cut off"
    );

    let (state, _) = store_a.encode_state_and_sv("kbc:kbq").await.unwrap();
    store_b
        .apply_update("kbc:kbq", &state, None, TEST_BY)
        .await
        .unwrap();
    store_b.set_kb_anchor("kbq", o1_pk).await.unwrap();
    let r = dispatch_as(
        &store_b,
        &bc,
        Some("owner2"),
        Some(&o2),
        kb_revoke_msg("kbq", &bob),
        &mut docs_b,
    )
    .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert!(
        !still_receives(&bc, &mut bob_rx, &node),
        "quorum reached: cut off"
    );
}
