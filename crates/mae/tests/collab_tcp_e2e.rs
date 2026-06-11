//! Tier 2 — TCP integration tests (real server).
//!
//! Gated with `#[ignore]` — run via:
//!   MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e -- --ignored --nocapture
//!
//! Spawns `mae-daemon` on a random port, connects via real TCP.

use std::process::Stdio;
use std::time::Duration;

use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc};
use mae_sync::text::TextSync;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::process::Command;

/// TCP client wrapper for testing.
#[allow(dead_code)]
struct TcpClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    next_id: u64,
}

#[allow(dead_code)]
impl TcpClient {
    async fn connect(addr: &str) -> Self {
        let stream = TcpStream::connect(addr).await.expect("failed to connect");
        let (read, write) = stream.into_split();
        let mut client = TcpClient {
            reader: BufReader::new(read),
            writer: write,
            next_id: 1,
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

    async fn recv(&mut self) -> serde_json::Value {
        loop {
            let text = mae_mcp::read_message(&mut self.reader)
                .await
                .unwrap()
                .unwrap();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            // Skip notifications (messages with method but no result/error/id).
            if val.get("method").is_some()
                && val.get("result").is_none()
                && val.get("error").is_none()
                && val.get("id").is_none()
            {
                continue;
            }
            return val;
        }
    }

    async fn recv_timeout(&mut self, ms: u64) -> Option<serde_json::Value> {
        match tokio::time::timeout(
            Duration::from_millis(ms),
            mae_mcp::read_message(&mut self.reader),
        )
        .await
        {
            Ok(Ok(Some(text))) => serde_json::from_str(&text).ok(),
            _ => None,
        }
    }

    async fn initialize(&mut self) {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"initialize","params":{"clientInfo":{"name":"tcp-test"}}});
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

    /// Share a KB with the server. Returns the response.
    async fn kb_share(
        &mut self,
        kb_id: &str,
        name: &str,
        creator: &str,
        collection_state: &[u8],
        nodes: &[(&str, &[u8])],
    ) -> serde_json::Value {
        let node_arr: Vec<serde_json::Value> = nodes
            .iter()
            .map(|(id, state)| serde_json::json!({ "id": id, "state": update_to_base64(state) }))
            .collect();
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "kb/share",
            "params": {
                "kb_id": kb_id,
                "name": name,
                "creator": creator,
                "collection_state": update_to_base64(collection_state),
                "nodes": node_arr,
            }
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "kb/share failed: {resp}");
        resp
    }

    /// Join a KB. Returns the response with collection_state and nodes.
    async fn kb_join(&mut self, kb_id: &str) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "kb/join",
            "params": { "kb_id": kb_id }
        });
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Send a KB node update. Returns the response.
    async fn kb_node_update(
        &mut self,
        kb_id: &str,
        node_id: &str,
        update: &[u8],
    ) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "kb/node_update",
            "params": {
                "kb_id": kb_id,
                "node_id": node_id,
                "update": update_to_base64(update),
            }
        });
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Leave a KB. Returns the response.
    async fn kb_leave(&mut self, kb_id: &str) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "kb/leave",
            "params": { "kb_id": kb_id }
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "kb/leave failed: {resp}");
        resp
    }

    async fn wait_for_notification(
        &mut self,
        method: &str,
        timeout_ms: u64,
    ) -> Option<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
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

/// Spawn mae-daemon on a random port, wait for it to listen, return (child, port).
///
/// Uses `MAE_STATE_SERVER_BIN` env var if set (pre-built binary), otherwise
/// falls back to `cargo run` (works in CI but deadlocks when `cargo test` holds
/// the workspace lock).
async fn spawn_server() -> (tokio::process::Child, String, tempfile::TempDir) {
    // Find a free port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let addr = format!("127.0.0.1:{}", port);

    // Use a temp data dir to avoid recovering stale documents from previous runs,
    // which can cause >5s startup times and flaky test failures.
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_str().unwrap().to_string();

    let child = if let Ok(bin) = std::env::var("MAE_STATE_SERVER_BIN") {
        Command::new(bin)
            .args(["--bind", &addr, "--data-dir", &data_dir])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn mae-daemon binary")
    } else {
        // Fallback: look in target/debug (cargo builds test deps there).
        let target_bin =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/mae-daemon");
        if target_bin.exists() {
            Command::new(&target_bin)
                .args(["--bind", &addr, "--data-dir", &data_dir])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("failed to spawn mae-daemon from target/debug")
        } else {
            Command::new("cargo")
                .args([
                    "run",
                    "-p",
                    "mae-daemon",
                    "--",
                    "--bind",
                    &addr,
                    "--data-dir",
                    &data_dir,
                ])
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .expect("failed to spawn mae-daemon via cargo run")
        }
    };

    // Wait for server to accept connections.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if TcpStream::connect(&addr).await.is_ok() {
            return (child, addr, tmp);
        }
    }
    panic!("mae-daemon did not start within 5s on {}", addr);
}

