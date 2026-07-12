//! Tier 2 — TCP integration tests (real server), part 2a: KB protocol E2E
//! (share/join/leave/update roundtrips).
//!
//! Split from collab_tcp_e2e.rs (was 1431 lines, over the 500-line test
//! ceiling); shared helpers live in collab_tcp_e2e_support/mod.rs.
//!
//! Gated with `#[ignore]` — run via:
//!   MAE_TCP_E2E=1 cargo test -p mae --test collab_tcp_e2e_kb_protocol -- --ignored --nocapture

use std::time::Duration;

use mae_sync::encoding::base64_to_update;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc};

mod collab_tcp_e2e_support;
use collab_tcp_e2e_support::*;

// ============================================================================
// KB Protocol E2E Tests
// ============================================================================

/// Share KB with 3 org-mode nodes → join from second client → content matches.
#[tokio::test]
#[ignore]
async fn tcp_kb_share_and_join_roundtrip() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let body = realistic_org_body();
    let nodes_spec: Vec<(&str, &str, &str, &[&str])> = vec![
        (
            "node-1",
            "Architecture Overview",
            body,
            &["research", "crdt"],
        ),
        (
            "node-2",
            "Buffer Management",
            "Ropey-backed buffer with CRDT sync.",
            &["core"],
        ),
        (
            "node-3",
            "Window System",
            "Tiled window manager with splits.",
            &["ui", "layout"],
        ),
    ];

    let (coll_state, node_states) = make_test_kb("test-kb-1", "Research", "alice", &nodes_spec);
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("test-kb-1", "Research", "alice", &coll_state, &nodes_ref)
        .await;

    // Client B joins
    let join_resp = client_b.kb_join("test-kb-1").await;
    assert!(
        join_resp.get("error").is_none(),
        "kb/join should succeed: {join_resp}"
    );

    let result = &join_resp["result"];
    let joined_nodes = result["nodes"].as_array().expect("nodes should be array");
    assert_eq!(
        joined_nodes.len(),
        3,
        "should receive all 3 nodes, got {}",
        joined_nodes.len()
    );

    // Verify collection state decodes correctly
    let coll_b64 = result["collection_state"].as_str().unwrap();
    let coll_bytes = base64_to_update(coll_b64).unwrap();
    let coll = KbCollectionDoc::from_bytes(&coll_bytes).unwrap();
    assert_eq!(coll.name(), "Research", "collection name should match");
    assert_eq!(coll.creator(), "alice", "collection creator should match");
    assert_eq!(
        coll.node_count(),
        3,
        "collection should list 3 nodes, got {}",
        coll.node_count()
    );

    // Verify each node's content
    for joined_node in joined_nodes {
        let node_id = joined_node["id"].as_str().unwrap();
        let state_b64 = joined_node["state"].as_str().unwrap();
        let state_bytes = base64_to_update(state_b64).unwrap();
        let node_doc = KbNodeDoc::from_bytes(&state_bytes).unwrap();

        match node_id {
            "node-1" => {
                assert_eq!(node_doc.title(), "Architecture Overview");
                assert_eq!(
                    node_doc.body(),
                    body,
                    "node-1 body should be byte-for-byte identical to original org content (got {} bytes, expected {} bytes)",
                    node_doc.body().len(),
                    body.len()
                );
                assert_eq!(node_doc.tags(), vec!["research", "crdt"]);
            }
            "node-2" => {
                assert_eq!(node_doc.title(), "Buffer Management");
                assert_eq!(node_doc.body(), "Ropey-backed buffer with CRDT sync.");
            }
            "node-3" => {
                assert_eq!(node_doc.title(), "Window System");
                assert_eq!(node_doc.body(), "Tiled window manager with splits.");
            }
            other => panic!("unexpected node_id: {other}"),
        }
    }
}

