//! Adversarial proof that ONE shared `Arc<CozoKbStore>` (one `Db<S>`, one
//! connection) survives genuinely concurrent access from many OS threads
//! within the SAME daemon process (ADR-054's "Implementation note" addendum).
//!
//! This is a different, complementary axis to the existing
//! `sqlite_multi_instance_concurrent_writes_converge`
//! (`crates/core/src/editor/kb_ops/tests/kb_ops_concurrency_tests.rs`), which
//! opens TWO SEPARATE `CozoKbStore` handles on the same file to model
//! cross-*process* contention through `run_with_busy_retry`'s SQLITE_BUSY
//! path. Here there is exactly one store handle, accessed concurrently by
//! many `spawn_blocking` tasks (genuinely different OS threads via tokio's
//! blocking pool) — this exercises Cozo's own in-process `relation_locks` /
//! `running_queries` machinery directly, the thing the coarse
//! `Mutex<DaemonState>` previously prevented this codebase from ever truly
//! testing before the ADR-054 rewrite removed it from the read/write arms.

use super::*;
use mae_kb::CozoKbStore;

/// N (>= 3 per principle #14) concurrent writers, each inserting a distinct
/// node directly into the SAME shared store handle — no serialization
/// point in this codebase to force them onto one thread, so this only
/// passes if Cozo's own concurrency control is sound for this usage.
#[tokio::test]
async fn concurrent_inserts_from_many_threads_are_all_durably_applied() {
    let store = Arc::new(CozoKbStore::open_mem().expect("open store"));

    const N: usize = 12;
    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let store = Arc::clone(&store);
        handles.push(tokio::task::spawn_blocking(move || {
            let node = Node::new(
                format!("concurrent:node-{i}"),
                format!("Concurrent node {i}"),
                NodeKind::Note,
                format!("body for concurrently-inserted node {i}"),
            );
            store.insert_node(&node)
        }));
    }
    for h in handles {
        h.await
            .expect("writer task panicked")
            .expect("insert_node must not fail under concurrent access");
    }

    let all = store.load_all().expect("load_all after concurrent inserts");
    let ids: std::collections::HashSet<&str> = all.iter().map(|n| n.id.as_str()).collect();
    for i in 0..N {
        let expected = format!("concurrent:node-{i}");
        assert!(
            ids.contains(expected.as_str()),
            "node {expected} missing after concurrent insert — a write was lost. Present: {ids:?}"
        );
    }
    assert_eq!(
        all.len(),
        N,
        "expected exactly {N} nodes, got {} — either a lost write or a duplicate",
        all.len()
    );
}

/// The write RPC arms specifically (`kb/hygiene_accept` / `kb/hygiene_dismiss`,
/// ADR-054's rewritten snapshot-then-drop + `spawn_blocking` arms) driven
/// concurrently through `dispatch` — not just direct store calls — against
/// distinct suggestions on the shared store, proving the dispatch-level
/// rewrite itself (not only the underlying store) is safe under concurrency.
#[tokio::test]
async fn concurrent_hygiene_accept_dismiss_via_dispatch_all_apply_with_no_lost_update() {
    let store = CozoKbStore::open_mem().expect("open store");
    const N: usize = 6;
    for i in 0..N {
        let node_id = format!("hygiene:node-{i}");
        store
            .insert_node(&Node::new(
                &node_id,
                format!("Hygiene node {i}"),
                NodeKind::Note,
                "body",
            ))
            .unwrap();
        // suggestion_id is 1-based per node_id, so each writer below targets a
        // distinct (node_id, suggestion_id) row — genuinely concurrent access
        // to the shared relation, no same-row race to additionally resolve.
        store
            .insert_suggestion(&node_id, "test-category", "a suggestion", "{}", 0.9)
            .unwrap();
    }

    let mut st = DaemonState::new();
    st.store = Some(Arc::new(store));
    let state = Arc::new(Mutex::new(st));

    // Even-indexed nodes get accepted, odd-indexed get dismissed — both write
    // arms exercised concurrently against the same shared store.
    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let state = Arc::clone(&state);
        let node_id = format!("hygiene:node-{i}");
        let method = if i % 2 == 0 {
            "kb/hygiene_accept"
        } else {
            "kb/hygiene_dismiss"
        };
        handles.push(tokio::spawn(async move {
            crate::handler::dispatch(
                method,
                json!({"node_id": node_id, "suggestion_id": 1}),
                &state,
            )
            .await
        }));
    }
    for (i, h) in handles.into_iter().enumerate() {
        let result = h.await.expect("dispatch task panicked");
        assert!(
            result.is_ok(),
            "concurrent dispatch for node {i} must not fail: {result:?}"
        );
    }

    // Read back through the store directly — every status transition must
    // have durably applied, none lost or overwritten by a concurrent sibling.
    let store = state.lock().await.store.clone().unwrap();
    let accepted = store.list_suggestions(None, Some("accepted")).unwrap();
    let dismissed = store.list_suggestions(None, Some("dismissed")).unwrap();
    assert_eq!(
        accepted.len(),
        N.div_ceil(2),
        "accepted count mismatch — a concurrent hygiene_accept was lost: {accepted:?}"
    );
    assert_eq!(
        dismissed.len(),
        N / 2,
        "dismissed count mismatch — a concurrent hygiene_dismiss was lost: {dismissed:?}"
    );
}
