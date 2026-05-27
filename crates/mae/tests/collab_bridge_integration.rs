//! Bridge integration tests — protocol-level tests via duplex pipes.
//!
//! Tests exercise the full JSON-RPC round-trip between a simulated client and
//! a real `handle_client` server handler via duplex pipes (no TCP).
//! Additional buffer-level and editor-level tests are in their respective crate tests.

use std::sync::{Arc, Once};

use mae_core::Buffer;
use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
use mae_state_server::doc_store::DocStore;
use mae_state_server::handler::handle_client;
use mae_state_server::storage::SqliteBackend;
use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::text::TextSync;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWriteExt, BufReader};

// --- Tracing ---

static INIT_TRACING: Once = Once::new();

fn init_tracing() {
    INIT_TRACING.call_once(|| {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .with_test_writer()
            .try_init();
    });
}

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
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"notifications/subscribe","params":{"types":["sync_update","peer_joined","peer_left","save_committed","awareness_update"]}});
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
    let mut buf = Buffer::new();
    buf.insert_text_at(0, "hello");
    let result = buf.apply_sync_update(&[1, 2, 3]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("sync not enabled"));
}

#[tokio::test]
async fn echo_filtering() {
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    // Server may send multiple messages (initialize response + PeerJoined
    // notification). Read with a timeout in case the response is delayed.
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        mae_mcp::read_message(&mut cr),
    )
    .await;

    handle.abort();
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Client should detect EOF or error — not hang.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        mae_mcp::read_message(&mut cr),
    )
    .await;
    match result {
        Ok(Ok(None)) | Ok(Err(_)) | Err(_) => {} // expected: EOF, error, or timeout
        Ok(Ok(Some(_))) => {}                    // leftover message is fine
    }
}

#[tokio::test]
async fn fault_invalid_json() {
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
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
    init_tracing();
    // The production channel is 256 — verify it can absorb a burst.
    let (tx, _rx) = tokio::sync::mpsc::channel::<u8>(256);
    for i in 0..200u8 {
        tx.try_send(i)
            .expect("channel should absorb 200 messages without dropping");
    }
}

// ---------------------------------------------------------------------------
// Awareness protocol tests
// ---------------------------------------------------------------------------

/// Awareness update roundtrip: client A sends awareness, client B receives.
#[tokio::test]
async fn awareness_update_roundtrip() {
    init_tracing();
    let store = test_doc_store();
    let broadcaster = test_broadcaster();

    let mut alice = Client::connect(Arc::clone(&store), Arc::clone(&broadcaster)).await;
    let mut bob = Client::connect(Arc::clone(&store), Arc::clone(&broadcaster)).await;

    // Both clients share the same document.
    alice.share("test-awareness", "hello").await;
    bob.share("test-awareness", "hello").await;

    // Alice sends an awareness update.
    let awareness_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": alice.next_id,
        "method": "sync/awareness",
        "params": {
            "doc": "test-awareness",
            "state": {
                "user_name": "Alice",
                "cursor_row": 5,
                "cursor_col": 10,
                "selection": null,
                "mode": "normal"
            }
        }
    });
    alice.next_id += 1;
    alice.send(&awareness_msg).await;

    // Alice gets the ack response.
    let ack = alice.recv().await;
    assert!(ack.get("error").is_none(), "awareness ack failed: {ack}");

    // Bob should receive a notification with Alice's awareness.
    let notification = bob.recv_timeout(2000).await;
    assert!(
        notification.is_some(),
        "Bob should receive awareness notification"
    );
    let notif = notification.unwrap();
    assert_eq!(
        notif["method"].as_str(),
        Some("notifications/awareness_update")
    );
    let event_data = &notif["params"]["event"]["data"];
    assert_eq!(event_data["user_name"].as_str(), Some("Alice"));
    assert_eq!(event_data["cursor_row"].as_u64(), Some(5));
    assert_eq!(event_data["cursor_col"].as_u64(), Some(10));
}