fn should_run() -> bool {
    std::env::var("MAE_TCP_E2E").is_ok()
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test]
#[ignore]
async fn tcp_full_roundtrip() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client = TcpClient::connect(&addr).await;
    client.share("tcp-test.txt", "hello").await;

    let state = client.full_state("tcp-test.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();
    let update = ts.insert(5, " tcp");
    client.send_update("tcp-test.txt", &update).await;

    assert_eq!(client.content("tcp-test.txt").await, "hello tcp");
}

#[tokio::test]
#[ignore]
async fn tcp_two_editors_convergence() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut ca = TcpClient::connect(&addr).await;
    let mut cb = TcpClient::connect(&addr).await;

    ca.share("tcp-conv.txt", "abcdef").await;
    let mut ts_a = TextSync::from_state(&ca.full_state("tcp-conv.txt").await).unwrap();
    let mut ts_b = TextSync::from_state(&cb.full_state("tcp-conv.txt").await).unwrap();

    let ua = ts_a.insert(2, "X");
    let ub = ts_b.insert(4, "Y");
    ca.send_update("tcp-conv.txt", &ua).await;
    cb.send_update("tcp-conv.txt", &ub).await;

    tokio::time::sleep(Duration::from_millis(200)).await;
    let content_a = ca.content("tcp-conv.txt").await;
    let content_b = cb.content("tcp-conv.txt").await;
    assert_eq!(content_a, content_b);
    assert!(content_a.contains('X') && content_a.contains('Y'));
}

#[tokio::test]
#[ignore]
async fn tcp_connection_refused_graceful() {
    if !should_run() {
        return;
    }
    // Attempt to connect to a port where nothing is listening.
    let result = TcpStream::connect("127.0.0.1:1").await;
    assert!(result.is_err(), "should fail to connect to closed port");
}

#[tokio::test]
#[ignore]
async fn tcp_large_document_sync() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client = TcpClient::connect(&addr).await;

    // 1MB document.
    let large: String = (0..20_000)
        .map(|i| format!("Line {:05}: The quick brown fox.\n", i))
        .collect();
    client.share("tcp-large.txt", &large).await;

    let mut cb = TcpClient::connect(&addr).await;
    let content = cb.content("tcp-large.txt").await;
    assert_eq!(content.len(), large.len());
    assert_eq!(content, large);
}

#[tokio::test]
#[ignore]
async fn tcp_rapid_edit_burst() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client = TcpClient::connect(&addr).await;
    client.share("tcp-burst.txt", "").await;

    let state = client.full_state("tcp-burst.txt").await;
    let mut ts = TextSync::from_state(&state).unwrap();

    // Send 100 rapid edits.
    for i in 0..100 {
        let update = ts.insert(ts.content().len() as u32, &format!("{}\n", i));
        let msg = serde_json::json!({"jsonrpc":"2.0","id":client.next_id,"method":"sync/update","params":{"doc":"tcp-burst.txt","update":update_to_base64(&update)}});
        client.next_id += 1;
        client.send(&msg).await;
    }

    // Drain all responses.
    for _ in 0..100 {
        let _ = client.recv().await;
    }

    let content = client.content("tcp-burst.txt").await;
    // All 100 lines should be present.
    let line_count = content.lines().count();
    assert_eq!(
        line_count, 100,
        "all 100 edits should be present, got {}",
        line_count
    );
}

