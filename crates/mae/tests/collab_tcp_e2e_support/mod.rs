//! Shared helpers for the collab_tcp_e2e test suite, split across
//! collab_tcp_e2e_*.rs files to stay under the 500-line test ceiling. NOT
//! itself a test target: Cargo's `tests/*.rs` auto-discovery only globs
//! direct children of `tests/`, so `tests/<dir>/mod.rs` is never picked up
//! as its own integration-test binary.
//!
//! Gated with `#[ignore]` -- run via:
//!   MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e_tests -- --ignored --nocapture
//! (and similarly for the other collab_tcp_e2e_*.rs split files)
//!
//! Not every consumer uses every helper (each file is a separate compiled
//! crate) -- #[allow(dead_code)] suppresses the resulting per-binary warning.

use std::process::Stdio;
use std::time::Duration;

use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc};
use mae_sync::text::TextSync;
use tokio::io::BufReader;
use tokio::net::TcpStream;
use tokio::process::Command;

/// TCP client wrapper for testing.
#[allow(dead_code)]
pub struct TcpClient {
    pub reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    pub writer: tokio::net::tcp::OwnedWriteHalf,
    pub next_id: u64,
}

#[allow(dead_code)]
impl TcpClient {
    pub async fn connect(addr: &str) -> Self {
        let stream = TcpStream::connect(addr).await.expect("failed to connect");
        let (read, write) = stream.into_split();
        let mut client = TcpClient {
            reader: BufReader::new(read),
            writer: write,
            next_id: 1,
        };
        client.initialize().await;
        client.subscribe().await;
        client
    }

    pub async fn send(&mut self, msg: &serde_json::Value) {
        let body = serde_json::to_vec(msg).unwrap();
        mae_mcp::write_framed(&mut self.writer, &body, std::time::Duration::from_secs(5))
            .await
            .unwrap();
    }

    pub async fn recv(&mut self) -> serde_json::Value {
        loop {
            let text = mae_mcp::read_message(&mut self.reader)
                .await
                .unwrap()
                .unwrap();
            let val: serde_json::Value = serde_json::from_str(&text).unwrap();
            // Skip notifications (messages with method but no result/error/id).
            if val.get("method").is_some()
                && val.get("result").is_none()
                && val.get("error").is_none()
                && val.get("id").is_none()
            {
                continue;
            }
            return val;
        }
    }

    pub async fn recv_timeout(&mut self, ms: u64) -> Option<serde_json::Value> {
        match tokio::time::timeout(
            Duration::from_millis(ms),
            mae_mcp::read_message(&mut self.reader),
        )
        .await
        {
            Ok(Ok(Some(text))) => serde_json::from_str(&text).ok(),
            _ => None,
        }
    }

    pub async fn initialize(&mut self) {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"initialize","params":{"clientInfo":{"name":"tcp-test"}}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "initialize failed: {resp}");
    }

    pub async fn subscribe(&mut self) {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"notifications/subscribe","params":{"types":["sync_update","peer_joined","peer_left","save_committed"]}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "subscribe failed: {resp}");
    }

    pub async fn share(&mut self, doc: &str, content: &str) {
        let ts = TextSync::new(content);
        let state = ts.encode_state();
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/share","params":{"doc":doc,"update":update_to_base64(&state)}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "share failed: {resp}");
    }

    pub async fn send_update(&mut self, doc: &str, update: &[u8]) -> serde_json::Value {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/update","params":{"doc":doc,"update":update_to_base64(update)}});
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    pub async fn full_state(&mut self, doc: &str) -> Vec<u8> {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"sync/full_state","params":{"doc":doc}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        base64_to_update(resp["result"]["state"].as_str().unwrap()).unwrap()
    }

    pub async fn content(&mut self, doc: &str) -> String {
        let msg = serde_json::json!({"jsonrpc":"2.0","id":self.next_id,"method":"docs/content","params":{"doc":doc}});
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        resp["result"]["content"].as_str().unwrap().to_string()
    }

    /// Share a KB with the server. Returns the response.
    pub async fn kb_share(
        &mut self,
        kb_id: &str,
        name: &str,
        creator: &str,
        collection_state: &[u8],
        nodes: &[(&str, &[u8])],
    ) -> serde_json::Value {
        // Build via the SHARED wire builder, not a hand-rolled literal — so this
        // e2e exercises the exact serialization production emits (ADR-020 B-8: the
        // bug hid precisely because a hand-rolled test sent a different shape).
        let nodes_b64: Vec<(String, String)> = nodes
            .iter()
            .map(|(id, state)| (id.to_string(), update_to_base64(state)))
            .collect();
        let msg = mae_sync::wire::kb_share_request(
            self.next_id,
            kb_id,
            name,
            creator,
            &update_to_base64(collection_state),
            &nodes_b64,
        );
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "kb/share failed: {resp}");
        resp
    }

    /// Join a KB. Returns the response with collection_state and nodes.
    pub async fn kb_join(&mut self, kb_id: &str) -> serde_json::Value {
        let msg = mae_sync::wire::kb_join_request(self.next_id, kb_id, &[]);
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Send a KB node update. Returns the response.
    pub async fn kb_node_update(
        &mut self,
        kb_id: &str,
        node_id: &str,
        update: &[u8],
    ) -> serde_json::Value {
        let msg = mae_sync::wire::kb_node_update_request(
            self.next_id,
            kb_id,
            node_id,
            &update_to_base64(update),
        );
        self.next_id += 1;
        self.send(&msg).await;
        self.recv().await
    }

    /// Leave a KB. Returns the response.
    pub async fn kb_leave(&mut self, kb_id: &str) -> serde_json::Value {
        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": self.next_id,
            "method": "kb/leave",
            "params": { "kb_id": kb_id }
        });
        self.next_id += 1;
        self.send(&msg).await;
        let resp = self.recv().await;
        assert!(resp.get("error").is_none(), "kb/leave failed: {resp}");
        resp
    }

    pub async fn wait_for_notification(
        &mut self,
        method: &str,
        timeout_ms: u64,
    ) -> Option<serde_json::Value> {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return None;
            }
            match tokio::time::timeout(remaining, mae_mcp::read_message(&mut self.reader)).await {
                Ok(Ok(Some(text))) => {
                    if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                        if val.get("method").and_then(|m| m.as_str()) == Some(method) {
                            return Some(val);
                        }
                    }
                }
                _ => return None,
            }
        }
    }
}

