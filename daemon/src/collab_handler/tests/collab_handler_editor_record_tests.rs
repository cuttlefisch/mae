//! ADR-113 D3: each collab write path records the principal that applied it.
//!
//! Driven through the real dispatch (`handle_doc_request_inner`, the function a
//! live session calls) so what is checked is the attribution each handler
//! actually threads into the store, not what a unit test passes by hand.

use super::*;
use crate::doc_editors::EditorKind;

fn principal(p: &str) -> (EditorKind, String) {
    (EditorKind::Principal, p.to_string())
}

/// Two members editing one KB node are each recorded under their OWN session
/// principal — and a non-member whose write is refused is recorded nowhere.
/// The owner's share and membership changes are recorded against the owner.
#[tokio::test]
async fn kb_node_edits_are_recorded_against_each_sessions_own_principal() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (alice, bob, carol, dave) = (fp("alice"), fp("bob"), fp("carol"), fp("dave"));
    let kb = "kbed";
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&alice),
        kb,
        "alice",
        &mut docs,
    )
    .await;
    for member in [&bob, &carol] {
        let add = kb_member_msg("kb/add_member", kb, member, Some("editor"));
        let r = dispatch_as(&store, &bc, Some("alice"), Some(&alice), add, &mut docs).await;
        assert!(r.error.is_none(), "admit: {:?}", r.error);
    }
    assert_eq!(
        sorted_editors(&store, &format!("kbc:{kb}")).await,
        vec![principal(&alice)],
        "share + add_member are the owner's writes"
    );

    for (who, text) in [(&bob, "from bob"), (&carol, "from carol")] {
        let join = dispatch_as(&store, &bc, None, Some(who), kb_join_msg(kb), &mut docs).await;
        assert!(join.error.is_none(), "join: {:?}", join.error);
        let edit = kb_node_update_msg_as(kb, who, 0, text);
        let r = dispatch_as(&store, &bc, None, Some(who), edit, &mut docs).await;
        assert!(r.error.is_none(), "{who} edit: {:?}", r.error);
    }
    let refused = kb_node_update_msg_as(kb, &dave, 0, "from dave");
    let r = dispatch_as(&store, &bc, None, Some(&dave), refused, &mut docs).await;
    assert!(r.error.is_some(), "precondition: a non-member is refused");

    let node_doc = mae_sync::kb_node_doc_name(kb, "concept:n");
    assert_eq!(
        sorted_editors(&store, &node_doc).await,
        vec![principal(&bob), principal(&carol)]
    );
}

/// `sync/update` records the session's principal; a session with NO principal
/// is recorded as unauthenticated, never as a principal (not even an empty one).
#[tokio::test]
async fn sync_update_records_the_session_or_marks_it_unauthenticated() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let erin = fp("erin");
    let doc = "file:proj/notes.txt";
    for (who, client) in [(Some(erin.as_str()), 1u64), (None, 2u64)] {
        let mut ts = TextSync::with_client_id("", client);
        let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
            "params":{"doc":doc,"update":update_to_base64(&ts.insert(0, "x"))}});
        let r = dispatch_as(&store, &bc, None, who, msg, &mut docs).await;
        assert!(r.error.is_none(), "{who:?}: {:?}", r.error);
    }
    assert_eq!(
        sorted_editors(&store, doc).await,
        vec![
            principal(&erin),
            (EditorKind::Unauthenticated, "unauthenticated".to_string()),
        ]
    );
}

/// `sync/share` replaces a document, so it records the sharer.
#[tokio::test]
async fn sync_share_records_the_sharer() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let frank = fp("frank");
    let doc = "file:proj/shared.txt";
    let state = TextSync::with_client_id("", 3).insert(0, "shared");
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/share",
        "params":{"doc":doc,"update":update_to_base64(&state)}});
    let r = dispatch_as(&store, &bc, None, Some(&frank), msg, &mut docs).await;
    assert!(r.error.is_none(), "share: {:?}", r.error);
    assert_eq!(sorted_editors(&store, doc).await, vec![principal(&frank)]);
}

/// The scheduler's own lease claim is the daemon acting for no session: it is
/// recorded as the internal `scheduler` writer — not as the lease holder it
/// names, and not as an unauthenticated session.
#[tokio::test]
async fn a_scheduler_lease_claim_is_recorded_as_the_daemon() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (alice, holder) = (fp("alice"), fp("holder"));
    let kb = "kb-sched-record";
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&alice),
        kb,
        "alice",
        &mut docs,
    )
    .await;
    kb_lease::claim_lease_for_scheduler(&store, &bc, kb, "enrichment", &holder, 60)
        .await
        .expect("claim");
    assert_eq!(
        sorted_editors(&store, &format!("kbc:{kb}")).await,
        vec![
            (EditorKind::Internal, "scheduler".to_string()),
            principal(&alice),
        ]
    );
}

/// `sync/update` over the P2P transport, as the hub sees it.
async fn dispatch_p2p(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    principal: &str,
    msg: serde_json::Value,
    docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    handle_doc_request_inner(
        &msg.to_string(),
        store,
        &crate::quota::NoQuota,
        bc,
        std::time::Instant::now(),
        0,
        None,
        Some(principal),
        None,
        docs,
        Transport::P2p,
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        None,
    )
    .await
}

