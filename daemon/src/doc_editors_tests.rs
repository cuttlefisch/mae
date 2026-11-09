//! The per-document editor record through `DocStore` (ADR-113 D3).
//!
//! Every test here drives the real `apply_update`/`share_doc` against a real
//! SQLite backend, with the editor-record clock injected so edit order is set,
//! not slept for.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mae_sync::text::TextSync;

use super::*;
use crate::doc_store::DocStore;
use crate::storage::{SqliteBackend, StorageBackend, StorageError};

const ALICE: Attribution<'static> = Attribution::Principal("SHA256:alice");
const BOB: Attribution<'static> = Attribution::Principal("SHA256:bob");

/// A store whose editor-record clock reads `now`.
fn clocked(backend: Arc<SqliteBackend>, now: &Arc<AtomicU64>) -> DocStore {
    let now = Arc::clone(now);
    DocStore::new(backend as Arc<dyn StorageBackend>, 500)
        .with_clock(Arc::new(move || now.load(Ordering::SeqCst)))
}

fn mem() -> Arc<SqliteBackend> {
    Arc::new(SqliteBackend::open_memory().unwrap())
}

/// `(kind, editor)` of each record, in the store's order.
fn who(records: &[EditorRecord]) -> Vec<(EditorKind, &str)> {
    records
        .iter()
        .map(|r| (r.kind, r.editor.as_str()))
        .collect()
}

