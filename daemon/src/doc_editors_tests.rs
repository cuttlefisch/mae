//! The per-document editor record through `DocStore` (ADR-113 D3).
//!
//! Every test drives the real `apply_update`/`share_doc`/`restore_doc` against
//! a real SQLite backend. Cutoffs are WAL ids taken from the writes themselves.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use mae_sync::kb::Transport;
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

fn store() -> DocStore {
    clocked(mem(), &Arc::new(AtomicU64::new(1)))
}

/// The rows of a `Known` answer; panics on `Incomplete`.
async fn known(store: &DocStore, doc: &str, after: u64) -> Vec<EditorRecord> {
    match store.editors_since_wal(doc, after).await.unwrap() {
        EditorsSince::Known(rows) => rows,
        other => panic!("{doc} after {after}: expected Known, got {other:?}"),
    }
}

/// `(kind, editor)` of each record, in the store's order.
fn who(records: &[EditorRecord]) -> Vec<(EditorKind, &str)> {
    records
        .iter()
        .map(|r| (r.kind, r.editor.as_str()))
        .collect()
}

fn p(editor: &str) -> (EditorKind, &str) {
    (EditorKind::Principal, editor)
}

/// Two principals editing one document are both recorded; the cutoff is a WAL
/// id, and an editor appears exactly when their LATEST update is after it.
/// Another document's editor never leaks in, and each editor is one row.
#[tokio::test]
async fn editors_since_wal_returns_exactly_those_after_the_cutoff() {
    let store = store();
    let (mut a, mut b) = (
        TextSync::with_client_id("", 1),
        TextSync::with_client_id("", 2),
    );
    let w1 = store
        .apply_update("doc1", &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    let w2 = store
        .apply_update("doc1", &b.insert(0, "b"), None, BOB)
        .await
        .unwrap()
        .wal_seq;
    let w3 = store
        .apply_update("doc1", &a.insert(1, "c"), None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    let mut c = TextSync::with_client_id("", 3);
    let carol = Attribution::Principal("SHA256:carol");
    store
        .apply_update("doc2", &c.insert(0, "z"), None, carol)
        .await
        .unwrap();

    let all = known(&store, "doc1", 0).await;
    assert_eq!(who(&all), vec![p("SHA256:bob"), p("SHA256:alice")]);
    assert_eq!(
        (all[1].last_wal_id, all[1].updates),
        (w3, 2),
        "alice coalesced"
    );
    // An edit AT the cutoff is not after it; one past it is.
    assert_eq!(
        who(&known(&store, "doc1", w1).await),
        vec![p("SHA256:bob"), p("SHA256:alice")]
    );
    assert_eq!(
        who(&known(&store, "doc1", w2).await),
        vec![p("SHA256:alice")]
    );
    assert!(
        known(&store, "doc1", w3).await.is_empty(),
        "nobody since the last write"
    );
}

/// The wall clock is display-only: an edit stamped with an EARLIER time after
/// a clock step-back is still after the WAL cutoff, and a row's `last_at`
/// keeps the maximum it has seen.
#[tokio::test]
async fn a_backwards_clock_cannot_hide_an_edit() {
    let now = Arc::new(AtomicU64::new(500));
    let store = clocked(mem(), &now);
    let mut a = TextSync::with_client_id("", 1);
    let mut b = TextSync::with_client_id("", 2);
    let cut = store
        .apply_update("doc1", &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    now.store(100, Ordering::SeqCst);
    store
        .apply_update("doc1", &b.insert(0, "b"), None, BOB)
        .await
        .unwrap();
    store
        .apply_update("doc1", &a.insert(1, "c"), None, ALICE)
        .await
        .unwrap();

    let rows = known(&store, "doc1", cut).await;
    assert_eq!(who(&rows), vec![p("SHA256:bob"), p("SHA256:alice")]);
    assert_eq!((rows[1].first_at_ms, rows[1].last_at_ms), (100, 500));
}

/// An update refused by the ADR-109 D6 size cap records nobody — on an
/// existing document and on a fresh one (which stays "nobody edited it").
#[tokio::test]
async fn a_refused_oversized_update_records_nobody() {
    let store = store().with_max_document_size(16);
    let mallory = Attribution::Principal("SHA256:mallory");
    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update("doc1", &a.insert(0, "small"), None, ALICE)
        .await
        .unwrap();
    let big = TextSync::with_client_id("", 9).insert(0, &"x".repeat(64));
    for doc in ["doc1", "fresh"] {
        match store.apply_update(doc, &big, None, mallory).await {
            Err(StorageError::TooLarge { .. }) => {}
            other => panic!("{doc}: expected TooLarge, got {other:?}"),
        }
    }
    assert_eq!(
        who(&known(&store, "doc1", 0).await),
        vec![p("SHA256:alice")]
    );
    assert!(known(&store, "fresh", 0).await.is_empty());
}

/// The record and the pending marks are persisted: a new `DocStore` over a new
/// connection to the same file still answers.
#[tokio::test]
async fn records_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collab.db");
    let doc = "kbn:kb1:concept:x";
    {
        let store = clocked(
            Arc::new(SqliteBackend::open(&path).unwrap()),
            &Arc::new(AtomicU64::new(1)),
        );
        let mut a = TextSync::with_client_id("", 1);
        store
            .apply_update(doc, &a.insert(0, "a"), None, ALICE)
            .await
            .unwrap();
        store
            .apply_update(doc, &a.insert(1, "b"), None, BOB)
            .await
            .unwrap();
    }
    let reopened = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
    assert_eq!(
        who(&known(&reopened, doc, 0).await),
        vec![p("SHA256:alice"), p("SHA256:bob")]
    );
}

/// The gap-filler's next edit, as they will send it, and an edit by the
/// planter built on top of it — from the gap-filler's state AFTER that edit,
/// so it depends on their item and yrs must hold it pending until it arrives.
fn planted_on_next_item(gap_cid: u64, planter_cid: u64, text: &str) -> (Vec<u8>, Vec<u8>) {
    let mut gap = TextSync::with_client_id("", gap_cid);
    let next = gap.insert(0, "gap ");
    let mut planter =
        TextSync::from_state_with_client_id(&gap.encode_state(), planter_cid).unwrap();
    let planted = planter.insert(4, text);
    (next, planted)
}

fn planted_on_bobs_next_item() -> (Vec<u8>, Vec<u8>) {
    planted_on_next_item(2, 1, "PLANTED")
}

/// The reviewer's scenario. Alice submits an update that cannot integrate yet;
/// a cutoff is frozen; Bob's later update integrates Alice's planted text. The
/// change since the cutoff contains Alice's content, so Alice must be an
/// editor after the cutoff — not only Bob, who merely filled the gap.
#[tokio::test]
async fn held_back_content_is_attributed_when_it_integrates() {
    let store = store();
    let (bobs_next, planted) = planted_on_bobs_next_item();
    let cut = store
        .apply_update("doc1", &planted, None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    assert!(
        !store.content("doc1").await.unwrap().contains("PLANTED"),
        "precondition: pending"
    );

    store
        .apply_update("doc1", &bobs_next, None, BOB)
        .await
        .unwrap();
    assert!(
        store.content("doc1").await.unwrap().contains("PLANTED"),
        "precondition: integrated"
    );

    let rows = known(&store, "doc1", cut).await;
    let mut got = who(&rows);
    got.sort();
    assert_eq!(got, vec![p("SHA256:alice"), p("SHA256:bob")]);
}

/// The pending mark is persisted with the record: a daemon restart between the
/// planted update and the one that integrates it still attributes Alice. The
/// WAL replay re-pends her update, and the mark comes back from storage.
#[tokio::test]
async fn a_pending_mark_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collab.db");
    let (bobs_next, planted) = planted_on_bobs_next_item();
    let cut = {
        let store = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
        store
            .apply_update("doc1", &planted, None, ALICE)
            .await
            .unwrap()
            .wal_seq
    };
    let store = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
    store
        .apply_update("doc1", &bobs_next, None, BOB)
        .await
        .unwrap();
    assert!(store.content("doc1").await.unwrap().contains("PLANTED"));
    let mut got = who(&known(&store, "doc1", cut).await)
        .into_iter()
        .map(|(k, e)| (k, e.to_string()))
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(
        got,
        vec![
            (EditorKind::Principal, "SHA256:alice".into()),
            (EditorKind::Principal, "SHA256:bob".into())
        ]
    );
}

/// Once the pending set drains, the mark is cleared: when a LATER, unrelated
/// pending update drains, only its own submitter is stamped again — not Alice,
/// whose content integrated long before.
#[tokio::test]
async fn a_drained_mark_is_not_restamped() {
    let store = store();
    let (bobs_next, planted) = planted_on_bobs_next_item();
    store
        .apply_update("doc1", &planted, None, ALICE)
        .await
        .unwrap();
    let cut = store
        .apply_update("doc1", &bobs_next, None, BOB)
        .await
        .unwrap()
        .wal_seq;

    let (carol, dave) = (
        Attribution::Principal("SHA256:carol"),
        Attribution::Principal("SHA256:dave"),
    );
    let (daves_next, carols) = planted_on_next_item(4, 3, "SECOND");
    store
        .apply_update("doc1", &carols, None, carol)
        .await
        .unwrap();
    store
        .apply_update("doc1", &daves_next, None, dave)
        .await
        .unwrap();
    assert!(
        store.content("doc1").await.unwrap().contains("SECOND"),
        "precondition"
    );

    let rows = known(&store, "doc1", cut).await;
    let mut got = who(&rows);
    got.sort();
    assert_eq!(got, vec![p("SHA256:carol"), p("SHA256:dave")]);
}

/// A share replaces a document's content, so it records the sharer — and the
/// delete it does first must not wipe the editors before it. It does NOT reset
/// the record origin: the sharer is credited with what they shared.
#[tokio::test]
async fn a_share_records_the_sharer_and_keeps_earlier_editors() {
    let store = store();
    let doc = "kbn:kb1:concept:x";
    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update(doc, &a.insert(0, "draft"), None, ALICE)
        .await
        .unwrap();
    let replacement = TextSync::with_client_id("", 2).insert(0, "resealed");
    store.share_doc(doc, &replacement, BOB).await.unwrap();
    assert_eq!(
        who(&known(&store, doc, 0).await),
        vec![p("SHA256:alice"), p("SHA256:bob")]
    );
}

/// A restore installs content of unrecorded authorship: the origin moves to
/// the restore, so a cutoff before it is Incomplete — never the restorer, and
/// never "nobody". Edits after it are recorded normally.
#[tokio::test]
async fn a_restore_resets_the_origin_so_earlier_cutoffs_are_incomplete() {
    let store = store();
    let doc = "kbn:kb1:concept:x";
    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update(doc, &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap();
    let restorer = Attribution::Internal(InternalWriter::CheckpointRestore);
    let restored = TextSync::with_client_id("", 5).insert(0, "from a backup");
    let w = store
        .restore_doc(doc, &restored, restorer)
        .await
        .unwrap()
        .wal_seq;
    for before in [0, w - 1] {
        assert_eq!(
            store.editors_since_wal(doc, before).await.unwrap(),
            EditorsSince::Incomplete {
                recorded_after_wal_id: w
            }
        );
    }
    assert!(known(&store, doc, w).await.is_empty());
    let mut b = TextSync::from_state_with_client_id(&restored, 2).unwrap();
    store
        .apply_update(doc, &b.insert(0, "x"), None, BOB)
        .await
        .unwrap();
    assert_eq!(who(&known(&store, doc, w).await), vec![p("SHA256:bob")]);
}

/// Pre-existing content gets an origin at upgrade: a database written before
/// the record existed answers Incomplete for a cutoff before its high-water
/// mark — "nobody" would be a lie — and edits after the upgrade are recorded.
/// A document whose origin row is missing anyway reads the same way.
#[tokio::test]
async fn content_from_before_the_record_reads_as_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("collab.db");
    let old = TextSync::with_client_id("", 7).insert(0, "pre-upgrade");
    {
        // A database as a daemon without the editor record left it.
        let conn = sqlite::Connection::open(&path).unwrap();
        conn.execute(
            "CREATE TABLE wal (id INTEGER PRIMARY KEY AUTOINCREMENT, doc_name TEXT NOT NULL,
                 update_bytes BLOB NOT NULL, client_id INTEGER,
                 created_at TEXT NOT NULL DEFAULT (datetime('now')));
             CREATE TABLE snapshots (doc_name TEXT PRIMARY KEY, state BLOB NOT NULL,
                 wal_id INTEGER NOT NULL, hash TEXT NOT NULL DEFAULT '',
                 updated_at TEXT NOT NULL DEFAULT (datetime('now')));",
        )
        .unwrap();
        for doc in ["old", "orphan", "reshared"] {
            let mut s = conn
                .prepare("INSERT INTO wal (doc_name, update_bytes) VALUES (?1, ?2)")
                .unwrap();
            s.bind((1, doc)).unwrap();
            s.bind((2, old.as_slice())).unwrap();
            s.next().unwrap();
        }
    }
    let store = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
    sqlite::Connection::open(&path)
        .unwrap()
        .execute("DELETE FROM doc_editor_origin WHERE doc_name = 'orphan'")
        .unwrap();
    for (doc, hw) in [("old", 1), ("orphan", 2)] {
        assert_eq!(
            store.editors_since_wal(doc, 0).await.unwrap(),
            EditorsSince::Incomplete {
                recorded_after_wal_id: hw
            },
            "{doc}"
        );
    }
    for doc in ["old", "orphan"] {
        let mut a = TextSync::from_state_with_client_id(&old, 1).unwrap();
        store
            .apply_update(doc, &a.insert(0, "x"), None, ALICE)
            .await
            .unwrap();
    }
    assert_eq!(who(&known(&store, "old", 1).await), vec![p("SHA256:alice")]);
    // The first recorded write to an origin-less document starts its record
    // there: what it held before stays unattributed.
    assert_eq!(
        store.editors_since_wal("orphan", 0).await.unwrap(),
        EditorsSince::Incomplete {
            recorded_after_wal_id: 2
        }
    );
    assert_eq!(
        who(&known(&store, "orphan", 2).await),
        vec![p("SHA256:alice")]
    );
    // A share deletes the old content before writing: only the origin set at
    // upgrade remembers that the document held unattributed content, which a
    // full-state share carries forward.
    store.share_doc("reshared", &old, BOB).await.unwrap();
    assert_eq!(
        store.editors_since_wal("reshared", 0).await.unwrap(),
        EditorsSince::Incomplete {
            recorded_after_wal_id: 3
        }
    );
}

/// Deleting an ephemeral document drops its editors and origin (the tables
/// stay bounded); deleting a durable KB doc keeps them.
#[tokio::test]
async fn deleting_forgets_an_ephemeral_docs_editors_but_not_a_durable_docs() {
    let store = store();
    let (ephemeral, durable) = ("file:proj/notes.txt", "kbn:kb1:concept:x");
    for doc in [ephemeral, durable] {
        let mut a = TextSync::with_client_id("", 1);
        store
            .apply_update(doc, &a.insert(0, "a"), None, ALICE)
            .await
            .unwrap();
        store.delete_doc(doc).await.unwrap();
    }
    assert!(known(&store, ephemeral, 0).await.is_empty());
    assert_eq!(known(&store, durable, 0).await.len(), 1);
}

/// Each non-principal kind is recorded as what it is, and every
/// unauthenticated session coalesces into ONE row per document.
#[tokio::test]
async fn non_principal_writers_are_recorded_as_what_they_are() {
    let store = store();
    let writers = [
        Attribution::MeshPeer("SHA256:peer"),
        Attribution::Internal(InternalWriter::CheckpointRestore),
        Attribution::session(None, Transport::Hub),
        Attribution::session(Some(""), Transport::P2p),
    ];
    for (i, by) in writers.into_iter().enumerate() {
        let mut t = TextSync::with_client_id("", i as u64 + 1);
        store
            .apply_update("doc1", &t.insert(0, "x"), None, by)
            .await
            .unwrap();
    }
    let mut rows = known(&store, "doc1", 0).await;
    rows.sort_by_key(|r| r.kind);
    let got: Vec<_> = rows
        .iter()
        .map(|r| (r.kind, r.editor.as_str(), r.updates))
        .collect();
    assert_eq!(
        got,
        vec![
            (EditorKind::Unauthenticated, "unauthenticated", 2),
            (EditorKind::MeshPeer, "SHA256:peer", 1),
            (EditorKind::Internal, "checkpoint-restore", 1),
        ]
    );
}

/// The verdict through the store: principals alone are Known; any hidden
/// author since the cutoff makes the whole set Unknown.
#[tokio::test]
async fn authors_since_wal_refuses_when_an_author_is_hidden() {
    let store = store();
    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update("doc1", &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap();
    let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        store.authors_since_wal("doc1", 0).await.unwrap(),
        Authors::Known(names(&["SHA256:alice"]))
    );
    let mut m = TextSync::with_client_id("", 2);
    let relay = Attribution::MeshPeer("SHA256:relay");
    store
        .apply_update("doc1", &m.insert(0, "m"), None, relay)
        .await
        .unwrap();
    assert!(matches!(
        store.authors_since_wal("doc1", 0).await.unwrap(),
        Authors::Unknown(UnknownAuthors::NotPrincipals(_))
    ));
}

/// N1, the reviewer's repro: a SHARE installs held-back text. Alice plants text
/// on Bob's next item; the cutoff is taken; Bob's client syncs (the full state
/// carries Alice's pending structs), edits, and shares its whole state. The
/// document now holds Alice's text, so she is an editor after the cutoff.
#[tokio::test]
async fn a_share_installing_held_back_text_credits_its_author() {
    let store = store();
    let (_, planted) = planted_on_bobs_next_item();
    let cut = store
        .apply_update("doc1", &planted, None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    let synced = store.encode_state("doc1").await.unwrap();
    let mut bob = TextSync::from_state_with_client_id(&synced, 2).unwrap();
    bob.insert(0, "gap ");
    store
        .share_doc("doc1", &bob.encode_state(), BOB)
        .await
        .unwrap();
    assert_eq!(store.content("doc1").await.unwrap(), "gap PLANTED");

    let rows = known(&store, "doc1", cut).await;
    let mut got = who(&rows);
    got.sort();
    assert_eq!(got, vec![p("SHA256:alice"), p("SHA256:bob")]);
}

/// N3: WAL ids are global to the database, so a cutoff past this document's
/// high-water mark came from somewhere else. It must not read as "nobody".
#[tokio::test]
async fn a_cutoff_past_the_documents_high_water_is_not_nobody() {
    let store = store();
    let mut a = TextSync::with_client_id("", 1);
    let w1 = store
        .apply_update("doc1", &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    let mut b = TextSync::with_client_id("", 2);
    let w2 = store
        .apply_update("doc2", &b.insert(0, "b"), None, BOB)
        .await
        .unwrap()
        .wal_seq;
    let ahead = EditorsSince::CutoffAhead { high_water: w1 };
    assert_eq!(store.editors_since_wal("doc1", w2).await.unwrap(), ahead);
    assert_eq!(
        store.authors_since_wal("doc1", w2).await.unwrap(),
        Authors::Unknown(UnknownAuthors::CutoffAhead { high_water: w1 })
    );
    assert!(
        known(&store, "doc1", w1).await.is_empty(),
        "its own high water is fine"
    );
}

/// Drive `state_with_high_water` against a concurrent `writer`: pause the
/// reader between its two reads, give the writer up to 200ms to finish, then
/// let the reader go. Returns `(content read, high water read, writer's WAL id)`.
async fn race_high_water<F>(store: &Arc<DocStore>, writer: F) -> (String, u64, u64)
where
    F: std::future::Future<Output = u64> + Send + 'static,
{
    let pause = store.pause_next_high_water_read();
    let reader = {
        let s = Arc::clone(store);
        tokio::spawn(async move { s.state_with_high_water("doc1").await.unwrap() })
    };
    pause.reached.notified().await;
    let mut writer = tokio::spawn(writer);
    let finished_first = tokio::time::timeout(std::time::Duration::from_millis(200), &mut writer)
        .await
        .is_ok();
    pause.proceed.notify_one();
    let (state, hw) = reader.await.unwrap();
    let w = writer.await.unwrap();
    assert!(!finished_first, "the writer ran inside the reader's freeze");
    let content = TextSync::from_state(&state).unwrap().content();
    (content, hw, w)
}

/// N2, the reviewer's interleaving, driven step by step: a share passes its
/// size probe; a freeze then locks the OLD entry and reads its content; the
/// share deletes storage, swaps the entry and appends its WAL row — none of
/// which needs the entry lock — and only then does the freeze read the high
/// water. Without the replace gate that pairs "OLD" with the share's WAL id,
/// dropping the sharer from the next revision's authors.
#[tokio::test]
async fn a_freeze_and_a_share_never_interleave() {
    let store = Arc::new(store());
    let old = TextSync::with_client_id("", 1).insert(0, "OLD");
    let w_old = store
        .apply_update("doc1", &old, None, ALICE)
        .await
        .unwrap()
        .wal_seq;

    let replace_pause = store.pause_next_replace();
    let share = {
        let s = Arc::clone(&store);
        let new = TextSync::with_client_id("", 2).insert(0, "NEW");
        tokio::spawn(async move { s.share_doc("doc1", &new, BOB).await.unwrap().wal_seq })
    };
    replace_pause.reached.notified().await;

    let read_pause = store.pause_next_high_water_read();
    let reader = {
        let s = Arc::clone(&store);
        tokio::spawn(async move { s.state_with_high_water("doc1").await.unwrap() })
    };
    // With the gate the reader cannot get this far while the share is mid-way.
    let window = std::time::Duration::from_millis(200);
    let _ = tokio::time::timeout(window, read_pause.reached.notified()).await;
    replace_pause.proceed.notify_one();
    let w_share = share.await.unwrap();
    read_pause.proceed.notify_one();
    let (state, hw) = reader.await.unwrap();

    let content = TextSync::from_state(&state).unwrap().content();
    let consistent = [("OLD".to_string(), w_old), ("NEW".to_string(), w_share)];
    assert!(
        consistent.contains(&(content.clone(), hw)),
        "froze {content:?} at WAL {hw}"
    );
}

/// N2 / M8: `apply_update` appends to the WAL under the entry lock, so the
/// freeze must hold that lock across BOTH reads; reading the high water after
/// releasing it would pair the old content with the edit's WAL id.
#[tokio::test]
async fn a_freeze_and_an_edit_never_interleave() {
    let store = Arc::new(store());
    let mut a = TextSync::with_client_id("", 1);
    let w_old = store
        .apply_update("doc1", &a.insert(0, "OLD"), None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    let s = Arc::clone(&store);
    let edit = a.insert(3, "+bob");
    let apply = async move {
        s.apply_update("doc1", &edit, None, BOB)
            .await
            .unwrap()
            .wal_seq
    };
    let (content, hw, w_edit) = race_high_water(&store, apply).await;
    assert_eq!((content.as_str(), hw), ("OLD", w_old));
    assert!(w_edit > w_old);
}
