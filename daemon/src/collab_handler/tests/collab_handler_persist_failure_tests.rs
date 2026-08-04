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

// ---------------------------------------------------------------------------
// Audit #589.4 — the signed membership op-log (ADR-026) is what peers verify
// membership against WITHOUT trusting the relay. `append_signed_membership`
// returned `()` on both of its failure paths, warn-logging and moving on, while
// every caller reported unconditional success. Two distinct severities follow
// from that, and both are tested here against a REAL injected backend failure.
// ---------------------------------------------------------------------------

/// An owner-signing doc store on top of the failure-injecting backend, so the
/// signed-op path is actually taken (`append_signed_membership` short-circuits
/// as `NotOwned` unless this daemon's signer IS the collection owner).
fn owned_failing_store() -> (Arc<DocStore>, Arc<AtomicBool>, String) {
    use mae_mcp::identity::Identity;
    let (store, fail_writes) = failing_doc_store();
    let id = Identity::generate("owner");
    let owner_fp = id.fingerprint();
    store.set_signer(Arc::new(id));
    (store, fail_writes, owner_fp)
}

/// `kb/set_governance` has NO effect other than the signed append. Swallowing
/// the failure produced a wholly fabricated success: the client was told
/// governance was now `quorum:2` while the op-log still derived `SingleOwner`,
/// so a later m-of-n revoke would silently be evaluated under the old rule.
#[tokio::test]
async fn set_governance_reports_failure_when_the_signed_op_cannot_persist() {
    use mae_sync::membership::{derive_governance, Governance};

    let (store, fail_writes, owner_fp) = owned_failing_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbgf",
        "owner",
        &mut docs,
    )
    .await;
    let owner_pubkey = store.signer().unwrap().public().to_bytes();

    // Arm the failure only for the governance change itself.
    fail_writes.store(true, Ordering::SeqCst);
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_set_governance_msg("kbgf", "quorum:2"),
        &mut docs,
    )
    .await;

    assert!(
        resp.error.is_some(),
        "set_governance must NOT report success when its only effect failed to \
         persist: {:?}",
        resp.result
    );
    assert!(
        resp.result.is_none(),
        "no success payload alongside the error"
    );

    // Selective oracle: the governance genuinely did not change. Asserting on
    // the derived value (what peers actually enforce) rather than on the RPC
    // shape alone — a handler could return an error and still have written.
    fail_writes.store(false, Ordering::SeqCst);
    let coll = load_coll(&store, "kbgf").await;
    assert_eq!(
        derive_governance(&coll.oplog_ops(), &owner_pubkey),
        Governance::SingleOwner,
        "governance must still be the pre-call value"
    );
}

/// The counter-case that keeps the guard above honest: with writes working, the
/// same call succeeds and the derived governance really changes. Without this a
/// handler that errored unconditionally would pass the test above.
#[tokio::test]
async fn set_governance_succeeds_and_changes_derivation_when_persist_works() {
    use mae_sync::membership::{derive_governance, Governance};

    let (store, _fail_writes, owner_fp) = owned_failing_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbgok",
        "owner",
        &mut docs,
    )
    .await;
    let owner_pubkey = store.signer().unwrap().public().to_bytes();

    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_set_governance_msg("kbgok", "quorum:2"),
        &mut docs,
    )
    .await;
    assert!(resp.error.is_none(), "{:?}", resp.error);

    let coll = load_coll(&store, "kbgok").await;
    assert_eq!(
        derive_governance(&coll.oplog_ops(), &owner_pubkey),
        Governance::Quorum { threshold: 2 }
    );
}

/// `kb/add_member` is the *other* severity: the legacy `member_roles` mutation
/// really did persist, so the call is a genuine success — but the peer-verifiable
/// op-log silently diverged. The response must surface that divergence instead
/// of implying both landed, or an owner has no way to know the membership their
/// peers can verify no longer matches the membership they just set.
#[tokio::test]
async fn add_member_flags_the_signed_oplog_divergence_instead_of_hiding_it() {
    let (store, fail_writes, owner_fp) = owned_failing_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        "kbmf",
        "owner",
        &mut docs,
    )
    .await;

    // Control: with writes healthy, the signed mirror lands and is reported so.
    let ok = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/add_member",
            "params":{"kb_id":"kbmf","member":fp("carol"),"role":"editor"}}),
        &mut docs,
    )
    .await;
    assert!(ok.error.is_none(), "{:?}", ok.error);
    assert_eq!(
        ok.result.as_ref().and_then(|r| r["signed_oplog"].as_bool()),
        Some(true),
        "a healthy add must report the signed op-log was updated"
    );
    assert!(
        ok.result.as_ref().map(|r| r["warning"].is_null()) == Some(true),
        "no warning when nothing diverged"
    );

    // Now break the backend and add a different member.
    fail_writes.store(true, Ordering::SeqCst);
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/add_member",
            "params":{"kb_id":"kbmf","member":fp("bob"),"role":"editor"}}),
        &mut docs,
    )
    .await;

    // The legacy persist is what fails first here, so EITHER an outright error
    // or a success carrying the divergence flag is acceptable — what is NOT
    // acceptable is a clean unqualified success, which is what shipped.
    if resp.error.is_none() {
        let result = resp.result.as_ref().expect("a result or an error");
        assert_eq!(
            result["signed_oplog"].as_bool(),
            Some(false),
            "a success returned while the signed op-log failed must say so: {result}"
        );
        assert!(
            result["warning"]
                .as_str()
                .is_some_and(|w| w.contains("op-log")),
            "the divergence must be spelled out for the caller: {result}"
        );
    }
}
