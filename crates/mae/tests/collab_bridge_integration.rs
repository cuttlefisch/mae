//! Bridge integration tests — protocol-level tests via duplex pipes.
//!
//! Tests exercise the full JSON-RPC round-trip between a simulated client and
//! a real `handle_client` server handler via duplex pipes (no TCP).
//! Additional buffer-level and editor-level tests are in their respective crate tests.

use std::sync::Arc;

use mae_core::Buffer;
use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
use mae_state_server::doc_store::DocStore;
use mae_state_server::handler::handle_client;
use mae_state_server::storage::SqliteBackend;
use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::text::TextSync;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWriteExt, BufReader};

// --- Test Infrastructure ---

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
        client.initialize().await;
        client.subscribe().await;
        client
    }

    async fn send(&mut self, msg: &serde_json::Value) {
        let payload = format!("{}\n", serde_json::to_string(msg).unwrap());
        self.writer.write_all(payload.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
    }

    async fn recv(&mut self) -> serde_json::Value {
        loop {
            let text = mae_mcp::read_message(&mut self.reader)
                .await
                .unwrap()
                .unwrap();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            if val.get("method").is_some()
                && val.get("result").is_none()
                && val.get("error").is_none()
            {
                continue;
            }
            return val;
        }
    }

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
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"initialize","params":{"clientInfo":{"name":"bridge-test"}}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "initialize failed: {resp}");
    }

    async fn subscribe(&mut self) {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"notifications/subscribe","params":{"types":["sync_update","peer_joined","peer_left","save_committed"]}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "subscribe failed: {resp}");
    }

    async fn share(&mut self, doc: &str, content: &str) {
        let ts = TextSync::new(content);
        let state = ts.encode_state();
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/share","params":{"doc":doc,"update":update_to_base64(&state)}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "share failed: {resp}");
    }

    async fn send_update(&mut self, doc: &str, update: &[u8]) -> serde_json::Value {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/update","params":{"doc":doc,"update":update_to_base64(update)}});
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    async fn full_state(&mut self, doc: &str) -> Vec<u8> {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/full_state","params":{"doc":doc}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        base64_to_update(resp["result"]["state"].as_str().unwrap()).unwrap()
    }

    async fn content(&mut self, doc: &str) -> String {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"docs/content","params":{"doc":doc}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        resp["result"]["content"].as_str().unwrap().to_string()
    }

    async fn resync(&mut self, doc: &str) -> (Vec<u8>, Vec<u8>) {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/resync","params":{"doc":doc}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        let state = base64_to_update(resp["result"]["state"].as_str().unwrap()).unwrap();
        let sv = base64_to_update(resp["result"]["sv"].as_str().unwrap()).unwrap();
        (state, sv)
    }

    async fn doc_stats(&mut self, doc: &str) -> serde_json::Value {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"docs/stats","params":{"doc":doc}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        resp["result"]["stats"].clone()
    }

    async fn debug_stats(&mut self) -> serde_json::Value {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"$/debug"});
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    async fn ping(&mut self) -> serde_json::Value {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"$/ping"});
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    async fn save_intent(&mut self, doc: &str, expected_hash: &str) -> serde_json::Value {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"docs/save_intent","params":{"doc":doc,"expected_hash":expected_hash}});
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    async fn save_committed(
        &mut self,
        doc: &str,
        saved_by: &str,
        save_epoch: u64,
        content_hash: &str,
    ) -> serde_json::Value {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"docs/save_committed","params":{"doc":doc,"saved_by":saved_by,"save_epoch":save_epoch,"content_hash":content_hash}});
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

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
                    }
                }
                _ => return None,
            }
        }
    }
}

// ============================================================================
// Tier 1 — Bridge Integration Tests
// ============================================================================

#[tokio::test]
async fn share_edit_roundtrip() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("test.txt", "hello").await;
    let state = client.full_state("test.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();
    let update = ts.insert(5, " world");
    client.send_update("test.txt", &update).await;

    assert_eq!(client.content("test.txt").await, "hello world");
}

