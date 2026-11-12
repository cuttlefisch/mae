//! ADR-116 P3, the reads: `kb/history.list`, `get`, `diff` and `verify`,
//! against history recorded from real traffic, and against a `state.db` whose
//! rows someone with write access has changed.

use super::collab_handler_history_tests::{
    checkpoint, error_kind, retitle, shared_kb, Env, KB, NODE,
};
use super::*;

async fn call(
    env: &mut Env,
    who: &str,
    method: &str,
    params: serde_json::Value,
) -> JsonRpcResponse {
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,"params":params});
    dispatch_as(
        &env.store,
        &env.bc,
        None,
        Some(&fp(who)),
        msg,
        &mut env.docs,
    )
    .await
}

fn ok(r: &JsonRpcResponse) -> serde_json::Value {
    r.result.clone().unwrap_or_else(|| panic!("{:?}", r.error))
}

/// Three versions of NODE: "A" (alice), "B" (bob), "C" (carol); alice's
/// "D" is the live node.
async fn three_versions() -> Env {
    let mut env = shared_kb().await;
    for (who, title) in [("alice", "A"), ("bob", "B"), ("carol", "C"), ("alice", "D")] {
        retitle(&mut env, who, title).await;
    }
    env
}

fn tamper(env: &Env, set: &str, seq: u64) {
    env.backend.execute_for_test(&format!(
        "UPDATE kb_node_history SET {set} WHERE kb_id = '{KB}' AND node_id = '{NODE}' AND seq = {seq}"
    ));
}

/// `list` reports every version's record and no content; a viewer may read
/// it, a non-member may not.
#[tokio::test]
async fn list_reports_records_to_readers_only() {
    let mut env = three_versions().await;
    let add = kb_member_msg("kb/add_member", KB, &fp("vic"), Some("viewer"));
    let alice = fp("alice");
    let r = dispatch_as(
        &env.store,
        &env.bc,
        Some("alice"),
        Some(&alice),
        add,
        &mut env.docs,
    )
    .await;
    assert!(r.error.is_none());
    let v = ok(&call(
        &mut env,
        "vic",
        "kb/history.list",
        serde_json::json!({"kb_id":KB,"node_id":NODE}),
    )
    .await);
    let versions = v["versions"].as_array().unwrap();
    let seqs: Vec<u64> = versions
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [1, 2, 3]);
    assert!(versions
        .iter()
        .all(|e| e.get("content").is_none() && e["pruned"] == false));
    let r = call(
        &mut env,
        "dave",
        "kb/history.list",
        serde_json::json!({"kb_id":KB,"node_id":NODE}),
    )
    .await;
    assert!(r.error.is_some(), "a non-member reads no history");
}

/// `get` returns the content the version recorded, with the chain's status.
/// A row changed in `state.db` is reported: a changed summary breaks the
/// entry hash; a changed content is withheld rather than shown as genuine.
#[tokio::test]
async fn get_returns_content_and_reports_tampering() {
    let mut env = three_versions().await;
    let get = |seq: u64| serde_json::json!({"kb_id":KB,"node_id":NODE,"seq":seq});
    let v = ok(&call(&mut env, "bob", "kb/history.get", get(2)).await);
    assert_eq!(v["content"]["title"], "B");
    assert_eq!(v["verification"], serde_json::json!({"ok": true}));
    assert_eq!(
        error_kind(&call(&mut env, "bob", "kb/history.get", get(9)).await),
        "doc_not_found"
    );

    tamper(&env, "summary = 'rewritten'", 2);
    let v = ok(&call(&mut env, "bob", "kb/history.get", get(3)).await);
    assert_eq!(
        v["verification"],
        serde_json::json!({"ok": false, "seq": 2, "broken": "entry_hash"})
    );

    let other = mae_sync::kb::history::encode_content(
        &mae_sync::kb::KbNodeDoc::new_created_at(NODE, "forged", "", &[], 1).materialize(),
    );
    tamper(&env, &format!("content = X'{}'", hex::encode(other)), 1);
    let v = ok(&call(&mut env, "bob", "kb/history.get", get(1)).await);
    assert!(
        v["content"].is_null(),
        "a content that does not verify is not shown"
    );
}

