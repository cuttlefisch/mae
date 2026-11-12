//! A live sync-WebSocket session, over the real listener (ADR-099 Amendment 1,
//! B2/B3/B4; B7 "Renewal" and "Multiplexing" — slow documents excepted).

mod daemon_harness;
mod ws_common;

use std::time::Duration;

use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::text::TextSync;
use serde_json::json;
use ws_common::*;

fn ok(resp: &serde_json::Value) -> bool {
    resp["error"].is_null()
}

/// The principal `session/hello` reports is the one `kb/join` reports: a
/// client keys its identity on that, so the two must never disagree.
#[tokio::test]
async fn hello_and_kb_join_report_the_same_principal() {
    let d = spawn(Opts::default()).await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    assert_eq!(ws.log.len(), 1, "hello is the first frame");
    let p = &hello["params"];
    assert_eq!(p["limits"]["max_open_docs"], 1024);
    assert_eq!(
        p["limits"]["max_frame_bytes"], 1_048_576,
        "defaults to the body cap"
    );
    assert!(ok(&ws.call(1, "kb/share", kb_share("kh", &[])).await));
    let joined = ws.call(2, "kb/join", json!({"kb_id": "kh"})).await;
    assert_eq!(joined["result"]["principal"], p["principal"]);
    assert_eq!(p["principal"], "SHA256:alice");
}

/// One socket carrying several documents is ONE connection on the listener.
#[tokio::test]
async fn many_documents_over_one_socket_are_one_connection() {
    let d = spawn(Opts::default()).await;
    let active = |s: serde_json::Value| s["connections"]["oauth"]["active"].as_u64();
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let nodes = [
        ("n:1", "one", "a"),
        ("n:2", "two", "b"),
        ("n:3", "three", "c"),
    ];
    assert!(ok(&ws.call(1, "kb/share", kb_share("km1", &nodes)).await));
    assert!(ok(&ws.call(2, "kb/share", kb_share("km2", &nodes)).await));
    // Wait for the upgrade's HTTP exchange to settle, then count.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        active(daemon_status(&d.socket).await),
        Some(1),
        "8 documents, 1 connection"
    );
    drop(ws);
    let mut n = None;
    for _ in 0..50 {
        n = active(daemon_status(&d.socket).await);
        if n == Some(0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(n, Some(0), "closing the socket releases its slot");
}

/// B3: `session/renew_needed` arrives `renew_lead` before the deadline; a
/// client that does not renew is closed `4401 token_expired`.
#[tokio::test]
async fn a_missed_deadline_closes_4401() {
    let d = spawn(Opts {
        renew_lead_secs: 2,
        ..Opts::default()
    })
    .await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 4)).await;
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    assert!(hello.get("id").is_none(), "hello is a notification");
    let deadline = deadline_unix(&hello["params"]["auth_deadline"]);
    let needed = ws.until(|v| v["method"] == "session/renew_needed").await;
    assert!(needed.get("id").is_none(), "renew_needed is a notification");
    assert_eq!(deadline_unix(&needed["params"]["auth_deadline"]), deadline);
    assert!(
        now_unix() < deadline,
        "renew_needed must come before the deadline"
    );
    assert_eq!(ws.closed().await, (4401, "token_expired".to_string()));
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// B3: a renewal with the same identity moves the deadline, and the socket
/// outlives the first token.
#[tokio::test]
async fn a_renewal_moves_the_deadline() {
    let d = spawn(Opts {
        renew_lead_secs: 2,
        ..Opts::default()
    })
    .await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3)).await;
    ws.until(|v| v["method"] == "session/renew_needed").await;
    let renewed = ws
        .call(
            1,
            "session/renew",
            json!({"token": d.token("SHA256:alice", 60)}),
        )
        .await;
    let new_deadline = deadline_unix(&renewed["result"]["auth_deadline"]);
    assert!(new_deadline > now_unix() + 30);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        ok(&ws.call(2, "kb/share", kb_share("kr", &[])).await),
        "alive past the first token"
    );
}

/// B3: a renewal carrying a different subject closes the socket.
#[tokio::test]
async fn a_renewal_with_a_different_principal_closes() {
    let d = spawn(Opts::default()).await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let bob = d.token("SHA256:bob", 3600);
    ws.send(
        json!({"jsonrpc": "2.0", "id": 1, "method": "session/renew", "params": {"token": bob}}),
    )
    .await;
    assert_eq!(ws.closed().await, (4403, "principal_changed".to_string()));
}