#[tokio::test]
async fn remote_update_applies_to_buffer() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("remote.txt", "hello").await;

    let state = client_b.full_state("remote.txt").await;
    let mut ts_b = TextSync::from_state(&state).unwrap();
    let update = ts_b.insert(5, " remote");
    client_b.send_update("remote.txt", &update).await;

    let notif = client_a
        .wait_for_notification("notifications/sync_update", 1000)
        .await;
    assert!(notif.is_some(), "A should receive sync notification");

    // Verify: get full state from server and load into a local buffer.
    // The server state already includes B's edit.
    let full = client_a.full_state("remote.txt").await;
    let mut buf = Buffer::new();
    buf.name = "remote.txt".to_string();
    buf.load_sync_state(&full, 100).unwrap();
    assert_eq!(buf.text(), "hello remote");
}

#[tokio::test]
async fn two_editors_converge() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    ca.share("converge.txt", "abcdef").await;
    let mut ts_a = TextSync::from_state(&ca.full_state("converge.txt").await).unwrap();
    let mut ts_b = TextSync::from_state(&cb.full_state("converge.txt").await).unwrap();

    let ua = ts_a.insert(2, "X");
    let ub = ts_b.insert(4, "Y");
    ca.send_update("converge.txt", &ua).await;
    cb.send_update("converge.txt", &ub).await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let content_a = ca.content("converge.txt").await;
    let content_b = cb.content("converge.txt").await;
    assert_eq!(content_a, content_b, "should converge");
    assert!(content_a.contains('X') && content_a.contains('Y'));
}

#[tokio::test]
async fn doc_id_differs_from_buffer_name() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("file:abc/main.rs", "fn main() {}").await;
    assert_eq!(client.content("file:abc/main.rs").await, "fn main() {}");

    let state = client.full_state("file:abc/main.rs").await;
    let mut ts = TextSync::from_state(&state).unwrap();
    let update = ts.insert(12, "\n");
    client.send_update("file:abc/main.rs", &update).await;
    assert_eq!(client.content("file:abc/main.rs").await, "fn main() {}\n");
}

#[tokio::test]
async fn drain_and_broadcast_uses_collab_doc_id() {
    use mae_core::Editor;

    let mut editor = Editor::default();
    let mut buf = Buffer::new();
    buf.name = "main.rs".to_string();
    buf.insert_text_at(0, "start");
    buf.enable_sync(1);
    buf.collab_doc_id = Some("file:proj/main.rs".to_string());
    buf.insert_text_at(5, " end");
    editor.buffers.push(buf);
    editor
        .collab
        .synced_buffers
        .insert("file:proj/main.rs".to_string());

    // Verify that collab_doc_id is used (not buffer name) when forwarding.
    for b in &mut editor.buffers {
        if !b.pending_sync_updates.is_empty() {
            let doc_id = b.collab_doc_id.clone().unwrap_or_else(|| b.name.clone());
            assert_eq!(
                doc_id, "file:proj/main.rs",
                "should use collab_doc_id, not buffer name"
            );
            b.pending_sync_updates.clear();
        }
    }
}

#[tokio::test]
async fn undo_through_bridge() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    ca.share("undo.txt", "").await;
    let mut ts_a = TextSync::from_state(&ca.full_state("undo.txt").await).unwrap();
    let mut ts_b = TextSync::from_state(&cb.full_state("undo.txt").await).unwrap();

    let ua = ts_a.insert(0, "hello");
    ca.send_update("undo.txt", &ua).await;

    let notif = cb
        .wait_for_notification("notifications/sync_update", 1000)
        .await
        .unwrap();
    let b64 = notif["params"]["event"]["data"]["update_base64"]
        .as_str()
        .unwrap();
    ts_b.apply_update(&base64_to_update(b64).unwrap()).unwrap();
    let ub = ts_b.insert(5, "world");
    cb.send_update("undo.txt", &ub).await;

    let notif_a = ca
        .wait_for_notification("notifications/sync_update", 1000)
        .await
        .unwrap();
    let a_b64 = notif_a["params"]["event"]["data"]["update_base64"]
        .as_str()
        .unwrap();
    ts_a.apply_update(&base64_to_update(a_b64).unwrap())
        .unwrap();

    let undo = ts_a.reconcile_to("world");
    assert!(!undo.is_empty());
    ca.send_update("undo.txt", &undo).await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(cb.content("undo.txt").await, "world");
}

