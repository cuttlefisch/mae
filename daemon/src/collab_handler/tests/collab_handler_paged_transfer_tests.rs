//! ADR-022 Amendment 1: a whole-KB transfer is paged, both ways, and a message
//! over the session's cap is answered rather than dropped.
//!
//! Driven through a real session loop (`handle_client_governed`, the loop every
//! transport runs) with a small `max_message_bytes`, so a KB of a few dozen KB
//! needs several pages, the way a KB of hundreds of MB does at the stock bound.

use super::*;
use governor::governor;
use mae_sync::kb::KbNodeDoc;

const CAP: usize = 32 * 1024;
const ALICE: &str = "SHA256:alice-paged";
const BOB: &str = "SHA256:bob-paged";
const KB: &str = "kpaged";
const NODES: usize = 12;

type Pipe = (
    tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
);

fn store(cap: Option<usize>) -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    Arc::new(DocStore::new(backend, 500).with_max_message_bytes(cap))
}

fn session(store: &Arc<DocStore>, bc: &SharedBroadcaster, principal: &str) -> Pipe {
    session_bounded(store, bc, principal, None)
}

/// A session whose transport caps its replies at `reply_bound` (as the
/// WebSocket does with `max_outbound_bytes`); `None` is a transport that does
/// not, like raw TCP.
fn session_bounded(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    principal: &str,
    reply_bound: Option<usize>,
) -> Pipe {
    let deps = SessionDeps {
        doc_store: Arc::clone(store),
        broadcaster: Arc::clone(bc),
        start_time: std::time::Instant::now(),
        artifact_store: Arc::new(crate::artifact_store::NoArtifactStore),
        quota: Arc::new(crate::quota::NoQuota),
        kb_query_limits: crate::kb_query::KbQueryLimits::default(),
    };
    let (client, server) = tokio::io::duplex(1 << 16);
    let (sr, sw) = tokio::io::split(server);
    let (gov, _h) = governor(0, u64::MAX);
    let gov = match reply_bound {
        Some(max) => gov.with_reply_bound(max),
        None => gov,
    };
    let peer = mae_mcp::identity::PeerIdentity::bearer(principal);
    tokio::spawn(handle_client_governed(
        tokio::io::BufReader::new(sr),
        sw,
        peer,
        deps,
        gov,
    ));
    let (cr, cw) = tokio::io::split(client);
    (tokio::io::BufReader::new(cr), cw)
}

async fn send(pipe: &mut Pipe, body: &[u8]) {
    let t = std::time::Duration::from_secs(5);
    mae_mcp::write_framed(&mut pipe.1, body, t).await.unwrap();
}

/// The next message, raw; `None` once the session has ended.
async fn recv(pipe: &mut Pipe) -> Option<String> {
    let t = std::time::Duration::from_secs(5);
    let read = mae_mcp::read_message_limited(&mut pipe.0, mae_mcp::MAX_DAEMON_RESPONSE_SIZE);
    tokio::time::timeout(t, read).await.ok()?.ok()?
}

/// One request, and the raw reply to it (notifications skipped).
async fn call_raw(pipe: &mut Pipe, msg: serde_json::Value) -> String {
    let id = msg["id"].clone();
    send(pipe, msg.to_string().as_bytes()).await;
    loop {
        let raw = recv(pipe).await.expect("a reply");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        if v["id"] == id {
            return raw;
        }
    }
}

async fn call(pipe: &mut Pipe, msg: serde_json::Value) -> serde_json::Value {
    serde_json::from_str(&call_raw(pipe, msg).await).unwrap()
}

fn node_id(i: usize) -> String {
    format!("n{i:02}")
}

/// About 6 KB of content per node, distinct per node.
fn node_state(i: usize) -> Vec<u8> {
    let body = format!("{i}:").repeat(2000);
    KbNodeDoc::new(&node_id(i), &format!("Node {i}"), &body, &[]).encode_state()
}

/// Alice shares `NODES` nodes the way a paging client does (E1): the manifest
/// of every node with the first batch, then the rest in batches, each request
/// under the cap.
async fn share_in_batches(alice: &mut Pipe) {
    let mut coll = KbCollectionDoc::new_owned(KB, "", "");
    for i in 0..NODES {
        let _ = coll.add_node(&node_id(i), &format!("Node {i}"));
    }
    let collection = update_to_base64(&coll.encode_state());
    for (batch, ids) in (0..NODES).collect::<Vec<_>>().chunks(3).enumerate() {
        let nodes: Vec<_> = ids
            .iter()
            .map(|&i| serde_json::json!({"id": node_id(i), "state": update_to_base64(&node_state(i))}))
            .collect();
        let msg = serde_json::json!({"jsonrpc": "2.0", "id": 100 + batch, "method": "kb/share",
            "params": {"kb_id": KB, "name": KB, "collection_state": collection, "nodes": nodes}});
        assert!(msg.to_string().len() < CAP, "a batch must fit the cap");
        let r = call(alice, msg).await;
        assert!(r["error"].is_null(), "batch {batch}: {r}");
    }
    let add = kb_member_msg("kb/add_member", KB, BOB, Some("viewer"));
    let r = call(alice, add).await;
    assert!(r["error"].is_null(), "{r}");
}