/// Find the mae-daemon binary. Checks daemon workspace target dirs first,
/// then editor workspace target dirs, then falls back to None (use cargo run).
#[allow(dead_code)]
pub fn find_daemon_binary() -> Option<std::path::PathBuf> {
    if let Ok(bin) = std::env::var("MAE_DAEMON_BIN") {
        return Some(std::path::PathBuf::from(bin));
    }
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let candidates = [
        workspace_root.join("daemon/target/debug/mae-daemon"),
        workspace_root.join("daemon/target/release/mae-daemon"),
        workspace_root.join("target/debug/mae-daemon"),
        workspace_root.join("target/release/mae-daemon"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

/// Spawn mae-daemon with given args. Uses pre-built binary or falls back to cargo run.
#[allow(dead_code)]
pub fn spawn_daemon(args: &[&str]) -> tokio::process::Child {
    if let Some(bin) = find_daemon_binary() {
        Command::new(bin)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn mae-daemon")
    } else {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        Command::new("cargo")
            .args(
                std::iter::once("run")
                    .chain(std::iter::once("--"))
                    .chain(args.iter().copied()),
            )
            .current_dir(workspace_root.join("daemon"))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("failed to spawn mae-daemon via cargo run in daemon/")
    }
}

/// Spawn mae-daemon on a random port, wait for it to listen, return (child, port).
///
/// Uses `MAE_DAEMON_BIN` env var if set (pre-built binary), otherwise
/// falls back to `cargo run` (works in CI but deadlocks when `cargo test` holds
/// the workspace lock).
#[allow(dead_code)]
pub async fn spawn_server() -> (tokio::process::Child, String, tempfile::TempDir) {
    // Find a free port.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let addr = format!("127.0.0.1:{}", port);

    // Use a temp data dir to avoid recovering stale documents from previous runs,
    // which can cause >5s startup times and flaky test failures.
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_str().unwrap().to_string();

    let child = spawn_daemon(&["--bind", &addr, "--data-dir", &data_dir]);

    // Wait for server to accept connections.
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if TcpStream::connect(&addr).await.is_ok() {
            return (child, addr, tmp);
        }
    }
    panic!("mae-daemon did not start within 5s on {}", addr);
}

#[allow(dead_code)]
pub fn should_run() -> bool {
    std::env::var("MAE_TCP_E2E").is_ok()
}

/// Helper: realistic org-mode body for testing.
#[allow(dead_code)]
pub fn realistic_org_body() -> &'static str {
    ":PROPERTIES:\n:ID: test-node-001\n:ROAM_REFS: https://example.com\n:END:\n\
     #+TITLE: Test Node — CRDT Round-Trip\n#+FILETAGS: :research:crdt:\n\n\
     * Overview\nThis node tests the full round-trip: SQLite → KbNodeDoc → base64 → server → base64 → KbNodeDoc → SQLite.\n\n\
     ** Sub-heading with [[id:other-node][internal link]]\n\
     Content with Unicode: café, naïve, 日本語\n\n\
     #+begin_src rust\nfn main() { println!(\"hello\"); }\n#+end_src\n"
}

/// Helper: create a test KB with N nodes and share it via a client.
/// Returns (kb_id, collection_state, node_states).
#[allow(dead_code)]
pub fn make_test_kb(
    _kb_id: &str,
    name: &str,
    creator: &str,
    nodes: &[(&str, &str, &str, &[&str])], // (id, title, body, tags)
) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let mut coll = KbCollectionDoc::new(name, creator);
    let mut node_states = Vec::new();
    for (id, title, body, tags) in nodes {
        let tag_strings: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        let node = KbNodeDoc::new(id, title, body, &tag_strings);
        coll.add_node(id, title);
        node_states.push((id.to_string(), node.encode()));
    }
    (coll.encode_state(), node_states)
}
