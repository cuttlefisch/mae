use super::*;
use crate::storage::SqliteBackend;
use mae_mcp::broadcast::EventBroadcaster;
use mae_sync::encoding::update_to_base64;
use mae_sync::text::TextSync;
use tokio::io::BufReader;

fn test_broadcaster() -> SharedBroadcaster {
    Arc::new(std::sync::Mutex::new(EventBroadcaster::new()))
}

fn test_doc_store() -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500))
}

#[tokio::test]
async fn handle_doc_sync_update_and_read() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Generate a real yrs update.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "hello");
    let update_b64 = update_to_base64(&update);

    // sync/update
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/update",
        "params": { "doc": "test", "update": update_b64 }
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
    assert!(resp.error.is_none(), "sync/update failed: {:?}", resp.error);
    assert!(resp.result.unwrap()["wal_seq"].as_u64().unwrap() > 0);

    // docs/content
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "docs/content",
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
    assert_eq!(resp.result.unwrap()["content"], "hello");
}

/// #169 M1 — the attacker's test. A `kb:{node}` write smuggled in via `sync/update`
/// WITHOUT a `kb_id` must be REJECTED: it would otherwise skip `verify_relayed_content_op`
/// (gated on `kb_id`), `kb_access`, AND the epoch fence, then apply + broadcast. The
/// selective control: a plain (non-`kb:`) buffer is unaffected.
#[tokio::test]
async fn sync_update_to_kb_doc_without_kb_id_is_rejected() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "smuggled-node-write");
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/update",
        "params": { "doc": "kb:concept:smuggle", "update": update_to_base64(&update) }
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
        "a kb: sync/update with no kb_id MUST be rejected (#169 M1 bypass)"
    );
    assert!(
        resp.error.unwrap().message.contains("kb_id"),
        "the rejection cites the missing kb_id"
    );

    // SELECTIVE control: a non-kb (text buffer) doc without kb_id still applies — the gate
    // is specific to kb: docs, not a blanket sync/update break.
    let mut ts2 = TextSync::with_client_id("", 2);
    let upd2 = ts2.insert(0, "ok");
    let msg2 = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "sync/update",
        "params": { "doc": "plain-buffer", "update": update_to_base64(&upd2) }
    });
    let resp2 = handle_doc_request(
        &msg2.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut HashSet::new(),
    )
    .await;
    assert!(
        resp2.error.is_none(),
        "a non-kb doc is unaffected by the kb: gate: {:?}",
        resp2.error
    );
}

#[tokio::test]
async fn handle_doc_state_vector() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/state_vector",
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
    assert!(resp.error.is_none());
    let sv = resp.result.unwrap()["sv"].as_str().unwrap().to_string();
    assert!(!sv.is_empty());
}

#[tokio::test]
async fn handle_doc_full_state() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/full_state",
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
    assert!(resp.error.is_none());
}

#[tokio::test]
async fn handle_docs_list() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Create two docs.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "a");
    store.apply_update("alpha", &update, None).await.unwrap();
    store.apply_update("beta", &update, None).await.unwrap();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "docs/list"
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
    let docs = resp.result.unwrap()["documents"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(docs.len(), 2);
}

#[tokio::test]
async fn debug_method_returns_uptime_and_connections() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "$/debug"
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
    assert!(resp.error.is_none(), "$/debug failed: {:?}", resp.error);
    let result = resp.result.unwrap();
    assert!(
        result.get("uptime_secs").is_some(),
        "should include uptime_secs"
    );
    assert!(
        result.get("connection_count").is_some(),
        "should include connection_count"
    );
    assert!(result.get("version").is_some(), "should include version");
    // C3: the build SHA lets an editor's collab-doctor detect an
    // editor↔daemon build mismatch across machines.
    let build = result
        .get("build")
        .and_then(|v| v.as_str())
        .expect("$/debug should include the build SHA");
    assert!(!build.is_empty(), "build SHA must be populated");
    assert_eq!(build, crate::BUILD_SHA);
    assert!(
        result.get("documents").is_some(),
        "should include document count"
    );
    assert!(
        result.get("doc_stats").is_some(),
        "should include doc_stats"
    );
    // Uptime should be a small non-negative integer for a just-started server.
    assert!(result["uptime_secs"].as_u64().is_some());
    // No clients connected in this test.
    assert_eq!(result["connection_count"].as_u64().unwrap(), 0);
}

#[tokio::test]
async fn full_client_session_over_pipe() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Create an in-memory duplex stream.
    let (client_stream, server_stream) = tokio::io::duplex(4096);

    let (server_read, server_write) = tokio::io::split(server_stream);
    let server_reader = BufReader::new(server_read);

    // Spawn handler.
    let store_clone = Arc::clone(&store);
    let bc_clone = Arc::clone(&bc);
    tokio::spawn(async move {
        handle_client(
            server_reader,
            server_write,
            store_clone,
            bc_clone,
            std::time::Instant::now(),
            Transport::Hub,
        )
        .await;
    });

    // Client side.
    let (client_read, mut client_write) = tokio::io::split(client_stream);
    let mut client_reader = BufReader::new(client_read);

    // Send initialize.
    let init_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"clientInfo": {"name": "test-pipe"}}
    });
    let payload = format!("{}\n", serde_json::to_string(&init_msg).unwrap());
    tokio::io::AsyncWriteExt::write_all(&mut client_write, payload.as_bytes())
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut client_write)
        .await
        .unwrap();

    // Read response.
    let resp_msg = mae_mcp::read_message(&mut client_reader)
        .await
        .unwrap()
        .unwrap();
    let resp: JsonRpcResponse = serde_json::from_str(&resp_msg).unwrap();
    assert!(resp.error.is_none(), "initialize failed: {:?}", resp.error);
    assert_eq!(resp.result.unwrap()["serverInfo"]["name"], "mae-editor");

    // Ping.
    let ping_msg = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"});
    let payload = format!("{}\n", serde_json::to_string(&ping_msg).unwrap());
    tokio::io::AsyncWriteExt::write_all(&mut client_write, payload.as_bytes())
        .await
        .unwrap();
    tokio::io::AsyncWriteExt::flush(&mut client_write)
        .await
        .unwrap();

    let resp_msg = mae_mcp::read_message(&mut client_reader)
        .await
        .unwrap()
        .unwrap();
    let resp: JsonRpcResponse = serde_json::from_str(&resp_msg).unwrap();
    assert_eq!(resp.result.unwrap(), "pong");
}

#[tokio::test]
async fn resync_tracks_session_doc() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut session_docs = HashSet::new();

    // First create the doc via sync/update.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "resync test");
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/update",
        "params": { "doc": "resync-doc", "update": update_to_base64(&update) }
    });
    handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut session_docs,
    )
    .await;

    // Clear session_docs to simulate a fresh session.
    session_docs.clear();

    // sync/resync should track the doc in session_docs.
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "sync/resync",
        "params": { "doc": "resync-doc" }
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
    assert!(resp.error.is_none(), "resync failed: {:?}", resp.error);
    assert!(
        session_docs.contains("resync-doc"),
        "resync must track doc in session_docs"
    );
}