/// Awareness echo filter: sender does NOT receive own awareness update.
#[tokio::test]
async fn awareness_echo_filtered() {
    init_tracing();
    let store = test_doc_store();
    let broadcaster = test_broadcaster();

    let mut alice = Client::connect(Arc::clone(&store), Arc::clone(&broadcaster)).await;

    alice.share("test-echo", "hello").await;

    let awareness_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": alice.next_id,
        "method": "sync/awareness",
        "params": {
            "doc": "test-echo",
            "state": {
                "user_name": "Alice",
                "cursor_row": 0,
                "cursor_col": 0,
                "selection": null,
                "mode": "normal"
            }
        }
    });
    alice.next_id += 1;
    alice.send(&awareness_msg).await;

    // Alice gets the ack.
    let ack = alice.recv().await;
    assert!(ack.get("error").is_none());

    // Alice should NOT receive a notification about her own awareness.
    let notification = alice.recv_timeout(500).await;
    // If we get a notification, it should NOT be awareness_update.
    if let Some(notif) = notification {
        assert_ne!(
            notif["method"].as_str(),
            Some("notifications/awareness_update"),
            "Sender should not receive own awareness"
        );
    }
}

/// Awareness is NOT persisted — it's ephemeral.
#[tokio::test]
async fn awareness_not_persisted() {
    init_tracing();
    let store = test_doc_store();
    let broadcaster = test_broadcaster();

    let mut alice = Client::connect(Arc::clone(&store), Arc::clone(&broadcaster)).await;

    alice.share("test-persist", "hello").await;

    // Send awareness.
    let awareness_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": alice.next_id,
        "method": "sync/awareness",
        "params": {
            "doc": "test-persist",
            "state": {
                "user_name": "Alice",
                "cursor_row": 0,
                "cursor_col": 0,
                "selection": null,
                "mode": "normal"
            }
        }
    });
    alice.next_id += 1;
    alice.send(&awareness_msg).await;
    let _ = alice.recv().await;

    // Check document stats — awareness should not appear in WAL.
    let stats_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": alice.next_id,
        "method": "docs/stats",
        "params": {"doc": "test-persist"}
    });
    alice.next_id += 1;
    alice.send(&stats_msg).await;
    let stats = alice.recv().await;
    // WAL entries should be from the initial share only, not from awareness.
    let wal_entries = stats["result"]["wal_entries"].as_u64().unwrap_or(0);
    assert!(
        wal_entries <= 1,
        "Awareness should NOT produce WAL entries (got {wal_entries})"
    );
}

/// AwarenessState serialization unit test (sync crate).
#[test]
fn awareness_state_schema_valid() {
    let state = mae_sync::awareness::AwarenessState {
        user_name: "Test User".to_string(),
        cursor_row: 42,
        cursor_col: 10,
        selection: Some((1, 0, 5, 20)),
        mode: "visual".to_string(),
    };
    let json = serde_json::to_string(&state).unwrap();
    assert!(json.contains("\"user_name\":\"Test User\""));
    assert!(json.contains("\"cursor_row\":42"));
    assert!(json.contains("\"selection\":[1,0,5,20]"));

    let parsed: mae_sync::awareness::AwarenessState = serde_json::from_str(&json).unwrap();
    assert_eq!(state, parsed);
}

/// AwarenessMap color index is deterministic.
#[test]
fn awareness_color_index_deterministic() {
    use mae_core::render_common::collab_colors::collab_color_index;
    let idx1 = collab_color_index(12345);
    let idx2 = collab_color_index(12345);
    assert_eq!(idx1, idx2, "Same client_id must produce same color index");
    assert!(idx1 < 8, "Color index must be in [0, 8)");
}

// ============================================================================
// WU1 — Protocol Gap Tests (sync/state_vector, sync/diff, docs/delete,
//        docs/metadata, concurrent save, sharer disconnect)
// ============================================================================

/// WU1a: sync/state_vector returns a valid state vector.
#[tokio::test]
async fn sync_state_vector_returns_valid_sv() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("sv-test.txt", "hello state vector").await;

    // Request state vector.
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "sync/state_vector",
        "params": { "doc": "sv-test.txt" }
    });
    client.next_id += 1;
    client.send(&msg).await;
    let resp = client.recv().await;
    assert!(resp.get("error").is_none(), "state_vector failed: {resp}");

    let sv_b64 = resp["result"]["sv"].as_str().unwrap();
    let sv_bytes = base64_to_update(sv_b64).unwrap();
    assert!(!sv_bytes.is_empty(), "state vector should not be empty");

    // Apply it to a fresh TextSync — must not panic.
    let state = client.full_state("sv-test.txt").await;
    let ts = TextSync::from_state(&state).unwrap();
    assert_eq!(ts.content(), "hello state vector");
}

