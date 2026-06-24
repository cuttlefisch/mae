//! Network E2E tests for mae-daemon over real TCP.
//!
//! Each test spawns its own `mae-daemon` on a free port and connects real TCP
//! clients — self-contained, no external server. Gated on `MAE_TCP_E2E` (the same
//! run-gate as `crates/mae/tests/collab_tcp_e2e.rs`).
//!
//! Run: `MAE_TCP_E2E=1 cargo test -p mae-daemon --test network_e2e`
//!
//! (Was gated on `MAE_STATE_SERVER=host:port`, which pointed at the retired
//! state-server and required an externally-running daemon — so it never ran in
//! CI. These tests are the only e2e coverage of `sync/resync`.)

use std::net::SocketAddr;
use std::time::Duration;

use mae_mcp::protocol::JsonRpcResponse;
use mae_sync::encoding::update_to_base64;
use mae_sync::text::TextSync;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Holds a spawned `mae-daemon` (+ its temp data dir) for a test's lifetime.
/// Dropping it kills the daemon (`kill_on_drop`) and removes the temp dir.
struct ServerGuard {
    _child: tokio::process::Child,
    _tmp: tempfile::TempDir,
    addr: SocketAddr,
}

/// Spawn a `mae-daemon` on a free TCP port for this test. Returns `None` (the
/// caller returns early, skipping) unless `MAE_TCP_E2E` is set.
async fn spawn_server() -> Option<ServerGuard> {
    if std::env::var("MAE_TCP_E2E").is_err() {
        eprintln!("skipping: MAE_TCP_E2E not set");
        return None;
    }
    // Reserve a free port, then hand it to the daemon.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    drop(listener);
    let tmp = tempfile::tempdir().unwrap();
    // Isolate this daemon fully so tests run in parallel and alongside any other
    // daemon (incl. a developer's live one): a per-test XDG_RUNTIME_DIR gives it a
    // unique Unix socket (the daemon also binds `$XDG_RUNTIME_DIR/mae-daemon.sock`,
    // not just TCP), and a per-test XDG_CONFIG_HOME means it finds no daemon.toml →
    // runs with default (no-auth) config.
    let child = tokio::process::Command::new(env!("CARGO_BIN_EXE_mae-daemon"))
        .args([
            "--bind",
            &addr.to_string(),
            "--data-dir",
            tmp.path().to_str().unwrap(),
        ])
        .env("XDG_RUNTIME_DIR", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .expect("failed to spawn mae-daemon");
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return Some(ServerGuard {
                _child: child,
                _tmp: tmp,
                addr,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("mae-daemon did not start within 5s on {addr}");
}

/// Compute SHA-256 of content (matching the server's content hash).
fn sha256(content: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    hex::encode(hasher.finalize())
}

/// Read a Content-Length framed message from a TCP stream.
async fn read_framed(
    stream: &mut tokio::net::TcpStream,
    timeout_ms: u64,
) -> Option<serde_json::Value> {
    let result = tokio::time::timeout(std::time::Duration::from_millis(timeout_ms), async {
        let mut header_buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream.read_exact(&mut byte).await.ok()?;
            header_buf.push(byte[0]);
            if header_buf.len() >= 4 && &header_buf[header_buf.len() - 4..] == b"\r\n\r\n" {
                break;
            }
        }
        let header = String::from_utf8(header_buf).ok()?;
        let content_length: usize = header
            .lines()
            .find_map(|line| line.strip_prefix("Content-Length: "))
            .and_then(|v| v.trim().parse().ok())?;
        let mut body = vec![0u8; content_length];
        stream.read_exact(&mut body).await.ok()?;
        serde_json::from_slice(&body).ok()
    })
    .await;
    result.unwrap_or_default()
}

/// Send a JSON-RPC message and read the response.
async fn send_recv(stream: &mut tokio::net::TcpStream, msg: &serde_json::Value) -> JsonRpcResponse {
    let payload = format!("{}\n", serde_json::to_string(msg).unwrap());
    stream.write_all(payload.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let value = read_framed(stream, 5000).await.expect("expected response");
    serde_json::from_value(value).unwrap()
}

// Each test spawns its own mae-daemon via spawn_server() (gated on MAE_TCP_E2E).

#[tokio::test]
async fn tcp_initialize_and_ping() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();

    // Initialize.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "e2e-test"}}
        }),
    )
    .await;
    assert!(resp.error.is_none());

    // Ping.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"}),
    )
    .await;
    assert_eq!(resp.result.unwrap(), "pong");
}

#[tokio::test]
async fn tcp_sync_update_roundtrip() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();

    // Initialize.
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "sync-test"}}
        }),
    )
    .await;

    // Generate a yrs update.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "hello from e2e");
    let update_b64 = update_to_base64(&update);

    // Send sync/update.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/update",
            "params": { "doc": "e2e-test-doc", "update": update_b64 }
        }),
    )
    .await;
    assert!(resp.error.is_none());
    assert!(resp.result.unwrap()["wal_seq"].as_u64().unwrap() > 0);

    // Read back via docs/content.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "docs/content",
            "params": { "doc": "e2e-test-doc" }
        }),
    )
    .await;
    assert_eq!(resp.result.unwrap()["content"], "hello from e2e");
}

