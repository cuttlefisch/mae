//! Round-4 item 2: adversarial tests for the daemon-side silent-persist-failure
//! pattern in `kb_membership.rs` — `let _ = persist_and_broadcast_collection(...).await;`
//! discarding the `Result` while the caller proceeds as if it succeeded. The `Pending`
//! branch was the most serious instance: it unconditionally returned
//! `{"status": "pending"}` even when the pending record was never durably written, so
//! a client believed its join request was recorded (and would show up for the owner
//! to approve) when it had actually vanished the moment the session ended.
//!
//! `StorageBackend` is a real trait object (`Arc<dyn StorageBackend>` inside
//! `DocStore`), so `FailingBackend` below drives a REAL persist failure through the
//! actual `DocStore::apply_update` → `persist_and_broadcast_collection` →
//! `handle_kb_join` call chain — not a synthetic short-circuit — per CLAUDE.md
//! principle #14.

use super::*;
use crate::storage::{DocumentState, StorageBackend, StorageError};
use std::sync::atomic::{AtomicBool, Ordering};

/// Wraps a real in-memory `SqliteBackend`, delegating everything transparently
/// EXCEPT `wal_append`/`compact`, which fail with a `StorageError` whenever
/// `fail_writes` is armed. Lets a test set up a KB normally (writes succeed), then
/// flip the flag to simulate "the backend became unavailable" for one specific
/// subsequent operation — a real failure the calling code must handle honestly,
/// not a stub that never does anything.
struct FailingBackend {
    inner: SqliteBackend,
    fail_writes: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl StorageBackend for FailingBackend {
    async fn wal_append(
        &self,
        doc_name: &str,
        update: &[u8],
        client_id: Option<u64>,
    ) -> Result<u64, StorageError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(StorageError::Io("simulated disk failure".to_string()));
        }
        self.inner.wal_append(doc_name, update, client_id).await
    }

    async fn load_document(&self, doc_name: &str) -> Result<Option<DocumentState>, StorageError> {
        self.inner.load_document(doc_name).await
    }

    async fn compact(
        &self,
        doc_name: &str,
        state: &[u8],
        up_to_wal_id: u64,
    ) -> Result<(), StorageError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(StorageError::Io("simulated disk failure".to_string()));
        }
        self.inner.compact(doc_name, state, up_to_wal_id).await
    }

    async fn list_documents(&self) -> Result<Vec<String>, StorageError> {
        self.inner.list_documents().await
    }

    async fn delete_document(&self, doc_name: &str) -> Result<(), StorageError> {
        self.inner.delete_document(doc_name).await
    }
}

fn failing_doc_store() -> (Arc<DocStore>, Arc<AtomicBool>) {
    let fail_writes = Arc::new(AtomicBool::new(false));
    let backend = Arc::new(FailingBackend {
        inner: SqliteBackend::open_memory().unwrap(),
        fail_writes: fail_writes.clone(),
    });
    (Arc::new(DocStore::new(backend, 500)), fail_writes)
}

#[tokio::test]
async fn pending_join_reports_error_when_persist_fails_not_a_false_pending() {
    let (store, fail_writes) = failing_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    // Share succeeds normally (writes still allowed).
    let shared = kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbi",
        "alice",
        &mut docs,
    )
    .await;
    assert!(shared.error.is_none(), "setup: share must succeed");
    // default policy = invite → a non-member join goes through AccessDecision::Pending

    // NOW arm the failure — the backend becomes unavailable for bob's join.
    fail_writes.store(true, Ordering::SeqCst);

    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbi"),
        &mut docs,
    )
    .await;

    assert!(
        resp.error.is_some(),
        "a join whose pending record failed to persist must return an error, not a \
         false 'status: pending' success: {:?}",
        resp.result
    );
    assert!(
        resp.result.is_none(),
        "no success result should accompany the error"
    );

    // Disarm and verify with a real read: the pending request was never recorded.
    fail_writes.store(false, Ordering::SeqCst);
    let coll = load_coll(&store, "kbi").await;
    assert_eq!(
        coll.pending().len(),
        0,
        "bob's join must NOT appear as pending — it was never durably persisted"
    );
}

#[tokio::test]
async fn pending_join_succeeds_and_is_durably_recorded_when_persist_works() {
    // Control case: confirms `FailingBackend` with the flag OFF behaves identically
    // to the plain in-memory backend (the existing `invite_nonmember_join_pending`
    // assertions), so the error case above is attributable to the injected failure,
    // not to some other difference introduced by the wrapper.
    let (store, _fail_writes) = failing_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("alice"),
        Some(&fp("alice")),
        "kbi2",
        "alice",
        &mut docs,
    )
    .await;
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&fp("bob")),
        kb_join_msg("kbi2"),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none());
    assert_eq!(
        resp.result.as_ref().and_then(|r| r["status"].as_str()),
        Some("pending")
    );
    let coll = load_coll(&store, "kbi2").await;
    assert_eq!(coll.pending().len(), 1, "join durably recorded as pending");
}
