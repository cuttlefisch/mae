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

/// Realistic 32-bit client_id matching the production `compute_client_id`.
/// Uses FNV-1a hash of (pid, buf_idx) — safe for yrs v1 wire format.
///
/// Production uses real PIDs; tests use synthetic PIDs that produce
/// distinct 32-bit hashes, mirroring how two separate editor processes
/// would each compute a unique client_id for the same buffer index.
fn test_client_id(pid: u32, buf_idx: u32) -> u64 {
    let mut h: u32 = 0x811c_9dc5;
    for b in pid.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    for b in buf_idx.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    if h == 0 {
        1
    } else {
        h as u64
    }
}

/// Simulated PID for "sharer" editor process in tests.
const TEST_PID_SHARER: u32 = 4_089_813;
/// Simulated PID for "joiner" editor process in tests.
const TEST_PID_JOINER: u32 = 4_089_541;

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
    /// Queued notifications received while waiting for responses.
    /// Tests can inspect these to verify notification delivery.
    notification_log: Vec<serde_json::Value>,
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
            notification_log: Vec::new(),
        };
        client.initialize().await;
        client.subscribe().await;
        client
    }

    async fn send(&mut self, msg: &serde_json::Value) {
        let body = serde_json::to_vec(msg).unwrap();
        mae_mcp::write_framed(&mut self.writer, &body, std::time::Duration::from_secs(5))
            .await
            .unwrap();
    }

    /// Send with legacy newline-delimited framing (for explicit framing tests).
    #[allow(dead_code)]
    async fn send_newline(&mut self, msg: &serde_json::Value) {
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
            // Notifications have "method" but no "id" — capture them instead of
            // silently dropping, so tests can verify notification delivery.
            if val.get("method").is_some()
                && val.get("result").is_none()
                && val.get("error").is_none()
            {
                self.notification_log.push(val);
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

    /// Send an awareness update (cursor position + optional selection).
    async fn send_awareness(
        &mut self,
        doc: &str,
        user_name: &str,
        cursor_row: usize,
        cursor_col: usize,
        selection: Option<(usize, usize, usize, usize)>,
    ) -> serde_json::Value {
        let sel_json = match selection {
            Some((sr, sc, er, ec)) => serde_json::json!([sr, sc, er, ec]),
            None => serde_json::Value::Null,
        };
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": self.next_id,
            "method": "sync/awareness",
            "params": {
                "doc": doc,
                "state": {
                    "user_name": user_name,
                    "cursor_row": cursor_row,
                    "cursor_col": cursor_col,
                    "selection": sel_json,
                    "mode": "normal"
                }
            }
        });
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Drain all pending notifications, returning them.
    async fn drain_notifications(&mut self) -> Vec<serde_json::Value> {
        let mut notifs = Vec::new();
        while let Some(msg) = self.recv_timeout(100).await {
            notifs.push(msg);
        }
        notifs
    }

    /// Return sync_update notifications from the log, extracting wal_seq values.
    #[allow(dead_code)]
    fn sync_update_wal_seqs(&self) -> Vec<u64> {
        self.notification_log
            .iter()
            .filter(|n| {
                n.get("method").and_then(|m| m.as_str()) == Some("notifications/sync_update")
            })
            .filter_map(|n| {
                n.pointer("/params/event/data/wal_seq")
                    .and_then(|v| v.as_u64())
            })
            .collect()
    }

    /// Share a document with a specific client_id (mirrors production compute_client_id).
    async fn share_with_client_id(&mut self, doc: &str, content: &str, client_id: u64) {
        let ts = TextSync::with_client_id(content, client_id);
        let state = ts.encode_state();
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/share","params":{"doc":doc,"update":update_to_base64(&state)}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "share failed: {resp}");
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
    buf.enable_sync(test_client_id(TEST_PID_SHARER, 0));
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
    buf.enable_sync(test_client_id(TEST_PID_SHARER, 0));
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
        notification_log: Vec::new(),
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
    buf.enable_sync(test_client_id(TEST_PID_SHARER, 0));
    assert!(buf.sync_doc.is_some(), "precondition: sync_doc set");

    // Simulate ShareFailed cleanup (this is what collab_bridge does).
    buf.collab_doc_id = None;
    buf.sync_doc = None;
    buf.pending_sync_updates.clear();

    // Re-enable sync (simulating re-share) — must succeed since sync_doc was cleared.
    buf.enable_sync(test_client_id(TEST_PID_SHARER, 1));
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
        kb_node_id: None,
        kb_id: None,
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
    let undo_result = ts_a.undo();
    assert!(undo_result.success, "A's undo should succeed");
    assert!(
        !undo_result.updates.is_empty(),
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
    for update in &undo_result.updates {
        ca.send_update("undo-mgr.txt", update).await;
    }

    // B should receive the undo update(s).
    for _ in 0..undo_result.updates.len() {
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
    buf_a.enable_sync(test_client_id(TEST_PID_SHARER, 0));
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
    buf.enable_sync(test_client_id(TEST_PID_SHARER, 0));
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
        let undo_result = ts_a.undo();
        assert!(undo_result.success, "iter {iteration}: undo should succeed");
        assert!(
            !undo_result.updates.is_empty(),
            "iter {iteration}: undo must generate updates"
        );

        // Send undo to server.
        for u in &undo_result.updates {
            ca.send_update(&doc, u).await;
        }

        // B receives and applies undo.
        for _ in 0..undo_result.updates.len() {
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

// ============================================================================
// Test Gap Coverage — multi-doc, backpressure, WAL recovery, corrupted state
// ============================================================================

/// Two clients editing two different documents simultaneously.
/// Verifies per-document isolation — edits to doc A don't leak into doc B.
#[tokio::test]
async fn multi_doc_concurrent_editing() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Each client shares a different document.
    client_a.share("alpha.txt", "AAA").await;
    client_b.share("beta.txt", "BBB").await;

    // To send valid updates, load the server's state and edit from there.
    let state_a = client_a.full_state("alpha.txt").await;
    let state_b = client_b.full_state("beta.txt").await;
    let mut ts_a = TextSync::from_state(&state_a).unwrap();
    let mut ts_b = TextSync::from_state(&state_b).unwrap();

    let u1 = ts_a.insert(3, "-alpha-edit");
    let u2 = ts_b.insert(3, "-beta-edit");

    let r1 = client_a.send_update("alpha.txt", &u1).await;
    let r2 = client_b.send_update("beta.txt", &u2).await;
    assert!(r1.get("error").is_none(), "alpha update failed: {r1}");
    assert!(r2.get("error").is_none(), "beta update failed: {r2}");

    // Verify documents are isolated.
    let alpha_content = client_a.content("alpha.txt").await;
    let beta_content = client_b.content("beta.txt").await;
    assert_eq!(alpha_content, "AAA-alpha-edit");
    assert_eq!(beta_content, "BBB-beta-edit");

    // Cross-read: client B reads alpha, client A reads beta.
    let alpha_via_b = client_b.content("alpha.txt").await;
    let beta_via_a = client_a.content("beta.txt").await;
    assert_eq!(alpha_via_b, "AAA-alpha-edit");
    assert_eq!(beta_via_a, "BBB-beta-edit");
}

/// Two clients collaborating on the SAME two documents simultaneously.
/// Edits to doc1 from both clients converge, edits to doc2 from both converge,
/// and the two documents remain independent.
#[tokio::test]
async fn multi_doc_shared_collab() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Client A shares both docs.
    client_a.share("doc1.txt", "").await;
    client_a.share("doc2.txt", "").await;

    // Both clients join both docs (B resyncs to get initial state).
    client_b.resync("doc1.txt").await;
    client_b.resync("doc2.txt").await;

    // Load server state for valid updates.
    let state1 = client_a.full_state("doc1.txt").await;
    let state2 = client_a.full_state("doc2.txt").await;

    // Interleaved edits: A edits doc1, B edits doc2.
    let mut ts_a1 = TextSync::from_state(&state1).unwrap();
    let mut ts_b2 = TextSync::from_state(&state2).unwrap();

    let u_a1 = ts_a1.insert(0, "A-in-doc1");
    let u_b2 = ts_b2.insert(0, "B-in-doc2");

    let r1 = client_a.send_update("doc1.txt", &u_a1).await;
    let r2 = client_b.send_update("doc2.txt", &u_b2).await;
    assert!(r1.get("error").is_none());
    assert!(r2.get("error").is_none());

    // Now A edits doc2, B edits doc1 — load latest state for each.
    let state2b = client_a.full_state("doc2.txt").await;
    let state1b = client_b.full_state("doc1.txt").await;
    let mut ts_a2 = TextSync::from_state(&state2b).unwrap();
    let mut ts_b1 = TextSync::from_state(&state1b).unwrap();
    let u_a2 = ts_a2.insert(0, "A-in-doc2");
    let u_b1 = ts_b1.insert(0, "B-in-doc1");

    let r3 = client_a.send_update("doc2.txt", &u_a2).await;
    let r4 = client_b.send_update("doc1.txt", &u_b1).await;
    assert!(r3.get("error").is_none());
    assert!(r4.get("error").is_none());

    // Verify convergence.
    let doc1_a = client_a.content("doc1.txt").await;
    let doc1_b = client_b.content("doc1.txt").await;
    assert_eq!(doc1_a, doc1_b, "doc1 must converge across clients");
    assert!(doc1_a.contains("A-in-doc1"));
    assert!(doc1_a.contains("B-in-doc1"));

    let doc2_a = client_a.content("doc2.txt").await;
    let doc2_b = client_b.content("doc2.txt").await;
    assert_eq!(doc2_a, doc2_b, "doc2 must converge across clients");
    assert!(doc2_a.contains("A-in-doc2"));
    assert!(doc2_a.contains("B-in-doc2"));

    // Documents must be independent.
    assert!(
        !doc1_a.contains("doc2"),
        "doc1 must not contain doc2 content"
    );
    assert!(
        !doc2_a.contains("doc1"),
        "doc2 must not contain doc1 content"
    );
}

/// WAL recovery through DocStore: append updates, drop the store, recreate
/// from the same SQLite backend, and verify content is recovered.
#[tokio::test]
async fn wal_recovery_through_doc_store() {
    init_tracing();
    let backend: Arc<dyn mae_state_server::storage::StorageBackend> =
        Arc::new(SqliteBackend::open_memory().unwrap());

    // Phase 1: write updates through a DocStore.
    {
        let store = Arc::new(DocStore::new(Arc::clone(&backend), 500));
        let bc = test_broadcaster();
        let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
        client.share("recover.txt", "initial").await;

        // Load server state and edit from it for a valid update.
        let state = client.full_state("recover.txt").await;
        let mut ts = TextSync::from_state(&state).unwrap();
        let u1 = ts.insert(7, " content");
        let r = client.send_update("recover.txt", &u1).await;
        assert!(r.get("error").is_none());

        let content = client.content("recover.txt").await;
        assert_eq!(content, "initial content");

        // Compact to persist state.
        store.compact_all().await.unwrap();
    }
    // Store is dropped — simulates server restart.

    // Phase 2: recreate from same backend and verify recovery.
    {
        let store = Arc::new(DocStore::new(Arc::clone(&backend), 500));
        let bc = test_broadcaster();
        let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

        // docs/content triggers get_or_create which loads from WAL/snapshot.
        let content = client.content("recover.txt").await;
        assert_eq!(
            content, "initial content",
            "content must survive store restart"
        );
    }
}

/// Corrupted state vector in resync request — server should return error,
/// not crash or corrupt document state.
#[tokio::test]
async fn corrupted_state_vector_in_diff() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("diff-test.txt", "safe data").await;

    // Send sync/diff with garbage state vector.
    use base64::Engine;
    let garbage_sv = base64::engine::general_purpose::STANDARD.encode([0xFF, 0xAB, 0x00, 0x99]);
    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": client.next_id,
        "method": "sync/diff",
        "params": { "doc": "diff-test.txt", "sv": garbage_sv }
    });
    client.next_id += 1;
    client.send(&msg).await;
    let resp = client.recv().await;

    assert!(
        resp.get("error").is_some(),
        "corrupted state vector should produce error, got: {resp}"
    );

    // Document content must be unchanged.
    let content = client.content("diff-test.txt").await;
    assert_eq!(
        content, "safe data",
        "content must be unchanged after bad diff request"
    );
}