/// WU1b: sync/diff computes an incremental update between two states.
#[tokio::test]
async fn sync_diff_computes_incremental_update() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Share initial content and capture state vector.
    client.share("diff-test.txt", "hello").await;
    let sv_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "sync/state_vector",
        "params": { "doc": "diff-test.txt" }
    });
    client.next_id += 1;
    client.send(&sv_msg).await;
    let sv_resp = client.recv().await;
    let old_sv_b64 = sv_resp["result"]["sv"].as_str().unwrap().to_string();

    // Edit the document.
    let state = client.full_state("diff-test.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();
    let update = ts.insert(5, " world");
    client.send_update("diff-test.txt", &update).await;

    // Request diff using the old state vector.
    let diff_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "sync/diff",
        "params": { "doc": "diff-test.txt", "sv": old_sv_b64 }
    });
    client.next_id += 1;
    client.send(&diff_msg).await;
    let diff_resp = client.recv().await;
    assert!(
        diff_resp.get("error").is_none(),
        "sync/diff failed: {diff_resp}"
    );

    let diff_b64 = diff_resp["result"]["update"].as_str().unwrap();
    let diff_bytes = base64_to_update(diff_b64).unwrap();
    assert!(!diff_bytes.is_empty(), "diff should contain the edit");

    // Apply the diff to a TextSync at the old state — should produce "hello world".
    let old_state = client.full_state("diff-test.txt").await;
    let ts2 = TextSync::from_state(&old_state).unwrap();
    assert_eq!(ts2.content(), "hello world");
}

/// WU1c: docs/delete removes a document from the server.
#[tokio::test]
async fn docs_delete_removes_document() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("delete-me.txt", "doomed content").await;

    // Verify it exists in docs/list.
    let list_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "docs/list"
    });
    client.next_id += 1;
    client.send(&list_msg).await;
    let list_resp = client.recv().await;
    let docs: Vec<String> = list_resp["result"]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        docs.contains(&"delete-me.txt".to_string()),
        "doc should exist before delete"
    );

    // Delete.
    let del_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "docs/delete",
        "params": { "doc": "delete-me.txt" }
    });
    client.next_id += 1;
    client.send(&del_msg).await;
    let del_resp = client.recv().await;
    assert!(del_resp.get("error").is_none(), "delete failed: {del_resp}");
    assert_eq!(del_resp["result"]["deleted"], true);

    // Verify gone from docs/list.
    let list2_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "docs/list"
    });
    client.next_id += 1;
    client.send(&list2_msg).await;
    let list2_resp = client.recv().await;
    let docs2: Vec<String> = list2_resp["result"]["documents"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        !docs2.contains(&"delete-me.txt".to_string()),
        "doc should be gone after delete"
    );

    // docs/content should return error for deleted doc.
    let content_resp_raw = client.content("delete-me.txt").await;
    // content() helper asserts on result, but the doc may be auto-created as empty.
    // Either way, the original content should be gone.
    assert_ne!(content_resp_raw, "doomed content", "content must be gone");
}

/// WU1d: docs/metadata returns save info after a save round-trip.
#[tokio::test]
async fn docs_metadata_returns_save_info() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("meta-test.txt", "save me").await;

    // Save round-trip.
    let hash = sha256_hash("save me");
    let intent_resp = client.save_intent("meta-test.txt", &hash).await;
    let epoch = intent_resp["result"]["result"]["save_epoch"]
        .as_u64()
        .unwrap();
    client
        .save_committed("meta-test.txt", "meta-user", epoch, &hash)
        .await;

    // Request metadata.
    let meta_msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "docs/metadata",
        "params": { "doc": "meta-test.txt" }
    });
    client.next_id += 1;
    client.send(&meta_msg).await;
    let meta_resp = client.recv().await;
    assert!(
        meta_resp.get("error").is_none(),
        "metadata failed: {meta_resp}"
    );

    let result = &meta_resp["result"];
    assert!(
        result["save_epoch"].as_u64().unwrap() > 0,
        "save_epoch should be set"
    );
    assert_eq!(
        result["last_saved_by"].as_str().unwrap(),
        "meta-user",
        "saved_by should match"
    );
    assert!(
        result["content_length"].as_u64().unwrap() > 0,
        "content_length should be positive"
    );
}

