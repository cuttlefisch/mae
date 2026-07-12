//! Split from the monolithic `collab_bridge_tests.rs`: buffer-joined language/syntax (Bug 2 regression), share-sync, disconnect/offline recovery (Bug 1 regression).

use super::*;

#[test]
fn buffer_joined_sets_language_and_invalidates_syntax() {
    let mut editor = Editor::new();

    // Create a sync doc with org content, then encode its state bytes.
    let org_content = "#+TITLE: Test\n\n- bullet one\n- bullet two\n";
    let sync = mae_sync::text::TextSync::with_client_id(org_content, 1);
    let state_bytes = sync.encode_state();

    // Feed a BufferJoined event with an org doc_id.
    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "daily.org".to_string(),
            state_bytes,
        },
    );

    let idx = editor
        .find_buffer_by_name("daily.org")
        .expect("joined buffer should exist");

    // Language should be detected as Org.
    let lang = editor.syntax.language_of(idx);
    assert_eq!(
        lang,
        Some(mae_core::syntax::Language::Org),
        "joined .org buffer should have Org language set"
    );

    // The syntax cache should be invalidated (no stale spans/tree).
    assert!(
        !editor
            .syntax
            .has_cached_spans(idx, editor.buffers[idx].generation),
        "syntax cache should be invalidated after join (no stale spans)"
    );

    // Buffer content should match the shared org content.
    assert!(editor.buffers[idx].text().contains("bullet one"));
}
#[test]
fn buffer_joined_reuses_existing_buffer_by_collab_doc_id() {
    // Regression test: if a buffer was shared (collab_doc_id set) and the
    // user also joins the same doc, BufferJoined must reuse the existing
    // buffer instead of creating a duplicate. Creating a duplicate causes
    // remote updates to be applied to the wrong sync_doc (the one without
    // the locally-typed operations), making all updates no-ops.
    let mut editor = Editor::new();

    // Simulate: buffer "2026-05-27.org" was shared, enable_sync + collab_doc_id set.
    let mut buf = mae_core::Buffer::new();
    buf.name = "2026-05-27.org".to_string();
    buf.insert_text_at(0, "shared content");
    buf.enable_sync(1000);
    buf.collab_doc_id = Some("file:abc123/daily/2026-05-27.org".to_string());
    editor.buffers.push(buf);
    editor
        .collab
        .synced_buffers
        .insert("file:abc123/daily/2026-05-27.org".to_string());
    let original_idx = editor.buffers.len() - 1;

    // Simulate: user also joins the same doc. The join resolves to
    // buf_name="daily/2026-05-27.org" (different from "2026-05-27.org").
    let sync = mae_sync::text::TextSync::with_client_id("shared content", 2000);
    let state_bytes = sync.encode_state();

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "file:abc123/daily/2026-05-27.org".to_string(),
            state_bytes,
        },
    );

    // Should NOT have created a new buffer — should reuse the existing one.
    assert!(
        editor.find_buffer_by_name("daily/2026-05-27.org").is_none(),
        "should not create duplicate buffer with different name"
    );
    // The original buffer should still be the one with the collab_doc_id.
    assert_eq!(
        editor.buffers[original_idx].collab_doc_id.as_deref(),
        Some("file:abc123/daily/2026-05-27.org"),
    );
    // Only one buffer should have this collab_doc_id.
    let matching: Vec<_> = editor
        .buffers
        .iter()
        .filter(|b| b.collab_doc_id.as_deref() == Some("file:abc123/daily/2026-05-27.org"))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "exactly one buffer should have this collab_doc_id"
    );
}
#[test]
fn buffer_joined_non_org_gets_no_language() {
    let mut editor = Editor::new();

    let content = "just plain text\n";
    let sync = mae_sync::text::TextSync::with_client_id(content, 1);
    let state_bytes = sync.encode_state();

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "notes.txt".to_string(),
            state_bytes,
        },
    );

    let idx = editor
        .find_buffer_by_name("notes.txt")
        .expect("joined buffer should exist");

    // .txt files don't have a tree-sitter grammar, so no language set.
    assert_eq!(editor.syntax.language_of(idx), None);
}

