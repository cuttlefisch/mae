//! Split from the monolithic `collab_bridge_tests.rs`: inbound sync-update notifications (serde + legacy formats) and outbound response handlers (list-docs, share-buffer, kb-join, join-seq-tracker, null-id logging).

use super::*;

#[tokio::test]
async fn handle_incoming_sync_update_notification_serde_format() {
    // Test the actual serde format: #[serde(tag = "type", content = "data")]
    let (tx, mut rx) = mpsc::channel(8);
    let mut pending = std::collections::HashMap::new();
    let mut shared = vec!["test.rs".to_string()];

    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/sync_update",
        "params": {
            "seq": 1,
            "event": {
                "type": "sync_update",
                "data": {
                    "buffer_name": "test.rs",
                    "update_base64": "AQIDBA==",
                    "wal_seq": 0
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
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::RemoteUpdate { doc_id, .. } => {
            assert_eq!(doc_id, "test.rs");
        }
        other => panic!("expected RemoteUpdate, got {:?}", other),
    }
}
#[tokio::test]
async fn handle_incoming_sync_update_notification_legacy_format() {
    // Test backward compat with the old "sync_update" key format.
    let (tx, mut rx) = mpsc::channel(8);
    let mut pending = std::collections::HashMap::new();
    let mut shared = vec!["legacy.rs".to_string()];

    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/sync_update",
        "params": {
            "seq": 1,
            "event": {
                "sync_update": {
                    "buffer_name": "legacy.rs",
                    "update_base64": "AQIDBA==",
                    "wal_seq": 0
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
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::RemoteUpdate { doc_id, .. } => {
            assert_eq!(doc_id, "legacy.rs");
        }
        other => panic!("expected RemoteUpdate, got {:?}", other),
    }
}
#[tokio::test]
async fn handle_response_list_docs() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "documents": ["a.rs", "b.org"]
        }
    });
    handle_response(
        &val,
        PendingResponseKind::ListDocs { for_join: true },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::DocList {
            documents,
            for_join,
        } => {
            assert!(for_join);
            assert_eq!(documents, vec!["a.rs", "b.org"]);
        }
        other => panic!("expected DocList, got {:?}", other),
    }
}
#[tokio::test]
async fn handle_response_share_buffer() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "doc": "test.rs", "wal_seq": 1 }
    });
    let mut seq = std::collections::HashMap::new();
    handle_response(
        &val,
        PendingResponseKind::ShareBuffer {
            doc_id: "test.rs".to_string(),
        },
        &tx,
        &mut shared,
        &mut seq,
        kb_ctx!(),
    );
    assert!(shared.contains(&"test.rs".to_string()));
    // WU2: seq_tracker should be seeded from share response wal_seq.
    assert_eq!(seq.get("test.rs"), Some(&1));
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, CollabEvent::BufferShared { doc_id } if doc_id == "test.rs"));
}
/// ADR-020 B-13 regression: a successful `kb/join` must add the collection AND
/// each node doc to `shared_docs`, or later inbound `sync_update` broadcasts for
/// `kb:<node>` are dropped at the `shared_docs.contains()` filter and the member
/// never receives live edits (emit works, receive is dead).
#[tokio::test]
async fn handle_response_kb_join_subscribes_to_collection_and_node_docs() {
    let (tx, _rx) = mpsc::channel(8);
    let mut shared: Vec<String> = Vec::new();
    let mut seq = std::collections::HashMap::new();

    let coll = mae_sync::kb::KbCollectionDoc::new("testkb", "owner");
    let coll_b64 = mae_sync::encoding::update_to_base64(&coll.encode_state());
    let node = mae_sync::kb::KbNodeDoc::new("testkb:n1", "T", "b", &[]);
    let node_b64 = mae_sync::encoding::update_to_base64(&node.encode_state());

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "collection_state": coll_b64,
            "nodes": [ { "id": "testkb:n1", "state": node_b64 } ]
        }
    });
    handle_response(
        &val,
        PendingResponseKind::KbJoin {
            kb_id: "testkb".to_string(),
        },
        &tx,
        &mut shared,
        &mut seq,
        kb_ctx!(),
    );
    assert!(
        shared.contains(&"kbc:testkb".to_string()),
        "join must subscribe to the collection doc"
    );
    assert!(
        shared.contains(&"kb:testkb:n1".to_string()),
        "join must subscribe to each node doc — else inbound live updates are dropped (B-13)"
    );
}
#[tokio::test]
async fn handle_response_join_seeds_seq_tracker() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();
    let mut seq = std::collections::HashMap::new();

    // Create a real yrs state to encode.
    let ts = mae_sync::text::TextSync::with_client_id("joined content", 1);
    let state_b64 = mae_sync::encoding::update_to_base64(&ts.encode_state());

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": { "doc": "joined.rs", "state": state_b64, "wal_seq": 7 }
    });
    handle_response(
        &val,
        PendingResponseKind::JoinDoc {
            doc_id: "joined.rs".to_string(),
        },
        &tx,
        &mut shared,
        &mut seq,
        kb_ctx!(),
    );

    // WU2: seq_tracker should be seeded from join response wal_seq.
    assert_eq!(seq.get("joined.rs"), Some(&7));
    let event = rx.try_recv().unwrap();
    assert!(matches!(event, CollabEvent::BufferJoined { doc_id, .. } if doc_id == "joined.rs"));
}
#[tokio::test]
async fn handle_incoming_logs_null_id_response() {
    // WU3: Responses with null id should be logged but not panic or emit events.
    let (tx, mut rx) = mpsc::channel(8);
    let mut pending = std::collections::HashMap::new();
    let mut shared = Vec::new();
    let mut seq = std::collections::HashMap::new();

    let msg = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#;
    handle_incoming_message(msg, &tx, &mut pending, &mut shared, &mut seq, kb_ctx!());

    // Should not emit any event (the warning is logged by tracing).
    assert!(rx.try_recv().is_err());
}

// -----------------------------------------------------------------------
// Bug 2 regression: join must set language AND invalidate syntax cache
// -----------------------------------------------------------------------
