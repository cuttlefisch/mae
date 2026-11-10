//! Principle #16 / no existence oracle: adding `error.data.kind` must not tell
//! a NON-member anything the old message text did not.
//!
//! A non-member asking about a node gets `not_member` — the SAME error, code,
//! message and data, byte for byte — whether the node is in the KB, is in no KB
//! at all, or is a node of another KB this caller can read. Only after the read
//! gate passes does `node_not_in_kb` exist, which the positive control below
//! proves (otherwise "every answer is identical" could hold vacuously, e.g. if
//! the stranger were refused for some unrelated reason).

use super::collab_handler_error_kind_tests::{expect_kind, fixture, rpc, update_as, KB, NODE};
use super::*;

const THEIRS: &str = "theirs";
const THEIR_NODE: &str = "concept:theirs";

/// The stranger owns a KB of its own with a node in it, so "a node that exists
/// somewhere on this daemon" is a real third probe, not a hypothetical.
async fn stranger_owns_a_kb(f: &mut super::collab_handler_error_kind_tests::Fixture) {
    let me = fp("stranger");
    let s = kb_share_as(
        &f.store,
        &f.bc,
        Some("stranger"),
        Some(&me),
        THEIRS,
        "stranger",
        &mut f.docs,
    )
    .await;
    assert!(s.error.is_none(), "{:?}", s.error);
    let add = rpc(
        "kb/collection_node_add",
        serde_json::json!({"kb_id": THEIRS, "node_id": THEIR_NODE, "title": "t"}),
    );
    assert!(f.call(Some("stranger"), add).await.error.is_none());
}

/// The node-addressed methods on the collab listener, for one node id.
fn probes(node: &str) -> Vec<serde_json::Value> {
    let update = update_as(&fp("stranger"), 0, "probe");
    vec![
        rpc(
            "kb/node_fetch",
            serde_json::json!({"kb_id": KB, "node_id": node}),
        ),
        rpc(
            "kb/query.get",
            serde_json::json!({"kb_id": KB, "node_id": node}),
        ),
        rpc(
            "kb/node_update",
            serde_json::json!({"kb_id": KB, "node_id": node, "update": update}),
        ),
    ]
}

fn serialized(resp: &JsonRpcResponse) -> serde_json::Value {
    serde_json::to_value(resp.error.as_ref().expect("refused")).unwrap()
}

#[tokio::test]
async fn a_non_member_cannot_tell_whether_a_node_exists() {
    let mut f = fixture().await;
    stranger_owns_a_kb(&mut f).await;
    let nodes = [NODE, "concept:absent", THEIR_NODE];
    for i in 0..probes(NODE).len() {
        let mut answers = Vec::new();
        for node in nodes {
            let resp = f.call(Some("stranger"), probes(node).remove(i)).await;
            expect_kind(&resp, "not_member", &format!("probe {i} on {node}"));
            answers.push(serialized(&resp));
        }
        assert!(
            answers.windows(2).all(|w| w[0] == w[1]),
            "probe {i}: a non-member's answers differ by node, which is an oracle: {answers:#?}"
        );
    }
    // The raw document path names no KB gate at all: the same refusal for every
    // node document, whether or not it exists.
    let mut raw = Vec::new();
    for node in nodes {
        let doc = mae_sync::kb_node_doc_name(KB, node);
        let resp = f
            .call(
                Some("stranger"),
                rpc("sync/full_state", serde_json::json!({"doc": doc})),
            )
            .await;
        expect_kind(&resp, "doc_unavailable", node);
        raw.push(serialized(&resp));
    }
    assert!(raw.windows(2).all(|w| w[0] == w[1]), "{raw:#?}");
}

/// The HTTPS listener's node-addressed methods (`kb_query::dispatch`), same
/// property: the refusal does not depend on the node.
#[tokio::test]
async fn a_non_member_cannot_tell_whether_a_node_exists_over_kb_query() {
    let mut f = fixture().await;
    stranger_owns_a_kb(&mut f).await;
    let limits = crate::kb_query::KbQueryLimits::default();
    let who = fp("stranger");
    for method in ["kb/query.get", "kb/query.links", "kb/query.neighborhood"] {
        let mut answers = Vec::new();
        for node in [NODE, "concept:absent", THEIR_NODE] {
            let params = serde_json::json!({"kb_id": KB, "node_id": node});
            let err = crate::kb_query::dispatch(method, &params, &f.store, Some(&who), limits)
                .await
                .expect_err(method);
            answers.push(serde_json::to_value(&err).unwrap());
        }
        assert_eq!(answers[0]["data"]["kind"], "not_member", "{method}");
        assert!(
            answers.windows(2).all(|w| w[0] == w[1]),
            "{method}: {answers:#?}"
        );
    }
}

/// Positive control: a MEMBER does learn the difference — so the identical
/// answers above come from the gate, not from every probe failing alike.
#[tokio::test]
async fn a_member_is_told_the_node_is_not_in_the_kb() {
    let mut f = fixture().await;
    stranger_owns_a_kb(&mut f).await;
    let fetch = |node: &str| {
        rpc(
            "kb/node_fetch",
            serde_json::json!({"kb_id": KB, "node_id": node}),
        )
    };
    let present = f.call(Some("viewer"), fetch(NODE)).await;
    assert!(present.error.is_none(), "{:?}", present.error);
    for node in ["concept:absent", THEIR_NODE] {
        let resp = f.call(Some("viewer"), fetch(node)).await;
        expect_kind(&resp, "node_not_in_kb", node);
    }
}
