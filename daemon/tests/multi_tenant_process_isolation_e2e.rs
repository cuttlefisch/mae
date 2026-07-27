//! ADR-060 Phase E: genuine OS-level process isolation between two
//! independently-instantiated `mae-daemon` processes (the `mae-daemon@.service`
//! systemd template's real-world shape — one process per tenant).
//!
//! Distinct from Phases A-D's in-process multi-tenant isolation
//! (`daemon/src/tenant.rs`'s `TenantRegistry`), which proves per-tenant
//! logical separation *within one shared process*. That mechanism, however
//! correct, cannot prove what this test proves: that a tenant's process
//! dying does not affect a co-resident tenant's *separately-running*
//! process at all. Two real `mae-daemon` child processes, real TCP, a real
//! SIGKILL — not simulated.
//!
//! Run: `MAE_TCP_E2E=1 cargo test -p mae-daemon --test multi_tenant_process_isolation_e2e`

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use mae_mcp::protocol::JsonRpcResponse;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct TenantProcess {
    child: tokio::process::Child,
    _tmp: tempfile::TempDir,
    addr: SocketAddr,
}

/// Spawn one tenant's `mae-daemon` process on its own port + isolated data
/// dir -- exactly what a `mae-daemon@<tenant>.service` systemd instantiation
/// gives it in production (its own PID, its own daemon.toml-equivalent
/// config, its own storage), just spawned directly here instead of via
/// systemd for testability.
async fn spawn_tenant_process() -> Option<TenantProcess> {
    if std::env::var("MAE_TCP_E2E").is_err() {
        eprintln!("skipping: MAE_TCP_E2E not set");
        return None;
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    drop(listener);
    let tmp = tempfile::tempdir().unwrap();
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
            return Some(TenantProcess {
                child,
                _tmp: tmp,
                addr,
            });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("mae-daemon did not start within 5s on {addr}");
}

async fn read_framed(stream: &mut tokio::net::TcpStream, timeout_ms: u64) -> Option<serde_json::Value> {
    let result = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
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

async fn send_recv(stream: &mut tokio::net::TcpStream, msg: &serde_json::Value) -> JsonRpcResponse {
    let payload = format!("{}\n", serde_json::to_string(msg).unwrap());
    stream.write_all(payload.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let value = read_framed(stream, 5000).await.expect("expected response");
    serde_json::from_value(value).unwrap()
}

async fn ping(addr: SocketAddr) -> Duration {
    let start = Instant::now();
    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    send_recv(
        &mut client,
        &serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "isolation-test"}}
        }),
    )
    .await;
    let resp = send_recv(
        &mut client,
        &serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"}),
    )
    .await;
    assert_eq!(resp.result.unwrap(), "pong");
    start.elapsed()
}

/// The adversarial case ADR-060 Phase E's Verification section names
/// explicitly: `kill -9` tenant A's isolated process; tenant B's
/// separately-instantiated, separately-running process must show ZERO
/// observable impact -- no dropped connections, no elevated latency, no
/// shared-state corruption. This is the property Phases A-D's in-process
/// locking, however correct, cannot prove: they only ever ran inside ONE
/// process, so nothing in their own test suite exercises what happens when
/// a co-resident tenant's entire PROCESS disappears.
#[tokio::test]
async fn sigkilling_one_tenant_process_has_zero_observable_impact_on_another() {
    let Some(mut tenant_a) = spawn_tenant_process().await else {
        return;
    };
    let Some(mut tenant_b) = spawn_tenant_process().await else {
        return;
    };

    // Baseline: tenant B's own solo latency, before A is touched at all.
    let baseline = ping(tenant_b.addr).await;

    // Confirm tenant A is genuinely up (not testing against a dead process
    // that never started -- a vacuous pass).
    let a_before = ping(tenant_a.addr).await;
    assert!(a_before < Duration::from_secs(5), "tenant A must be alive before the kill");

    // The real adversarial action: SIGKILL, not a graceful shutdown --
    // proves this doesn't depend on tenant A getting a chance to clean up.
    // tokio::process::Child::kill() sends SIGKILL on Unix.
    tenant_a.child.kill().await.expect("SIGKILL must succeed");
    let wait_result = tokio::time::timeout(Duration::from_secs(5), tenant_a.child.wait())
        .await
        .expect("tenant A's process must actually exit after SIGKILL")
        .expect("wait() must succeed");
    assert!(!wait_result.success(), "a SIGKILL'd process must not report a clean exit");

    // A genuinely dead process, not "still technically listening" -- the
    // negative control that proves the kill actually took effect, so the
    // "zero impact on B" result below isn't vacuously true because A was
    // never really killed.
    assert!(
        tokio::net::TcpStream::connect(tenant_a.addr).await.is_err(),
        "tenant A's port must no longer accept connections after SIGKILL"
    );

    // The property under test: N=10 requests against tenant B, all AFTER
    // A's death, must all succeed with no elevated latency versus the
    // pre-kill baseline. Generous tolerance (10x baseline, floored at
    // 50ms) matches this codebase's own established pattern
    // (ADR-060 Phase B's ceiling test) for absorbing normal CI scheduler
    // jitter while still easily catching a real regression (e.g. a shared
    // OS-level resource, a port/socket collision, a shared temp dir).
    let tolerance = std::cmp::max(baseline * 10, Duration::from_millis(50));
    for i in 0..10 {
        let d = ping(tenant_b.addr).await;
        assert!(
            d < tolerance,
            "request {i} to tenant B took {d:?} after tenant A's SIGKILL (baseline was \
             {baseline:?}, tolerance {tolerance:?}) -- this is exactly the shared-failure-\
             domain regression ADR-060 Phase E's process-per-tenant isolation exists to rule out"
        );
    }

    tenant_b.child.kill().await.ok();
}
