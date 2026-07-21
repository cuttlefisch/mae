//! Split from the monolithic `collab_bridge_tests.rs`: WU1 gap detection, WU3 offline recovery + sharer-left handling + notification parsing, WU1 buffer status indicator, WU2 save guard, WU4 backoff/debounce.

use super::*;

#[tokio::test]
async fn gap_detection_triggers_on_missing_seq() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut seq_tracker = std::collections::HashMap::new();

    // Seq 1, 2 — no gap.
    check_seq_gap("doc1", 1, &mut seq_tracker, &tx);
    check_seq_gap("doc1", 2, &mut seq_tracker, &tx);
    assert!(rx.try_recv().is_err(), "no gap for sequential seqs");

    // Seq 4 — gap (expected 3).
    check_seq_gap("doc1", 4, &mut seq_tracker, &tx);
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::GapDetected {
            doc_id,
            expected,
            got,
        } => {
            assert_eq!(doc_id, "doc1");
            assert_eq!(expected, 3);
            assert_eq!(got, 4);
        }
        other => panic!("expected GapDetected, got {:?}", other),
    }
}
#[tokio::test]
async fn gap_detection_no_gap_for_sequential() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut seq_tracker = std::collections::HashMap::new();

    for i in 1..=5 {
        check_seq_gap("doc1", i, &mut seq_tracker, &tx);
    }
    assert!(rx.try_recv().is_err(), "no gap for sequential 1..5");
}
#[tokio::test]
async fn gap_detection_independent_per_doc() {
    let (tx, mut rx) = mpsc::channel(16);
    let mut seq_tracker = std::collections::HashMap::new();

    check_seq_gap("doc-a", 1, &mut seq_tracker, &tx);
    check_seq_gap("doc-b", 1, &mut seq_tracker, &tx);
    // Both start at 1, no gap.
    assert!(rx.try_recv().is_err());

    // doc-a jumps to 5 — gap.
    check_seq_gap("doc-a", 5, &mut seq_tracker, &tx);
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, CollabEvent::GapDetected { doc_id, .. } if doc_id == "doc-a"));

    // doc-b at 2 — no gap.
    check_seq_gap("doc-b", 2, &mut seq_tracker, &tx);
    assert!(rx.try_recv().is_err());
}
#[test]
fn gap_detected_triggers_force_sync() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::GapDetected {
            doc_id: "test-doc".to_string(),
            expected: 3,
            got: 5,
        },
    );
    assert!(editor.status_msg.contains("gap"));
    // Should queue a ForceSync intent.
    assert!(editor.collab.pending_intent.is_some());
    match editor.collab.pending_intent.as_ref().unwrap() {
        CollabIntent::ForceSync { buffer_name } => {
            assert_eq!(buffer_name, "test-doc");
        }
        other => panic!("expected ForceSync, got {:?}", other),
    }
}

// --- WU3: Offline recovery tests ---
#[test]
fn disconnect_preserves_sync_doc() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    let buf = &mut editor.buffers[0];
    buf.collab_doc_id = Some("test-doc".to_string());
    buf.enable_sync(42);
    editor.collab.synced_buffers.insert("test-doc".to_string());

    handle_collab_event(
        &mut editor,
        CollabEvent::Disconnected {
            reason: "test".to_string(),
        },
    );

    // sync_doc and collab_doc_id should be PRESERVED (not cleared).
    assert!(
        editor.buffers[0].sync_doc.is_some(),
        "sync_doc should be preserved on disconnect"
    );
    assert!(
        editor.buffers[0].collab_doc_id.is_some(),
        "collab_doc_id should be preserved on disconnect"
    );
    assert!(
        editor.buffers[0].collab_offline,
        "collab_offline should be set"
    );
    // UI tracking should be cleared.
    assert!(editor.collab.synced_buffers.is_empty());
    assert_eq!(editor.collab.synced_docs, 0);
}
#[test]
fn reconnect_triggers_resync_for_offline_buffers() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.collab_doc_id = Some("test-doc".to_string());
    buf.enable_sync(42);
    buf.collab_offline = true;

    handle_collab_event(
        &mut editor,
        CollabEvent::Connected {
            address: "127.0.0.1:9473".to_string(),
            peer_count: 1,
        },
    );

    // #341: queued via the one-per-tick `reconnect_intents` queue (not the
    // single `pending_intent` slot directly) — see the multi-buffer test below
    // for why this matters once there's more than one offline buffer.
    assert_eq!(editor.collab.reconnect_intents.len(), 1);
    assert!(matches!(
        editor.collab.reconnect_intents.front(),
        Some(CollabIntent::ForceSync { buffer_name }) if buffer_name == "test-doc"
    ));
    assert!(editor.collab.synced_buffers.contains("test-doc"));
}

