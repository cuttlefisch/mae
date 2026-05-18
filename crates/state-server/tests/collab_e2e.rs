//! In-memory collaborative editing E2E tests.
//!
//! Tests exercise the full multi-client flow using duplex pipes (no TCP,
//! no env gating). Each test spawns server handlers + simulated clients.

use std::sync::Arc;

use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
use mae_state_server::doc_store::DocStore;
use mae_state_server::handler::handle_client;
use mae_state_server::storage::SqliteBackend;
use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::text::TextSync;
use tokio::io::{AsyncWriteExt, BufReader};

// --- Helpers ---

fn test_broadcaster() -> SharedBroadcaster {
    Arc::new(std::sync::Mutex::new(EventBroadcaster::new()))
}

fn test_doc_store() -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500))
}

struct Client {
    writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    reader: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    next_id: u64,
}

impl Client {
    /// Connect a simulated client via duplex pipe. Spawns server handler task.
    async fn connect(store: Arc<DocStore>, broadcaster: SharedBroadcaster) -> Self {
        let (client_stream, server_stream) = tokio::io::duplex(8192);

        let (server_read, server_write) = tokio::io::split(server_stream);
        let server_reader = BufReader::new(server_read);

        tokio::spawn(async move {
            handle_client(
                server_reader,
                server_write,
                store,
                broadcaster,
                std::time::Instant::now(),
            )
            .await;
        });

        let (client_read, client_write) = tokio::io::split(client_stream);
        let client_reader = BufReader::new(client_read);

        let mut client = Client {
            writer: client_write,
            reader: client_reader,
            next_id: 1,
        };

        // Handshake: initialize + subscribe to sync_update + peer events
        client.initialize().await;
        client.subscribe().await;
        client
    }

    async fn send(&mut self, msg: &serde_json::Value) {
        let payload = format!("{}\n", serde_json::to_string(msg).unwrap());
        self.writer.write_all(payload.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    /// Read the next JSON-RPC response, skipping notifications.
    async fn recv(&mut self) -> serde_json::Value {
        loop {
            let text = mae_mcp::read_message(&mut self.reader)
                .await
                .unwrap()
                .unwrap();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            // Skip notifications (have "method" but no response "id" with result/error).
            if val.get("method").is_some()
                && val.get("result").is_none()
                && val.get("error").is_none()
            {
                continue; // notification, skip
            }
            return val;
        }
    }

    /// Try to read a message with timeout. Returns None if no message within duration.
    async fn recv_timeout(&mut self, ms: u64) -> Option<serde_json::Value> {
        match tokio::time::timeout(
            std::time::Duration::from_millis(ms),
            mae_mcp::read_message(&mut self.reader),
        )
        .await
        {
            Ok(Ok(Some(text))) => serde_json::from_str(&text).ok(),
            _ => None,
        }
    }

    async fn initialize(&mut self) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "initialize",
            "params": {"clientInfo": {"name": "test-client"}}
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "initialize failed: {resp}");
    }

    async fn subscribe(&mut self) {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "notifications/subscribe",
            "params": {"types": ["sync_update", "peer_joined", "peer_left", "save_committed"]}
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "subscribe failed: {resp}");
    }

    /// Share a document: send sync/share with initial content.
    async fn share(&mut self, doc: &str, content: &str) {
        let ts = TextSync::new(content);
        let state = ts.encode_state();
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "sync/share",
            "params": { "doc": doc, "update": update_to_base64(&state) }
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "share failed: {resp}");
    }

    /// Send a sync/update with the given yrs update bytes.
    async fn send_update(&mut self, doc: &str, update: &[u8]) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "sync/update",
            "params": { "doc": doc, "update": update_to_base64(update) }
        });
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Get full state for a document.
    async fn full_state(&mut self, doc: &str) -> Vec<u8> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "sync/full_state",
            "params": { "doc": doc }
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        let state_b64 = resp["result"]["state"].as_str().unwrap();
        base64_to_update(state_b64).unwrap()
    }

    /// Get text content for a document.
    async fn content(&mut self, doc: &str) -> String {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "docs/content",
            "params": { "doc": doc }
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        resp["result"]["content"].as_str().unwrap().to_string()
    }

    /// Send docs/save_intent.
    async fn save_intent(&mut self, doc: &str, expected_hash: &str) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "docs/save_intent",
            "params": { "doc": doc, "expected_hash": expected_hash }
        });
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Send docs/save_committed.
    async fn save_committed(
        &mut self,
        doc: &str,
        save_epoch: u64,
        content_hash: &str,
        saved_by: &str,
    ) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "docs/save_committed",
            "params": {
                "doc": doc,
                "save_epoch": save_epoch,
                "content_hash": content_hash,
                "saved_by": saved_by,
            }
        });
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Drain any pending notifications (non-blocking).
    async fn drain_notifications(&mut self) -> Vec<serde_json::Value> {
        let mut notifications = Vec::new();
        while let Some(msg) = self.recv_timeout(50).await {
            if msg.get("method").is_some() {
                notifications.push(msg);
            }
        }
        notifications
    }

    /// Wait for a notification matching the given method, draining others.
    /// Returns None if timeout expires.
    async fn wait_for_notification(
        &mut self,
        method: &str,
        timeout_ms: u64,
    ) -> Option<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match tokio::time::timeout(remaining, mae_mcp::read_message(&mut self.reader)).await {
                Ok(Ok(Some(text))) => {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        if val.get("method").and_then(|m| m.as_str()) == Some(method) {
                            return Some(val);
                        }
                        // Not the method we want, continue draining.
                    }
                }
                _ => return None,
            }
        }
    }
}

