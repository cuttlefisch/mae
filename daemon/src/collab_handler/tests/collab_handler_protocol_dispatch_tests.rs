use super::*;

#[tokio::test]
async fn kb_share_preserves_membership_on_owner_reshare() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut sd = HashSet::new();

    // First share: a collection that already carries an approved member.
    let mut coll = KbCollectionDoc::new("testkb", "alice");
    coll.add_node("testkb:n1", "T");
    coll.upsert_member("SHA256:bob", "bob", SyncRole::Editor);
    let share1 = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {
            "kb_id": "testkb", "name": "testkb",
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": []
        }
    });
    handle_doc_request(
        &share1.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut sd,
    )
    .await;
    let c1 = load_collection(&store, "testkb").await.unwrap();
    assert!(
        c1.role_of("SHA256:bob").is_some(),
        "bob is a member after the first share"
    );

    // Owner RE-SHARES an owner-only collection (no members) — the clobber input.
    let owner_only = KbCollectionDoc::new("testkb", "alice");
    let share2 = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "kb/share",
        "params": {
            "kb_id": "testkb", "name": "testkb",
            "collection_state": update_to_base64(&owner_only.encode_state()),
            "nodes": []
        }
    });
    handle_doc_request(
        &share2.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        1,
        &mut HashSet::new(),
    )
    .await;

    // B-12: bob's membership must SURVIVE the re-share.
    let c2 = load_collection(&store, "testkb").await.unwrap();
    assert!(
        c2.role_of("SHA256:bob").is_some(),
        "B-12: owner re-share must preserve approved members, not silently revoke them"
    );
}

#[tokio::test]
async fn kb_collection_node_add_remove_updates_manifest() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut sd = HashSet::new();

    // Share a collection that starts with one node.
    let mut coll = KbCollectionDoc::new("testkb", "alice");
    coll.add_node("testkb:n1", "One");
    let share = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {
            "kb_id": "testkb", "name": "testkb",
            "collection_state": update_to_base64(&coll.encode_state()), "nodes": []
        }
    });
    handle_doc_request(
        &share.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut sd,
    )
    .await;

    // Add a node to the manifest via the new RPC.
    let add = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "kb/collection_node_add",
        "params": { "kb_id": "testkb", "node_id": "testkb:n2", "title": "Two" }
    });
    let resp = handle_doc_request(
        &add.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut sd,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "collection_node_add failed: {:?}",
        resp.error
    );
    let ids: Vec<String> = load_collection(&store, "testkb")
        .await
        .unwrap()
        .list_nodes()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(
        ids.contains(&"testkb:n2".to_string()),
        "added node must be in the manifest: {ids:?}"
    );

    // Remove the original node.
    let rm = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "kb/collection_node_remove",
        "params": { "kb_id": "testkb", "node_id": "testkb:n1" }
    });
    let resp = handle_doc_request(
        &rm.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut sd,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "collection_node_remove failed: {:?}",
        resp.error
    );
    let ids: Vec<String> = load_collection(&store, "testkb")
        .await
        .unwrap()
        .list_nodes()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert!(
        !ids.contains(&"testkb:n1".to_string()),
        "removed node must be gone: {ids:?}"
    );
    assert!(
        ids.contains(&"testkb:n2".to_string()),
        "the added node remains: {ids:?}"
    );
}

#[tokio::test]
async fn sync_update_missing_doc_returns_error() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // sync/update without "doc" param should return an error (not silently use "default").
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/update",
        "params": { "update": "AAAA" }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut HashSet::new(),
    )
    .await;
    assert!(
        resp.error.is_some(),
        "sync/update without doc should return error"
    );
}

#[tokio::test]
async fn sync_update_oversized_rejected() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Create a base64 string that decodes to > the effective per-update gate.
    let big_data = vec![0u8; store.max_update_size() + 1];
    let big_b64 = mae_sync::encoding::update_to_base64(&big_data);

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/update",
        "params": { "doc": "test", "update": big_b64 }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut HashSet::new(),
    )
    .await;
    assert!(resp.error.is_some(), "oversized update should be rejected");
    let err_msg = resp.error.unwrap().message;
    assert!(
        err_msg.contains("too large"),
        "error should mention size: {err_msg}"
    );
}

