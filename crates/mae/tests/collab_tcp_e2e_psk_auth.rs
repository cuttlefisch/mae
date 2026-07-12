//! Tier 2 — TCP integration tests (real server), part 3: PSK authentication.
//!
//! Split from collab_tcp_e2e.rs (was 1431 lines, over the 500-line test
//! ceiling); shared helpers live in collab_tcp_e2e_support/mod.rs.
//!
//! Gated with `#[ignore]` — run via:
//!   MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e_psk_auth -- --ignored --nocapture

use std::time::Duration;

use mae_sync::encoding::base64_to_update;
use mae_sync::kb::KbNodeDoc;
use tokio::io::BufReader;
use tokio::net::TcpStream;

mod collab_tcp_e2e_support;
use collab_tcp_e2e_support::*;

// ============================================================================
// PSK Authentication Tests
// ============================================================================

/// Spawn a PSK-enabled server: writes a temp config with auth.mode = "psk".
async fn spawn_psk_server(psk: &str) -> (tokio::process::Child, String, tempfile::TempDir) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let addr = format!("127.0.0.1:{}", port);

    let tmp = tempfile::tempdir().unwrap();
    let config_path = tmp.path().join("daemon.toml");
    let data_dir = tmp.path().join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(
        &config_path,
        format!(
            r#"data_dir = "{data_dir}"

[collab]
bind = "{addr}"

[collab.auth]
mode = "psk"
psk = "{psk}"

[collab.storage]
data_dir = "{data_dir}"
"#,
            data_dir = data_dir.display()
        ),
    )
    .unwrap();

    let child = spawn_daemon(&["--config", config_path.to_str().unwrap()]);

    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if TcpStream::connect(&addr).await.is_ok() {
            return (child, addr, tmp);
        }
    }
    panic!("PSK mae-daemon did not start within 5s on {}", addr);
}

/// TcpClient with PSK auth: performs client_handshake before initialize.
impl TcpClient {
    async fn connect_with_psk(addr: &str, psk: &str) -> Self {
        let stream = TcpStream::connect(addr).await.expect("failed to connect");
        let (read, write) = stream.into_split();
        let mut client = TcpClient {
            reader: BufReader::new(read),
            writer: write,
            next_id: 1,
        };

        // Perform PSK handshake before initialize.
        use mae_mcp::auth::{AuthProvider, PskAuth};
        let auth = PskAuth::new(psk);
        auth.client_handshake(&mut client.reader, &mut client.writer)
            .await
            .expect("PSK client handshake failed");

        client.initialize().await;
        client.subscribe().await;
        client
    }
}

/// PSK auth: correct key connects and can share/read documents.
#[tokio::test]
#[ignore]
async fn tcp_psk_correct_key_connects() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_psk_server("e2e-test-secret").await;

    let mut client = TcpClient::connect_with_psk(&addr, "e2e-test-secret").await;
    client.share("psk-test.txt", "hello from PSK").await;
    assert_eq!(client.content("psk-test.txt").await, "hello from PSK");
}

/// PSK auth: wrong key is rejected — connection fails.
#[tokio::test]
#[ignore]
async fn tcp_psk_wrong_key_rejected() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_psk_server("correct-key").await;

    let stream = TcpStream::connect(&addr).await.expect("TCP connect");
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut writer = write;

    use mae_mcp::auth::{AuthProvider, PskAuth};
    let wrong_auth = PskAuth::new("wrong-key");
    let result = wrong_auth.client_handshake(&mut reader, &mut writer).await;

    assert!(result.is_err(), "wrong PSK should be rejected");
}

