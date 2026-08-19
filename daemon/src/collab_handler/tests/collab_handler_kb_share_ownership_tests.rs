//! ADVERSARIAL (ADR-105 D5 / finding E): a KB id belongs to whoever shared it
//! first, and is not claimable by whoever asks second.
//!
//! Split from `collab_handler_kb_lifecycle_tests.rs` to stay under the structural
//! ceiling. The oracle in these tests is deliberately the OWNER and the CONTENT,
//! never the response — a refused-but-applied share passes a response-only check.

use super::*;

/// ADVERSARIAL (ADR-105 D5 / finding E): a KB id is not claimable by whoever
/// asks second.
///
/// `kb/share` preserved an existing collection rather than clobbering it (B-12,
/// correct — it holds the durable membership) and then subscribed the caller to it
/// regardless of who owned it. So a second principal sharing an id someone else
/// already owned got a success response and a subscription to a stranger's KB. The
/// id was claimed first-come-first-served and held forever, since `kb/unregister`
/// removes only metadata and the collection doc survives idle eviction.
///
/// That is the mechanism behind finding F: every editor's primary was called
/// "default", so the first tenant to connect to a shared daemon owned `kbc:default`
/// and every later tenant's primary share landed here.
///
/// D4's minted ids should make this unreachable, which is exactly why it must be
/// LOUD if reached — a duplicate id after D4 means two mints collided or a client
/// supplied an id it did not mint.
#[tokio::test]
async fn kb_share_refuses_an_id_owned_by_another_principal() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut alice_docs = HashSet::new();
    let mut mallory_docs = HashSet::new();

    let kb = "contested-id";
    let node = "concept:architecture";

    let alice_node = make_test_node(node, "Architecture", "ALICE-ORIGINAL", &[]);
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned(kb, "Alice's KB", "alice");
    coll.add_node(node, node);
    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"kb/share",
        "params":{
            "kb_id": kb, "name": "Alice's KB", "creator": "alice",
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": [{"id": node, "state": update_to_base64(&alice_node)}],
        }
    });
    let r = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        msg,
        &mut alice_docs,
    )
    .await;
    assert!(r.error.is_none(), "alice's first share must succeed: {r:?}");

    // Mallory shares the SAME id, with her own collection naming herself owner.
    let mallory_node = make_test_node(node, "Architecture", "MALLORY-WAS-HERE", &[]);
    let mut m_coll = mae_sync::kb::KbCollectionDoc::new_owned(kb, "Mallory's KB", "mallory");
    m_coll.add_node(node, node);
    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":2,"method":"kb/share",
        "params":{
            "kb_id": kb, "name": "Mallory's KB", "creator": "mallory",
            "collection_state": update_to_base64(&m_coll.encode_state()),
            "nodes": [{"id": node, "state": update_to_base64(&mallory_node)}],
        }
    });
    let r = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        msg,
        &mut mallory_docs,
    )
    .await;

    let err = r.error.as_ref().unwrap_or_else(|| {
        panic!("sharing an id owned by another principal must be refused, got: {r:?}")
    });
    assert!(
        err.message.contains(kb),
        "the refusal must name the contested id: {}",
        err.message
    );
    // The CODE is the contract, not the prose. The editor's recovery (discard an
    // id it minted but never got confirmed, mint a fresh one, retry) branches on
    // this value — if it drifts, that recovery silently stops running and the KB
    // becomes permanently unshareable instead.
    assert_eq!(
        err.code,
        mae_mcp::protocol::KB_ID_OWNED_BY_ANOTHER,
        "an ownership refusal must be distinguishable from any other failure by \
         code alone: {err:?}"
    );

    // The response is not the oracle — a refused-but-applied share would pass a
    // response-only check. Assert on the OWNER and the CONTENT.
    let coll_after = load_collection(&store, kb).await.expect("collection loads");
    assert_eq!(
        coll_after.owner(),
        fp("alice"),
        "the refused share rebound the collection's owner"
    );
    assert!(
        !mallory_docs.contains(&format!("kbc:{kb}")),
        "a refused share must not subscribe the caller to the owner's collection"
    );

    let body = read_node_body(&store, &bc, "alice", kb, node, &mut alice_docs).await;
    assert!(
        body.contains("ALICE-ORIGINAL") && !body.contains("MALLORY-WAS-HERE"),
        "the refused share overwrote the owner's node content: {body:?}"
    );
}

