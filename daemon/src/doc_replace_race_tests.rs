//! Writes racing a whole-document replace (`share_doc`).
//!
//! A replace deletes storage, swaps the in-memory entry and appends its WAL row
//! outside any entry lock. Anything that already holds the OLD entry — an edit,
//! a compaction — must be ordered wholly before or wholly after it, or memory
//! and disk disagree and an acknowledged edit can vanish. Every test checks
//! memory against disk (a fresh `DocStore` over the same storage).

use std::sync::Arc;
use std::time::Duration;

use mae_sync::text::TextSync;

use super::*;
use crate::storage::SqliteBackend;

const DOC: &str = "kbn:kb1:concept:x";
/// Long enough for an unblocked in-memory write to finish many times over.
const WINDOW: Duration = Duration::from_millis(200);

/// A store holding "OLD", and its storage.
async fn store_with_old() -> (Arc<DocStore>, Arc<SqliteBackend>) {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = Arc::new(DocStore::new(backend.clone(), 500));
    let old = TextSync::with_client_id("", 1).insert(0, "OLD");
    store.apply_update(DOC, &old, None).await.unwrap();
    (store, backend)
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
        s.share_doc(DOC, &new).await.unwrap().wal_seq
    })
}

fn carol_edits(store: &Arc<DocStore>) -> tokio::task::JoinHandle<u64> {
    let s = Arc::clone(store);
    tokio::spawn(async move {
        let edit = TextSync::with_client_id("", 3).insert(0, "CAROL");
        s.apply_update(DOC, &edit, None).await.unwrap().wal_seq
    })
}

/// Memory equals disk; returns the content.
async fn assert_consistent(store: &Arc<DocStore>, backend: &Arc<SqliteBackend>) -> String {
    let memory = store.content(DOC).await.unwrap();
    assert_eq!(memory, on_disk(backend).await, "memory and disk disagree");
    memory
}

/// Give `task` up to [`WINDOW`]: its output if it finished, else the task.
async fn within_window<T>(
    mut task: tokio::task::JoinHandle<T>,
) -> Result<T, tokio::task::JoinHandle<T>> {
    match tokio::time::timeout(WINDOW, &mut task).await {
        Ok(out) => Ok(out.unwrap()),
        Err(_) => Err(task),
    }
}

/// The output of a [`within_window`] task, waiting for it if it is still running.
async fn finish<T>(early: Result<T, tokio::task::JoinHandle<T>>) -> T {
    match early {
        Ok(out) => out,
        Err(task) => task.await.unwrap(),
    }
}

/// An edit that already holds the OLD entry when a share starts: the share
/// waits for it. The edit lands on the old document and the share then
/// replaces that document — the edit is superseded, exactly as an edit made a
/// moment earlier would be, and memory and disk agree on it. Before, the share
/// ran over it: the edit applied to the discarded entry in memory while its
/// WAL row landed on the new document.
#[tokio::test]
async fn an_edit_holding_the_old_entry_completes_before_the_replace() {
    let (store, backend) = store_with_old().await;
    let pause = store.pause_next(|p| &p.apply);
    let edit = carol_edits(&store);
    pause.reached.notified().await;

    let replace = within_window(share(&store)).await;
    let ran_through = replace.is_ok();
    pause.proceed.notify_one();
    let (w_carol, w_share) = (edit.await.unwrap(), finish(replace).await);
    assert_eq!(assert_consistent(&store, &backend).await, "NEW");
    assert!(w_carol < w_share);
    assert!(!ran_through, "the share ran over a live edit");
}

/// An edit that arrives while a share is mid-way waits for it, then lands on
/// the NEW document, in memory and on disk. Before, it applied to the old
/// entry and was deleted with it while its client was told it succeeded.
#[tokio::test]
async fn an_edit_arriving_during_a_replace_lands_on_the_new_document() {
    let (store, backend) = store_with_old().await;
    let pause = store.pause_next(|p| &p.replace);
    let replace = share(&store);
    pause.reached.notified().await;

    let edit = within_window(carol_edits(&store)).await;
    let ran_through = edit.is_ok();
    pause.proceed.notify_one();
    let (w_share, w_carol) = (replace.await.unwrap(), finish(edit).await);
    let content = assert_consistent(&store, &backend).await;
    assert!(
        content.contains("NEW") && content.contains("CAROL"),
        "acknowledged edit lost: {content:?}"
    );
    assert!(w_share < w_carol);
    assert!(!ran_through, "the edit ran inside the share");
}

/// A compaction that has read the OLD state when a share starts: the share
/// waits for its snapshot. Before, the old snapshot could be written over the
/// new document's storage, so a restart would load a mix of both.
#[tokio::test]
async fn a_compaction_holding_the_old_state_completes_before_the_replace() {
    let (store, backend) = store_with_old().await;
    let pause = store.pause_next(|p| &p.compact);
    let compaction = {
        let s = Arc::clone(&store);
        tokio::spawn(async move { s.compact_doc(DOC).await.unwrap() })
    };
    pause.reached.notified().await;

    let replace = within_window(share(&store)).await;
    let ran_through = replace.is_ok();
    pause.proceed.notify_one();
    compaction.await.unwrap();
    finish(replace).await;
    assert_eq!(assert_consistent(&store, &backend).await, "NEW");
    assert!(!ran_through, "the share ran over a compaction");
}

/// Backstop: truly concurrent shares and edits on a multi-threaded runtime. Whatever the
/// order, memory equals disk, and the edit is either superseded (it went
/// first) or present (it went second) — never acknowledged and then lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_edits_and_replaces_never_diverge_or_vanish() {
    for _ in 0..500 {
        let (store, backend) = store_with_old().await;
        let (replace, edit) = (share(&store), carol_edits(&store));
        let (w_share, w_carol) = (replace.await.unwrap(), edit.await.unwrap());
        let content = assert_consistent(&store, &backend).await;
        if w_carol > w_share {
            assert!(
                content.contains("CAROL"),
                "acknowledged edit lost: {content:?}"
            );
        } else {
            assert_eq!(content, "NEW");
        }
    }
}
