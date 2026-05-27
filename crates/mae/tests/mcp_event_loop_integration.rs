//! Integration tests that exercise behavior through the REAL event loop.
//!
//! These tests spawn a full `mae` process with a PTY (so it enters
//! `run_terminal_loop`) and drive it via the MCP socket. This validates
//! that event-loop-dependent behavior (hooks, async yields, mode transitions)
//! works end-to-end — not through synthetic flushes or manual drains.
//!
//! No sleeps between operations: hooks drain synchronously in the same
//! event loop iteration as the MCP request, so by the time the client
//! receives the reply, all side effects have been processed.
//!
//! Requires: `mae` binary built (cargo build -p mae).
//! Marked `#[ignore]` by default — run with:
//!   MAE_EVENT_LOOP_E2E=1 cargo test -p mae --test mcp_event_loop_integration -- --ignored --nocapture

use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

/// Helper: find the mae binary (debug build).
fn mae_binary() -> PathBuf {
    let mut path = std::env::current_exe()
        .expect("current_exe")
        .parent()
        .expect("parent")
        .parent()
        .expect("parent")
        .to_path_buf();
    path.push("mae");
    if !path.exists() {
        panic!(
            "mae binary not found at {}. Run `cargo build -p mae` first.",
            path.display()
        );
    }
    path
}

/// Spawn mae with a PTY so it enters the real terminal event loop.
/// Returns (child, mcp_socket_path, master_fd_file).
fn spawn_mae_with_pty() -> (std::process::Child, String, std::fs::File) {
    let mae = mae_binary();

    // Create a PTY pair so mae thinks it has a real terminal.
    let mut master_fd: libc::c_int = 0;
    let mut slave_fd: libc::c_int = 0;
    let ret = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    assert_eq!(ret, 0, "openpty failed");

    // Spawn mae with the slave end as stdin/stdout/stderr.
    let child = unsafe {
        Command::new(&mae)
            .env("TERM", "xterm-256color")
            .args(["-q"]) // Clean mode: skip init.scm, modules, KB for fast startup
            .env("MAE_LOG", "mae=info")
            .env("MAE_SKIP_WIZARD", "1")
            // Prevent loading user init.scm that might interfere.
            .env("XDG_CONFIG_HOME", "/tmp/mae-test-config-nonexistent")
            .env("SHELL", "/bin/sh")
            .pre_exec(move || {
                // Create a new session and set controlling terminal.
                libc::setsid();
                libc::ioctl(slave_fd, libc::TIOCSCTTY, 0);
                libc::dup2(slave_fd, 0); // stdin
                libc::dup2(slave_fd, 1); // stdout
                libc::dup2(slave_fd, 2); // stderr
                if slave_fd > 2 {
                    libc::close(slave_fd);
                }
                Ok(())
            })
            .spawn()
            .expect("failed to spawn mae")
    };

    // Close the slave fd in the parent — mae owns it now.
    unsafe { libc::close(slave_fd) };

    // Keep master_fd alive so the PTY doesn't close.
    let master_file = unsafe { std::fs::File::from_raw_fd(master_fd) };

    let pid = child.id();
    let socket_path = format!("/tmp/mae-{}.sock", pid);

    (child, socket_path, master_file)
}