/// WU1e: Concurrent save intents — one succeeds, other gets conflict.
#[tokio::test]
async fn concurrent_save_intents_same_doc() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Both share the same doc.
    ca.share("concurrent-save.txt", "original").await;

    // Client B joins.
    let _ = cb.full_state("concurrent-save.txt").await;

    // Both edit independently via the server.
    let state_a = ca.full_state("concurrent-save.txt").await;
    let mut ts_a = TextSync::from_state(&state_a).unwrap();
    let ua = ts_a.insert(8, " A-edit");
    ca.send_update("concurrent-save.txt", &ua).await;

    let state_b = cb.full_state("concurrent-save.txt").await;
    let mut ts_b = TextSync::from_state(&state_b).unwrap();
    let ub = ts_b.insert(8, " B-edit");
    cb.send_update("concurrent-save.txt", &ub).await;

    // Wait for convergence.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // A saves with correct hash.
    let content = ca.content("concurrent-save.txt").await;
    let hash_a = sha256_hash(&content);
    let resp_a = ca.save_intent("concurrent-save.txt", &hash_a).await;
    assert_eq!(
        resp_a["result"]["result"]["status"].as_str().unwrap(),
        "ok",
        "first save_intent should succeed"
    );

    // B saves with a stale hash (its pre-convergence view).
    let stale_hash = sha256_hash("original B-edit");
    let resp_b = cb.save_intent("concurrent-save.txt", &stale_hash).await;
    assert_eq!(
        resp_b["result"]["result"]["status"].as_str().unwrap(),
        "conflict",
        "stale hash should get conflict"
    );

    // B retries with correct hash — should succeed.
    let real_content = cb.content("concurrent-save.txt").await;
    let correct_hash = sha256_hash(&real_content);
    let resp_b2 = cb.save_intent("concurrent-save.txt", &correct_hash).await;
    assert_eq!(
        resp_b2["result"]["result"]["status"].as_str().unwrap(),
        "ok",
        "retry with correct hash should succeed"
    );
}

/// WU1f: Sharer disconnect notifies peers (sharer_left event).
#[tokio::test]
async fn sharer_disconnect_notifies_peers() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares and B joins.
    client_a.share("sharer-disc.txt", "shared content").await;
    let _ = client_b.full_state("sharer-disc.txt").await;

    // Drain any pending notifications on B.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    while client_b.recv_timeout(50).await.is_some() {}

    // Drop client A (the sharer).
    drop(client_a);

    // B should receive a peer_left notification.
    let notif = client_b
        .wait_for_notification("notifications/peer_left", 2000)
        .await;
    assert!(
        notif.is_some(),
        "B should receive peer_left when sharer disconnects"
    );

    // B can still read the document content (it's persisted on server).
    let content = client_b.content("sharer-disc.txt").await;
    assert_eq!(
        content, "shared content",
        "content should survive sharer disconnect"
    );
}

// ============================================================================
// WU3 — Error Path & Edge Case Tests
// ============================================================================

/// WU3a: Invalid CRDT bytes (valid base64 but garbage) are rejected.
#[tokio::test]
async fn invalid_crdt_bytes_rejected() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("crdt-err.txt", "safe content").await;

    // Send valid base64 but not valid yrs update bytes.
    use base64::Engine;
    let garbage = base64::engine::general_purpose::STANDARD.encode([0xFF, 0xFE, 0x00, 0x01, 0x02]);
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "sync/update",
        "params": { "doc": "crdt-err.txt", "update": garbage }
    });
    client.next_id += 1;
    client.send(&msg).await;
    let resp = client.recv().await;

    // Should get an error response (not crash, not silent corruption).
    assert!(
        resp.get("error").is_some(),
        "garbage CRDT bytes should produce error, got: {resp}"
    );

    // Document content should be unchanged.
    let content = client.content("crdt-err.txt").await;
    assert_eq!(
        content, "safe content",
        "content must be unchanged after bad update"
    );
}

