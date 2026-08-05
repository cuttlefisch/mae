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
        !mallory_docs.contains("kb:concept:b-secret"),
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
        mallory_docs.contains("kb:concept:a-own"),
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
    assert!(!victim_docs.contains("kb:concept:a-own"));
}