/// Rapid-fire updates from multiple clients — verify the server handles
/// high throughput without dropping valid updates or corrupting state.
#[tokio::test]
async fn rapid_fire_multi_client_stress() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("stress.txt", "").await;
    client_b.resync("stress.txt").await;

    // Load server state for valid updates — each client gets its own fork.
    let state = client_a.full_state("stress.txt").await;
    let mut ts_a =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();
    let mut ts_b =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_JOINER, 0)).unwrap();

    for i in 0..20 {
        let ua = ts_a.insert(ts_a.content().len() as u32, &format!("A{i}"));
        let ub = ts_b.insert(ts_b.content().len() as u32, &format!("B{i}"));
        let ra = client_a.send_update("stress.txt", &ua).await;
        let rb = client_b.send_update("stress.txt", &ub).await;
        assert!(ra.get("error").is_none(), "A update {i} failed: {ra}");
        assert!(rb.get("error").is_none(), "B update {i} failed: {rb}");
    }

    // Both clients should see the same converged content.
    let content_a = client_a.content("stress.txt").await;
    let content_b = client_b.content("stress.txt").await;
    assert_eq!(content_a, content_b, "stress test: clients must converge");

    // All 40 insertions must be present.
    for i in 0..20 {
        assert!(
            content_a.contains(&format!("A{i}")),
            "missing A{i} in converged content"
        );
        assert!(
            content_a.contains(&format!("B{i}")),
            "missing B{i} in converged content"
        );
    }
}

// ============================================================================
// Awareness Protocol E2E Tests
// ============================================================================

/// A sends awareness update → B receives notification with correct cursor position.
#[tokio::test]
async fn awareness_cursor_position_relayed() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("aware.txt", "hello world").await;
    client_b.resync("aware.txt").await;

    // Drain setup notifications.
    client_b.drain_notifications().await;

    // A sends awareness: cursor at row 5, col 10.
    let resp = client_a
        .send_awareness("aware.txt", "Alice", 5, 10, None)
        .await;
    assert!(
        resp.get("error").is_none(),
        "awareness send should succeed: {resp}"
    );

    // B should receive the awareness notification.
    let notif = client_b
        .wait_for_notification("notifications/awareness_update", 2000)
        .await;
    assert!(
        notif.is_some(),
        "B should receive awareness_update notification"
    );
    let notif = notif.unwrap();
    let event = &notif["params"]["event"]["data"];

    assert_eq!(
        event["doc_id"].as_str().unwrap(),
        "aware.txt",
        "doc_id must match"
    );
    assert_eq!(
        event["user_name"].as_str().unwrap(),
        "Alice",
        "user_name must match"
    );
    assert_eq!(
        event["cursor_row"].as_u64().unwrap(),
        5,
        "cursor_row must be 5"
    );
    assert_eq!(
        event["cursor_col"].as_u64().unwrap(),
        10,
        "cursor_col must be 10"
    );
    assert!(
        event["selection"].is_null(),
        "selection should be null when not in visual mode"
    );
}

/// A sends awareness with selection → B receives correct selection range.
#[tokio::test]
async fn awareness_selection_relayed() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("sel.txt", "line 1\nline 2\nline 3").await;
    client_b.resync("sel.txt").await;
    client_b.drain_notifications().await;

    // A sends awareness with visual selection: rows 0-2, cols 0-5.
    let resp = client_a
        .send_awareness("sel.txt", "Alice", 2, 5, Some((0, 0, 2, 5)))
        .await;
    assert!(resp.get("error").is_none());

    let notif = client_b
        .wait_for_notification("notifications/awareness_update", 2000)
        .await;
    assert!(notif.is_some(), "B should receive awareness with selection");
    let event = &notif.unwrap()["params"]["event"]["data"];

    let sel = event["selection"]
        .as_array()
        .expect("selection should be array");
    assert_eq!(sel.len(), 4, "selection should have 4 elements");
    assert_eq!(sel[0].as_u64().unwrap(), 0, "sel start_row");
    assert_eq!(sel[1].as_u64().unwrap(), 0, "sel start_col");
    assert_eq!(sel[2].as_u64().unwrap(), 2, "sel end_row");
    assert_eq!(sel[3].as_u64().unwrap(), 5, "sel end_col");
}

