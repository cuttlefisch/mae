//! Refusal kinds that need something other than a plain KB to reach: a
//! storage backend that fails, a tenant quota that refuses, a small update
//! cap, and a request on the P2P transport. Each is driven through the real
//! dispatch with that one condition real, not stubbed out of the call chain.

use super::collab_handler_error_kind_tests::{expect_kind, rpc, update_as, KB, NODE};
use super::*;
use crate::storage::{DocumentState, StorageBackend, StorageError};
use std::sync::atomic::{AtomicBool, Ordering};

/// A real in-memory backend whose reads and writes can each be made to fail.
struct ArmedBackend {
    inner: SqliteBackend,
    fail_reads: AtomicBool,
    fail_writes: AtomicBool,
}

fn armed(flag: &AtomicBool) -> Result<(), StorageError> {
    if flag.load(Ordering::SeqCst) {
        return Err(StorageError::Io("simulated disk failure".to_string()));
    }
    Ok(())
}

#[async_trait::async_trait]
impl StorageBackend for ArmedBackend {
    async fn wal_append(
        &self,
        doc: &str,
        u: &[u8],
        c: Option<u64>,
        by: &crate::doc_editors::EditStamp,
    ) -> Result<u64, StorageError> {
        armed(&self.fail_writes)?;
        self.inner.wal_append(doc, u, c, by).await
    }
    async fn editors_since_wal(
        &self,
        doc: &str,
        after: u64,
    ) -> Result<crate::doc_editors::EditorsSince, StorageError> {
        self.inner.editors_since_wal(doc, after).await
    }
    async fn wal_high_water(&self, doc: &str) -> Result<u64, StorageError> {
        self.inner.wal_high_water(doc).await
    }
    async fn history_session(
        &self,
        kb: &str,
        node: &str,
    ) -> Result<Vec<(crate::doc_editors::EditorKind, String)>, StorageError> {
        self.inner.history_session(kb, node).await
    }
    async fn history_cut(
        &self,
        cp: crate::storage::Checkpoint,
        now: i64,
    ) -> Result<Option<mae_sync::kb::history::HistoryEntry>, StorageError> {
        self.inner.history_cut(cp, now).await
    }
    async fn history_list(
        &self,
        kb: &str,
        node: &str,
    ) -> Result<Vec<mae_sync::kb::history::HistoryEntry>, StorageError> {
        self.inner.history_list(kb, node).await
    }
    async fn history_page(
        &self,
        kb: &str,
        node: &str,
        before: Option<u64>,
        limit: u32,
    ) -> Result<Vec<crate::storage::Listed>, StorageError> {
        self.inner.history_page(kb, node, before, limit).await
    }
    async fn history_holds_content(&self, kb: &str) -> Result<bool, StorageError> {
        self.inner.history_holds_content(kb).await
    }
    async fn history_nodes(&self, kb: &str) -> Result<Vec<String>, StorageError> {
        self.inner.history_nodes(kb).await
    }
    async fn history_kbs(&self) -> Result<Vec<String>, StorageError> {
        self.inner.history_kbs().await
    }
    async fn history_tips(
        &self,
        kb: &str,
    ) -> Result<Vec<mae_sync::kb::history::Tip>, StorageError> {
        self.inner.history_tips(kb).await
    }
    async fn history_append_head(
        &self,
        head: &mae_sync::kb::history::SignedHead,
    ) -> Result<(), StorageError> {
        self.inner.history_append_head(head).await
    }
    async fn history_latest_head(
        &self,
        kb: &str,
    ) -> Result<Option<mae_sync::kb::history::SignedHead>, StorageError> {
        self.inner.history_latest_head(kb).await
    }

