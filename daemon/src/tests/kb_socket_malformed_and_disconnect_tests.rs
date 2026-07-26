//! ADR-060 Phase B's two remaining named adversarial cases (beyond the
//! slow-query isolation test in `handler.rs`'s own test module):
//!
//! 1. "A malformed or oversized RPC addressed at tenant A's instance must
//!    not starve or crash tenant B's connection or session state."
//! 2. Named reproduction of the Emacs bug#11639/bug#23499 shape: "a client
//!    that disconnects mid-request, or sends a malformed follow-up message
//!    after a valid handshake, must not hang the shared directory lock ...
//!    for other tenants' unrelated RPCs."
//!
//! Both are already structurally prevented by this daemon's existing
//! architecture, verified here rather than assumed: `accept_loop` spawns one
//! independent `tokio::task` per connection (`main.rs`'s `handle_client`),
//! and no task ever holds `Arc<Mutex<DaemonState>>` across a blocking read
//! or a slow query (ADR-054's snapshot-then-drop, generalized). A malformed
//! message or a stalled/dropped connection can only ever end or block ITS
//! OWN task — never the shared state, and never another connection's task.

use super::*;

#[tokio::test]
async fn malformed_json_on_one_connection_does_not_starve_or_hang_another() {
    let state = Arc::new(Mutex::new(seeded_two_store_state()));
    let socket = spawn_kb_socket(Arc::clone(&state), 0, Duration::ZERO).await;

    // Tenant A: a real Content-Length-framed message whose body is garbage,
    // not merely a malformed frame header (mae-mcp's own framing layer
    // already covers header-level malformity elsewhere, e.g.
    // shared/mcp/src/lib.rs's `framing_*` tests) -- this exercises
    // `handle_client`'s own `serde_json::from_str(&msg)?` failure path.
    let mut a_stream = UnixStream::connect(&socket.path).await.expect("connect a");
    {
        let (_r, mut w) = a_stream.split();
        let garbage = b"{ this is not valid json at all !! ";
        mae_mcp::write_framed(&mut w, garbage, Duration::from_secs(2))
            .await
            .expect("write malformed body");
    }

    // Tenant B, on a SEPARATE connection, issues a real, valid RPC
    // concurrently -- it must complete promptly, not wait on A's malformed
    // message in any way (no shared lock, no shared read state).
    let start = std::time::Instant::now();
    let mut b_stream = UnixStream::connect(&socket.path).await.expect("connect b");
    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        call(
            &mut b_stream,
            "kb/search",
            json!({"query": "beta", "limit": 10}),
        ),
    )
    .await
    .expect("tenant B's request must not hang behind tenant A's malformed message");
    assert!(
        resp.get("error").is_none(),
        "tenant B's own valid request must succeed: {resp:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "tenant B's request took {:?} -- a malformed message on another connection must never \
         add measurable latency to an unrelated tenant's request",
        start.elapsed()
    );

    // A's own connection is left to be torn down by the server (its task
    // exits on the JSON parse error) -- not asserted here, since the
    // property under test is B's isolation, not A's specific error shape.
    drop(a_stream);
}

#[tokio::test]
async fn client_disconnect_mid_request_does_not_hang_other_tenants_rpcs() {
    // Named reproduction of the Emacs bug#11639/bug#23499 shape: a client
    // that vanishes mid-request must not wedge shared server state.
    let state = Arc::new(Mutex::new(seeded_two_store_state()));
    let socket = spawn_kb_socket(Arc::clone(&state), 0, Duration::ZERO).await;

    // Tenant A: send a Content-Length header promising a body, then write
    // only PART of that body, then drop the connection without ever
    // completing the message -- the daemon's `read_message` is left
    // waiting on bytes that will never arrive, then observes the drop.
    {
        let mut a_stream = UnixStream::connect(&socket.path).await.expect("connect a");
        let full_body =
            br#"{"jsonrpc": "2.0", "id": 1, "method": "kb/search", "params": {"query": "x"}}"#;
        let header = format!("Content-Length: {}\r\n\r\n", full_body.len());
        use tokio::io::AsyncWriteExt;
        a_stream
            .write_all(header.as_bytes())
            .await
            .expect("write header");
        // Only the first half of the promised body -- a genuine mid-message
        // disconnect, not a clean "sent nothing at all" case (already
        // covered by the idle-timeout tests).
        a_stream
            .write_all(&full_body[..full_body.len() / 2])
            .await
            .expect("write partial body");
        a_stream.flush().await.expect("flush partial body");
        // Dropping here closes the socket mid-message -- no graceful
        // shutdown, no completed request.
    }

    // Give the server's per-connection task a moment to observe the drop
    // and unwind -- this must happen without touching shared state either
    // way, so this sleep is just letting the OS/task scheduler catch up,
    // not something the assertion below depends on for correctness.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Tenant B, on its own connection, must be completely unaffected --
    // real request, prompt real response.
    let start = std::time::Instant::now();
    let mut b_stream = UnixStream::connect(&socket.path).await.expect("connect b");
    let resp = tokio::time::timeout(
        Duration::from_secs(5),
        call(
            &mut b_stream,
            "kb/search",
            json!({"query": "delta", "limit": 10}),
        ),
    )
    .await
    .expect(
        "tenant B's request must not hang behind tenant A's mid-request disconnect -- this is \
         the specific Emacs bug#11639/bug#23499 shape ADR-060 Phase B names explicitly",
    );
    assert!(
        resp.get("error").is_none(),
        "tenant B's own valid request must succeed: {resp:?}"
    );
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "tenant B's request took {:?} -- a disconnected peer on another connection must never \
         add measurable latency to an unrelated tenant's request",
        start.elapsed()
    );

    // A THIRD, independent connection concurrently issued WHILE a fresh
    // "connect but never send anything" client sits open in the background
    // -- proving the property holds even with a currently-stalled (not yet
    // disconnected) peer present, not only after the disconnect completes.
    let _stalled_c = UnixStream::connect(&socket.path)
        .await
        .expect("connect stalled c");
    // c never sends a byte -- it just sits there, exactly like a client
    // that hung after a valid handshake but before its next request.
    let mut d_stream = UnixStream::connect(&socket.path).await.expect("connect d");
    let resp_d = tokio::time::timeout(
        Duration::from_secs(5),
        call(&mut d_stream, "daemon/status", json!({})),
    )
    .await
    .expect("tenant D's request must not hang behind a currently-stalled peer connection");
    assert!(
        resp_d.get("error").is_none(),
        "tenant D's request must succeed: {resp_d:?}"
    );
}
