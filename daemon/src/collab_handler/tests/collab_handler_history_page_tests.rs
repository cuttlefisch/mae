//! ADR-116 D7: `kb/history.list` pages, newest first, against history cut by
//! real traffic. The oracle is the store's own unpaged read of the node.

use super::collab_handler_history_tests::{
    checkpoint, error_kind, retitle, shared_kb, Env, KB, NODE,
};
use super::*;
use crate::collab_handler::kb_history::{entry_json, LIST_DEFAULT_LIMIT, LIST_MAX_LIMIT};
use mae_sync::kb::history::{Editor, EditorKind, HistoryEntry, VersionKind};

async fn list(env: &mut Env, extra: serde_json::Value) -> JsonRpcResponse {
    let mut params = serde_json::json!({"kb_id": KB, "node_id": NODE});
    for (k, v) in extra.as_object().unwrap() {
        params[k] = v.clone();
    }
    let msg =
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/history.list","params":params});
    dispatch_as(
        &env.store,
        &env.bc,
        None,
        Some(&fp("bob")),
        msg,
        &mut env.docs,
    )
    .await
}

fn ok(r: &JsonRpcResponse) -> serde_json::Value {
    r.result.clone().unwrap_or_else(|| panic!("{:?}", r.error))
}

/// `(seq, kind)` of every version, newest first, from the store's unpaged read.
async fn oracle(env: &Env) -> Vec<(u64, String)> {
    let mut h: Vec<(u64, String)> = env
        .store
        .history_list(KB, NODE)
        .await
        .unwrap()
        .iter()
        .map(|e| (e.seq, e.kind.as_str().to_string()))
        .collect();
    h.reverse();
    h
}

/// Cut versions until the node has at least `n`: alice and bob take turns
/// (each handoff cuts an `edit`), and each round ends with a `named` one.
async fn at_least(n: usize) -> Env {
    let mut env = shared_kb().await;
    let mut round = 0;
    while env.store.history_list(KB, NODE).await.unwrap().len() < n {
        retitle(&mut env, "alice", &format!("a{round}")).await;
        retitle(&mut env, "bob", &format!("b{round}")).await;
        let r = checkpoint(&mut env, "bob", NODE, &format!("v{round}")).await;
        assert!(r.error.is_none(), "{:?}", r.error);
        round += 1;
        assert!(round < 1000, "versions are not being cut");
    }
    env
}

/// Follow `next` from `first`'s params to the end; the pages, in order.
async fn pages(env: &mut Env, first: serde_json::Value) -> Vec<Vec<(u64, String)>> {
    let mut out = Vec::new();
    let mut params = first;
    loop {
        let v = ok(&list(env, params.clone()).await);
        out.push(
            v["versions"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    (
                        e["seq"].as_u64().unwrap(),
                        e["kind"].as_str().unwrap().into(),
                    )
                })
                .collect(),
        );
        match v["next"].as_u64() {
            Some(next) => params["before_seq"] = next.into(),
            None => return out,
        }
        assert!(out.len() < 100, "paging does not end");
    }
}

/// Paging with the default limit returns every version exactly once, newest
/// first, with its kind, in as many full pages as the count needs and no
/// trailing empty one. Both kinds a history view tells apart are present.
#[tokio::test]
async fn paging_returns_every_version_once_newest_first() {
    let mut env = at_least(250).await;
    let want = oracle(&env).await;
    let got = pages(&mut env, serde_json::json!({})).await;
    let per = LIST_DEFAULT_LIMIT as usize;
    assert_eq!(
        got[0].len(),
        100,
        "the documented default: a first screen, so 250 versions take three calls"
    );
    assert_eq!(got.len(), want.len().div_ceil(per), "page count");
    assert!(got[..got.len() - 1].iter().all(|p| p.len() == per));
    assert_eq!(got.concat(), want);
    let kinds: std::collections::BTreeSet<&str> = want.iter().map(|(_, k)| k.as_str()).collect();
    assert!(
        kinds.contains("edit") && kinds.contains("named"),
        "{kinds:?}"
    );
}