/// Compute SHA-256 hash of content (matching server's content_hash).
fn sha256(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

// --- Tests ---

#[tokio::test]
async fn two_clients_bidirectional_sync() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares a document with "hello".
    client_a.share("test.txt", "hello").await;

    // B gets full state.
    let state = client_b.full_state("test.txt").await;
    let mut ts_b = TextSync::from_state(&state).unwrap();
    assert_eq!(ts_b.content(), "hello");

    // B inserts " world" at offset 5.
    let update_b = ts_b.insert(5, " world");
    client_b.send_update("test.txt", &update_b).await;

    // A should receive the sync_update notification (may need to skip peer_joined).
    let notif = client_a
        .wait_for_notification("notifications/sync_update", 1000)
        .await;
    assert!(notif.is_some(), "A should receive sync notification");

    // Server content should be "hello world".
    let content = client_a.content("test.txt").await;
    assert_eq!(content, "hello world");
}

#[tokio::test]
async fn undo_does_not_corrupt_peer() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares empty doc.
    client_a.share("undo.txt", "").await;

    // Both get their own TextSync from the shared state.
    let state = client_a.full_state("undo.txt").await;
    let mut ts_a = TextSync::from_state(&state).unwrap();

    let state = client_b.full_state("undo.txt").await;
    let mut ts_b = TextSync::from_state(&state).unwrap();

    // A types "hello".
    let update_a = ts_a.insert(0, "hello");
    client_a.send_update("undo.txt", &update_a).await;

    // B applies A's update and types "world".
    let notif = client_b
        .wait_for_notification("notifications/sync_update", 1000)
        .await
        .unwrap();
    let event_data = &notif["params"]["event"]["data"];
    let update_b64 = event_data["update_base64"].as_str().unwrap();
    let remote_update = base64_to_update(update_b64).unwrap();
    ts_b.apply_update(&remote_update).unwrap();
    assert_eq!(ts_b.content(), "hello");

    let update_b = ts_b.insert(5, "world");
    client_b.send_update("undo.txt", &update_b).await;

    // A receives B's sync_update notification and applies it locally.
    let notif_a = client_a
        .wait_for_notification("notifications/sync_update", 1000)
        .await
        .unwrap();
    let a_update_b64 = notif_a["params"]["event"]["data"]["update_base64"]
        .as_str()
        .unwrap();
    ts_a.apply_update(&base64_to_update(a_update_b64).unwrap())
        .unwrap();
    assert_eq!(ts_a.content(), "helloworld");

    // Server should have "helloworld".
    let content = client_a.content("undo.txt").await;
    assert_eq!(content, "helloworld");

    // A undoes "hello" by reconciling to "world".
    // reconcile_to produces a minimal CRDT delta (not full-state replacement).
    let undo_update = ts_a.reconcile_to("world");
    assert!(!undo_update.is_empty(), "reconcile should produce update");
    client_a.send_update("undo.txt", &undo_update).await;

    // B should receive the undo delta.
    let _ = client_b
        .wait_for_notification("notifications/sync_update", 500)
        .await;

    // Server content should be "world" (A's "hello" undone, B's "world" preserved).
    let content = client_b.content("undo.txt").await;
    assert_eq!(content, "world");
}

