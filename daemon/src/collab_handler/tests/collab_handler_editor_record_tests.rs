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
            (EditorKind::Unauthenticated, "session:0".to_string()),
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