/// Wait for the MCP socket to appear.
fn wait_for_socket(path: &str, timeout: Duration) -> bool {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if std::path::Path::new(path).exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Send a JSON-RPC message and read the response via Content-Length framing.
async fn mcp_call(stream: &mut UnixStream, msg: &serde_json::Value) -> serde_json::Value {
    let payload = serde_json::to_string(msg).unwrap();
    stream
        .write_all(format!("{}\n", payload).as_bytes())
        .await
        .unwrap();
    stream.flush().await.unwrap();

    // Read Content-Length framed response.
    let mut buf = vec![0u8; 8192];
    let mut accumulated = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);

    loop {
        if tokio::time::Instant::now() > deadline {
            panic!(
                "Timeout waiting for MCP response. Accumulated: {}",
                String::from_utf8_lossy(&accumulated)
            );
        }

        let n = tokio::time::timeout(Duration::from_secs(10), stream.readable())
            .await
            .expect("readable timeout");
        n.expect("readable error");

        match stream.try_read(&mut buf) {
            Ok(0) => panic!("MCP socket closed"),
            Ok(n) => {
                accumulated.extend_from_slice(&buf[..n]);
                // Try to parse Content-Length header + body.
                if let Some(value) = try_parse_content_length_response(&accumulated) {
                    return value;
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Err(e) => panic!("MCP read error: {}", e),
        }
    }
}

/// Try to parse a Content-Length framed JSON-RPC response from accumulated bytes.
fn try_parse_content_length_response(data: &[u8]) -> Option<serde_json::Value> {
    let text = std::str::from_utf8(data).ok()?;
    let header_end = text.find("\r\n\r\n")?;
    let header = &text[..header_end];

    let content_length: usize = header.lines().find_map(|l| {
        let l = l.trim();
        if l.to_lowercase().starts_with("content-length:") {
            l.split(':').nth(1)?.trim().parse().ok()
        } else {
            None
        }
    })?;

    let body_start = header_end + 4;
    if data.len() >= body_start + content_length {
        let body = &text[body_start..body_start + content_length];
        serde_json::from_str(body).ok()
    } else {
        None
    }
}

/// Initialize MCP session.
async fn mcp_initialize(stream: &mut UnixStream) {
    let resp = mcp_call(
        stream,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": {"name": "integration-test", "version": "1.0"},
                "protocolVersion": "2025-11-25"
            }
        }),
    )
    .await;
    assert!(
        resp.get("result").is_some(),
        "initialize failed: {:?}",
        resp
    );

    // Send initialized notification.
    let payload = serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }))
    .unwrap();
    stream
        .write_all(format!("{}\n", payload).as_bytes())
        .await
        .unwrap();
    stream.flush().await.unwrap();
}

/// Call an MCP tool and return the result.
async fn mcp_tool_call(
    stream: &mut UnixStream,
    id: u64,
    tool: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    mcp_call(
        stream,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": {
                "name": tool,
                "arguments": args
            }
        }),
    )
    .await
}

/// Extract text content from an MCP tool result.
fn extract_tool_text(resp: &serde_json::Value) -> String {
    resp["result"]["content"]
        .as_array()
        .and_then(|arr| arr.first())
        .and_then(|item| item["text"].as_str())
        .unwrap_or("")
        .to_string()
}

/// RAII guard to kill child process and clean up socket on drop.
struct MaeProcess {
    child: std::process::Child,
    socket_path: String,
    _master_fd: std::fs::File,
}

impl Drop for MaeProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Boot mae and connect via MCP. Returns (process_guard, connected_stream).
async fn boot_mae() -> (MaeProcess, UnixStream) {
    let (child, socket_path, master_fd) = spawn_mae_with_pty();
    let mut proc = MaeProcess {
        child,
        socket_path: socket_path.clone(),
        _master_fd: master_fd,
    };

    if !wait_for_socket(&socket_path, Duration::from_secs(30)) {
        let _ = proc.child.kill();
        panic!("MCP socket {} did not appear within 30s", socket_path);
    }

    // Brief delay for server to accept connections after socket creation.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut stream = UnixStream::connect(&socket_path)
        .await
        .expect("connect to MCP socket");

    mcp_initialize(&mut stream).await;

    (proc, stream)
}

