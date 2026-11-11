//! ADR-099 Amendment 1 B5: presence in the y-protocols awareness encoding.
//! A session speaks only for client ids it owns, every relayed entry carries
//! the sender's principal as the daemon knows it, and a malformed update is a
//! parameter error. Real y-protocols bytes, built with yrs' own encoder.

use super::*;
use mae_sync::yrs::sync::awareness::{AwarenessUpdate, AwarenessUpdateEntry};
use mae_sync::yrs::updates::encoder::Encode;

const DOC: &str = "shared:pad";

fn awareness(clients: &[(u64, &str)]) -> String {
    let mut u = AwarenessUpdate {
        clients: Default::default(),
    };
    for (id, json) in clients {
        u.clients.insert(
            mae_sync::yrs::block::ClientID::new(*id),
            AwarenessUpdateEntry {
                clock: 1,
                json: (*json).into(),
            },
        );
    }
    mae_sync::encoding::update_to_base64(&u.encode_v1())
}

async fn send(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    (session, principal): (u64, &str),
    update: serde_json::Value,
) -> JsonRpcResponse {
    let msg = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "sync/awareness",
        "params": {"doc": DOC, "update": update}});
    let mut docs = HashSet::new();
    handle_doc_request_inner(
        &msg.to_string(),
        store,
        &crate::quota::NoQuota,
        bc,
        std::time::Instant::now(),
        session,
        Some(principal),
        Some(principal),
        None,
        &mut docs,
        Transport::Hub,
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        None,
    )
    .await
}

fn kind(r: &JsonRpcResponse) -> String {
    r.error
        .as_ref()
        .and_then(|e| e.data.as_ref())
        .map(|d| d["kind"].as_str().unwrap_or("").to_string())
        .unwrap_or_default()
}

/// Kills: relaying without stamping, stamping what the client wrote, letting
/// one session speak for another's cursor, or keeping a claim after its
/// session ended.
#[tokio::test]
async fn a_session_speaks_only_for_its_own_cursors_and_is_labelled_by_the_daemon() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut watcher = {
        let mut b = bc.lock().unwrap();
        let rx = b.subscribe_doc_scoped(9, vec!["awareness_yjs".into()]);
        b.subscribe_doc(9, DOC);
        rx
    };
    let forged = r#"{"user":{"name":"mallory-as-alice"}}"#;
    let alice = awareness(&[(11, forged)]);
    let r = send(&store, &bc, (1, "SHA256:alice"), alice.clone().into()).await;
    assert!(r.error.is_none(), "{:?}", r.error);
    match watcher.try_recv().expect("relayed") {
        EditorEvent::AwarenessYjs {
            update_base64,
            principals,
            ..
        } => {
            assert_eq!(update_base64, alice, "relayed as sent");
            assert_eq!(
                principals.get("11").map(String::as_str),
                Some("SHA256:alice")
            );
        }
        other => panic!("expected AwarenessYjs, got {other:?}"),
    }

    // Bob may not speak for client 11, alone or mixed with his own.
    for ids in [&[(11, "{}")][..], &[(22, "{}"), (11, "{}")][..]] {
        let r = send(&store, &bc, (2, "SHA256:bob"), awareness(ids).into()).await;
        assert_eq!(kind(&r), "awareness_not_owned");
    }
    assert!(
        watcher.try_recv().is_err(),
        "nothing relayed for a refused update"
    );

    // Once alice's session ends, the id is free.
    bc.lock().unwrap().unsubscribe(1);
    let r = send(
        &store,
        &bc,
        (2, "SHA256:bob"),
        awareness(&[(11, "{}")]).into(),
    )
    .await;
    assert!(r.error.is_none(), "{:?}", r.error);
}

/// A malformed or oversized update is a parameter error and relays nothing.
#[tokio::test]
async fn a_malformed_presence_update_is_refused() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let many: Vec<(u64, &str)> = (0..17).map(|i| (i, "{}")).collect();
    for bad in [
        serde_json::json!("not base64 !!"),
        serde_json::json!(mae_sync::encoding::update_to_base64(&[0xff, 0xff, 0xff])),
        serde_json::json!(awareness(&many)),
        serde_json::json!(7),
    ] {
        let r = send(&store, &bc, (1, "SHA256:alice"), bad.clone()).await;
        assert_eq!(kind(&r), "invalid_params", "{bad}");
    }
}

/// The rate floor: a second update inside 50 ms is accepted and not relayed.
#[tokio::test]
async fn presence_faster_than_the_floor_is_dropped_not_refused() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut watcher = {
        let mut b = bc.lock().unwrap();
        let rx = b.subscribe_doc_scoped(9, vec!["awareness_yjs".into()]);
        b.subscribe_doc(9, DOC);
        rx
    };
    for _ in 0..2 {
        let r = send(
            &store,
            &bc,
            (1, "SHA256:alice"),
            awareness(&[(11, "{}")]).into(),
        )
        .await;
        assert!(r.error.is_none());
    }
    assert!(watcher.try_recv().is_ok());
    assert!(
        watcher.try_recv().is_err(),
        "the second, inside the floor, was relayed"
    );
}
