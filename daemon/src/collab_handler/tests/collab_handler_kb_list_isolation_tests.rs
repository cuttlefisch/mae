//! ADVERSARIAL (#653): tenant ENUMERATION across the `kb/list` surface.
//!
//! Third sibling to `collab_handler_cross_kb_node_isolation_tests` (which pins
//! doc *addressing*) and `collab_handler_cross_kb_role_isolation_tests` (which
//! pins *role composition*). The property here is neither of those: membership
//! in one KB must not reveal that another KB **exists**.
//!
//! `handle_kb_list` took no principal and performed no `kb_access` call at all,
//! so it returned `list_kb_metas()` wholesale — the id, name and node count of
//! every KB co-hosted on the daemon — to any client authenticated to the collab
//! listener. On the multi-tenant daemon of ADR-060 that is a roster leak on its
//! own, and it also hands an attacker the exact `kb_id` values that the cross-KB
//! content paths key on (#571 for reads, #718 for writes).
//!
//! The oracle is deliberately two-sided. Asserting only that the victim's KB is
//! absent would pass just as well if the filter returned nothing at all, so each
//! test also asserts the caller still sees its *own* KB.

use super::*;

/// Extract the `kb_id`s from a `kb/list` response.
async fn kb_list_ids_as(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    docs: &mut HashSet<String>,
) -> Vec<String> {
    let resp = dispatch_as(
        store,
        bc,
        auth_label,
        auth_principal,
        serde_json::json!({"jsonrpc":"2.0","id":99,"method":"kb/list","params":{}}),
        docs,
    )
    .await;
    assert!(resp.error.is_none(), "kb/list errored: {:?}", resp.error);
    resp.result
        .expect("kb/list returned no result")
        .get("kbs")
        .and_then(|v| v.as_array())
        .expect("kb/list result has no `kbs` array")
        .iter()
        .filter_map(|m| m.get("kb_id").and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

#[tokio::test]
async fn kb_list_does_not_reveal_a_kb_the_caller_is_not_a_member_of() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    // Mallory OWNS her own KB — deliberately the strongest role available, so
    // that if an owner cannot enumerate a foreign KB, no lesser role can.
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

    // Victim owns a KB Mallory has no membership in whatsoever.
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

    let seen = kb_list_ids_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        &mut mallory_docs,
    )
    .await;

    assert!(
        !seen.iter().any(|id| id == "kb-b"),
        "mallory enumerated a KB she is not a member of: {seen:?}"
    );
    // The other half of the oracle: the filter must not simply return nothing.
    assert!(
        seen.iter().any(|id| id == "kb-a"),
        "mallory must still see her own KB, got: {seen:?}"
    );
}

/// The victim's side of the same daemon, to prove the filter is per-principal
/// rather than a global switch that happens to hide one KB.
#[tokio::test]
async fn kb_list_is_filtered_per_principal_not_globally() {
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

    let victim_seen = kb_list_ids_as(
        &store,
        &bc,
        Some("victim"),
        Some(&fp("victim")),
        &mut victim_docs,
    )
    .await;

    assert!(
        victim_seen.iter().any(|id| id == "kb-b"),
        "victim must see their own KB, got: {victim_seen:?}"
    );
    assert!(
        !victim_seen.iter().any(|id| id == "kb-a"),
        "victim enumerated mallory's KB: {victim_seen:?}"
    );
}

/// Regression guard for the single-user case: with authentication off there is
/// no principal to filter by, and `kb_access` short-circuits to `Allow` before
/// loading any collection. A local daemon must therefore see exactly what it saw
/// before this fix — the filter must not become a silent "everything is hidden".
#[tokio::test]
async fn kb_list_is_unfiltered_for_an_unauthenticated_caller() {
    let store = test_doc_store();
    let bc = test_broadcaster();

    let mut docs = HashSet::new();
    kb_share_as(&store, &bc, None, None, "kb-a", "local", &mut docs).await;
    kb_share_as(&store, &bc, None, None, "kb-b", "local", &mut docs).await;

    let seen = kb_list_ids_as(&store, &bc, None, None, &mut docs).await;

    for expected in ["kb-a", "kb-b"] {
        assert!(
            seen.iter().any(|id| id == expected),
            "an unauthenticated caller must still see {expected}, got: {seen:?}"
        );
    }
}