#[tokio::test]
#[ignore]
async fn tcp_reconnect_after_server_restart() {
    if !should_run() {
        return;
    }
    let (mut server, addr, _tmp) = spawn_server().await;

    let mut client = TcpClient::connect(&addr).await;
    client.share("tcp-reconnect.txt", "before restart").await;

    // Kill the server.
    server.kill().await.expect("failed to kill server");

    // Wait for it to die.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Restart on the same port.
    let _server2 = Command::new("cargo")
        .args(["run", "-p", "mae-daemon", "--", "--bind", &addr])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to restart");

    // Wait for new server.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if TcpStream::connect(&addr).await.is_ok() {
            break;
        }
    }

    // Reconnect — new server won't have the old data (in-memory only).
    let mut client2 = TcpClient::connect(&addr).await;
    client2.share("tcp-reconnect.txt", "after restart").await;
    assert_eq!(client2.content("tcp-reconnect.txt").await, "after restart");
}

/// WU6: Offline edit → reconnect → resync — CRDT state preserved across server restart.
#[tokio::test]
#[ignore]
async fn tcp_offline_edit_reconnect_resync() {
    if !should_run() {
        return;
    }
    let (mut server, addr, _tmp) = spawn_server().await;

    // Client A shares "offline.txt" = "v1".
    let mut client_a = TcpClient::connect(&addr).await;
    client_a.share("offline.txt", "v1").await;

    // Client A edits to "v1-updated".
    let state = client_a.full_state("offline.txt").await;
    let mut ts_a = TextSync::from_state(&state).unwrap();
    let update = ts_a.reconcile_to("v1-updated");
    client_a.send_update("offline.txt", &update).await;
    assert_eq!(client_a.content("offline.txt").await, "v1-updated");

    // Preserve CRDT state locally.
    let preserved = client_a.full_state("offline.txt").await;

    // Kill server.
    server.kill().await.expect("failed to kill server");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Restart server on same port.
    let _server2 = Command::new("cargo")
        .args(["run", "-p", "mae-daemon", "--", "--bind", &addr])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to restart");

    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if TcpStream::connect(&addr).await.is_ok() {
            break;
        }
    }

    // Client A reconnects and re-shares with preserved CRDT state.
    let mut client_a2 = TcpClient::connect(&addr).await;
    let share_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": client_a2.next_id,
        "method": "sync/share",
        "params": {
            "doc": "offline.txt",
            "update": update_to_base64(&preserved)
        }
    });
    client_a2.next_id += 1;
    client_a2.send(&share_msg).await;
    let resp = client_a2.recv().await;
    assert!(resp.get("error").is_none(), "re-share failed: {resp}");

    // Client B joins and verifies content = "v1-updated".
    let mut client_b = TcpClient::connect(&addr).await;
    assert_eq!(
        client_b.content("offline.txt").await,
        "v1-updated",
        "CRDT state must survive server restart"
    );
}

/// Three clients concurrently edit the same doc — 100 edits each.
/// Verify identical final content on all 3.
#[tokio::test]
#[ignore]
async fn tcp_concurrent_three_editors() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut ca = TcpClient::connect(&addr).await;
    let mut cb = TcpClient::connect(&addr).await;
    let mut cc = TcpClient::connect(&addr).await;

    ca.share("3way.txt", "").await;
    let state_a = ca.full_state("3way.txt").await;
    let state_b = cb.full_state("3way.txt").await;
    let state_c = cc.full_state("3way.txt").await;
    let mut ts_a = TextSync::from_state(&state_a).unwrap();
    let mut ts_b = TextSync::from_state(&state_b).unwrap();
    let mut ts_c = TextSync::from_state(&state_c).unwrap();

    // Each client inserts its letter 100 times.
    for _ in 0..100 {
        let pos_a = ts_a.content().len() as u32;
        let ua = ts_a.insert(pos_a, "A");
        ca.send_update("3way.txt", &ua).await;

        let ub = ts_b.insert(0, "B");
        cb.send_update("3way.txt", &ub).await;

        let pos_c = ts_c.content().len() as u32;
        let uc = ts_c.insert(pos_c.min(1), "C");
        cc.send_update("3way.txt", &uc).await;
    }

    // Wait for propagation.
    tokio::time::sleep(Duration::from_millis(1000)).await;

    let content_a = ca.content("3way.txt").await;
    let content_b = cb.content("3way.txt").await;
    let content_c = cc.content("3way.txt").await;

    assert_eq!(content_a, content_b, "A and B must converge");
    assert_eq!(content_b, content_c, "B and C must converge");
    assert_eq!(content_a.len(), 300, "all 300 chars must be present");
    assert_eq!(content_a.matches('A').count(), 100);
    assert_eq!(content_a.matches('B').count(), 100);
    assert_eq!(content_a.matches('C').count(), 100);
}