#[tokio::test]
async fn resync_with_suffix_matching() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Create a doc with a file: prefix address.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "shared content");
    store
        .apply_update("file:no-project/test.txt", &update, None)
        .await
        .unwrap();

    // Resync using bare filename — suffix matching should resolve.
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/resync",
        "params": { "doc": "test.txt" }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut HashSet::new(),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "resync should succeed: {:?}",
        resp.error
    );
    let result = resp.result.unwrap();
    // The response should use the resolved full name.
    assert_eq!(result["doc"], "file:no-project/test.txt");
    // State should be non-empty (contains the shared content).
    assert!(!result["state"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn docs_metadata_returns_save_epoch() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Create a doc and record a save.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "hello");
    store.apply_update("test", &update, Some(1)).await.unwrap();
    store.record_save("test", "alice").await.unwrap();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "docs/metadata",
        "params": { "doc": "test" }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut HashSet::new(),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "docs/metadata failed: {:?}",
        resp.error
    );
    let result = resp.result.unwrap();
    assert_eq!(result["doc"], "test");
    assert_eq!(result["last_saved_by"], "alice");
    assert!(result["content_length"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn unknown_method_returns_error() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/nonexistent"
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut HashSet::new(),
    )
    .await;
    assert!(resp.error.is_some());
    assert!(resp.error.unwrap().message.contains("Unknown method"));
}

#[test]
fn is_notification_detects_no_id() {
    let notif = r#"{"jsonrpc":"2.0","method":"sync/awareness","params":{}}"#;
    assert!(is_notification(notif));

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"sync/awareness","params":{}}"#;
    assert!(!is_notification(request));

    let response = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700}}"#;
    assert!(!is_notification(response));
}

#[tokio::test]
async fn awareness_notification_no_response() {
    // Sending sync/awareness as a notification (no id) should relay the
    // broadcast but NOT generate any response.
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Subscribe a second client to receive the broadcast.
    let session_id_sender = 1u64;
    let session_id_receiver = 2u64;
    let mut rx = {
        let mut b = bc.lock().unwrap();
        b.subscribe(session_id_receiver, vec!["sync_update".to_string()])
    };

    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "sync/awareness",
        "params": {
            "doc": "test.rs",
            "state": {
                "user_name": "alice",
                "cursor_row": 10,
                "cursor_col": 5
            }
        }
    });

    let mut session_docs = HashSet::new();
    handle_doc_notification(
        &msg.to_string(),
        &store,
        &bc,
        session_id_sender,
        &mut session_docs,
    )
    .await;

    // Verify: session_docs tracks the doc for cleanup.
    assert!(session_docs.contains("test.rs"));

    // Verify: broadcast was relayed (receiver should get AwarenessUpdate).
    if let Ok(event) = rx.try_recv() {
        match event {
            EditorEvent::AwarenessUpdate {
                doc_id,
                user_name,
                cursor_row,
                cursor_col,
                ..
            } => {
                assert_eq!(doc_id, "test.rs");
                assert_eq!(user_name, "alice");
                assert_eq!(cursor_row, 10);
                assert_eq!(cursor_col, 5);
            }
            other => panic!("expected AwarenessUpdate, got {:?}", other),
        }
    }
    // No response was generated — that's the whole point of handling notifications.
}

#[tokio::test]
async fn awareness_with_id_returns_ack() {
    // Backward compat: sync/awareness WITH an id should return a success response.
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "sync/awareness",
        "params": {
            "doc": "test.rs",
            "state": {
                "user_name": "bob",
                "cursor_row": 0,
                "cursor_col": 0
            }
        }
    });

    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        1,
        &mut HashSet::new(),
    )
    .await;

    // Should succeed (not error) and echo back the doc name.
    assert!(
        resp.error.is_none(),
        "awareness with id should succeed: {:?}",
        resp.error
    );
    assert_eq!(resp.result.unwrap()["doc"], "test.rs");
}

#[tokio::test]
async fn notification_for_unknown_method_is_silently_dropped() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = r#"{"jsonrpc":"2.0","method":"sync/unknown_notification","params":{}}"#;
    let mut session_docs = HashSet::new();

    // Should not panic or error — just log and return.
    handle_doc_notification(msg, &store, &bc, 1, &mut session_docs).await;
}
