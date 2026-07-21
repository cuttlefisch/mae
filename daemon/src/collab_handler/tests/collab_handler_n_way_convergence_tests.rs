//! Round-4 item 3: a genuine ≥3-member content-sync convergence test through the
//! REAL daemon protocol dispatch (`handle_doc_request_inner` / `kb/node_update`), not
//! the raw `TextSync`/yrs layer directly.
//!
//! `#265` and CLAUDE.md principle #14 both name the same gap: a 3-principal
//! convergence test already exists for *governance/quorum derivation*
//! (`quorum_requires_two_distinct_owners_to_revoke`), and `shared/sync/tests/
//! crdt_stress.rs` has genuine N-way (100-client) convergence at the raw `TextSync`
//! layer — but nothing drove ≥3 members' CONCURRENT `kb/node_update`s through the
//! real daemon dispatch (membership/epoch/relay semantics included) and asserted
//! convergence. Every existing content-update test in this suite is
//! single-writer-then-reader.
//!
//! "Concurrent" here means each member computes their insert from the SAME shared
//! base state, blind to the other two members' edits (mirroring
//! `crdt_stress.rs`'s `stress_100_clients_single_doc`) — not one after another with
//! each seeing the prior edit first.

use super::*;

/// Three members (the owner + two editors) of the same KB each independently
/// insert distinct text at position 0 of the same node, blind to each other's
/// edits, submitted to the daemon in some order — asserts no edit is lost
/// (clobbered) by the CRDT merge.
#[tokio::test]
async fn three_concurrent_editors_converge_without_losing_any_edit() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-converge",
        "alice",
        &mut docs,
    )
    .await;
    for member in ["bob", "carol"] {
        let r = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kb-converge", &fp(member), Some("editor")),
            &mut docs,
        )
        .await;
        assert!(r.error.is_none(), "owner admits {member}: {:?}", r.error);
    }

    // Each principal's CURRENT epoch (a fresh grant is epoch 0, but read it live
    // rather than assume — matches the established pattern in
    // collab_handler_viewer_epoch_tests.rs).
    let (kbc_state, _) = store.encode_state_and_sv("kbc:kb-converge").await.unwrap();
    let coll = KbCollectionDoc::from_bytes(&kbc_state).unwrap();
    let epoch_of = |p: &str| coll.epoch_of(&fp(p));

    // All three messages are built from independent, position-0 inserts BEFORE any
    // of them is dispatched — genuinely concurrent, not sequential.
    let alice_msg = kb_node_update_msg_as("kb-converge", &fp("alice"), epoch_of("alice"), "ALICE");
    let bob_msg = kb_node_update_msg_as("kb-converge", &fp("bob"), epoch_of("bob"), "BOB");
    let carol_msg = kb_node_update_msg_as("kb-converge", &fp("carol"), epoch_of("carol"), "CAROL");

    // Dispatch out of alphabetical/admission order — the daemon has no reason to
    // care, but it rules out an accidental "first writer wins" implementation.
    for (who, msg) in [("carol", carol_msg), ("alice", alice_msg), ("bob", bob_msg)] {
        let r = dispatch_as(&store, &bc, Some(who), Some(&fp(who)), msg, &mut docs).await;
        assert!(
            r.error.is_none(),
            "{who}'s concurrent edit applied: {:?}",
            r.error
        );
    }

    let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
    let merged = TextSync::from_state(&state).unwrap().content();
    for text in ["ALICE", "BOB", "CAROL"] {
        assert!(
            merged.contains(text),
            "concurrent edit '{text}' must survive the 3-way merge, got: {merged:?}"
        );
    }
}

/// The SAME three concurrent edits, submitted in two DIFFERENT orders against two
/// independent stores — asserts the merged result is byte-identical regardless of
/// dispatch order. This is the actual convergence property (not just "no data
/// loss"): a CRDT that applied edits order-DEPENDENTLY would pass the test above
/// (nothing lost) while still failing this one (different peers ending up with
/// different final content).
#[tokio::test]
async fn three_concurrent_editors_converge_to_identical_content_regardless_of_order() {
    async fn run_with_order(order: [&str; 3]) -> String {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();

        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kb-converge",
            "alice",
            &mut docs,
        )
        .await;
        for member in ["bob", "carol"] {
            dispatch_as(
                &store,
                &bc,
                Some("alice"),
                Some(&fp("alice")),
                kb_member_msg("kb/add_member", "kb-converge", &fp(member), Some("editor")),
                &mut docs,
            )
            .await;
        }

        let (kbc_state, _) = store.encode_state_and_sv("kbc:kb-converge").await.unwrap();
        let coll = KbCollectionDoc::from_bytes(&kbc_state).unwrap();
        let epoch_of = |p: &str| coll.epoch_of(&fp(p));

        let mut msgs: std::collections::HashMap<&str, serde_json::Value> = [
            (
                "alice",
                kb_node_update_msg_as("kb-converge", &fp("alice"), epoch_of("alice"), "ALICE"),
            ),
            (
                "bob",
                kb_node_update_msg_as("kb-converge", &fp("bob"), epoch_of("bob"), "BOB"),
            ),
            (
                "carol",
                kb_node_update_msg_as("kb-converge", &fp("carol"), epoch_of("carol"), "CAROL"),
            ),
        ]
        .into_iter()
        .collect();

        for who in order {
            let msg = msgs.remove(who).unwrap();
            let r = dispatch_as(&store, &bc, Some(who), Some(&fp(who)), msg, &mut docs).await;
            assert!(
                r.error.is_none(),
                "{who}'s concurrent edit applied: {:?}",
                r.error
            );
        }

        let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
        TextSync::from_state(&state).unwrap().content()
    }

    let forward = run_with_order(["alice", "bob", "carol"]).await;
    let reversed = run_with_order(["carol", "bob", "alice"]).await;
    let shuffled = run_with_order(["bob", "carol", "alice"]).await;

    assert_eq!(
        forward, reversed,
        "3-way merge must converge to identical content regardless of dispatch order"
    );
    assert_eq!(
        forward, shuffled,
        "3-way merge must converge to identical content regardless of dispatch order"
    );
}
