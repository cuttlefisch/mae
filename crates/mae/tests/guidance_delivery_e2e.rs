//! Real subprocess e2e for ADR-063 Phase A/C: does the guidance KB's actual content
//! reach a real MCP client's `initialize.instructions` field over the real wire, byte-
//! correct, and does the size budget's fallback behave correctly when it doesn't fit.
//!
//! Mirrors `mcp_tool_tiering_e2e.rs`'s real-subprocess pattern (`mae --headless`,
//! `ADR-055`, real `UnixListener`, isolated per-test `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/
//! `HOME` — no mocks) — duplicated rather than shared, matching this session's own
//! established convention of keeping each `tests/*.rs` integration file self-contained.
//!
//! **Scope note (ADR-063 Decision C / Phase C):** this file proves the MAE-side half of
//! the guidance-delivery mechanism works end-to-end over a real MCP handshake — the
//! actual content that would reach an external client, byte-correct, budget-respecting,
//! with no silent partial/truncated delivery. It deliberately does NOT claim to prove a
//! real VS Code + Copilot agent *acts on* that content — that requires a live external
//! agent session this headless CI environment has no way to drive (no browser/GUI
//! automation available, per ADR-050's own documented constraint, carried forward
//! honestly here rather than silently assumed satisfied). See
//! `docs/verification/adr-063-copilot-live-check.md` for the human-executable script
//! that closes that specific, remaining gap.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mae_kb::KbStore;
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

/// Registers a real KB instance (`kb-registry.toml`) and seeds a real CozoDB store with
/// an `index` node — the exact shape `mae_ai::guidance::read_guidance_kb_context` reads
/// (`## Required Practices (KB: {name})\n{index node's body}`). Must run BEFORE the
/// headless subprocess starts, since it reads this registry at its own startup.
fn seed_guidance_kb(xdg_data: &Path, kb_name: &str, index_body: &str) {
    let mut registry = mae_kb::federation::KbRegistry::default();
    let org_dir = xdg_data.join("guidance-src");
    let uuid = registry.register(kb_name.to_string(), org_dir, xdg_data, None);
    registry.save(xdg_data).expect("save kb-registry.toml");

    let inst = registry
        .find_by_uuid(&uuid)
        .expect("just-registered instance");
    let store = mae_kb::CozoKbStore::open(&inst.db_path).expect("open seeded guidance KB store");
    store.seed_type_system().expect("seed type system");
    store
        .insert_node(&mae_kb::Node::new(
            "index",
            "Index",
            mae_kb::NodeKind::Note,
            index_body,
        ))
        .expect("insert index node");
}

