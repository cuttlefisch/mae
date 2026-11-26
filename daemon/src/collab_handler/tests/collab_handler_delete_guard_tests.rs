//! ADR-113 E7 through real traffic: a node that holds a publication-review
//! revision, in any state, is never removed from its KB, on any write path,
//! and the refusal is the one the API fixtures pin. A node without one is
//! removed as before.

use super::collab_handler_history_tests::{collection_op, error_kind, shared_kb, Env, KB};
use super::*;
use crate::review::store::{insert_revision, Revision};

/// A revision of `node` in `state`, as a request freezes it.
async fn revision(env: &Env, node: &str, state: &str) {
    let rev = Revision {
        revision_id: format!("rv-{node}"),
        kb_id: KB.into(),
        node_id: node.into(),
        request_id: format!("rq-{node}"),
        doc_code: None,
        rev_no: None,
        source_sha256: "00".repeat(32),
        version: "00".repeat(16),
        cutoff_wal: 0,
        history_seq: 1,
        state: state.into(),
        doc_type: None,
        template_id: None,
        template_sha256: None,
        approval_record: "act".into(),
        replaces: None,
        created_by: fp("bob"),
        created_by_kind: "person".into(),
        created_at: 0,
        approved_at: None,
        in_force_since: None,
        ended_at: None,
    };
    env.store
        .review_write(move |c| insert_revision(c, &rev))
        .await
        .unwrap();
}

/// `method` (`kb/collection_node_add` or `_remove`) on `node`, as alice.
async fn node_op(env: &mut Env, method: &str, node: &str) -> JsonRpcResponse {
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,
        "params":{"kb_id":KB,"node_id":node,"title":node}});
    let alice = fp("alice");
    dispatch_as(
        &env.store,
        &env.bc,
        Some("alice"),
        Some(&alice),
        msg,
        &mut env.docs,
    )
    .await
}

async fn listed(env: &Env) -> Vec<String> {
    let state = env.store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    let c = KbCollectionDoc::from_bytes(&state).unwrap();
    c.list_nodes().into_iter().map(|(id, _)| id).collect()
}

/// The fixture's refusal data: `{kind, act, reason}`, exactly.
fn pinned() -> serde_json::Value {
    let path = format!(
        "{}/../docs/verification/fixtures/review-api/other_surface_refusals.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let f: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    f["refusals"][0]["response"]["error"]["data"].clone()
}

/// The hub's node removal and a raw collection update are both refused for a
/// node with a revision in any state, with the fixture's data, and leave it
/// listed; a node without one is removed. Kills: a guard on one method only,
/// a guard that reads only some states, one that refuses every removal, and a
/// refusal that does not take the write back.
#[tokio::test]
async fn a_node_with_a_revision_is_never_removed_from_its_kb() {
    for state in [
        "in_review",
        "withdrawn",
        "rejected",
        "in_force",
        "superseded",
    ] {
        let mut env = shared_kb().await;
        for n in ["kept", "free"] {
            assert!(node_op(&mut env, "kb/collection_node_add", n)
                .await
                .error
                .is_none());
        }
        revision(&env, "kept", state).await;

        let r = node_op(&mut env, "kb/collection_node_remove", "kept").await;
        assert_eq!(error_kind(&r), "invalid_transition", "{state}");
        assert_eq!(*r.error.unwrap().data.unwrap(), pinned(), "{state}");
        let r = collection_op(&mut env, |c| c.remove_node("kept")).await;
        assert_eq!(error_kind(&r), "invalid_transition", "{state}: raw update");
        assert!(listed(&env).await.contains(&"kept".to_string()), "{state}");

        let r = node_op(&mut env, "kb/collection_node_remove", "free").await;
        assert!(r.error.is_none(), "{state}: {:?}", r.error);
        assert!(!listed(&env).await.contains(&"free".to_string()));
    }
}

/// The guard is in the write itself: a mesh peer's update and a whole-state
/// `share_doc`/`restore_doc` dropping the node are each refused, and an update
/// that also adds a node is refused whole. Kills a guard on the handlers only,
/// and one on `apply_update` but not on a replace.
#[tokio::test]
async fn every_write_path_refuses_removing_a_node_with_a_revision_whole() {
    let mut env = shared_kb().await;
    assert!(node_op(&mut env, "kb/collection_node_add", "kept")
        .await
        .error
        .is_none());
    revision(&env, "kept", "in_force").await;
    let kbc = format!("kbc:{KB}");
    let state = env.store.encode_state(&kbc).await.unwrap();
    let dropped = || {
        let mut c = KbCollectionDoc::from_bytes(&state).unwrap();
        let _ = c.add_node("extra", "extra");
        let _ = c.remove_node("kept");
        c.encode_state()
    };
    let peer = crate::doc_editors::Attribution::MeshPeer("peer");
    let by = crate::doc_editors::Attribution::Principal("alice");
    let results = [
        env.store.apply_update(&kbc, &dropped(), None, peer).await,
        env.store.share_doc(&kbc, &dropped(), by).await,
        env.store.restore_doc(&kbc, &dropped(), by).await,
    ];
    for (i, r) in results.into_iter().enumerate() {
        let e = r
            .err()
            .unwrap_or_else(|| panic!("path {i} removed the node"));
        assert_eq!(
            storage_error(e).data.unwrap()["reason"],
            "has_revisions",
            "path {i}"
        );
    }
    let now = listed(&env).await;
    assert!(now.contains(&"kept".to_string()));
    assert!(!now.contains(&"extra".to_string()), "half-applied");
}