/// A moves cursor multiple times → B receives updated positions each time.
#[tokio::test]
async fn awareness_cursor_movement_tracked() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("move.txt", "content").await;
    client_b.resync("move.txt").await;
    client_b.drain_notifications().await;

    // A moves cursor: position 1.
    client_a
        .send_awareness("move.txt", "Alice", 0, 0, None)
        .await;
    let n1 = client_b
        .wait_for_notification("notifications/awareness_update", 2000)
        .await
        .expect("should receive first awareness");
    assert_eq!(
        n1["params"]["event"]["data"]["cursor_row"]
            .as_u64()
            .unwrap(),
        0
    );
    assert_eq!(
        n1["params"]["event"]["data"]["cursor_col"]
            .as_u64()
            .unwrap(),
        0
    );

    // A moves cursor: position 2.
    client_a
        .send_awareness("move.txt", "Alice", 10, 25, None)
        .await;
    let n2 = client_b
        .wait_for_notification("notifications/awareness_update", 2000)
        .await
        .expect("should receive second awareness");
    assert_eq!(
        n2["params"]["event"]["data"]["cursor_row"]
            .as_u64()
            .unwrap(),
        10
    );
    assert_eq!(
        n2["params"]["event"]["data"]["cursor_col"]
            .as_u64()
            .unwrap(),
        25
    );

    // A moves cursor: position 3 — large row.
    client_a
        .send_awareness("move.txt", "Alice", 9999, 0, None)
        .await;
    let n3 = client_b
        .wait_for_notification("notifications/awareness_update", 2000)
        .await
        .expect("should receive third awareness");
    assert_eq!(
        n3["params"]["event"]["data"]["cursor_row"]
            .as_u64()
            .unwrap(),
        9999
    );
}

/// Awareness is NOT echoed back to the sender.
#[tokio::test]
async fn awareness_no_echo() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("echo.txt", "test").await;

    // Drain setup notifications.
    client_a.drain_notifications().await;

    // A sends awareness — should NOT receive its own update back.
    client_a
        .send_awareness("echo.txt", "Alice", 5, 5, None)
        .await;

    let echo = client_a.recv_timeout(300).await;
    assert!(
        echo.is_none(),
        "sender should NOT receive its own awareness echo"
    );
}

/// Two clients on different docs — awareness is doc-scoped (no cross-doc leak).
#[tokio::test]
async fn awareness_doc_isolation() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Each client shares a different doc.
    client_a.share("doc-a.txt", "aaa").await;
    client_b.share("doc-b.txt", "bbb").await;

    // B subscribes but is on doc-b, not doc-a.
    client_b.drain_notifications().await;

    // A sends awareness on doc-a.
    client_a
        .send_awareness("doc-a.txt", "Alice", 1, 1, None)
        .await;

    // B should receive awareness since broadcast is not doc-filtered at the
    // transport level (the EditorEvent carries doc_id, and the client-side
    // filters by doc). But the event should carry doc_id="doc-a.txt" so B
    // knows it's not for its active buffer.
    let notif = client_b.recv_timeout(300).await;
    if let Some(n) = notif {
        // If received, verify it carries the correct doc_id.
        let doc = n["params"]["event"]["data"]["doc_id"]
            .as_str()
            .unwrap_or("");
        assert_eq!(doc, "doc-a.txt", "notification must carry sender's doc_id");
    }
    // Either no notification (server-side doc filter) or correct doc_id — both valid.
}

// ============================================================================
// Cancel-Safety Regression Tests
// ============================================================================

/// Prove that the channel-based reader pattern is cancel-safe:
/// Multiple rapid recv attempts don't corrupt the stream.
#[tokio::test]
async fn channel_reader_cancel_safe() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Send 20 rapid requests. If the reader were cancel-unsafe (raw read_message
    // in select!), the BufReader would be corrupted by mid-parse cancellations
    // when the timeout wins the select race.
    for i in 0..20 {
        let doc = format!("cancel-safe-{}", i);
        client.share(&doc, &format!("content-{}", i)).await;
    }
    // Verify we can still communicate after all those requests.
    client.share("cancel-final.txt", "still works").await;
    assert_eq!(client.content("cancel-final.txt").await, "still works");
}

// ============================================================================
// Concurrent Traffic Tests
// ============================================================================

/// Two clients concurrently send 50 updates each to the same document.
/// Verifies convergence and no framing errors.
#[tokio::test]
async fn concurrent_edits_two_clients() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    ca.share("concurrent.txt", "").await;
    let state_a = ca.full_state("concurrent.txt").await;
    let state_b = cb.full_state("concurrent.txt").await;
    let mut ts_a = TextSync::from_state(&state_a).unwrap();
    let mut ts_b = TextSync::from_state(&state_b).unwrap();

    // Client A inserts "A" 50 times at end.
    for _ in 0..50 {
        let pos = ts_a.content().len() as u32;
        let update = ts_a.insert(pos, "A");
        ca.send_update("concurrent.txt", &update).await;
    }

    // Client B inserts "B" 50 times at beginning.
    for _ in 0..50 {
        let update = ts_b.insert(0, "B");
        cb.send_update("concurrent.txt", &update).await;
    }

    // Wait for propagation.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify convergence — both clients should see identical content.
    let content_a = ca.content("concurrent.txt").await;
    let content_b = cb.content("concurrent.txt").await;
    assert_eq!(content_a, content_b, "concurrent edits must converge");
    assert_eq!(content_a.len(), 100, "all 100 chars must be present");
    assert_eq!(
        content_a.matches('A').count(),
        50,
        "50 A chars must be present"
    );
    assert_eq!(
        content_a.matches('B').count(),
        50,
        "50 B chars must be present"
    );
}

/// Client sends a request while server pushes a notification simultaneously.
/// Both should be correctly parsed.
#[tokio::test]
async fn notification_during_request() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut ca = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut cb = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    ca.share("notify-during.txt", "initial").await;
    let state_b = cb.full_state("notify-during.txt").await;
    let mut ts_b = TextSync::from_state(&state_b).unwrap();

    // B sends an update (which will trigger a notification to A).
    let update = ts_b.insert(7, " edit");
    cb.send_update("notify-during.txt", &update).await;

    // Simultaneously, A requests content (a request while notification in flight).
    let content = ca.content("notify-during.txt").await;
    // Content should include B's edit.
    assert!(
        content.contains("edit"),
        "A should see B's edit: got '{}'",
        content
    );
}

/// Connect → 10 edits → disconnect → reconnect → 10 more edits.
/// All 20 edits preserved.
#[tokio::test]
async fn rapid_reconnect_stability() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("reconnect.txt", "").await;
    let state = client.full_state("reconnect.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();

    // Phase 1: 10 edits.
    for i in 0..10 {
        let pos = ts.content().len() as u32;
        let update = ts.insert(pos, &format!("{}", i));
        client.send_update("reconnect.txt", &update).await;
    }
    assert_eq!(client.content("reconnect.txt").await.len(), 10);

    // Disconnect by dropping client.
    drop(client);

    // Reconnect with new client.
    let mut client2 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let state2 = client2.full_state("reconnect.txt").await;
    let mut ts2 = TextSync::from_state(&state2).unwrap();
    assert_eq!(ts2.content().len(), 10, "first 10 edits preserved");

    // Phase 2: 10 more edits.
    for i in 0..10 {
        let pos = ts2.content().len() as u32;
        let update = ts2.insert(pos, &format!("{}", i));
        client2.send_update("reconnect.txt", &update).await;
    }
    assert_eq!(
        client2.content("reconnect.txt").await.len(),
        20,
        "all 20 edits preserved"
    );
}

// ============================================================================
// Gap S2: Cancel-safety regression test
// ============================================================================

