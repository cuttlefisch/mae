//! `KbCollectionDoc` ADR-034 per-KB artifact-sharing settings tests (mirrors
//! `collection_artifacts.rs`).

use super::*;

#[test]
fn embedding_model_absent_by_default() {
    let coll = KbCollectionDoc::new("KB1", "alice");
    assert_eq!(coll.embedding_model(), None);
}

#[test]
fn embedding_model_round_trips() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.set_embedding_model("nomic-embed-text");
    assert_eq!(coll.embedding_model(), Some("nomic-embed-text".to_string()));
}

#[test]
fn chunk_version_defaults_to_zero() {
    let coll = KbCollectionDoc::new("KB1", "alice");
    assert_eq!(coll.chunk_version(), 0);
}

#[test]
fn chunk_version_round_trips() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.set_chunk_version(3);
    assert_eq!(coll.chunk_version(), 3);
}

#[test]
fn share_derived_artifacts_defaults_to_false() {
    // ADR-034: opt-in, matching TransportPolicy/Encryption's own "absent ⇒
    // conservative default" convention.
    let coll = KbCollectionDoc::new("KB1", "alice");
    assert!(!coll.share_derived_artifacts());
}

#[test]
fn share_derived_artifacts_round_trips_both_directions() {
    let mut coll = KbCollectionDoc::new("KB1", "alice");
    coll.set_share_derived_artifacts(true);
    assert!(coll.share_derived_artifacts());
    coll.set_share_derived_artifacts(false);
    assert!(!coll.share_derived_artifacts());
}

/// The genuine CRDT-safety property (matching `collection_lease_tests.rs`'s own
/// discipline): two peers set DIFFERENT settings concurrently from the same
/// base state, merge both ways, and must converge to an IDENTICAL final state
/// (yrs's own LWW resolves plain scalar keys deterministically — unlike the
/// lease's nested-map trap, a top-level scalar key is safe for concurrent
/// writes without eager seeding).
#[test]
fn concurrent_setting_changes_converge_identically_regardless_of_merge_order() {
    fn run_with_order(apply_a_first: bool) -> (Option<String>, i64) {
        let mut coll_a = KbCollectionDoc::new_with_client_id("KB1", "alice", 1);
        let base = coll_a.encode_state();
        let mut coll_b = KbCollectionDoc::from_bytes(&base).unwrap();

        let u_a = coll_a.set_embedding_model("model-a");
        let u_b = coll_b.set_chunk_version(7);

        if apply_a_first {
            coll_b.apply_update(&u_a).unwrap();
            (coll_b.embedding_model(), coll_b.chunk_version())
        } else {
            coll_a.apply_update(&u_b).unwrap();
            (coll_a.embedding_model(), coll_a.chunk_version())
        }
    }

    let first = run_with_order(true);
    let second = run_with_order(false);
    assert_eq!(
        first, second,
        "both peers must converge to the identical merged settings regardless of order"
    );
    assert_eq!(first.0, Some("model-a".to_string()));
    assert_eq!(first.1, 7);
}
