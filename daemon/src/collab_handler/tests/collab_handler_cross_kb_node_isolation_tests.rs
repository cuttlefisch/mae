//! ADVERSARIAL (#571): content addressing across tenants.
//!
//! Sibling to `collab_handler_cross_kb_role_isolation_tests`, which pins a
//! different property — that a ROLE held in one KB confers nothing in another.
//! This file pins the addressing property: the `DocStore` doc namespace is FLAT
//! (`kb:{node_id}`, no `kb_id` component), so authorizing a KB must not
//! authorize an arbitrary globally-addressed document.
//!
//! `kb/node_fetch` is the surface with the most to lose, because a successful
//! fetch does not merely return one response — it inserts the doc into
//! `session_docs` and `subscribe_doc`s the session, granting a STANDING live
//! feed of that node's future updates. A check placed after the fetch would
//! still pass a "the response was an error" assertion while leaking every
//! subsequent edit, so the subscription assertions below are the load-bearing
//! ones.

use super::*;

#[tokio::test]
async fn kb_node_fetch_cannot_read_a_node_belonging_to_another_kb() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Mallory OWNS kb-a. Deliberately the highest role available: if an owner
    // of one KB cannot reach another KB's node, no lesser role can either.
    let mut mallory_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;

    // Victim owns kb-b, with a node genuinely in its manifest.
    let mut victim_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        "kb-b",
        "victim",
        &mut victim_docs,
    )
    .await;
    let added = dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
            "params":{"kb_id":"kb-b","node_id":"concept:b-secret","title":"B"}}),
        &mut victim_docs,
    )
    .await;
    assert!(added.error.is_none(), "seed failed: {:?}", added.error);

    // Mallory's own node, so the positive control below is a real fetch.
    let added_a = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({
            "jsonrpc":"2.0","id":2,"method":"kb/collection_node_add",
            "params":{"kb_id":"kb-a","node_id":"concept:a-own","title":"A"}}),
        &mut mallory_docs,
    )
    .await;
    assert!(added_a.error.is_none(), "seed failed: {:?}", added_a.error);

    // --- THE ATTACK: authorized on kb-a, asking for kb-b's node ---
    let attack = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({
            "jsonrpc":"2.0","id":3,"method":"kb/node_fetch",
            "params":{"kb_id":"kb-a","node_id":"concept:b-secret"}}),
        &mut mallory_docs,
    )
    .await;

    assert!(
        attack.error.is_some(),
        "an owner of kb-a fetched kb-b's node: {:?}",
        attack.result
    );
    assert_eq!(
        attack.error.as_ref().unwrap().message,
        "node 'concept:b-secret' is not in KB 'kb-a'",
        "denial text must be exact, and must not reveal that the node exists elsewhere"
    );

    // THE LOAD-BEARING ASSERTION: no standing subscription was granted. This is
    // what pins the scope check ABOVE the `session_docs.insert` /
    // `subscribe_doc` block rather than merely before the response is built.
    assert!(
        !mallory_docs.contains("kbn:kb-b:concept:b-secret"),
        "refused fetch still subscribed the session to kb-b's node — every future \
         update to it would be delivered to Mallory"
    );

    // POSITIVE CONTROL: Mallory can still fetch a node genuinely in her own KB,
    // and THAT one does subscribe — so the guard is scoping, not just denying.
    let own = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        serde_json::json!({
            "jsonrpc":"2.0","id":4,"method":"kb/node_fetch",
            "params":{"kb_id":"kb-a","node_id":"concept:a-own"}}),
        &mut mallory_docs,
    )
    .await;
    assert!(
        own.error.is_none(),
        "owner can no longer fetch their own KB's node: {:?}",
        own.error
    );
    assert!(
        mallory_docs.contains("kbn:kb-a:concept:a-own"),
        "a legitimate fetch must still subscribe the session"
    );

    // SYMMETRY: the property must hold in both directions, or it is not isolation.
    let reverse = dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        serde_json::json!({
            "jsonrpc":"2.0","id":5,"method":"kb/node_fetch",
            "params":{"kb_id":"kb-b","node_id":"concept:a-own"}}),
        &mut victim_docs,
    )
    .await;
    assert!(
        reverse.error.is_some(),
        "the victim read Mallory's node — isolation is one-directional"
    );
    assert!(!victim_docs.contains("kbn:kb-a:concept:a-own"));
}

