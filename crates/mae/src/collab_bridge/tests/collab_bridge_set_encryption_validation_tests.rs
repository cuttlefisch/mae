//! #340 regression coverage: `kb-set-encryption`'s local validation failures (invalid
//! mode, not-owner, wrong governance, collection-decode failure, self-wrap failure, no
//! signing identity) used to be `warn!`-log-only — no status message, no notification,
//! no MCP-visible error. A demo of "let's turn on E2E" that silently no-ops looked
//! like nothing happened. Reuses the real-connection harness from
//! `collab_bridge_write_failure_tests.rs` (`spawn_fake_daemon`/`connect_client`/
//! `owned_e2e_collection_state`) since, like that file's bugs, these checks live
//! inline in `run_collab_task`'s match arms with no extractable pure-function seam.

use super::collab_bridge_write_failure_tests::{
    connect_client, owned_e2e_collection_state, recv_until, spawn_fake_daemon,
};
use super::*;
use mae_mcp::identity::Identity;
use std::time::Duration;

#[tokio::test]
async fn invalid_encryption_mode_reports_a_visible_error() {
    let client_id = Identity::from_seed(&[91u8; 32], "client");
    let server_id = Identity::from_seed(&[92u8; 32], "daemon");
    let client_pub = client_id.public();
    let server_pub = server_id.public();

    let (addr, _hangup_tx, _daemon_handle) = spawn_fake_daemon(server_id, client_pub).await;
    let (cmd_tx, mut evt_rx) = connect_client(addr, client_id, server_pub).await;

    cmd_tx
        .send(CollabCommand::KbSetEncryption {
            kb_id: "kb-mode-test".to_string(),
            mode: "plaintext-ish-typo".to_string(),
            collection_state: Vec::new(),
            node_states: Vec::new(),
        })
        .await
        .unwrap();

    let ev = tokio::time::timeout(
        Duration::from_secs(2),
        recv_until(&mut evt_rx, |e| matches!(e, CollabEvent::Error { .. })),
    )
    .await
    .expect("an invalid mode must produce a visible CollabEvent::Error, not silence");
    match ev {
        CollabEvent::Error { message } => {
            assert!(message.contains("kb-mode-test"));
            assert!(message.contains("plaintext-ish-typo"));
        }
        other => panic!("expected CollabEvent::Error, got {other:?}"),
    }
}

#[tokio::test]
async fn non_owner_set_encryption_reports_a_visible_error() {
    let client_id = Identity::from_seed(&[93u8; 32], "client");
    let true_owner = Identity::from_seed(&[94u8; 32], "someone-else");
    let server_id = Identity::from_seed(&[95u8; 32], "daemon");
    let client_pub = client_id.public();
    let server_pub = server_id.public();

    let (addr, _hangup_tx, _daemon_handle) = spawn_fake_daemon(server_id, client_pub).await;
    let (cmd_tx, mut evt_rx) = connect_client(addr, client_id, server_pub).await;

    // Collection genesis-owned by a DIFFERENT identity than the connecting client —
    // the client attempting kb-set-encryption is not the owner.
    let collection_state = owned_e2e_collection_state("kb-not-mine", &true_owner);
    cmd_tx
        .send(CollabCommand::KbSetEncryption {
            kb_id: "kb-not-mine".to_string(),
            mode: "e2e".to_string(),
            collection_state,
            node_states: Vec::new(),
        })
        .await
        .unwrap();

    let ev = tokio::time::timeout(
        Duration::from_secs(2),
        recv_until(&mut evt_rx, |e| matches!(e, CollabEvent::Error { .. })),
    )
    .await
    .expect("a non-owner set-encryption attempt must produce a visible error, not silence");
    match ev {
        CollabEvent::Error { message } => {
            assert!(message.contains("kb-not-mine"));
            assert!(message.to_lowercase().contains("owner"));
        }
        other => panic!("expected CollabEvent::Error, got {other:?}"),
    }
}