#[tokio::test]
async fn resync_increments_connected_clients() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut session_docs = HashSet::new();

    // Create doc.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "hello");
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "sync/update",
        "params": { "doc": "cc-doc", "update": update_to_base64(&update) }
    });
    handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut session_docs,
    )
    .await;

    // Resync from a different session.
    let mut session2 = HashSet::new();
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "sync/resync",
        "params": { "doc": "cc-doc" }
    });
    handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        1,
        &mut session2,
    )
    .await;

    // Check doc_stats — connected_clients should be at least 1.
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 3, "method": "docs/stats",
        "params": { "doc": "cc-doc" }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        1,
        &mut session2,
    )
    .await;
    let stats = &resp.result.unwrap()["stats"];
    assert!(
        stats["connected_clients"].as_u64().unwrap() >= 1,
        "resync must increment connected_clients, got: {stats}"
    );
}

/// ADR-020 B-12: an owner reconnect/re-share must PRESERVE the daemon's
/// authoritative collection membership, not clobber it. `share_doc` was
/// destructive (delete+replace), so re-sharing the owner-only collection
/// silently revoked every approved member on each owner restart — unacceptable
/// for a trusted-peer system. The fix preserves an existing collection.
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

/// Phase D1.1 (ADR-029): `kb/collection_node_add`/`_remove` mutate the collection
/// manifest (`kbc:`) so the projector materializes a created node / drops a deleted
/// one. The daemon computes the update server-side (mirrors `kb/add_member`).
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

    // Create a base64 string that decodes to > 1 MB.
    let big_data = vec![0u8; MAX_UPDATE_SIZE + 1];
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

// WU1: Notification handling tests

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

// --- KB protocol handler tests (Phase 0.5) ---

/// Helper: create a KbNodeDoc with realistic org content and return encoded bytes.
fn make_test_node(id: &str, title: &str, body: &str, tags: &[&str]) -> Vec<u8> {
    use mae_sync::kb::KbNodeDoc;
    let node = KbNodeDoc::new(
        id,
        title,
        body,
        &tags.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    );
    node.encode()
}

/// Realistic org content for testing (properties drawer, links, code block, Unicode).
fn realistic_org_body() -> &'static str {
    ":PROPERTIES:\n:ID: test-node-001\n:ROAM_REFS: https://example.com\n:END:\n\
         #+TITLE: Test Node — CRDT Round-Trip\n#+FILETAGS: :research:crdt:\n\n\
         * Overview\n\
         This node tests the full round-trip: SQLite → KbNodeDoc → base64 → server → base64 → KbNodeDoc → SQLite.\n\n\
         ** Sub-heading with [[id:other-node][internal link]]\n\
         Content with Unicode: café, naïve, 日本語\n\n\
         #+begin_src rust\nfn main() { println!(\"hello\"); }\n#+end_src\n"
}

// --- ADR-018 access-control test harness (principals, not labels) ---

/// A peer's principal (fake key fingerprint) from a label.
fn fp(label: &str) -> String {
    format!("SHA256:{label}")
}

/// Share a KB authenticated as `auth_principal` (key fingerprint) with display
/// `auth_label`. The daemon stamps the owner from the principal; any claimed
/// `creator` is ignored.
async fn kb_share_as(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    kb_id: &str,
    claimed_creator: &str,
    session_docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    let coll = KbCollectionDoc::new_owned(kb_id, "", auth_label.unwrap_or(""));
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {
            "kb_id": kb_id,
            "name": kb_id,
            "creator": claimed_creator,
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": [],
        }
    });
    handle_doc_request_inner(
        &msg.to_string(),
        store,
        bc,
        std::time::Instant::now(),
        0,
        auth_label,
        auth_principal,
        None,
        session_docs,
        Transport::Hub,
    )
    .await
}

/// Dispatch an arbitrary doc request as a peer (label + principal).
async fn dispatch_as(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    msg: serde_json::Value,
    docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    handle_doc_request_inner(
        &msg.to_string(),
        store,
        bc,
        std::time::Instant::now(),
        0,
        auth_label,
        auth_principal,
        None,
        docs,
        Transport::Hub,
    )
    .await
}

async fn load_coll(store: &Arc<DocStore>, kb_id: &str) -> KbCollectionDoc {
    let (state, _) = store
        .encode_state_and_sv(&format!("kbc:{kb_id}"))
        .await
        .expect("collection exists");
    KbCollectionDoc::from_bytes(&state).expect("valid collection")
}

fn kb_join_msg(kb_id: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/join","params":{"kb_id":kb_id}})
}
fn kb_node_update_msg(kb_id: &str) -> serde_json::Value {
    let mut ts = TextSync::with_client_id("", 7);
    let upd = ts.insert(0, "x");
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":"concept:n","update":update_to_base64(&upd)}})
}

/// ADR-023: a node edit authored under the sender's CURRENT-epoch KB client_id
/// `derive_kb_client_id(principal, epoch)` — what the editor's `kb_client_id_for`
/// produces and what the daemon's epoch fence accepts. `text` lets a test vary
/// the op so a re-authored edit is distinguishable from a stale one.
fn kb_node_update_msg_as(
    kb_id: &str,
    principal: &str,
    epoch: u64,
    text: &str,
) -> serde_json::Value {
    let cid = derive_kb_client_id(principal, epoch);
    let mut ts = TextSync::with_client_id("", cid);
    let upd = ts.insert(0, text);
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":"concept:n","update":update_to_base64(&upd)}})
}
/// member is a PRINCIPAL (fingerprint); optional role.
fn kb_member_msg(method: &str, kb_id: &str, member: &str, role: Option<&str>) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,
            "params":{"kb_id":kb_id,"member":member,"role":role,"label":member}})
}
fn kb_policy_msg(kb_id: &str, policy: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/set_policy",
            "params":{"kb_id":kb_id,"policy":policy}})
}
fn kb_approve_msg(kb_id: &str, principal: &str, role: Option<&str>) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/approve_member",
            "params":{"kb_id":kb_id,"principal":principal,"role":role}})
}

#[tokio::test]
async fn share_ignores_claimed_creator_and_binds_owner_to_principal() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    // Authenticated principal = alice's key; claims creator "mallory" → SUCCEEDS,
    // owner bound to the principal (the I-7 reject is gone; the claim is ignored).
    let resp = kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb1",
        "mallory",
        &mut docs,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "claimed creator must be ignored, not rejected: {:?}",
        resp.error
    );
    let coll = load_coll(&store, "kb1").await;
    assert_eq!(coll.owner(), fp("alice"), "owner = verified principal");
    assert_eq!(coll.role_of(&fp("alice")), Some(SyncRole::Owner));
    assert_eq!(
        coll.role_of(&fp("mallory")),
        None,
        "spoofed name is not a member"
    );
}

#[tokio::test]
async fn anonymous_share_succeeds() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let resp = kb_share_as(&store, &bc, None, None, "kb3", "whoever", &mut docs).await;
    assert!(
        resp.error.is_none(),
        "anonymous (none) share must succeed: {:?}",
        resp.error
    );
}

