//! #339 regression coverage: `kb/approve_member` (legacy), `kb/set_policy`,
//! `kb/block_principal`/`unblock_principal`, and `kb/list_pending` previously sent a
//! request with an `id` but never registered it in `pending_responses` — the eventual
//! daemon reply (including a rejection: wrong role, bad fingerprint, not-owner) fell
//! into the generic "unknown/expired request id" fallback and was completely
//! invisible, not even `warn!`-logged. These tests drive `handle_response` directly
//! (the same lean pattern `collab_bridge_join_save_tests.rs` uses for `SaveIntent`) —
//! no fake daemon/socket needed, since the bug and the fix are both entirely in how a
//! JSON-RPC response is dispatched once received.

use super::*;

#[tokio::test]
async fn kb_approve_member_rejection_is_not_silent() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32000, "message": "not authorized: caller is not the KB owner" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbApproveMember {
            kb_id: "research".to_string(),
            principal: "SHA256:deadbeef".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx
        .try_recv()
        .expect("a rejection must produce a visible event, not silence");
    match event {
        CollabEvent::Error { message } => {
            assert!(message.contains("SHA256:deadbeef"));
            assert!(message.contains("research"));
            assert!(message.contains("not authorized"));
        }
        other => panic!("expected CollabEvent::Error, got {other:?}"),
    }
}

#[tokio::test]
async fn kb_approve_member_success_reports_status() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": { "ok": true } });
    handle_response(
        &val,
        PendingResponseKind::KbApproveMember {
            kb_id: "research".to_string(),
            principal: "SHA256:deadbeef".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx.try_recv().unwrap();
    match event {
        CollabEvent::StatusReport { lines } => {
            assert!(lines.iter().any(|l| l.contains("Approved")));
        }
        other => panic!("expected CollabEvent::StatusReport, got {other:?}"),
    }
}

#[tokio::test]
async fn kb_set_policy_rejection_is_not_silent() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32000, "message": "not authorized: caller is not the KB owner" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbSetPolicyResult {
            kb_id: "research".to_string(),
            policy: "permissive".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx
        .try_recv()
        .expect("a rejection must produce a visible event, not silence");
    match event {
        CollabEvent::Error { message } => {
            assert!(message.contains("permissive"));
            assert!(message.contains("research"));
            assert!(message.contains("not authorized"));
        }
        other => panic!("expected CollabEvent::Error, got {other:?}"),
    }
}

#[tokio::test]
async fn kb_block_principal_rejection_is_not_silent() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32000, "message": "unknown kb_id" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbBlockPrincipalResult {
            kb_id: "research".to_string(),
            principal: "SHA256:deadbeef".to_string(),
            block: true,
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx
        .try_recv()
        .expect("a rejection must produce a visible event, not silence");
    assert!(matches!(event, CollabEvent::Error { .. }));
}

#[tokio::test]
async fn kb_list_pending_rejection_is_not_silent() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32000, "message": "unknown kb_id" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbListPendingResult {
            kb_id: "research".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    let event = rx
        .try_recv()
        .expect("a rejection must produce a visible event, not silence");
    assert!(matches!(event, CollabEvent::Error { .. }));
}