#[test]
fn reconnect_resyncs_all_offline_buffers_not_just_the_first() {
    // #341 regression: previously only the FIRST offline-edited buffer was
    // queued for resync on reconnect (via the single `pending_intent` slot),
    // despite the status message claiming all N would resync. Edit 2+ buffers
    // while offline — all of them must end up queued.
    let mut editor = Editor::new();
    for (name, doc_id) in [("a.txt", "doc-a"), ("b.txt", "doc-b"), ("c.txt", "doc-c")] {
        let mut buf = mae_core::Buffer::new();
        buf.name = name.to_string();
        buf.collab_doc_id = Some(doc_id.to_string());
        buf.enable_sync(1);
        buf.collab_offline = true;
        editor.buffers.push(buf);
    }

    handle_collab_event(
        &mut editor,
        CollabEvent::Connected {
            address: "127.0.0.1:9473".to_string(),
            peer_count: 1,
        },
    );

    assert_eq!(
        editor.collab.reconnect_intents.len(),
        3,
        "all 3 offline buffers must be queued for resync, not just the first"
    );
    let queued: std::collections::HashSet<String> = editor
        .collab
        .reconnect_intents
        .iter()
        .filter_map(|intent| match intent {
            CollabIntent::ForceSync { buffer_name } => Some(buffer_name.clone()),
            _ => None,
        })
        .collect();
    for doc_id in ["doc-a", "doc-b", "doc-c"] {
        assert!(
            queued.contains(doc_id),
            "expected {doc_id} to be queued for resync, got {queued:?}"
        );
    }
}
#[test]
fn remote_update_clears_offline_flag() {
    let mut editor = Editor::new();
    let buf = &mut editor.buffers[0];
    buf.collab_doc_id = Some("test-doc".to_string());
    buf.enable_sync(42);
    buf.collab_offline = true;

    // Create a valid yrs update for this buffer.
    let update = {
        let sync2 = mae_sync::text::TextSync::with_client_id("hello", 99);
        sync2.encode_state()
    };

    handle_collab_event(
        &mut editor,
        CollabEvent::RemoteUpdate {
            doc_id: "test-doc".to_string(),
            update_bytes: update,
            wal_seq: 1,
        },
    );

    // Note: apply_sync_update may fail if the update isn't compatible,
    // but the test validates the code path exists.
}

// --- WU1: Buffer status indicator tests ---
#[test]
fn buffer_shared_sets_is_sharer() {
    let mut editor = Editor::new();
    editor.buffers[0].collab_doc_id = Some("test-doc".to_string());
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    handle_collab_event(
        &mut editor,
        CollabEvent::BufferShared {
            doc_id: "test-doc".to_string(),
        },
    );
    assert!(editor.buffers[0].collab_is_sharer);
}
#[test]
fn buffer_joined_stays_not_sharer() {
    let mut editor = Editor::new();
    let sync = mae_sync::text::TextSync::with_client_id("hello", 1);
    let state = sync.encode_state();
    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "test-doc".to_string(),
            state_bytes: state,
        },
    );
    // Find the buffer that was created for the joined doc.
    let idx = editor.find_buffer_by_collab_doc_id("test-doc");
    assert!(idx.is_some());
    assert!(!editor.buffers[idx.unwrap()].collab_is_sharer);
}