    async fn history_key(
        &self,
        kb: &str,
    ) -> Result<Option<mae_sync::kb::history::HistoryKey>, StorageError> {
        self.inner.history_key(kb).await
    }
    async fn forget_editors(&self, doc: &str) -> Result<(), StorageError> {
        self.inner.forget_editors(doc).await
    }
    async fn load_document(&self, doc: &str) -> Result<Option<DocumentState>, StorageError> {
        armed(&self.fail_reads)?;
        self.inner.load_document(doc).await
    }
    async fn compact(&self, doc: &str, state: &[u8], upto: u64) -> Result<(), StorageError> {
        armed(&self.fail_writes)?;
        self.inner.compact(doc, state, upto).await
    }
    async fn list_documents(&self) -> Result<Vec<String>, StorageError> {
        self.inner.list_documents().await
    }
    async fn doc_exists(&self, doc: &str) -> Result<bool, StorageError> {
        self.inner.doc_exists(doc).await
    }
    async fn delete_document(&self, doc: &str) -> Result<(), StorageError> {
        self.inner.delete_document(doc).await
    }
}

fn armed_backend() -> Arc<ArmedBackend> {
    Arc::new(ArmedBackend {
        inner: SqliteBackend::open_memory().unwrap(),
        fail_reads: AtomicBool::new(false),
        fail_writes: AtomicBool::new(false),
    })
}

async fn share(store: &Arc<DocStore>, bc: &SharedBroadcaster, docs: &mut HashSet<String>) {
    let r = kb_share_as(
        store,
        bc,
        Some("owner"),
        Some(&fp("owner")),
        KB,
        "owner",
        docs,
    )
    .await;
    assert!(r.error.is_none(), "share: {:?}", r.error);
}

/// A collection that is durably there but cannot be read is not "no such KB".
#[tokio::test]
async fn an_unreadable_collection_is_collection_unavailable_not_kb_not_found() {
    let backend = armed_backend();
    let (bc, mut docs) = (test_broadcaster(), HashSet::new());
    share(
        &Arc::new(DocStore::new(backend.clone(), 500)),
        &bc,
        &mut docs,
    )
    .await;
    // A fresh store over the same backend has nothing in memory, so it must read.
    let cold = Arc::new(DocStore::new(backend.clone(), 500));
    backend.fail_reads.store(true, Ordering::SeqCst);
    let fetch = rpc(
        "kb/node_fetch",
        serde_json::json!({"kb_id": KB, "node_id": NODE}),
    );
    let owner = fp("owner");
    let resp = dispatch_as(
        &cold,
        &bc,
        Some("owner"),
        Some(&owner),
        fetch.clone(),
        &mut docs,
    )
    .await;
    expect_kind(&resp, "collection_unavailable", "collection read fails");
    // Control: the same request for an id with no collection at all.
    let missing = rpc(
        "kb/node_fetch",
        serde_json::json!({"kb_id": "other", "node_id": NODE}),
    );
    let resp = dispatch_as(&cold, &bc, Some("owner"), Some(&owner), missing, &mut docs).await;
    expect_kind(&resp, "kb_not_found", "no collection");
}

/// A persistence failure is the daemon's fault, reported as `internal`.
#[tokio::test]
async fn a_failed_write_is_internal() {
    let backend = armed_backend();
    let (store, bc, mut docs) = (
        Arc::new(DocStore::new(backend.clone(), 500)),
        test_broadcaster(),
        HashSet::new(),
    );
    share(&store, &bc, &mut docs).await; // default policy: invite → a stranger's join is pending
    backend.fail_writes.store(true, Ordering::SeqCst);
    let stranger = fp("stranger");
    let resp = dispatch_as(
        &store,
        &bc,
        Some("stranger"),
        Some(&stranger),
        kb_join_msg(KB),
        &mut docs,
    )
    .await;
    expect_kind(&resp, "internal", "pending join not persisted");
}

#[tokio::test]
async fn an_update_over_the_per_update_cap_is_update_too_large() {
    let store = Arc::new(
        DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500)
            .with_max_update_size(16),
    );
    let (bc, mut docs) = (test_broadcaster(), HashSet::new());
    share(&store, &bc, &mut docs).await;
    let big = update_as(&fp("owner"), 0, &"x".repeat(64));
    let size = base64_to_update(&big).unwrap().len();
    let owner = fp("owner");
    let node = rpc(
        "kb/node_update",
        serde_json::json!({"kb_id": KB, "node_id": NODE, "update": big}),
    );
    let raw = rpc(
        "sync/update",
        serde_json::json!({"doc": "plain-doc", "update": big}),
    );
    for msg in [node, raw] {
        let resp = dispatch_as(&store, &bc, Some("owner"), Some(&owner), msg, &mut docs).await;
        let data = expect_kind(&resp, "update_too_large", "64-char insert, 16-byte cap");
        assert_eq!(
            (data["size"].as_u64(), data["limit"].as_u64()),
            (Some(size as u64), Some(16))
        );
    }
}