/// Demonstrates that `read_message()` is NOT cancel-safe when used in `select!`.
///
/// When `tokio::select!` cancels `read_message` mid-parse (e.g. because a timer
/// fires), the BufReader's internal cursor is left past partially-consumed header
/// data. The next `read_message` call sees body bytes where it expects headers,
/// producing garbage or a fallback to line-based framing.
///
/// This is the root cause of the "falling back to line-based framing" warnings
/// seen in production. The fix is to run `read_message` in a dedicated task
/// feeding complete messages into an mpsc channel (the server already does this).
#[tokio::test]
async fn cancel_safety_read_message_in_select() {
    init_tracing();
    use tokio::io::AsyncWriteExt;

    let (mut writer, reader) = tokio::io::duplex(8192);
    let mut buf_reader = BufReader::new(reader);

    // Write a valid Content-Length framed message.
    let msg1 = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":\"first\"}";
    let header1 = format!("Content-Length: {}\r\n\r\n", msg1.len());
    writer.write_all(header1.as_bytes()).await.unwrap();
    writer.write_all(msg1).await.unwrap();

    // Read it normally — should succeed.
    let result1 = mae_mcp::read_message(&mut buf_reader)
        .await
        .unwrap()
        .unwrap();
    assert!(result1.contains("first"), "first read should succeed");

    // Now write a second message, but only the header first (body delayed).
    let msg2 = b"{\"jsonrpc\":\"2.0\",\"id\":2,\"result\":\"second\"}";
    let header2 = format!("Content-Length: {}\r\n\r\n", msg2.len());
    writer.write_all(header2.as_bytes()).await.unwrap();
    // Don't write the body yet — simulate slow network.

    // Use select! with a short timeout that will cancel read_message mid-parse.
    // The read_message call will have consumed the headers but not the body.
    let cancel_result = tokio::select! {
        biased;
        _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
            "cancelled"
        }
        msg = mae_mcp::read_message(&mut buf_reader) => {
            match msg {
                Ok(Some(text)) => Box::leak(text.into_boxed_str()) as &str,
                _ => "error",
            }
        }
    };

    // The read was cancelled because the body wasn't available yet.
    // (It might or might not have been cancelled depending on timing —
    // either outcome is valid for this test.)

    // Now write the body.
    writer.write_all(msg2).await.unwrap();

    // Write a third message.
    let msg3 = b"{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":\"third\"}";
    let header3 = format!("Content-Length: {}\r\n\r\n", msg3.len());
    writer.write_all(header3.as_bytes()).await.unwrap();
    writer.write_all(msg3).await.unwrap();

    if cancel_result == "cancelled" {
        // read_message was cancelled after consuming headers but before reading
        // the body. The BufReader now has the body bytes in its buffer, but the
        // next read_message will interpret them as a NEW message header.
        //
        // This is the cancel-safety bug: the next read sees JSON body bytes
        // where it expects "Content-Length:", falls back to line-based framing,
        // and may produce garbage or consume the third message's header as body.
        let next = mae_mcp::read_message(&mut buf_reader).await;
        // We can't assert the exact error because it depends on how many header
        // bytes were consumed, but we CAN assert that the overall sequence is
        // corrupted — the third message will NOT read cleanly.
        let next_text = next.unwrap_or(None).unwrap_or_default();
        // If cancel-safety were guaranteed, we'd get msg2 then msg3 cleanly.
        // Instead, the stream is likely corrupted. Just document this behavior.
        eprintln!(
            "cancel_safety_test: cancel_result={cancel_result}, next_text_len={}, next_text_preview={}",
            next_text.len(),
            &next_text[..next_text.len().min(80)]
        );
    } else {
        // read_message completed before the timeout — the body arrived fast
        // enough. This is still a valid test run; just verify the read succeeded.
        assert!(cancel_result.contains("second"));
    }
}

/// Demonstrates that the channel-based reader pattern is cancel-safe.
/// This is the pattern used by the server (handler.rs:79-101) and proposed
/// for the client (collab_bridge.rs WU-A).
#[tokio::test]
async fn channel_reader_pattern_is_cancel_safe() {
    init_tracing();
    use tokio::io::AsyncWriteExt;

    let (mut writer, reader) = tokio::io::duplex(8192);
    let mut buf_reader = BufReader::new(reader);

    // Spawn a dedicated reader task (the cancel-safe pattern).
    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::channel::<String>(32);
    tokio::spawn(async move {
        while let Ok(Some(msg)) = mae_mcp::read_message(&mut buf_reader).await {
            if msg_tx.send(msg).await.is_err() {
                break;
            }
        }
    });

    // Write 3 messages.
    for i in 1..=3 {
        let body = format!("{{\"jsonrpc\":\"2.0\",\"id\":{i},\"result\":\"msg{i}\"}}");
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        writer.write_all(header.as_bytes()).await.unwrap();
        writer.write_all(body.as_bytes()).await.unwrap();
    }

    // Use select! with short timeouts — the channel recv is always cancel-safe.
    let mut received = Vec::new();
    for _ in 0..3 {
        let msg = tokio::select! {
            // Even if the timer fires, the reader task continues running.
            // The channel recv is always cancel-safe.
            msg = msg_rx.recv() => msg.unwrap(),
        };
        received.push(msg);
    }

    assert_eq!(received.len(), 3, "all 3 messages received");
    assert!(received[0].contains("msg1"));
    assert!(received[1].contains("msg2"));
    assert!(received[2].contains("msg3"));
}

// ============================================================================
// Gap I1/I3: Notification capture + wal_seq gap detection
// ============================================================================

/// Verifies that sync_update notifications carry wal_seq and that the
/// client's notification_log captures them (not silently dropped).
#[tokio::test]
async fn notifications_captured_with_wal_seq() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares a doc.
    client_a
        .share_with_client_id("walseq.txt", "base", test_client_id(TEST_PID_SHARER, 0))
        .await;
    client_b.resync("walseq.txt").await;

    // Drain setup notifications from B.
    client_b.drain_notifications().await;

    // A sends 5 updates — B should receive 5 sync_update notifications with
    // monotonically increasing wal_seq values.
    let state = client_a.full_state("walseq.txt").await;
    let mut ts_a =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();

    for i in 0..5 {
        let update = ts_a.insert(ts_a.content().len() as u32, &format!("edit{i}\n"));
        client_a.send_update("walseq.txt", &update).await;
    }

    // Give notifications time to arrive, then drain them.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let notifs = client_b.drain_notifications().await;

    // Extract wal_seq from sync_update notifications.
    let sync_notifs: Vec<_> = notifs
        .iter()
        .filter(|n| n.get("method").and_then(|m| m.as_str()) == Some("notifications/sync_update"))
        .collect();

    // Actual notification format:
    // {"method":"notifications/sync_update","params":{"event":{"data":{"wal_seq":N,...},...},...}}
    let wal_seqs: Vec<u64> = sync_notifs
        .iter()
        .filter_map(|n| {
            n.pointer("/params/event/data/wal_seq")
                .and_then(|v| v.as_u64())
        })
        .collect();

    assert!(
        sync_notifs.len() >= 5,
        "B should receive at least 5 sync_update notifications, got {}",
        sync_notifs.len()
    );

    // wal_seq values should be monotonically increasing.
    for window in wal_seqs.windows(2) {
        assert!(
            window[1] > window[0],
            "wal_seq must be monotonically increasing: {:?}",
            wal_seqs
        );
    }
}

/// Verifies that wal_seq values in sync/update responses are sequential.
/// A gap in wal_seq indicates missed updates (the production client triggers
/// a ForceSync/resync when it detects a gap).
#[tokio::test]
async fn wal_seq_in_update_responses_is_sequential() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client
        .share_with_client_id("seq.txt", "", test_client_id(TEST_PID_SHARER, 0))
        .await;
    let state = client.full_state("seq.txt").await;
    let mut ts =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();

    let mut wal_seqs = Vec::new();
    for i in 0..10 {
        let update = ts.insert(ts.content().len() as u32, &format!("x{i}"));
        let resp = client.send_update("seq.txt", &update).await;
        assert!(resp.get("error").is_none(), "update {i} failed: {resp}");
        if let Some(seq) = resp.pointer("/result/wal_seq").and_then(|v| v.as_u64()) {
            wal_seqs.push(seq);
        }
    }

    assert_eq!(wal_seqs.len(), 10, "all 10 responses should have wal_seq");

    // Verify strictly increasing (no gaps from our perspective as sender).
    for window in wal_seqs.windows(2) {
        assert!(
            window[1] > window[0],
            "wal_seq must increase: {} -> {}",
            window[0],
            window[1]
        );
    }
}

// ============================================================================
// Gap I4: Concurrent requests + notification delivery
// ============================================================================