#[tokio::test]
async fn restrictive_nonmember_join_denied() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbr",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_policy_msg("kbr", "restrictive"),
        &mut docs,
    )
    .await;
    let denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbr"),
        &mut docs,
    )
    .await;
    assert!(
        denied.error.is_some(),
        "restrictive: non-member join denied"
    );
    assert!(denied.error.unwrap().message.contains("not a member"));
}

#[tokio::test]
async fn invite_nonmember_join_pending() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    // default policy = invite
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbi",
        "alice",
        &mut docs,
    )
    .await;
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbi"),
        &mut docs,
    )
    .await;
    assert!(
        resp.error.is_none(),
        "invite join returns success+pending, not error"
    );
    assert_eq!(
        resp.result.as_ref().and_then(|r| r["status"].as_str()),
        Some("pending")
    );
    let coll = load_coll(&store, "kbi").await;
    assert_eq!(coll.pending().len(), 1, "join recorded as pending");
    assert_eq!(
        coll.role_of(&fp("bob")),
        None,
        "pending peer is not yet a member"
    );
}

#[tokio::test]
async fn permissive_nonmember_join_autoadds_viewer() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbp",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_policy_msg("kbp", "permissive"),
        &mut docs,
    )
    .await;
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbp"),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "permissive join succeeds");
    let coll = load_coll(&store, "kbp").await;
    assert_eq!(
        coll.role_of(&fp("bob")),
        Some(SyncRole::Viewer),
        "auto-granted least privilege"
    );
}

#[tokio::test]
async fn owner_add_member_then_join_and_edit() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbm2",
        "alice",
        &mut docs,
    )
    .await;
    // bob denied edit before being added.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg("kbm2"),
        &mut docs
    )
    .await
    .error
    .is_some());
    // owner adds bob (default editor).
    assert!(dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbm2", &fp("bob"), None),
        &mut docs
    )
    .await
    .error
    .is_none());
    // bob now joins (member) + edits.
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kbm2"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "member joins directly"
    );
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            // bob is a freshly-added editor ⇒ epoch 0; he authors under his
            // current-epoch client_id, which the ADR-023 fence accepts.
            kb_node_update_msg_as("kbm2", &fp("bob"), 0, "x"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "editor may edit"
    );
    // owner removes bob → next edit denied.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/remove_member", "kbm2", &fp("bob"), None),
        &mut docs
    )
    .await
    .error
    .is_none());
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg("kbm2"),
            &mut docs
        )
        .await
        .error
        .is_some(),
        "removed member denied"
    );
}

#[tokio::test]
async fn viewer_cannot_node_update() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbv",
        "alice",
        &mut docs,
    )
    .await;
    // add bob as VIEWER
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbv", &fp("bob"), Some("viewer")),
        &mut docs,
    )
    .await;
    // viewer may join/read but not edit.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbv"),
        &mut docs
    )
    .await
    .error
    .is_none());
    let denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_node_update_msg("kbv"),
        &mut docs,
    )
    .await;
    assert!(
        denied.error.is_some(),
        "viewer must not edit (least privilege)"
    );
}

/// ADR-023 (B-19) — the deferred-privilege-escalation exploit, end to end at the
/// daemon. A viewer authors edits locally under their viewer-epoch client_id while
/// DENIED at the daemon; once later granted editor, those pre-grant edits must NOT
/// cascade. The epoch fence rejects the stale lineage (`"rebase required"`); only a
/// fresh, current-epoch edit is accepted. (Red before the fence, green after.)
#[tokio::test]
async fn viewer_era_edits_do_not_cascade_on_grant() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbx",
        "alice",
        &mut docs,
    )
    .await;

    // bob is added as a VIEWER — a fresh grant ⇒ epoch 0.
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("viewer")),
        &mut docs,
    )
    .await;

    // bob (viewer) authors an edit under his epoch-0 client_id and pushes it —
    // DENIED at the role gate. He keeps it in his local lineage (the cascade seed).
    let viewer_era = kb_node_update_msg_as("kbx", &fp("bob"), 0, "VIEWER-ERA");
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            viewer_era.clone(),
            &mut docs
        )
        .await
        .error
        .is_some(),
        "viewer edit denied at the role gate"
    );

    // Owner PROMOTES bob viewer→editor — a role change ⇒ bob's epoch bumps to 1.
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("editor")),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "owner promotes bob to editor"
    );

    // THE EXPLOIT: bob re-pushes his VIEWER-ERA op (still authored under epoch 0).
    // The role gate now passes (he is an editor), but the EPOCH FENCE must reject
    // it — the op is from his stale, pre-grant client_id.
    //
    // Strong no-cascade oracle: snapshot the canonical state BEFORE the fenced push
    // and assert it is BYTE-IDENTICAL after — a fenced op must perturb the
    // authoritative node by exactly zero bytes (stronger than a substring check).
    let (before, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        viewer_era.clone(),
        &mut docs,
    )
    .await;
    let msg = resp
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        resp.error.is_some() && msg.contains("rebase required"),
        "viewer-era lineage must be fenced on grant; got: {msg:?}"
    );

    // NO CASCADE: the canonical state is byte-identical (and, redundantly, never
    // contains the viewer-era edit).
    let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    assert_eq!(
        state, before,
        "a fenced op must leave the canonical node byte-identical (no cascade)"
    );
    let canonical = TextSync::from_state(&state).unwrap().content();
    assert!(
        !canonical.contains("VIEWER-ERA"),
        "pre-grant edit must not cascade; canonical = {canonical:?}"
    );

    // bob CAN make a fresh, current-epoch edit — that is accepted. Post-promotion
    // his epoch is an unpredictable token (#72), so read it from the collection
    // rather than assuming the old prev+1 value.
    let (kbc_state, _) = store.encode_state_and_sv("kbc:kbx").await.unwrap();
    let bob_epoch = KbCollectionDoc::from_bytes(&kbc_state)
        .unwrap()
        .epoch_of(&fp("bob"));
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg_as("kbx", &fp("bob"), bob_epoch, "FRESH"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "a fresh current-epoch edit is accepted"
    );
    let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    assert!(
        TextSync::from_state(&state)
            .unwrap()
            .content()
            .contains("FRESH"),
        "fresh current-epoch edit is applied"
    );

    // MALICIOUS-CLIENT VARIANT: re-sending the divergent op stays rejected (its
    // new ops are still from the stale-epoch client_id, never C_now).
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            viewer_era,
            &mut docs
        )
        .await
        .error
        .is_some(),
        "re-sent stale-epoch op stays fenced"
    );
}