struct Refuses;

impl crate::quota::QuotaCharger for Refuses {
    fn charge(&self, _: Option<&str>, _: &str) -> Result<crate::quota::QuotaLease, String> {
        Err("tenant quota exceeded for 'owner'".to_string())
    }
}

/// Dispatch with an explicit quota and transport (`dispatch_as` fixes both).
async fn dispatch_with(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    principal: &str,
    msg: serde_json::Value,
    docs: &mut HashSet<String>,
    (quota, transport): (&dyn crate::quota::QuotaCharger, Transport),
) -> JsonRpcResponse {
    handle_doc_request_inner(
        &msg.to_string(),
        store,
        quota,
        bc,
        std::time::Instant::now(),
        0,
        Some(principal),
        Some(principal),
        None,
        docs,
        transport,
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        None,
    )
    .await
}

#[tokio::test]
async fn quota_and_transport_refusals_carry_their_kinds() {
    let (store, bc, mut docs) = (test_doc_store(), test_broadcaster(), HashSet::new());
    share(&store, &bc, &mut docs).await;
    let editor = fp("editor");
    let add = kb_member_msg("kb/add_member", KB, &editor, Some("editor"));
    assert!(dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&fp("owner")),
        add,
        &mut docs
    )
    .await
    .error
    .is_none());
    let fetch = || {
        rpc(
            "kb/node_fetch",
            serde_json::json!({"kb_id": KB, "node_id": NODE}),
        )
    };

    let throttled = dispatch_with(
        &store,
        &bc,
        &editor,
        fetch(),
        &mut docs,
        (&Refuses, Transport::Hub),
    )
    .await;
    expect_kind(&throttled, "tenant_quota_exceeded", "a refusing charger");

    // The KB is shared over the hub only (the default), and the editor is not
    // the owner, so the mesh does not reach it.
    let quota = &crate::quota::NoQuota;
    let mesh = dispatch_with(
        &store,
        &bc,
        &editor,
        fetch(),
        &mut docs,
        (quota, Transport::P2p),
    )
    .await;
    let data = expect_kind(&mesh, "transport_forbidden", "hub-only KB over P2P");
    assert_eq!(data["transport"], "p2p");
}

/// The mesh requires signed content ops (ADR-036): a member cleared to write
/// the KB over P2P still has an unsigned relay refused. (The caller's own gate
/// runs first, so the KB is shared over P2P here — on a hub-only KB the same
/// request is `transport_forbidden`, as above.)
#[tokio::test]
async fn an_unsigned_content_op_on_the_mesh_is_signature_required() {
    let (mut f, _owner, editor) = super::collab_handler_error_kind_write_tests::anchored().await;
    let widen = load_coll(&f.store, KB)
        .await
        .set_transport_policy(mae_sync::kb::TransportPolicy::Both);
    let coll_doc = format!("kbc:{KB}");
    f.store
        .apply_update(&coll_doc, &widen, None, TEST_BY)
        .await
        .unwrap();
    let me = editor.fingerprint();
    let doc = mae_sync::kb_node_doc_name(KB, NODE);
    let update = update_as(&me, 0, "unsigned");
    let msg = rpc(
        "sync/update",
        serde_json::json!({"doc": doc, "update": update, "kb_id": KB}),
    );
    let quota = &crate::quota::NoQuota;
    let resp = dispatch_with(
        &f.store,
        &f.bc,
        &me,
        msg,
        &mut f.docs,
        (quota, Transport::P2p),
    )
    .await;
    expect_kind(&resp, "signature_required", "unsigned op over P2P");
}
