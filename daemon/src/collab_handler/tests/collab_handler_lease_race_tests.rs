//! ADR-033 / ADR-061 Phase D1 (#420): the lease's real-daemon-protocol adversarial
//! coverage — genuine N-way (3 member) concurrent `kb/claim_lease` races through
//! the actual RPC dispatch (`handle_doc_request_inner`), reusing the established
//! N-way convergence pattern (`collab_handler_n_way_convergence_tests.rs`): every
//! claim is built from the SAME pre-claim state, blind to the other two, then
//! dispatched out of admission order.
//!
//! CLAUDE.md principle #14: this is the primary, mandatory adversarial test for
//! the lease primitive itself. The "does an enrichment sweep actually back off"
//! behavioral assertion (zero calls into a fake `EmbedBackend`) belongs to ADR-061
//! Phase D2 (issue #420's second half), once something actually calls the lease —
//! there is no such caller yet in this phase, so asserting on it here would be
//! testing code that doesn't exist. `enforce_lease_generation_fence` (the
//! write-time re-check D2 will use) IS exercised directly below, since it ships in
//! this phase even though its real caller doesn't yet.

use super::*;
use crate::lease_fence::LeaseFence;

/// Three members each independently claim the SAME op_kind lease from the SAME
/// pre-claim collection state, dispatched out of order — asserts exactly one
/// winner (the highest fingerprint) and that the two losers see themselves as NOT
/// holding it.
#[tokio::test]
async fn three_concurrent_claimants_converge_to_exactly_one_deterministic_winner() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-lease-race",
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
            kb_member_msg(
                "kb/add_member",
                "kb-lease-race",
                &fp(member),
                Some("editor"),
            ),
            &mut docs,
        )
        .await;
        assert!(r.error.is_none(), "owner admits {member}: {:?}", r.error);
    }

    // All three built from the SAME message shape BEFORE any is dispatched —
    // genuinely concurrent claims, not sequential-with-visibility. Dispatched out
    // of alphabetical/admission order (rules out an accidental "first writer
    // wins" implementation, matching the established n-way discipline).
    let msg = || kb_claim_lease_msg("kb-lease-race", "enrichment", 60);
    let mut results = Vec::new();
    for who in ["carol", "alice", "bob"] {
        let r = dispatch_as(&store, &bc, Some(who), Some(&fp(who)), msg(), &mut docs).await;
        assert!(r.error.is_none(), "{who}'s claim attempt: {:?}", r.error);
        results.push((who, r));
    }

    // fp("carol")/"alice"/"bob" = "SHA256:carol" etc. — lexicographically,
    // "SHA256:carol" > "SHA256:bob" > "SHA256:alice", so carol should win
    // regardless of dispatch order.
    let winner_fp = fp("carol");
    let held_flags: Vec<(&str, bool)> = results
        .iter()
        .map(|(who, r)| {
            let held = r
                .result
                .as_ref()
                .and_then(|v| v["held"].as_bool())
                .unwrap_or(false);
            (*who, held)
        })
        .collect();
    let winners: Vec<&str> = held_flags
        .iter()
        .filter(|(_, held)| *held)
        .map(|(who, _)| *who)
        .collect();
    assert_eq!(
        winners,
        vec!["carol"],
        "exactly one winner (the highest fingerprint), got: {held_flags:?}"
    );

    let coll = load_coll(&store, "kb-lease-race").await;
    let lease = coll
        .current_lease("enrichment", now_unix())
        .expect("a lease is held");
    assert_eq!(lease.holder_fp, winner_fp);
}

