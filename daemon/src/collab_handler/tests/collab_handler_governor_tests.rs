//! The session governor (ADR-099 Amendment 1, B3/B4): the deadline the session
//! loop enforces itself, the open-document cap, and re-authorization on renewal.
//! The socket-level behaviour (4401, doc_closed over the wire) is in
//! `daemon/tests/ws_session_e2e.rs`; these pin the mechanisms where they live.

use super::*;
use governor::{governor, Admission};

fn sync_update(doc: &str) -> String {
    serde_json::json!({"jsonrpc": "2.0", "id": 4, "method": "sync/update",
        "params": {"doc": doc, "update": "AAA="}})
    .to_string()
}

fn open(docs: &[&str]) -> HashSet<String> {
    docs.iter().map(|d| d.to_string()).collect()
}

#[test]
fn every_frame_is_refused_from_the_deadline_on_and_a_renewal_moves_it() {
    let (gov, handle) = governor(0, 1_000);
    let msg = sync_update("shared:pad");
    assert!(matches!(
        gov.admit(&msg, &open(&[]), 999),
        Admission::Dispatch
    ));
    assert!(matches!(
        gov.admit(&msg, &open(&[]), 1_000),
        Admission::Expired
    ));
    handle.set_deadline(2_000);
    assert!(matches!(
        gov.admit(&msg, &open(&[]), 1_500),
        Admission::Dispatch
    ));
}

fn refused_kind(a: Admission) -> String {
    match a {
        Admission::Refuse(resp) => {
            let err = resp.error.expect("a refusal is an error");
            err.data.expect("kinded")["kind"]
                .as_str()
                .unwrap()
                .to_string()
        }
        _ => "dispatched".to_string(),
    }
}

#[test]
fn at_the_cap_only_a_new_document_is_refused() {
    let (gov, _h) = governor(2, u64::MAX);
    let full = open(&["kbc:kra", "shared:pad"]);
    assert_eq!(
        refused_kind(gov.admit(&sync_update("shared:other"), &full, 0)),
        "too_many_open_docs"
    );
    let node_fetch = serde_json::json!({"jsonrpc": "2.0", "id": 5, "method": "kb/node_fetch",
        "params": {"kb_id": "kra", "node_id": "concept:n"}})
    .to_string();
    assert_eq!(
        refused_kind(gov.admit(&node_fetch, &full, 0)),
        "too_many_open_docs"
    );
    // A document it already holds, a read-through query, and a request naming
    // no document all still run.
    assert!(matches!(
        gov.admit(&sync_update("shared:pad"), &full, 0),
        Admission::Dispatch
    ));
    let query = serde_json::json!({"jsonrpc": "2.0", "id": 6, "method": "kb/query.search",
        "params": {"kb_id": "other", "query": "x"}})
    .to_string();
    assert!(matches!(gov.admit(&query, &full, 0), Admission::Dispatch));
    let list = r#"{"jsonrpc":"2.0","id":7,"method":"docs/list"}"#;
    assert!(matches!(gov.admit(list, &full, 0), Admission::Dispatch));
    // Under the cap, a new document runs.
    let one = open(&["shared:pad"]);
    assert!(matches!(
        gov.admit(&sync_update("shared:other"), &one, 0),
        Admission::Dispatch
    ));
}