// -----------------------------------------------------------------------
// Bug 1 regression: unbiased select ensures server messages are processed
// -----------------------------------------------------------------------
// NOTE: The actual `run_collab_task` loop requires a real TCP connection,
// so we can't unit-test it directly. Instead we verify the architectural
// property: `handle_incoming_message` correctly processes a notification
// even when called after a burst of commands. This test ensures the
// message-handling path itself works; the `biased` removal ensures it
// actually gets called.
#[test]
fn drain_share_sets_synced_immediately() {
    let mut editor = Editor::new();
    let buf_name = editor.buffers[0].name.clone();
    editor.collab.pending_intent = Some(CollabIntent::ShareBuffer {
        buffer_name: buf_name.clone(),
    });
    let (tx, _rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);

    // BUG A: doc_id must be in collab_synced_buffers IMMEDIATELY.
    let expected_doc_id = format!("shared:{}", buf_name);
    assert!(
        editor.collab.synced_buffers.contains(&expected_doc_id),
        "doc_id should be in collab_synced_buffers immediately after drain"
    );
    assert_eq!(editor.collab.synced_docs, 1);
}
#[test]
fn share_failure_removes_from_synced() {
    let mut editor = Editor::new();
    // Simulate: doc was optimistically added during share.
    editor.collab.synced_buffers.insert("test-doc".to_string());
    editor.collab.synced_docs = 1;
    // Also set collab_doc_id on a buffer so the rollback can clear it.
    editor.buffers[0].collab_doc_id = Some("test-doc".to_string());

    handle_collab_event(
        &mut editor,
        CollabEvent::ShareFailed {
            doc_id: "test-doc".to_string(),
            message: "server error".to_string(),
        },
    );

    assert!(!editor.collab.synced_buffers.contains("test-doc"));
    assert_eq!(editor.collab.synced_docs, 0);
    assert!(editor.buffers[0].collab_doc_id.is_none());
}
#[test]
fn handle_disconnect_preserves_sync_for_offline_recovery() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    // Set up a buffer as if it were synced.
    let buf = &mut editor.buffers[0];
    buf.collab_doc_id = Some("test-doc".to_string());
    buf.enable_sync(42);
    buf.insert_text_at(5, "x"); // generates pending_sync_update
    editor.collab.synced_buffers.insert("test-doc".to_string());

    handle_collab_event(
        &mut editor,
        CollabEvent::Disconnected {
            reason: "test".to_string(),
        },
    );

    assert!(editor.collab.synced_buffers.is_empty());
    assert_eq!(editor.collab.synced_docs, 0);
    // WU3: sync_doc and collab_doc_id are PRESERVED for offline recovery.
    assert!(editor.buffers[0].collab_doc_id.is_some());
    assert!(editor.buffers[0].sync_doc.is_some());
    assert!(editor.buffers[0].collab_offline);
}
#[tokio::test]
async fn share_failure_emits_share_failed() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32000, "message": "storage full" }
    });
    handle_response(
        &val,
        PendingResponseKind::ShareBuffer {
            doc_id: "fail.rs".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );

    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::ShareFailed { doc_id, message } => {
            assert_eq!(doc_id, "fail.rs");
            assert!(message.contains("storage full"));
        }
        other => panic!("expected ShareFailed, got {:?}", other),
    }
    // Should NOT be in shared_docs.
    assert!(!shared.contains(&"fail.rs".to_string()));
}
#[test]
fn disconnect_sets_offline_on_all_synced_buffers() {
    // WU3: disconnect preserves sync_doc for offline recovery.
    // Buffers with sync_doc get collab_offline=true.
    // Buffers without sync_doc (ShareFailed cleared it) get collab_doc_id cleared.
    use mae_core::Buffer;
    let mut editor = Editor::new();

    // Buffer A: tracked in synced_buffers, has sync_doc.
    editor.buffers[0].name = "tracked.rs".to_string();
    editor.buffers[0].enable_sync(1);
    editor.buffers[0].collab_doc_id = Some("doc-tracked".to_string());
    editor
        .collab
        .synced_buffers
        .insert("doc-tracked".to_string());

    // Buffer B: has collab_doc_id but no sync_doc (ShareFailed cleared it).
    let mut buf_b = Buffer::new();
    buf_b.name = "orphaned.rs".to_string();
    buf_b.collab_doc_id = Some("doc-orphaned".to_string());
    // No enable_sync → sync_doc is None.
    editor.buffers.push(buf_b);

    editor.collab.status = CollabStatus::Connected { peer_count: 1 };
    editor.collab.synced_docs = 1;

    handle_collab_event(
        &mut editor,
        CollabEvent::Disconnected {
            reason: "test".to_string(),
        },
    );

    // Buffer A: sync_doc preserved, collab_offline = true.
    assert!(
        editor.buffers[0].sync_doc.is_some(),
        "tracked buffer should preserve sync_doc"
    );
    assert!(
        editor.buffers[0].collab_offline,
        "tracked buffer should be offline"
    );
    assert!(editor.buffers[0].collab_doc_id.is_some());

    // Buffer B: no sync_doc → collab_doc_id cleared (nothing to preserve).
    assert!(
        editor.buffers[1].collab_doc_id.is_none(),
        "orphaned buffer should have collab_doc_id cleared"
    );
    assert!(!editor.buffers[1].collab_offline);
}
#[test]
fn disconnect_after_share_failure_preserves_good_buffer() {
    // WU3: ShareFailed on one buffer, then Disconnect: the good buffer
    // should have its sync_doc preserved for offline recovery.
    use mae_core::Buffer;
    let mut editor = Editor::new();

    editor.buffers[0].name = "good.rs".to_string();
    editor.buffers[0].enable_sync(1);
    editor.buffers[0].collab_doc_id = Some("doc-good".to_string());
    editor.collab.synced_buffers.insert("doc-good".to_string());

    let mut buf_bad = Buffer::new();
    buf_bad.name = "bad.rs".to_string();
    buf_bad.enable_sync(2);
    buf_bad.collab_doc_id = Some("doc-bad".to_string());
    editor.buffers.push(buf_bad);
    editor.collab.status = CollabStatus::Connected { peer_count: 1 };

    // ShareFailed clears doc-bad from the buffer.
    handle_collab_event(
        &mut editor,
        CollabEvent::ShareFailed {
            doc_id: "doc-bad".to_string(),
            message: "test".to_string(),
        },
    );

    // Disconnect.
    handle_collab_event(
        &mut editor,
        CollabEvent::Disconnected {
            reason: "test".to_string(),
        },
    );

    // Good buffer: sync_doc preserved, offline=true.
    assert!(
        editor.buffers[0].sync_doc.is_some(),
        "good buffer should keep sync_doc"
    );
    assert!(editor.buffers[0].collab_offline);
    // Bad buffer: ShareFailed already cleared sync_doc, so disconnect clears collab_doc_id.
    assert!(
        editor.buffers[1].collab_doc_id.is_none(),
        "bad buffer should have doc_id cleared"
    );
}
#[tokio::test]
async fn server_notification_processed_after_command_burst() {
    let (tx, mut rx) = mpsc::channel(32);
    let mut pending = std::collections::HashMap::new();
    // Pre-subscribe to all docs so the filter passes.
    let mut shared: Vec<String> = (0..5).map(|i| format!("file{}.rs", i)).collect();

    // Simulate N sync_update notifications arriving in quick succession
    // (as would happen when they pile up during biased starvation).
    for i in 0..5 {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/sync_update",
            "params": {
                "seq": i,
                "event": {
                    "type": "sync_update",
                    "data": {
                        "buffer_name": format!("file{}.rs", i),
                        "update_base64": "AQIDBA==",
                        "wal_seq": i
                    }
                }
            }
        });
        handle_incoming_message(
            &msg.to_string(),
            &tx,
            &mut pending,
            &mut shared,
            &mut std::collections::HashMap::new(),
            kb_ctx!(),
        );
    }

    // All 5 should have produced RemoteUpdate events.
    let mut received = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let CollabEvent::RemoteUpdate { doc_id, .. } = event {
            received.push(doc_id);
        }
    }
    assert_eq!(
        received.len(),
        5,
        "all queued server notifications must be processed; got {:?}",
        received
    );
}
#[tokio::test]
async fn unsubscribed_doc_sync_update_ignored() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut pending = std::collections::HashMap::new();
    let mut shared = vec!["subscribed.rs".to_string()]; // Only subscribed to one doc.

    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/sync_update",
        "params": {
            "seq": 1,
            "event": {
                "type": "sync_update",
                "data": {
                    "buffer_name": "other-client.rs",
                    "update_base64": "AQIDBA==",
                    "wal_seq": 1
                }
            }
        }
    });
    handle_incoming_message(
        &msg.to_string(),
        &tx,
        &mut pending,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    // No event should be emitted for the unsubscribed doc.
    assert!(
        rx.try_recv().is_err(),
        "sync_update for unsubscribed doc should be ignored"
    );
}

// -----------------------------------------------------------------------
// Join-save model: joined buffers have no auto file_path
// -----------------------------------------------------------------------
