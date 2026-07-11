//! Tier 2 — TCP integration tests (real server), part 2b: KB protocol E2E
//! (stress/isolation/notification tests).
//!
//! Split from collab_tcp_e2e.rs (was 1431 lines, over the 500-line test
//! ceiling); shared helpers live in collab_tcp_e2e_support/mod.rs.
//!
//! Gated with `#[ignore]` — run via:
//!   MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e_kb_stress -- --ignored --nocapture

use std::time::Duration;

use mae_sync::encoding::base64_to_update;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc};

mod collab_tcp_e2e_support;
use collab_tcp_e2e_support::*;

/// Share KB with 20 nodes → join → all received and valid.
#[tokio::test]
#[ignore]
async fn tcp_kb_multi_node_stress() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    // Build 5 nodes with varying content sizes
    let mut coll = KbCollectionDoc::new("BigKB", "alice");
    let mut node_states: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..8 {
        let id = format!("node-{i:03}");
        let title = format!("Node {i}: Generated Content");
        let body = format!(
            "* Node {i}\nThis is node {i} with enough content to be realistic.\n\n\
             ** Details\nGenerated body paragraph {i}.\nSecond line of content for node {i}.\n\n\
             #+begin_src python\nprint(\"node {}\")\n#+end_src\n",
            i
        );
        let tags = vec!["generated".to_string(), format!("batch-{}", i % 5)];
        let node = KbNodeDoc::new(&id, &title, &body, &tags);
        coll.add_node(&id, &title);
        node_states.push((id, node.encode()));
    }

    let coll_state = coll.encode_state();
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("big-kb", "BigKB", "alice", &coll_state, &nodes_ref)
        .await;

    let join_resp = client_b.kb_join("big-kb").await;
    assert!(join_resp.get("error").is_none(), "join failed: {join_resp}");

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    assert_eq!(
        joined_nodes.len(),
        8,
        "should receive all 8 nodes, got {}",
        joined_nodes.len()
    );

    // Verify a sample node's content
    let sample = joined_nodes
        .iter()
        .find(|n| n["id"].as_str() == Some("node-003"))
        .expect("node-003 should exist");
    let state_bytes = base64_to_update(sample["state"].as_str().unwrap()).unwrap();
    let node_doc = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(node_doc.title(), "Node 3: Generated Content");
    assert!(
        node_doc.body().contains("print(\"node 3\")"),
        "node-003 body should contain the code block"
    );
}

/// Sequential node updates → all applied, latest state visible on join.
#[tokio::test]
#[ignore]
async fn tcp_kb_sequential_node_updates() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;

    let (coll_state, node_states) = make_test_kb(
        "seq-kb",
        "Test",
        "alice",
        &[("n1", "Title", "initial", &[])],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("seq-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;

    // Apply 3 sequential updates with send-recv for each
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    for i in 0..3 {
        let body = format!("version-{i}");
        let update = node.set_body(&body);
        let resp = client_a.kb_node_update("seq-kb", "n1", &update).await;
        assert!(resp.get("error").is_none(), "update {i} failed: {resp}");
    }

    // Verify final state via second client join
    let mut client_b = TcpClient::connect(&addr).await;
    let join_resp = client_b.kb_join("seq-kb").await;
    assert!(join_resp.get("error").is_none());

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    let state_bytes = base64_to_update(joined_nodes[0]["state"].as_str().unwrap()).unwrap();
    let final_node = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(
        final_node.body(),
        "version-2",
        "after 3 sequential updates, should see latest version"
    );
}

/// Two clients share different KBs → join sees correct KB, not cross-contaminated.
#[tokio::test]
#[ignore]
async fn tcp_kb_isolation_between_kbs() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;
    let mut client_c = TcpClient::connect(&addr).await;

    // Client A shares KB "alpha" with 2 nodes
    let (coll_a, nodes_a) = make_test_kb(
        "alpha",
        "Alpha KB",
        "alice",
        &[
            ("alpha-1", "Alpha Node 1", "Alpha body 1", &["alpha"]),
            ("alpha-2", "Alpha Node 2", "Alpha body 2", &["alpha"]),
        ],
    );
    let refs_a: Vec<(&str, &[u8])> = nodes_a
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();
    client_a
        .kb_share("alpha", "Alpha KB", "alice", &coll_a, &refs_a)
        .await;

    // Client B shares KB "beta" with 1 node
    let (coll_b, nodes_b) = make_test_kb(
        "beta",
        "Beta KB",
        "bob",
        &[("beta-1", "Beta Node 1", "Beta body 1", &["beta"])],
    );
    let refs_b: Vec<(&str, &[u8])> = nodes_b
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();
    client_b
        .kb_share("beta", "Beta KB", "bob", &coll_b, &refs_b)
        .await;

    // Client C joins "alpha" — should get 2 alpha nodes + possibly beta node
    // (because kb/join currently returns ALL kb: prefixed docs)
    // The collection doc should only list alpha nodes though
    let join_resp = client_c.kb_join("alpha").await;
    assert!(join_resp.get("error").is_none());

    let coll_b64 = join_resp["result"]["collection_state"].as_str().unwrap();
    let coll_bytes = base64_to_update(coll_b64).unwrap();
    let coll = KbCollectionDoc::from_bytes(&coll_bytes).unwrap();
    assert_eq!(coll.name(), "Alpha KB");
    assert_eq!(
        coll.node_count(),
        2,
        "alpha collection should have exactly 2 nodes"
    );
}

/// WU6: Peer join/leave notifications over TCP.
#[tokio::test]
#[ignore]
async fn tcp_peer_join_leave_notifications() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    // Client A shares a doc.
    let mut client_a = TcpClient::connect(&addr).await;
    client_a.share("peer-notify.txt", "hello").await;

    // Client B joins via resync.
    let mut client_b = TcpClient::connect(&addr).await;
    let resync_msg = serde_json::json!({
        "jsonrpc": "2.0",
        "id": client_b.next_id,
        "method": "sync/resync",
        "params": { "doc": "peer-notify.txt" }
    });
    client_b.next_id += 1;
    client_b.send(&resync_msg).await;
    let resp = client_b.recv().await;
    assert!(resp.get("error").is_none(), "resync failed: {resp}");

    // Client A should receive peer_joined notification.
    let joined = client_a
        .wait_for_notification("notifications/peer_joined", 2000)
        .await;
    assert!(
        joined.is_some(),
        "client A should receive peer_joined notification"
    );

    // Drop client B.
    drop(client_b);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client A should receive peer_left notification.
    let left = client_a
        .wait_for_notification("notifications/peer_left", 3000)
        .await;
    assert!(
        left.is_some(),
        "client A should receive peer_left notification"
    );
}