/// The non-vacuity control. D5 must refuse a DIFFERENT owner, not re-sharing as
/// such — an owner reconnecting and re-sharing their own KB is the ordinary case
/// B-12 exists to serve, and breaking it would break every reconnect.
#[tokio::test]
async fn an_owner_can_still_reshare_their_own_kb() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let kb = "alices-kb";
    let node = "concept:architecture";
    let n = make_test_node(node, "Architecture", "ALICE-ORIGINAL", &[]);
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned(kb, "Alice's KB", "alice");
    coll.add_node(node, node);
    let payload = |id: u32| {
        serde_json::json!({
            "jsonrpc":"2.0","id":id,"method":"kb/share",
            "params":{
                "kb_id": kb, "name": "Alice's KB", "creator": "alice",
                "collection_state": update_to_base64(&coll.encode_state()),
                "nodes": [{"id": node, "state": update_to_base64(&n)}],
            }
        })
    };

    for attempt in 1..=2u32 {
        let r = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            payload(attempt),
            &mut docs,
        )
        .await;
        assert!(
            r.error.is_none(),
            "alice re-sharing her OWN kb (attempt {attempt}) must succeed — this is \
             the reconnect path: {r:?}"
        );
    }
}

/// ADVERSARIAL (ADR-105): asking about a KB must not CREATE it.
///
/// `load_collection` reached the doc store through `encode_state_and_sv`, which
/// goes through `get_or_create` — so every read of an unknown KB materialized an
/// empty `kbc:{kb_id}`, and that function's own "not found" error was unreachable.
///
/// The consequence is a pre-squat, and it defeats D5 rather than being caught by
/// it. Mallory joins an id nobody has shared: the collection springs into
/// existence with NO owner, and she is recorded pending on it. When the real owner
/// later shares that id, an ownerless collection reads as merely "unowned", so D5
/// allows it — and ADR-020 B-12's preserve-don't-clobber branch then keeps the
/// squatted empty collection and discards the owner's real one, genesis, owner
/// binding, members and all. The owner ends up owning nothing, with a stranger's
/// pending request already inside.
#[tokio::test]
async fn joining_an_unshared_kb_neither_succeeds_nor_creates_it() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut mallory_docs = HashSet::new();

    let kb = "not-yet-shared";
    let r = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        kb_join_msg(kb),
        &mut mallory_docs,
    )
    .await;

    assert!(
        r.error.is_some(),
        "joining a KB nobody has shared must fail, not record a pending request \
         against a KB that does not exist: {r:?}"
    );
    assert!(
        !store.has_doc(&format!("kbc:{kb}")).await,
        "a refused join still materialized the collection — that is the squat"
    );
}

/// The consequence made observable end to end: after a refused join, the real
/// owner's share must land with THEM as owner.
///
/// The oracle is the owner field, not the share's return value. Before the fix the
/// share still "succeeded" — it just preserved the squatted collection and threw
/// the owner's away, so a response-only check sees nothing wrong.
#[tokio::test]
async fn a_refused_join_cannot_pre_empt_the_real_owners_share() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut mallory_docs = HashSet::new();
    let mut alice_docs = HashSet::new();

    let kb = "contested-before-share";
    let _ = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        kb_join_msg(kb),
        &mut mallory_docs,
    )
    .await;

    let node = "concept:architecture";
    let n = make_test_node(node, "Architecture", "ALICE-ORIGINAL", &[]);
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned(kb, "Alice's KB", "alice");
    coll.add_node(node, node);
    let msg = serde_json::json!({
        "jsonrpc":"2.0","id":1,"method":"kb/share",
        "params":{
            "kb_id": kb, "name": "Alice's KB", "creator": "alice",
            "collection_state": update_to_base64(&coll.encode_state()),
            "nodes": [{"id": node, "state": update_to_base64(&n)}],
        }
    });
    let r = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        msg,
        &mut alice_docs,
    )
    .await;
    assert!(r.error.is_none(), "alice's share must succeed: {r:?}");

    let coll_after = load_collection(&store, kb).await.expect("collection loads");
    assert_eq!(
        coll_after.owner(),
        fp("alice"),
        "the owner's collection was discarded in favour of a pre-created one, so \
         the KB has no real owner"
    );
    assert!(
        coll_after.pending().is_empty(),
        "a stranger's pending request survived into the owner's KB: {:?}",
        coll_after.pending()
    );
}

