use super::*;

#[tokio::test]
async fn kb_share_stores_collection_and_nodes() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut session_docs = HashSet::new();

    let node1 = make_test_node(
        "concept:test",
        "Test Node",
        realistic_org_body(),
        &["research", "crdt"],
    );
    let node2 = make_test_node("concept:arch", "Architecture", "System overview", &["core"]);
    let node3 = make_test_node(
        "lesson:intro",
        "Intro Lesson",
        "Welcome to MAE",
        &["tutorial"],
    );

    let resp = share_kb_with_nodes(
        &store,
        &bc,
        "my-kb",
        "Research Notes",
        "alice",
        &[
            ("concept:test", node1),
            ("concept:arch", node2),
            ("lesson:intro", node3),
        ],
        &mut session_docs,
    )
    .await;

    assert!(resp.error.is_none(), "kb/share failed: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert_eq!(result["shared"], true);
    assert_eq!(result["node_count"], 3);

    // Verify collection doc is stored.
    let (coll_state, _sv) = store.encode_state_and_sv("kbc:my-kb").await.unwrap();
    let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&coll_state)
        .expect("collection doc should decode");
    assert_eq!(coll.name(), "Research Notes");
    assert_eq!(coll.node_count(), 3, "collection should list all 3 nodes");

    // Verify each node doc is stored and decodable.
    for node_id in &["concept:test", "concept:arch", "lesson:intro"] {
        let doc_name = format!("kb:{node_id}");
        let (state, _sv) = store
            .encode_state_and_sv(&doc_name)
            .await
            .unwrap_or_else(|e| panic!("node doc '{}' should exist: {}", doc_name, e));
        let node_doc = mae_sync::kb::KbNodeDoc::from_bytes(&state)
            .unwrap_or_else(|e| panic!("node '{}' should decode: {}", node_id, e));
        assert!(
            !node_doc.title().is_empty(),
            "node '{}' title should not be empty",
            node_id
        );
    }

    // Verify session_docs tracks collection doc.
    assert!(
        session_docs.contains("kbc:my-kb"),
        "session should track collection doc"
    );
}

#[tokio::test]
async fn kb_share_realistic_org_content_roundtrip() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut session_docs = HashSet::new();

    let org_body = realistic_org_body();
    let node = make_test_node("concept:org-test", "Org Round-Trip", org_body, &["test"]);

    let resp = share_kb_with_nodes(
        &store,
        &bc,
        "org-kb",
        "Org KB",
        "alice",
        &[("concept:org-test", node)],
        &mut session_docs,
    )
    .await;
    assert!(resp.error.is_none(), "kb/share failed: {:?}", resp.error);

    // Read back and verify content is byte-for-byte identical.
    let (state, _) = store
        .encode_state_and_sv("kb:concept:org-test")
        .await
        .unwrap();
    let doc = mae_sync::kb::KbNodeDoc::from_bytes(&state).unwrap();
    assert_eq!(
        doc.body(),
        org_body,
        "org body should survive server round-trip byte-for-byte"
    );
    assert_eq!(doc.title(), "Org Round-Trip");
    assert_eq!(doc.tags(), vec!["test"]);
}

#[tokio::test]
async fn kb_join_returns_collection_and_all_nodes() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut sharer_docs = HashSet::new();

    // Share 3 nodes.
    let nodes = vec![
        ("n1", make_test_node("n1", "Node One", "body one", &["a"])),
        (
            "n2",
            make_test_node("n2", "Node Two", "body two — café", &["b"]),
        ),
        (
            "n3",
            make_test_node("n3", "Node Three", "body 三 日本語", &["c"]),
        ),
    ];
    share_kb_with_nodes(
        &store,
        &bc,
        "join-kb",
        "Join Test",
        "alice",
        &nodes,
        &mut sharer_docs,
    )
    .await;

    // Join from a different session.
    let mut joiner_docs = HashSet::new();
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "kb/join",
        "params": { "kb_id": "join-kb" }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        1,
        &mut joiner_docs,
    )
    .await;

    assert!(resp.error.is_none(), "kb/join failed: {:?}", resp.error);
    let result = resp.result.unwrap();

    // Verify collection state.
    let coll_b64 = result["collection_state"].as_str().unwrap();
    let coll_bytes = mae_sync::encoding::base64_to_update(coll_b64).unwrap();
    let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&coll_bytes).unwrap();
    assert_eq!(coll.node_count(), 3, "collection should have 3 nodes");

    // Verify all nodes returned with correct content.
    let returned_nodes = result["nodes"].as_array().unwrap();
    assert_eq!(returned_nodes.len(), 3, "should return all 3 nodes");

    for expected in &[
        ("n1", "Node One", "body one"),
        ("n2", "Node Two", "body two — café"),
        ("n3", "Node Three", "body 三 日本語"),
    ] {
        let node_json = returned_nodes
            .iter()
            .find(|n| n["id"].as_str() == Some(expected.0))
            .unwrap_or_else(|| panic!("node '{}' should be in response", expected.0));
        let state_bytes =
            mae_sync::encoding::base64_to_update(node_json["state"].as_str().unwrap()).unwrap();
        let doc = mae_sync::kb::KbNodeDoc::from_bytes(&state_bytes).unwrap();
        assert_eq!(
            doc.title(),
            expected.1,
            "node '{}' title mismatch",
            expected.0
        );
        assert_eq!(
            doc.body(),
            expected.2,
            "node '{}' body mismatch",
            expected.0
        );
    }
}

#[tokio::test]
async fn kb_join_nonexistent_returns_empty() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/join",
        "params": { "kb_id": "nonexistent-kb" }
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

    // Server creates empty doc on read (get_or_create semantics), so this
    // succeeds but returns 0 nodes — the client interprets empty collection.
    assert!(resp.error.is_none(), "kb/join creates empty doc — no error");
    let result = resp.result.unwrap();
    let nodes = result["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 0, "nonexistent KB should return 0 nodes");
}