/// The same attack via the raw sync surface, which was ungated entirely.
///
/// `deny_kb_doc_read` existed and was correct — it was called from exactly TWO
/// of the paths that needed it (`sync/state_vector`, `sync/full_state`).
/// `sync/resync` returns the same bytes under a different method name, and
/// `sync/diff` returns them as a delta. Its own doc comment says KB content
/// "must be fetched via the access-gated `kb/node_fetch`"; these two were what
/// made that false.
///
/// `sync/resync` is the worse of the pair, for the reason this file's header
/// gives about `kb/node_fetch`: it also `subscribe_doc`s the session, so an
/// ungated call grants a STANDING feed of every future edit, not just one read.
#[tokio::test]
async fn raw_sync_methods_cannot_read_a_node_belonging_to_another_kb() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut mallory_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;

    let mut victim_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        "kb-b",
        "victim",
        &mut victim_docs,
    )
    .await;
    let added = dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
            "params":{"kb_id":"kb-b","node_id":"concept:b-secret","title":"B"}}),
        &mut victim_docs,
    )
    .await;
    assert!(added.error.is_none(), "seed failed: {:?}", added.error);

    for method in ["sync/resync", "sync/diff"] {
        let before = mallory_docs.len();
        let attack = dispatch_as(
            &store,
            &bc,
            Some("mallory"),
            Some(&fp("mallory")),
            serde_json::json!({
                "jsonrpc":"2.0","id":9,"method": method,
                "params":{"doc":"kbn:kb-b:concept:b-secret","sv":""}}),
            &mut mallory_docs,
        )
        .await;

        assert!(
            attack.error.is_some(),
            "{method} returned kb-b's node to an outsider: {:?}",
            attack.result
        );
        // The load-bearing half: a refusal that still subscribed would pass the
        // assertion above and keep leaking every subsequent edit.
        assert_eq!(
            mallory_docs.len(),
            before,
            "{method} subscribed the session to another KB's node despite refusing the read"
        );
    }
}

/// ADVERSARIAL: `sync/share` and `sync/awareness` are classified as gating
/// themselves (`DocScope::SelfGatedNamedDoc`), so the dispatcher lets them
/// through. An outsider aiming them at another KB's node or collection doc
/// must be refused, must not be subscribed to it, and must not be able to
/// replace it: a replaced `kbc:` doc is a replaced membership.
#[tokio::test]
async fn share_and_awareness_cannot_touch_another_kbs_documents() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut mallory_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;
    let mut victim_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        "kb-b",
        "victim",
        &mut victim_docs,
    )
    .await;
    let add = |node: &str| {
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
            "params":{"kb_id":"kb-b","node_id":node,"title":"B"}})
    };
    let added = dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        add("concept:b-secret"),
        &mut victim_docs,
    )
    .await;
    assert!(added.error.is_none(), "seed failed: {:?}", added.error);

    for doc in ["kbn:kb-b:concept:b-secret", "kbc:kb-b"] {
        for (method, extra) in [
            ("sync/share", serde_json::json!({"update": "AAA="})),
            (
                "sync/awareness",
                serde_json::json!({"state": {"cursor": 1}}),
            ),
        ] {
            let before = mallory_docs.len();
            let mut params = extra.clone();
            params["doc"] = serde_json::json!(doc);
            let attack = dispatch_as(
                &store,
                &bc,
                Some("mallory"),
                Some(&fp("mallory")),
                serde_json::json!({"jsonrpc":"2.0","id":9,"method":method,"params":params}),
                &mut mallory_docs,
            )
            .await;
            assert!(
                attack.error.is_some(),
                "{method} on {doc} accepted from an outsider: {:?}",
                attack.result
            );
            assert_eq!(
                mallory_docs.len(),
                before,
                "{method} subscribed an outsider to {doc}"
            );
        }
    }
    // A member's own cursors still relay: the gate is access, not a ban.
    let own = dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        serde_json::json!({"jsonrpc":"2.0","id":3,"method":"sync/awareness",
            "params":{"doc":"kbn:kb-b:concept:b-secret","state":{"cursor":1}}}),
        &mut victim_docs,
    )
    .await;
    assert!(
        own.error.is_none(),
        "a member's awareness on its own KB was refused: {:?}",
        own.error
    );
    // The load-bearing half for share: the victim still owns kb-b and can
    // still write to it, so its collection was not replaced.
    let again = dispatch_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        add("concept:b-two"),
        &mut victim_docs,
    )
    .await;
    assert!(
        again.error.is_none(),
        "kb-b's collection was replaced by an outsider: {:?}",
        again.error
    );
}

