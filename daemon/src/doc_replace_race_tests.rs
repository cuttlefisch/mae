//! Writes racing a whole-document replace (`share_doc`/`restore_doc`).
//!
//! A replace deletes storage, swaps the in-memory entry and appends its WAL row
//! outside any entry lock. Anything that already holds the OLD entry — an edit,
//! a compaction — must be ordered wholly before or wholly after it, or memory
//! and disk disagree and an acknowledged edit can vanish. Each interleaving is
//! driven deterministically with the `cfg(test)` pause seams; a stress loop is
//! the backstop. Every test checks memory against disk (a fresh `DocStore` over
//! the same storage).

use std::sync::Arc;
use std::time::Duration;

use mae_sync::text::TextSync;

use super::*;
use crate::doc_editors::{Attribution, Authors, EditorsSince};
use crate::storage::SqliteBackend;

const ALICE: Attribution<'static> = Attribution::Principal("SHA256:alice");
const BOB: Attribution<'static> = Attribution::Principal("SHA256:bob");
const CAROL: Attribution<'static> = Attribution::Principal("SHA256:carol");
const DOC: &str = "kbn:kb1:concept:x";
/// Long enough for an unblocked in-memory write to finish many times over.
const WINDOW: Duration = Duration::from_millis(200);

/// A store holding "OLD" (by Alice), and its storage.
async fn store_with_old() -> (Arc<DocStore>, Arc<SqliteBackend>, u64) {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = Arc::new(DocStore::new(backend.clone(), 500));
    let old = TextSync::with_client_id("", 1).insert(0, "OLD");
    let w_old = store
        .apply_update(DOC, &old, None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    (store, backend, w_old)
}

/// What storage holds, read back the way a restarted daemon would.
async fn on_disk(backend: &Arc<SqliteBackend>) -> String {
    let fresh = DocStore::new(backend.clone() as Arc<dyn StorageBackend>, 500);
    fresh.content(DOC).await.unwrap()
}

fn share(store: &Arc<DocStore>) -> tokio::task::JoinHandle<u64> {
    let s = Arc::clone(store);
    tokio::spawn(async move {
        let new = TextSync::with_client_id("", 2).insert(0, "NEW");
        s.share_doc(DOC, &new, BOB).await.unwrap().wal_seq
    })
}

fn carol_edits(store: &Arc<DocStore>) -> tokio::task::JoinHandle<u64> {
    let s = Arc::clone(store);
    tokio::spawn(async move {
        let edit = TextSync::with_client_id("", 3).insert(0, "CAROL");
        s.apply_update(DOC, &edit, None, CAROL)
            .await
            .unwrap()
            .wal_seq
    })
}

/// Whether `task` finished within [`WINDOW`] (it is not consumed).
async fn finishes_in_window<T>(task: &mut tokio::task::JoinHandle<T>) -> bool {
    tokio::time::timeout(WINDOW, task).await.is_ok()
}

/// Memory equals disk, and a freeze pairs that content with a matching cutoff.
async fn assert_consistent(store: &Arc<DocStore>, backend: &Arc<SqliteBackend>) -> String {
    let memory = store.content(DOC).await.unwrap();
    assert_eq!(memory, on_disk(backend).await, "memory and disk disagree");
    let (state, hw) = store.state_with_high_water(DOC).await.unwrap();
    assert_eq!(TextSync::from_state(&state).unwrap().content(), memory);
    assert_eq!(hw, store.storage.wal_high_water(DOC).await.unwrap());
    memory
}

/// An edit that already holds the OLD entry when a share starts: the share
/// waits for it. The edit lands on the old document and the share then
/// replaces that document — the edit is superseded, exactly as an edit made a
/// moment earlier would be, and memory and disk agree on it.
#[tokio::test]
async fn an_edit_holding_the_old_entry_completes_before_the_replace() {
    let (store, backend, _) = store_with_old().await;
    let pause = store.pause_next(|p| &p.apply);
    let edit = carol_edits(&store);
    pause.reached.notified().await;

    let mut replace = share(&store);
    assert!(
        !finishes_in_window(&mut replace).await,
        "the share ran over a live edit"
    );
    pause.proceed.notify_one();
    let (w_carol, w_share) = (edit.await.unwrap(), replace.await.unwrap());

    assert!(w_carol < w_share);
    assert_eq!(assert_consistent(&store, &backend).await, "NEW");
    assert_eq!(
        store.editors_since_wal(DOC, w_share).await.unwrap(),
        EditorsSince::Known(vec![])
    );
}

/// An edit that arrives while a share is mid-way waits for it, then lands on
/// the NEW document: in memory, on disk, and in the editor record after the
/// share. Before, it applied to the old entry and was deleted with it while
/// its client was told it succeeded.
#[tokio::test]
async fn an_edit_arriving_during_a_replace_lands_on_the_new_document() {
    let (store, backend, _) = store_with_old().await;
    let pause = store.pause_next(|p| &p.replace);
    let replace = share(&store);
    pause.reached.notified().await;

    let mut edit = carol_edits(&store);
    assert!(
        !finishes_in_window(&mut edit).await,
        "the edit ran inside the share"
    );
    pause.proceed.notify_one();
    let (w_share, w_carol) = (replace.await.unwrap(), edit.await.unwrap());

    assert!(w_share < w_carol);
    let content = assert_consistent(&store, &backend).await;
    assert!(
        content.contains("NEW") && content.contains("CAROL"),
        "{content:?}"
    );
    let carol: std::collections::BTreeSet<String> = ["SHA256:carol".to_string()].into();
    assert_eq!(
        store.authors_since_wal(DOC, w_share).await.unwrap(),
        Authors::Known(carol)
    );
}

/// A compaction that has read the OLD state when a share starts: the share
/// waits for its snapshot. Before, the old snapshot could be written over the
/// new document's storage, so a restart would load a mix of both.
#[tokio::test]
async fn a_compaction_holding_the_old_state_completes_before_the_replace() {
    let (store, backend, _) = store_with_old().await;
    let pause = store.pause_next(|p| &p.compact);
    let compaction = {
        let s = Arc::clone(&store);
        tokio::spawn(async move { s.compact_doc(DOC).await.unwrap() })
    };
    pause.reached.notified().await;

    let mut replace = share(&store);
    assert!(
        !finishes_in_window(&mut replace).await,
        "the share ran over a compaction"
    );
    pause.proceed.notify_one();
    compaction.await.unwrap();
    replace.await.unwrap();
    assert_eq!(assert_consistent(&store, &backend).await, "NEW");
}

/// Backstop: unpaused, truly concurrent shares and edits on a multi-threaded
/// runtime. Whatever the order, memory equals disk, and the edit is either
/// superseded (it went first) or present and attributed (it went second).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_and_replaces_never_diverge_or_vanish() {
    for _ in 0..200 {
        let (store, backend, _) = store_with_old().await;
        let (replace, edit) = (share(&store), carol_edits(&store));
        let (w_share, w_carol) = (replace.await.unwrap(), edit.await.unwrap());
        let content = assert_consistent(&store, &backend).await;
        if w_carol > w_share {
            assert!(
                content.contains("CAROL"),
                "acknowledged edit lost: {content:?}"
            );
            let editors = store.editors_since_wal(DOC, w_share).await.unwrap();
            assert!(
                matches!(editors, EditorsSince::Known(ref r) if r.len() == 1),
                "{editors:?}"
            );
        } else {
            assert_eq!(content, "NEW");
        }
    }
}