/// Create a text buffer and switch to it (dashboard blocks mode changes).
async fn ensure_text_buffer(stream: &mut UnixStream, name: &str) {
    let code = format!(r#"(create-buffer "{}") "ok""#, name);
    let resp = mcp_tool_call(stream, 2, "eval_scheme", serde_json::json!({"code": code})).await;
    assert!(
        extract_tool_text(&resp).contains("ok"),
        "create-buffer failed: {}",
        extract_tool_text(&resp)
    );
    let resp = mcp_tool_call(
        stream,
        3,
        "execute_command",
        serde_json::json!({"command": "next-buffer"}),
    )
    .await;
    assert!(
        extract_tool_text(&resp).contains("Executed"),
        "next-buffer failed: {}",
        extract_tool_text(&resp)
    );
}

// ---------------------------------------------------------------------------
// Tests — all #[ignore] by default, run with MAE_EVENT_LOOP_E2E=1
// ---------------------------------------------------------------------------

fn should_run() -> bool {
    std::env::var("MAE_EVENT_LOOP_E2E").is_ok()
}

/// Test that hooks fire through the real event loop when triggered via MCP.
///
/// No sleeps: hooks drain synchronously in the same event loop iteration
/// as the MCP request. By the time the client gets the reply, hooks have
/// already been processed.
#[tokio::test]
#[ignore]
async fn hooks_fire_through_event_loop() {
    if !should_run() {
        return;
    }

    let (_proc, mut stream) = boot_mae().await;
    ensure_text_buffer(&mut stream, "*test-hooks*").await;

    // Register a hook that tracks whether it fired and what mode it saw.
    let setup_code = r#"
        (define *hook-test-fired* #f)
        (define *hook-test-mode* "")
        (define (test-mode-hook)
          (set! *hook-test-fired* #t)
          (set! *hook-test-mode* *mode*))
        (add-hook! "mode-change" "test-mode-hook")
        "setup-done"
    "#;
    let resp = mcp_tool_call(
        &mut stream,
        10,
        "eval_scheme",
        serde_json::json!({"code": setup_code}),
    )
    .await;
    let text = extract_tool_text(&resp);
    assert!(text.contains("setup-done"), "setup eval failed: {}", text);

    // Trigger mode change — hooks drain in the same event loop iteration.
    let resp = mcp_tool_call(
        &mut stream,
        11,
        "execute_command",
        serde_json::json!({"command": "enter-insert-mode"}),
    )
    .await;
    assert!(
        extract_tool_text(&resp).contains("Executed"),
        "enter-insert-mode failed: {}",
        extract_tool_text(&resp)
    );

    // Check that the hook fired — no sleep needed.
    let resp = mcp_tool_call(
        &mut stream,
        12,
        "eval_scheme",
        serde_json::json!({"code": "*hook-test-fired*"}),
    )
    .await;
    let text = extract_tool_text(&resp);
    assert!(
        text.contains("#t"),
        "Hook did not fire! *hook-test-fired* = {}",
        text
    );

    // Check mode value captured by hook.
    let resp = mcp_tool_call(
        &mut stream,
        13,
        "eval_scheme",
        serde_json::json!({"code": "*hook-test-mode*"}),
    )
    .await;
    let text = extract_tool_text(&resp);
    assert!(
        text.contains("insert"),
        "Hook captured wrong mode: {}",
        text
    );

    // Return to normal mode — hook fires again.
    let resp = mcp_tool_call(
        &mut stream,
        14,
        "execute_command",
        serde_json::json!({"command": "enter-normal-mode"}),
    )
    .await;
    assert!(extract_tool_text(&resp).contains("Executed"));

    let resp = mcp_tool_call(
        &mut stream,
        15,
        "eval_scheme",
        serde_json::json!({"code": "*hook-test-mode*"}),
    )
    .await;
    let text = extract_tool_text(&resp);
    assert!(
        text.contains("normal"),
        "Hook did not capture normal mode: {}",
        text
    );

    // Remove hook and verify it stops firing.
    let resp = mcp_tool_call(
        &mut stream,
        16,
        "eval_scheme",
        serde_json::json!({"code": r#"
            (set! *hook-test-fired* #f)
            (remove-hook! "mode-change" "test-mode-hook")
            "removed"
        "#}),
    )
    .await;
    assert!(extract_tool_text(&resp).contains("removed"));

    let resp = mcp_tool_call(
        &mut stream,
        17,
        "execute_command",
        serde_json::json!({"command": "enter-insert-mode"}),
    )
    .await;
    assert!(extract_tool_text(&resp).contains("Executed"));

    let resp = mcp_tool_call(
        &mut stream,
        18,
        "eval_scheme",
        serde_json::json!({"code": "*hook-test-fired*"}),
    )
    .await;
    let text = extract_tool_text(&resp);
    assert!(
        text.contains("#f"),
        "Hook fired after removal! *hook-test-fired* = {}",
        text
    );
}

/// Test mode transitions track correctly through hooks.
#[tokio::test]
#[ignore]
async fn mode_transition_fires_hooks_via_mcp() {
    if !should_run() {
        return;
    }

    let (_proc, mut stream) = boot_mae().await;
    ensure_text_buffer(&mut stream, "*test-modes*").await;

    // Track all mode transitions.
    let setup = r#"
        (define *mode-history* '())
        (define (track-mode-hook)
          (set! *mode-history* (cons *mode* *mode-history*)))
        (add-hook! "mode-change" "track-mode-hook")
        "ok"
    "#;
    let resp = mcp_tool_call(
        &mut stream,
        10,
        "eval_scheme",
        serde_json::json!({"code": setup}),
    )
    .await;
    assert!(extract_tool_text(&resp).contains("ok"));

    // Cycle through modes: normal → insert → normal → visual-char → normal
    for (id, cmd) in [
        (11u64, "enter-insert-mode"),
        (12, "enter-normal-mode"),
        (13, "enter-visual-char"),
        (14, "enter-normal-mode"),
    ] {
        let resp = mcp_tool_call(
            &mut stream,
            id,
            "execute_command",
            serde_json::json!({"command": cmd}),
        )
        .await;
        assert!(
            extract_tool_text(&resp).contains("Executed"),
            "{} failed: {}",
            cmd,
            extract_tool_text(&resp)
        );
    }

    // Read mode history length.
    let resp = mcp_tool_call(
        &mut stream,
        20,
        "eval_scheme",
        serde_json::json!({"code": "(length *mode-history*)"}),
    )
    .await;
    let text = extract_tool_text(&resp);
    // Extract number from output like "> (length *mode-history*)\n; => 4\n"
    let count: usize = text
        .lines()
        .find_map(|l| l.strip_prefix("; => ").and_then(|s| s.trim().parse().ok()))
        .unwrap_or(0);
    assert!(
        count >= 4,
        "Expected at least 4 mode transitions, got {} (full output: {})",
        count,
        text
    );
}

/// Test that eval_scheme handles yield-tick inline (drains hooks during eval).
#[tokio::test]
#[ignore]
async fn eval_scheme_handles_yield_tick() {
    if !should_run() {
        return;
    }

    let (_proc, mut stream) = boot_mae().await;
    ensure_text_buffer(&mut stream, "*test-yield*").await;

    // In a single eval_scheme call: set up hook, trigger mode change,
    // yield-tick to drain hooks, then check result.
    let code = r#"
        (define *yt-fired* #f)
        (define (yt-hook) (set! *yt-fired* #t))
        (add-hook! "mode-change" "yt-hook")
        (flush!)
        (run-command "enter-insert-mode")
        (yield-tick)
        *yt-fired*
    "#;
    let resp = mcp_tool_call(
        &mut stream,
        10,
        "eval_scheme",
        serde_json::json!({"code": code}),
    )
    .await;
    let text = extract_tool_text(&resp);
    assert!(
        text.contains("#t"),
        "yield-tick did not drain hooks: {}",
        text
    );
}

/// Test that set_mode is blocked on dashboard and reports it.
#[tokio::test]
#[ignore]
async fn set_mode_blocked_on_dashboard() {
    if !should_run() {
        return;
    }

    let (_proc, mut stream) = boot_mae().await;
    // Don't create a text buffer — stay on dashboard.

    // Try to enter insert mode — should fail silently but mode stays normal.
    let resp = mcp_tool_call(
        &mut stream,
        10,
        "execute_command",
        serde_json::json!({"command": "enter-insert-mode"}),
    )
    .await;
    assert!(extract_tool_text(&resp).contains("Executed"));

    // Mode should still be normal.
    let resp = mcp_tool_call(
        &mut stream,
        11,
        "eval_scheme",
        serde_json::json!({"code": "*mode*"}),
    )
    .await;
    let text = extract_tool_text(&resp);
    assert!(
        text.contains("normal"),
        "Expected normal mode on dashboard, got: {}",
        text
    );
}
