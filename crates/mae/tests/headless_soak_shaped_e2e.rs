//! Real subprocess e2e for L3c of the epic's issue-closure pass (#380,
//! ADR-055's soak-test DoD item) -- **honestly scoped down** from the
//! literal "multi-hour CI job, RSS/fd sampled over time" requirement to
//! something that actually fits per-PR CI: ~70s of real connect/disconnect
//! churn (simulating many short-lived VS Code sessions) with RSS/fd sampled
//! every few seconds, asserting the growth trend stays bounded.
//!
//! This is a REAL regression guard for the KIND of bug a soak test looks
//! for (unbounded per-connection growth -- a leaked cache entry, an
//! un-evicted subscriber, a socket/fd never closed), just not a substitute
//! for the full literal multi-hour job, which belongs in a separate
//! scheduled (`workflow_dispatch`/cron) CI job as its own tracked
//! fast-follow -- a true multi-hour run doesn't belong gating every PR.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use tokio::net::UnixStream;

fn isolated_env(cmd: &mut Command, xdg_config: &Path, xdg_data: &Path, home: &Path) {
    cmd.env("XDG_CONFIG_HOME", xdg_config)
        .env("XDG_DATA_HOME", xdg_data)
        .env("HOME", home)
        .env("SHELL", "/bin/sh")
        .env("MAE_SKIP_WIZARD", "1");
}

fn send_sigterm(child: &Child) {
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
}

fn wait_for_exit(child: &mut Child, timeout: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn socket_is_live(path: &Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

fn wait_for_socket_live(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if socket_is_live(path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn read_vm_rss_kb(pid: u32) -> u64 {
    let content = std::fs::read_to_string(format!("/proc/{pid}/status"))
        .unwrap_or_else(|e| panic!("failed to read /proc/{pid}/status: {e}"));
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
            return digits.parse().unwrap_or(0);
        }
    }
    panic!("VmRSS not found in /proc/{pid}/status");
}

fn read_fd_count(pid: u32) -> usize {
    std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap_or_else(|e| panic!("failed to read /proc/{pid}/fd: {e}"))
        .count()
}

async fn one_connect_disconnect_cycle(socket_path: &Path, call_id: u64) {
    let stream = UnixStream::connect(socket_path)
        .await
        .expect("connect to headless socket");
    let mut stream = tokio::io::BufReader::new(stream);

    let init_req = serde_json::json!({
        "jsonrpc": "2.0", "id": call_id, "method": "initialize",
        "params": {
            "clientInfo": {"name": "soak-shaped-e2e-test", "version": "1.0"},
            "protocolVersion": "2025-11-25"
        }
    });
    mae_mcp::write_framed(
        &mut stream,
        init_req.to_string().as_bytes(),
        Duration::from_secs(5),
    )
    .await
    .expect("write initialize");
    let _ = mae_mcp::read_message(&mut stream)
        .await
        .expect("read initialize response");

    let notif = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    mae_mcp::write_framed(
        &mut stream,
        notif.to_string().as_bytes(),
        Duration::from_secs(5),
    )
    .await
    .expect("write notifications/initialized");

    // A real Core-tier tool call -- read-only, cheap, exercises the same
    // dispatch path a real client's repeated small requests would.
    let call_req = serde_json::json!({
        "jsonrpc": "2.0", "id": call_id + 1, "method": "tools/call",
        "params": {"name": "buffer_read", "arguments": {}}
    });
    mae_mcp::write_framed(
        &mut stream,
        call_req.to_string().as_bytes(),
        Duration::from_secs(5),
    )
    .await
    .expect("write tools/call");
    let _ = mae_mcp::read_message(&mut stream)
        .await
        .expect("read tools/call response");

    // Connection drops here (stream out of scope) -- the churn part: many
    // short-lived sessions connecting, doing a little work, disconnecting.
}

#[tokio::test]
async fn sustained_connect_disconnect_churn_shows_bounded_rss_and_fd_growth() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(project_root.join(".git")).unwrap();
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();

    let mae = env!("CARGO_BIN_EXE_mae");

    let mut print_cmd = Command::new(mae);
    print_cmd
        .args(["--headless", "--print-socket-path"])
        .current_dir(&project_root);
    isolated_env(&mut print_cmd, &xdg_config, &xdg_data, tmp.path());
    let print_output = print_cmd.output().expect("print-socket-path failed");
    assert!(print_output.status.success());
    let socket_path = PathBuf::from(
        String::from_utf8_lossy(&print_output.stdout)
            .trim()
            .to_string(),
    );

    let mut spawn_cmd = Command::new(mae);
    spawn_cmd
        .args(["--headless"])
        .current_dir(&project_root)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    isolated_env(&mut spawn_cmd, &xdg_config, &xdg_data, tmp.path());
    let child = spawn_cmd.spawn().expect("failed to spawn mae --headless");
    let pid = child.id();

    assert!(
        wait_for_socket_live(&socket_path, Duration::from_secs(30)),
        "headless instance never bound its stable socket at {}",
        socket_path.display()
    );
    tokio::time::sleep(Duration::from_secs(1)).await;

    const TOTAL_DURATION: Duration = Duration::from_secs(70);
    const SAMPLE_INTERVAL: Duration = Duration::from_secs(10);

    let mut rss_samples: Vec<u64> = Vec::new();
    let mut fd_samples: Vec<usize> = Vec::new();
    let start = Instant::now();
    let mut call_id: u64 = 10;
    let mut next_sample_at = start;

    while start.elapsed() < TOTAL_DURATION {
        one_connect_disconnect_cycle(&socket_path, call_id).await;
        call_id += 10;

        if Instant::now() >= next_sample_at {
            rss_samples.push(read_vm_rss_kb(pid));
            fd_samples.push(read_fd_count(pid));
            next_sample_at += SAMPLE_INTERVAL;
        }
    }
    // Always take one final sample after the loop, regardless of interval
    // timing.
    rss_samples.push(read_vm_rss_kb(pid));
    fd_samples.push(read_fd_count(pid));

    let mut child_guard = child;
    send_sigterm(&child_guard);
    wait_for_exit(&mut child_guard, Duration::from_secs(10));

    assert!(
        rss_samples.len() >= 3,
        "expected multiple RSS samples over {TOTAL_DURATION:?}, got {}: {rss_samples:?}",
        rss_samples.len()
    );

    // Skip the very first sample (post-boot warmup: LSP/KB federation
    // background work can still be settling) -- baseline is the SECOND
    // sample, compared against the LAST.
    let baseline_rss = rss_samples[1];
    let final_rss = *rss_samples.last().unwrap();
    let baseline_fd = fd_samples[1];
    let final_fd = *fd_samples.last().unwrap();

    assert!(
        final_rss <= baseline_rss.saturating_mul(3).max(baseline_rss + 20_000),
        "RSS grew from {baseline_rss}KB to {final_rss}KB over {TOTAL_DURATION:?} of \
         connect/disconnect churn -- unbounded growth trend, full samples: {rss_samples:?}"
    );
    assert!(
        final_fd <= baseline_fd.saturating_mul(3).max(baseline_fd + 20),
        "open fd count grew from {baseline_fd} to {final_fd} over {TOTAL_DURATION:?} of \
         connect/disconnect churn -- likely a leaked socket/fd, full samples: {fd_samples:?}"
    );
}
