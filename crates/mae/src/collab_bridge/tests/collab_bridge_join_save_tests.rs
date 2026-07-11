//! Split from the monolithic `collab_bridge_tests.rs`: join-save model (joined buffers have no auto file_path), save-intent handling, peer count.

use super::*;

#[test]
fn buffer_joined_has_no_file_path() {
    let mut editor = Editor::new();
    let content = "shared text\n";
    let sync = mae_sync::text::TextSync::with_client_id(content, 1);
    let state_bytes = sync.encode_state();

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "file:abc123/src/main.rs".to_string(),
            state_bytes,
        },
    );

    let idx = editor
        .find_buffer_by_name("src/main.rs")
        .expect("joined buffer should use rel_path as name");
    // Joined buffers must NOT have auto file_path set.
    assert!(
        editor.buffers[idx].file_path().is_none(),
        "joined buffer should have no file_path by default"
    );
    // But collab_doc_id should be set.
    assert_eq!(
        editor.buffers[idx].collab_doc_id.as_deref(),
        Some("file:abc123/src/main.rs")
    );
}
#[test]
fn buffer_joined_sets_buffer_name_from_rel_path() {
    let mut editor = Editor::new();
    let sync = mae_sync::text::TextSync::with_client_id("hi", 1);
    let state_bytes = sync.encode_state();

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "file:proj/utils.rs".to_string(),
            state_bytes,
        },
    );

    assert!(
        editor.find_buffer_by_name("utils.rs").is_some(),
        "buffer name should be the rel_path from DocAddress"
    );
}
#[test]
fn buffer_joined_shared_doc_name_extraction() {
    let mut editor = Editor::new();
    let sync = mae_sync::text::TextSync::with_client_id("data", 1);
    let state_bytes = sync.encode_state();

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "shared:notes".to_string(),
            state_bytes,
        },
    );

    assert!(
        editor.find_buffer_by_name("notes").is_some(),
        "shared doc buffer name should be the name field"
    );
}
#[test]
fn drain_save_collab_sends_save_intent() {
    let mut editor = Editor::new();
    editor.collab.pending_intent = Some(CollabIntent::SaveCollab {
        doc_id: "file:abc/main.rs".to_string(),
        content_hash: "deadbeef".to_string(),
    });
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);
    let cmd = rx.try_recv().unwrap();
    match cmd {
        CollabCommand::SendSaveIntent {
            doc_id,
            expected_hash,
        } => {
            assert_eq!(doc_id, "file:abc/main.rs");
            assert_eq!(expected_hash, "deadbeef");
        }
        other => panic!("expected SendSaveIntent, got {:?}", other),
    }
}
#[test]
fn drain_pending_save_committed() {
    let mut editor = Editor::new();
    editor.collab.pending_save_committed = Some((
        "doc1".to_string(),
        42,
        "hash123".to_string(),
        "alice".to_string(),
    ));
    let (tx, mut rx) = mpsc::channel(8);
    drain_collab_intents(&mut editor, &tx);
    let cmd = rx.try_recv().unwrap();
    match cmd {
        CollabCommand::SendSaveCommitted {
            doc_id,
            save_epoch,
            content_hash,
            saved_by,
        } => {
            assert_eq!(doc_id, "doc1");
            assert_eq!(save_epoch, 42);
            assert_eq!(content_hash, "hash123");
            assert_eq!(saved_by, "alice");
        }
        other => panic!("expected SendSaveCommitted, got {:?}", other),
    }
    assert!(editor.collab.pending_save_committed.is_none());
}
#[test]
fn handle_save_intent_ok_queues_committed() {
    let mut editor = Editor::new();
    editor.collab.user_name = "bob".to_string();
    handle_collab_event(
        &mut editor,
        CollabEvent::SaveIntentOk {
            doc_id: "test-doc".to_string(),
            save_epoch: 5,
            content_hash: "abc".to_string(),
        },
    );
    assert!(editor.collab.pending_save_committed.is_some());
    let (doc_id, epoch, hash, saved_by) = editor.collab.pending_save_committed.as_ref().unwrap();
    assert_eq!(doc_id, "test-doc");
    assert_eq!(*epoch, 5);
    assert_eq!(hash, "abc");
    assert_eq!(saved_by, "bob");
}
#[test]
fn handle_save_intent_conflict_shows_status() {
    let mut editor = Editor::new();
    handle_collab_event(
        &mut editor,
        CollabEvent::SaveIntentConflict {
            doc_id: "test-doc".to_string(),
            message: "hash mismatch".to_string(),
        },
    );
    assert!(editor.status_msg.contains("conflict"));
}
#[tokio::test]
async fn handle_response_save_intent_ok() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "doc": "test.rs",
            "result": {
                "status": "ok",
                "server_hash": "abc123",
                "save_epoch": 3
            }
        }
    });
    handle_response(
        &val,
        PendingResponseKind::SaveIntent {
            doc_id: "test.rs".to_string(),
            expected_hash: "abc123".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::SaveIntentOk {
            doc_id, save_epoch, ..
        } => {
            assert_eq!(doc_id, "test.rs");
            assert_eq!(save_epoch, 3);
        }
        other => panic!("expected SaveIntentOk, got {:?}", other),
    }
}
#[tokio::test]
async fn handle_response_save_intent_conflict() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "doc": "test.rs",
            "result": {
                "status": "conflict",
                "server_hash": "xyz"
            }
        }
    });
    handle_response(
        &val,
        PendingResponseKind::SaveIntent {
            doc_id: "test.rs".to_string(),
            expected_hash: "abc123".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx.try_recv().unwrap();
    assert!(
        matches!(event, CollabEvent::SaveIntentConflict { .. }),
        "expected SaveIntentConflict, got {:?}",
        event
    );
}
/// B-1: a kb/join response must surface joined / pending / denied as three
/// DISTINCT outcomes — not "Joined (0 nodes)" for all of them.
#[tokio::test]
async fn kb_join_pending_response_is_distinct() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let val = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "kb_id": "collabtest", "status": "pending" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "collabtest".into(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    match rx.try_recv().unwrap() {
        CollabEvent::StatusReport { lines } => {
            assert!(
                lines.iter().any(|l| l.contains("pending")),
                "pending join should report pending approval, got {lines:?}"
            );
        }
        other => panic!("expected StatusReport for pending, got {other:?}"),
    }
}
#[tokio::test]
async fn kb_join_denied_response_is_distinct() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let val = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "error": { "code": -32603, "message": "not a member of KB 'collabtest'" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "collabtest".into(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    match rx.try_recv().unwrap() {
        CollabEvent::Error { message } => {
            assert!(
                message.contains("denied"),
                "denied join should report denial, got {message:?}"
            );
        }
        other => panic!("expected Error for denied join, got {other:?}"),
    }
}
#[tokio::test]
async fn kb_join_success_response_emits_joined() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let val = serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": { "kb_id": "collabtest", "collection_state": "", "nodes": [] }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "collabtest".into(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    assert!(
        matches!(rx.try_recv().unwrap(), CollabEvent::KbJoined { .. }),
        "a real join must emit KbJoined"
    );
}
#[test]
fn peer_count_zero_shows_all_disconnected() {
    let mut editor = Editor::new();
    editor.collab.status = CollabStatus::Connected { peer_count: 2 };
    handle_collab_event(&mut editor, CollabEvent::PeerCountChanged { peer_count: 0 });
    assert!(editor.status_msg.contains("disconnected"));
    assert_eq!(
        editor.collab.status,
        CollabStatus::Connected { peer_count: 0 }
    );
}
#[test]
fn save_pathless_collab_buffer_shows_guidance() {
    let mut editor = Editor::new();
    let sync = mae_sync::text::TextSync::with_client_id("text", 1);
    let state_bytes = sync.encode_state();

    handle_collab_event(
        &mut editor,
        CollabEvent::BufferJoined {
            doc_id: "shared:test".to_string(),
            state_bytes,
        },
    );

    let idx = editor
        .find_buffer_by_name("test")
        .expect("buffer should exist");
    editor.switch_to_buffer(idx);
    // Use dispatch_builtin("save") which is public and calls save_current_buffer.
    editor.dispatch_builtin("save");

    // Should show guidance about :saveas
    let status = &editor.status_msg;
    assert!(
        status.contains("saveas"),
        "status should mention :saveas, got: {status}"
    );
}

// --- WU1: Gap detection tests ---