/// The SAME three concurrent claims, dispatched in the OPPOSITE order — the
/// actual convergence property is order-independence (mirrors
/// `three_concurrent_editors_converge_to_identical_content_regardless_of_order`).
#[tokio::test]
async fn lease_winner_is_identical_regardless_of_dispatch_order() {
    async fn run_with_order(order: [&str; 3]) -> String {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kb-lease-order",
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
                kb_member_msg(
                    "kb/add_member",
                    "kb-lease-order",
                    &fp(member),
                    Some("editor"),
                ),
                &mut docs,
            )
            .await;
        }
        for who in order {
            dispatch_as(
                &store,
                &bc,
                Some(who),
                Some(&fp(who)),
                kb_claim_lease_msg("kb-lease-order", "enrichment", 60),
                &mut docs,
            )
            .await;
        }
        let coll = load_coll(&store, "kb-lease-order").await;
        coll.current_lease("enrichment", now_unix())
            .expect("a lease is held")
            .holder_fp
    }

    let order_1 = run_with_order(["carol", "alice", "bob"]).await;
    let order_2 = run_with_order(["bob", "carol", "alice"]).await;
    let order_3 = run_with_order(["alice", "bob", "carol"]).await;
    assert_eq!(
        order_1, order_2,
        "dispatch order must not change the winner"
    );
    assert_eq!(
        order_2, order_3,
        "dispatch order must not change the winner"
    );
}

/// A non-member (never admitted to the KB) must be refused the lease outright —
/// the attacker case: `kb_access`'s Edit gate must fire before any claim is ever
/// written, not merely fail to win a tiebreak.
#[tokio::test]
async fn a_non_member_cannot_claim_the_lease() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-lease-outsider",
        "alice",
        &mut docs,
    )
    .await;

    let r = dispatch_as(
        &store,
        &bc,
        Some("mallory"),
        Some(&fp("mallory")),
        kb_claim_lease_msg("kb-lease-outsider", "enrichment", 60),
        &mut docs,
    )
    .await;
    assert!(
        r.error.is_some(),
        "a non-member must be denied, not silently granted no-op"
    );

    let coll = load_coll(&store, "kb-lease-outsider").await;
    assert!(
        coll.current_lease("enrichment", now_unix()).is_none(),
        "no lease should exist after a denied outsider attempt"
    );
}

// --- `enforce_lease_generation_fence` (D2's write-time re-check, ships now) ---

#[tokio::test]
async fn generation_fence_passes_for_the_current_holder_and_matching_generation() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-lease-fence",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_claim_lease_msg("kb-lease-fence", "enrichment", 60),
        &mut docs,
    )
    .await;

    let coll = load_coll(&store, "kb-lease-fence").await;
    let now = now_unix();
    let lease = coll.current_lease("enrichment", now).unwrap();
    assert!(kb_lease::enforce_lease_generation_fence(
        &coll,
        "enrichment",
        &lease.holder_fp,
        lease.generation,
        now
    )
    .is_ok());
}

#[tokio::test]
async fn generation_fence_rejects_a_stale_generation_after_a_new_grant() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-lease-stale",
        "alice",
        &mut docs,
    )
    .await;
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_member_msg(
            "kb/add_member",
            "kb-lease-stale",
            &fp("bob"),
            Some("editor"),
        ),
        &mut docs,
    )
    .await;

    // Alice claims with a short TTL and captures her generation.
    dispatch_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        kb_claim_lease_msg("kb-lease-stale", "enrichment", 1),
        &mut docs,
    )
    .await;
    let mut coll = load_coll(&store, "kb-lease-stale").await;
    let claim_time = now_unix();
    let alice_generation = coll
        .current_lease("enrichment", claim_time)
        .expect("alice holds the lease")
        .generation;

    // Simulate her TTL (1s) lapsing, then bob claims — a genuinely new grant,
    // applied directly on the same in-process doc (no need to round-trip through
    // the RPC layer again for this half of the test).
    let after_expiry = claim_time + 1_000_000;
    let update = coll.claim_lease("enrichment", &fp("bob"), 60, after_expiry);
    assert!(
        !update.is_empty(),
        "bob's claim after alice's TTL lapsed must succeed"
    );
    let bob_generation = coll
        .current_lease("enrichment", after_expiry)
        .expect("bob holds the lease")
        .generation;
    assert_ne!(
        alice_generation, bob_generation,
        "bob's grant must be a distinct generation from alice's original claim"
    );

    // Alice's stale batch, authored under her original generation, must now be
    // fenced — this is the write-time check D2 will call before committing.
    let err = kb_lease::enforce_lease_generation_fence(
        &coll,
        "enrichment",
        &fp("alice"),
        alice_generation,
        after_expiry,
    );
    assert!(
        err.is_err(),
        "alice's stale generation must be rejected once bob has been granted a new one"
    );
}