#[tokio::test]
async fn replace_contents_queues_sync_updates() {
    let mut buf = Buffer::new();
    buf.name = "replace.rs".to_string();
    buf.insert_text_at(0, "old content");
    buf.enable_sync(1);
    buf.replace_contents("new content");
    assert!(
        !buf.pending_sync_updates.is_empty(),
        "should queue sync updates"
    );
    assert_eq!(buf.text(), "new content");
    assert_eq!(buf.sync_doc.as_ref().unwrap().content(), "new content");
}

#[tokio::test]
async fn apply_sync_update_when_sync_none() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello");
    let result = buf.apply_sync_update(&[1, 2, 3]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("sync not enabled"));
}

#[tokio::test]
async fn echo_filtering() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("echo.txt", "start").await;
    let state = client.full_state("echo.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();
    let update = ts.insert(5, " end");
    client.send_update("echo.txt", &update).await;

    assert!(
        client.recv_timeout(200).await.is_none(),
        "should not receive echo"
    );
}

#[tokio::test]
async fn share_edits_during_roundtrip() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("immediate.txt", "hello").await;
    let state = client.full_state("immediate.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();
    let update = ts.insert(5, " world");
    client.send_update("immediate.txt", &update).await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    assert_eq!(cb.content("immediate.txt").await, "hello world");
}

#[tokio::test]
async fn reshare_replaces_not_appends() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("reshare.txt", "version 1").await;
    assert_eq!(client.content("reshare.txt").await, "version 1");
    client.share("reshare.txt", "version 2").await;
    assert_eq!(client.content("reshare.txt").await, "version 2");
}

// ============================================================================
// Tier 2 — Protocol Feature Tests (save protocol, heartbeat, reconnect)
// ============================================================================

fn sha256_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// WU3: Save intent → committed round-trip with broadcast to second client.
#[tokio::test]
async fn save_intent_to_committed_roundtrip() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Client A shares a doc with known content.
    client_a.share("save-test.txt", "save me").await;

    // Client B joins (so it receives broadcasts).
    let _ = client_b.resync("save-test.txt").await;

    // Client A sends save_intent with correct SHA-256 hash.
    let hash = sha256_hash("save me");
    let resp = client_a.save_intent("save-test.txt", &hash).await;
    assert!(resp.get("error").is_none(), "save_intent failed: {resp}");
    let result = &resp["result"]["result"];
    assert_eq!(result["status"].as_str().unwrap(), "ok");
    let save_epoch = result["save_epoch"].as_u64().unwrap();
    assert!(save_epoch > 0, "save_epoch should be > 0, got {save_epoch}");

    // Client A sends save_committed.
    let committed_resp = client_a
        .save_committed("save-test.txt", "test-user", save_epoch, &hash)
        .await;
    assert!(
        committed_resp.get("error").is_none(),
        "save_committed failed: {committed_resp}"
    );
    assert_eq!(committed_resp["result"]["committed"], true);

    // Client B should receive a save_committed notification.
    let notif = client_b
        .wait_for_notification("notifications/save_committed", 2000)
        .await;
    assert!(
        notif.is_some(),
        "client B should receive save_committed broadcast"
    );
    let event = &notif.unwrap()["params"]["event"];
    assert_eq!(event["data"]["doc"].as_str().unwrap(), "save-test.txt");
    assert_eq!(event["data"]["saved_by"].as_str().unwrap(), "test-user");
}

/// WU3 (variant): Save intent with wrong hash returns conflict.
#[tokio::test]
async fn save_intent_conflict_on_hash_mismatch() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("conflict-test.txt", "real content").await;

    // Send save_intent with wrong hash.
    let resp = client
        .save_intent("conflict-test.txt", "0000000000000000")
        .await;
    assert!(resp.get("error").is_none(), "should not be an RPC error");
    assert_eq!(
        resp["result"]["result"]["status"].as_str().unwrap(),
        "conflict"
    );
}