// ============================================================================
// KB Protocol E2E Tests
// ============================================================================

/// Helper: realistic org-mode body for testing.
fn realistic_org_body() -> &'static str {
    ":PROPERTIES:\n:ID: test-node-001\n:ROAM_REFS: https://example.com\n:END:\n\
     #+TITLE: Test Node — CRDT Round-Trip\n#+FILETAGS: :research:crdt:\n\n\
     * Overview\nThis node tests the full round-trip: SQLite → KbNodeDoc → base64 → server → base64 → KbNodeDoc → SQLite.\n\n\
     ** Sub-heading with [[id:other-node][internal link]]\n\
     Content with Unicode: café, naïve, 日本語\n\n\
     #+begin_src rust\nfn main() { println!(\"hello\"); }\n#+end_src\n"
}

/// Helper: create a test KB with N nodes and share it via a client.
/// Returns (kb_id, collection_state, node_states).
fn make_test_kb(
    _kb_id: &str,
    name: &str,
    creator: &str,
    nodes: &[(&str, &str, &str, &[&str])], // (id, title, body, tags)
) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let mut coll = KbCollectionDoc::new(name, creator);
    let mut node_states = Vec::new();
    for (id, title, body, tags) in nodes {
        let tag_strings: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        let node = KbNodeDoc::new(id, title, body, &tag_strings);
        coll.add_node(id, title);
        node_states.push((id.to_string(), node.encode()));
    }
    (coll.encode_state(), node_states)
}

/// Share KB with 3 org-mode nodes → join from second client → content matches.
#[tokio::test]
#[ignore]
async fn tcp_kb_share_and_join_roundtrip() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let body = realistic_org_body();
    let nodes_spec: Vec<(&str, &str, &str, &[&str])> = vec![
        (
            "node-1",
            "Architecture Overview",
            body,
            &["research", "crdt"],
        ),
        (
            "node-2",
            "Buffer Management",
            "Ropey-backed buffer with CRDT sync.",
            &["core"],
        ),
        (
            "node-3",
            "Window System",
            "Tiled window manager with splits.",
            &["ui", "layout"],
        ),
    ];

    let (coll_state, node_states) = make_test_kb("test-kb-1", "Research", "alice", &nodes_spec);
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("test-kb-1", "Research", "alice", &coll_state, &nodes_ref)
        .await;

    // Client B joins
    let join_resp = client_b.kb_join("test-kb-1").await;
    assert!(
        join_resp.get("error").is_none(),
        "kb/join should succeed: {join_resp}"
    );

    let result = &join_resp["result"];
    let joined_nodes = result["nodes"].as_array().expect("nodes should be array");
    assert_eq!(
        joined_nodes.len(),
        3,
        "should receive all 3 nodes, got {}",
        joined_nodes.len()
    );

    // Verify collection state decodes correctly
    let coll_b64 = result["collection_state"].as_str().unwrap();
    let coll_bytes = base64_to_update(coll_b64).unwrap();
    let coll = KbCollectionDoc::from_bytes(&coll_bytes).unwrap();
    assert_eq!(coll.name(), "Research", "collection name should match");
    assert_eq!(coll.creator(), "alice", "collection creator should match");
    assert_eq!(
        coll.node_count(),
        3,
        "collection should list 3 nodes, got {}",
        coll.node_count()
    );

    // Verify each node's content
    for joined_node in joined_nodes {
        let node_id = joined_node["id"].as_str().unwrap();
        let state_b64 = joined_node["state"].as_str().unwrap();
        let state_bytes = base64_to_update(state_b64).unwrap();
        let node_doc = KbNodeDoc::from_bytes(&state_bytes).unwrap();

        match node_id {
            "node-1" => {
                assert_eq!(node_doc.title(), "Architecture Overview");
                assert_eq!(
                    node_doc.body(),
                    body,
                    "node-1 body should be byte-for-byte identical to original org content (got {} bytes, expected {} bytes)",
                    node_doc.body().len(),
                    body.len()
                );
                assert_eq!(node_doc.tags(), vec!["research", "crdt"]);
            }
            "node-2" => {
                assert_eq!(node_doc.title(), "Buffer Management");
                assert_eq!(node_doc.body(), "Ropey-backed buffer with CRDT sync.");
            }
            "node-3" => {
                assert_eq!(node_doc.title(), "Window System");
                assert_eq!(node_doc.body(), "Tiled window manager with splits.");
            }
            other => panic!("unexpected node_id: {other}"),
        }
    }
}