#[tokio::test]
async fn tcp_two_clients_converge() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;

    let doc_name = format!("converge-{}", std::process::id());

    // Client A.
    let mut client_a = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client_a,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "client-a"}}
        }),
    )
    .await;

    // Client B.
    let mut client_b = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client_b,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "client-b"}}
        }),
    )
    .await;

    // Client A sends an update.
    let mut ts_a = TextSync::with_client_id("", 1);
    let update_a = ts_a.insert(0, "hello");
    let update_a_b64 = update_to_base64(&update_a);

    send_recv(
        &mut client_a,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/update",
            "params": { "doc": doc_name, "update": update_a_b64, "client_id": 1 }
        }),
    )
    .await;

    // Client B gets the full state.
    let resp = send_recv(
        &mut client_b,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/full_state",
            "params": { "doc": doc_name }
        }),
    )
    .await;
    let state_b64 = resp.result.unwrap()["state"].as_str().unwrap().to_string();
    assert!(!state_b64.is_empty());

    // Client B applies and verifies.
    let state_bytes = mae_sync::encoding::base64_to_update(&state_b64).unwrap();
    let ts_b = TextSync::from_state(&state_bytes).unwrap();
    assert_eq!(ts_b.content(), "hello");
}

#[tokio::test]
async fn tcp_state_vector_diff_protocol() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;

    let doc_name = format!("diff-{}", std::process::id());

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "diff-test"}}
        }),
    )
    .await;

    // Send an update.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "diff test");
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/update",
            "params": { "doc": doc_name, "update": update_to_base64(&update) }
        }),
    )
    .await;

    // Get state vector of an empty client.
    let empty_sv = TextSync::new("").state_vector();
    let sv_b64 = update_to_base64(&empty_sv);

    // Request diff.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "sync/diff",
            "params": { "doc": doc_name, "sv": sv_b64 }
        }),
    )
    .await;
    let result = resp.result.unwrap();
    assert!(result["update"].as_str().is_some());
    assert!(result["server_sv"].as_str().is_some());
}

#[tokio::test]
async fn tcp_docs_list() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "list-test"}}
        }),
    )
    .await;

    let resp = send_recv(
        &mut client,
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "docs/list"}),
    )
    .await;
    let result = resp.result.unwrap();
    assert!(result["documents"].is_array());
}

#[tokio::test]
async fn tcp_docs_stats() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;
    let doc_name = format!("stats-{}", std::process::id());

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "stats-test"}}
        }),
    )
    .await;

    // Create the document with an update.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "stats document content");
    let update_b64 = update_to_base64(&update);

    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/update",
            "params": { "doc": doc_name, "update": update_b64 }
        }),
    )
    .await;

    // Request stats for that document.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "docs/stats",
            "params": { "doc": doc_name }
        }),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "docs/stats returned error: {:?}",
        resp.error
    );
    // docs/stats nests the metrics under a `stats` object.
    let stats = &resp.result.unwrap()["stats"];
    assert!(
        stats["wal_seq"].as_u64().is_some(),
        "expected stats.wal_seq field, got: {stats}"
    );
    assert!(
        stats["content_length"].as_u64().is_some(),
        "expected stats.content_length field, got: {stats}"
    );
}

#[tokio::test]
async fn tcp_save_intent_ok() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "save-intent-test"}}
        }),
    )
    .await;

    // Create the document.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "save intent test content");
    let update_b64 = update_to_base64(&update);

    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/update",
            "params": { "doc": "save-test-doc", "update": update_b64 }
        }),
    )
    .await;

    // Read back content so we can compute a hash.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "docs/content",
            "params": { "doc": "save-test-doc" }
        }),
    )
    .await;
    let content = resp.result.unwrap()["content"]
        .as_str()
        .unwrap()
        .to_string();

    // docs/save_intent requires `expected_hash` = the server's SHA-256 of the
    // current content (ADR-003 content-hash verification). Sending the matching
    // hash must report it's safe to save.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 4, "method": "docs/save_intent",
            "params": { "doc": "save-test-doc", "expected_hash": sha256(&content) }
        }),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "docs/save_intent returned error: {:?}",
        resp.error
    );
    let result = resp.result.unwrap();
    assert_eq!(
        result["result"]["status"], "ok",
        "matching expected_hash should be safe to save (status=ok), got: {result}"
    );
}

#[tokio::test]
async fn tcp_resync_protocol() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;
    let doc_name = format!("resync-{}", std::process::id());

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "resync-test"}}
        }),
    )
    .await;

    // Send an update to create the document.
    let mut ts = TextSync::with_client_id("", 1);
    let update = ts.insert(0, "resync content");
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/update",
            "params": { "doc": doc_name, "update": update_to_base64(&update) }
        }),
    )
    .await;

    // Request a full resync.
    let resp = send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "sync/resync",
            "params": { "doc": doc_name }
        }),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "sync/resync returned error: {:?}",
        resp.error
    );
    let result = resp.result.unwrap();
    assert!(
        result["state"].as_str().is_some(),
        "expected base64 state field, got: {result}"
    );
    assert!(
        result["sv"].as_str().is_some(),
        "expected base64 sv field, got: {result}"
    );
}

#[tokio::test]
async fn tcp_debug_endpoint() {
    let Some(server) = spawn_server().await else {
        return;
    };
    let addr = server.addr;

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "debug-endpoint-test"}}
        }),
    )
    .await;

    let resp = send_recv(
        &mut client,
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/debug"}),
    )
    .await;
    assert!(
        resp.error.is_none(),
        "$/debug returned error: {:?}",
        resp.error
    );
    let result = resp.result.unwrap();
    // `documents` is now a count; older builds returned an array/object.
    assert!(
        result["documents"].is_u64()
            || result["documents"].is_array()
            || result["documents"].is_object(),
        "expected documents field, got: {result}"
    );
    assert!(
        result["doc_stats"].is_object() || result["doc_stats"].is_array(),
        "expected doc_stats field, got: {result}"
    );
    assert!(
        result["version"].as_str().is_some(),
        "expected version field, got: {result}"
    );
}