/// Boots a real isolated `mae --headless` instance with a pre-seeded guidance KB (see
/// `seed_guidance_kb`) and `init.scm` setting `ai_guidance_kb` (+ optionally
/// `ai_guidance_inline_budget_chars`), returning the live socket path + a guard.
fn spawn_isolated_headless_with_guidance(
    guidance_kb_name: Option<&str>,
    guidance_index_body: &str,
    inline_budget_chars: Option<usize>,
) -> (PathBuf, HeadlessGuard, tempfile::TempDir) {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("project");
    std::fs::create_dir_all(project_root.join(".git")).unwrap();
    let xdg_config = tmp.path().join("config");
    let xdg_data = tmp.path().join("data");
    std::fs::create_dir_all(&xdg_config).unwrap();
    std::fs::create_dir_all(&xdg_data).unwrap();

    // The real data dir MAE resolves at runtime is $XDG_DATA_HOME/mae (see
    // mae_ai::guidance::default_data_dir), not $XDG_DATA_HOME itself -- seeding
    // anywhere else means the running process never finds this registry.
    let mae_data_dir = xdg_data.join("mae");
    std::fs::create_dir_all(&mae_data_dir).unwrap();

    let mut init_scm = String::new();
    if let Some(name) = guidance_kb_name {
        seed_guidance_kb(&mae_data_dir, name, guidance_index_body);
        init_scm.push_str(&format!("(set-option! \"ai_guidance_kb\" \"{name}\")\n"));
    }
    if let Some(budget) = inline_budget_chars {
        init_scm.push_str(&format!(
            "(set-option! \"ai_guidance_inline_budget_chars\" \"{budget}\")\n"
        ));
    }
    if !init_scm.is_empty() {
        let mae_config_dir = xdg_config.join("mae");
        std::fs::create_dir_all(&mae_config_dir).unwrap();
        std::fs::write(mae_config_dir.join("init.scm"), init_scm).unwrap();
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

/// Does the `initialize` handshake and returns the raw response (not just a bound
/// stream, unlike `mcp_tool_tiering_e2e.rs`'s helper) since this file's assertions are
/// all about the `initialize` response's own `instructions` field.
async fn mcp_initialize(socket_path: &Path) -> serde_json::Value {
    let stream = UnixStream::connect(socket_path)
        .await
        .expect("connect to the real headless socket");
    let mut stream = tokio::io::BufReader::new(stream);
    let init = mcp_roundtrip(
        &mut stream,
        1,
        "initialize",
        serde_json::json!({
            "clientInfo": {"name": "guidance-delivery-e2e-test", "version": "1.0"},
            "protocolVersion": "2025-11-25"
        }),
    )
    .await;
    assert!(init.get("result").is_some(), "initialize failed: {init}");
    init
}

const DISTINCTIVE_PRACTICE: &str =
    "MAE-GUIDANCE-E2E-MARKER-7f3a: every new Rust file must open with the exact comment \
     `// mae-adr063-canary` on line 1.";

/// ADR-063 Phase A, over the real wire: a guidance KB whose content fits within budget
/// must appear byte-identical in `initialize.instructions`, not as a bare pointer.
#[tokio::test]
async fn guidance_content_within_budget_is_inlined_byte_identical_over_the_real_wire() {
    let (socket_path, mut guard, _tmp) = spawn_isolated_headless_with_guidance(
        Some("MaeTestGuidance"),
        DISTINCTIVE_PRACTICE,
        Some(8000),
    );
    let init = mcp_initialize(&socket_path).await;
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("instructions field present");

    assert!(
        instructions.contains(DISTINCTIVE_PRACTICE),
        "expected the guidance KB's real content inlined verbatim, got: {instructions}"
    );
    assert!(
        !instructions.contains("consult KB 'MaeTestGuidance' for required practices"),
        "must not ALSO show the bare pointer once content is inlined: {instructions}"
    );

    drop(guard.child.take());
}

/// The negative/"dry run must fail" case ADR-063's own Verification section requires:
/// with no guidance KB configured at all, the distinctive marker must be ABSENT. This
/// is the concrete proof that the positive test above is not passing vacuously — if
/// guidance delivery were silently broken (neutered), this negative case is what the
/// positive test would degrade into, so running both side by side is the actual
/// regression guard.
#[tokio::test]
async fn no_guidance_kb_configured_means_marker_is_genuinely_absent() {
    let (socket_path, mut guard, _tmp) = spawn_isolated_headless_with_guidance(None, "", None);
    let init = mcp_initialize(&socket_path).await;
    let instructions = init["result"]["instructions"].as_str().unwrap_or("");
    assert!(
        !instructions.contains(DISTINCTIVE_PRACTICE),
        "no guidance KB was configured -- the marker must not appear by coincidence: \
         {instructions}"
    );
    drop(guard.child.take());
}

/// ADR-063 Phase A's budget fallback, over the real wire: content exceeding a small
/// configured budget must fall back cleanly to the pointer, with the real content never
/// appearing even partially/truncated.
#[tokio::test]
async fn guidance_content_over_budget_falls_back_to_pointer_over_the_real_wire() {
    let long_content = format!("{DISTINCTIVE_PRACTICE} {}", "filler ".repeat(50));
    assert!(
        long_content.chars().count() > 50,
        "sanity: fixture is actually long"
    );
    let (socket_path, mut guard, _tmp) =
        spawn_isolated_headless_with_guidance(Some("MaeTestGuidance"), &long_content, Some(50));
    let init = mcp_initialize(&socket_path).await;
    let instructions = init["result"]["instructions"]
        .as_str()
        .expect("instructions field present");

    assert!(
        instructions.contains("consult KB 'MaeTestGuidance' for required practices"),
        "over-budget content must fall back to the pointer: {instructions}"
    );
    assert!(
        !instructions.contains("MAE-GUIDANCE-E2E-MARKER"),
        "over-budget content must never appear even partially/truncated: {instructions}"
    );

    drop(guard.child.take());
}
