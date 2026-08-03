use super::*;
use crate::NodeSource;

#[test]
fn node_source_round_trips_through_the_store_for_every_variant() {
    // Locks in the two exhaustive NodeSource<->str match arms (db.rs,
    // util.rs) added for NodeSource::Promoted (#303) -- every variant,
    // including the new one, must persist and reload exactly.
    let (_tmp, store) = make_store();
    let variants = [
        NodeSource::Seed,
        NodeSource::UserOrg,
        NodeSource::Manual,
        NodeSource::Federation,
        NodeSource::Promoted,
    ];
    for (i, source) in variants.iter().enumerate() {
        let id = format!("user:round-trip-{i}");
        let node = Node::new(&id, "T", NodeKind::Note, "b").with_source(*source, 0);
        store.insert_node(&node).unwrap();
        let reloaded = store.get_node(&id).unwrap().expect("node must reload");
        assert_eq!(
            reloaded.source,
            Some(*source),
            "NodeSource::{source:?} must round-trip exactly"
        );
    }
}

#[test]
fn id_title_pairs_basic() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("concept:a", "Alpha", NodeKind::Concept, ""))
        .unwrap();
    store
        .insert_node(&Node::new("lesson:b", "Beta", NodeKind::Lesson, ""))
        .unwrap();

    let all = store.id_title_pairs(None).unwrap();
    assert_eq!(all.len(), 2);

    let concepts = store.id_title_pairs(Some("concept:")).unwrap();
    assert_eq!(concepts.len(), 1);
    assert_eq!(concepts[0].0, "concept:a");
    assert_eq!(concepts[0].1, "Alpha");
}

/// Deterministic coverage for `retry_on_transient_sqlite_busy` (`db.rs`)
/// — the real "database is locked" race it exists for is SQLite-internals-
/// timing-dependent (two SEPARATE real CI failures this session: first
/// surfacing as a panic, then -- after that was fixed -- surfacing again
/// via a totally different cozo-internal code path that returns a normal
/// `Err` instead, from the exact same underlying condition), so this
/// exercises the retry logic itself directly for BOTH shapes rather than
/// relying on getting lucky with real contention.
mod retry_on_transient_sqlite_busy_tests {
    use crate::cozo_store::db::{
        retry_on_transient_sqlite_busy, retry_on_transient_sqlite_busy_for_test,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn retries_and_eventually_succeeds_on_a_transient_busy_panic() {
        let attempts = AtomicU32::new(0);
        let result: Result<&str, String> = retry_on_transient_sqlite_busy(|| {
            let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 5 {
                panic!("database is locked");
            }
            Ok("ok")
        });
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            5,
            "must retry exactly until the first non-busy success, not more or fewer times"
        );
    }