/// B3: Bob holds Alice's KB and his own. Removed from Alice's between
/// renewals, he loses exactly her KB's documents at the next renewal — told
/// per document, `not_member` — and keeps his own on the same socket.
#[tokio::test]
async fn a_member_removed_between_renewals_loses_that_kb_only() {
    let d = spawn(Opts {
        renew_lead_secs: 2,
        ..Opts::default()
    })
    .await;
    let mut alice = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let nodes = [("a:1", "A one", "body")];
    assert!(ok(&alice
        .call(1, "kb/share", kb_share("kra", &nodes))
        .await));
    let bob_in = json!({"kb_id": "kra", "member": "SHA256:bob", "role": "viewer", "label": "bob"});
    assert!(ok(&alice.call(2, "kb/add_member", bob_in).await));

    let mut bob = connect_bearer(&d, &d.token("SHA256:bob", 4)).await;
    assert!(ok(&bob.call(1, "kb/share", kb_share("krb", &[])).await));
    assert!(ok(&bob.call(2, "kb/join", json!({"kb_id": "kra"})).await));
    let bob_out = json!({"kb_id": "kra", "member": "SHA256:bob"});
    assert!(ok(&alice.call(3, "kb/remove_member", bob_out).await));

    bob.until(|v| v["method"] == "session/renew_needed").await;
    let renew = json!({"token": d.token("SHA256:bob", 3600)});
    assert!(ok(&bob.call(3, "session/renew", renew).await));
    let mut closed = std::collections::BTreeSet::new();
    while closed.len() < 2 {
        let v = bob.until(|v| v["method"] == "session/doc_closed").await;
        assert_eq!(v["params"]["reason"], "not_member");
        assert_eq!(v["params"]["kb_id"], "kra");
        let doc = v["params"]["doc"].as_str().unwrap().to_string();
        let node = if doc.starts_with("kbn:") {
            json!("a:1")
        } else {
            json!(null)
        };
        assert_eq!(v["params"]["node_id"], node, "{v}");
        closed.insert(doc);
    }
    let want: std::collections::BTreeSet<String> = ["kbc:kra", "kbn:kra:a:1"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(closed, want);
    let own = bob
        .call(4, "sync/state_vector", json!({"doc": "kbc:krb"}))
        .await;
    assert!(ok(&own), "his own KB stays open on the same socket: {own}");
}

/// B4: a frame over `max_frame_bytes` closes the socket with `1009`.
#[tokio::test]
async fn a_frame_over_the_cap_closes_1009() {
    let d = spawn(Opts {
        max_frame_bytes: Some(1024),
        ..Opts::default()
    })
    .await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let hello = ws.until(|v| v["method"] == "session/hello").await;
    assert_eq!(hello["params"]["limits"]["max_frame_bytes"], 1024);
    let small = json!({"jsonrpc": "2.0", "id": 1, "method": "$/ping", "pad": "x".repeat(900)});
    assert!(ok(&{
        ws.send(small).await;
        ws.until(|v| v["id"] == 1).await
    }));
    ws.send_raw(format!("{{\"pad\":\"{}\"}}", "x".repeat(2000)))
        .await;
    assert_eq!(ws.closed().await.0, 1009);
}

/// B4: `max_open_docs` refuses a request naming a new document once the
/// session holds that many; one it already holds still works.
#[tokio::test]
async fn the_open_document_cap_is_enforced() {
    let d = spawn(Opts {
        max_open_docs: 2,
        ..Opts::default()
    })
    .await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    assert!(ok(&ws.call(1, "kb/share", kb_share("kc1", &[])).await));
    assert!(ok(&ws.call(2, "kb/share", kb_share("kc2", &[])).await));
    let third = ws.call(3, "kb/share", kb_share("kc3", &[])).await;
    assert_eq!(kind(&third), "too_many_open_docs", "{third}");
    assert_eq!(third["error"]["data"]["limit"], 2);
    let held = ws
        .call(4, "sync/state_vector", json!({"doc": "kbc:kc1"}))
        .await;
    assert!(ok(&held), "{held}");
}

/// B4: requests over the socket are charged to the principal's tenant exactly
/// as over the other listeners — a tiny budget runs out.
#[tokio::test]
async fn socket_requests_count_against_the_tenant_quota() {
    let budget = Some(("SHA256:alice".to_string(), 12));
    let d = spawn(Opts {
        tenant_budget: budget,
        ..Opts::default()
    })
    .await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    let mut kinds = Vec::new();
    for i in 0..4u64 {
        let resp = ws
            .call(i + 1, "kb/share", kb_share(&format!("kq{i}"), &[]))
            .await;
        kinds.push(kind(&resp));
    }
    assert_eq!(kinds[..2], ["", ""], "two 5-point writes fit in 12");
    assert_eq!(kinds[2], "tenant_quota_exceeded", "{kinds:?}");
}

/// The envelope is JSON text; a binary frame closes the socket `1003`.
#[tokio::test]
async fn a_binary_frame_closes_1003() {
    let d = spawn(Opts::default()).await;
    let mut ws = connect_bearer(&d, &d.token("SHA256:alice", 3600)).await;
    ws.send_binary(vec![0u8, 1, 2]).await;
    assert_eq!(ws.closed().await.0, 1003);
}

/// ADR-111 D6: the daemon pings a quiet session on `ws_ping_secs`, so a
/// proxy's idle timeout never closes it; `0` turns pings off. Kills: no
/// keepalive at all.
#[tokio::test]
async fn a_quiet_session_is_pinged_on_the_configured_period() {
    for (secs, expect) in [(1, true), (0, false)] {
        let d = spawn(Opts {
            ping_secs: Some(secs),
            ..Opts::default()
        })
        .await;
        let token = d.token("SHA256:alice", 3600);
        let mut ws = connect_bearer(&d, &token).await;
        let got = ws.pinged(std::time::Duration::from_millis(2500)).await;
        assert_eq!(got, expect, "ws_ping_secs = {secs}");
    }
}

/// Three writers on one document over the real socket converge: each makes
/// interleaved inserts at seeded-random offsets, sends them without waiting,
/// and applies whatever the daemon relays; afterwards every writer's text, and
/// the daemon's own state, are identical and hold every insert. Kills a
/// transport that drops, truncates or fails to relay updates (a naive "my
/// edit arrived" check passes that; this one does not).
#[tokio::test]
async fn three_writers_converge_through_the_socket() {
    let d = spawn(Opts::default()).await;
    let doc = "conv.txt";
    let mut socks = Vec::new();
    for who in ["SHA256:w1", "SHA256:w2", "SHA256:w3"] {
        let mut ws = connect_bearer(&d, &d.token(who, 3600)).await;
        ws.call(
            1,
            "notifications/subscribe",
            json!({"types": ["sync_update"]}),
        )
        .await;
        socks.push(ws);
    }
    let mut texts: Vec<TextSync> = (0..3)
        .map(|i| TextSync::with_client_id("", 100 + i as u64))
        .collect();
    let seed = texts[0].insert(0, "base:");
    let r = socks[0]
        .call(
            2,
            "sync/update",
            json!({"doc": doc, "update": update_to_base64(&seed)}),
        )
        .await;
    assert!(r.get("error").is_none(), "{r}");
    for i in 1..3 {
        let r = socks[i].call(2, "sync/resync", json!({"doc": doc})).await;
        let state = base64_to_update(r["result"]["state"].as_str().expect("state")).unwrap();
        texts[i].apply_update(&state).unwrap();
    }
    let last_id = write_interleaved(&mut socks, &mut texts, doc).await;
    // Every writer's last update acknowledged: the daemon has applied and
    // relayed everything. Then each writer applies what it has received until
    // it matches the daemon's state (a relay may still be in flight), bounded.
    for (w, ws) in socks.iter_mut().enumerate() {
        let last = last_id[w];
        ws.until(|v| v["id"] == last && v.get("method").is_none())
            .await;
    }
    let server = socks[0]
        .call(9001, "sync/full_state", json!({"doc": doc}))
        .await;
    let mut truth = TextSync::with_client_id("", 999);
    truth
        .apply_update(&base64_to_update(server["result"]["state"].as_str().unwrap()).unwrap())
        .unwrap();
    let want = truth.content();
    for (w, ws) in socks.iter_mut().enumerate() {
        catch_up(ws, &mut texts[w], &want, doc).await;
        assert_eq!(texts[w].content(), want, "writer {w} diverged");
    }
    // No character lost or invented: inserts at random offsets may land
    // inside one another's tokens, so compare the multiset of characters.
    let mut expected: Vec<char> = "base:".chars().collect();
    for w in 0..3 {
        for round in 0..15 {
            expected.extend(format!("[{w}.{round}]").chars());
        }
    }
    let mut got: Vec<char> = want.chars().collect();
    expected.sort_unstable();
    got.sort_unstable();
    assert_eq!(got, expected, "an insert was lost or duplicated: {want}");
}

/// Each of three writers makes 15 inserts of `[w.round]` at seeded-random
/// offsets, interleaved, sending each without waiting for relays. Returns each
/// writer's last request id.
async fn write_interleaved(socks: &mut [Ws], texts: &mut [TextSync], doc: &str) -> [u64; 3] {
    let mut rng: u64 = 0x5eed;
    let mut id = 10;
    let mut last_id = [0u64; 3];
    for round in 0..15 {
        for w in 0..3 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = texts[w].content().chars().count() as u64;
            let at = (rng >> 33) % (len + 1);
            let token = format!("[{w}.{round}]");
            let upd = texts[w].insert(at as u32, &token);
            id += 1;
            last_id[w] = id;
            socks[w]
                .send(json!({"jsonrpc": "2.0", "id": id, "method": "sync/update",
                "params": {"doc": doc, "update": update_to_base64(&upd)}}))
                .await;
        }
    }
    last_id
}

/// Applies every relayed update in `ws`'s log to `text` until it reads `want`,
/// prodding the socket between tries (a relay may still be in flight), bounded
/// at 50 tries. The caller asserts the result.
async fn catch_up(ws: &mut Ws, text: &mut TextSync, want: &str, doc: &str) {
    for attempt in 0..50 {
        for line in ws.log.clone() {
            let v: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
            if v["method"] == "notifications/sync_update" {
                let b64 = v["params"]["event"]["data"]["update_base64"]
                    .as_str()
                    .unwrap();
                text.apply_update(&base64_to_update(b64).unwrap()).unwrap();
            }
        }
        if text.content() == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        ws.call(9100 + attempt, "sync/state_vector", json!({"doc": doc}))
            .await;
    }
}
