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
