use super::*;
use tokio::io::BufReader;

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