/// A restart reloads a compacted document with no WAL tail; its next
/// compaction must not lower the document's WAL high-water mark. Otherwise
/// every cutoff frozen before the restart would read as `CutoffAhead` — not
/// unsafe (the verdict is Unknown), but every revision would be blocked.
#[tokio::test]
async fn compaction_after_a_restart_keeps_the_high_water_mark() {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = DocStore::new(backend.clone(), 500);
    let mut a = TextSync::with_client_id("", 1);
    store
        .apply_update(DOC, &a.insert(0, "a"), None, ALICE)
        .await
        .unwrap();
    let w = store
        .apply_update(DOC, &a.insert(1, "b"), None, ALICE)
        .await
        .unwrap()
        .wal_seq;
    store.compact_doc(DOC).await.unwrap();

    let restarted = DocStore::new(backend.clone(), 500);
    restarted.compact_doc(DOC).await.unwrap();
    assert_eq!(restarted.storage.wal_high_water(DOC).await.unwrap(), w);
    assert_eq!(
        restarted.editors_since_wal(DOC, w).await.unwrap(),
        EditorsSince::Known(vec![])
    );
}

// ---- Eviction racing an in-flight edit ----------------------------------

/// Give `task` up to [`WINDOW`]: its output if it finished, else the task.
async fn within_window<T>(
    mut task: tokio::task::JoinHandle<T>,
) -> Result<T, tokio::task::JoinHandle<T>> {
    match tokio::time::timeout(WINDOW, &mut task).await {
        Ok(out) => Ok(out.unwrap()),
        Err(_) => Err(task),
    }
}

/// The output of a [`within_window`] task, waiting for it if still running.
async fn finish<T>(early: Result<T, tokio::task::JoinHandle<T>>) -> T {
    match early {
        Ok(out) => out,
        Err(task) => task.await.unwrap(),
    }
}

const ONE: Attribution<'static> = Attribution::Principal("SHA256:one");
const THREE: Attribution<'static> = Attribution::Principal("SHA256:three");

fn edits(
    store: &Arc<DocStore>,
    cid: u64,
    text: &'static str,
    by: Attribution<'static>,
) -> tokio::task::JoinHandle<u64> {
    let s = Arc::clone(store);
    tokio::spawn(async move {
        let edit = TextSync::with_client_id("", cid).insert(0, text);
        s.apply_update(DOC, &edit, None, by).await.unwrap().wal_seq
    })
}