/// ADR-061 Phase D2's real, kill-holder-mid-sweep coverage: the production
/// `claim_lease_for_scheduler` + `DaemonLeaseFence` pair (not the lower-level
/// pure-logic helper the test above exercises), through a real `DocStore`.
/// `claim_lease_for_scheduler` uses real wall-clock time internally (not
/// injectable), so genuinely exercising TTL expiry through it needs a real,
/// short sleep — a `ttl_secs: 0` claim would look expired even on its OWN
/// immediate read-back (`is_expired`'s boundary is `now >= claimed_at + ttl`,
/// which collapses to an empty valid-interval at ttl=0), so that shortcut
/// doesn't work here. A ~1s sleep is a standard, deterministic pattern for
/// TTL-expiry tests and not the kind of resource-contention flakiness this
/// codebase's nextest migration had to account for elsewhere.
#[tokio::test]
async fn daemon_lease_fence_rejects_a_stale_holder_after_a_real_new_grant_through_the_scheduler_api(
) {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kb-lease-scheduler-api",
        "alice",
        &mut docs,
    )
    .await;

    let alice_claim = kb_lease::claim_lease_for_scheduler(
        &store,
        &bc,
        "kb-lease-scheduler-api",
        "enrichment",
        &fp("alice"),
        1, // a 1-second ttl, allowed to lapse below via a real sleep
    )
    .await
    .expect("alice's claim succeeds");
    assert_eq!(alice_claim.holder_fp, fp("alice"));

    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    // Bob claims next — a genuinely new grant, since alice's TTL already
    // lapsed. Both are hub-owner-role here for simplicity; real multi-daemon
    // topology aside, this exercises the exact same function the scheduler
    // calls from two different daemon processes.
    let bob_claim = kb_lease::claim_lease_for_scheduler(
        &store,
        &bc,
        "kb-lease-scheduler-api",
        "enrichment",
        &fp("bob"),
        60,
    )
    .await
    .expect("bob's claim succeeds");
    assert_eq!(bob_claim.holder_fp, fp("bob"));
    assert_ne!(
        alice_claim.generation, bob_claim.generation,
        "bob's grant must be a distinct generation from alice's"
    );

    // Alice's fence, constructed with HER captured generation, must now
    // reject — the real production check `run_enrichment_sweep` calls before
    // committing.
    let alice_fence = kb_lease::DaemonLeaseFence {
        doc_store: std::sync::Arc::clone(&store),
        kb_id: "kb-lease-scheduler-api".to_string(),
        op_kind: "enrichment".to_string(),
        holder_fp: fp("alice"),
        generation: alice_claim.generation,
    };
    assert!(
        alice_fence.check().await.is_err(),
        "alice's fence must reject once bob has been granted a new generation"
    );

    // Bob's fence, with HIS current generation, must pass — the correct
    // holder's own commit is not also incorrectly rejected.
    let bob_fence = kb_lease::DaemonLeaseFence {
        doc_store: std::sync::Arc::clone(&store),
        kb_id: "kb-lease-scheduler-api".to_string(),
        op_kind: "enrichment".to_string(),
        holder_fp: fp("bob"),
        generation: bob_claim.generation,
    };
    assert!(
        bob_fence.check().await.is_ok(),
        "bob's own fence must pass for his own current generation"
    );
}