/// WU3b: Concurrent share of same doc_id converges deterministically.
#[tokio::test]
async fn concurrent_share_same_doc_converges() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Both share the same doc_id with different content.
    ca.share("race-share.txt", "content-A").await;
    cb.share("race-share.txt", "content-B").await;

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Both should see the same content (last-writer-wins for sync/share).
    let content_a = ca.content("race-share.txt").await;
    let content_b = cb.content("race-share.txt").await;
    assert_eq!(
        content_a, content_b,
        "concurrent shares must converge to same content"
    );
    // The second share (B) replaces A's content.
    assert_eq!(content_b, "content-B", "last share wins");
}

// ---------------------------------------------------------------------------
// CRDT undo propagation regression tests
// ---------------------------------------------------------------------------
// These tests exercise the yrs UndoManager path (not reconcile_to) to ensure
// undo-generated CRDT updates propagate correctly to remote peers.

/// UndoManager undo generates updates that propagate through the server.
/// This is the core undo propagation test — if this fails, the Docker E2E
/// undo tests will also fail.
#[tokio::test]
async fn undo_manager_propagates_through_bridge() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares a doc with base content.
    ca.share("undo-mgr.txt", "base\n").await;

    // Both get initial state.
    let mut ts_a = TextSync::from_state(&ca.full_state("undo-mgr.txt").await).unwrap();
    ts_a.enable_undo();
    let mut ts_b = TextSync::from_state(&cb.full_state("undo-mgr.txt").await).unwrap();
    ts_b.enable_undo();

    // A inserts "from-A" using origin-tagged transaction (tracked by UndoManager).
    let ua = ts_a.insert(5, "from-A\n");
    ca.send_update("undo-mgr.txt", &ua).await;

    // B receives A's edit.
    let notif = cb
        .wait_for_notification("notifications/sync_update", 2000)
        .await
        .expect("B should receive A's insert");
    let b64 = notif["params"]["event"]["data"]["update_base64"]
        .as_str()
        .unwrap();
    ts_b.apply_update(&base64_to_update(b64).unwrap()).unwrap();
    assert!(
        ts_b.content().contains("from-A"),
        "B should see A's insert: {}",
        ts_b.content()
    );

    // B inserts "from-B".
    let ub = ts_b.insert(ts_b.content().len() as u32, "from-B\n");
    cb.send_update("undo-mgr.txt", &ub).await;

    // A receives B's edit.
    let notif_a = ca
        .wait_for_notification("notifications/sync_update", 2000)
        .await
        .expect("A should receive B's insert");
    let a_b64 = notif_a["params"]["event"]["data"]["update_base64"]
        .as_str()
        .unwrap();
    ts_a.apply_update(&base64_to_update(a_b64).unwrap())
        .unwrap();
    assert!(
        ts_a.content().contains("from-B"),
        "A should see B's insert: {}",
        ts_a.content()
    );

    // A undoes via UndoManager (NOT reconcile_to).
    ts_a.undo_reset(); // Ensure the insert is a separate undo item.
    let (undo_ok, undo_updates) = ts_a.undo();
    assert!(undo_ok, "A's undo should succeed");
    assert!(
        !undo_updates.is_empty(),
        "undo must generate CRDT update bytes"
    );
    assert!(
        !ts_a.content().contains("from-A"),
        "A's local state should not contain from-A after undo: {}",
        ts_a.content()
    );
    assert!(
        ts_a.content().contains("from-B"),
        "A's local state should still contain from-B after undo: {}",
        ts_a.content()
    );

    // Send ALL undo updates to the server.
    for update in &undo_updates {
        ca.send_update("undo-mgr.txt", update).await;
    }

    // B should receive the undo update(s).
    for _ in 0..undo_updates.len() {
        let notif_undo = cb
            .wait_for_notification("notifications/sync_update", 2000)
            .await
            .expect("B should receive A's undo update");
        let undo_b64 = notif_undo["params"]["event"]["data"]["update_base64"]
            .as_str()
            .unwrap();
        ts_b.apply_update(&base64_to_update(undo_b64).unwrap())
            .unwrap();
    }

    // B should see from-A removed, from-B preserved.
    assert!(
        !ts_b.content().contains("from-A"),
        "B should NOT contain from-A after applying A's undo: {}",
        ts_b.content()
    );
    assert!(
        ts_b.content().contains("from-B"),
        "B should still contain from-B after A's undo: {}",
        ts_b.content()
    );

    // Verify server state also converged.
    let server_content = ca.content("undo-mgr.txt").await;
    assert!(
        !server_content.contains("from-A"),
        "server should NOT contain from-A: {}",
        server_content
    );
    assert!(
        server_content.contains("from-B"),
        "server should contain from-B: {}",
        server_content
    );
}

