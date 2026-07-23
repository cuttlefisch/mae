//! In-process daemon concurrency test harness (ADR-054 / issue #379).
//!
//! Lives inside the `mae-daemon` *binary* crate (not `daemon/tests/`,
//! `daemon/src/lib.rs` only re-exports `checkpoint`/`collab_handler`/
//! `doc_store`/`projector`/`storage` — `handler`, `accept_loop`, and
//! `handle_client` are private to the bin crate, which is why the existing
//! `daemon/tests/network_e2e.rs` has to spawn a subprocess at all). A
//! `#[cfg(test)]` module compiled as part of the same bin crate reaches them
//! with zero visibility changes and, unlike `network_e2e.rs`, has neither a
//! subprocess spawn nor real TCP port contention to justify opt-in gating —
//! these tests run in default CI.

use crate::conn_limit::ConnLimiter;
use crate::handler::DaemonState;
use mae_kb::{CozoKbStore, KbStore, Node, NodeKind};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Mutex;

mod kb_socket_concurrency_tests;
mod kb_socket_connection_limit_tests;
mod kb_write_concurrency_tests;

/// Build a `DaemonState` with a real primary store + one named instance
/// store, each seeded with distinct, varied content (principle #14 — not one
/// cherry-picked id) — the "different KBs" axis `FederatedQuery` fans out
/// across, and the genuine two-store concurrency shape ADR-054's measured
/// test needs.
pub(crate) fn seeded_two_store_state() -> DaemonState {
    let primary = CozoKbStore::open_mem().expect("open primary store");
    primary
        .insert_node(&Node::new(
            "primary:alpha",
            "Alpha in the primary store",
            NodeKind::Note,
            "primary store body about alpha and rust ownership",
        ))
        .unwrap();
    primary
        .insert_node(&Node::new(
            "primary:beta",
            "Beta in the primary store",
            NodeKind::Note,
            "primary store body about beta and scheme continuations",
        ))
        .unwrap();
    primary
        .insert_node(&Node::new(
            "primary:gamma",
            "Gamma in the primary store",
            NodeKind::Note,
            "primary store body about gamma and concurrent daemons",
        ))
        .unwrap();

    let secondary = CozoKbStore::open_mem().expect("open secondary store");
    secondary
        .insert_node(&Node::new(
            "secondary:delta",
            "Delta in the secondary store",
            NodeKind::Note,
            "secondary store body about delta and iroh mesh transport",
        ))
        .unwrap();
    secondary
        .insert_node(&Node::new(
            "secondary:epsilon",
            "Epsilon in the secondary store",
            NodeKind::Note,
            "secondary store body about epsilon and cozodb relations",
        ))
        .unwrap();

    let mut st = DaemonState::new();
    st.store = Some(Arc::new(primary));
    st.instance_stores
        .insert("secondary".to_string(), Arc::new(secondary));
    st.rebuild_query_layer();
    st
}

/// A claimed, listening KB Unix socket backed by a real `accept_loop` task —
/// held alive for the test's duration (dropping this cancels the accept task
/// and removes the tempdir). `_shutdown_tx` MUST stay alive: a
/// `broadcast::Sender` with no live receivers/senders closes the channel,
/// which would make `accept_loop`'s `shutdown.recv()` resolve (as `Closed`)
/// immediately and exit the loop before it ever accepts a connection.
pub(crate) struct TestKbSocket {
    pub path: std::path::PathBuf,
    _tmp: tempfile::TempDir,
    _shutdown_tx: tokio::sync::broadcast::Sender<()>,
    _accept_task: tokio::task::JoinHandle<()>,
}

/// Bind a real `UnixListener` in a fresh tempdir and run the real
/// `accept_loop` (carrying the `ConnLimiter`/idle-timeout hardening from
/// ADR-054) against it as a background task.
pub(crate) async fn spawn_kb_socket(
    state: Arc<Mutex<DaemonState>>,
    max_connections: usize,
    idle_timeout: Duration,
) -> TestKbSocket {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("kb.sock");
    let listener = UnixListener::bind(&path).expect("bind kb socket");
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);
    let limiter = ConnLimiter::new(max_connections);
    let accept_task = tokio::spawn(crate::accept_loop(
        listener,
        state,
        shutdown_rx,
        limiter,
        idle_timeout,
    ));
    TestKbSocket {
        path,
        _tmp: tmp,
        _shutdown_tx: shutdown_tx,
        _accept_task: accept_task,
    }
}

/// Send one JSON-RPC request over `stream` (Content-Length framed, matching
/// `handle_client`) and return the parsed response.
pub(crate) async fn call(stream: &mut UnixStream, method: &str, params: Value) -> Value {
    let (r, mut w) = stream.split();
    let mut reader = tokio::io::BufReader::new(r);
    let req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let body = serde_json::to_vec(&req).unwrap();
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(5))
        .await
        .expect("write request");
    let msg = mae_mcp::read_message(&mut reader)
        .await
        .expect("read response")
        .expect("response before EOF");
    serde_json::from_str(&msg).expect("parse response")
}