/// B-20 regression (the live 9c cascade): a stale-epoch op that is a
/// *contiguous-clock continuation* of a client already canonical in the node
/// must still be fenced. Distinct from B-19's fresh-lineage case — here bob has
/// a PRIOR ACCEPTED edit, so his client is in the base; the pre-fix fence (which
/// keyed on the incoming update's own state vector) missed the continuation and
/// let it cascade.
#[tokio::test]
async fn stale_epoch_continuation_of_canonical_client_is_fenced() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbx",
        "alice",
        &mut docs,
    )
    .await;

    // bob added directly as EDITOR (fresh grant ⇒ epoch 0) and makes an ACCEPTED
    // edit, so his epoch-0 client becomes part of the node's canonical lineage.
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg_as("kbx", &fp("bob"), 0, "ACCEPTED-EDIT"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "bob's epoch-0 edit is accepted and becomes canonical"
    );

    // Owner DEMOTES bob → viewer (epoch 1) then RE-PROMOTES → editor (epoch 2).
    // bob's editor never rotated off the epoch-0 client (no rejoin), mirroring 9c.
    for role in ["viewer", "editor"] {
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("alice"),
                Some(&fp("alice")),
                kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some(role)),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "owner role change to {role} applies"
        );
    }

    // THE EXPLOIT: bob authors a CONTINUATION under his now-stale epoch-0 client,
    // chained onto the canonical state (not a fresh lineage). Role gate passes
    // (he is an editor); the epoch fence must still reject it.
    let (canonical_state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    let cid0 = derive_kb_client_id(&fp("bob"), 0);
    let mut ts = TextSync::from_state_with_client_id(&canonical_state, cid0).unwrap();
    let cont_update = ts.insert(0, "VIEWER-ERA-CONT ");
    let cont_msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":"kbx","node_id":"concept:n","update":update_to_base64(&cont_update)}});
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        cont_msg,
        &mut docs,
    )
    .await;
    let msg = resp
        .error
        .as_ref()
        .map(|e| e.message.clone())
        .unwrap_or_default();
    assert!(
        resp.error.is_some() && msg.contains("rebase required"),
        "stale-epoch continuation must be fenced (B-20); got: {msg:?}"
    );

    // NO CASCADE: the canonical state is byte-identical to before the fenced push
    // (`canonical_state` was captured just above to build the continuation), and
    // never gains the viewer-interval edit.
    let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    assert_eq!(
        state, canonical_state,
        "a fenced continuation must leave the canonical node byte-identical (no cascade)"
    );
    let canonical = TextSync::from_state(&state).unwrap().content();
    assert!(
        !canonical.contains("VIEWER-ERA-CONT"),
        "stale continuation must not cascade; canonical = {canonical:?}"
    );

    // bob CAN still converge by re-authoring under his CURRENT epoch — now an
    // unpredictable token (#72), read from the collection rather than assumed.
    let (kbc_state, _) = store.encode_state_and_sv("kbc:kbx").await.unwrap();
    let bob_epoch = KbCollectionDoc::from_bytes(&kbc_state)
        .unwrap()
        .epoch_of(&fp("bob"));
    assert!(
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg_as("kbx", &fp("bob"), bob_epoch, "REAUTHORED"),
            &mut docs
        )
        .await
        .error
        .is_none(),
        "a fresh current-epoch edit is accepted"
    );
}

/// ADR-024 R1: `kb/node_fetch` returns a node's authoritative state+sv to a
/// member (for adopt-and-re-author) and denies a non-member.
#[tokio::test]
async fn kb_node_fetch_serves_members_denies_nonmembers() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbnf",
        "alice",
        &mut docs,
    )
    .await;
    let fetch = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/node_fetch",
            "params":{"kb_id":"kbnf","node_id":"concept:n"}});

    // Owner (a member) gets state + sv.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        fetch.clone(),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "owner fetch ok: {:?}", resp.error);
    let result = resp.result.expect("result present");
    assert!(result.get("state").and_then(|v| v.as_str()).is_some());
    assert!(result.get("sv").and_then(|v| v.as_str()).is_some());

    // A non-member is denied.
    let denied = dispatch_as(
        &store,
        &bc,
        Some("carol"),
        Some(&fp("carol")),
        fetch,
        &mut docs,
    )
    .await;
    assert!(denied.error.is_some(), "non-member fetch must be denied");
}

#[tokio::test]
async fn only_owner_manages_members() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbm3",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "kbm3", &fp("bob"), None),
        &mut docs,
    )
    .await;
    // bob (editor, non-owner) cannot add carol.
    let denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_member_msg("kb/add_member", "kbm3", &fp("carol"), None),
        &mut docs,
    )
    .await;
    assert!(denied.error.is_some(), "non-owner must not manage members");

    // bob (editor, non-owner) likewise cannot change the join policy — the
    // same owner-only Manage gate (kb_access) covers set_policy.
    let policy_denied = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_policy_msg("kbm3", "permissive"),
        &mut docs,
    )
    .await;
    assert!(
        policy_denied.error.is_some(),
        "non-owner must not change the join policy"
    );
}

#[tokio::test]
async fn pending_then_approve_allows_join() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kba",
        "alice",
        &mut docs,
    )
    .await;
    // bob requests (invite default) → pending.
    dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kba"),
        &mut docs,
    )
    .await;
    // owner approves as editor.
    let ok = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_approve_msg("kba", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    assert!(ok.error.is_none(), "owner approve succeeds: {:?}", ok.error);
    let coll = load_coll(&store, "kba").await;
    assert!(coll.pending().is_empty(), "approval clears pending");
    assert_eq!(coll.role_of(&fp("bob")), Some(SyncRole::Editor));
    // bob now joins as a member.
    assert!(dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kba"),
        &mut docs
    )
    .await
    .error
    .is_none());
}

#[tokio::test]
async fn label_collision_two_keys_distinct_principals() {
    // Two peers share the same display label but have distinct principals — the
    // member added by one is NOT the other (no label-based impersonation).
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbc",
        "alice",
        &mut docs,
    )
    .await;
    // owner adds principal A under label "dupe".
    let a = "SHA256:keyA";
    let b = "SHA256:keyB";
    dispatch_as(&store, &bc, Some("alice"), Some(&fp("alice")),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/add_member","params":{"kb_id":"kbc","member":a,"role":"editor","label":"dupe"}}), &mut docs).await;
    let coll = load_coll(&store, "kbc").await;
    assert_eq!(coll.role_of(a), Some(SyncRole::Editor));
    assert_eq!(
        coll.role_of(b),
        None,
        "a different key with the same label is NOT a member"
    );
}

#[tokio::test]
async fn raw_collection_write_smuggling_denied() {
    // A non-owner cannot escalate by sending a raw `kbc:` sync/update that
    // grants itself ownership — the membership-smuggling defense.
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbs",
        "alice",
        &mut docs,
    )
    .await;
    let mut coll = load_coll(&store, "kbs").await;
    let evil = coll.upsert_member(&fp("bob"), "bob", SyncRole::Owner);
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
            "params":{"doc":"kbc:kbs","update":update_to_base64(&evil)}});
    let denied = dispatch_as(&store, &bc, Some("bob"), Some(&fp("bob")), msg, &mut docs).await;
    assert!(
        denied.error.is_some(),
        "non-owner raw collection write must be denied"
    );
    let after = load_coll(&store, "kbs").await;
    assert_eq!(
        after.role_of(&fp("bob")),
        None,
        "smuggled membership must not apply"
    );
}

