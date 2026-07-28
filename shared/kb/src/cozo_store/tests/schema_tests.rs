use super::*;

/// Issue #447: 3+ threads racing to open the identical, not-yet-existing
/// store path used to panic partway through schema creation ("index out of
/// bounds: the len is 0 but the index is 0" reading `source_files` — a
/// genuine CozoDB-relation-visibility race during concurrent first-time
/// `:create relation {...}` DDL, not a registry-level bug ADR-058 Phase B
/// already fixed separately). Fixed by serializing store creation via the
/// existing, already-adversarially-tested advisory file lock
/// (`mae_mcp::file_lock::LockGuard`) rather than a new mechanism.
///
/// **SQLite, not the default sled engine.** Writing this test surfaced a
/// real, separate fact worth documenting: sled enforces single-live-handle
/// exclusivity at the OS level (a second concurrent `open()` against a store
/// another handle still has open fails immediately with a clean `WouldBlock`
/// I/O error, verified directly — not a panic, and not something this fix
/// needs to address). Genuinely concurrent *multi-handle* access to the same
/// store — the scenario this bug needs — is exactly what SQLite's WAL mode
/// exists for in this codebase (ADR-004/ADR-054), and is what `mae-daemon`
/// actually uses in production (`daemon/Cargo.toml`: `default-features =
/// false, features = ["storage-sqlite"]` — sled isn't even compiled in
/// there). This test targets that real, reachable configuration rather than
/// asserting a scenario sled's own design already rules out.
#[cfg(feature = "storage-sqlite")]
#[test]
fn concurrent_first_time_sqlite_open_and_import_does_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    // A path that does NOT exist yet — every thread races to create it.
    let store_path = tmp.path().join("fresh_concurrent_store");

    let handles: Vec<_> = (0..5)
        .map(|i| {
            let store_path = store_path.clone();
            std::thread::spawn(move || {
                let store = CozoKbStore::open_with_engine(&store_path, "sqlite").unwrap();
                // Exactly the call chain issue #447's real repro hit:
                // record_source_file -> get_source_file_node_ids on a store
                // that may still be mid-creation on another thread.
                let node_id = format!("concurrent-node-{i}");
                store
                    .insert_node(&crate::Node::new(
                        &node_id,
                        format!("Concurrent node {i}"),
                        crate::NodeKind::Note,
                        "real body content, not a placeholder",
                    ))
                    .unwrap();
                store
                    .record_source_file(
                        &format!("file-{i}.org"),
                        &format!("hash-{i}"),
                        0,
                        std::slice::from_ref(&node_id),
                    )
                    .unwrap();
                // Read back through the exact function that panicked.
                let ids = store
                    .get_source_file_node_ids(&format!("file-{i}.org"))
                    .unwrap();
                assert_eq!(ids, vec![node_id]);
            })
        })
        .collect();

    for h in handles {
        h.join()
            .expect("no thread should panic during concurrent first-time open+import");
    }

    // Sanity: the store genuinely converged to one consistent, fully-schema'd
    // store on disk — reopening it and reading every thread's node back
    // confirms no partial/corrupted schema state was left behind.
    let reopened = CozoKbStore::open_with_engine(&store_path, "sqlite").unwrap();
    for i in 0..5 {
        let node = reopened.get_node(&format!("concurrent-node-{i}")).unwrap();
        assert!(
            node.is_some(),
            "node from thread {i} must be present after all concurrent opens converge"
        );
    }
}