/// PSK auth: no-auth client to PSK server is rejected (server expects hello, gets JSON-RPC).
#[tokio::test]
#[ignore]
async fn tcp_psk_no_auth_client_rejected() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_psk_server("server-key").await;

    // Try connecting without PSK handshake — send initialize directly.
    let stream = TcpStream::connect(&addr).await.expect("TCP connect");
    let (read, write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut writer = write;

    // send_initialize sends JSON-RPC — server expecting PSK hello will fail/hang.
    let result = tokio::time::timeout(Duration::from_secs(3), async {
        let init = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}});
        let body = serde_json::to_vec(&init).unwrap();
        if mae_mcp::write_framed(&mut writer, &body, Duration::from_secs(2))
            .await
            .is_err()
        {
            return false;
        }
        // Try to read response — server should either close connection or send auth error.
        match tokio::time::timeout(Duration::from_secs(2), mae_mcp::read_message(&mut reader)).await
        {
            Ok(Ok(Some(text))) => {
                // If server responds, it shouldn't be a valid initialize response.
                let val: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
                val.get("result")
                    .and_then(|r| r.get("serverInfo"))
                    .is_some()
            }
            _ => false,
        }
    })
    .await;

    match result {
        Ok(got_valid_init) => {
            assert!(
                !got_valid_init,
                "no-auth client should NOT get valid initialize from PSK server"
            );
        }
        Err(_) => {
            // Timeout is expected — server dropped the connection or is waiting for auth hello.
        }
    }
}

/// Join `kb_id` and return the raw state bytes for `node_id` from the response.
async fn join_node_state(client: &mut TcpClient, kb_id: &str, node_id: &str) -> Vec<u8> {
    let resp = client.kb_join(kb_id).await;
    assert!(resp.get("error").is_none(), "join failed: {resp}");
    let nodes = resp["result"]["nodes"]
        .as_array()
        .expect("nodes should be an array");
    let n = nodes
        .iter()
        .find(|n| n["id"].as_str() == Some(node_id))
        .unwrap_or_else(|| panic!("node {node_id} not in join response: {resp}"));
    base64_to_update(n["state"].as_str().unwrap()).unwrap()
}

/// A5 — concurrent convergence over a REAL daemon (T1/T4 cross-ref). Two peers
/// concurrently edit DISJOINT fields of the same KB node (owner→title,
/// peer→body) derived from the same base. The daemon merges both into its
/// authoritative per-node doc, and two fresh joiners read back a BYTE-IDENTICAL
/// state carrying BOTH edits — the CRDT guarantee (CLAUDE.md #11) end-to-end
/// through TCP framing + base64 + the daemon's authoritative doc, not just an
/// in-process `KnowledgeBase` merge (kb_sync_n_peer_e2e.rs covers those).
#[tokio::test]
#[ignore]
async fn tcp_kb_two_peers_concurrent_converge() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut owner = TcpClient::connect(&addr).await;
    let mut peer = TcpClient::connect(&addr).await;

    // Owner shares a KB with one node.
    let (coll_state, node_states) = make_test_kb(
        "converge-kb",
        "Converge",
        "alice",
        &[("n1", "Base Title", "base body", &["t"])],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();
    owner
        .kb_share("converge-kb", "Converge", "alice", &coll_state, &nodes_ref)
        .await;

    // Peer joins (membership + subscription).
    let join = peer.kb_join("converge-kb").await;
    assert!(join.get("error").is_none(), "peer join failed: {join}");

    // CONCURRENT, disjoint-field edits derived from the SAME base node state.
    let mut node_owner = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update_title = node_owner.set_title("Alice Title");
    let mut node_peer = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update_body = node_peer.set_body("Bob Body");

    let r1 = owner
        .kb_node_update("converge-kb", "n1", &update_title)
        .await;
    assert!(r1.get("error").is_none(), "owner title update failed: {r1}");
    let r2 = peer.kb_node_update("converge-kb", "n1", &update_body).await;
    assert!(
        r2.get("error").is_none(),
        "peer body update failed (fenced?): {r2}"
    );

    // Two fresh joiners read the merged authoritative state.
    let mut c = TcpClient::connect(&addr).await;
    let mut d = TcpClient::connect(&addr).await;
    let state_c = join_node_state(&mut c, "converge-kb", "n1").await;
    let state_d = join_node_state(&mut d, "converge-kb", "n1").await;

    // Byte-identical authoritative convergence.
    assert_eq!(
        state_c, state_d,
        "two fresh joiners must receive byte-identical authoritative node state"
    );

    // Both concurrent edits survived the CRDT merge.
    let merged = KbNodeDoc::from_bytes(&state_c).unwrap();
    assert_eq!(
        merged.title(),
        "Alice Title",
        "owner's concurrent title edit converged"
    );
    assert_eq!(
        merged.body(),
        "Bob Body",
        "peer's concurrent body edit converged"
    );
}
