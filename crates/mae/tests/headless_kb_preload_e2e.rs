//! Does a node that exists ONLY in the durable primary store at startup become
//! searchable in a real `mae --headless` session?
//!
//! It did not, for the entire life of the headless surface, and nothing here
//! noticed. `init_kb_federation` spawns the O(n) primary-store `load_all` off
//! the main thread (a synchronous load tripped the 10s startup watchdog) and
//! parks the receiver in `kb.pending_preload`. The GUI drained it in
//! `idle_work`, the TUI drained part of it by hand, and the headless loops —
//! including `--self-test`, and the mode the MCP server actually runs in —
//! drained none of it. So `kb.primary` stayed empty forever, and because
//! `primary_thin()` is false in that mode `kb_federated_search_scoped` ranked
//! over the empty mirror: `kb_search` returned **zero of the user's own notes,
//! with no error**, while the query-layer-backed tools (`kb_list`, `kb_graph`,
//! `kb_links_*`) saw them fine.
//!
//! # Why the existing headless e2e coverage could not catch it
//!
//! Verified rather than assumed, because "there is already a headless KB test"
//! is exactly the reasoning that let this ship. `headless_kb_convergence_e2e.rs`
//! drives `kb_create` -> `kb_get` CRDT convergence through a real daemon: every
//! node it asks about was written *by that same session*, so it lands in the
//! in-memory mirror through the write path and is found regardless of whether
//! the preload ever drained. It never asks about a node that was already on
//! disk before the process started — which is the only shape that exercises the
//! preload at all.
//!
//! So this test seeds the store **before** MAE launches, and asks a question
//! only the preload can answer.
//!
//! # Shape
//!
//! Real subprocess, real Unix socket, real MCP `tools/call` — the
//! `spawn_isolated_headless` pattern the other headless e2es use, with a
//! per-test `XDG_CONFIG_HOME`/`XDG_DATA_HOME`/`HOME`. No mocks: a unit test
//! calling `drain_kb_preload()` directly proves the drain function works, which
//! was never in doubt and is not what broke.

#![cfg(target_os = "linux")]

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use tokio::net::UnixStream;

mod headless_test_support;
use headless_test_support::{
    grant_permission_tier, isolated_env, wait_for_socket_live, HeadlessGuard,
};

/// `kb_search` is a read, but the spawned instance needs a tier that does not
/// *ask* — a non-interactive surface denies rather than prompting (ADR-090 D5).
const REQUIRED_TIER: &str = "write";

/// A title no other corpus contains, so a hit cannot come from MAE's own
/// bundled manual/practices content leaking into the result set. That
/// distinction is the whole point: MAE's docs reach the query layer by a
/// different path and would mask the defect.
const SEEDED_TITLE: &str = "Tidepool Survey Notes";
const SEEDED_ID: &str = "note:tidepool-survey";
const SEEDED_BODY: &str =
    "Quadrat counts from the north shelf, recorded before this MAE process existed.";

/// Write a real CozoDB primary store at the exact path `init_kb_federation`
/// opens (`<data dir>/kb/primary.cozo`), carrying one distinctive node.
///
/// Engine matches the documented default (`kb_storage_engine` = sqlite). Using
/// sled here would still *work*, but it would additionally trip the in-place
/// migration on first open and muddy what this test is measuring.
fn seed_primary_store(xdg_data: &Path) {
    use mae_kb::{KbStore, Node, NodeKind, NodeSource};

    let kb_root = xdg_data.join("mae").join("kb");
    std::fs::create_dir_all(&kb_root).expect("create kb root");
    let store = mae_kb::CozoKbStore::open_with_engine(kb_root.join("primary.cozo"), "sqlite")
        .expect("open seeded primary store");
    store.seed_type_system().expect("seed type system");

    // `UserOrg`, not `Seed`: this must look like the user's own content, since
    // `Seed` is what marks MAE's shipped read-only material and is filtered
    // differently in places.
    let mut node = Node::new(SEEDED_ID, SEEDED_TITLE, NodeKind::Note, SEEDED_BODY);
    node.source = Some(NodeSource::UserOrg);
    store.insert_node(&node).expect("insert seeded node");
}