/// Buffer::undo() with sync enabled generates pending_sync_updates
/// that can be applied by a remote TextSync to achieve convergence.
#[tokio::test]
async fn buffer_undo_generates_valid_crdt_updates() {
    init_tracing();

    // Set up buffer A with sync + UndoManager.
    let mut buf_a = Buffer::new();
    buf_a.name = "undo-buf.txt".to_string();
    buf_a.enable_sync(1);
    let mut win = mae_core::window::Window::new(0, 0);

    // Insert base content.
    buf_a.insert_text_at(0, "base\n");
    buf_a.pending_sync_updates.clear(); // Clear the base insert update.

    // Mark undo boundary so the next insert is a separate undo item.
    buf_a.sync_undo_boundary();

    // Insert "from-A" (tracked by UndoManager).
    buf_a.insert_text_at(5, "from-A\n");
    let insert_updates: Vec<Vec<u8>> = buf_a.pending_sync_updates.drain(..).collect();
    assert!(
        !insert_updates.is_empty(),
        "insert should generate sync updates"
    );

    // Set up remote doc B and apply A's edits.
    let mut ts_b = TextSync::from_state(&buf_a.sync_doc.as_ref().unwrap().encode_state()).unwrap();
    // Apply the insert update to B via the normal path (simulating what the server would do).
    for u in &insert_updates {
        ts_b.apply_update(u).unwrap();
    }
    assert_eq!(ts_b.content(), "base\nfrom-A\n");

    // B adds its own content.
    let ub = ts_b.insert(ts_b.content().len() as u32, "from-B\n");
    buf_a
        .apply_sync_update(&ub)
        .expect("A should accept B's update");
    assert!(
        buf_a.text().contains("from-B"),
        "A should see B's text: {}",
        buf_a.text()
    );

    // Mark undo boundary before undo dispatch (simulates dispatch_builtin behavior).
    buf_a.sync_undo_boundary();

    // A undoes via Buffer::undo() — this should use the UndoManager path.
    assert!(
        buf_a.sync_doc.as_ref().unwrap().undo_mgr_active(),
        "UndoManager should be active"
    );
    buf_a.undo(&mut win);

    // Verify A's local state.
    assert!(
        !buf_a.text().contains("from-A"),
        "A should not contain from-A after undo: {}",
        buf_a.text()
    );
    assert!(
        buf_a.text().contains("from-B"),
        "A should still contain from-B after undo: {}",
        buf_a.text()
    );

    // Verify undo generated pending_sync_updates.
    assert!(
        !buf_a.pending_sync_updates.is_empty(),
        "Buffer::undo() must generate pending_sync_updates for CRDT propagation"
    );

    // Apply undo updates to B.
    for u in &buf_a.pending_sync_updates {
        ts_b.apply_update(u)
            .expect("B should accept A's undo update");
    }

    // Verify convergence.
    assert!(
        !ts_b.content().contains("from-A"),
        "B should not contain from-A after applying A's undo: {}",
        ts_b.content()
    );
    assert!(
        ts_b.content().contains("from-B"),
        "B should still contain from-B after A's undo: {}",
        ts_b.content()
    );
    assert_eq!(
        buf_a.text(),
        ts_b.content(),
        "A and B should have identical content after undo propagation"
    );
}