#[tokio::test]
async fn none_mode_not_gated() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(&store, &bc, None, None, "kbn", "alice", &mut docs).await;
    assert!(
        dispatch_as(&store, &bc, None, None, kb_join_msg("kbn"), &mut docs)
            .await
            .error
            .is_none(),
        "none/loopback sessions are connection-trusted (dev only)"
    );
}

async fn share_kb_with_nodes(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    kb_id: &str,
    name: &str,
    creator: &str,
    nodes: &[(&str, Vec<u8>)],
    session_docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    use mae_sync::kb::KbCollectionDoc;

    let mut coll = KbCollectionDoc::new(name, creator);
    for (id, _) in nodes {
        coll.add_node(id, id); // title = id for simplicity
    }
    let collection_b64 = update_to_base64(&coll.encode_state());

    let nodes_json: Vec<serde_json::Value> = nodes
        .iter()
        .map(|(id, state)| serde_json::json!({ "id": id, "state": update_to_base64(state) }))
        .collect();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {
            "kb_id": kb_id,
            "name": name,
            "creator": creator,
            "collection_state": collection_b64,
            "nodes": nodes_json,
        }
    });
    handle_doc_request(
        &msg.to_string(),
        store,
        bc,
        std::time::Instant::now(),
        0,
        session_docs,
    )
    .await
}

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

#[tokio::test]
async fn transport_policy_gate() {
    use mae_sync::kb::TransportPolicy;

    // Share a fresh "kbx" collection with the given transport policy + bob as a
    // (non-owner) Editor member. `share_doc` replaces, so each call resets it.
    async fn share(store: &DocStore, owner: &str, member: &str, policy: TransportPolicy) {
        let mut coll = KbCollectionDoc::new_owned("KB", owner, "owner");
        coll.set_transport_policy(policy);
        coll.add_pending(member, "bob", "t0", None);
        coll.approve(member, SyncRole::Editor);
        store
            .share_doc("kbc:kbx", &coll.encode_state())
            .await
            .unwrap();
    }

    let store = test_doc_store();
    let owner = fp("owner");
    let bob = fp("bob");

    // --- p2p-only KB ---
    share(&store, &owner, &bob, TransportPolicy::P2p).await;
    // Owner over the hub: ALLOWED (owner bypass — the local editor reaches its own KB).
    assert_eq!(
        kb_access(&store, "kbx", Some(&owner), KbOp::Read, Transport::Hub)
            .await
            .unwrap(),
        AccessDecision::Allow
    );
    // Non-owner member over the hub: DENIED (not exposed on the hub).
    assert!(matches!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::Hub)
            .await
            .unwrap(),
        AccessDecision::Deny(_)
    ));
    // Non-owner member over the mesh: ALLOWED.
    assert_eq!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Allow
    );

    // --- hub-only KB ---
    share(&store, &owner, &bob, TransportPolicy::Hub).await;
    // Member over the mesh: DENIED (hub-only is not on the mesh).
    assert!(matches!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Edit, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Deny(_)
    ));
    // A non-member's JOIN over the mesh is transport-gated too (before join policy).
    let stranger = fp("stranger");
    assert!(matches!(
        kb_access(&store, "kbx", Some(&stranger), KbOp::Join, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Deny(_)
    ));

    // --- both ---
    share(&store, &owner, &bob, TransportPolicy::Both).await;
    assert_eq!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::Hub)
            .await
            .unwrap(),
        AccessDecision::Allow
    );
    assert_eq!(
        kb_access(&store, "kbx", Some(&bob), KbOp::Read, Transport::P2p)
            .await
            .unwrap(),
        AccessDecision::Allow
    );
}

#[tokio::test]
async fn kb_share_sets_and_widens_transport_policy() {
    use mae_sync::kb::TransportPolicy;

    fn share_msg(kb_id: &str, owner_label: &str, transport: &str) -> serde_json::Value {
        let coll = KbCollectionDoc::new_owned(kb_id, "", owner_label);
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": kb_id, "name": kb_id, "creator": owner_label,
                "collection_state": update_to_base64(&coll.encode_state()),
                "nodes": [], "transport": transport,
            }
        })
    }

    let store = test_doc_store();
    let bc = test_broadcaster();
    let owner = fp("owner");

    // First share over p2p ⇒ P2p-only (NOT widened with the conservative Hub default).
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        share_msg("kbx", "owner", "p2p"),
        &mut HashSet::new(),
    )
    .await;
    assert!(resp.error.is_none(), "kb/share p2p: {:?}", resp.error);
    assert_eq!(
        load_coll(&store, "kbx").await.transport_policy(),
        TransportPolicy::P2p
    );

    // The owner re-shares over hub ⇒ exposure WIDENS to Both (preserving p2p).
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        share_msg("kbx", "owner", "hub"),
        &mut HashSet::new(),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "kb/share hub re-share: {:?}",
        resp.error
    );
    assert_eq!(
        load_coll(&store, "kbx").await.transport_policy(),
        TransportPolicy::Both
    );

    // A KB shared with no transport param defaults to Hub-only.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner),
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": "kbhub", "name": "kbhub", "creator": "owner",
                "collection_state": update_to_base64(&KbCollectionDoc::new_owned("kbhub", "", "owner").encode_state()),
                "nodes": [],
            }
        }),
        &mut HashSet::new(),
    )
    .await;
    assert!(resp.error.is_none(), "kb/share default: {:?}", resp.error);
    assert_eq!(
        load_coll(&store, "kbhub").await.transport_policy(),
        TransportPolicy::Hub
    );
}

// --- ADR-026 signed membership op-log (slice 2b-6) ---

#[tokio::test]
async fn add_member_signs_oplog_for_owned_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::derive_valid_members;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    // The daemon's signing identity == the KB owner (the seam where the daemon
    // signs membership ops). Use a REAL key so fingerprints + signatures verify.
    let id = Identity::generate("daemon");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    // Owner shares the KB (authenticated as the signer's principal), then adds bob.
    let resp = kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbsig",
        "owner",
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "share: {:?}", resp.error);

    let bob = fp("bob");
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbsig", &bob, Some("editor")),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "add_member: {:?}", resp.error);

    // The op-log carries the genesis owner self-admit + the signed admit of bob;
    // every record verifies, and a peer derives owner+bob anchored on the owner key
    // — without trusting the relay (ADR-026).
    let coll = load_coll(&store, "kbsig").await;
    let ops = coll.oplog_ops();
    assert_eq!(ops.len(), 2, "genesis + admit");
    assert!(ops.iter().all(|o| o.verify_signed()), "all records verify");

    let members = derive_valid_members(&ops, &owner_pubkey, 0);
    assert_eq!(members.len(), 2);
    assert_eq!(members[&owner_fp].role, SyncRole::Owner);
    assert_eq!(members[&bob].role, SyncRole::Editor);
    assert_eq!(members[&bob].invited_by, owner_fp, "owner admitted bob");

    // A different anchor (a relay's forged collection) derives nothing.
    let stranger = Identity::generate("stranger").public().to_bytes();
    assert!(derive_valid_members(&ops, &stranger, 0).is_empty());

    // Removing bob appends a signed Remove; the peer no longer derives bob.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/remove_member", "kbsig", &bob, None),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "remove_member: {:?}", resp.error);
    let coll = load_coll(&store, "kbsig").await;
    let members = derive_valid_members(&coll.oplog_ops(), &owner_pubkey, 0);
    assert!(
        !members.contains_key(&bob),
        "bob removed in the derived set"
    );
    assert!(members.contains_key(&owner_fp));
}