#[tokio::test]
async fn save_intent_matches_crdt_content() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares.
    client_a.share("save.txt", "initial").await;

    // B gets state.
    let state = client_b.full_state("save.txt").await;
    let mut ts_b = TextSync::from_state(&state).unwrap();

    // B edits.
    let update_b = ts_b.insert(7, " content");
    client_b.send_update("save.txt", &update_b).await;

    // Drain A's notification.
    let _ = client_a
        .wait_for_notification("notifications/sync_update", 500)
        .await;

    // B checks save_intent with correct hash.
    let content = client_b.content("save.txt").await;
    assert_eq!(content, "initial content");
    let hash = sha256(&content);
    let resp = client_b.save_intent("save.txt", &hash).await;
    let result = &resp["result"]["result"];
    assert_eq!(result["status"], "ok", "save intent should succeed");
    assert!(result["save_epoch"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn save_intent_detects_conflict() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("conflict.txt", "version1").await;

    // Check with wrong hash.
    let resp = client_a
        .save_intent("conflict.txt", "wrong-hash-value")
        .await;
    let result = &resp["result"]["result"];
    assert_eq!(result["status"], "conflict");
}

#[tokio::test]
async fn client_disconnect_notifies_peers() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Drain peer_joined notifications that A received when B connected.
    let _ = client_a.drain_notifications().await;

    // Drop client_a entirely to simulate disconnect (closes both read and write halves).
    drop(client_a);

    // B should receive a peer_left notification.
    let notif = client_b
        .wait_for_notification("notifications/peer_left", 2000)
        .await;
    assert!(notif.is_some(), "B should receive peer_left notification");
}

#[tokio::test]
async fn concurrent_edits_converge() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares a document.
    client_a.share("concurrent.txt", "abcdef").await;

    // Both get state.
    let state = client_a.full_state("concurrent.txt").await;
    let mut ts_a = TextSync::from_state(&state).unwrap();

    let state = client_b.full_state("concurrent.txt").await;
    let mut ts_b = TextSync::from_state(&state).unwrap();

    // A inserts "X" at offset 2, B inserts "Y" at offset 4 — simultaneously.
    let update_a = ts_a.insert(2, "X");
    let update_b = ts_b.insert(4, "Y");

    // Send both without waiting for responses.
    client_a.send_update("concurrent.txt", &update_a).await;
    client_b.send_update("concurrent.txt", &update_b).await;

    // Allow time for processing.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Both should see same final content on the server.
    let content_a = client_a.content("concurrent.txt").await;
    let content_b = client_b.content("concurrent.txt").await;
    assert_eq!(content_a, content_b, "both clients should converge");
    // Both insertions should be present.
    assert!(content_a.contains('X'), "should contain A's edit");
    assert!(content_a.contains('Y'), "should contain B's edit");
}

#[tokio::test]
async fn rejoin_after_disconnect() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares.
    client_a.share("rejoin.txt", "original").await;

    // B joins, edits, disconnects.
    {
        let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
        let state = client_b.full_state("rejoin.txt").await;
        let mut ts_b = TextSync::from_state(&state).unwrap();
        let update = ts_b.insert(8, " modified");
        client_b.send_update("rejoin.txt", &update).await;
        // B disconnects (dropped at end of scope).
    }

    // Allow disconnect to propagate.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // B2 reconnects and gets latest state.
    let mut client_b2 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let content = client_b2.content("rejoin.txt").await;
    assert_eq!(content, "original modified");
}

#[tokio::test]
async fn save_committed_broadcasts_to_peers() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares.
    client_a.share("saved.txt", "content").await;

    // Drain B's notifications from share.
    let _ = client_b.drain_notifications().await;

    // A saves.
    let hash = sha256("content");
    let intent_resp = client_a.save_intent("saved.txt", &hash).await;
    let epoch = intent_resp["result"]["result"]["save_epoch"]
        .as_u64()
        .unwrap();
    client_a
        .save_committed("saved.txt", epoch, &hash, "alice")
        .await;

    // B should receive save_committed notification.
    let notif = client_b
        .wait_for_notification("notifications/save_committed", 1000)
        .await;
    assert!(
        notif.is_some(),
        "B should receive save_committed notification"
    );
}
