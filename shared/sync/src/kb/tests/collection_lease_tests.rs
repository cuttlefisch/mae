//! `KbCollectionDoc` ADR-033 advisory lease tests (mirrors `collection_lease.rs`):
//! claim/renew/expire semantics, the deterministic tiebreak under genuine
//! concurrent multi-peer claims, and the generation fencing token.

use super::*;

#[test]
fn fresh_claim_is_granted_at_generation_one() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    assert!(
        coll.current_lease("enrichment", 1000).is_none(),
        "no claim yet"
    );
    let update = coll.claim_lease("enrichment", "fp:alice", 60, 1000);
    assert!(!update.is_empty(), "a fresh claim must produce a delta");
    let lease = coll
        .current_lease("enrichment", 1000)
        .expect("claim granted");
    assert_eq!(lease.holder_fp, "fp:alice");
    assert_eq!(lease.generation, 1);
}

#[test]
fn renewal_by_the_current_holder_keeps_the_same_generation() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.claim_lease("enrichment", "fp:alice", 60, 1000);
    let gen1 = coll.current_lease("enrichment", 1000).unwrap().generation;

    // Renew well before expiry (now=1010, ttl started at 1000+60=1060).
    coll.claim_lease("enrichment", "fp:alice", 60, 1010);
    let lease = coll.current_lease("enrichment", 1010).unwrap();
    assert_eq!(
        lease.generation, gen1,
        "renewal must not advance generation"
    );
    assert_eq!(lease.holder_fp, "fp:alice");
}

#[test]
fn a_different_holder_cannot_claim_while_the_current_lease_is_unexpired() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.claim_lease("enrichment", "fp:bbbbb", 60, 1000); // higher fp, claims first
    let update = coll.claim_lease("enrichment", "fp:aaaaa", 60, 1005); // lower fp, tries next
    assert!(
        update.is_empty(),
        "a lower-fingerprint challenger must be refused while the holder is unexpired"
    );
    let lease = coll.current_lease("enrichment", 1005).unwrap();
    assert_eq!(
        lease.holder_fp, "fp:bbbbb",
        "original (higher-fp) holder keeps the lease"
    );
}

#[test]
fn a_higher_fingerprint_challenger_wins_and_advances_the_generation() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.claim_lease("enrichment", "fp:aaaaa", 60, 1000); // lower fp claims first
    let gen1 = coll.current_lease("enrichment", 1000).unwrap().generation;
    let update = coll.claim_lease("enrichment", "fp:zzzzz", 60, 1005); // higher fp challenges
    assert!(
        !update.is_empty(),
        "a higher-fingerprint challenger must win the tiebreak"
    );
    let lease = coll.current_lease("enrichment", 1005).unwrap();
    assert_eq!(lease.holder_fp, "fp:zzzzz");
    assert!(
        lease.generation > gen1,
        "a new grant (not a renewal) must advance the generation"
    );
}

#[test]
fn an_expired_claim_can_be_claimed_by_anyone_regardless_of_fingerprint() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.claim_lease("enrichment", "fp:zzzzz", 10, 1000); // ttl=10, expires at 1010
    let update = coll.claim_lease("enrichment", "fp:aaaaa", 60, 1011); // lower fp, but prior expired
    assert!(
        !update.is_empty(),
        "an expired claim must not block a new one even from a lower fingerprint"
    );
    let lease = coll.current_lease("enrichment", 1011).unwrap();
    assert_eq!(lease.holder_fp, "fp:aaaaa");
}

#[test]
fn current_lease_returns_none_once_the_ttl_lapses() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.claim_lease("enrichment", "fp:alice", 10, 1000);
    assert!(
        coll.current_lease("enrichment", 1005).is_some(),
        "still within ttl"
    );
    assert!(
        coll.current_lease("enrichment", 1011).is_none(),
        "ttl lapsed ⇒ no current holder"
    );
}

#[test]
fn distinct_op_kinds_have_independent_leases() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.claim_lease("enrichment", "fp:alice", 60, 1000);
    assert!(
        coll.current_lease("embedding_rebuild", 1000).is_none(),
        "a claim on one op_kind must not leak into another"
    );
}

/// The genuine CRDT-safety property: two peers (mirroring two independent
/// daemons) each claim the SAME op_kind from the SAME base state, blind to each
/// other's write (neither has synced yet) — then their updates are exchanged and
/// merged both ways. Both peers must converge to the IDENTICAL winner (the
/// higher fingerprint), not whatever yrs's internal merge order would pick if the
/// claim were a single LWW key instead of per-attempt entries.
#[test]
fn two_concurrent_daemon_claims_converge_to_the_same_deterministic_winner() {
    let mut coll_a = KbCollectionDoc::new_with_client_id("KB1", "alice", 1);
    let base = coll_a.encode_state();
    let mut coll_b = KbCollectionDoc::from_bytes(&base).unwrap();

    // Both claim from the SAME base state, at the same instant, blind to each other.
    // Fingerprint strings deliberately NOT named "a"/"b" to match their variable
    // names — chosen so the lexicographic winner is unambiguous by inspection
    // ("fp:zzz" > "fp:aaa"), avoiding the trap of a variable name implying an
    // ordering its string value doesn't actually have.
    let u_a = coll_a.claim_lease("enrichment", "fp:zzz-daemon", 60, 1000);
    let u_b = coll_b.claim_lease("enrichment", "fp:aaa-daemon", 60, 1000);
    assert!(
        !u_a.is_empty() && !u_b.is_empty(),
        "both attempts write a claim entry"
    );

    // Exchange and merge.
    coll_a.apply_update(&u_b).unwrap();
    coll_b.apply_update(&u_a).unwrap();

    let winner_a = coll_a.current_lease("enrichment", 1000).unwrap();
    let winner_b = coll_b.current_lease("enrichment", 1000).unwrap();
    assert_eq!(
        winner_a, winner_b,
        "both peers must derive the identical winning claim after merging"
    );
    assert_eq!(
        winner_a.holder_fp, "fp:zzz-daemon",
        "the deterministic tiebreak (highest fingerprint) must decide the winner, \
         not whichever write yrs's internal merge order happened to apply last"
    );
}

/// Same property, but merged in the OPPOSITE order — the actual convergence
/// guarantee is order-independence, not merely "no data loss" (see
/// `collab_handler_n_way_convergence_tests.rs`'s identical discipline).
#[test]
fn two_concurrent_daemon_claims_converge_regardless_of_merge_order() {
    fn run_with_order(apply_a_first: bool) -> LeaseClaim {
        let mut coll_a = KbCollectionDoc::new_with_client_id("KB1", "alice", 1);
        let base = coll_a.encode_state();
        let mut coll_b = KbCollectionDoc::from_bytes(&base).unwrap();

        let u_a = coll_a.claim_lease("enrichment", "fp:zzz-daemon", 60, 1000);
        let u_b = coll_b.claim_lease("enrichment", "fp:aaa-daemon", 60, 1000);

        if apply_a_first {
            coll_b.apply_update(&u_a).unwrap();
        } else {
            coll_a.apply_update(&u_b).unwrap();
        }
        // Whichever received the other's update first now has the full merged state.
        let merged = if apply_a_first { &coll_b } else { &coll_a };
        merged.current_lease("enrichment", 1000).unwrap()
    }

    let first = run_with_order(true);
    let second = run_with_order(false);
    assert_eq!(
        first, second,
        "the winning claim must be identical regardless of merge order"
    );
}
