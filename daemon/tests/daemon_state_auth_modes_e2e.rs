//! The collab handles in `DaemonState` must be present whenever the collab
//! server is running — not only under key-mode auth.
//!
//! Until #647, `spawn_collab_server` installed `doc_store`, `broadcaster` and
//! `owner` into `DaemonState` inside the `"key" =>` arm of its auth match. Under
//! `psk` or `none` — and `none` is mae-daemon's DEFAULT — the collab server ran
//! normally while all three stayed `None`. Consequences, none of them visible as
//! an error anywhere:
//!
//!   * `daemon/status` reported `kb_collections: []` and `primary_exists: false`
//!     for a daemon that genuinely hosted the primary KB, so an editor reading
//!     those (`should_attach_daemon_reads`) permanently declined to route KB
//!     reads through it;
//!   * `kb/node_crdt` returned `NotReady`, so thin-client node hydration was
//!     dead;
//!   * `connections.collab.sessions` was absent from `daemon/status`.
//!
//! ADR-053 hit the same wall and routed around it — `doc_store_for_query` is
//! constructed in `main()` precisely so the OAuth query surface works
//! "independent of whether the TCP listener's own auth setup succeeds". This
//! suite exists so the fix cannot silently regress into a fourth workaround.
//!
//! Gated on `MAE_TCP_E2E` like the rest of the real-daemon suite; harness shared
//! via `tests/common/mod.rs`.

mod common;

use common::{daemon_status, spawn_server, spawn_server_without_collab};
use std::path::Path;
use std::time::Duration;

/// Call a method on the daemon's KB Unix socket, returning the whole JSON-RPC
/// envelope so a test can assert on `error` as well as `result`.
async fn kb_call(socket: &Path, method: &str, params: serde_json::Value) -> serde_json::Value {
    use tokio::io::BufReader;
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .unwrap_or_else(|e| panic!("connect {}: {e}", socket.display()));
    let (r, mut w) = stream.split();
    let mut reader = BufReader::new(r);
    let body = serde_json::to_vec(
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
    )
    .unwrap();
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(5))
        .await
        .expect("write request");
    let msg = mae_mcp::read_message(&mut reader)
        .await
        .expect("read response")
        .expect("response before EOF");
    serde_json::from_str(&msg).expect("parse response")
}

/// The daemon spawned by `spawn_server` has no `daemon.toml`, so
/// `[collab.auth] mode` takes its default — `"none"`. That is the configuration
/// under test, and also the one a first deployment is most likely to be running.
#[tokio::test]
async fn kb_node_crdt_is_served_under_the_default_auth_mode() {
    let Some(server) = spawn_server().await else {
        return;
    };

    let resp = kb_call(
        &server.socket,
        "kb/node_crdt",
        serde_json::json!({"kb_id": "default", "id": "concept:does-not-exist"}),
    )
    .await;

    // The oracle distinguishes the two "nothing came back" cases that used to be
    // conflated: `NotReady` (the daemon has no doc store at all — the bug) versus
    // a null result (the doc store is there and simply has no such node).
    assert!(
        resp.get("error").is_none(),
        "kb/node_crdt must not error under auth.mode=none. Before #647 the collab \
         doc store was installed only in key mode, so this returned NotReady and \
         thin-client node hydration was dead on every psk/none daemon: {resp:?}"
    );
    assert!(
        resp["result"].is_null(),
        "no such node, so the result should be null (not an error, not data): {resp:?}"
    );
}

/// The non-vacuity control. If `kb/node_crdt` never returned `NotReady` under
/// ANY configuration, the assertion above would pass whether or not the fix is
/// present. With collab genuinely disabled there is no doc store to install, and
/// `NotReady` is the correct answer — so this proves the error path still exists
/// and that the test above is measuring the right thing.
#[tokio::test]
async fn kb_node_crdt_is_not_ready_when_collab_is_disabled() {
    let Some(server) = spawn_server_without_collab("[collab]\nenabled = false\n").await else {
        return;
    };

    // `kb_id` must be present even though this call is expected to fail: the
    // ADR-105 param check runs BEFORE the doc-store lookup, so omitting it yields
    // `InvalidParams` and this control silently stops observing `NotReady` at all.
    let resp = kb_call(
        &server.socket,
        "kb/node_crdt",
        serde_json::json!({"kb_id": "default", "id": "concept:does-not-exist"}),
    )
    .await;

    let err = resp
        .get("error")
        .unwrap_or_else(|| panic!("collab is disabled, so this must error: {resp:?}"));
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("not ready"),
        "expected a not-ready error when there is no collab doc store at all, got: {err:?}"
    );
}

/// ADR-105 Stage 2 made `kb_id` a REQUIRED param of `kb/node_crdt`, because a
/// node id alone cannot name a document once node docs are per-KB. Nothing
/// asserted that at the RPC boundary, and the omission bit immediately: both
/// tests above kept calling without it and were rejected as `InvalidParams`
/// before they reached the behaviour they exist to measure. Pin the rejection so
/// a future defaulting-instead-of-rejecting change — which would silently
/// reintroduce exactly the cross-KB collision Stage 2 removed — has to fail here
/// first.
#[tokio::test]
async fn kb_node_crdt_rejects_a_call_that_does_not_name_a_kb() {
    let Some(server) = spawn_server().await else {
        return;
    };

    let resp = kb_call(
        &server.socket,
        "kb/node_crdt",
        serde_json::json!({"id": "concept:does-not-exist"}),
    )
    .await;

    let err = resp.get("error").unwrap_or_else(|| {
        panic!(
            "an unscoped node address must be refused, never resolved against \
             some default KB: {resp:?}"
        )
    });
    assert!(
        err["message"]
            .as_str()
            .unwrap_or_default()
            .contains("kb_id"),
        "the error must name the missing param rather than fail generically: {err:?}"
    );
}

/// `daemon/status`'s doc-store-derived fields must be answered from a real doc
/// store rather than from the `None` fallback.
#[tokio::test]
async fn daemon_status_reports_collab_state_under_the_default_auth_mode() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let status = daemon_status(&server.socket).await;

    // `kb_collections` is `[]` either way on a fresh daemon — an empty list is
    // NOT evidence of a doc store. `sessions` is: it exists only when the
    // broadcaster was installed, which is the same wiring.
    assert_eq!(
        status["connections"]["collab"]["sessions"].as_u64(),
        Some(0),
        "collab is running with auth.mode=none, so the broadcaster must be \
         installed and `sessions` present (0, no clients yet). Its absence is \
         the #647 symptom: {status:?}"
    );
    assert!(
        status["kb_collections"].is_array(),
        "kb_collections must be an array, not absent: {status:?}"
    );
}

/// With collab disabled there is nothing to report, and the report must say so
/// by omission rather than by reporting zeros — the same distinction the
/// connection counters make.
#[tokio::test]
async fn daemon_status_omits_collab_state_when_collab_is_disabled() {
    let Some(server) = spawn_server_without_collab("[collab]\nenabled = false\n").await else {
        return;
    };
    let status = daemon_status(&server.socket).await;

    assert!(
        status["connections"]["collab"].is_null(),
        "collab is disabled, so it must not be reported at all: {status:?}"
    );
    assert!(
        status["connections"]["kb_socket"].is_object(),
        "the KB socket IS running and must still be reported — otherwise the \
         assertion above passes because nothing is reported: {status:?}"
    );
}