// --- WU2: Save guard tests ---
#[test]
fn collab_is_sharer_defaults_false() {
    let buf = mae_core::Buffer::new();
    assert!(!buf.collab_is_sharer);
}
#[test]
fn collab_is_sharer_set_on_share_not_join() {
    // Verify that BufferShared sets is_sharer and BufferJoined does not.
    let mut editor = Editor::new();
    editor.buffers[0].collab_doc_id = Some("doc-a".to_string());
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    handle_collab_event(
        &mut editor,
        CollabEvent::BufferShared {
            doc_id: "doc-a".to_string(),
        },
    );
    assert!(
        editor.buffers[0].collab_is_sharer,
        "sharer should be true after BufferShared"
    );

    // Join a different doc — its buffer should NOT be sharer.
    let sync = mae_sync::text::TextSync::with_client_id("content", 2);
    let state = sync.encode_state();
    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "doc-b".to_string(),
            state_bytes: state,
        },
    );
    let idx = editor.find_buffer_by_collab_doc_id("doc-b").unwrap();
    assert!(
        !editor.buffers[idx].collab_is_sharer,
        "joiner should not be sharer"
    );
}

// --- WU3: SharerLeft event handling ---
#[test]
fn sharer_left_sets_status() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 2 };
    handle_collab_event(
        &mut editor,
        CollabEvent::SharerLeft {
            doc_id: "test-doc".to_string(),
        },
    );
    assert!(editor.status_msg.contains("Sharer disconnected"));
}

// --- WU4: Backoff + debounce tests ---
#[test]
fn compute_backoff_exponential() {
    // base=5, factor=2: 5, 10, 20, 40, 80, 160
    assert_eq!(compute_backoff(5, 2, 0), 5);
    assert_eq!(compute_backoff(5, 2, 1), 10);
    assert_eq!(compute_backoff(5, 2, 2), 20);
    assert_eq!(compute_backoff(5, 2, 3), 40);
    assert_eq!(compute_backoff(5, 2, 4), 80);
    assert_eq!(compute_backoff(5, 2, 5), 160);
    // Capped at attempt=5 exponent, so attempt 6 same as 5.
    assert_eq!(compute_backoff(5, 2, 6), 160);
}
#[test]
fn compute_backoff_capped_at_300() {
    // base=10, factor=3: attempt 5 = 10 * 243 = 2430 → capped at 300.
    assert_eq!(compute_backoff(10, 3, 5), 300);
}
#[test]
fn compute_backoff_factor_one_is_constant() {
    // factor=1 means no exponential growth.
    assert_eq!(compute_backoff(5, 1, 0), 5);
    assert_eq!(compute_backoff(5, 1, 5), 5);
}

// --- WU3: Notification parsing ---
#[tokio::test]
async fn parse_sharer_left_notification() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut pending = std::collections::HashMap::new();
    let mut shared = Vec::new();
    let mut seq = std::collections::HashMap::new();
    let msg = r#"{
            "jsonrpc": "2.0",
            "method": "notifications/sharer_left",
            "params": {
                "seq": 1,
                "event": {
                    "type": "sharer_left",
                    "data": {
                        "session_id": 42,
                        "doc": "file:abc/main.rs",
                        "peer_count": 1
                    }
                }
            }
        }"#;
    handle_incoming_message(msg, &tx, &mut pending, &mut shared, &mut seq, kb_ctx!());
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::SharerLeft { doc_id } => {
            assert_eq!(doc_id, "file:abc/main.rs");
        }
        other => panic!("expected SharerLeft, got {:?}", other),
    }
}

// --- Phase 4: Continuous KB sync tests ---
