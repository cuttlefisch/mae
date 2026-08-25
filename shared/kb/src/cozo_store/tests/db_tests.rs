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

    /// sled's contention error, in the spelling sled 0.34 actually produces.
    /// Before the classifier was widened this matched none of the sqlite
    /// wording, so it was treated as permanent and failed on the FIRST
    /// attempt while sqlite got a 45s budget for contention identical in kind.
    /// That is the flake behind every sled->sqlite migration test: sled's
    /// exclusive dir lock is released during drop, not synchronously, so a
    /// seed-drop-reopen sequence loses the race on a loaded runner.
    #[test]
    fn a_sled_lock_contention_error_is_retried_not_treated_as_permanent() {
        for message in [
            "could not acquire lock on \"/tmp/x/db\": WouldBlock",
            "Os { code: 11, kind: WouldBlock, message: \"Resource temporarily unavailable\" }",
            "resource temporarily unavailable",
        ] {
            let attempts = AtomicU32::new(0);
            let result: Result<&str, String> = retry_on_transient_sqlite_busy(|| {
                let n = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                if n < 4 {
                    return Err(message.to_string());
                }
                Ok("ok")
            });
            assert_eq!(
                result.unwrap(),
                "ok",
                "sled contention must be retried, not classified permanent: {message}"
            );
            assert_eq!(attempts.load(Ordering::SeqCst), 4, "for: {message}");
        }
    }

    /// The negative case, and the one that constrains the fix: a lock that is
    /// GENUINELY held must still fail — and fail fast. sled's lock is
    /// exclusive, so persistent contention means another live handle exists,
    /// which the caller needs to be told about rather than blocked on for the
    /// full sqlite budget. Asserts the *bound*, not just the failure: the whole
    /// point of giving sled its own budget is that it does not inherit 45s.
    #[test]
    fn a_persistently_held_sled_lock_still_fails_and_does_not_wait_out_the_sqlite_budget() {
        let attempts = AtomicU32::new(0);
        let started = std::time::Instant::now();
        let result: Result<&str, String> = retry_on_transient_sqlite_busy(|| {
            attempts.fetch_add(1, Ordering::SeqCst);
            Err("could not acquire lock: WouldBlock".to_string())
        });
        let elapsed = started.elapsed();
        assert!(
            result.is_err(),
            "a genuinely held lock must not report success"
        );
        assert!(
            attempts.load(Ordering::SeqCst) > 1,
            "it should have retried at least once before giving up"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(20),
            "sled contention waited {elapsed:?} — it is inheriting sqlite's 45s budget \
             instead of its own much shorter one"
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

/// A busy store must be reported as **transient**, not as a durable storage
/// failure — that distinction is what lets a caller retry or degrade instead of
/// treating lock contention like disk corruption.
///
/// Without it, `KbStoreError` had three variants and a network partition was
/// indistinguishable from a corrupt file (C2).
#[test]
fn lock_contention_is_classified_transient_and_corruption_is_not() {
    // The conservative half matters most: a fatal error must NOT be transient,
    // or it would be retried to a deadline instead of failing fast.
    let durable = KbStoreError::Storage("CozoDB: disk I/O error".to_string());
    assert!(
        !durable.is_transient(),
        "a durable failure must never be retryable, or callers will retry forever"
    );

    let transient = KbStoreError::Transient("CozoDB: database is locked".to_string());
    assert!(transient.is_transient());

    // And it must say so in its own message, since that is what reaches a log.
    assert!(
        transient.to_string().contains("retry may help"),
        "the message must state retryability: {transient}"
    );
}

// ---------------------------------------------------------------------------
// D3 (#465/#466) — the four columns that could never hold a value.
//
// `origin_instance`, `assignee`, `due_date` and `sprint` were declared in the
// schema, read by the seeded stored views, and hardcoded to ""/0 by BOTH write
// paths. `view:sprint` filters `sprint != ""`, so it returned the empty set for
// every user, always.
// ---------------------------------------------------------------------------

fn node_with_props(id: &str, props: &[(&str, &str)]) -> Node {
    let mut n = Node::new(id, id, NodeKind::Task, "body");
    for (k, v) in props {
        n.properties.insert(k.to_string(), v.to_string());
    }
    n
}

fn column(store: &CozoKbStore, id: &str, col: &str) -> DataValue {
    let rows = store
        .run_immut_params(
            &format!("?[v] := id = $id, *nodes{{id, {col}: v}}"),
            super::super::util::btree_params([("id", super::super::util::dv_str(id))]),
        )
        .unwrap()
        .rows;
    rows.first().and_then(|r| r.first()).cloned().unwrap()
}

#[test]
fn assignee_and_sprint_are_derived_from_properties() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&node_with_props(
            "task:1",
            &[("assignee", "quintessa"), ("sprint", "S-42")],
        ))
        .unwrap();

    assert_eq!(
        column(&store, "task:1", "assignee").get_str(),
        Some("quintessa")
    );
    assert_eq!(column(&store, "task:1", "sprint").get_str(), Some("S-42"));
}