/// Every page of a paged join: `(raw length, node entries, next_after)`.
async fn paged_join(
    bob: &mut Pipe,
    node_svs: &[serde_json::Value],
) -> Vec<(usize, Vec<serde_json::Value>, Option<String>)> {
    let mut pages = Vec::new();
    let mut after: Option<String> = None;
    for id in 1.. {
        let mut params = serde_json::json!({"kb_id": KB, "paged": true, "node_svs": node_svs});
        if let Some(a) = &after {
            params["after"] = serde_json::json!(a);
        }
        let raw = call_raw(
            bob,
            serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "kb/join", "params": params}),
        )
        .await;
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert!(v["error"].is_null(), "page {id}: {v}");
        let nodes = v["result"]["nodes"].as_array().unwrap().clone();
        let next = v["result"]["next_after"].as_str().map(str::to_string);
        pages.push((raw.len(), nodes, next.clone()));
        match next {
            Some(n) => after = Some(n),
            None => break,
        }
        assert!(id < 50, "paging does not terminate");
    }
    pages
}

#[tokio::test]
async fn a_kb_larger_than_the_cap_is_shared_in_batches_and_joined_in_pages() {
    let s = store(Some(CAP));
    let bc = test_broadcaster();
    let mut alice = session(&s, &bc, ALICE);
    share_in_batches(&mut alice).await;

    let mut bob = session(&s, &bc, BOB);
    let pages = paged_join(&mut bob, &[]).await;
    assert!(pages.len() >= 3, "{} pages", pages.len());

    let mut served = Vec::new();
    for (k, (len, nodes, next)) in pages.iter().enumerate() {
        assert!(*len <= CAP, "page {k} is {len} bytes, over the cap {CAP}");
        assert!(!nodes.is_empty(), "page {k} is empty");
        let last = nodes.last().unwrap()["id"].as_str().unwrap();
        // `next_after` names the page's last node, and only the last page has none.
        assert_eq!(next.is_none(), k + 1 == pages.len());
        if let Some(n) = next {
            assert_eq!(n, last);
        }
        for n in nodes {
            let id = n["id"].as_str().unwrap().to_string();
            let state = base64_to_update(n["state"].as_str().unwrap()).unwrap();
            let doc = KbNodeDoc::from_bytes(&state).unwrap();
            let i: usize = id[1..].parse().unwrap();
            assert_eq!(doc.body(), format!("{i}:").repeat(2000), "{id}");
            served.push(id);
        }
    }
    let want: Vec<String> = (0..NODES).map(node_id).collect();
    assert_eq!(served, want, "every node exactly once, in id order");

    // An older client (no `paged`) still gets today's single reply.
    let whole = call(&mut bob, kb_join_msg(KB)).await;
    assert_eq!(whole["result"]["nodes"].as_array().unwrap().len(), NODES);
    assert!(whole["result"]["next_after"].is_null());
}

/// The node ids of each page.
fn page_ids(pages: &[(usize, Vec<serde_json::Value>, Option<String>)]) -> Vec<Vec<String>> {
    pages
        .iter()
        .map(|(_, nodes, _)| {
            nodes
                .iter()
                .map(|n| n["id"].as_str().unwrap().to_string())
                .collect()
        })
        .collect()
}

/// #1003: a session's reply bound lowers a page and never raises one. With
/// none (raw TCP), or one at or above the store's bound (a WebSocket left at
/// its default), the pages are exactly the ones the store's bound gives; with
/// a lower one, every page fits it and the KB is still served whole, in order.
#[tokio::test]
async fn a_sessions_reply_bound_lowers_pages_and_never_raises_them() {
    let s = store(Some(CAP));
    let bc = test_broadcaster();
    let mut alice = session(&s, &bc, ALICE);
    share_in_batches(&mut alice).await;
    let mut bob = session(&s, &bc, BOB);
    let baseline = page_ids(&paged_join(&mut bob, &[]).await);
    assert!(baseline.len() >= 3, "{baseline:?}");
    for bound in [CAP, CAP * 4, usize::MAX] {
        let mut bob = session_bounded(&s, &bc, BOB, Some(bound));
        let pages = paged_join(&mut bob, &[]).await;
        assert_eq!(page_ids(&pages), baseline, "reply bound {bound}");
    }

    let lower = CAP / 2;
    let mut bob = session_bounded(&s, &bc, BOB, Some(lower));
    let pages = paged_join(&mut bob, &[]).await;
    for (k, (len, _, _)) in pages.iter().enumerate() {
        assert!(
            *len <= lower,
            "page {k} is {len} bytes, over the bound {lower}"
        );
    }
    let lowered = page_ids(&pages);
    assert!(lowered.len() > baseline.len(), "{lowered:?}");
    assert_eq!(
        lowered.concat(),
        baseline.concat(),
        "the same nodes, once each"
    );
}