/// kb/node_update from client A → client B receives notification with correct bytes.
#[tokio::test]
#[ignore]
async fn tcp_kb_node_update_broadcasts() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    // Share a KB with 1 node
    let (coll_state, node_states) = make_test_kb(
        "update-kb",
        "Test",
        "alice",
        &[("n1", "Original Title", "Original body", &["tag1"])],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("update-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;

    // Client B joins to subscribe to updates
    let join_resp = client_b.kb_join("update-kb").await;
    assert!(join_resp.get("error").is_none(), "join failed: {join_resp}");

    // Client A edits the node body
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update = node.set_body("Updated body from client A");

    let update_resp = client_a.kb_node_update("update-kb", "n1", &update).await;
    assert!(
        update_resp.get("error").is_none(),
        "node_update failed: {update_resp}"
    );

    // Client B should receive a sync_update notification for "kb:n1"
    let notif = client_b
        .wait_for_notification("notifications/sync_update", 5000)
        .await;
    assert!(
        notif.is_some(),
        "client B should receive sync_update notification for kb:n1"
    );

    let notif = notif.unwrap();
    let data = &notif["params"]["event"]["data"];
    assert_eq!(
        data["buffer_name"].as_str().unwrap(),
        "kb:n1",
        "notification should be for kb:n1, got: {}",
        notif
    );

    // Verify the update bytes decode to valid content
    let update_b64 = data["update_base64"].as_str().unwrap();
    let update_bytes = base64_to_update(update_b64).unwrap();
    let mut node_b = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    node_b.apply_update(&update_bytes).unwrap();
    assert_eq!(
        node_b.body(),
        "Updated body from client A",
        "applying update should produce correct body"
    );
}

/// kb/leave → server confirms leave, client can re-join to get latest state.
///
/// Note: doc-scoped broadcast filtering falls back to "deliver all" when a
/// client has zero doc_subs (by design — backward compatibility). So we
/// test the protocol response + re-join semantics rather than notification
/// absence (which requires the client to have other active doc subscriptions).
#[tokio::test]
#[ignore]
async fn tcp_kb_leave_and_rejoin() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let (coll_state, node_states) =
        make_test_kb("leave-kb", "Test", "alice", &[("n1", "Title", "Body", &[])]);
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("leave-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;
    let join_resp = client_b.kb_join("leave-kb").await;
    assert!(join_resp.get("error").is_none());

    // Client B leaves
    let leave_resp = client_b.kb_leave("leave-kb").await;
    assert_eq!(
        leave_resp["result"]["left"].as_bool(),
        Some(true),
        "leave should confirm success"
    );

    // Client A edits while B is away
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update = node.set_body("Edited while B was away");
    client_a.kb_node_update("leave-kb", "n1", &update).await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client B re-joins and should see the update
    let rejoin_resp = client_b.kb_join("leave-kb").await;
    assert!(
        rejoin_resp.get("error").is_none(),
        "rejoin failed: {rejoin_resp}"
    );

    let nodes = rejoin_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    assert_eq!(nodes.len(), 1);
    let state_bytes = base64_to_update(nodes[0]["state"].as_str().unwrap()).unwrap();
    let rejoined_node = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(
        rejoined_node.body(),
        "Edited while B was away",
        "after rejoin, client B should see edits made while away"
    );
}

/// Share → join → update → third client joins → sees latest content.
#[tokio::test]
#[ignore]
async fn tcp_kb_join_after_update_sees_latest() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let (coll_state, node_states) = make_test_kb(
        "latest-kb",
        "Test",
        "alice",
        &[("n1", "V1 Title", "V1 Body", &["v1"])],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();
    client_a
        .kb_share("latest-kb", "Test", "alice", &coll_state, &nodes_ref)
        .await;

    // Client A updates the node
    let mut node = KbNodeDoc::from_bytes(&node_states[0].1).unwrap();
    let update = node.set_body("V2 Body — updated content");
    client_a.kb_node_update("latest-kb", "n1", &update).await;

    // Small delay for server to apply
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Client B joins AFTER the update
    let join_resp = client_b.kb_join("latest-kb").await;
    assert!(join_resp.get("error").is_none(), "join failed: {join_resp}");

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    assert_eq!(joined_nodes.len(), 1);

    let state_b64 = joined_nodes[0]["state"].as_str().unwrap();
    let state_bytes = base64_to_update(state_b64).unwrap();
    let joined_node = KbNodeDoc::from_bytes(&state_bytes).unwrap();
    assert_eq!(
        joined_node.body(),
        "V2 Body — updated content",
        "third client should see updated content after joining"
    );
}

/// Realistic org content with properties drawers, links, code blocks, and Unicode
/// survives the full round-trip byte-for-byte.
#[tokio::test]
#[ignore]
async fn tcp_kb_realistic_org_content_roundtrip() {
    if !should_run() {
        return;
    }
    let (_server, addr, _tmp) = spawn_server().await;

    let mut client_a = TcpClient::connect(&addr).await;
    let mut client_b = TcpClient::connect(&addr).await;

    let body = realistic_org_body();
    let (coll_state, node_states) = make_test_kb(
        "org-kb",
        "OrgNotes",
        "alice",
        &[(
            "org-1",
            "Test Node — CRDT Round-Trip",
            body,
            &["research", "crdt"],
        )],
    );
    let nodes_ref: Vec<(&str, &[u8])> = node_states
        .iter()
        .map(|(id, s)| (id.as_str(), s.as_slice()))
        .collect();

    client_a
        .kb_share("org-kb", "OrgNotes", "alice", &coll_state, &nodes_ref)
        .await;

    let join_resp = client_b.kb_join("org-kb").await;
    assert!(join_resp.get("error").is_none());

    let joined_nodes = join_resp["result"]["nodes"]
        .as_array()
        .expect("nodes array");
    let state_b64 = joined_nodes[0]["state"].as_str().unwrap();
    let state_bytes = base64_to_update(state_b64).unwrap();
    let node_doc = KbNodeDoc::from_bytes(&state_bytes).unwrap();

    assert_eq!(
        node_doc.body(),
        body,
        "org body should survive round-trip byte-for-byte\nexpected {} bytes, got {} bytes",
        body.len(),
        node_doc.body().len()
    );
    assert_eq!(node_doc.title(), "Test Node — CRDT Round-Trip");
    assert_eq!(node_doc.tags(), vec!["research", "crdt"]);
}