/// Bob holds documents of two KBs and a buffer. Removed from Alice's KB, he
/// loses exactly that KB's documents at re-authorization — and stops receiving
/// its updates — while his own KB and the buffer stay open.
#[tokio::test]
async fn reauthorization_closes_only_the_kb_the_principal_lost() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let (alice, bob) = (fp("alice"), fp("bob"));
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&alice),
        "kra",
        "",
        &mut docs,
    )
    .await;
    let add = kb_member_msg("kb/add_member", "kra", &bob, Some("viewer"));
    assert!(
        dispatch_as(&store, &bc, Some("alice"), Some(&alice), add, &mut docs)
            .await
            .error
            .is_none()
    );
    kb_share_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob),
        "krb",
        "",
        &mut HashSet::new(),
    )
    .await;

    let bob_session = 9;
    let mut rx = register_session(&bc, bob_session);
    bc.lock().unwrap().add_event_sub(bob_session, "sync_update");
    let mut bob_docs = open(&["kbc:kra", "kbc:krb", "shared:pad"]);
    for d in &bob_docs {
        bc.lock().unwrap().subscribe_doc(bob_session, d);
    }
    let remove = kb_member_msg("kb/remove_member", "kra", &bob, None);
    assert!(
        dispatch_as(&store, &bc, Some("alice"), Some(&alice), remove, &mut docs)
            .await
            .error
            .is_none()
    );

    let who = (bob_session, Some(bob.as_str()), Transport::Hub);
    let lost = governor::reauthorize_open_docs(&store, &bc, who, &mut bob_docs).await;
    assert_eq!(lost, vec![("kbc:kra".to_string(), "kra".to_string())]);
    assert_eq!(bob_docs, open(&["kbc:krb", "shared:pad"]));

    let update = |doc: &str| EditorEvent::SyncUpdate {
        buffer_name: doc.to_string(),
        update_base64: "AA==".to_string(),
        wal_seq: 1,
        content_header: None,
    };
    while rx.try_recv().is_ok() {}
    bc.lock().unwrap().broadcast(&update("kbc:kra"));
    assert!(
        rx.try_recv().is_err(),
        "a closed document's updates must stop"
    );
    bc.lock().unwrap().broadcast(&update("kbc:krb"));
    assert!(
        rx.try_recv().is_ok(),
        "a kept document's updates still arrive"
    );
}

type Pipe = (
    tokio::io::BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
    tokio::io::WriteHalf<tokio::io::DuplexStream>,
);

/// A governed session for a bearer `principal`, as the WebSocket route runs it.
fn governed(
    store: &Arc<DocStore>,
    principal: &str,
    deadline: u64,
) -> (Pipe, tokio::task::JoinHandle<()>) {
    let deps = SessionDeps {
        doc_store: Arc::clone(store),
        broadcaster: test_broadcaster(),
        start_time: std::time::Instant::now(),
        artifact_store: Arc::new(crate::artifact_store::NoArtifactStore),
        quota: Arc::new(crate::quota::NoQuota),
        kb_query_limits: crate::kb_query::KbQueryLimits::default(),
    };
    let (client, server) = tokio::io::duplex(1 << 16);
    let (sr, sw) = tokio::io::split(server);
    let (gov, _h) = governor(0, deadline);
    let peer = mae_mcp::identity::PeerIdentity::bearer(principal);
    let reader = tokio::io::BufReader::new(sr);
    let session = tokio::spawn(handle_client_governed(reader, sw, peer, deps, gov));
    let (cr, cw) = tokio::io::split(client);
    ((tokio::io::BufReader::new(cr), cw), session)
}

/// Send one message; the reply, if one comes within two seconds.
async fn roundtrip(pipe: &mut Pipe, msg: serde_json::Value) -> Option<serde_json::Value> {
    let body = msg.to_string();
    let t = std::time::Duration::from_secs(2);
    mae_mcp::write_framed(&mut pipe.1, body.as_bytes(), t)
        .await
        .ok()?;
    let reply = tokio::time::timeout(t, mae_mcp::read_message(&mut pipe.0)).await;
    reply
        .ok()?
        .ok()?
        .and_then(|m| serde_json::from_str(&m).ok())
}

fn share_msg(kb_id: &str) -> serde_json::Value {
    let coll = KbCollectionDoc::new_owned(kb_id, "", "");
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {"kb_id": kb_id, "name": kb_id,
            "collection_state": update_to_base64(&coll.encode_state()), "nodes": []}})
}

