//! Tier 2 — TCP integration tests (real server).
//!
//! Gated with `#[ignore]` — run via:
//!   MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e -- --ignored --nocapture
//!
//! Spawns `mae-state-server` on a random port, connects via real TCP.

use std::process::Stdio;
use std::time::Duration;

use mae_sync::encoding::{base64_to_update, update_to_base64};
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

/// Spawn mae-state-server on a random port, wait for it to listen, return (child, port).
async fn spawn_server() -> (tokio::process::Child, String) {
    // Find a free port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let addr = format!("127.0.0.1:{}", port);

    let child = Command::new("cargo")
        .args(["run", "-p", "mae-state-server", "--", "--bind", &addr])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn mae-state-server");

    // Wait for server to accept connections.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if TcpStream::connect(&addr).await.is_ok() {
            return (child, addr);
        }
    }
    panic!("mae-state-server did not start within 5s on {}", addr);
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
    let (_server, addr) = spawn_server().await;

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
    let (_server, addr) = spawn_server().await;

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
    let (_server, addr) = spawn_server().await;

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
    let (_server, addr) = spawn_server().await;

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
    let (mut server, addr) = spawn_server().await;

    let mut client = TcpClient::connect(&addr).await;
    client.share("tcp-reconnect.txt", "before restart").await;

    // Kill the server.
    server.kill().await.expect("failed to kill server");

    // Wait for it to die.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Restart on the same port.
    let _server2 = Command::new("cargo")
        .args(["run", "-p", "mae-state-server", "--", "--bind", &addr])
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
    let (mut server, addr) = spawn_server().await;

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
        .args(["run", "-p", "mae-state-server", "--", "--bind", &addr])
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
    let (_server, addr) = spawn_server().await;

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

/// WU6: Peer join/leave notifications over TCP.
#[tokio::test]
#[ignore]
async fn tcp_peer_join_leave_notifications() {
    if !should_run() {
        return;
    }
    let (_server, addr) = spawn_server().await;

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