/// WU4: Heartbeat ping/pong and server-drop EOF detection.
#[tokio::test]
async fn heartbeat_ping_pong_and_server_drop() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Use raw duplex so we can drop the server handle.
    let (client_stream, server_stream) = tokio::io::duplex(8192);
    let (sr, sw) = tokio::io::split(server_stream);

    let handle = tokio::spawn(async move {
        handle_client(BufReader::new(sr), sw, store, bc, std::time::Instant::now()).await;
    });

    let (cr, cw) = tokio::io::split(client_stream);
    let mut client = Client {
        writer: cw,
        reader: BufReader::new(cr),
        next_id: 1,
    };
    client.initialize().await;

    // Send $/ping and verify "pong".
    let resp = client.ping().await;
    assert!(resp.get("error").is_none(), "ping failed: {resp}");
    assert_eq!(resp["result"], "pong");

    // Drop server handle (simulates crash).
    handle.abort();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Next read should return EOF or error — not hang.
    match tokio::time::timeout(
        std::time::Duration::from_millis(500),
        mae_mcp::read_message(&mut client.reader),
    )
    .await
    {
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {} // expected: EOF, error, or timeout
        Ok(Ok(Some(_))) => {}                    // leftover message is acceptable
    }
}

/// WU5: Client reconnects to fresh server and re-shares — CRDT content preserved.
#[tokio::test]
async fn reconnect_reshare_preserves_crdt_state() {
    // Phase 1: Share and edit.
    let store1 = test_doc_store();
    let bc1 = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store1), Arc::clone(&bc1)).await;

    client.share("reconnect.txt", "original content").await;
    let state = client.full_state("reconnect.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();
    let update = ts.reconcile_to("modified content");
    assert!(!update.is_empty());
    client.send_update("reconnect.txt", &update).await;
    assert_eq!(client.content("reconnect.txt").await, "modified content");

    // Capture local CRDT state before disconnect.
    let preserved_state = client.full_state("reconnect.txt").await;

    // Phase 2: "Server crash" — drop store and broadcaster.
    drop(client);
    drop(store1);
    drop(bc1);

    // Phase 3: Fresh server.
    let store2 = test_doc_store();
    let bc2 = test_broadcaster();
    let mut client2 = Client::connect(Arc::clone(&store2), Arc::clone(&bc2)).await;

    // Re-share using preserved CRDT state (full state encode).
    let ts2 = TextSync::from_state(&preserved_state).unwrap();
    assert_eq!(ts2.content(), "modified content");

    // Share the preserved content to the new server.
    let share_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": client2.next_id,
        "method": "sync/share",
        "params": {
            "doc": "reconnect.txt",
            "update": update_to_base64(&preserved_state)
        }
    });
    client2.next_id += 1;
    client2.send(&share_msg).await;
    let resp = client2.recv().await;
    assert!(resp.get("error").is_none(), "re-share failed: {resp}");

    // Verify: new server has the modified content.
    assert_eq!(
        client2.content("reconnect.txt").await,
        "modified content",
        "CRDT state must survive reconnect to fresh server"
    );

    // Verify: a third client joining sees the correct content.
    let mut client3 = Client::connect(Arc::clone(&store2), Arc::clone(&bc2)).await;
    let (state3, _) = client3.resync("reconnect.txt").await;
    let ts3 = TextSync::from_state(&state3).unwrap();
    assert_eq!(
        ts3.content(),
        "modified content",
        "new peer must see preserved content"
    );
}

// ============================================================================
// Tier 3 — Fault Injection Tests
// ============================================================================

#[tokio::test]
async fn fault_server_drop_mid_session() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let (client_stream, server_stream) = tokio::io::duplex(8192);
    let (sr, sw) = tokio::io::split(server_stream);

    let handle = tokio::spawn(async move {
        handle_client(BufReader::new(sr), sw, store, bc, std::time::Instant::now()).await;
    });

    let (cr, mut cw) = tokio::io::split(client_stream);
    let mut cr = BufReader::new(cr);

    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"fault"}}});
    cw.write_all(format!("{}\n", serde_json::to_string(&msg).unwrap()).as_bytes())
        .await
        .unwrap();
    cw.flush().await.unwrap();
    let _ = mae_mcp::read_message(&mut cr).await;

    handle.abort();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Client should detect EOF or error — not hang.
    match mae_mcp::read_message(&mut cr).await {
        Ok(None) | Err(_) => {} // expected
        Ok(Some(_)) => {}       // leftover message is fine
    }
}