struct Session {
    child: std::process::Child,
    stream: tokio::io::BufReader<UnixStream>,
    next_id: u64,
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Session {
    async fn start(project_root: &Path, xdg_config: &Path, xdg_data: &Path, home: &Path) -> Self {
        let mae = env!("CARGO_BIN_EXE_mae");

        let mut print_cmd = Command::new(mae);
        print_cmd
            .args(["--headless", "--print-socket-path"])
            .current_dir(project_root);
        isolated_env(&mut print_cmd, xdg_config, xdg_data, home);
        let out = print_cmd.output().expect("run --print-socket-path");
        assert!(out.status.success(), "--print-socket-path failed");
        let socket_path =
            std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());

        let mut spawn_cmd = Command::new(mae);
        spawn_cmd
            .args(["--headless"])
            .current_dir(project_root)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        isolated_env(&mut spawn_cmd, xdg_config, xdg_data, home);
        grant_permission_tier(&mut spawn_cmd, REQUIRED_TIER);
        // Guarded the instant spawn() returns — every step below can panic.
        let guard = HeadlessGuard::new(spawn_cmd.spawn().expect("spawn mae --headless"));

        assert!(
            wait_for_socket_live(&socket_path, Duration::from_secs(30)),
            "headless instance never bound its socket at {}",
            socket_path.display()
        );

        let mut stream =
            tokio::io::BufReader::new(UnixStream::connect(&socket_path).await.unwrap());
        let init = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "clientInfo": {"name": "kb-preload-e2e", "version": "1.0"},
                "protocolVersion": "2025-11-25"
            }
        });
        mae_mcp::write_framed(
            &mut stream,
            init.to_string().as_bytes(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        let resp = mae_mcp::read_message(&mut stream).await.unwrap().unwrap();
        let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert!(v.get("result").is_some(), "initialize failed: {v}");
        let notif = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        mae_mcp::write_framed(
            &mut stream,
            notif.to_string().as_bytes(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        Session {
            child: guard.into_child(),
            stream,
            next_id: 2,
        }
    }

    async fn call_tool(&mut self, name: &str, arguments: serde_json::Value) -> String {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        });
        mae_mcp::write_framed(
            &mut self.stream,
            req.to_string().as_bytes(),
            Duration::from_secs(10),
        )
        .await
        .unwrap_or_else(|e| panic!("write tools/call({name}): {e}"));
        let resp = mae_mcp::read_message(&mut self.stream)
            .await
            .unwrap_or_else(|e| panic!("read tools/call({name}): {e}"))
            .unwrap_or_else(|| panic!("tools/call({name}) response missing"));
        let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert!(v.get("error").is_none(), "tools/call({name}) errored: {v}");
        // A permission denial is NOT a JSON-RPC error — it comes back as a
        // well-formed result with `isError: true`. Catch it here rather than
        // letting it masquerade as "the node wasn't found", which is exactly
        // the wrong conclusion for this test to reach.
        let result = v.get("result").unwrap_or(&serde_json::Value::Null).clone();
        assert!(
            result.get("isError") != Some(&serde_json::Value::Bool(true)),
            "tools/call({name}) was denied or failed: {result}"
        );
        result.to_string()
    }
}

/// The regression test: seed the store, start MAE, search for the seeded node.
///
/// Fails before the `drain_kb_background` fix — the search returns nothing,
/// successfully, forever.
#[tokio::test]
async fn a_node_already_in_the_store_is_searchable_in_a_headless_session() {
    let project = tempfile::tempdir().unwrap();
    let xdg_config = tempfile::tempdir().unwrap();
    let xdg_data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    // `--headless` refuses a directory with no detectable project root, and its
    // socket path is project-keyed. A bare `.git` is the cheapest marker
    // `detect_project_root` accepts.
    std::fs::create_dir_all(project.path().join(".git")).unwrap();

    // BEFORE the process exists. This ordering is the test.
    seed_primary_store(xdg_data.path());

    let mut session = Session::start(
        project.path(),
        xdg_config.path(),
        xdg_data.path(),
        home.path(),
    )
    .await;

    // The preload is asynchronous by design, and the drain runs on the loop's
    // idle tick — so poll rather than assuming the first call wins. Bounded,
    // and the failure message says which of the two possible causes it is.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut last;
    loop {
        last = session
            .call_tool("kb_search", serde_json::json!({"query": SEEDED_TITLE}))
            .await;
        if last.contains(SEEDED_ID) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "a node present in the primary store BEFORE startup never became \
             searchable headless. The primary-store preload is spawned by \
             `init_kb_federation` and drained by `Editor::drain_kb_background`; \
             if the headless loop stopped calling it, `kb.primary` stays empty \
             and `kb_search` ranks over nothing. Last result: {last}"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    assert!(
        last.contains(SEEDED_TITLE),
        "found the id but not the title — the mirror holds a stub rather than \
         the real node: {last}"
    );
}
