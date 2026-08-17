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

/// ADR-105 D5: the daemon's "that id is someone else's" refusal must be routed to
/// the RECOVERY path, and routed by its error CODE.
///
/// This covers the seam the recovery tests cannot: they dispatch
/// `KbShareIdConflict` directly, so they prove what happens once the event exists
/// and say nothing about whether it is ever emitted. Verified by mutation — with
/// the code comparison broken, those tests still pass and only this one fails.
///
/// Branching on the code rather than the message is the point. A recovery that
/// hinges on error prose staying byte-identical is a recovery that stops working
/// the first time someone rewords a string, and it fails by silently reverting to
/// "share failed forever".
#[tokio::test]
async fn kb_share_id_conflict_is_routed_by_code_not_message() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {
            "code": mae_mcp::protocol::KB_ID_OWNED_BY_ANOTHER,
            // Deliberately NOT the daemon's real wording: if this were routed by
            // message text, this test would fail — which is the property under test.
            "message": "some entirely different phrasing"
        }
    });
    handle_response(
        &val,
        PendingResponseKind::KbShare {
            kb_id: "contested".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    match rx.try_recv().expect("a conflict must produce an event") {
        CollabEvent::KbShareIdConflict { kb_id, detail } => {
            assert_eq!(kb_id, "contested");
            assert_eq!(detail, "some entirely different phrasing");
        }
        other => panic!(
            "a KB_ID_OWNED_BY_ANOTHER refusal must become KbShareIdConflict (the \
             recoverable path), got {other:?}"
        ),
    }
}

/// The control: every OTHER share failure must stay a plain error. Routing them
/// all to the recovery path would re-mint a KB's id on any transient failure,
/// which is the finding-A destruction this whole design is built to avoid.
#[tokio::test]
async fn an_ordinary_share_failure_is_not_treated_as_an_id_conflict() {
    let (tx, mut rx) = mpsc::channel(8);
    let mut shared = Vec::new();

    let val = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": -32603, "message": "failed to share collection doc: disk full" }
    });
    handle_response(
        &val,
        PendingResponseKind::KbShare {
            kb_id: "mine".to_string(),
        },
        &tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        kb_ctx!(),
    );
    match rx.try_recv().expect("a failure must produce an event") {
        CollabEvent::Error { message } => assert!(message.contains("disk full")),
        other => panic!(
            "an unrelated failure must NOT enter the re-mint path — that would change \
             a live KB's id on any transient error, got {other:?}"
        ),
    }
}