#[tokio::test]
async fn add_member_unsigned_without_a_signer() {
    // No signer (psk/none mode) ⇒ the legacy member_roles path only; no op-log.
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let owner = fp("alice");
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&owner),
        "kbu",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&owner),
        kb_member_msg("kb/add_member", "kbu", &fp("bob"), Some("editor")),
        &mut docs,
    )
    .await;
    let coll = load_coll(&store, "kbu").await;
    assert_eq!(coll.oplog_len(), 0, "no signer ⇒ no signed op-log");
    assert_eq!(
        coll.role_of(&fp("bob")),
        Some(SyncRole::Editor),
        "legacy path"
    );
}

#[tokio::test]
async fn approve_member_signs_oplog_for_owned_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::derive_valid_members;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let id = Identity::generate("daemon");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbap",
        "owner",
        &mut docs,
    )
    .await;
    // bob requests to join (invite default ⇒ pending), owner approves as editor.
    let bob = fp("bob");
    dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob),
        kb_join_msg("kbap"),
        &mut docs,
    )
    .await;
    let ok = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_approve_msg("kbap", &bob, Some("editor")),
        &mut docs,
    )
    .await;
    assert!(ok.error.is_none(), "approve: {:?}", ok.error);

    // The approval is a signed Admit: a peer derives owner + the approved member.
    let coll = load_coll(&store, "kbap").await;
    let members = derive_valid_members(&coll.oplog_ops(), &owner_pubkey, 0);
    assert_eq!(members.len(), 2);
    assert_eq!(members[&bob].role, SyncRole::Editor);
    assert_eq!(members[&bob].invited_by, owner_fp);
}

#[tokio::test]
async fn kb_access_derives_from_oplog_when_anchored() {
    use mae_mcp::identity::Identity;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let id = Identity::generate("daemon");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    // Owner shares + adds bob (editor) ⇒ a signed op-log.
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbanc",
        "owner",
        &mut docs,
    )
    .await;
    let bob = fp("bob");
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbanc", &bob, Some("editor")),
        &mut docs,
    )
    .await;

    // Register the external anchor (what the dialer does for a JOINED KB), so
    // kb_access derives membership from the signed op-log, not member_roles.
    store.set_kb_anchor("kbanc", owner_pubkey).await;

    let access = |p: String, op: KbOp| {
        let store = Arc::clone(&store);
        async move { kb_access(&store, "kbanc", Some(&p), op, Transport::Hub).await }
    };
    assert!(
        matches!(
            access(owner_fp.clone(), KbOp::Manage).await,
            Ok(AccessDecision::Allow)
        ),
        "owner derives Manage from the op-log"
    );
    assert!(
        matches!(
            access(bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "bob (editor) derives Edit"
    );
    assert!(
        matches!(
            access(bob.clone(), KbOp::Manage).await,
            Ok(AccessDecision::Deny(_))
        ),
        "editor may not Manage"
    );
    assert!(
        matches!(
            access(fp("carol"), KbOp::Edit).await,
            Ok(AccessDecision::Deny(_))
        ),
        "non-member denied"
    );

    // Remove bob (signed Remove); the derived gate now denies him.
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/remove_member", "kbanc", &bob, None),
        &mut docs,
    )
    .await;
    assert!(
        matches!(
            access(bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Deny(_))
        ),
        "removed member denied via derived membership"
    );
}

// --- ADR-026 §A4 quorum governance in the daemon gate (#132) ---

fn kb_set_governance_msg(kb_id: &str, governance: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/set_governance",
            "params":{"kb_id":kb_id,"governance":governance}})
}
fn kb_revoke_msg(kb_id: &str, member: &str) -> serde_json::Value {
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/revoke",
            "params":{"kb_id":kb_id,"member":member}})
}

/// The owner records a `SetGovernance` op; the log derives the new rule, and a bad
/// spec is rejected. Governance is owner-signed + hash-chained like any op, so every
/// peer reads the identical rule.
#[tokio::test]
async fn set_governance_signs_oplog_and_derive_reads_quorum() {
    use mae_mcp::identity::Identity;
    use mae_sync::membership::{derive_governance, Governance};

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let id = Identity::generate("owner");
    let owner_fp = id.fingerprint();
    let owner_pubkey = id.public().to_bytes();
    store.set_signer(Arc::new(id));

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbg",
        "owner",
        &mut docs,
    )
    .await;

    // The default (no SetGovernance op) is single-owner.
    let coll = load_coll(&store, "kbg").await;
    assert_eq!(
        derive_governance(&coll.oplog_ops(), &owner_pubkey),
        Governance::SingleOwner
    );

    // Owner sets quorum:2; the signed log now derives Quorum{2}.
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_set_governance_msg("kbg", "quorum:2"),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "set_governance: {:?}", resp.error);
    let coll = load_coll(&store, "kbg").await;
    assert_eq!(
        derive_governance(&coll.oplog_ops(), &owner_pubkey),
        Governance::Quorum { threshold: 2 }
    );

    // A meaningless threshold is rejected at the RPC boundary.
    let bad = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_set_governance_msg("kbg", "quorum:0"),
        &mut docs,
    )
    .await;
    assert!(bad.error.is_some(), "quorum:0 is rejected");
}

