//! `daemon/status` must report how many clients are attached.
//!
//! Before this, `daemon/status` reported version, uptime, stores, registered
//! instances, KB collections and live tenants — and nothing at all about
//! connections. The only connection counts in the daemon were per-document
//! (`docs/metadata`) or broadcast to subscribers of one document, so "three
//! editors are connected to the hub" was not a claim anyone could check. That is
//! the exact claim a hub-and-spoke deployment exists to make.
//!
//! These tests use the real `accept_loop` over a real Unix socket, so the count
//! is exercised through genuine connect/disconnect rather than by poking the
//! counter directly.

use super::*;

async fn status(stream: &mut UnixStream) -> Value {
    let resp = call(stream, "daemon/status", json!({})).await;
    assert!(
        resp.get("error").is_none(),
        "daemon/status failed: {resp:?}"
    );
    resp["result"].clone()
}

fn kb_active(status: &Value) -> u64 {
    status["connections"]["kb_socket"]["active"]
        .as_u64()
        .unwrap_or_else(|| panic!("no connections.kb_socket.active in {status:?}"))
}

#[tokio::test]
async fn status_reports_a_connection_count_that_tracks_real_clients() {
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let socket = spawn_kb_socket(Arc::clone(&state), 8, Duration::from_secs(30)).await;

    // One client, asking about itself: it is connected, so the count includes it.
    let mut a = UnixStream::connect(&socket.path).await.expect("connect a");
    let s = status(&mut a).await;
    assert_eq!(
        kb_active(&s),
        1,
        "the asking client is itself a connection: {s:?}"
    );
    assert_eq!(
        s["connections"]["kb_socket"]["max"].as_u64(),
        Some(8),
        "the cap must be reported next to the count, so `3 connected` can be \
         read as `3 of 8` without opening the config: {s:?}"
    );

    // Hold several more open. The oracle is that the number CHANGES with the
    // number of clients — a hardcoded constant or a stale snapshot passes a
    // "field exists" assertion and fails this one.
    let mut held = Vec::new();
    for i in 2..=5u64 {
        let mut c = UnixStream::connect(&socket.path).await.expect("connect");
        let s = status(&mut c).await;
        assert_eq!(
            kb_active(&s),
            i,
            "expected {i} live connections after opening {i}: {s:?}"
        );
        held.push(c);
    }

    // …and back down as they close. `ConnGuard` decrements on drop, but the
    // server task only drops it once it notices EOF, so poll rather than assume
    // it is instantaneous.
    drop(held);
    let mut settled = None;
    for _ in 0..50 {
        let n = kb_active(&status(&mut a).await);
        if n == 1 {
            settled = Some(n);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        settled,
        Some(1),
        "closing 4 of 5 clients must bring the count back to 1 (the surviving \
         asker); a count that only ever rises is a leak, not a gauge"
    );
}

#[tokio::test]
async fn a_listener_that_is_not_running_is_absent_rather_than_zero() {
    // Zero connections and no listener are different facts. Reporting a
    // disabled collab server as `collab: {active: 0}` makes a hub that never
    // started look like a healthy idle one — the failure this deployment most
    // needs to be able to see.
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let socket = spawn_kb_socket(Arc::clone(&state), 4, Duration::from_secs(30)).await;
    let mut c = UnixStream::connect(&socket.path).await.expect("connect");
    let s = status(&mut c).await;

    assert!(
        s["connections"]["collab"].is_null(),
        "collab is not running in this harness, so it must not be reported at \
         all: {s:?}"
    );
    assert!(
        s["connections"]["kb_socket"].is_object(),
        "the KB socket IS running and must be reported — otherwise the previous \
         assertion passes vacuously because nothing is ever reported: {s:?}"
    );
}

#[tokio::test]
async fn the_count_survives_a_client_that_dies_without_closing_cleanly() {
    // A client that vanishes (killed editor, severed network) must not hold a
    // slot forever: an over-reporting gauge eventually reads "at capacity" on an
    // idle hub, and `try_acquire` then refuses real clients.
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let socket = spawn_kb_socket(Arc::clone(&state), 4, Duration::from_secs(30)).await;

    let mut observer = UnixStream::connect(&socket.path).await.expect("connect");
    assert_eq!(kb_active(&status(&mut observer).await), 1);

    {
        // Connect, send a request, read the reply, then drop the socket without
        // any shutdown handshake — the abrupt shape, not a graceful close.
        let mut doomed = UnixStream::connect(&socket.path).await.expect("connect");
        assert_eq!(kb_active(&status(&mut doomed).await), 2);
    }

    let mut settled = None;
    for _ in 0..50 {
        let n = kb_active(&status(&mut observer).await);
        if n == 1 {
            settled = Some(n);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        settled,
        Some(1),
        "an abruptly-dropped client must release its slot"
    );
}

#[tokio::test]
async fn collab_sessions_are_reported_when_a_broadcaster_exists() {
    // `sessions` counts authenticated sync sessions, as distinct from accepted
    // TCP connections. Its only source is `DaemonState.broadcaster`, which
    // `main.rs` installs solely under key-mode auth (#647) — so the real-daemon
    // e2e runs with `auth.mode = none` and can only assert its ABSENCE. This
    // test drives the present case, so the branch is covered rather than dead.
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let bc: mae_mcp::broadcast::SharedBroadcaster = Arc::new(std::sync::Mutex::new(
        mae_mcp::broadcast::EventBroadcaster::new(),
    ));
    {
        let mut s = state.lock().await;
        s.broadcaster = Some(Arc::clone(&bc));
        // A collab listener must exist for `sessions` to have anywhere to live —
        // it is nested under `collab`, not reported on its own.
        s.collab_conn = Some(crate::conn_limit::ConnLimiter::new(16));
    }
    let socket = spawn_kb_socket(Arc::clone(&state), 4, Duration::from_secs(30)).await;
    let mut c = UnixStream::connect(&socket.path).await.expect("connect");

    let s = status(&mut c).await;
    assert_eq!(
        s["connections"]["collab"]["sessions"].as_u64(),
        Some(0),
        "a broadcaster with no subscribers reports zero sessions: {s:?}"
    );

    // Subscribe three sessions with distinct ids — varied rather than one
    // cherry-picked value, and enough that an off-by-one or a hardcoded
    // constant would show.
    let _rx: Vec<_> = [11u64, 4242, 999_999]
        .iter()
        .map(|id| {
            bc.lock()
                .unwrap_or_else(|e| e.into_inner())
                .subscribe(*id, vec![])
        })
        .collect();
    let s = status(&mut c).await;
    assert_eq!(
        s["connections"]["collab"]["sessions"].as_u64(),
        Some(3),
        "three subscribed sessions must be reported: {s:?}"
    );
    assert_eq!(
        s["connections"]["collab"]["active"].as_u64(),
        Some(0),
        "sessions and active are different numbers: no TCP connection was made \
         here, so `active` must stay 0 while `sessions` is 3. If these ever \
         track each other, one of them is being computed from the other: {s:?}"
    );

    bc.lock()
        .unwrap_or_else(|e| e.into_inner())
        .unsubscribe(4242);
    let s = status(&mut c).await;
    assert_eq!(
        s["connections"]["collab"]["sessions"].as_u64(),
        Some(2),
        "an unsubscribed session must stop being counted: {s:?}"
    );
}

#[tokio::test]
async fn the_reported_count_and_the_admission_decision_agree() {
    // The number an operator reads must be the number the daemon acts on. If
    // `daemon/status` says 2 of 2 while a third client is still admitted, the
    // gauge is decorative.
    let state = Arc::new(Mutex::new(DaemonState::new()));
    let socket = spawn_kb_socket(Arc::clone(&state), 2, Duration::from_secs(30)).await;

    let a = UnixStream::connect(&socket.path).await.expect("connect a");
    let mut b = UnixStream::connect(&socket.path).await.expect("connect b");
    let s = status(&mut b).await;
    assert_eq!(kb_active(&s), 2, "at the cap: {s:?}");
    assert_eq!(s["connections"]["kb_socket"]["max"].as_u64(), Some(2));

    // At the cap the next client is rejected: closed before any JSON-RPC, so
    // the read side sees EOF or a reset (same shape as
    // `connection_cap_rejects_the_nplus1th_client`).
    let mut over = UnixStream::connect(&socket.path).await.expect("connect");
    let (r, mut w) = over.split();
    let mut reader = tokio::io::BufReader::new(r);
    let body = serde_json::to_vec(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "daemon/status", "params": {}}),
    )
    .unwrap();
    let _ = mae_mcp::write_framed(&mut w, &body, Duration::from_secs(2)).await;
    let outcome = tokio::time::timeout(Duration::from_secs(2), mae_mcp::read_message(&mut reader))
        .await
        .expect("must not hang");
    match outcome {
        Ok(msg) => assert!(
            msg.is_none(),
            "status said the cap was reached, so this client must be rejected: {msg:?}"
        ),
        Err(_) => { /* connection reset — also valid rejection evidence */ }
    }

    drop(a);
    drop(over);
}
