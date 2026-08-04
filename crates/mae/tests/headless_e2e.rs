//! Real subprocess e2e for `mae --headless` (ADR-055, Phase E).
//!
//! Every existing test for this feature (`headless_loop.rs`'s own
//! `#[cfg(test)] mod tests`) drives internal Rust functions in-process —
//! real `UnixListener`s, but never the actual compiled binary. That leaves a
//! real gap a QA pass on this epic surfaced: CLI flag wiring
//! (`--headless`/`--print-socket-path`), full process bootstrap, the real
//! accept loop, and real SIGTERM handling are all unverified end-to-end.
//! This test spawns the real `mae` binary (`env!("CARGO_BIN_EXE_mae")`,
//! Cargo's own guarantee that it's built before this test runs — no
//! separate artifact download/env-var dance needed, since `mae` is a
//! sibling binary within this same crate) and talks to it exactly as a real
//! client (or the "MAE for VS Code" extension) would: resolve the stable
//! socket path via `--print-socket-path`, spawn the long-running instance,
//! do a real MCP round trip over the real socket, then SIGTERM it and
//! confirm clean shutdown.
//!
//! Isolated via a per-test tempdir `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`HOME`
//! and a throwaway project directory — never touches the real user's config
//! or a shared path. Linux-only (SIGTERM-via-libc, matching the daemon
//! supervisor's real-process test precedent, `crates/mae/src/
//! daemon_supervisor.rs`) — cross-platform parity for the underlying
//! headless mechanism itself is covered by `headless_loop.rs`'s own
//! cross-platform unit tests.

#![cfg(target_os = "linux")]

use std::time::Duration;

use tokio::net::UnixStream;

mod headless_test_support;
use headless_test_support::{socket_is_live, spawn_isolated_headless};

#[tokio::test]
async fn headless_real_subprocess_boots_serves_mcp_and_shuts_down_cleanly() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(project_root.join(".git")).unwrap();
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();

    // 1 & 2. Resolve the stable socket path the same way an external tool
    // (the VS Code extension, `crates/mae/src/cli.rs::handle_print_socket_path`)
    // would, then spawn the real long-running headless instance -- proving
    // that flag and the long-running instance below agree on the exact same
    // path, not just that each independently "works." 30s, not 15s: a cold
    // headless boot was observed using 14.7s out of a 15s budget even in
    // isolation on this machine -- flaky-under-load, not a real regression
    // (found while hardening K2's own tiering e2e test,
    // mcp_tool_tiering_e2e.rs, the same class of fix as the VS Code
    // extension's own too-tight timeout bump, c2edc5f0) --
    // `spawn_isolated_headless` already waits up to 30s for the socket.
    let (socket_path, mut guard) =
        spawn_isolated_headless(&project_root, &xdg_config, &xdg_data, tmp.path(), None);
    assert!(
        !socket_path.as_os_str().is_empty(),
        "expected a real resolved socket path"
    );

    // 3. A real MCP round trip over the real socket (Content-Length framing,
    // reusing mae_mcp's own framing helpers rather than hand-rolling them
    // again in this test — principle #8).
    let stream = UnixStream::connect(&socket_path)
        .await
        .expect("connect to the real headless socket");
    let mut stream = tokio::io::BufReader::new(stream);

    let init_req = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "clientInfo": {"name": "headless-e2e-test", "version": "1.0"},
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
    let init_resp = mae_mcp::read_message(&mut stream)
        .await
        .expect("read initialize response")
        .expect("initialize response present");
    let init_value: serde_json::Value = serde_json::from_str(&init_resp).unwrap();
    assert!(
        init_value.get("result").is_some(),
        "initialize failed: {init_value}"
    );

    let notif = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    mae_mcp::write_framed(
        &mut stream,
        notif.to_string().as_bytes(),
        Duration::from_secs(5),
    )
    .await
    .expect("write notifications/initialized");

    let tools_req = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    mae_mcp::write_framed(
        &mut stream,
        tools_req.to_string().as_bytes(),
        Duration::from_secs(5),
    )
    .await
    .expect("write tools/list");
    let tools_resp = mae_mcp::read_message(&mut stream)
        .await
        .expect("read tools/list response")
        .expect("tools/list response present");
    let tools_value: serde_json::Value = serde_json::from_str(&tools_resp).unwrap();
    let tools = tools_value["result"]["tools"]
        .as_array()
        .expect("tools array present");
    // K2 (post-ship quality pass, `mcp_tools_tiered_by_default` defaults
    // true): a fresh editor with no explicit override now advertises only
    // the Core tier (~85 tools) + request_tools, not the full ~700+ catalog
    // — see mcp_tool_tiering_e2e.rs for the dedicated tiering test, this is
    // just confirming the real default didn't silently regress back to the
    // full flat list.
    assert!(
        tools.len() < 100,
        "expected the K2 tiered-by-default Core tool set, got {} (full flat list?)",
        tools.len()
    );

    // Regression guard spanning two phases through one real process: Phase
    // A's annotation wiring must still be live end-to-end, not just correct
    // in its own isolated unit tests. `buffer_read` is Core-tier, unlike
    // `kb_search` (Extended), so it stays present under K2's default tiering.
    let buffer_read = tools
        .iter()
        .find(|t| t["name"] == "buffer_read")
        .expect("buffer_read tool present in the real tool set");
    assert_eq!(buffer_read["annotations"]["readOnlyHint"], true);

    drop(stream);

    // 4. Graceful shutdown: real SIGTERM, real process, confirm clean exit
    // AND that the listener genuinely stopped accepting connections
    // (deliberately NOT asserting the stable socket FILE is removed —
    // headless_loop.rs's own claim_stable_socket_at only self-heals a stale
    // file lazily on the NEXT claim attempt, by design; asserting file
    // removal here would encode a behavior the real code doesn't have).
    let exit_status = guard
        .shutdown(Duration::from_secs(10))
        .expect("`mae --headless` did not exit within 10s of SIGTERM");
    assert!(
        exit_status.success(),
        "expected a clean exit on SIGTERM, got {exit_status:?}"
    );
    assert!(
        !socket_is_live(&socket_path),
        "the stable socket must stop accepting connections after clean shutdown"
    );
}