/// B3's "no frame is applied after the deadline", at the place that enforces
/// it: a governed session whose deadline has passed applies nothing, while the
/// same frame under a live deadline is applied (the control).
#[tokio::test]
async fn a_governed_session_applies_nothing_after_its_deadline() {
    for (deadline, applied) in [(1, false), (u64::MAX, true)] {
        let store = test_doc_store();
        let (mut pipe, session) = governed(&store, &fp("alice"), deadline);
        let reply = roundtrip(&mut pipe, share_msg("kdl")).await;
        drop(pipe);
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), session).await;
        assert_eq!(
            store.has_doc("kbc:kdl").await,
            applied,
            "deadline {deadline}"
        );
        assert_eq!(reply.is_some(), applied, "a reply only when applied");
    }
}

/// A bearer identity has no key, so a pending join it makes records none —
/// never the all-zero placeholder an owner would then wrap a content key to.
#[tokio::test]
async fn a_bearer_sessions_pending_join_records_no_key() {
    let store = test_doc_store();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &test_broadcaster(),
        Some("alice"),
        Some(&fp("alice")),
        "kpj",
        "",
        &mut docs,
    )
    .await;
    let (mut pipe, _session) = governed(&store, &fp("bob"), u64::MAX);
    let join = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "kb/join", "params": {"kb_id": "kpj"}});
    let reply = roundtrip(&mut pipe, join).await.expect("a reply");
    assert_eq!(reply["result"]["status"], "pending", "{reply}");
    let coll = load_collection(&store, "kpj").await.unwrap();
    let pending = coll.pending();
    let bob = pending
        .iter()
        .find(|p| p.fingerprint == fp("bob"))
        .expect("bob is pending");
    assert_eq!(bob.pubkey, None);
}

/// ADR-099 A1 B4, at the session: when the broadcaster closes a document for
/// this session because its queue was full, the next delivery tells the
/// client, once, with `session/doc_closed` (`slow_consumer`, with `kb_id` and
/// `node_id` for a node document), and the session forgets the document.
/// Kills: closing silently, which leaves the client missing edits for good.
#[tokio::test]
async fn a_document_closed_for_a_full_queue_is_reported_to_the_client() {
    let store = test_doc_store();
    let broadcaster = test_broadcaster();
    let mut session = mae_mcp::session::ClientSession::new();
    let sid = session.id;
    let doc = "kbn:kra:concept:n";
    let mut rx = {
        let mut b = broadcaster.lock().unwrap();
        let rx = b.subscribe_doc_scoped(sid, vec!["*".into()]);
        b.subscribe_doc(sid, doc);
        rx
    };
    let update = |n| mae_mcp::broadcast::EditorEvent::SyncUpdate {
        buffer_name: doc.into(),
        update_base64: "AAA=".into(),
        wal_seq: n,
        content_header: None,
    };
    for n in 0..=100 {
        broadcaster.lock().unwrap().broadcast(&update(n));
    }
    let mut docs: HashSet<String> = [doc.to_string()].into();
    let first = rx.try_recv().expect("queued");
    let mut out: Vec<u8> = Vec::new();
    let timeout = std::time::Duration::from_secs(1);
    assert!(
        deliver_event(
            &first,
            &mut session,
            &store,
            &broadcaster,
            &mut docs,
            (&mut out, timeout)
        )
        .await
    );
    let text = String::from_utf8(out.clone()).unwrap();
    let closed: Vec<&str> = text.matches("session/doc_closed").collect();
    assert_eq!(closed.len(), 1, "{text}");
    for want in [
        "\"reason\":\"slow_consumer\"",
        "\"kb_id\":\"kra\"",
        "\"node_id\":\"concept:n\"",
    ] {
        assert!(text.contains(want), "{want} missing: {text}");
    }
    assert!(
        !docs.contains(doc),
        "the session still counts the closed document"
    );
    out.clear();
    let next = rx.try_recv().expect("queued");
    deliver_event(
        &next,
        &mut session,
        &store,
        &broadcaster,
        &mut docs,
        (&mut out, timeout),
    )
    .await;
    assert!(
        !String::from_utf8(out).unwrap().contains("doc_closed"),
        "reported twice"
    );
}