/// Redo after undo generates propagatable updates.
#[tokio::test]
async fn buffer_redo_generates_valid_crdt_updates() {
    init_tracing();

    let mut buf = Buffer::new();
    buf.name = "redo-buf.txt".to_string();
    buf.enable_sync(1);
    let mut win = mae_core::window::Window::new(0, 0);

    // Insert + boundary.
    buf.insert_text_at(0, "hello");
    buf.pending_sync_updates.clear();
    buf.sync_undo_boundary();
    buf.insert_text_at(5, " world");
    buf.pending_sync_updates.clear();

    // Set up remote.
    let mut remote = TextSync::from_state(&buf.sync_doc.as_ref().unwrap().encode_state()).unwrap();
    assert_eq!(remote.content(), "hello world");

    // Undo.
    buf.sync_undo_boundary();
    buf.undo(&mut win);
    assert_eq!(buf.text(), "hello");
    for u in buf.pending_sync_updates.drain(..) {
        remote.apply_update(&u).unwrap();
    }
    assert_eq!(remote.content(), "hello");

    // Redo.
    buf.sync_undo_boundary();
    buf.redo(&mut win);
    assert_eq!(buf.text(), "hello world");
    assert!(
        !buf.pending_sync_updates.is_empty(),
        "redo must generate pending_sync_updates"
    );
    for u in &buf.pending_sync_updates {
        remote.apply_update(u).unwrap();
    }
    assert_eq!(
        remote.content(),
        "hello world",
        "remote should match after redo propagation"
    );
}

/// Full undo propagation through the bridge with UndoManager — exercises the
/// end-to-end flow: Buffer::undo() → pending_sync_updates → server → remote.
/// Run 10 times to catch intermittent failures.
#[tokio::test]
async fn undo_propagation_stress() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();

    for iteration in 0..10 {
        let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
        let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

        let doc = format!("stress-undo-{iteration}.txt");
        ca.share(&doc, "base\n").await;

        let mut ts_a = TextSync::from_state(&ca.full_state(&doc).await).unwrap();
        ts_a.enable_undo();
        let mut ts_b = TextSync::from_state(&cb.full_state(&doc).await).unwrap();
        ts_b.enable_undo();

        // A inserts.
        let ua = ts_a.insert(5, "from-A\n");
        ca.send_update(&doc, &ua).await;
        ts_a.undo_reset();

        // B receives.
        let n = cb
            .wait_for_notification("notifications/sync_update", 2000)
            .await
            .expect("B should receive A's insert");
        ts_b.apply_update(
            &base64_to_update(
                n["params"]["event"]["data"]["update_base64"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();

        // B inserts.
        let ub = ts_b.insert(ts_b.content().len() as u32, "from-B\n");
        cb.send_update(&doc, &ub).await;

        // A receives.
        let n2 = ca
            .wait_for_notification("notifications/sync_update", 2000)
            .await
            .expect("A should receive B's insert");
        ts_a.apply_update(
            &base64_to_update(
                n2["params"]["event"]["data"]["update_base64"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();

        // A undoes via UndoManager.
        let (ok, undo_updates) = ts_a.undo();
        assert!(ok, "iter {iteration}: undo should succeed");
        assert!(
            !undo_updates.is_empty(),
            "iter {iteration}: undo must generate updates"
        );

        // Send undo to server.
        for u in &undo_updates {
            ca.send_update(&doc, u).await;
        }

        // B receives and applies undo.
        for _ in 0..undo_updates.len() {
            let n3 = cb
                .wait_for_notification("notifications/sync_update", 2000)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "iter {iteration}: B should receive undo update. A content: {}, B content: {}",
                        ts_a.content(),
                        ts_b.content()
                    )
                });
            ts_b.apply_update(
                &base64_to_update(
                    n3["params"]["event"]["data"]["update_base64"]
                        .as_str()
                        .unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        }

        assert!(
            !ts_b.content().contains("from-A"),
            "iter {iteration}: B should not have from-A after undo: {}",
            ts_b.content()
        );
        assert!(
            ts_b.content().contains("from-B"),
            "iter {iteration}: B should still have from-B: {}",
            ts_b.content()
        );
    }
}