#[tokio::test]
async fn a_page_after_the_first_still_reconciles_from_the_members_state_vector() {
    // ADR-022's crash case on a later page: the member sends its state vector
    // for the last node only, so every other node goes out whole and the last
    // one, on the last page, comes back as a diff.
    let s = store(Some(CAP));
    let bc = test_broadcaster();
    let mut alice = session(&s, &bc, ALICE);
    share_in_batches(&mut alice).await;
    let mut bob = session(&s, &bc, BOB);

    let last = node_id(NODES - 1);
    let doc = KbNodeDoc::from_bytes(&node_state(NODES - 1)).unwrap();
    let svs = [serde_json::json!({"id": last, "sv": update_to_base64(&doc.state_vector())})];
    let pages = paged_join(&mut bob, &svs).await;
    assert!(pages.len() >= 2, "{} pages", pages.len());
    let (_, final_page, _) = pages.last().unwrap();
    let entry = final_page
        .iter()
        .find(|n| n["id"] == last.as_str())
        .unwrap();
    assert!(
        entry["diff"].is_string() && entry["state"].is_null(),
        "{entry}"
    );
    for (_, nodes, _) in &pages {
        for n in nodes.iter().filter(|n| n["id"] != last.as_str()) {
            assert!(n["state"].is_string(), "{n}");
        }
    }
}

#[tokio::test]
async fn a_request_over_the_cap_is_answered_then_the_session_ends() {
    let s = store(Some(1024));
    let bc = test_broadcaster();
    let mut pipe = session(&s, &bc, ALICE);
    let body =
        serde_json::json!({"jsonrpc": "2.0", "id": 7, "method": "$/ping", "pad": "p".repeat(2000)})
            .to_string();
    send(&mut pipe, body.as_bytes()).await;
    let raw = recv(&mut pipe).await.expect("an answer, not a silent drop");
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v["id"].is_null(), "{v}");
    assert_eq!(v["error"]["data"]["kind"], "request_too_large", "{v}");
    assert_eq!(v["error"]["data"]["size"], body.len());
    assert_eq!(v["error"]["data"]["limit"], 1024);
    assert!(recv(&mut pipe).await.is_none(), "the session ends after it");
}

#[tokio::test]
async fn a_request_at_the_cap_is_read() {
    // The cap is inclusive: the bound is what one document fits, not one less.
    let s = store(Some(1024));
    let bc = test_broadcaster();
    let mut pipe = session(&s, &bc, ALICE);
    let mut msg = serde_json::json!({"jsonrpc": "2.0", "id": 8, "method": "$/ping", "pad": ""});
    let fill = 1024 - msg.to_string().len();
    msg["pad"] = serde_json::json!("p".repeat(fill));
    assert_eq!(msg.to_string().len(), 1024);
    let r = call(&mut pipe, msg).await;
    assert!(r["error"].is_null(), "{r}");
}

#[tokio::test]
async fn initialize_reports_the_session_limits() {
    for (cap, want) in [
        (Some(CAP), CAP),
        (None, crate::message_limits::message_bound(0)),
    ] {
        let s = store(cap);
        let bc = test_broadcaster();
        let mut pipe = session(&s, &bc, ALICE);
        let init = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "t", "version": "0"}}});
        let r = call(&mut pipe, init).await;
        assert_eq!(r["result"]["limits"]["max_frame_bytes"], want, "{r}");
        assert_eq!(r["result"]["limits"]["max_outbound_bytes"], want, "{r}");
    }
}

#[tokio::test]
async fn a_node_larger_than_a_page_still_goes_out_alone() {
    // A node stored above the bound (before the limit existed, or written
    // directly) must still be served, one per page, never skipped and never
    // an endless run of empty pages.
    let s = store(Some(4096));
    let by = crate::doc_editors::Attribution::Principal("test-editor");
    let mut coll = KbCollectionDoc::new_owned(KB, ALICE, "alice");
    let _ = coll.upsert_member(BOB, "bob", mae_sync::kb::Role::Viewer);
    for i in 0..3 {
        let _ = coll.add_node(&node_id(i), &format!("Node {i}"));
        s.share_doc(
            &mae_sync::kb_node_doc_name(KB, &node_id(i)),
            &node_state(i),
            by,
        )
        .await
        .unwrap();
    }
    s.share_doc(&format!("kbc:{KB}"), &coll.encode_state(), by)
        .await
        .unwrap();
    let bc = test_broadcaster();
    let mut bob = session(&s, &bc, BOB);
    let pages = paged_join(&mut bob, &[]).await;
    let per_page: Vec<usize> = pages.iter().map(|(_, n, _)| n.len()).collect();
    assert_eq!(per_page, [1, 1, 1], "one oversized node per page");
}