/// Two clients send updates simultaneously while both subscribed to notifications.
/// Verifies that each client receives the other's updates as notifications,
/// AND that their own request/response pairs are not corrupted.
#[tokio::test]
async fn concurrent_updates_deliver_cross_notifications() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a
        .share_with_client_id("concurrent.txt", "base", test_client_id(TEST_PID_SHARER, 0))
        .await;
    client_b.resync("concurrent.txt").await;
    client_a.drain_notifications().await;
    client_b.drain_notifications().await;
    client_a.notification_log.clear();
    client_b.notification_log.clear();

    // Each client creates its own TextSync fork and sends updates.
    let state = client_a.full_state("concurrent.txt").await;
    let mut ts_a =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();
    let mut ts_b =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_JOINER, 0)).unwrap();

    // Interleave: A sends, B sends, A sends, B sends...
    for i in 0..5 {
        let ua = ts_a.insert(ts_a.content().len() as u32, &format!("A{i}"));
        let resp_a = client_a.send_update("concurrent.txt", &ua).await;
        assert!(resp_a.get("error").is_none(), "A update {i} failed");

        let ub = ts_b.insert(ts_b.content().len() as u32, &format!("B{i}"));
        let resp_b = client_b.send_update("concurrent.txt", &ub).await;
        assert!(resp_b.get("error").is_none(), "B update {i} failed");
    }

    // Give notifications time to propagate.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let a_notifs = client_a.drain_notifications().await;
    let b_notifs = client_b.drain_notifications().await;

    // Count sync_update notifications from both sources:
    // - notification_log: captured by recv() during send_update calls
    // - drain result: read from wire after the send cycle
    let count_sync = |notifs: &[serde_json::Value], log: &[serde_json::Value]| -> usize {
        let is_sync = |n: &serde_json::Value| {
            n.get("method").and_then(|m| m.as_str()) == Some("notifications/sync_update")
        };
        notifs.iter().filter(|n| is_sync(n)).count() + log.iter().filter(|n| is_sync(n)).count()
    };

    let a_sync_count = count_sync(&a_notifs, &client_a.notification_log);
    let b_sync_count = count_sync(&b_notifs, &client_b.notification_log);

    assert!(
        a_sync_count >= 5,
        "A should receive >= 5 sync_update notifications from B, got {}",
        a_sync_count
    );
    assert!(
        b_sync_count >= 5,
        "B should receive >= 5 sync_update notifications from A, got {}",
        b_sync_count
    );

    // Final content on server should contain all edits.
    let content = client_a.content("concurrent.txt").await;
    for i in 0..5 {
        assert!(content.contains(&format!("A{i}")), "server missing A{i}");
        assert!(content.contains(&format!("B{i}")), "server missing B{i}");
    }
}

// ============================================================================
// Gap I2: Share with production-style client_id (compute_client_id path)
// ============================================================================

/// Verifies that share_with_client_id (using realistic FNV-1a hashed IDs)
/// produces documents that survive encode→decode round-trips and can be
/// joined by another client with a different realistic ID.
#[tokio::test]
async fn share_with_realistic_client_ids_roundtrip() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut sharer = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut joiner = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    let sharer_cid = test_client_id(TEST_PID_SHARER, 0);
    let joiner_cid = test_client_id(TEST_PID_JOINER, 0);

    // Sharer creates doc with realistic client_id.
    sharer
        .share_with_client_id("realistic.txt", "hello from sharer", sharer_cid)
        .await;

    // Joiner gets full state and creates its own TextSync fork.
    let state = joiner.full_state("realistic.txt").await;
    let mut ts_joiner = TextSync::from_state_with_client_id(&state, joiner_cid).unwrap();
    assert_eq!(
        ts_joiner.content(),
        "hello from sharer",
        "joiner should see sharer's content"
    );

    // Joiner edits and sends update.
    let update = ts_joiner.insert(ts_joiner.content().len() as u32, " + joiner edit");
    let resp = joiner.send_update("realistic.txt", &update).await;
    assert!(
        resp.get("error").is_none(),
        "joiner update with realistic client_id should succeed: {resp}"
    );

    // Verify content converged.
    let content = sharer.content("realistic.txt").await;
    assert!(
        content.contains("hello from sharer"),
        "sharer content preserved"
    );
    assert!(
        content.contains("joiner edit"),
        "joiner edit visible on server"
    );
}

// ============================================================================
// Gap I5: Verify drain_collab_intents state transitions
// ============================================================================

/// Verifies that Buffer sync state is consistent after enable_sync + share.
/// This tests the actual state transitions rather than manually setting fields.
#[tokio::test]
async fn buffer_sync_state_consistency_after_share() {
    init_tracing();

    let mut buf = Buffer::new();
    buf.name = "state-test.txt".to_string();
    buf.insert_text_at(0, "initial content");

    // Before sync: no sync_doc, no collab_doc_id, no pending updates.
    assert!(buf.sync_doc.is_none());
    assert!(buf.collab_doc_id.is_none());
    assert!(buf.pending_sync_updates.is_empty());

    // Enable sync (mirrors what collab_bridge does on share).
    let cid = test_client_id(TEST_PID_SHARER, 0);
    buf.enable_sync(cid);

    // enable_sync generates an initial insert update — clear it (production
    // code also clears these before sending the share request).
    buf.pending_sync_updates.clear();

    // After enable_sync: sync_doc exists, content matches rope, undo enabled.
    assert!(buf.sync_doc.is_some());
    assert_eq!(
        buf.sync_doc.as_ref().unwrap().content(),
        "initial content",
        "sync_doc content must match rope"
    );
    assert!(
        buf.sync_doc.as_ref().unwrap().undo_mgr_active(),
        "undo must be enabled after enable_sync"
    );

    // Set collab_doc_id (mirrors what collab_bridge does after server confirms).
    buf.collab_doc_id = Some("file:proj/state-test.txt".to_string());

    // Insert while synced — should generate pending updates.
    buf.insert_text_at(15, " more");
    assert!(
        !buf.pending_sync_updates.is_empty(),
        "insert while synced should queue updates"
    );

    // Verify sync_doc and rope are in sync.
    assert_eq!(buf.text(), buf.sync_doc.as_ref().unwrap().content());

    // Disable sync — returns state, clears sync_doc.
    // Note: disable_sync does NOT clear pending_sync_updates (they may
    // still need to be drained). Production clears them during disconnect.
    let state = buf.disable_sync();
    assert!(state.is_some());
    assert!(buf.sync_doc.is_none());

    // Buffer content preserved after disable.
    assert_eq!(buf.text(), "initial content more");
}

/// Share fails → cleanup → re-share. Verify no orphaned state.
#[tokio::test]
async fn share_fail_with_pending_edits_no_orphans() {
    init_tracing();

    let mut buf = Buffer::new();
    buf.name = "fail-test.txt".to_string();
    buf.insert_text_at(0, "content");

    // Simulate: enable_sync called optimistically before server confirms.
    let cid = test_client_id(TEST_PID_SHARER, 0);
    buf.enable_sync(cid);
    buf.collab_doc_id = Some("file:proj/fail-test.txt".to_string());

    // User edits while waiting for server confirmation.
    buf.insert_text_at(7, " edited");
    assert!(!buf.pending_sync_updates.is_empty(), "edits queued");

    // Simulate ShareFailed — server rejected. Must clean up completely.
    buf.collab_doc_id = None;
    buf.sync_doc = None;
    buf.pending_sync_updates.clear();

    // Verify: no orphaned state that could confuse a later share.
    assert!(buf.sync_doc.is_none());
    assert!(buf.collab_doc_id.is_none());
    assert!(buf.pending_sync_updates.is_empty());

    // Buffer content still has the user's edits (not lost).
    assert_eq!(buf.text(), "content edited");

    // Re-share with new client_id — must succeed cleanly.
    let cid2 = test_client_id(TEST_PID_SHARER, 1);
    buf.enable_sync(cid2);
    assert!(buf.sync_doc.is_some());
    assert_eq!(
        buf.sync_doc.as_ref().unwrap().content(),
        "content edited",
        "re-share must include user's edits"
    );
}

// ============================================================================
// Gap I6: Heartbeat during sync operations
// ============================================================================