/// The sled-side half of the same investigation: concurrent opens of the
/// identical not-yet-existing store must never panic, even though sled's own
/// single-live-handle exclusivity means they can't all succeed the way the
/// SQLite case above does. Either outcome — `Ok` with a genuinely usable
/// store, or a clean `Err` (sled's own lock contention) — is acceptable;
/// only a panic is the regression this guards against.
#[test]
fn concurrent_first_time_sled_open_never_panics_even_under_lock_contention() {
    let tmp = tempfile::tempdir().unwrap();
    let store_path = tmp.path().join("fresh_sled_store");

    let handles: Vec<_> = (0..5)
        .map(|i| {
            let store_path = store_path.clone();
            std::thread::spawn(move || {
                // Catch a panic here explicitly (rather than only relying on
                // the join below) so a single thread's panic can't poison
                // the store/lock state for the others mid-test.
                std::panic::catch_unwind(move || {
                    if let Ok(store) = CozoKbStore::open(&store_path) {
                        let node_id = format!("sled-node-{i}");
                        let _ = store.insert_node(&crate::Node::new(
                            &node_id,
                            format!("Sled node {i}"),
                            crate::NodeKind::Note,
                            "real body content",
                        ));
                    }
                    // A returned Err (e.g. sled's own lock contention) is a
                    // clean, acceptable outcome -- only a panic is not.
                })
            })
        })
        .collect();

    for h in handles {
        assert!(
            h.join().unwrap().is_ok(),
            "concurrent sled opens must never panic, even when contended"
        );
    }
}

/// Deterministic coverage for `retry_on_transient_sqlite_busy` (`schema.rs`)
/// — the real "database is locked" race it exists for is SQLite-internals-
/// timing-dependent (two SEPARATE real CI failures this session: first
/// surfacing as a panic, then -- after that was fixed -- surfacing again
/// via a totally different cozo-internal code path that returns a normal
/// `Err` instead, from the exact same underlying condition), so this
/// exercises the retry logic itself directly for BOTH shapes rather than
/// relying on getting lucky with real contention.
mod retry_on_transient_sqlite_busy_tests {
    use crate::cozo_store::schema::{
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

#[test]
fn schema_creates_all_relations() {
    let (_tmp, store) = make_store();
    // Verify all Phase B relations exist by querying them
    let relations = [
        "node_types",
        "rel_types",
        "blocks",
        "meta_members",
        "node_versions",
        "views",
        "hygiene_suggestions",
        "instance_meta",
        "embeddings",
    ];
    // Verify all Phase B relations exist by doing a count query on each.
    // Each relation has a different key column, so use :columns introspection.
    for rel in relations {
        let query = format!("::columns {rel}");
        let result = store.run_immut(&query);
        assert!(result.is_ok(), "relation {rel} should exist: {result:?}");
    }
}

#[test]
fn instance_id_generated_on_open() {
    let (_tmp, store) = make_store();
    let id = store.instance_id().unwrap();
    assert!(!id.is_empty());
    assert!(id.contains('-'), "should be UUID format: {id}");
    // Idempotent — second call returns same ID
    let id2 = store.instance_id().unwrap();
    assert_eq!(id, id2);
}

#[test]
fn seed_type_system_populates_metadata() {
    let (_tmp, store) = make_store();
    store.seed_type_system().unwrap();

    // Check node_types
    let (headers, rows) = store
        .raw_query("?[kind, label] := *node_types{kind, label}")
        .unwrap();
    assert!(headers.contains(&"kind".to_string()));
    assert!(
        rows.len() >= 14,
        "should have at least 14 node types, got {}",
        rows.len()
    );

    // Check rel_types
    let (_, rel_rows) = store
        .raw_query("?[name, inverse] := *rel_types{name, inverse_name: inverse}")
        .unwrap();
    assert!(
        rel_rows.len() >= 20,
        "should have at least 20 rel types, got {}",
        rel_rows.len()
    );

    // Idempotent — re-seeding doesn't duplicate
    store.seed_type_system().unwrap();
    let (_, rows2) = store.raw_query("?[kind] := *node_types{kind}").unwrap();
    assert_eq!(rows.len(), rows2.len());
}

#[test]
fn seed_views_creates_view_nodes() {
    let (_tmp, store) = make_store();
    store.seed_views().unwrap();

    // Views should be in the views relation
    let result = store
        .run_immut("?[id, title, kind] := *views{id, title, kind}")
        .unwrap();
    assert!(
        result.rows.len() >= 6,
        "should have at least 6 seeded views, got {}",
        result.rows.len()
    );

    // View nodes should also exist as regular KB nodes
    let kanban = store.get_node("view:kanban").unwrap();
    assert!(kanban.is_some(), "kanban view should exist as a node");
    assert_eq!(kanban.unwrap().title, "Kanban Board");

    // Idempotent: seeding again should not error
    store.seed_views().unwrap();
}