    /// The NEW shape this fix specifically closes: the busy condition
    /// surfacing as a normal `Err`, not a panic (cozo's `initialize()` /
    /// `load_last_ids()` internal step, which propagates via `?` rather
    /// than the bootstrap step's `.unwrap()`) -- the real second CI failure
    /// this session hit after the panic-only version of this function
    /// shipped.
    #[test]
    fn retries_and_eventually_succeeds_on_a_transient_busy_err_not_just_a_panic() {
        let attempts = AtomicU32::new(0);
        let result: Result<&str, String> = retry_on_transient_sqlite_busy(|| {
            let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 5 {
                return Err("database is locked (code 5)".to_string());
            }
            Ok("ok")
        });
        assert_eq!(result.unwrap(), "ok");
        assert_eq!(attempts.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn recognizes_the_sqlite_busy_error_code_wording_too() {
        let attempts = AtomicU32::new(0);
        let result: Result<i32, String> = retry_on_transient_sqlite_busy(|| {
            if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                panic!("Error {{ code: Some(5), message: Some(\"SQLITE_BUSY\") }}");
            }
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
    }

    /// The non-panic mirror of the above: an `Err` (not panic) carrying the
    /// SQLITE_BUSY wording specifically, not just "database is locked".
    #[test]
    fn recognizes_the_sqlite_busy_error_code_wording_in_an_err_too() {
        let attempts = AtomicU32::new(0);
        let result: Result<i32, String> = retry_on_transient_sqlite_busy(|| {
            if attempts.fetch_add(1, Ordering::SeqCst) < 2 {
                return Err("Error { code: Some(5), message: Some(\"SQLITE_BUSY\") }".to_string());
            }
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn does_not_retry_an_unrelated_panic_real_corruption_must_fail_fast() {
        let attempts = AtomicU32::new(0);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retry_on_transient_sqlite_busy(|| -> Result<&str, String> {
                attempts.fetch_add(1, Ordering::SeqCst);
                panic!("disk I/O error: permission denied");
            })
        }));
        assert!(
            outcome.is_err(),
            "a non-busy panic must still propagate as a panic, not be silently swallowed into \
             an Err -- open_with_engine's own outer catch_unwind is what's responsible for \
             converting it to a clean error, matching this function's doc comment"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "an unrelated panic must fail on the FIRST attempt, never retried -- retrying a \
             genuinely corrupt/inaccessible store would just waste the caller's time before \
             failing anyway"
        );
    }

    /// The non-panic mirror: an unrelated `Err` (not a busy condition) must
    /// also fail fast on the first attempt, never retried.
    #[test]
    fn does_not_retry_an_unrelated_err_real_corruption_must_fail_fast() {
        let attempts = AtomicU32::new(0);
        let result: Result<&str, String> = retry_on_transient_sqlite_busy(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err("disk I/O error: permission denied".to_string())
        });
        assert!(result.is_err());
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    /// Issue #484: bounded by wall-clock time, not a fixed attempt count (see
    /// `retry_on_transient_sqlite_busy_with_deadline`'s own doc comment for why
    /// a fixed count under-provisions on a slower/more-loaded CI runner). Uses
    /// the test-only short-deadline seam so this stays fast -- a
    /// persistent-contention closure genuinely runs for the ENTIRE deadline by
    /// construction (that's the property under test), so exercising the real
    /// 20s production deadline directly here would make the suite slow for no
    /// added coverage.
    const TEST_DEADLINE: std::time::Duration = std::time::Duration::from_millis(200);

    #[test]
    fn gives_up_after_the_bounded_retry_budget_on_persistent_contention() {
        let attempts = AtomicU32::new(0);
        let start = std::time::Instant::now();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retry_on_transient_sqlite_busy_for_test(
                || -> Result<(), String> {
                    attempts.fetch_add(1, Ordering::SeqCst);
                    panic!("database is locked");
                },
                TEST_DEADLINE,
            )
        }));
        let elapsed = start.elapsed();
        assert!(
            outcome.is_err(),
            "persistent (never-clearing) contention must eventually surface as a propagated \
             panic, not loop forever"
        );
        assert!(
            attempts.load(Ordering::SeqCst) > 1,
            "must have actually retried at least once before giving up, not fail on the first \
             attempt like the unrelated-panic case does"
        );
        // Generous upper bound (deadline + one worst-case 8ms sleep + scheduling
        // slack), never a lower bound on real elapsed time -- the point is
        // "bounded, not infinite," not pinning an exact duration.
        assert!(
            elapsed < TEST_DEADLINE + std::time::Duration::from_millis(500),
            "must give up within the documented deadline budget, took {elapsed:?}"
        );
    }

    /// The non-panic mirror of the budget test: persistent contention
    /// surfacing as `Err` every time must also give up within budget.
    #[test]
    fn gives_up_after_the_bounded_retry_budget_on_persistent_err_contention() {
        let attempts = AtomicU32::new(0);
        let start = std::time::Instant::now();
        let result: Result<(), String> = retry_on_transient_sqlite_busy_for_test(
            || {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err("database is locked".to_string())
            },
            TEST_DEADLINE,
        );
        let elapsed = start.elapsed();
        assert!(result.is_err());
        assert!(
            attempts.load(Ordering::SeqCst) > 1,
            "must have actually retried at least once before giving up"
        );
        assert!(
            elapsed < TEST_DEADLINE + std::time::Duration::from_millis(500),
            "must give up within the documented deadline budget, took {elapsed:?}"
        );
    }
}