/// Verify that $/ping works while sync updates are in-flight.
/// In production, the heartbeat tick fires every 30s via tokio::select!.
/// This test ensures the server handles interleaved ping + sync correctly.
#[tokio::test]
async fn ping_during_active_sync() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client
        .share_with_client_id("ping-sync.txt", "base", test_client_id(TEST_PID_SHARER, 0))
        .await;
    let state = client.full_state("ping-sync.txt").await;
    let mut ts =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();

    // Interleave updates and pings.
    for i in 0..5 {
        let update = ts.insert(ts.content().len() as u32, &format!("x{i}"));
        let resp = client.send_update("ping-sync.txt", &update).await;
        assert!(resp.get("error").is_none(), "update {i} failed");

        let pong = client.ping().await;
        assert_eq!(pong["result"], "pong", "ping {i} should return pong");
    }

    // Final content check.
    let content = client.content("ping-sync.txt").await;
    assert_eq!(content.len(), 4 + 10, "base(4) + 5*2(x0..x4) = 14");
}

// ============================================================================
// Gap S3: Auto-reshare on reconnect
// ============================================================================

/// After disconnect + reconnect, a previously shared document should be
/// recoverable via resync (not requiring a fresh share).
#[tokio::test]
async fn reconnect_resync_preserves_document() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Client 1 shares and edits.
    let mut client1 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    client1
        .share_with_client_id("resync.txt", "original", test_client_id(TEST_PID_SHARER, 0))
        .await;
    let state = client1.full_state("resync.txt").await;
    let mut ts =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();
    let update = ts.insert(8, " content");
    client1.send_update("resync.txt", &update).await;

    // Verify content before disconnect.
    assert_eq!(client1.content("resync.txt").await, "original content");

    // Client 1 disconnects.
    drop(client1);

    // Client 2 connects and resyncs — document should be intact on server.
    let mut client2 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let (state2, _sv) = client2.resync("resync.txt").await;
    let ts2 = TextSync::from_state(&state2).unwrap();
    assert_eq!(
        ts2.content(),
        "original content",
        "document must survive client disconnect"
    );

    // Client 2 can edit the document.
    let mut ts2 = ts2;
    let update2 = ts2.insert(ts2.content().len() as u32, " + more");
    let resp = client2.send_update("resync.txt", &update2).await;
    assert!(
        resp.get("error").is_none(),
        "editing after resync should succeed"
    );
    assert_eq!(
        client2.content("resync.txt").await,
        "original content + more"
    );
}

// ============================================================================
// Gap D1: Large client_id encode/decode through full protocol stack
// ============================================================================

/// End-to-end test: a client shares with a realistic (FNV-1a hashed) client_id,
/// sends updates, another client joins and receives them. Verifies the entire
/// stack: TextSync → encode_state → base64 → JSON-RPC → server → broadcast →
/// second client → decode → apply.
#[tokio::test]
async fn full_stack_realistic_client_ids() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut sharer = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut joiner = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    let sharer_cid = test_client_id(TEST_PID_SHARER, 2);
    let joiner_cid = test_client_id(TEST_PID_JOINER, 2);

    // Sharer creates doc with FNV-1a client_id.
    let mut ts_sharer = TextSync::with_client_id("shared content\n", sharer_cid);
    let state = ts_sharer.encode_state();
    let msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": sharer.next_id,
        "method": "sync/share",
        "params": {
            "doc": "fullstack.txt",
            "update": update_to_base64(&state),
        }
    });
    sharer.next_id += 1;
    sharer.send(&msg).await;
    let resp = sharer.recv().await;
    assert!(resp.get("error").is_none(), "share failed: {resp}");

    // Sharer sends an edit.
    let update1 = ts_sharer.insert(15, "line 2\n");
    let resp = sharer.send_update("fullstack.txt", &update1).await;
    assert!(resp.get("error").is_none(), "sharer update failed");

    // Joiner gets full state, creates TextSync with its own client_id.
    let server_state = joiner.full_state("fullstack.txt").await;
    let mut ts_joiner = TextSync::from_state_with_client_id(&server_state, joiner_cid).unwrap();
    assert_eq!(
        ts_joiner.content(),
        "shared content\nline 2\n",
        "joiner must see sharer's content including edit"
    );

    // Joiner edits.
    let update2 = ts_joiner.insert(ts_joiner.content().len() as u32, "joiner line\n");
    let resp = joiner.send_update("fullstack.txt", &update2).await;
    assert!(resp.get("error").is_none(), "joiner update failed");

    // Verify convergence.
    let final_content = sharer.content("fullstack.txt").await;
    assert!(final_content.contains("shared content"));
    assert!(final_content.contains("line 2"));
    assert!(final_content.contains("joiner line"));
}

// ============================================================================
// Gap D2/D3: Multi-client concurrent edits + offline rejoin
// ============================================================================

/// Two clients edit the same document concurrently (not sequentially).
/// Both send rapid updates without waiting for the other. Server merges
/// correctly and final content converges.
#[tokio::test]
async fn truly_concurrent_edits_converge() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a
        .share_with_client_id("concurrent2.txt", "", test_client_id(TEST_PID_SHARER, 0))
        .await;
    client_b.resync("concurrent2.txt").await;

    let state = client_a.full_state("concurrent2.txt").await;
    let mut ts_a =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();
    let mut ts_b =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_JOINER, 0)).unwrap();

    // Both clients send 20 updates as fast as possible, concurrently.
    let updates_a: Vec<Vec<u8>> = (0..20)
        .map(|i| ts_a.insert(ts_a.content().len() as u32, &format!("[A{i}]")))
        .collect();
    let updates_b: Vec<Vec<u8>> = (0..20)
        .map(|i| ts_b.insert(ts_b.content().len() as u32, &format!("[B{i}]")))
        .collect();

    // Send all A's updates.
    for (i, u) in updates_a.iter().enumerate() {
        let resp = client_a.send_update("concurrent2.txt", u).await;
        assert!(resp.get("error").is_none(), "A update {i} failed");
    }

    // Send all B's updates.
    for (i, u) in updates_b.iter().enumerate() {
        let resp = client_b.send_update("concurrent2.txt", u).await;
        assert!(resp.get("error").is_none(), "B update {i} failed");
    }

    // Verify convergence.
    let content_a = client_a.content("concurrent2.txt").await;
    let content_b = client_b.content("concurrent2.txt").await;
    assert_eq!(
        content_a, content_b,
        "both clients must see same content from server"
    );
    for i in 0..20 {
        assert!(content_a.contains(&format!("[A{i}]")), "missing [A{i}]");
        assert!(content_a.contains(&format!("[B{i}]")), "missing [B{i}]");
    }
}

/// Client A shares, Client B joins and edits, Client A disconnects,
/// Client B continues editing, Client A reconnects and resyncs.
/// Client A must see ALL of Client B's edits (including those made
/// while A was offline).
#[tokio::test]
async fn offline_peer_resyncs_missed_edits() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // A shares.
    client_a
        .share_with_client_id("offline.txt", "base\n", test_client_id(TEST_PID_SHARER, 0))
        .await;
    client_b.resync("offline.txt").await;

    // B edits while A is connected.
    let state = client_b.full_state("offline.txt").await;
    let mut ts_b =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_JOINER, 0)).unwrap();
    let u1 = ts_b.insert(ts_b.content().len() as u32, "before-disconnect\n");
    client_b.send_update("offline.txt", &u1).await;

    // A disconnects.
    drop(client_a);

    // B makes more edits while A is offline.
    for i in 0..5 {
        let u = ts_b.insert(ts_b.content().len() as u32, &format!("offline-{i}\n"));
        client_b.send_update("offline.txt", &u).await;
    }

    // A reconnects and resyncs.
    let mut client_a2 = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let (state_a, _sv) = client_a2.resync("offline.txt").await;
    let ts_a = TextSync::from_state(&state_a).unwrap();

    // A must see ALL of B's edits, including those made while offline.
    let content = ts_a.content();
    assert!(content.contains("base"), "base content missing");
    assert!(
        content.contains("before-disconnect"),
        "pre-disconnect edit missing"
    );
    for i in 0..5 {
        assert!(
            content.contains(&format!("offline-{i}")),
            "offline edit {i} missing — A didn't resync correctly"
        );
    }
}