/// `diff` names the fields that changed, with a line diff for text; it
/// refuses a pruned version and one that does not verify.
#[tokio::test]
async fn diff_names_the_changes_and_refuses_unverified_content() {
    let mut env = three_versions().await;
    let d = |from: u64, to: u64| serde_json::json!({"kb_id":KB,"node_id":NODE,"from":from,"to":to});
    let v = ok(&call(&mut env, "bob", "kb/history.diff", d(1, 3)).await);
    let changes = v["changes"].as_array().unwrap();
    let fields: Vec<&str> = changes
        .iter()
        .map(|c| c["field"].as_str().unwrap())
        .collect();
    assert_eq!(fields, ["title"]);
    let unified = changes[0]["change"]["unified"].as_str().unwrap();
    assert!(
        unified.contains("-A") && unified.contains("+C"),
        "{unified}"
    );
    let same = ok(&call(&mut env, "bob", "kb/history.diff", d(2, 2)).await);
    assert!(same["changes"].as_array().unwrap().is_empty());
    tamper(&env, "content = NULL", 1);
    assert_eq!(
        error_kind(&call(&mut env, "bob", "kb/history.diff", d(1, 3)).await),
        "invalid_params"
    );
    let forged = mae_sync::kb::history::encode_content(
        &mae_sync::kb::KbNodeDoc::new_created_at(NODE, "forged", "", &[], 1).materialize(),
    );
    tamper(&env, &format!("content = X'{}'", hex::encode(forged)), 2);
    assert_eq!(
        error_kind(&call(&mut env, "bob", "kb/history.diff", d(2, 3)).await),
        "invalid_params"
    );
}

/// `verify` over the whole KB reports each failing node and the KB's head; the
/// head moves with each new version. Without a head kept elsewhere, a removed
/// last version is not seen (`head_checked: false`), which is what the
/// signed head is for.
#[tokio::test]
async fn verify_reports_failures_and_the_head() {
    let mut env = three_versions().await;
    let whole = serde_json::json!({"kb_id": KB});
    let v = ok(&call(&mut env, "bob", "kb/history.verify", whole.clone()).await);
    assert_eq!(
        (
            v["ok"].as_bool(),
            v["nodes"].as_u64(),
            v["versions"].as_u64()
        ),
        (Some(true), Some(1), Some(3))
    );
    let head = v["head"].as_str().unwrap().to_string();
    let r = checkpoint(&mut env, "bob", NODE, "n").await;
    assert!(r.error.is_none());
    let v = ok(&call(&mut env, "bob", "kb/history.verify", whole.clone()).await);
    assert_ne!(
        v["head"].as_str().unwrap(),
        head,
        "the head follows the tips"
    );

    env.backend.execute_for_test(&format!(
        "DELETE FROM kb_node_history WHERE kb_id = '{KB}' AND seq = 2"
    ));
    let v = ok(&call(&mut env, "bob", "kb/history.verify", whole.clone()).await);
    assert_eq!(v["ok"], false);
    assert_eq!(
        v["failures"],
        serde_json::json!([{"node_id": NODE, "seq": 3, "broken": "seq"}])
    );

    let mut env = three_versions().await;
    env.backend.execute_for_test(&format!(
        "DELETE FROM kb_node_history WHERE kb_id = '{KB}' AND seq = 3"
    ));
    let v = ok(&call(&mut env, "bob", "kb/history.verify", whole).await);
    assert_eq!(
        (v["ok"].as_bool(), v["head_checked"].as_bool()),
        (Some(true), Some(false))
    );
}