/// The m-of-n oracle across TWO real daemons (distinct DocStores, each with its own
/// `OnceCell` signer — the production identity model). owner1's daemon authors one
/// revoke; the signed collection state syncs to owner2's daemon (modeled by applying
/// the exported doc update), where owner2's daemon co-signs a second. Under Quorum{2}
/// the first revoke is below threshold (bob keeps access on BOTH daemons) and only
/// the second distinct-owner co-signature removes him — the gate on owner2's daemon
/// deriving exactly what every honest peer derives from the merged log.
#[tokio::test]
async fn quorum_requires_two_distinct_owners_to_revoke() {
    use mae_mcp::identity::Identity;

    // Daemon A (owner1, genesis) and daemon B (owner2). bob never signs, so a
    // synthetic fingerprint suffices for the subject.
    let store_a = test_doc_store();
    let store_b = test_doc_store();
    let bc = test_broadcaster();
    let mut docs_a = HashSet::new();
    let mut docs_b = HashSet::new();

    let owner1 = Identity::generate("owner1");
    let owner1_fp = owner1.fingerprint();
    let owner1_pk = owner1.public().to_bytes();
    let owner2 = Identity::generate("owner2");
    let owner2_fp = owner2.fingerprint();
    let bob = fp("bob");

    store_a.set_signer(Arc::new(owner1));
    store_b.set_signer(Arc::new(owner2));

    // Daemon A: owner1 shares, promotes owner2 to Owner, admits bob (editor),
    // sets quorum:2, then authors the FIRST revoke of bob.
    kb_share_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        "kbq",
        "owner1",
        &mut docs_a,
    )
    .await;
    dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_member_msg("kb/add_member", "kbq", &owner2_fp, Some("owner")),
        &mut docs_a,
    )
    .await;
    dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_member_msg("kb/add_member", "kbq", &bob, Some("editor")),
        &mut docs_a,
    )
    .await;
    let g = dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_set_governance_msg("kbq", "quorum:2"),
        &mut docs_a,
    )
    .await;
    assert!(g.error.is_none(), "set_governance: {:?}", g.error);
    store_a.set_kb_anchor("kbq", owner1_pk).await;
    let r1 = dispatch_as(
        &store_a,
        &bc,
        Some("owner1"),
        Some(&owner1_fp),
        kb_revoke_msg("kbq", &bob),
        &mut docs_a,
    )
    .await;
    assert!(r1.error.is_none(), "owner1 revoke: {:?}", r1.error);

    // One owner's revoke is below quorum ⇒ bob still has access on daemon A.
    let access = |store: Arc<DocStore>, p: String, op: KbOp| async move {
        kb_access(&store, "kbq", Some(&p), op, Transport::Hub).await
    };
    assert!(
        matches!(
            access(Arc::clone(&store_a), bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "one owner's revoke is below quorum ⇒ bob retains access on daemon A"
    );

    // Sync A's signed collection state to daemon B (what the mesh transport carries).
    let (state, _) = store_a
        .encode_state_and_sv("kbc:kbq")
        .await
        .expect("collection exists on A");
    store_b
        .apply_update("kbc:kbq", &state, None)
        .await
        .expect("apply A's collection state onto B");
    store_b.set_kb_anchor("kbq", owner1_pk).await;

    // B derives the same anchored membership: owner2 is an Owner, and bob still has
    // access there too (still one revoke).
    assert!(
        matches!(
            access(Arc::clone(&store_b), owner2_fp.clone(), KbOp::Manage).await,
            Ok(AccessDecision::Allow)
        ),
        "owner2 derives Owner on its own daemon ⇒ may co-sign"
    );
    assert!(
        matches!(
            access(Arc::clone(&store_b), bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Allow)
        ),
        "bob still below quorum on daemon B before owner2 co-signs"
    );

    // Daemon B: owner2 co-signs the revoke ⇒ TWO distinct owners ⇒ bob removed.
    let r2 = dispatch_as(
        &store_b,
        &bc,
        Some("owner2"),
        Some(&owner2_fp),
        kb_revoke_msg("kbq", &bob),
        &mut docs_b,
    )
    .await;
    assert!(r2.error.is_none(), "owner2 revoke: {:?}", r2.error);
    assert!(
        matches!(
            access(Arc::clone(&store_b), bob.clone(), KbOp::Edit).await,
            Ok(AccessDecision::Deny(_))
        ),
        "two distinct owners co-signed ⇒ bob removed by quorum"
    );
}

/// A non-owner principal cannot drive either governance primitive: the derived gate
/// denies `kb/revoke` (Manage) and `kb/set_governance` (owner-only) before anything
/// is signed.
#[tokio::test]
async fn non_owner_cannot_revoke_or_set_governance() {
    use mae_mcp::identity::Identity;

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_pk = owner.public().to_bytes();
    store.set_signer(Arc::new(owner));

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbn",
        "owner",
        &mut docs,
    )
    .await;
    let editor = fp("editor");
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbn", &editor, Some("editor")),
        &mut docs,
    )
    .await;
    store.set_kb_anchor("kbn", owner_pk).await;

    // An editor principal may not revoke (Manage denied at the derived gate) ...
    let r = dispatch_as(
        &store,
        &bc,
        Some("editor"),
        Some(&editor),
        kb_revoke_msg("kbn", &fp("someone")),
        &mut docs,
    )
    .await;
    assert!(r.error.is_some(), "editor may not revoke");

    // ... nor set governance.
    let gov = dispatch_as(
        &store,
        &bc,
        Some("editor"),
        Some(&editor),
        kb_set_governance_msg("kbn", "quorum:2"),
        &mut docs,
    )
    .await;
    assert!(gov.error.is_some(), "editor may not set governance");
}

// --- ADR-036 signed content ops — daemon verify-on-apply (#91 D3) ---

/// Build a `kb/node_update` request whose params carry the ADR-036 signed
/// authorship header (the wire form the editor's sign-on-push produces).
fn signed_node_update_msg(
    kb_id: &str,
    node_id: &str,
    update: &[u8],
    signed: &mae_sync::content_ops::SignedContentOp,
) -> serde_json::Value {
    let mut params = serde_json::json!({
        "kb_id": kb_id,
        "node_id": node_id,
        "update": update_to_base64(update),
    });
    for (k, v) in signed.header_params().as_object().unwrap() {
        params[k] = v.clone();
    }
    serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update","params":params})
}

/// On an ANCHORED KB the daemon verifies a signed content op against the DERIVED
/// membership: a valid editor's signed edit applies; a tampered signature, and an
/// edit mis-attributed to a non-member, are both rejected — the relay can neither
/// forge nor mis-attribute (ADR-036 §D3). bob signs with a REAL key.
#[tokio::test]
async fn signed_content_op_verified_or_rejected_on_anchored_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_ops::{ContentOp, SignedContentOp};

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    let owner_pk = owner.public().to_bytes();
    store.set_signer(Arc::new(owner));

    // bob is a real editor identity (he must SIGN, so a synthetic fp won't do).
    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();
    let bob_secret = bob.secret_bytes();
    let bob_pub = bob.public().to_bytes();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbc",
        "owner",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbc", &bob_fp, Some("editor")),
        &mut docs,
    )
    .await;
    // Anchor ⇒ the daemon derives membership from the signed op-log (the mesh path).
    store.set_kb_anchor("kbc", owner_pk).await;

    // A yrs edit authored under bob's epoch-0 KB client_id (so the ADR-023 fence
    // also passes for the valid case — signing is an ADDITIONAL gate, not a bypass).
    let cid = mae_sync::kb::derive_kb_client_id(&bob_fp, 0);
    let mut ts = TextSync::with_client_id("", cid);
    let upd = ts.insert(0, "hello mesh");

    // (1) VALID: bob signs his own edit → verified + applied.
    let op = ContentOp {
        kb_id: "kbc".to_string(),
        node_id: "concept:n".to_string(),
        base_sv: vec![],
        author: bob_fp.clone(),
        epoch: 0,
        issued_at: 1_700_000_000,
    };
    let sig = op.sign(&bob_secret, &upd);
    let signed = SignedContentOp {
        op,
        payload: upd.clone(),
        sig,
        author_pubkey: bob_pub,
    };
    let ok = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        signed_node_update_msg("kbc", "concept:n", &upd, &signed),
        &mut docs,
    )
    .await;
    assert!(
        ok.error.is_none(),
        "valid signed edit applies: {:?}",
        ok.error
    );

    // (2) FORGED SIGNATURE: flip a signature byte → BadSignature, rejected.
    let mut tampered = signed.clone();
    tampered.sig[0] ^= 0xff;
    let bad = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        signed_node_update_msg("kbc", "concept:n", &upd, &tampered),
        &mut docs,
    )
    .await;
    assert!(
        bad.error
            .as_ref()
            .map(|e| e.message.contains("signed content op rejected"))
            .unwrap_or(false),
        "tampered signature rejected, got {:?}",
        bad.error
    );

    // (3) MIS-ATTRIBUTION: a member (bob) relays an op attributed to a NON-member
    // (mallory), validly signed by mallory's own key. The signature + fingerprint
    // bind, but mallory ∉ members ⇒ NotAMember, rejected.
    let mallory = Identity::generate("mallory");
    let m_op = ContentOp {
        kb_id: "kbc".to_string(),
        node_id: "concept:n".to_string(),
        base_sv: vec![],
        author: mallory.fingerprint(),
        epoch: 0,
        issued_at: 1_700_000_000,
    };
    let m_sig = m_op.sign(&mallory.secret_bytes(), &upd);
    let m_signed = SignedContentOp {
        op: m_op,
        payload: upd.clone(),
        sig: m_sig,
        author_pubkey: mallory.public().to_bytes(),
    };
    let injected = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        signed_node_update_msg("kbc", "concept:n", &upd, &m_signed),
        &mut docs,
    )
    .await;
    assert!(
        injected
            .error
            .as_ref()
            .map(|e| e.message.contains("signed content op rejected"))
            .unwrap_or(false),
        "non-member-attributed op rejected, got {:?}",
        injected.error
    );
}