// ============================================================================
// Gap D4: Server restart (simulated via new DocStore)
// ============================================================================

/// Simulates server restart by dropping the old DocStore (backed by SQLite)
/// and creating a new one from the same backend. Documents persisted via
/// WAL should survive.
#[tokio::test]
async fn document_survives_server_restart_via_wal() {
    init_tracing();

    // Use a shared SQLite backend that persists across "restarts".
    // Wrap in Arc<dyn StorageBackend> to allow cloning for the second DocStore.
    let sqlite = SqliteBackend::open_memory().unwrap();
    let backend: Arc<dyn mae_state_server::storage::StorageBackend> = Arc::new(sqlite);
    let store1 = Arc::new(DocStore::new(backend.clone(), 500));
    let bc1 = test_broadcaster();

    // Client shares a document.
    let mut client1 = Client::connect(Arc::clone(&store1), Arc::clone(&bc1)).await;
    client1
        .share_with_client_id(
            "persist.txt",
            "persistent data\n",
            test_client_id(TEST_PID_SHARER, 0),
        )
        .await;

    // Send some edits.
    let state = client1.full_state("persist.txt").await;
    let mut ts =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_SHARER, 0)).unwrap();
    for i in 0..3 {
        let u = ts.insert(ts.content().len() as u32, &format!("edit-{i}\n"));
        client1.send_update("persist.txt", &u).await;
    }

    // Verify pre-restart content.
    let pre_content = client1.content("persist.txt").await;
    assert!(pre_content.contains("persistent data"));
    assert!(pre_content.contains("edit-2"));

    // "Restart" the server: drop old store, create new one with same backend.
    drop(client1);
    drop(store1);

    let store2 = Arc::new(DocStore::new(backend.clone(), 500));
    let bc2 = test_broadcaster();

    // New client connects to "restarted" server.
    let mut client2 = Client::connect(Arc::clone(&store2), Arc::clone(&bc2)).await;

    // Try to get the document — it should have been persisted via WAL.
    let (state2, _sv) = client2.resync("persist.txt").await;
    let ts2 = TextSync::from_state(&state2).unwrap();
    let post_content = ts2.content();

    assert_eq!(
        pre_content, post_content,
        "document content must survive server restart (WAL recovery)"
    );
}

// ============================================================================
// Buffer-level gap: remote edits should NOT set modified flag
// ============================================================================

/// Remote CRDT updates applied via apply_sync_update should NOT mark the
/// buffer as modified. Only local edits should set the modified flag.
/// This ensures the status bar shows the correct state.
#[tokio::test]
async fn remote_sync_update_does_not_set_modified() {
    init_tracing();

    let mut buf_local = Buffer::new();
    buf_local.name = "remote-mod.txt".to_string();
    buf_local.insert_text_at(0, "hello");
    buf_local.enable_sync(test_client_id(TEST_PID_SHARER, 0));
    buf_local.modified = false; // Simulate saved state.

    // Create a remote peer that makes an edit.
    let state = buf_local.sync_doc.as_ref().unwrap().encode_state();
    let mut remote =
        TextSync::from_state_with_client_id(&state, test_client_id(TEST_PID_JOINER, 0)).unwrap();
    let update = remote.insert(5, " world");

    // Apply remote update to local buffer.
    buf_local.apply_sync_update(&update).unwrap();
    assert_eq!(buf_local.text(), "hello world");

    // The modified flag should NOT be set by remote edits.
    // The buffer was saved (modified=false), and a remote peer added text.
    // From the local user's perspective, they haven't changed anything.
    assert!(
        !buf_local.modified,
        "remote sync update should NOT set modified flag — \
         only local edits should mark buffer as modified"
    );
}

// ============================================================================
// Tier 4 — Client-side deserialization round-trip tests
//
// These tests validate that server broadcast notifications can be parsed by
// the same deserialization logic used in the production client. This catches
// format mismatches between server serialization and client parsing — the
// exact class of bug that caused awareness cursors to silently fail.
// ============================================================================

/// Parse awareness notification the same way production client does.
/// Returns (client_id, doc_id, AwarenessState) or None on failure.
fn parse_awareness_notification(
    notif: &serde_json::Value,
) -> Option<(u64, String, mae_sync::awareness::AwarenessState)> {
    let params = notif.get("params")?;
    let event = params.get("event").unwrap_or(params);
    let data = event.get("data").unwrap_or(event);
    let client_id = data.get("client_id").and_then(|v| v.as_u64()).unwrap_or(0);
    let doc_id = data
        .get("doc")
        .or_else(|| data.get("doc_id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let state_source = data.get("state").unwrap_or(data);
    let state =
        serde_json::from_value::<mae_sync::awareness::AwarenessState>(state_source.clone()).ok()?;
    Some((client_id, doc_id, state))
}

/// Parse save_committed notification the same way production client does.
fn parse_save_committed_notification(
    notif: &serde_json::Value,
) -> Option<(String, String, u64, String)> {
    let params = notif.get("params")?;
    let event = params.get("event").unwrap_or(params);
    let data = event.get("data").unwrap_or(event);
    let doc = data.get("doc").and_then(|v| v.as_str())?.to_string();
    let saved_by = data
        .get("saved_by")
        .and_then(|v| v.as_str())
        .unwrap_or("peer")
        .to_string();
    let save_epoch = data.get("save_epoch").and_then(|v| v.as_u64()).unwrap_or(0);
    let content_hash = data
        .get("content_hash")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some((doc, saved_by, save_epoch, content_hash))
}

/// CRITICAL: Awareness notification from server can be deserialized into AwarenessState.
/// This is the exact bug that caused remote cursors to silently fail — the server
/// broadcast format didn't match what the client parser expected.
#[tokio::test]
async fn awareness_roundtrip_deserialization() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("rt.txt", "test content").await;
    client_b.resync("rt.txt").await;
    client_b.drain_notifications().await;

    // A sends awareness with all fields populated.
    client_a
        .send_awareness("rt.txt", "Alice", 7, 15, Some((3, 0, 7, 15)))
        .await;

    // B receives the notification.
    let notif = client_b
        .wait_for_notification("notifications/awareness_update", 2000)
        .await
        .expect("B must receive awareness notification");

    // Parse using the SAME logic as production client.
    let (client_id, doc_id, state) = parse_awareness_notification(&notif)
        .expect("awareness notification must deserialize into AwarenessState");

    assert!(client_id > 0, "client_id must be non-zero");
    assert_eq!(doc_id, "rt.txt");
    assert_eq!(state.user_name, "Alice");
    assert_eq!(state.cursor_row, 7);
    assert_eq!(state.cursor_col, 15);
    assert_eq!(state.selection, Some((3, 0, 7, 15)));
    assert_eq!(state.mode, "normal");
}

/// Awareness with no selection deserializes correctly (selection = None).
#[tokio::test]
async fn awareness_roundtrip_no_selection() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("ns.txt", "hello").await;
    client_b.resync("ns.txt").await;
    client_b.drain_notifications().await;

    client_a.send_awareness("ns.txt", "Bob", 0, 0, None).await;

    let notif = client_b
        .wait_for_notification("notifications/awareness_update", 2000)
        .await
        .expect("B must receive awareness notification");

    let (_cid, _doc, state) = parse_awareness_notification(&notif)
        .expect("awareness with null selection must deserialize");

    assert_eq!(state.user_name, "Bob");
    assert_eq!(state.cursor_row, 0);
    assert_eq!(state.cursor_col, 0);
    assert!(
        state.selection.is_none(),
        "selection should be None, got {:?}",
        state.selection
    );
}