/// After ONE and THREE: both are in memory and on disk, the freeze pairs that
/// content with the latest WAL id, and both are authors since the OLD write.
async fn assert_one_and_three_kept(
    store: &Arc<DocStore>,
    backend: &Arc<SqliteBackend>,
    w_old: u64,
) {
    let content = assert_consistent(store, backend).await;
    assert!(
        content.contains("ONE") && content.contains("THREE"),
        "edit lost: {content:?}"
    );
    let names = ["SHA256:one", "SHA256:three"].map(String::from).into();
    assert_eq!(
        store.authors_since_wal(DOC, w_old).await.unwrap(),
        Authors::Known(names)
    );
}

/// The reviewer's repro, `evict_idle` path: edit ONE holds the entry but has
/// not locked it; an idle-eviction drops that entry; edit THREE loads a fresh
/// one; then ONE applies to the dropped entry while its WAL row lands on disk —
/// memory "OLDTHREE", disk "OLDONETHREE", and ONE's author missed by a freeze.
/// The eviction must wait for ONE.
#[tokio::test]
async fn idle_eviction_waits_for_an_edit_holding_the_entry() {
    let (store, backend, w_old) = store_with_old().await;
    let pause = store.pause_next(|p| &p.apply);
    let one = edits(&store, 11, "ONE", ONE);
    pause.reached.notified().await;

    let evict = {
        let s = Arc::clone(&store);
        within_window(tokio::spawn(async move { s.evict_idle(0).await })).await
    };
    let ran_through = evict.is_ok();
    let three = within_window(edits(&store, 13, "THREE", THREE)).await;
    pause.proceed.notify_one();
    one.await.unwrap();
    finish(evict).await;
    finish(three).await;

    assert_one_and_three_kept(&store, &backend, w_old).await;
    assert!(
        !ran_through,
        "eviction dropped an entry an edit was holding"
    );
}

/// The same race through the LRU eviction in `get_or_create`, which runs when
/// the in-memory working set is full and another document is loaded.
#[tokio::test]
async fn lru_eviction_skips_an_entry_an_edit_is_holding() {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = Arc::new(DocStore::new(backend.clone(), 500).with_max_documents(1));
    let old = TextSync::with_client_id("", 1).insert(0, "OLD");
    let w_old = store
        .apply_update(DOC, &old, None, ALICE)
        .await
        .unwrap()
        .wal_seq;

    let pause = store.pause_next(|p| &p.apply);
    let one = edits(&store, 11, "ONE", ONE);
    pause.reached.notified().await;
    // Loading another document fills the working set: DOC is the LRU victim.
    let other = TextSync::with_client_id("", 5).insert(0, "other");
    store
        .apply_update("file:proj/other.txt", &other, None, BOB)
        .await
        .unwrap();
    let three = within_window(edits(&store, 13, "THREE", THREE)).await;
    pause.proceed.notify_one();
    one.await.unwrap();
    finish(three).await;

    assert_one_and_three_kept(&store, &backend, w_old).await;
}

/// Backstop: edits and idle-evictions truly concurrent on a multi-threaded
/// runtime. Every acknowledged edit is in memory and on disk.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_and_evictions_never_lose_an_edit() {
    const TEXTS: [&str; 4] = ["AAA", "BBB", "CCC", "DDD"];
    for _ in 0..200 {
        let (store, backend, _) = store_with_old().await;
        let writers: Vec<_> = TEXTS
            .iter()
            .enumerate()
            .map(|(i, t)| edits(&store, 20 + i as u64, t, CAROL))
            .collect();
        let evictors: Vec<_> = (0..2)
            .map(|_| {
                let s = Arc::clone(&store);
                tokio::spawn(async move { s.evict_idle(0).await })
            })
            .collect();
        for w in writers {
            w.await.unwrap();
        }
        for e in evictors {
            e.await.unwrap();
        }
        let content = assert_consistent(&store, &backend).await;
        assert!(
            TEXTS.iter().all(|t| content.contains(t)),
            "edit lost: {content:?}"
        );
    }
}

/// Two compactions of one document: A reads the state, an edit lands, B reads
/// the newer state, writes its snapshot and trims the WAL up to the edit; then
/// A writes. A's snapshot is stale — writing it would replace B's snapshot
/// with one that lacks the edit whose WAL row B already trimmed, losing it on
/// disk. A stale compaction must change nothing.
#[tokio::test]
async fn a_stale_compaction_never_overwrites_a_newer_snapshot() {
    let (store, backend, _) = store_with_old().await;
    let pause = store.pause_next(|p| &p.compact);
    let stale = {
        let s = Arc::clone(&store);
        tokio::spawn(async move { s.compact_doc(DOC).await.unwrap() })
    };
    pause.reached.notified().await;
    edits(&store, 11, "ONE", ONE).await.unwrap();
    store.compact_doc(DOC).await.unwrap();
    pause.proceed.notify_one();
    stale.await.unwrap();

    let content = assert_consistent(&store, &backend).await;
    assert!(content.contains("ONE"), "edit lost: {content:?}");
}