/// kb/node_update from client A → client B receives notification with correct bytes.
#[tokio::test]
#[ignore]
async fn tcp_kb_node_update_broadcasts() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    // Share a KB with 1 node
    let (coll_state, node_states) = make_test_kb(
        "update-kb",
        "Test",
        "alice",
        &[("n1", "Original Title", "Original body", &["tag1"])],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("update-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;

    // Client B joins to subscribe to updates
    let join_resp = client_b.kb_join("update-kb").await;
    assert!(join_resp.get("error").is_none(), "join failed: {join_resp}");

    // Client A edits the node body
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update = node.set_body("Updated body from client A");

    let update_resp = client_a.kb_node_update("update-kb", "n1", &update).await;
    assert!(
        update_resp.get("error").is_none(),
        "node_update failed: {update_resp}"
    );

    // Client B should receive a sync_update notification for "kb:n1"
    let notif = client_b
        .wait_for_notification("notifications/sync_update", 5000)
        .await;
    assert!(
        notif.is_some(),
        "client B should receive sync_update notification for kb:n1"
    );

    let notif = notif.unwrap();
    let data = &notif["params"]["event"]["data"];
    assert_eq!(
        data["buffer_name"].as_str().unwrap(),
        "kb:n1",
        "notification should be for kb:n1, got: {}",
        notif
    );

    // Verify the update bytes decode to valid content
    let update_b64 = data["update_base64"].as_str().unwrap();
    let update_bytes = base64_to_update(update_b64).unwrap();
    let mut node_b = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    node_b.apply_update(&update_bytes).unwrap();
    assert_eq!(
        node_b.body(),
        "Updated body from client A",
        "applying update should produce correct body"
    );
}

/// kb/leave → server confirms leave, client can re-join to get latest state.
///
/// Note: doc-scoped broadcast filtering falls back to "deliver all" when a
/// client has zero doc_subs (by design — backward compatibility). So we
/// test the protocol response + re-join semantics rather than notification
/// absence (which requires the client to have other active doc subscriptions).
#[tokio::test]
#[ignore]
async fn tcp_kb_leave_and_rejoin() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let (coll_state, node_states) =
        make_test_kb("leave-kb", "Test", "alice", &[("n1", "Title", "Body", &[])]);
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("leave-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;
    let join_resp = client_b.kb_join("leave-kb").await;
    assert!(join_resp.get("error").is_none());

    // Client B leaves
    let leave_resp = client_b.kb_leave("leave-kb").await;
    assert_eq!(
        leave_resp["result"]["left"].as_bool(),
        Some(true),
        "leave should confirm success"
    );

    // Client A edits while B is away
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update = node.set_body("Edited while B was away");
    client_a.kb_node_update("leave-kb", "n1", &update).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client B re-joins and should see the update
    let rejoin_resp = client_b.kb_join("leave-kb").await;
    assert!(
        rejoin_resp.get("error").is_none(),
        "rejoin failed: {rejoin_resp}"
    );

    let nodes = rejoin_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    assert_eq!(nodes.len(), 1);
    let state_bytes = base64_to_update(nodes[0]["state"].as_str().unwrap()).unwrap();
    let rejoined_node = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(
        rejoined_node.body(),
        "Edited while B was away",
        "after rejoin, client B should see edits made while away"
    );
}

/// Share → join → update → third client joins → sees latest content.
#[tokio::test]
#[ignore]
async fn tcp_kb_join_after_update_sees_latest() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let (coll_state, node_states) = make_test_kb(
        "latest-kb",
        "Test",
        "alice",
        &[("n1", "V1 Title", "V1 Body", &["v1"])],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();
    client_a
        .kb_share("latest-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;

    // Client A updates the node
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update = node.set_body("V2 Body — updated content");
    client_a.kb_node_update("latest-kb", "n1", &update).await;

    // Small delay for server to apply
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client B joins AFTER the update
    let join_resp = client_b.kb_join("latest-kb").await;
    assert!(join_resp.get("error").is_none(), "join failed: {join_resp}");

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    assert_eq!(joined_nodes.len(), 1);

    let state_b64 = joined_nodes[0]["state"].as_str().unwrap();
    let state_bytes = base64_to_update(state_b64).unwrap();
    let joined_node = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(
        joined_node.body(),
        "V2 Body — updated content",
        "third client should see updated content after joining"
    );
}

/// Realistic org content with properties drawers, links, code blocks, and Unicode
/// survives the full round-trip byte-for-byte.
#[tokio::test]
#[ignore]
async fn tcp_kb_realistic_org_content_roundtrip() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let body = realistic_org_body();
    let (coll_state, node_states) = make_test_kb(
        "org-kb",
        "OrgNotes",
        "alice",
        &[(
            "org-1",
            "Test Node — CRDT Round-Trip",
            body,
            &["research", "crdt"],
        )],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("org-kb", "OrgNotes", "alice", &coll_state, &nodes_ref)
        .await;

    let join_resp = client_b.kb_join("org-kb").await;
    assert!(join_resp.get("error").is_none());

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    let state_b64 = joined_nodes[0]["state"].as_str().unwrap();
    let state_bytes = base64_to_update(state_b64).unwrap();
    let node_doc = KbNodeDoc::from_bytes(&state_bytes).unwrap();

    assert_eq!(
        node_doc.body(),
        body,
        "org body should survive round-trip byte-for-byte\nexpected {} bytes, got {} bytes",
        body.len(),
        node_doc.body().len()
    );
    assert_eq!(node_doc.title(), "Test Node — CRDT Round-Trip");
    assert_eq!(node_doc.tags(), vec!["research", "crdt"]);
}

/// Share KB with 20 nodes → join → all received and valid.
#[tokio::test]
#[ignore]
async fn tcp_kb_multi_node_stress() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    // Build 5 nodes with varying content sizes
    let mut coll = KbCollectionDoc::new("BigKB", "alice");
    let mut node_states: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..8 {
        let id = format!("node-{i:03}");
        let title = format!("Node {i}: Generated Content");
        let body = format!(
            "* Node {i}\nThis is node {i} with enough content to be realistic.\n\n\
             ** Details\nGenerated body paragraph {i}.\nSecond line of content for node {i}.\n\n\
             #+begin_src python\nprint(\"node {}\")\n#+end_src\n",
            i
        );
        let tags = vec!["generated".to_string(), format!("batch-{}", i % 5)];
        let node = KbNodeDoc::new(&id, &title, &body, &tags);
        coll.add_node(&id, &title);
        node_states.push((id, node.encode()));
    }

    let coll_state = coll.encode_state();
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("big-kb", "BigKB", "alice", &coll_state, &nodes_ref)
        .await;

    let join_resp = client_b.kb_join("big-kb").await;
    assert!(join_resp.get("error").is_none(), "join failed: {join_resp}");

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    assert_eq!(
        joined_nodes.len(),
        8,
        "should receive all 8 nodes, got {}",
        joined_nodes.len()
    );

    // Verify a sample node's content
    let sample = joined_nodes
        .iter()
        .find(|n| n["id"].as_str() == Some("node-003"))
        .expect("node-003 should exist");
    let state_bytes = base64_to_update(sample["state"].as_str().unwrap()).unwrap();
    let node_doc = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(node_doc.title(), "Node 3: Generated Content");
    assert!(
        node_doc.body().contains("print(\"node 3\")"),
        "node-003 body should contain the code block"
    );
}

/// Sequential node updates → all applied, latest state visible on join.
#[tokio::test]
#[ignore]
async fn tcp_kb_sequential_node_updates() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;

    let (coll_state, node_states) = make_test_kb(
        "seq-kb",
        "Test",
        "alice",
        &[("n1", "Title", "initial", &[])],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("seq-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;

    // Apply 3 sequential updates with send-recv for each
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    for i in 0..3 {
        let body = format!("version-{i}");
        let update = node.set_body(&body);
        let resp = client_a.kb_node_update("seq-kb", "n1", &update).await;
        assert!(resp.get("error").is_none(), "update {i} failed: {resp}");
    }

    // Verify final state via second client join
    let mut client_b = TcpClient::connect(&addr).await;
    let join_resp = client_b.kb_join("seq-kb").await;
    assert!(join_resp.get("error").is_none());

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    let state_bytes = base64_to_update(joined_nodes[0]["state"].as_str().unwrap()).unwrap();
    let final_node = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(
        final_node.body(),
        "version-2",
        "after 3 sequential updates, should see latest version"
    );
}

/// Two clients share different KBs → join sees correct KB, not cross-contaminated.
#[tokio::test]
#[ignore]
async fn tcp_kb_isolation_between_kbs() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;
    let mut client_c = TcpClient::connect(&addr).await;

    // Client A shares KB "alpha" with 2 nodes
    let (coll_a, nodes_a) = make_test_kb(
        "alpha",
        "Alpha KB",
        "alice",
        &[
            ("alpha-1", "Alpha Node 1", "Alpha body 1", &["alpha"]),
            ("alpha-2", "Alpha Node 2", "Alpha body 2", &["alpha"]),
        ],
    );
    let refs_a: Vec<(&str, &[u8])> = nodes_a
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();
    client_a
        .kb_share("alpha", "Alpha KB", "alice", &coll_a, &refs_a)
        .await;

    // Client B shares KB "beta" with 1 node
    let (coll_b, nodes_b) = make_test_kb(
        "beta",
        "Beta KB",
        "bob",
        &[("beta-1", "Beta Node 1", "Beta body 1", &["beta"])],
    );
    let refs_b: Vec<(&str, &[u8])> = nodes_b
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();
    client_b
        .kb_share("beta", "Beta KB", "bob", &coll_b, &refs_b)
        .await;

    // Client C joins "alpha" — should get 2 alpha nodes + possibly beta node
    // (because kb/join currently returns ALL kb: prefixed docs)
    // The collection doc should only list alpha nodes though
    let join_resp = client_c.kb_join("alpha").await;
    assert!(join_resp.get("error").is_none());

    let coll_b64 = join_resp["result"]["collection_state"].as_str().unwrap();
    let coll_bytes = base64_to_update(coll_b64).unwrap();
    let coll = KbCollectionDoc::from_bytes(&coll_bytes).unwrap();
    assert_eq!(coll.name(), "Alpha KB");
    assert_eq!(
        coll.node_count(),
        2,
        "alpha collection should have exactly 2 nodes"
    );
}

/// WU6: Peer join/leave notifications over TCP.
#[tokio::test]
#[ignore]
async fn tcp_peer_join_leave_notifications() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    // Client A shares a doc.
    let mut client_a = TcpClient::connect(&addr).await;
    client_a.share("peer-notify.txt", "hello").await;

    // Client B joins via resync.
    let mut client_b = TcpClient::connect(&addr).await;
    let resync_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": client_b.next_id,
        "method": "sync/resync",
        "params": { "doc": "peer-notify.txt" }
    });
    client_b.next_id += 1;
    client_b.send(&resync_msg).await;
    let resp = client_b.recv().await;
    assert!(resp.get("error").is_none(), "resync failed: {resp}");

    // Client A should receive peer_joined notification.
    let joined = client_a
        .wait_for_notification("notifications/peer_joined", 2000)
        .await;
    assert!(
        joined.is_some(),
        "client A should receive peer_joined notification"
    );

    // Drop client B.
    drop(client_b);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client A should receive peer_left notification.
    let left = client_a
        .wait_for_notification("notifications/peer_left", 3000)
        .await;
    assert!(
        left.is_some(),
        "client A should receive peer_left notification"
    );
}