/// Unit coverage for the shared relay verifier on an OWNED KB (the B→A direction —
/// the owner re-verifying a joiner's relayed op, anchor = its own signer key). Drives
/// every branch: a valid member's op verifies; an unsigned op is rejected under
/// require-signed (mesh) but accepted without it (hub migration); a non-member is
/// rejected; a non-KB doc passes through. Fast — no network.
#[tokio::test]
async fn verify_relayed_content_op_owned_kb_branches() {
    use mae_mcp::identity::Identity;
    use mae_sync::content_ops::{ContentOp, SignedContentOp};

    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    store.set_signer(Arc::new(owner));
    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbo",
        "owner",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kbo", &bob_fp, Some("editor")),
        &mut docs,
    )
    .await;
    // Deliberately NOT anchored: an OWNED KB, so `resolve_content_anchor` must fall
    // back to the daemon's own signer key (owner == signer).

    let upd = {
        let mut ts = TextSync::with_client_id("", 5);
        ts.insert(0, "hi")
    };
    let header = |id: &Identity, epoch: u64| {
        let op = ContentOp {
            kb_id: "kbo".to_string(),
            node_id: "concept:n".to_string(),
            base_sv: vec![],
            author: id.fingerprint(),
            epoch,
            issued_at: 0,
        };
        let sig = op.sign(&id.secret_bytes(), &upd);
        SignedContentOp {
            op,
            payload: upd.clone(),
            sig,
            author_pubkey: id.public().to_bytes(),
        }
        .header_params()
    };
    let doc = "kb:concept:n";

    // A valid member's signed op verifies (owned-KB anchor resolved from the signer).
    let h = header(&bob, 0);
    assert!(
        matches!(
            verify_relayed_content_op(&store, "kbo", doc, &upd, Some(&h), true).await,
            Ok(Some(_))
        ),
        "valid member op on an owned KB verifies"
    );
    // Unsigned: rejected under require-signed (mesh), accepted without it (hub).
    assert!(
        verify_relayed_content_op(&store, "kbo", doc, &upd, None, true)
            .await
            .is_err(),
        "unsigned rejected when require_signed (mesh)"
    );
    assert!(
        matches!(
            verify_relayed_content_op(&store, "kbo", doc, &upd, None, false).await,
            Ok(None)
        ),
        "unsigned accepted on the hub (migration)"
    );
    // A non-member's validly-signed op is rejected (NotAMember).
    let stranger = header(&Identity::generate("stranger"), 0);
    assert!(
        verify_relayed_content_op(&store, "kbo", doc, &upd, Some(&stranger), true)
            .await
            .is_err(),
        "non-member op rejected even with a valid signature"
    );
    // A non-KB doc is not a content op → passes through.
    assert!(
        matches!(
            verify_relayed_content_op(&store, "kbo", "buffer:foo.txt", &upd, None, true).await,
            Ok(None)
        ),
        "non-KB doc passes through"
    );
}

/// ADR-039 A1 (#157): for an ANCHORED KB the ADR-023 epoch fence must read a member's
/// epoch from the SIGNED op-log (the same authority as the role), not the legacy
/// `member_roles` map — which is frozen on a mesh join (B-12), so an op-log-only member
/// would read epoch 0 and have every valid (non-epoch-0) edit wrongly fenced.
#[tokio::test]
async fn kb_member_epoch_reads_oplog_not_legacy_for_anchored_kb() {
    use mae_mcp::identity::Identity;
    use mae_sync::kb::{KbCollectionDoc, Role};
    use mae_sync::membership::MembershipAction;

    let store = test_doc_store();
    let owner = Identity::generate("owner");
    let ofp = owner.fingerprint();
    let opk = owner.public().to_bytes();
    let osec = owner.secret_bytes();
    let mfp = fp("member"); // op-log-only (never written to member_roles, as on a mesh join)

    let mut coll = KbCollectionDoc::new_owned("kbe", &ofp, "owner");
    let g = coll.build_membership_op(
        "kbe",
        MembershipAction::Admit,
        &ofp,
        Some(Role::Owner),
        true,
        &ofp,
        0,
        None,
        0,
    );
    let gsig = g.sign(&osec);
    coll.append_signed_op(&g, &gsig, &opk);
    // Admit the member at a NON-ZERO epoch (a re-grant carries a fresh epoch in reality).
    let a = coll.build_membership_op(
        "kbe",
        MembershipAction::Admit,
        &mfp,
        Some(Role::Editor),
        false,
        &ofp,
        0,
        None,
        7,
    );
    let asig = a.sign(&osec);
    coll.append_signed_op(&a, &asig, &opk);

    // The bug's precondition: legacy member_roles has no entry ⇒ epoch_of → 0.
    assert_eq!(
        coll.epoch_of(&mfp),
        0,
        "member_roles is empty for an op-log-only member"
    );

    // Anchored ⇒ kb_member_epoch derives the NON-ZERO epoch from the signed op-log (the fix).
    store.set_kb_anchor("kbe", opk).await;
    assert_eq!(
        kb_member_epoch(&store, "kbe", &coll, &mfp).await,
        7,
        "epoch comes from the signed op-log, not the frozen member_roles"
    );
    // Un-anchored (owned KB) ⇒ falls back to the legacy member_roles epoch (0 here).
    let store2 = test_doc_store();
    assert_eq!(
        kb_member_epoch(&store2, "kbe", &coll, &mfp).await,
        0,
        "an un-anchored KB keeps the legacy member_roles epoch"
    );
}