/// ADVERSARIAL: the same awareness, sent as a NOTIFICATION (no `id`), which
/// takes a different entry point and must apply the same access check. End
/// to end: the outsider subscribes to every event type, sends the
/// notification naming another KB's node, and the owner then writes; nothing
/// may reach it. A member's notification still relays.
#[tokio::test]
async fn awareness_notification_cannot_subscribe_an_outsider_to_another_kbs_node() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let (mut mallory_docs, mut victim_docs) = (HashSet::new(), HashSet::new());
    kb_share_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        "kb-a",
        "mallory",
        &mut mallory_docs,
    )
    .await;
    kb_share_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        "kb-b",
        "victim",
        &mut victim_docs,
    )
    .await;
    let node = "kbn:kb-b:concept:b-secret";

    let (mallory, victim) = (7u64, 8u64);
    let all = || vec!["sync_update".to_string(), "awareness_update".to_string()];
    // Registered as the daemon registers every collab session, then opted in
    // to every event type.
    let mut mallory_rx = register_session(&bc, mallory);
    let _victim_rx = register_session(&bc, victim);
    for sid in [mallory, victim] {
        for t in all() {
            bc.lock().unwrap().add_event_sub(sid, &t);
        }
    }
    let note = |doc: &str| {
        serde_json::json!({"jsonrpc":"2.0","method":"sync/awareness",
            "params":{"doc":doc,"state":{"cursor_row":0,"cursor_col":0}}})
        .to_string()
    };
    let send = |sid: u64, who: &'static str, docs: &mut HashSet<String>| {
        let (store, bc, msg) = (Arc::clone(&store), Arc::clone(&bc), note(node));
        let mut owned = std::mem::take(docs);
        async move {
            handle_doc_notification_inner(
                &msg,
                &store,
                &bc,
                sid,
                Some(who),
                Some(&fp(who)),
                Transport::Hub,
                &mut owned,
            )
            .await;
            owned
        }
    };

    mallory_docs = send(mallory, "mallory", &mut mallory_docs).await;
    assert!(
        !mallory_docs.contains(node),
        "the outsider's session tracked {node}"
    );

    // The owner writes to the node: the daemon broadcasts the update to the
    // doc's subscribers. The outsider must not be one of them.
    bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
        buffer_name: node.to_string(),
        update_base64: "AAA=".to_string(),
        wal_seq: 1,
        content_header: None,
    });
    assert!(
        mallory_rx.try_recv().is_err(),
        "an outsider received another KB's node update"
    );

    // A member's own cursor notification is still relayed and subscribed.
    victim_docs = send(victim, "victim", &mut victim_docs).await;
    assert!(
        victim_docs.contains(node),
        "a member's awareness notification was refused"
    );
}

/// ADVERSARIAL: a collab session that subscribes to `sync_update` and touches
/// no document at all receives no document's events. The broadcaster's
/// default treats a client with no document subscriptions as wanting every
/// document (the editor's MCP server relies on it); a daemon session must
/// not, since its documents are other KBs' content.
#[tokio::test]
async fn a_session_subscribed_to_no_document_receives_no_documents_events() {
    let bc = test_broadcaster();
    let mut rx = register_session(&bc, 11);
    bc.lock().unwrap().add_event_sub(11, "sync_update");
    for doc in ["kbn:any-kb:any-node", "kbc:any-kb", "notes.org"] {
        bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
            buffer_name: doc.to_string(),
            update_base64: "AAA=".to_string(),
            wal_seq: 1,
            content_header: None,
        });
    }
    assert!(
        rx.try_recv().is_err(),
        "a session with no documents received a document's update"
    );
    // Once it subscribes to a document (which every doc method gates on
    // access), that document's events do reach it.
    bc.lock().unwrap().subscribe_doc(11, "notes.org");
    bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
        buffer_name: "notes.org".to_string(),
        update_base64: "AAA=".to_string(),
        wal_seq: 2,
        content_header: None,
    });
    assert!(
        rx.try_recv().is_ok(),
        "a subscribed document's update did not arrive"
    );
}
