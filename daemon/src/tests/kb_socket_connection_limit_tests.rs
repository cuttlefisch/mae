//! Real-socket coverage for the KB Unix socket's connection cap and idle
//! timeout (ADR-054) — mirrors
//! `collab_handler_connection_limits_tests.rs`/`network_e2e.rs`'s
//! `connection_cap_rejects_the_nplus1th_client` shape, in-process.

use super::*;

#[tokio::test]
async fn connection_cap_rejects_the_nplus1th_client() {
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let socket = spawn_kb_socket(Arc::clone(&state), 2, Duration::ZERO).await;

    // Open 2 connections and keep them alive (matches max_connections=2) —
    // each must complete a real call successfully.
    let mut kept = Vec::new();
    for i in 0..2 {
        let mut stream = UnixStream::connect(&socket.path).await.expect("connect");
        let resp = call(&mut stream, "daemon/status", json!({})).await;
        assert!(
            resp.get("error").is_none(),
            "connection {i} (within the cap) should succeed: {resp:?}"
        );
        kept.push(stream);
    }

    // The 3rd connection exceeds the cap — the daemon closes the socket
    // immediately (before any JSON-RPC), so the read side observes either a
    // clean EOF or a connection-reset-style I/O error (observed in practice
    // on Unix domain sockets when the server drops its half with the
    // client's just-sent bytes still unread) — either is valid rejection
    // evidence; a real JSON-RPC response is not.
    let mut over_cap = UnixStream::connect(&socket.path).await.expect("connect");
    let (r, mut w) = over_cap.split();
    let mut reader = tokio::io::BufReader::new(r);
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": "daemon/status", "params": {}});
    let body = serde_json::to_vec(&req).unwrap();
    // The server may already have closed its end by the time this write
    // runs; a write error here is an acceptable manifestation of rejection.
    let _ = mae_mcp::write_framed(&mut w, &body, Duration::from_secs(2)).await;
    let outcome = tokio::time::timeout(Duration::from_secs(2), mae_mcp::read_message(&mut reader))
        .await
        .expect("read must not hang for a rejected over-cap connection");
    match outcome {
        Ok(msg) => assert!(
            msg.is_none(),
            "the (max_connections+1)th client should be rejected (EOF/reset, no response), got: {msg:?}"
        ),
        Err(_) => {
            // A connection-reset-shaped I/O error is also valid rejection
            // evidence — the server never got far enough to respond.
        }
    }

    drop(kept); // keep the 2 in-cap connections alive until this point
}

/// Verifies the self-healing claim the ADR-054 plan makes explicit: a
/// server-side idle-close must not be a hard failure from the client's
/// perspective — a fresh reconnect immediately after must succeed
/// transparently, exactly like `DaemonClient`'s own reconnect-on-I/O-error
/// behavior in production.
#[tokio::test]
async fn idle_connection_is_closed_after_timeout_and_a_fresh_reconnect_succeeds() {
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let idle_timeout = Duration::from_millis(150);
    let socket = spawn_kb_socket(Arc::clone(&state), 0, idle_timeout).await;

    // A real client that connects and sends NOTHING — past idle_timeout the
    // server must close it on its own.
    let mut idle_stream = UnixStream::connect(&socket.path).await.expect("connect");
    let (r, _w) = idle_stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let msg = tokio::time::timeout(
        idle_timeout + Duration::from_secs(2),
        mae_mcp::read_message(&mut reader),
    )
    .await
    .expect("server must close the idle connection within idle_timeout + margin")
    .expect("read_message io result");
    assert!(
        msg.is_none(),
        "server must close a silently-idle connection past idle_timeout (EOF), got: {msg:?}"
    );
    drop(idle_stream);

    // Self-healing: a fresh connection right after works normally.
    let mut fresh = UnixStream::connect(&socket.path).await.expect("reconnect");
    let resp = call(&mut fresh, "daemon/status", json!({})).await;
    assert!(
        resp.get("error").is_none(),
        "a fresh reconnect after an idle-close must succeed transparently: {resp:?}"
    );
}

/// `idle_timeout = 0` must disable the timeout entirely — a connection that
/// sits silent far longer than any of the above tests' timeouts must still
/// be alive and answer a call whenever the client finally sends one.
#[tokio::test]
async fn zero_idle_timeout_disables_the_timeout() {
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let socket = spawn_kb_socket(Arc::clone(&state), 0, Duration::ZERO).await;

    let mut stream = UnixStream::connect(&socket.path).await.expect("connect");
    // Idle well past every timeout used elsewhere in this file.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let resp = call(&mut stream, "daemon/status", json!({})).await;
    assert!(
        resp.get("error").is_none(),
        "idle_timeout=0 must mean the connection is never closed for idleness: {resp:?}"
    );
}
