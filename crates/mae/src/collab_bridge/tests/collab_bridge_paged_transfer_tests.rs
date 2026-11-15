//! ADR-022 Amendment 1, client side: a paged join folds into one `KbJoined`, a
//! batched share reports once with the whole share's count, and the request
//! cap comes from the daemon's `initialize` answer.

use super::*;

fn node(id: &str) -> serde_json::Value {
    let state = mae_sync::kb::KbNodeDoc::new(id, id, "body", &[]).encode_state();
    serde_json::json!({"id": id, "state": mae_sync::encoding::update_to_base64(&state)})
}

fn page(id: u64, nodes: &[&str], next_after: Option<&str>) -> serde_json::Value {
    let coll = mae_sync::kb::KbCollectionDoc::new_owned("kp", "", "");
    let mut v = serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {
        "kb_id": "kp",
        "collection_state": mae_sync::encoding::update_to_base64(&coll.encode_state()),
        "nodes": nodes.iter().map(|n| node(n)).collect::<Vec<_>>(),
    }});
    if let Some(n) = next_after {
        v["result"]["next_after"] = serde_json::json!(n);
    }
    v
}

/// Run one response through `handle_response` with a fresh context; the
/// follow-ups it queued.
fn respond(
    val: &serde_json::Value,
    kind: PendingResponseKind,
    tx: &mpsc::Sender<CollabEvent>,
) -> Vec<JoinFollowUp> {
    let mut pages = Vec::new();
    let mut shared = Vec::new();
    handle_response(
        val,
        kind,
        tx,
        &mut shared,
        &mut std::collections::HashMap::new(),
        &mut KbCryptoCtx {
            content_keys: &mut std::collections::HashMap::new(),
            op_sets: &mut std::collections::HashMap::new(),
            node_to_kb: &mut std::collections::HashMap::new(),
            seen_ops: &mut std::collections::HashMap::new(),
            kb_collections: &mut std::collections::HashMap::new(),
            signing_identity: None,
            pending_collection_ops: &mut Vec::new(),
            pending_join_pages: &mut pages,
        },
    );
    pages
}

#[tokio::test]
async fn a_paged_join_emits_one_kb_joined_with_every_page_in_order() {
    let (tx, mut rx) = mpsc::channel(8);
    let kb_id = "kp".to_string();
    let first = respond(
        &page(1, &["a", "b"], Some("b")),
        PendingResponseKind::KbJoin {
            kb_id: kb_id.clone(),
        },
        &tx,
    );
    assert!(
        rx.try_recv().is_err(),
        "nothing is reported before the last page"
    );
    let [next] = <[JoinFollowUp; 1]>::try_from(first).expect("one follow-up");
    assert_eq!(next.after, "b");

    let second = respond(
        &page(2, &["c"], Some("c")),
        PendingResponseKind::KbJoinPage {
            kb_id: kb_id.clone(),
            acc: next.acc,
        },
        &tx,
    );
    let [next] = <[JoinFollowUp; 1]>::try_from(second).expect("one follow-up");
    assert!(rx.try_recv().is_err());

    let last = respond(
        &page(3, &["d"], None),
        PendingResponseKind::KbJoinPage {
            kb_id,
            acc: next.acc,
        },
        &tx,
    );
    assert!(last.is_empty(), "the last page queues nothing");
    match rx.try_recv().unwrap() {
        CollabEvent::KbJoined { nodes, .. } => {
            let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
            assert_eq!(ids, ["a", "b", "c", "d"]);
        }
        other => panic!("expected KbJoined, got {other:?}"),
    }
}

#[tokio::test]
async fn a_refusal_on_a_later_page_reports_the_join_as_denied() {
    let (tx, mut rx) = mpsc::channel(8);
    let denied = serde_json::json!({"jsonrpc": "2.0", "id": 2,
        "error": {"code": -32603, "message": "not a member of KB 'kp'"}});
    let acc = vec![node("a")];
    let q = respond(
        &denied,
        PendingResponseKind::KbJoinPage {
            kb_id: "kp".into(),
            acc,
        },
        &tx,
    );
    assert!(q.is_empty());
    match rx.try_recv().unwrap() {
        CollabEvent::Error { message } => assert!(message.contains("join denied"), "{message}"),
        other => panic!("expected the join's denial, got {other:?}"),
    }
}

#[tokio::test]
async fn a_batched_share_reports_once_with_the_whole_count() {
    let (tx, mut rx) = mpsc::channel(8);
    let ok = |id: u64, n: u64| {
        serde_json::json!({"jsonrpc": "2.0", "id": id,
        "result": {"kb_id": "kp", "shared": true, "node_count": n, "collection_state": ""}})
    };
    respond(
        &ok(1, 3),
        PendingResponseKind::KbShareBatch {
            kb_id: "kp".into(),
            last: None,
        },
        &tx,
    );
    assert!(rx.try_recv().is_err(), "an earlier batch reports nothing");
    respond(
        &ok(2, 2),
        PendingResponseKind::KbShareBatch {
            kb_id: "kp".into(),
            last: Some(5),
        },
        &tx,
    );
    match rx.try_recv().unwrap() {
        CollabEvent::KbShared { node_count, .. } => assert_eq!(node_count, 5),
        other => panic!("expected KbShared, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failed_earlier_share_batch_is_reported() {
    let (tx, mut rx) = mpsc::channel(8);
    let err = serde_json::json!({"jsonrpc": "2.0", "id": 1,
        "error": {"code": -32600, "message": "too large", "data": {"kind": "request_too_large"}}});
    respond(
        &err,
        PendingResponseKind::KbShareBatch {
            kb_id: "kp".into(),
            last: None,
        },
        &tx,
    );
    match rx.try_recv().unwrap() {
        CollabEvent::Error { message } => {
            assert!(message.contains("Failed to share KB"), "{message}")
        }
        other => panic!("expected an error, got {other:?}"),
    }
}

#[test]
fn the_request_cap_comes_from_initialize_or_defaults_to_ten_mib() {
    let with = serde_json::json!({"result": {"limits": {"max_frame_bytes": 15_029_592}}});
    assert_eq!(daemon_request_cap_of(Some(&with)), 15_029_592);
    let without = serde_json::json!({"result": {"serverInfo": {}}});
    assert_eq!(
        daemon_request_cap_of(Some(&without)),
        mae_mcp::MAX_MESSAGE_SIZE
    );
    assert_eq!(daemon_request_cap_of(None), mae_mcp::MAX_MESSAGE_SIZE);
}