/// **The shipped view that never returned a row.**
///
/// A selective oracle: the assigned task must appear and the unassigned one must
/// not, so the test fails both on the old always-empty behaviour and on a fix
/// that returned everything.
#[test]
fn view_sprint_returns_rows_now_that_sprint_can_hold_a_value() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&node_with_props("task:in", &[("sprint", "S-1")]))
        .unwrap();
    store
        .insert_node(&node_with_props("task:out", &[]))
        .unwrap();

    let rows = store
        .run_immut(r#"?[id, sprint] := *nodes{id, sprint}, sprint != """#)
        .unwrap()
        .rows;
    let ids: Vec<&str> = rows.iter().filter_map(|r| r.first()?.get_str()).collect();
    assert_eq!(
        ids,
        vec!["task:in"],
        "only the node with a sprint may match -- this query returned NOTHING for \
         every user before D3"
    );
}

#[test]
fn due_date_parses_the_org_timestamp_shapes_org_actually_writes() {
    let (_tmp, store) = make_store();
    let expected = crate::activity::epoch_seconds_utc_midnight(2026, 8, 25);
    for (i, raw) in [
        "<2026-08-25 Tue>",
        "[2026-08-25]",
        "<2026-08-25 Tue 14:30>",
        "2026-08-25",
    ]
    .iter()
    .enumerate()
    {
        let id = format!("task:d{i}");
        store
            .insert_node(&node_with_props(&id, &[("deadline", raw)]))
            .unwrap();
        assert_eq!(
            column(&store, &id, "due_date").get_int(),
            Some(expected),
            "{raw} must parse to the same day"
        );
    }
}

/// A malformed timestamp must not fail the write — the node still lands, with no
/// deadline. Losing a whole node over one bad property value would be a far worse
/// outcome than losing the date.
#[test]
fn an_unparseable_deadline_is_absent_not_an_error() {
    let (_tmp, store) = make_store();
    for (i, raw) in ["<not a date>", "", "2026-13-45", "tomorrow"]
        .iter()
        .enumerate()
    {
        let id = format!("task:bad{i}");
        store
            .insert_node(&node_with_props(&id, &[("deadline", raw)]))
            .expect("a bad deadline must not fail the node write");
        assert_eq!(column(&store, &id, "due_date").get_int(), Some(0), "{raw}");
    }
}

#[test]
fn origin_instance_records_which_replica_holds_the_node() {
    let (_tmp, store) = make_store();
    store.insert_node(&node_with_props("task:o", &[])).unwrap();
    let got = column(&store, "task:o", "origin_instance");
    assert_eq!(
        got.get_str(),
        Some(store.instance_id().unwrap().as_str()),
        "origin_instance is not node content -- it is WHICH REPLICA holds this"
    );
}

/// The derived columns must be a pure function of CRDT truth, so a rebuild
/// reproduces them. This is the property that made "derive" the right answer
/// rather than four new CRDT keys.
#[test]
fn the_derived_columns_survive_a_rewrite_from_the_same_node() {
    let (_tmp, store) = make_store();
    let node = node_with_props("task:r", &[("assignee", "kit"), ("sprint", "S-9")]);
    store.insert_node(&node).unwrap();
    // Re-project the identical node, as `rebuild_kb` would.
    store.insert_node(&node).unwrap();
    assert_eq!(column(&store, "task:r", "assignee").get_str(), Some("kit"));
    assert_eq!(column(&store, "task:r", "sprint").get_str(), Some("S-9"));
}

/// **`created_at` must not be reset by a write, through EITHER door.**
///
/// `node_row` (bulk) already preserved it; `node_put_params` (single) did not, so
/// the same field meant different things depending on which path wrote it, and
/// `insert_node` destroyed node age outright.
#[test]
fn created_at_survives_a_rewrite_through_the_single_insert_path() {
    let (_tmp, store) = make_store();
    let mut node = Node::new("task:age", "Age", NodeKind::Task, "body");
    node.created_at = Some(1_000_000);
    store.insert_node(&node).unwrap();
    assert_eq!(
        column(&store, "task:age", "created_at").get_int(),
        Some(1_000_000)
    );

    // A later write must not move it.
    let mut edited = node.clone();
    edited.title = "Age edited".into();
    store.insert_node(&edited).unwrap();
    assert_eq!(
        column(&store, "task:age", "created_at").get_int(),
        Some(1_000_000),
        "an edit must not reset node age -- view:backlog orders by this"
    );
}
