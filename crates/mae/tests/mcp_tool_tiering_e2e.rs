//! Real subprocess e2e for K2 of the post-ship quality pass: the MCP
//! server's `tools/list` response is tiered (Core + `request_tools`) by
//! default, and an Extended-tier tool discovered via `request_tools` remains
//! directly callable via `tools/call` even though it was never advertised.
//!
//! Root cause this closes (found via live QA testing with an external VS
//! Code Copilot session): MAE handed every MCP client the full ~758-tool
//! flat `tools/list`, unfiltered — the built-in agent already solves this
//! exact problem (`classify_tool_tier`'s Core/Extended split +
//! `request_tools`/`search_tools`, `crates/ai/src/session/mod.rs`), but it
//! was never applied to the MCP server's own `tools/list`
//! (`crates/mae/src/main.rs`). A large flat tool list measurably degrades
//! external tool-selection accuracy (`docs/MODEL_SUPPORT.md`) — this was
//! observed live as an external agent calling the wrong tool with empty
//! arguments 9 times in a row instead of a well-named Core tool.
//!
//! Uses `mae --headless` (ADR-055) as the real, spawnable server under test
//! — the shared bootstrap it reuses (`crates/mae/src/main.rs`'s tools/list
//! construction) is identical to the interactive editor's own MCP socket.
//! Mirrors `headless_e2e.rs`'s real-subprocess pattern (real `UnixListener`
//! -- no mocks, isolated per-test `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`HOME`).

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

struct HeadlessGuard {
    child: Option<Child>,
}

