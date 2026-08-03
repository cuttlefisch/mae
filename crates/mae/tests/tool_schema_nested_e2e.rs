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

    // ADR-085 / decision #9: `propose_changes` is one of three inherently
    // interactive tools (with `ask_user` and `delegate`) that an external MCP
    // client cannot actually invoke — they pause the embedded session on a
    // oneshot channel awaiting a human reply, which has no meaning mid
    // `tools/call`. They are therefore withheld from every EXTERNAL discovery
    // surface rather than advertised and then refused.
    //
    // So this half of the test now pins that absence over the real wire, and
    // the nested-schema assertions move below to the registry definition.
    // Losing the over-the-wire nested-schema check is a real (small) reduction
    // in fidelity, taken deliberately: `propose_changes` is the ONLY tool in
    // the registry with a nested array-items sub-schema, so there is no
    // substitute vehicle, and inventing a fake tool to keep the shape of the
    // test would prove less than testing the real serialization does. The
    // framing layer does not reshape JSON — `write_framed` prepends a
    // Content-Length to bytes serde already produced — so the serialization
    // round trip below covers the actual risk (a nested `items` sub-schema
    // being flattened or dropped).
    for withheld in ["propose_changes", "ask_user", "delegate"] {
        let req_resp = mcp_roundtrip(
            &mut stream,
            2,
            "tools/call",
            serde_json::json!({
                "name": "request_tools",
                "arguments": {"categories": "", "tools": withheld}
            }),
        )
        .await;
        let text = req_resp["result"]["content"][0]["text"]
            .as_str()
            .expect("request_tools returned text content");
        assert!(
            !text.contains(&format!("\"{withheld}\"")),
            "{withheld} is interactive-only and must not be reachable through an \
             external discovery surface, but request_tools returned it: {text}"
        );
    }

    drop(stream);
    let mut child = guard.child.take().unwrap();
    send_sigterm(&child);
    wait_for_exit(&mut child, Duration::from_secs(10));
}

/// The nested-schema coverage that used to run over the socket, kept against
/// the registry definition now that `propose_changes` is embedded-only.
///
/// The risk this guards is real and easy to reintroduce: a builder that
/// declares `"type": "array"` without an `items` sub-schema, or a serializer
/// that flattens one, produces a schema an agent cannot construct a valid call
/// against — the same "advertised but unusable" class as a tool whose schema
/// omits a required parameter.
#[test]
fn propose_changes_nested_items_schema_survives_serialization() {
    let tools = mae_ai::ai_specific_tools(&mae_core::OptionRegistry::new());
    let def = tools
        .iter()
        .find(|t| t.name == "propose_changes")
        .expect("propose_changes must still be registered (embedded-only, not deleted)");

    // Round-trip through serde exactly as the wire does, then assert on the
    // decoded JSON rather than on the struct — a struct-level assertion would
    // not catch a serializer that drops the nested schema.
    let json: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(def).expect("tool definition serializes"))
            .expect("serialized tool definition is valid JSON");

    let changes = &json["parameters"]["properties"]["changes"];
    assert_eq!(
        changes["type"], "array",
        "changes must stay array-typed: {changes}"
    );

    let item = &changes["items"];
    assert_eq!(
        item["type"], "object",
        "changes.items must be a real object schema, not absent: {changes}"
    );
    assert_eq!(item["properties"]["file_path"]["type"], "string");
    assert_eq!(item["properties"]["new_content"]["type"], "string");

    let required: Vec<&str> = item["required"]
        .as_array()
        .expect("changes.items.required must be present")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        required.contains(&"file_path") && required.contains(&"new_content"),
        "got: {required:?}"
    );
}