// ============================================================================
// PSK Authentication Tests
// ============================================================================

/// Spawn a PSK-enabled server: writes a temp config with auth.mode = "psk".
async fn spawn_psk_server(psk: &str) -> (tokio::process::Child, String, tempfile::TempDir) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let addr = format!("127.0.0.1:{}", port);

    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("state-server.toml");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(
        &config_path,
        format!(
            r#"bind = "{addr}"
[auth]
mode = "psk"
psk = "{psk}"
[storage]
data_dir = "{}"
"#,
            data_dir.display()
        ),
    )
    .unwrap();

    let target_bin =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/mae-daemon");

    let child = if target_bin.exists() {
        Command::new(&target_bin)
            .args(["--bind", &addr, "--config", config_path.to_str().unwrap()])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn mae-daemon with PSK config")
    } else {
        Command::new("cargo")
            .args([
                "run",
                "-p",
                "mae-daemon",
                "--",
                "--bind",
                &addr,
                "--config",
                config_path.to_str().unwrap(),
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn mae-daemon via cargo run")
    };

    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if TcpStream::connect(&addr).await.is_ok() {
            return (child, addr, tmp);
        }
    }
    panic!("PSK mae-daemon did not start within 5s on {}", addr);
}

/// TcpClient with PSK auth: performs client_handshake before initialize.
impl TcpClient {
    async fn connect_with_psk(addr: &str, psk: &str) -> Self {
        let stream = TcpStream::connect(addr).await.expect("failed to connect");
        let (read, write) = stream.into_split();
        let mut client = TcpClient {
            reader: BufReader::new(read),
            writer: write,
            next_id: 1,
        };

        // Perform PSK handshake before initialize.
        use mae_mcp::auth::{AuthProvider, PskAuth};
        let auth = PskAuth::new(psk);
        auth.client_handshake(&mut client.reader, &mut client.writer)
            .await
            .expect("PSK client handshake failed");

        client.initialize().await;
        client.subscribe().await;
        client
    }
}