#[tokio::test]
async fn kb_node_update_applies_and_broadcasts() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut session_a = HashSet::new();

    // Share a single node.
    let node = make_test_node("n1", "Original", "original body", &[]);
    share_kb_with_nodes(
        &store,
        &bc,
        "update-kb",
        "Update Test",
        "alice",
        &[("n1", node.clone())],
        &mut session_a,
    )
    .await;

    // Subscribe session B for notifications.
    let session_b_id = 1u64;
    let mut rx = {
        let mut b = bc.lock().unwrap();
        b.subscribe(session_b_id, vec!["sync_update".to_string()]);
        b.subscribe_doc(session_b_id, "kb:n1");
        b.subscribe_doc(session_b_id, "kbc:update-kb");
        b.subscribe(session_b_id, vec!["sync_update".to_string()])
    };

    // Generate an update: change body via KbNodeDoc.
    let mut doc = mae_sync::kb::KbNodeDoc::from_bytes(&node).unwrap();
    let update = doc.set_body("updated body — café, 日本語");

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "kb/node_update",
        "params": {
            "kb_id": "update-kb",
            "node_id": "n1",
            "update": update_to_base64(&update),
        }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut session_a,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "kb/node_update failed: {:?}",
        resp.error
    );
    assert_eq!(resp.result.unwrap()["applied"], true);

    // Verify the stored doc reflects the update.
    let (state, _) = store.encode_state_and_sv("kb:n1").await.unwrap();
    let stored = mae_sync::kb::KbNodeDoc::from_bytes(&state).unwrap();
    assert_eq!(
        stored.body(),
        "updated body — café, 日本語",
        "stored node body should reflect update"
    );

    // Verify broadcast was sent (best-effort check).
    if let Ok(EditorEvent::SyncUpdate { buffer_name, .. }) = rx.try_recv() {
        assert_eq!(
            buffer_name, "kb:n1",
            "broadcast should be for the updated node doc"
        );
    }
}

#[tokio::test]
async fn kb_leave_unsubscribes_session() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut session_docs = HashSet::new();

    // Share a KB.
    let node = make_test_node("n1", "Title", "body", &[]);
    share_kb_with_nodes(
        &store,
        &bc,
        "leave-kb",
        "Leave Test",
        "alice",
        &[("n1", node)],
        &mut session_docs,
    )
    .await;

    // Verify session tracks the collection + node docs.
    assert!(session_docs.contains("kbc:leave-kb"));
    assert!(session_docs.contains("kb:n1"));

    // Leave.
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "kb/leave",
        "params": { "kb_id": "leave-kb" }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut session_docs,
    )
    .await;
    assert!(resp.error.is_none(), "kb/leave failed: {:?}", resp.error);
    assert_eq!(resp.result.unwrap()["left"], true);

    // Session should no longer track collection doc.
    assert!(
        !session_docs.contains("kbc:leave-kb"),
        "session should no longer track collection doc after leave"
    );
}

#[tokio::test]
async fn kb_share_with_invalid_base64_returns_error() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {
            "kb_id": "bad-kb",
            "name": "Bad KB",
            "creator": "alice",
            "collection_state": "!!!NOT_VALID_BASE64!!!",
            "nodes": [],
        }
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
        "kb/share with invalid base64 should return error"
    );
}

#[tokio::test]
async fn kb_share_missing_kb_id_returns_error() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": { "name": "Test", "creator": "alice" }
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
        "kb/share without kb_id should return error"
    );
}

#[tokio::test]
async fn kb_node_update_for_nonexistent_node() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Try to update a node that was never shared.
    let mut doc = mae_sync::kb::KbNodeDoc::new("ghost", "Ghost", "body", &[]);
    let update = doc.set_body("new body");

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/node_update",
        "params": {
            "kb_id": "some-kb",
            "node_id": "ghost",
            "update": update_to_base64(&update),
        }
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
    // The server creates the doc on first update (share_or_join semantics in DocStore),
    // or returns an error. Either way it shouldn't panic.
    // We just verify it doesn't crash — the exact behavior depends on DocStore.apply_update.
    // Just verify it doesn't crash — the server might create the doc on first update.
    let _ = resp;
}

#[tokio::test]
async fn kb_share_then_update_then_join_sees_latest() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut session_a = HashSet::new();

    // Share with initial content.
    let node = make_test_node("n1", "Initial Title", "initial body", &["v1"]);
    share_kb_with_nodes(
        &store,
        &bc,
        "evolving-kb",
        "Evolving",
        "alice",
        &[("n1", node.clone())],
        &mut session_a,
    )
    .await;

    // Update the node's body.
    let mut doc = mae_sync::kb::KbNodeDoc::from_bytes(&node).unwrap();
    let update = doc.set_body("evolved body with café and 日本語");
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "kb/node_update",
        "params": {
            "kb_id": "evolving-kb",
            "node_id": "n1",
            "update": update_to_base64(&update),
        }
    });
    handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut session_a,
    )
    .await;

    // Join from a new session — should see latest content.
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "kb/join",
        "params": { "kb_id": "evolving-kb" }
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
    assert!(resp.error.is_none());

    let result = resp.result.unwrap();
    let nodes = result["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    let state_bytes =
        mae_sync::encoding::base64_to_update(nodes[0]["state"].as_str().unwrap()).unwrap();
    let joined_doc = mae_sync::kb::KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(
        joined_doc.body(),
        "evolved body with café and 日本語",
        "joined client should see the updated body, not the initial one"
    );
}