impl Drop for HeadlessGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            if child.try_wait().ok().flatten().is_none() {
                send_sigterm(&child);
                if wait_for_exit(&mut child, Duration::from_secs(3)).is_none() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

/// Boots a real isolated `mae --headless` instance, optionally with a
/// pre-seeded `init.scm` (e.g. to flip `mcp_tools_tiered_by_default` off),
/// and returns the live socket path + a guard that SIGTERMs it on drop.
fn spawn_isolated_headless(init_scm: Option<&str>) -> (PathBuf, HeadlessGuard, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(project_root.join(".git")).unwrap();
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();

    if let Some(content) = init_scm {
        let mae_config_dir = xdg_config.join("mae");
        std::fs::create_dir_all(&mae_config_dir).unwrap();
        std::fs::write(mae_config_dir.join("init.scm"), content).unwrap();
    }

    let mae = env!("CARGO_BIN_EXE_mae");

    let mut print_cmd = Command::new(mae);
    print_cmd
        .args(["--headless", "--print-socket-path"])
        .current_dir(&project_root);
    isolated_env(&mut print_cmd, &xdg_config, &xdg_data, tmp.path());
    let print_output = print_cmd
        .output()
        .expect("failed to run `mae --headless --print-socket-path`");
    assert!(print_output.status.success());
    let socket_path = PathBuf::from(
        String::from_utf8_lossy(&print_output.stdout)
            .trim()
            .to_string(),
    );

    let stderr_log = std::fs::File::create(tmp.path().join("headless-stderr.log")).unwrap();
    let mut spawn_cmd = Command::new(mae);
    spawn_cmd
        .args(["--headless"])
        .current_dir(&project_root)
        .stdout(Stdio::null())
        .stderr(stderr_log);
    isolated_env(&mut spawn_cmd, &xdg_config, &xdg_data, tmp.path());
    let child = spawn_cmd.spawn().expect("failed to spawn `mae --headless`");
    let guard = HeadlessGuard { child: Some(child) };

    // 30s, not 15s: three real subprocess boots in this file plus
    // whatever else the machine is doing can push a cold headless boot
    // uncomfortably close to a tight budget (observed: 14.7s used out of a
    // 15s budget even in isolation) -- same class of fix as this session's
    // VS Code extension timeout bump (c2edc5f0), applied here too rather
    // than leaving a flaky-under-load test.
    let bound = wait_for_socket_live(&socket_path, Duration::from_secs(30));
    if !bound {
        let log = std::fs::read_to_string(tmp.path().join("headless-stderr.log"))
            .unwrap_or_else(|e| format!("<failed to read stderr log: {e}>"));
        eprintln!("=== headless stderr ===\n{log}\n=== end ===");
    }
    assert!(
        bound,
        "headless instance never bound its stable socket at {}",
        socket_path.display()
    );

    (socket_path, guard, tmp)
}

async fn mcp_roundtrip(
    stream: &mut tokio::io::BufReader<UnixStream>,
    id: u64,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let req = serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
    mae_mcp::write_framed(stream, req.to_string().as_bytes(), Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| panic!("write {method} failed: {e}"));
    let resp = mae_mcp::read_message(stream)
        .await
        .unwrap_or_else(|e| panic!("read {method} response failed: {e}"))
        .unwrap_or_else(|| panic!("{method} response missing"));
    serde_json::from_str(&resp).unwrap()
}

async fn mcp_handshake(socket_path: &Path) -> tokio::io::BufReader<UnixStream> {
    let stream = UnixStream::connect(socket_path)
        .await
        .expect("connect to the real headless socket");
    let mut stream = tokio::io::BufReader::new(stream);
    let init = mcp_roundtrip(
        &mut stream,
        1,
        "initialize",
        serde_json::json!({
            "clientInfo": {"name": "tiering-e2e-test", "version": "1.0"},
            "protocolVersion": "2025-11-25"
        }),
    )
    .await;
    assert!(init.get("result").is_some(), "initialize failed: {init}");
    let notif = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    mae_mcp::write_framed(
        &mut stream,
        notif.to_string().as_bytes(),
        Duration::from_secs(5),
    )
    .await
    .expect("write notifications/initialized");
    stream
}

#[tokio::test]
async fn fresh_client_gets_core_tier_tools_list_by_default() {
    let (socket_path, mut guard, _tmp) = spawn_isolated_headless(None);
    let mut stream = mcp_handshake(&socket_path).await;

    let tools_resp = mcp_roundtrip(&mut stream, 2, "tools/list", serde_json::Value::Null).await;
    let tools = tools_resp["result"]["tools"]
        .as_array()
        .expect("tools array present");

    assert!(
        tools.len() < 100,
        "expected a Core-tier-only tools/list by default, got {} tools (full flat list?)",
        tools.len()
    );
    assert!(
        tools.iter().any(|t| t["name"] == "request_tools"),
        "request_tools must be present so a client can discover the escalation path"
    );
    assert!(
        tools.iter().any(|t| t["name"] == "search_tools"),
        "search_tools (Core-tier) must be present"
    );
    // A known Extended-tier tool must NOT be advertised up front.
    assert!(
        !tools
            .iter()
            .any(|t| t["name"] == "command_kb_set_search_scope"),
        "command_kb_set_search_scope is Extended-tier and must not appear in the default list"
    );

    drop(stream);
    let mut child = guard.child.take().unwrap();
    send_sigterm(&child);
    wait_for_exit(&mut child, Duration::from_secs(10));
}

#[tokio::test]
async fn extended_tier_tool_is_discoverable_via_request_tools_and_directly_callable() {
    let (socket_path, mut guard, _tmp) = spawn_isolated_headless(None);
    let mut stream = mcp_handshake(&socket_path).await;

    // 1. Confirm the target Extended-tier tool is absent from tools/list.
    let tools_resp = mcp_roundtrip(&mut stream, 2, "tools/list", serde_json::Value::Null).await;
    let tools = tools_resp["result"]["tools"].as_array().unwrap();
    assert!(
        !tools
            .iter()
            .any(|t| t["name"] == "command_kb_set_search_scope"),
        "precondition: command_kb_set_search_scope must not be pre-listed"
    );

    // 2. Call request_tools by exact name -- the escalation path a fresh
    // client is told about via initialize.instructions (K2).
    let call_resp = mcp_roundtrip(
        &mut stream,
        3,
        "tools/call",
        serde_json::json!({
            "name": "request_tools",
            "arguments": {"categories": "", "tools": "command_kb_set_search_scope"}
        }),
    )
    .await;
    let content_text = call_resp["result"]["content"][0]["text"]
        .as_str()
        .expect("request_tools returned text content");
    assert!(
        content_text.contains("command_kb_set_search_scope"),
        "request_tools result must include the requested tool's definition, got: {content_text}"
    );
    assert!(
        content_text.contains("input_schema"),
        "request_tools must return enough (a schema) for an external client to construct a \
         valid call, not just a name -- got: {content_text}"
    );

    // 3. The tool is now directly callable via tools/call, even though it
    // was never in tools/list -- proving tools/call dispatch is never
    // restricted to what was advertised (the actual mechanism that makes
    // tiering safe).
    let dispatch_resp = mcp_roundtrip(
        &mut stream,
        4,
        "tools/call",
        serde_json::json!({
            "name": "command_kb_set_search_scope",
            "arguments": {}
        }),
    )
    .await;
    assert!(
        dispatch_resp.get("error").is_none(),
        "expected command_kb_set_search_scope to dispatch successfully once discovered, got \
         error: {dispatch_resp:?}"
    );
    let dispatch_text = dispatch_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(
        !dispatch_text.contains("Unknown tool"),
        "the tool must actually reach dispatch (not be rejected as unrecognized) even though \
         it was never in tools/list -- got: {dispatch_text}"
    );

    drop(stream);
    let mut child = guard.child.take().unwrap();
    send_sigterm(&child);
    wait_for_exit(&mut child, Duration::from_secs(10));
}

#[tokio::test]
async fn tiering_can_be_disabled_via_option_for_a_deployment_tuned_around_the_full_list() {
    let (socket_path, mut guard, _tmp) = spawn_isolated_headless(Some(
        "(set-option! \"mcp_tools_tiered_by_default\" \"false\")\n",
    ));
    let mut stream = mcp_handshake(&socket_path).await;

    let tools_resp = mcp_roundtrip(&mut stream, 2, "tools/list", serde_json::Value::Null).await;
    let tools = tools_resp["result"]["tools"].as_array().unwrap();
    assert!(
        tools.len() > 100,
        "mcp_tools_tiered_by_default=false must restore the full flat tool set, got {}",
        tools.len()
    );
    assert!(
        tools
            .iter()
            .any(|t| t["name"] == "command_kb_set_search_scope"),
        "the full list must include Extended-tier tools when tiering is disabled"
    );

    drop(stream);
    let mut child = guard.child.take().unwrap();
    send_sigterm(&child);
    wait_for_exit(&mut child, Duration::from_secs(10));
}