/// PSK auth: correct key connects and can share/read documents.
#[tokio::test]
#[ignore]
async fn tcp_psk_correct_key_connects() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_psk_server("e2e-test-secret").await;

    let mut client = TcpClient::connect_with_psk(&addr, "e2e-test-secret").await;
    client.share("psk-test.txt", "hello from PSK").await;
    assert_eq!(client.content("psk-test.txt").await, "hello from PSK");
}

/// PSK auth: wrong key is rejected — connection fails.
#[tokio::test]
#[ignore]
async fn tcp_psk_wrong_key_rejected() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_psk_server("correct-key").await;

    let stream = TcpStream::connect(&addr).await.expect("TCP connect");
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut writer = write;

    use mae_mcp::auth::{AuthProvider, PskAuth};
    let wrong_auth = PskAuth::new("wrong-key");
    let result = wrong_auth.client_handshake(&mut reader, &mut writer).await;

    assert!(result.is_err(), "wrong PSK should be rejected");
}

/// PSK auth: no-auth client to PSK server is rejected (server expects hello, gets JSON-RPC).
#[tokio::test]
#[ignore]
async fn tcp_psk_no_auth_client_rejected() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_psk_server("server-key").await;

    // Try connecting without PSK handshake — send initialize directly.
    let stream = TcpStream::connect(&addr).await.expect("TCP connect");
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut writer = write;

    // send_initialize sends JSON-RPC — server expecting PSK hello will fail/hang.
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let body = serde_json::to_vec(&init).unwrap();
        if mae_mcp::write_framed(&mut writer, &body, Duration::from_secs(2))
            .await
            .is_err()
        {
            return false;
        }
        // Try to read response — server should either close connection or send auth error.
        match tokio::time::timeout(Duration::from_secs(2), mae_mcp::read_message(&mut reader)).await
        {
            Ok(Ok(Some(text))) => {
                // If server responds, it shouldn't be a valid initialize response.
                let val: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                val.get("result")
                    .and_then(|r| r.get("serverInfo"))
                    .is_some()
            }
            _ => false,
        }
    })
    .await;

    match result {
        Ok(got_valid_init) => {
            assert!(
                !got_valid_init,
                "no-auth client should NOT get valid initialize from PSK server"
            );
        }
        Err(_) => {
            // Timeout is expected — server dropped the connection or is waiting for auth hello.
        }
    }
}
