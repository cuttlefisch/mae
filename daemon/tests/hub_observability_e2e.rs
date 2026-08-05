//! Is the hub observably a hub?
//!
//! Split out of `network_e2e.rs` when that file hit its size ceiling. Gated on
//! `MAE_TCP_E2E` like the rest of the real-TCP suite; the harness is shared via
//! `tests/common/mod.rs`.

mod common;

use common::{await_collab_active, daemon_status, send_recv, spawn_server};

/// The claim a hub-and-spoke deployment exists to make — "N clients are
/// connected to the hub" — has to be observable from the server, and until
/// `daemon/status` grew a `connections` field it was not: the count existed only
/// per-document, or as a number broadcast to subscribers of one document.
///
/// This is the real shape: a spawned daemon, real TCP collab clients, and the
/// count read over the KB Unix socket. The in-process tests in
/// `src/tests/connection_observability_tests.rs` cover the KB socket's own
/// counter; only this one exercises the collab listener's.
#[tokio::test]
async fn tcp_daemon_status_reports_connected_collab_clients() {
    let Some(server) = spawn_server().await else {
        return;
    };

    let before = daemon_status(&server.socket).await;
    assert!(
        before["connections"]["collab"].is_object(),
        "the collab listener is running, so it must be reported: {before:?}"
    );
    // `spawn_server`'s readiness probe is itself a real TCP connection, so the
    // count may not be 0 the instant it returns — wait for the baseline rather
    // than asserting into a race. (Asserting immediately here passed once and
    // failed the next run; a flaky gauge test is worse than none.)
    let baseline = await_collab_active(&server.socket, 0).await;
    assert_eq!(
        baseline,
        Some(0),
        "the hub must settle to zero collab clients before any connect"
    );

    // Three real editors' worth of connections, each completing a genuine
    // JSON-RPC exchange so they are attached rather than merely dialed.
    let mut clients = Vec::new();
    for i in 1..=3u64 {
        let mut c = tokio::net::TcpStream::connect(server.addr).await.unwrap();
        let resp = send_recv(
            &mut c,
            &serde_json::json!({
                "jsonrpc": "2.0", "id": i, "method": "initialize",
                "params": {"clientInfo": {"name": format!("hub-client-{i}")}}
            }),
        )
        .await;
        assert!(resp.error.is_none(), "client {i} failed to initialize");
        clients.push(c);

        let s = daemon_status(&server.socket).await;
        assert_eq!(
            s["connections"]["collab"]["active"].as_u64(),
            Some(i),
            "after {i} collab client(s) the hub must report {i}: {s:?}"
        );
    }

    // `sessions` (authenticated sync sessions, as distinct from accepted TCP
    // connections) is deliberately ABSENT here, and that absence is asserted
    // rather than skipped past: this daemon runs `auth.mode = "none"` (no
    // daemon.toml), and `DaemonState.broadcaster` — the only source of that
    // number — is installed by `main.rs` solely under key-mode auth. A test
    // that merely tolerated a missing field would silently stop covering
    // `sessions` if that wiring changed; this one fails and points at #647.
    let s = daemon_status(&server.socket).await;
    assert!(
        s["connections"]["collab"]["sessions"].is_null(),
        "auth.mode=none installs no broadcaster, so `sessions` must be absent \
         (see #647). If this now reports a number, the wiring changed — extend \
         this test to assert it equals `active` instead of removing it: {s:?}"
    );

    // …and it comes back down. A gauge that only rises reads "at capacity" on an
    // idle hub and would then have the limiter refuse real clients.
    drop(clients);
    assert_eq!(
        await_collab_active(&server.socket, 0).await,
        Some(0),
        "disconnecting every collab client must return the count to 0"
    );
}
