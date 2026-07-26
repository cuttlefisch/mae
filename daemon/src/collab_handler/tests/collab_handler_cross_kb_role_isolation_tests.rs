//! ADR-060 Phase D: tenant-boundary role composition.
//!
//! "Per-KB roles (ADR-017/ADR-018's Owner/Editor/Viewer model) continue to
//! compose normally across tenants a given principal happens to be a member
//! of -- a principal that is Owner on one tenant's KB and Viewer on
//! another's is still exactly that, unchanged by this ADR." This is a
//! pre-existing property of `kb_access`'s per-collection role derivation
//! (each `kbc:{kb_id}` collection's membership is its own independent
//! signed op-log -- ADR-060's Phase A/B addressing work never touched this
//! code at all, it lives entirely in `collab_handler/`, not
//! `daemon/src/handler.rs`). Verified here rather than assumed, per the
//! same principle-#15 discipline that resolved Phase B: no existing test in
//! this test suite explicitly proves a single principal holding two
//! DIFFERENT roles on two DIFFERENT KBs simultaneously doesn't leak the
//! stronger role across the boundary.

use super::*;

#[tokio::test]
async fn owner_of_one_kb_is_not_owner_of_another_kb_where_only_viewer() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    // bob owns his own KB outright.
    kb_share_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        "bobs-own-kb",
        "bob",
        &mut docs,
    )
    .await;
    let bobs_kb = load_coll(&store, "bobs-own-kb").await;
    assert_eq!(
        bobs_kb.role_of(&fp("bob")),
        Some(SyncRole::Owner),
        "sanity: bob is genuinely Owner of his own KB"
    );

    // alice owns a SEPARATE KB and approves bob as a mere Viewer on it.
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "alices-kb",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("alices-kb"),
        &mut docs,
    )
    .await;
    let approve = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_approve_msg("alices-kb", &fp("bob"), Some("viewer")),
        &mut docs,
    )
    .await;
    assert!(
        approve.error.is_none(),
        "alice approving bob as viewer must succeed: {:?}",
        approve.error
    );
    let alices_kb = load_coll(&store, "alices-kb").await;
    assert_eq!(
        alices_kb.role_of(&fp("bob")),
        Some(SyncRole::Viewer),
        "sanity: bob is genuinely only Viewer on alice's KB"
    );

    // The property under test: bob's real Owner status on his OWN KB must
    // never leak into alice's KB. Attempting an Owner-only action
    // (add_member) on alice's KB, authenticated as bob, must be denied --
    // exactly as if bob had no elevated role anywhere.
    let escalation_attempt = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_member_msg("kb/add_member", "alices-kb", &fp("carol"), Some("editor")),
        &mut docs,
    )
    .await;
    assert!(
        escalation_attempt.error.is_some(),
        "bob's Owner role on HIS OWN kb must not compose into Owner-level access on alice's kb \
         where he is only a Viewer -- this is the exact tenant-boundary role-composition \
         property ADR-060 Phase D names explicitly"
    );
    let alices_kb_after = load_coll(&store, "alices-kb").await;
    assert_eq!(
        alices_kb_after.role_of(&fp("carol")),
        None,
        "the smuggled add_member must not have taken effect"
    );

    // And the reverse must also hold: alice (a mere non-member of bob's own
    // KB) gets no elevated access there either, from any role she holds
    // elsewhere.
    let alice_on_bobs_kb = dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg("kb/add_member", "bobs-own-kb", &fp("dave"), Some("editor")),
        &mut docs,
    )
    .await;
    assert!(
        alice_on_bobs_kb.error.is_some(),
        "alice (Owner of her own kb, non-member of bob's) must not gain any access on bob's kb"
    );
}