/// Versions cut while a client pages are not in the pages it has yet to
/// read, and nothing it has not read is skipped: the cursor is a seq, not an
/// offset.
#[tokio::test]
async fn versions_cut_while_paging_shift_nothing() {
    let mut env = at_least(30).await;
    let want = oracle(&env).await;
    let first = ok(&list(&mut env, serde_json::json!({"limit": 10})).await);
    retitle(&mut env, "carol", "late").await;
    retitle(&mut env, "alice", "later").await;
    assert!(oracle(&env).await.len() > want.len(), "a version was cut");
    let mut got: Vec<u64> = first["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    let rest = pages(
        &mut env,
        serde_json::json!({"limit": 10, "before_seq": first["next"]}),
    )
    .await;
    got.extend(rest.concat().into_iter().map(|(s, _)| s));
    assert_eq!(got, want.iter().map(|(s, _)| *s).collect::<Vec<_>>());
}

/// A cursor before every version returns an empty last page; one past the
/// newest returns the newest page. Out-of-range limits and cursors are
/// refused rather than adjusted.
#[tokio::test]
async fn cursor_and_limit_bounds() {
    let mut env = at_least(5).await;
    let newest = oracle(&env).await[0].0;
    let v = ok(&list(&mut env, serde_json::json!({"before_seq": 1})).await);
    assert_eq!(v["versions"], serde_json::json!([]));
    assert!(v["next"].is_null());
    let v = ok(&list(
        &mut env,
        serde_json::json!({"before_seq": u64::MAX, "limit": 1}),
    )
    .await);
    assert_eq!(v["versions"][0]["seq"].as_u64(), Some(newest));
    assert_eq!(v["next"].as_u64(), Some(newest));
    let max = ok(&list(&mut env, serde_json::json!({"limit": LIST_MAX_LIMIT})).await);
    assert!(max["next"].is_null());
    for bad in [
        serde_json::json!({"limit": 0}),
        serde_json::json!({"limit": LIST_MAX_LIMIT + 1}),
        serde_json::json!({"limit": -1}),
        serde_json::json!({"limit": "10"}),
        serde_json::json!({"before_seq": 0}),
        serde_json::json!({"before_seq": "9"}),
    ] {
        let r = list(&mut env, bad.clone()).await;
        assert_eq!(error_kind(&r), "invalid_params", "{bad}");
    }
}

/// A listing says which versions are pruned without reading any content.
#[tokio::test]
async fn a_page_reports_pruned_from_the_row() {
    let mut env = at_least(3).await;
    env.backend.execute_for_test(&format!(
        "UPDATE kb_node_history SET content = NULL WHERE kb_id = '{KB}' AND node_id = '{NODE}' AND seq = 2"
    ));
    let v = ok(&list(&mut env, serde_json::json!({})).await);
    let pruned: Vec<(u64, bool)> = v["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["seq"].as_u64().unwrap(), e["pruned"].as_bool().unwrap()))
        .collect();
    assert!(pruned.contains(&(2, true)), "{pruned:?}");
    assert!(pruned.iter().filter(|(_, p)| *p).count() == 1, "{pruned:?}");
}

/// Why the maximum is what it is: a page of the largest records a version
/// can carry (a 200-character name of 4-byte characters, a summary naming a
/// long principal, a dozen editors) stays well under the 10 MiB frame limit
/// of `mae_mcp` (shared/mcp/src/lib.rs, `MAX_MESSAGE_SIZE`).
#[test]
fn a_maximal_page_fits_a_frame_with_room_to_spare() {
    let principal = "x".repeat(128);
    let e = HistoryEntry {
        kb_id: "k".repeat(64),
        node_id: "n".repeat(128),
        seq: u64::MAX,
        kind: VersionKind::Named,
        created_at: 4_000_000_000,
        editors: (0..12)
            .map(|i| Editor {
                kind: EditorKind::Principal,
                principal: format!("{principal}{i}"),
            })
            .collect(),
        wal_from: u64::MAX,
        wal_to: u64::MAX,
        summary: Some(format!("named by {principal}")),
        name: Some("\u{1F600}".repeat(200)),
        restored_from: None,
        content: None,
        content_hash: [0xff; 32],
        prev_hash: [0xff; 32],
        entry_hash: [0xff; 32],
    };
    let record = serde_json::to_vec(&entry_json(&e)).unwrap().len();
    let page = record * LIST_MAX_LIMIT as usize;
    assert!(
        page < 10 * 1024 * 1024 / 4,
        "{page} bytes in a maximal page"
    );
}
