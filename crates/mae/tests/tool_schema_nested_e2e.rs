//! Real subprocess e2e for L1 of the epic's issue-closure pass (#376, Phase
//! A's flat-schema-extension DoD item): `ToolProperty` now supports nested
//! `items`/`properties` (JSON Schema `array`-of-`object` shape), applied to
//! `propose_changes`'s `changes` parameter — a genuinely structured param
//! (each element needs `file_path` + `new_content`) that previously
//! serialized as a bare `{"type": "array"}` with zero information about
//! what belongs inside, giving an external MCP client nothing to construct
//! a valid call from.
//!
//! Spawns a real `mae --headless` instance, requests `propose_changes` via
//! `request_tools` (it's Extended-tier under K2's default tiering, so this
//! also doubles as a real end-to-end proof that nested schemas survive the
//! full MCP `tools/call` round trip, not just direct serialization), and
//! asserts the real JSON on the wire has a proper `items.properties`
//! sub-schema.

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

#[tokio::test]
async fn propose_changes_schema_has_a_real_items_sub_schema_over_the_real_wire() {
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
    let mut guard = HeadlessGuard { child: Some(child) };

    assert!(
        wait_for_socket_live(&socket_path, Duration::from_secs(30)),
        "headless instance never bound its stable socket at {}",
        socket_path.display()
    );

    let stream = UnixStream::connect(&socket_path).await.unwrap();
    let mut stream = tokio::io::BufReader::new(stream);

    let init = mcp_roundtrip(
        &mut stream,
        1,
        "initialize",
        serde_json::json!({
            "clientInfo": {"name": "tool-schema-nested-e2e-test", "version": "1.0"},
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
    .unwrap();

    // propose_changes is Extended-tier (K2 default tiering), so request it
    // explicitly -- this also proves nested schemas survive request_tools'
    // own JSON round trip, not just direct struct serialization.
    let req_resp = mcp_roundtrip(
        &mut stream,
        2,
        "tools/call",
        serde_json::json!({
            "name": "request_tools",
            "arguments": {"categories": "", "tools": "propose_changes"}
        }),
    )
    .await;
    let text = req_resp["result"]["content"][0]["text"]
        .as_str()
        .expect("request_tools returned text content");
    let tools: serde_json::Value = serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("request_tools output wasn't valid JSON: {e}\n{text}"));
    let propose_changes = tools
        .as_array()
        .and_then(|arr| arr.iter().find(|t| t["name"] == "propose_changes"))
        .unwrap_or_else(|| panic!("propose_changes not found in request_tools output: {text}"));

    let changes_schema = &propose_changes["input_schema"]["properties"]["changes"];
    assert_eq!(
        changes_schema["type"], "array",
        "changes param must stay array-typed, got: {changes_schema}"
    );
    let item_schema = &changes_schema["items"];
    assert_eq!(
        item_schema["type"], "object",
        "changes.items must be a real object schema, not absent -- got: {changes_schema}"
    );
    assert!(
        item_schema["properties"]["file_path"]["type"] == "string",
        "changes.items.properties.file_path must be present and string-typed, got: {item_schema}"
    );
    assert!(
        item_schema["properties"]["new_content"]["type"] == "string",
        "changes.items.properties.new_content must be present and string-typed, got: {item_schema}"
    );
    let required: Vec<&str> = item_schema["required"]
        .as_array()
        .expect("changes.items.required must be present")
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"file_path") && required.contains(&"new_content"));

    drop(stream);
    let mut child = guard.child.take().unwrap();
    send_sigterm(&child);
    wait_for_exit(&mut child, Duration::from_secs(10));
}