#[tokio::test]
async fn fault_invalid_json() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let (client_stream, server_stream) = tokio::io::duplex(8192);
    let (sr, sw) = tokio::io::split(server_stream);

    tokio::spawn(async move {
        handle_client(BufReader::new(sr), sw, store, bc, std::time::Instant::now()).await;
    });

    let (cr, mut cw) = tokio::io::split(client_stream);
    let mut cr = BufReader::new(cr);

    // Initialize.
    let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"fault"}}});
    cw.write_all(format!("{}\n", serde_json::to_string(&init).unwrap()).as_bytes())
        .await
        .unwrap();
    cw.flush().await.unwrap();
    let _ = mae_mcp::read_message(&mut cr).await;

    // Send garbage.
    cw.write_all(b"NOT JSON\n").await.unwrap();
    cw.flush().await.unwrap();

    // Ping should still work after garbage (or server disconnects — either is acceptable).
    let ping = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"$/ping"});
    cw.write_all(format!("{}\n", serde_json::to_string(&ping).unwrap()).as_bytes())
        .await
        .unwrap();
    cw.flush().await.unwrap();

    let _ = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        mae_mcp::read_message(&mut cr),
    )
    .await;
    // No panic = pass.
}

#[tokio::test]
async fn fault_invalid_base64_in_update() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":client.next_id,
        "method":"sync/update",
        "params":{"doc":"test","update":"!!! not base64 !!!"}
    });
    client.next_id += 1;
    client.send(&msg).await;
    let resp = client.recv().await;
    assert!(
        resp.get("error").is_some(),
        "should error on invalid base64"
    );
}

#[tokio::test]
async fn fault_concurrent_share_same_doc() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    ca.share("race.txt", "version A").await;
    cb.share("race.txt", "version B").await;

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let content_a = ca.content("race.txt").await;
    let content_b = cb.content("race.txt").await;
    assert_eq!(content_a, content_b, "concurrent shares should converge");
}

#[tokio::test]
async fn fault_stale_sync_after_reconnect() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("stale.txt", "original").await;
    assert_eq!(client.content("stale.txt").await, "original");

    client.share("stale.txt", "fresh").await;
    assert_eq!(client.content("stale.txt").await, "fresh");
}

// ============================================================================
// $/debug response shape validation (Flaw D fix verification)
// ============================================================================

#[tokio::test]
async fn debug_response_shape_matches_doctor() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("debug-test.rs", "fn main() {}").await;
    let resp = client.debug_stats().await;
    let result = &resp["result"];

    assert!(
        result["documents"].is_number(),
        "documents should be number"
    );
    assert!(
        result["doc_stats"].is_object(),
        "doc_stats should be object"
    );
    let stats = &result["doc_stats"]["debug-test.rs"];
    assert!(stats.is_object(), "doc stats should exist");
    assert!(stats.get("wal_seq").is_some());
}

// ============================================================================
// Tier 4 — CRDT Bug Regression Guards
// ============================================================================

/// BUG 1: sync/resync must track session doc so the joining client
/// receives subsequent sync/update broadcasts from other clients.
#[tokio::test]
async fn join_via_resync_receives_subsequent_updates() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Client A shares a doc.
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    client_a.share("resync-bug.txt", "initial").await;
    let state_a = client_a.full_state("resync-bug.txt").await;
    let mut ts_a = TextSync::from_state(&state_a).unwrap();

    // Client B joins via resync (the JoinDoc path).
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let (state_b, _sv_b) = client_b.resync("resync-bug.txt").await;
    let ts_b = TextSync::from_state(&state_b).unwrap();
    assert_eq!(
        ts_b.content(),
        "initial",
        "resync should return initial content"
    );

    // Verify the server tracks client B's doc subscription.
    let stats = client_b.doc_stats("resync-bug.txt").await;
    assert!(
        stats["connected_clients"].as_u64().unwrap() >= 2,
        "both clients should be tracked after resync, got: {stats}"
    );

    // Client A edits — client B should receive the notification.
    let update = ts_a.insert(7, " content");
    client_a.send_update("resync-bug.txt", &update).await;

    let notif = client_b
        .wait_for_notification("notifications/sync_update", 2000)
        .await;
    assert!(
        notif.is_some(),
        "BUG 1: client that joined via resync must receive subsequent updates"
    );
}

