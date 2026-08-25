//! Shared harness for the real-TCP `mae-daemon` e2e suites.
//!
//! Each integration-test binary that spawns a real daemon (`network_e2e.rs`,
//! `hub_observability_e2e.rs`) pulls this in with `mod common;`. Rust compiles
//! it once per binary, so some helpers are unused in some of them — hence the
//! blanket `dead_code` allow rather than per-item attributes that would drift.
//!
//! Extracted when `network_e2e.rs` hit its size ceiling: it held two spawn
//! helpers differing only in whether they wrote a `daemon.toml` first, so the
//! split consolidated them into `spawn_server_with_config` rather than carrying
//! a third copy into the new file (principle #8).

#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use mae_mcp::protocol::JsonRpcResponse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Holds a spawned `mae-daemon` (+ its temp data dir) for a test's lifetime.
/// Dropping it kills the daemon (`kill_on_drop`) and removes the temp dir.
pub struct ServerGuard {
    _child: tokio::process::Child,
    _tmp: tempfile::TempDir,
    pub addr: SocketAddr,
    /// The daemon's KB Unix socket (`$XDG_RUNTIME_DIR/mae-daemon.sock`), where
    /// `daemon/status` lives. Collab is TCP; status is not — so observing "how
    /// many collab clients are attached" means asking on the other listener.
    pub socket: std::path::PathBuf,
}

/// Spawn a `mae-daemon` on a free TCP port for this test. Returns `None` (the
/// caller returns early, skipping) unless `MAE_TCP_E2E` is set.
pub async fn spawn_server() -> Option<ServerGuard> {
    spawn_server_with_config(None).await
}

/// As `spawn_server`, but first writes `daemon_toml` to the instance's
/// `daemon.toml` — e.g. a small `collab.max_connections` so a connection-cap
/// test is deterministic without opening hundreds of sockets (#342).
pub async fn spawn_server_with_config(daemon_toml: Option<&str>) -> Option<ServerGuard> {
    spawn_server_opts(daemon_toml, true).await
}

/// As `spawn_server_with_config`, for a config that DISABLES collab. Readiness
/// cannot wait on the TCP port in that case — there is no listener — so this
/// waits only for the KB socket.
pub async fn spawn_server_without_collab(daemon_toml: &str) -> Option<ServerGuard> {
    spawn_server_opts(Some(daemon_toml), false).await
}

/// `wait_for_collab`: whether readiness requires the collab listener to be up.
///
/// Readiness is established by asking `daemon/status` on the KB socket, NOT by
/// opening a TCP connection to the collab port. A probe connection is a real
/// client: it shows up in `connections.collab.active` and races any test that
/// asserts a baseline of zero — which is exactly what happened when the
/// hub-observability test asserted `active == 0` immediately after spawn and
/// passed once, then failed on the next run.
async fn spawn_server_opts(
    daemon_toml: Option<&str>,
    wait_for_collab: bool,
) -> Option<ServerGuard> {
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
    // runs with default (no-auth) config unless `daemon_toml` says otherwise.
    if let Some(toml) = daemon_toml {
        let config_dir = tmp.path().join("config").join("mae");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("daemon.toml"), toml).unwrap();
    }
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
    let socket = tmp.path().join("mae-daemon.sock");
    // Bounded by wall-clock, not by an iteration count — see `mae_mcp::ready`.
    // This helper is shared by most of the daemon e2e suite, so the old fixed
    // 5s budget set the flake floor for all of them at once.
    let ready = mae_mcp::ready::wait_until(|| async {
        match try_daemon_status(&socket).await {
            Some(status) => status["connections"]["collab"].is_object() == wait_for_collab,
            None => false,
        }
    })
    .await;
    assert!(
        ready,
        "{}",
        mae_mcp::ready::timeout_message(&format!(
            "mae-daemon (addr {addr}, socket {}, expecting collab {})",
            socket.display(),
            if wait_for_collab { "up" } else { "disabled" }
        ))
    );
    Some(ServerGuard {
        _child: child,
        _tmp: tmp,
        addr,
        socket,
    })
}

/// Read a Content-Length framed message from a TCP stream.
pub async fn read_framed(
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
pub async fn send_recv(
    stream: &mut tokio::net::TcpStream,
    msg: &serde_json::Value,
) -> JsonRpcResponse {
    let payload = format!("{}\n", serde_json::to_string(msg).unwrap());
    stream.write_all(payload.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let value = read_framed(stream, 5000).await.expect("expected response");
    serde_json::from_value(value).unwrap()
}

/// Ask the daemon's KB Unix socket for `daemon/status`. The hub's collab clients
/// arrive over TCP; the count is read here, on the other listener — which is the
/// operator's real vantage point (`mae-daemon` on the server, editors elsewhere).
pub async fn daemon_status(socket: &Path) -> serde_json::Value {
    use tokio::io::BufReader;
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .unwrap_or_else(|e| panic!("connect {}: {e}", socket.display()));
    let (r, mut w) = stream.split();
    let mut reader = BufReader::new(r);
    let body = serde_json::to_vec(
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "daemon/status", "params": {}}),
    )
    .unwrap();
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(5))
        .await
        .expect("write daemon/status");
    let msg = mae_mcp::read_message(&mut reader)
        .await
        .expect("read daemon/status")
        .expect("response before EOF");
    let v: serde_json::Value = serde_json::from_str(&msg).expect("parse daemon/status");
    assert!(v.get("error").is_none(), "daemon/status failed: {v:?}");
    v["result"].clone()
}

/// Poll `daemon/status` until the collab connection count reaches `want`,
/// returning the last value seen. Connection teardown is observed by the server
/// asynchronously, so every count assertion that follows a disconnect has to
/// wait rather than sample once.
pub async fn await_collab_active(socket: &Path, want: u64) -> Option<u64> {
    // Bounded by wall-clock, not by an iteration count — see `mae_mcp::ready`.
    // Deliberately still returns the last value SEEN rather than panicking:
    // callers assert on it themselves, so a mismatch is reported with their own
    // context. Only the budget changes here, not what is waited for.
    let hit = mae_mcp::ready::wait_for_some(|| async {
        daemon_status(socket).await["connections"]["collab"]["active"]
            .as_u64()
            .filter(|v| *v == want)
    })
    .await;
    match hit {
        Some(v) => Some(v),
        // Timed out — sample once more so the caller's assertion names the
        // count actually observed, exactly as the old loop's `last` did.
        None => daemon_status(socket).await["connections"]["collab"]["active"].as_u64(),
    }
}

/// `daemon_status`, but returning `None` instead of panicking while the daemon
/// is still coming up. Used only by the readiness loop.
async fn try_daemon_status(socket: &Path) -> Option<serde_json::Value> {
    use tokio::io::BufReader;
    let mut stream = tokio::net::UnixStream::connect(socket).await.ok()?;
    let (r, mut w) = stream.split();
    let mut reader = BufReader::new(r);
    let body = serde_json::to_vec(
        &serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "daemon/status", "params": {}}),
    )
    .ok()?;
    mae_mcp::write_framed(&mut w, &body, Duration::from_secs(2))
        .await
        .ok()?;
    let msg = mae_mcp::read_message(&mut reader).await.ok()??;
    let v: serde_json::Value = serde_json::from_str(&msg).ok()?;
    v.get("result").cloned()
}