/// Two principals editing one document are both recorded; `editors_since(T)`
/// returns exactly those whose LATEST edit is after T (strictly), coalesced to
/// one row each; and another document's editor never leaks in.
#[tokio::test]
async fn editors_since_returns_exactly_those_who_edited_after_t() {
    let now = Arc::new(AtomicU64::new(100));
    let store = clocked(mem(), &now);
    let mut a = TextSync::with_client_id("", 1);
    let mut b = TextSync::with_client_id("", 2);

    store
        .apply_update("doc1", &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap();
    now.store(200, Ordering::SeqCst);
    store
        .apply_update("doc1", &b.insert(0, "b"), None, BOB)
        .await
        .unwrap();
    now.store(300, Ordering::SeqCst);
    store
        .apply_update("doc1", &a.insert(1, "c"), None, ALICE)
        .await
        .unwrap();
    // Someone else's document, edited later than everything above.
    now.store(400, Ordering::SeqCst);
    let mut c = TextSync::with_client_id("", 3);
    store
        .apply_update(
            "doc2",
            &c.insert(0, "z"),
            None,
            Attribution::Principal("SHA256:carol"),
        )
        .await
        .unwrap();

    let all = store.editors_since("doc1", 0).await.unwrap();
    assert_eq!(
        who(&all),
        vec![
            (EditorKind::Principal, "SHA256:bob"),
            (EditorKind::Principal, "SHA256:alice"),
        ],
        "both editors, one row each, ordered by latest edit"
    );
    let alice = &all[1];
    assert_eq!(
        (alice.first_at_ms, alice.last_at_ms, alice.updates),
        (100, 300, 2)
    );
    assert_eq!(
        (all[0].first_at_ms, all[0].last_at_ms, all[0].updates),
        (200, 200, 1)
    );
    assert!(
        alice.last_wal_id > all[0].last_wal_id,
        "WAL id orders edits too"
    );

    assert_eq!(
        who(&store.editors_since("doc1", 250).await.unwrap()),
        vec![(EditorKind::Principal, "SHA256:alice")]
    );
    // Strict: an edit AT T is not "since" T.
    assert_eq!(
        who(&store.editors_since("doc1", 200).await.unwrap()),
        vec![(EditorKind::Principal, "SHA256:alice")]
    );
    assert!(store.editors_since("doc1", 300).await.unwrap().is_empty());
    assert!(store
        .editors_since("never-written", 0)
        .await
        .unwrap()
        .is_empty());
}

/// A wall clock that steps backwards must never move an editor's `last_at`
/// back: that would hide their later edit from `editors_since`, dropping them
/// from an author set.
#[tokio::test]
async fn a_clock_stepping_back_never_hides_an_edit() {
    let now = Arc::new(AtomicU64::new(500));
    let store = clocked(mem(), &now);
    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update("doc1", &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap();
    now.store(100, Ordering::SeqCst);
    store
        .apply_update("doc1", &a.insert(1, "b"), None, ALICE)
        .await
        .unwrap();

    let rows = store.editors_since("doc1", 400).await.unwrap();
    assert_eq!(rows.len(), 1, "the edit stamped 500 is still after 400");
    assert_eq!((rows[0].first_at_ms, rows[0].last_at_ms), (100, 500));
}

/// An update refused by the ADR-109 D6 size cap is restored from storage and
/// records nobody — on an existing document and on a fresh one.
#[tokio::test]
async fn a_refused_oversized_update_records_nobody() {
    let now = Arc::new(AtomicU64::new(10));
    let store = clocked(mem(), &now).with_max_document_size(16);
    let mallory = Attribution::Principal("SHA256:mallory");

    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update("doc1", &a.insert(0, "small"), None, ALICE)
        .await
        .unwrap();
    let mut m = TextSync::with_client_id("", 9);
    let big = m.insert(0, &"x".repeat(64));
    for doc in ["doc1", "fresh"] {
        match store.apply_update(doc, &big, None, mallory).await {
            Err(StorageError::TooLarge { .. }) => {}
            other => panic!("{doc}: expected TooLarge, got {other:?}"),
        }
    }

    assert_eq!(
        who(&store.editors_since("doc1", 0).await.unwrap()),
        vec![(EditorKind::Principal, "SHA256:alice")]
    );
    assert!(store.editors_since("fresh", 0).await.unwrap().is_empty());
}

/// The record is persisted, not held in memory: a new `DocStore` over a new
/// connection to the same database file still answers.
#[tokio::test]
async fn records_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collab.db");
    let now = Arc::new(AtomicU64::new(42));
    {
        let store = clocked(Arc::new(SqliteBackend::open(&path).unwrap()), &now);
        let mut a = TextSync::with_client_id("", 1);
        store
            .apply_update("kbn:kb1:concept:x", &a.insert(0, "a"), None, ALICE)
            .await
            .unwrap();
        store
            .apply_update("kbn:kb1:concept:x", &a.insert(1, "b"), None, BOB)
            .await
            .unwrap();
    }
    let reopened = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
    let rows = reopened
        .editors_since("kbn:kb1:concept:x", 41)
        .await
        .unwrap();
    assert_eq!(
        who(&rows),
        vec![
            (EditorKind::Principal, "SHA256:alice"),
            (EditorKind::Principal, "SHA256:bob"),
        ]
    );
}

/// A share replaces a document's content, so it records the sharer — and the
/// delete it does first must not wipe the editors before it (a reseal or
/// rebirth would otherwise erase a node's authors).
#[tokio::test]
async fn a_share_records_the_sharer_and_keeps_earlier_editors() {
    let now = Arc::new(AtomicU64::new(1));
    let store = clocked(mem(), &now);
    let doc = "kbn:kb1:concept:x";
    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update(doc, &a.insert(0, "draft"), None, ALICE)
        .await
        .unwrap();
    now.store(2, Ordering::SeqCst);
    let replacement = TextSync::with_client_id("", 2).insert(0, "resealed");
    store.share_doc(doc, &replacement, BOB).await.unwrap();

    assert_eq!(
        who(&store.editors_since(doc, 0).await.unwrap()),
        vec![
            (EditorKind::Principal, "SHA256:alice"),
            (EditorKind::Principal, "SHA256:bob"),
        ]
    );
}

/// Deleting an ephemeral document drops its editors (the table stays bounded);
/// deleting a durable KB doc keeps them — they are the author record, and a
/// durable doc is never meant to lose history to a delete-and-recreate.
#[tokio::test]
async fn deleting_forgets_an_ephemeral_docs_editors_but_not_a_durable_docs() {
    let store = clocked(mem(), &Arc::new(AtomicU64::new(1)));
    let ephemeral = "file:proj/notes.txt";
    let durable = "kbn:kb1:concept:x";
    for doc in [ephemeral, durable] {
        let mut a = TextSync::with_client_id("", 1);
        store
            .apply_update(doc, &a.insert(0, "a"), None, ALICE)
            .await
            .unwrap();
        store.delete_doc(doc).await.unwrap();
    }
    assert!(store.editors_since(ephemeral, 0).await.unwrap().is_empty());
    assert_eq!(store.editors_since(durable, 0).await.unwrap().len(), 1);
}

/// Each non-principal kind is recorded under its own kind, so a relay, the
/// daemon or an unauthenticated socket can never be read back as a principal.
#[tokio::test]
async fn non_principal_writers_are_recorded_as_what_they_are() {
    let store = clocked(mem(), &Arc::new(AtomicU64::new(1)));
    let writers = [
        Attribution::MeshPeer("SHA256:peer"),
        Attribution::Internal(InternalWriter::CheckpointRestore),
        Attribution::Unauthenticated { session_id: 7 },
    ];
    for (i, by) in writers.into_iter().enumerate() {
        let mut t = TextSync::with_client_id("", i as u64 + 1);
        store
            .apply_update("doc1", &t.insert(0, "x"), None, by)
            .await
            .unwrap();
    }
    let mut got = who(&store.editors_since("doc1", 0).await.unwrap())
        .into_iter()
        .map(|(k, e)| (k, e.to_string()))
        .collect::<Vec<_>>();
    got.sort_by_key(|(k, _)| k.as_str());
    assert_eq!(
        got,
        vec![
            (EditorKind::Internal, "checkpoint-restore".to_string()),
            (EditorKind::MeshPeer, "SHA256:peer".to_string()),
            (EditorKind::Unauthenticated, "session:7".to_string()),
        ]
    );
}