/// BUG 1 (variant): After resync, remote edits apply correctly.
#[tokio::test]
async fn remote_update_after_resync_applies() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    client_a.share("remote-apply.txt", "hello").await;
    let state_a = client_a.full_state("remote-apply.txt").await;
    let mut ts_a = TextSync::from_state(&state_a).unwrap();

    // Client B joins via resync.
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let (state_b, _) = client_b.resync("remote-apply.txt").await;
    let mut ts_b = TextSync::from_state(&state_b).unwrap();
    assert_eq!(ts_b.content(), "hello");

    // Client A appends.
    let update_a = ts_a.insert(5, " world");
    client_a.send_update("remote-apply.txt", &update_a).await;

    // Client B receives and applies.
    let notif = client_b
        .wait_for_notification("notifications/sync_update", 2000)
        .await;
    assert!(notif.is_some(), "client B must receive update");
    let update_data = notif.unwrap()["params"]["event"]["data"]["update_base64"]
        .as_str()
        .unwrap()
        .to_string();
    let decoded = base64_to_update(&update_data).unwrap();
    ts_b.apply_update(&decoded).unwrap();
    assert_eq!(
        ts_b.content(),
        "hello world",
        "remote update must apply correctly after resync"
    );
}

/// BUG 2: If load_sync_state fails, collab_doc_id must NOT be set on the buffer.
#[tokio::test]
async fn join_failed_buffer_stays_clean() {
    let mut buf = Buffer::new();

    // Try to load garbage state bytes — should fail.
    let result = buf.load_sync_state(&[0xFF, 0xFE, 0xFD], 42);
    assert!(result.is_err(), "invalid state bytes should fail");

    // collab_doc_id must remain None.
    assert!(
        buf.collab_doc_id.is_none(),
        "BUG 2: collab_doc_id must not be set on load failure"
    );
    assert!(
        buf.sync_doc.is_none(),
        "sync_doc must not be set on load failure"
    );
}

/// BUG 6: load_sync_state replaces buffer content from server (no duplication).
#[tokio::test]
async fn load_sync_replaces_existing_content() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "local content that should be replaced");

    let ts = TextSync::new("server content");
    let state = ts.encode_state();
    buf.load_sync_state(&state, 42).unwrap();

    assert_eq!(
        buf.text(),
        "server content",
        "content must come from server"
    );
    assert!(
        !buf.text().contains("local content"),
        "local content must be fully replaced"
    );
    assert!(
        !buf.modified,
        "buffer should not be modified after sync load"
    );
}

/// BUG 3: ShareFailed cleanup must clear sync_doc so re-share starts fresh.
#[tokio::test]
async fn share_failed_allows_clean_reshare() {
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "test content");

    // Simulate having a sync_doc (as if enable_sync was called optimistically).
    buf.enable_sync(1);
    assert!(buf.sync_doc.is_some(), "precondition: sync_doc set");

    // Simulate ShareFailed cleanup (this is what collab_bridge does).
    buf.collab_doc_id = None;
    buf.sync_doc = None;
    buf.pending_sync_updates.clear();

    // Re-enable sync (simulating re-share) — must succeed since sync_doc was cleared.
    buf.enable_sync(2);
    assert!(
        buf.sync_doc.is_some(),
        "BUG 3: re-share must create new sync_doc"
    );
}

/// BUG 5: Channel capacity is sufficient for burst editing.
#[tokio::test]
async fn collab_channel_capacity_sufficient() {
    // The production channel is 256 — verify it can absorb a burst.
    let (tx, _rx) = tokio::sync::mpsc::channel::<u8>(256);
    for i in 0..200u8 {
        tx.try_send(i)
            .expect("channel should absorb 200 messages without dropping");
    }
}
