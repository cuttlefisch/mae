//! ADVERSARIAL: a node document is authorized as part of the KB it belongs
//! to, never the KB the caller names.
//!
//! `sync/update` checks membership, signature and epoch against the
//! request's `kb_id` and writes to the request's `doc`. When the two named
//! different KBs, a caller cleared for one KB (say, one it owns) wrote a node
//! of another.

use super::*;

const OWNER: &str = "alice";
const STRANGER: &str = "mallory";

async fn seed(store: &Arc<DocStore>, bc: &SharedBroadcaster) {
    let mut docs = HashSet::new();
    kb_share_as(
        store,
        bc,
        Some(OWNER),
        Some(&fp(OWNER)),
        "ek",
        OWNER,
        &mut docs,
    )
    .await;
    let added = dispatch_as(
        store,
        bc,
        Some(OWNER),
        Some(&fp(OWNER)),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
            "params":{"kb_id":"ek","node_id":"concept:n","title":"n"}}),
        &mut docs,
    )
    .await;
    assert!(added.error.is_none(), "{:?}", added.error);
    let body = make_test_node("concept:n", "n", "body", &[]);
    let wrote = dispatch_as(
        store,
        bc,
        Some(OWNER),
        Some(&fp(OWNER)),
        serde_json::json!({"jsonrpc":"2.0","id":2,"method":"sync/update",
            "params":{"doc":"kbn:ek:concept:n","kb_id":"ek","update":update_to_base64(&body)}}),
        &mut docs,
    )
    .await;
    assert!(wrote.error.is_none(), "{:?}", wrote.error);
    // The stranger owns a KB of their own, so they clear every check for it.
    let mut mine = HashSet::new();
    kb_share_as(
        store,
        bc,
        Some(STRANGER),
        Some(&fp(STRANGER)),
        "theirs",
        STRANGER,
        &mut mine,
    )
    .await;
}

#[tokio::test]
async fn a_write_naming_another_kb_cannot_reach_this_kbs_node() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    seed(&store, &bc).await;
    let before = store.encode_state("kbn:ek:concept:n").await.unwrap();

    let forged = make_test_node("concept:n", "n", "WRITTEN-BY-A-STRANGER", &[]);
    let mut docs = HashSet::new();
    let reply = dispatch_as(
        &store,
        &bc,
        Some(STRANGER),
        Some(&fp(STRANGER)),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"sync/update",
            "params":{"doc":"kbn:ek:concept:n","kb_id":"theirs","update":update_to_base64(&forged)}}),
        &mut docs,
    )
    .await;
    let err = reply
        .error
        .as_ref()
        .unwrap_or_else(|| panic!("the write was answered: {:?}", reply.result));
    let data = err.data.as_deref().cloned().unwrap_or_default();
    assert_eq!(data["kind"], "doc_kb_mismatch", "{}", err.message);
    assert_eq!(
        store.encode_state("kbn:ek:concept:n").await.unwrap(),
        before
    );
}

/// The shared content-op check refuses a node doc of a different KB, so the
/// mesh path (which calls it with the ticket's KB) is bound the same way.
#[tokio::test]
async fn the_content_op_check_refuses_a_node_of_another_kb() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    seed(&store, &bc).await;
    let update = make_test_node("concept:n", "n", "x", &[]);
    let r =
        verify_relayed_content_op(&store, "theirs", "kbn:ek:concept:n", &update, None, false).await;
    assert_eq!(r.map_err(|e| e.kind), Err(RefusalKind::DocKbMismatch));
    let same =
        verify_relayed_content_op(&store, "ek", "kbn:ek:concept:n", &update, None, false).await;
    assert!(same.is_ok(), "{same:?}");
}

/// A refused write leaves the caller subscribed to nothing: before, a
/// non-member's refused `sync/update` still joined the node's live feed and
/// received every later update to it.
#[tokio::test]
async fn a_refused_write_does_not_subscribe_the_caller() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    seed(&store, &bc).await;
    let forged = make_test_node("concept:n", "n", "X", &[]);
    for kb_id in ["ek", "theirs"] {
        let mut docs = HashSet::new();
        let reply = dispatch_as(
            &store,
            &bc,
            Some(STRANGER),
            Some(&fp(STRANGER)),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"sync/update",
                "params":{"doc":"kbn:ek:concept:n","kb_id":kb_id,"update":update_to_base64(&forged)}}),
            &mut docs,
        )
        .await;
        assert!(
            reply.error.is_some(),
            "kb_id {kb_id}: the write was answered"
        );
        assert!(
            docs.is_empty(),
            "kb_id {kb_id}: a refused caller was subscribed to {docs:?}"
        );
    }
}