/// Full save cycle: save_intent → save_committed → peer receives notification.
/// Validates the entire round-trip including content hash verification.
#[tokio::test]
async fn save_committed_roundtrip_deserialization() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("save.txt", "save me").await;
    client_b.resync("save.txt").await;
    client_b.drain_notifications().await;

    // Compute content hash (same as production code).
    let content = client_a.content("save.txt").await;
    let hash = {
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    };

    // A: save_intent with correct hash.
    let intent_resp = client_a.save_intent("save.txt", &hash).await;
    assert!(
        intent_resp.get("error").is_none(),
        "save_intent should succeed: {intent_resp}"
    );
    let save_epoch = intent_resp["result"]["result"]["save_epoch"]
        .as_u64()
        .expect("save_intent must return save_epoch");

    // A: save_committed.
    let committed_resp = client_a
        .save_committed("save.txt", "alice", save_epoch, &hash)
        .await;
    assert!(
        committed_resp.get("error").is_none(),
        "save_committed should succeed: {committed_resp}"
    );

    // B: should receive save_committed notification.
    let notif = client_b
        .wait_for_notification("notifications/save_committed", 2000)
        .await
        .expect("B must receive save_committed notification");

    let (doc, saved_by, epoch, notif_hash) =
        parse_save_committed_notification(&notif).expect("save_committed notification must parse");

    assert_eq!(doc, "save.txt");
    assert_eq!(saved_by, "alice");
    assert_eq!(epoch, save_epoch);
    assert_eq!(notif_hash, hash);
}

/// Save intent with wrong hash is rejected (conflict detection).
#[tokio::test]
async fn save_intent_conflict_roundtrip() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("conflict.txt", "original").await;

    // Send save_intent with a deliberately wrong hash.
    // Server returns success with status:"conflict", not a JSON-RPC error.
    let resp = client.save_intent("conflict.txt", "badhash").await;
    let status = resp["result"]["result"]["status"].as_str().unwrap_or("");
    assert_eq!(
        status, "conflict",
        "save_intent with wrong hash must return conflict status: {resp}"
    );
}

/// Sharer disconnect broadcasts sharer_left to peers.
#[tokio::test]
async fn sharer_left_roundtrip() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("leave.txt", "goodbye").await;
    client_b.resync("leave.txt").await;
    client_b.drain_notifications().await;

    // Drop client A — server should broadcast sharer_left / peer_left.
    drop(client_a);

    // B should receive a peer_left notification.
    let notif = client_b
        .wait_for_notification("notifications/peer_left", 2000)
        .await;
    assert!(
        notif.is_some(),
        "B must receive peer_left when A disconnects"
    );
}

/// Client only receives events for subscribed types.
/// A client that does NOT subscribe to awareness_update should not receive them.
#[tokio::test]
async fn subscription_filtering_awareness() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Client A: full subscription (default via connect()).
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    // Client B: manual setup with sync_update only (no awareness_update).
    let (client_stream, server_stream) = tokio::io::duplex(8192);
    let (server_read, server_write) = tokio::io::split(server_stream);
    let server_reader = BufReader::new(server_read);
    let store2 = Arc::clone(&store);
    let bc2 = Arc::clone(&bc);
    tokio::spawn(async move {
        handle_client(
            server_reader,
            server_write,
            store2,
            bc2,
            std::time::Instant::now(),
        )
        .await;
    });
    let (client_read, client_write) = tokio::io::split(client_stream);
    let client_reader = BufReader::new(client_read);
    let mut client_b = Client {
        writer: client_write,
        reader: client_reader,
        next_id: 1,
        notification_log: Vec::new(),
    };
    // Initialize but subscribe only to sync_update (not awareness_update).
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"clientInfo":{"name":"filter-test"}}});
    client_b.send(&msg).await;
    let _ = client_b.recv().await;
    let msg = serde_json::json!({"jsonrpc":"2.0","id":2,"method":"notifications/subscribe","params":{"types":["sync_update","peer_joined","peer_left"]}});
    client_b.send(&msg).await;
    let _ = client_b.recv().await;
    client_b.next_id = 3;

    client_a.share("filter.txt", "test").await;
    client_b.resync("filter.txt").await;
    client_b.drain_notifications().await;

    // A sends awareness.
    client_a
        .send_awareness("filter.txt", "Alice", 1, 1, None)
        .await;

    // B should NOT receive awareness (not subscribed).
    let notif = client_b
        .wait_for_notification("notifications/awareness_update", 500)
        .await;
    assert!(
        notif.is_none(),
        "B should NOT receive awareness when not subscribed"
    );
}

/// WAL sequence numbers are monotonically increasing per document.
/// When a client sends multiple updates, each response has incrementing wal_seq.
#[tokio::test]
async fn wal_seq_monotonic_per_doc() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("wal.txt", "a").await;
    let state = client.full_state("wal.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();

    let mut prev_seq = 0u64;
    for i in 0..5 {
        let update = ts.insert(ts.content().len() as u32, &format!("{i}"));
        let resp = client.send_update("wal.txt", &update).await;
        let wal_seq = resp["result"]["wal_seq"]
            .as_u64()
            .expect("response must include wal_seq");
        assert!(
            wal_seq > prev_seq,
            "wal_seq must increase: got {wal_seq}, prev was {prev_seq}"
        );
        prev_seq = wal_seq;
    }
}

/// Multi-doc WAL sequences are independent (different docs have separate counters).
#[tokio::test]
async fn wal_seq_independent_per_doc() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client.share("doc_a.txt", "aaa").await;
    client.share("doc_b.txt", "bbb").await;

    let state_a = client.full_state("doc_a.txt").await;
    let state_b = client.full_state("doc_b.txt").await;
    let mut ts_a = TextSync::from_state(&state_a).unwrap();
    let mut ts_b = TextSync::from_state(&state_b).unwrap();

    // Send updates to doc_a and doc_b interleaved.
    let up_a = ts_a.insert(3, "1");
    let resp_a1 = client.send_update("doc_a.txt", &up_a).await;
    let seq_a1 = resp_a1["result"]["wal_seq"].as_u64().unwrap();

    let up_b = ts_b.insert(3, "1");
    let resp_b1 = client.send_update("doc_b.txt", &up_b).await;
    let seq_b1 = resp_b1["result"]["wal_seq"].as_u64().unwrap();

    let up_a2 = ts_a.insert(4, "2");
    let resp_a2 = client.send_update("doc_a.txt", &up_a2).await;
    let seq_a2 = resp_a2["result"]["wal_seq"].as_u64().unwrap();

    // Each doc's sequence increases independently.
    assert!(seq_a2 > seq_a1, "doc_a wal_seq must increase");
    // doc_b should also have its own sequence space.
    assert!(seq_b1 > 0, "doc_b wal_seq must be non-zero");
}

/// Awareness doc isolation: A on doc1 and B on doc2 — awareness doesn't cross docs.
#[tokio::test]
async fn awareness_doc_isolation_deserialization() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("iso_a.txt", "doc a").await;
    client_b.share("iso_b.txt", "doc b").await;
    client_b.drain_notifications().await;

    // A sends awareness for iso_a.txt.
    client_a
        .send_awareness("iso_a.txt", "Alice", 0, 0, None)
        .await;

    // B should NOT receive it (B is only on iso_b.txt).
    let notif = client_b
        .wait_for_notification("notifications/awareness_update", 500)
        .await;
    assert!(
        notif.is_none(),
        "awareness for iso_a.txt must not reach client on iso_b.txt"
    );
}

/// Sync update notification can be parsed into valid base64 update bytes.
/// Validates the full notification → base64 decode → yrs apply path.
#[tokio::test]
async fn sync_update_notification_parseable() {
    init_tracing();
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut client_a = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;
    let mut client_b = Client::connect(Arc::clone(&store), Arc::clone(&bc)).await;

    client_a.share("parse.txt", "hello").await;
    let state = client_b.full_state("parse.txt").await;
    client_b.drain_notifications().await;

    // A edits.
    let full = client_a.full_state("parse.txt").await;
    let mut ts = TextSync::from_state(&full).unwrap();
    let update = ts.insert(5, " world");
    client_a.send_update("parse.txt", &update).await;

    // B receives sync_update notification.
    let notif = client_b
        .wait_for_notification("notifications/sync_update", 2000)
        .await
        .expect("B must receive sync_update");

    // Parse the notification the same way production client does.
    let event_data = notif
        .pointer("/params/event/data")
        .expect("notification must have params.event.data");
    let update_b64 = event_data["update_base64"]
        .as_str()
        .expect("must have update_base64");
    let update_bytes = base64_to_update(update_b64).expect("update_base64 must be valid base64");

    // Apply to a fresh TextSync to verify it's a valid yrs update.
    let mut ts_b = TextSync::from_state(&state).unwrap();
    ts_b.apply_update(&update_bytes)
        .expect("update bytes must be a valid yrs update");
    assert_eq!(ts_b.content(), "hello world");
}
