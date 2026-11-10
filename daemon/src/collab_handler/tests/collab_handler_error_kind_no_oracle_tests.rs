//! Principle #16 / no existence oracle: adding `error.data.kind` must not tell
//! a NON-member anything the old message text did not.
//!
//! A non-member asking about a node gets `not_member` — the SAME error, code,
//! message and data, byte for byte — whether the node is in the KB, is in no KB
//! at all, or is a node of another KB this caller can read. Only after the read
//! gate passes does `node_not_in_kb` exist, which the positive control below
//! proves (otherwise "every answer is identical" could hold vacuously, e.g. if
//! the stranger were refused for some unrelated reason).

use super::collab_handler_error_kind_tests::{
    expect_kind, fixture, rpc, update_as, Fixture, KB, NODE,
};
use super::collab_handler_error_kind_write_tests::{anchored, signed_op};
use super::*;
use mae_mcp::identity::Identity;

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

/// A captured signed op of `author` at `epoch`, replayed over `sync/update`.
fn replayed(author: &Identity, epoch: u64) -> serde_json::Value {
    let (upd, signed) = signed_op(author, epoch);
    rpc(
        "sync/update",
        serde_json::json!({
            "doc": mae_sync::kb_node_doc_name(KB, NODE),
            "kb_id": KB,
            "update": update_to_base64(&upd),
            "content_header": signed.header_params(),
        }),
    )
}

async fn set_member(f: &mut Fixture, owner: &str, method: &str, who: &str, role: Option<&str>) {
    let msg = kb_member_msg(method, KB, who, role);
    let r = f.call_as(Some(owner), msg).await;
    assert!(r.error.is_none(), "{method} {who}: {:?}", r.error);
}

/// The fence epoch `kb/join` reports for `who`.
async fn epoch_of(f: &mut Fixture, who: &str) -> u64 {
    let r = f.call_as(Some(who), kb_join_msg(KB)).await;
    let epoch = r.result.as_ref().and_then(|v| v["epoch"].as_u64());
    epoch.unwrap_or_else(|| panic!("join {who}: {:?}", r.error))
}

/// Three captured ops the author checks refuse, each for a different fact
/// about its AUTHOR: removed since it signed, a viewer, and an epoch that
/// moved (a role change) since it signed. Real identities, real signed log.
async fn captured_ops(f: &mut Fixture, owner: &str) -> [serde_json::Value; 3] {
    let removed = Identity::generate("removed");
    let r = removed.fingerprint();
    set_member(f, owner, "kb/add_member", &r, Some("editor")).await;
    let removed_op = replayed(&removed, epoch_of(f, &r).await);
    set_member(f, owner, "kb/remove_member", &r, None).await;

    let viewer = Identity::generate("viewer");
    let v = viewer.fingerprint();
    set_member(f, owner, "kb/add_member", &v, Some("viewer")).await;
    let viewer_op = replayed(&viewer, epoch_of(f, &v).await);

    let stale = Identity::generate("stale");
    let s = stale.fingerprint();
    set_member(f, owner, "kb/add_member", &s, Some("editor")).await;
    let stale_op = replayed(&stale, epoch_of(f, &s).await);
    set_member(f, owner, "kb/add_member", &s, Some("viewer")).await;
    set_member(f, owner, "kb/add_member", &s, Some("editor")).await;
    [removed_op, viewer_op, stale_op]
}

/// The error `data` a member relaying each op gets.
async fn member_answers(
    f: &mut Fixture,
    member: &str,
    ops: &[serde_json::Value],
) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    for op in ops {
        let resp = f.call_as(Some(member), op.clone()).await;
        out.push(serialized(&resp));
    }
    out
}

/// On `sync/update` the CALLER's own gate runs before the op's author is
/// examined, so a non-member replaying someone's captured signed op gets one
/// refusal — code, message and data — whoever signed it and whatever their
/// standing. Before, it was told the author had been removed (`not_member`),
/// was a viewer (`role_forbidden`), or was fenced at a named epoch
/// (`epoch_fenced {current_epoch, caller_epoch}`).
#[tokio::test]
async fn a_non_member_replaying_a_captured_op_learns_nothing_about_its_author() {
    let (mut f, owner, editor) = anchored().await;
    let ops = captured_ops(&mut f, &owner.fingerprint()).await;
    let doc = mae_sync::kb_node_doc_name(KB, NODE);
    let before = f.store.encode_state(&doc).await.ok();

    let stranger = Identity::generate("stranger").fingerprint();
    let mut answers = Vec::new();
    for op in &ops {
        let resp = f.call_as(Some(&stranger), op.clone()).await;
        answers.push(serialized(&resp));
    }
    assert!(
        answers.windows(2).all(|w| w[0] == w[1]),
        "a non-member's answers differ by the op's author, which is an oracle: {answers:#?}"
    );
    assert_eq!(answers[0]["data"]["kind"], "not_member", "{answers:#?}");
    assert_eq!(
        f.store.encode_state(&doc).await.ok(),
        before,
        "nothing written"
    );

    // Positive control: a member relaying the same ops IS told about each
    // author, so the identical answers above come from the caller's gate and
    // not from three ops that happen to fail alike.
    let got = member_answers(&mut f, &editor.fingerprint(), &ops).await;
    let data: Vec<&serde_json::Value> = got.iter().map(|e| &e["data"]).collect();
    assert_eq!(data[0]["kind"], "not_member", "{got:#?}");
    assert_ne!(got[0]["message"], answers[0]["message"], "names the author");
    assert_eq!(data[1]["kind"], "role_forbidden", "{got:#?}");
    assert_eq!(data[1]["required"], "editor", "{got:#?}");
    assert_eq!(data[2]["kind"], "epoch_fenced", "{got:#?}");
    let (caller, current) = (
        data[2]["caller_epoch"].as_u64(),
        data[2]["current_epoch"].as_u64(),
    );
    assert!(caller.unwrap() < current.unwrap(), "{got:#?}");
}