/// An owned, mesh-exposed KB with two editor members: Bob (an author) and a
/// relaying peer daemon. Returns `(store, bc, docs, bob, bob_fp, relay_fp)`.
async fn relay_kb(
    kb: &str,
) -> (
    Arc<DocStore>,
    SharedBroadcaster,
    HashSet<String>,
    mae_mcp::identity::Identity,
    String,
    String,
) {
    use mae_mcp::identity::Identity;
    let (store, bc, mut docs) = (test_doc_store(), test_broadcaster(), HashSet::new());
    let owner = Identity::generate("owner");
    let owner_fp = owner.fingerprint();
    store.set_signer(Arc::new(owner));
    let own = (Some("owner"), Some(owner_fp.as_str()));
    kb_share_as(&store, &bc, own.0, own.1, kb, "owner", &mut docs).await;
    let widen = load_coll(&store, kb)
        .await
        .set_transport_policy(mae_sync::kb::TransportPolicy::Both);
    let coll_doc = format!("kbc:{kb}");
    store
        .apply_update(&coll_doc, &widen, None, TEST_BY)
        .await
        .unwrap();
    let (bob, relay) = (
        Identity::generate("bob"),
        Identity::generate("relay-daemon"),
    );
    let (bob_fp, relay_fp) = (bob.fingerprint(), relay.fingerprint());
    for member in [&bob_fp, &relay_fp] {
        let add = kb_member_msg("kb/add_member", kb, member, Some("editor"));
        let r = dispatch_as(&store, &bc, own.0, own.1, add, &mut docs).await;
        assert!(r.error.is_none(), "admit: {:?}", r.error);
    }
    (store, bc, docs, bob, bob_fp, relay_fp)
}

/// `text` inserted under yrs client `cid`, signed by `author` as an ADR-036
/// content op on `kb`'s `concept:n`.
fn signed_edit(
    author: &mae_mcp::identity::Identity,
    kb: &str,
    cid: u64,
    text: &str,
) -> (Vec<u8>, mae_sync::content_ops::SignedContentOp) {
    use mae_sync::content_ops::{ContentOp, SignedContentOp};
    let upd = TextSync::with_client_id("", cid).insert(0, text);
    let op = ContentOp {
        kb_id: kb.to_string(),
        node_id: "concept:n".to_string(),
        base_sv: vec![],
        author: author.fingerprint(),
        epoch: 0,
        issued_at: 1_700_000_000,
    };
    let sig = op.sign(&author.secret_bytes(), &upd);
    let pubkey = author.public().to_bytes();
    let signed = SignedContentOp {
        op,
        payload: upd.clone(),
        sig,
        author_pubkey: pubkey,
    };
    (upd, signed)
}

/// What a signed relayed edit must leave behind: Bob as the principal, the
/// relay as a signed relay, and a Known author set of exactly Bob.
async fn assert_bob_relayed(store: &Arc<DocStore>, node_doc: &str, bob_fp: &str, relay_fp: &str) {
    assert_eq!(
        sorted_editors(store, node_doc).await,
        vec![
            principal(bob_fp),
            (EditorKind::SignedRelay, relay_fp.to_string())
        ]
    );
    let authors = store.authors_since_wal(node_doc, 0).await.unwrap();
    let bob: std::collections::BTreeSet<String> = [bob_fp.to_string()].into_iter().collect();
    assert_eq!(authors, crate::doc_editors::Authors::Known(bob));
}

/// A mesh peer forwarding a member's signed edit to the hub. The session
/// principal is the RELAYING DAEMON, which `kb_access` authorizes as a member —
/// but it is not the author: it is recorded as a signed relay, and the verified
/// signer is recorded as the principal. Before, the relay was recorded as the
/// principal and the author was lost.
#[tokio::test]
async fn a_relayed_signed_edit_records_the_relay_and_the_signer() {
    let kb = "kb-relay";
    let (store, bc, mut docs, bob, bob_fp, relay_fp) = relay_kb(kb).await;
    let cid = mae_sync::kb::derive_kb_client_id(&bob_fp, 0);
    let (upd, signed) = signed_edit(&bob, kb, cid, "bob's words");
    let node_doc = mae_sync::kb_node_doc_name(kb, "concept:n");
    let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
        "params":{"doc":node_doc,"kb_id":kb,"update":update_to_base64(&upd),
                  "content_header":signed.header_params()}});
    let r = dispatch_p2p(&store, &bc, &relay_fp, msg, &mut docs).await;
    assert!(r.error.is_none(), "relayed signed edit: {:?}", r.error);
    assert_bob_relayed(&store, &node_doc, &bob_fp, &relay_fp).await;
}

/// `kb/node_update` records a verified signer too. The op is built under the
/// relay's own yrs client id (the epoch fence binds it to the session) and
/// signed by Bob, so the signer differs from the session principal and only
/// the signature names him.
#[tokio::test]
async fn a_signed_node_update_records_its_signer() {
    let kb = "kb-relay-nu";
    let (store, bc, mut docs, bob, bob_fp, relay_fp) = relay_kb(kb).await;
    let cid = mae_sync::kb::derive_kb_client_id(&relay_fp, 0);
    let (upd, signed) = signed_edit(&bob, kb, cid, "bob's words");
    let msg = signed_node_update_msg(kb, "concept:n", &upd, &signed);
    let r = dispatch_p2p(&store, &bc, &relay_fp, msg, &mut docs).await;
    assert!(r.error.is_none(), "signed node update: {:?}", r.error);
    let node_doc = mae_sync::kb_node_doc_name(kb, "concept:n");
    assert_bob_relayed(&store, &node_doc, &bob_fp, &relay_fp).await;
}