/// The must-exist guard above must not mistake an idle-EVICTED KB for a missing
/// one.
///
/// A collection doc is durable (ADR-032 A2): memory-evicted when idle, kept on
/// disk, lazy-reloaded. `has_doc` looks only in memory, so guarding with it turns
/// "nobody has touched this KB recently" into "no such KB" — an intermittent
/// outage on exactly the KBs a busy daemon evicts first, and one that would look
/// like a flake rather than a bug. `has_durable_doc` exists for this distinction
/// and says so in its own doc comment.
///
/// Written because the first version of that guard used `has_doc` and the entire
/// daemon suite still passed: nothing else forces eviction, so the defect was
/// invisible until eviction is driven deliberately. Uses `share_doc` +
/// `evict_idle(0)` directly — the technique `durable_kb_doc_survives_idle_eviction`
/// established — rather than going through `kb/share`, whose connected-client
/// bookkeeping keeps the doc pinned and would make the eviction silently not happen.
#[tokio::test]
async fn an_idle_evicted_kb_is_still_found() {
    let store = test_doc_store();

    let kb = "evicted-but-real";
    let node = "concept:architecture";
    let mut coll = mae_sync::kb::KbCollectionDoc::new_owned(kb, "Alice's KB", "alice");
    coll.add_node(node, node);
    store
        .share_doc(&format!("kbc:{kb}"), &coll.encode_state())
        .await
        .unwrap();
    store
        .track_client_disconnect(&format!("kbc:{kb}"))
        .await
        .unwrap();

    let evicted = store.evict_idle(0).await;
    assert!(
        evicted.contains(&format!("kbc:{kb}")),
        "precondition: the collection must actually be evicted, or this test proves \
         nothing (evicted: {evicted:?})"
    );
    assert!(
        !store.has_doc(&format!("kbc:{kb}")).await,
        "precondition: and it must be out of MEMORY"
    );

    let reloaded = load_collection(&store, kb)
        .await
        .expect("an evicted-but-durable KB must still load — it is on disk");
    assert!(
        reloaded.has_node(node),
        "the reloaded collection must be the REAL one, not a fresh empty doc: {:?}",
        reloaded.list_nodes()
    );
}

#[tokio::test]
async fn kb_join_of_a_kb_that_was_never_shared_is_refused() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let msg = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/join",
        "params": { "kb_id": "nonexistent-kb" }
    });
    let resp = handle_doc_request(
        &msg.to_string(),
        &store,
        &bc,
        std::time::Instant::now(),
        0,
        &mut HashSet::new(),
    )
    .await;

    // ADR-105: this used to assert the OPPOSITE — "server creates empty doc on read
    // (get_or_create semantics), so this succeeds but returns 0 nodes". That was the
    // defect written down as the expectation: a read brought the KB into existence,
    // which let anyone pre-squat an id by joining it, and the real owner's later
    // share was then merged into the squatted collection rather than replacing it.
    // Joining a KB nobody has shared is an error, and the collection must not exist
    // afterwards.
    assert!(
        resp.error.is_some(),
        "joining a KB that was never shared must fail, not mint an empty one: {resp:?}"
    );
    assert!(
        !store.has_durable_doc("kbc:nonexistent-kb").await,
        "the refused join still materialized the collection"
    );
}
