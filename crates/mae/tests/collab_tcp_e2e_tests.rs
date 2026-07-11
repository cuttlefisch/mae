//! Tier 2 — TCP integration tests (real server), part 1: connection/sync tests.
//!
//! Split from collab_tcp_e2e.rs (was 1431 lines, over the 500-line test
//! ceiling); shared helpers live in collab_tcp_e2e_support/mod.rs.
//!
//! Gated with `#[ignore]` — run via:
//!   MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e_tests -- --ignored --nocapture

use std::time::Duration;

use mae_sync::encoding::update_to_base64;
use mae_sync::text::TextSync;
use tokio::net::TcpStream;

mod collab_tcp_e2e_support;
use collab_tcp_e2e_support::*;

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
    let _server2 = spawn_daemon(&["--bind", &addr]);

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
    let _server2 = spawn_daemon(&["--bind", &addr]);

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
