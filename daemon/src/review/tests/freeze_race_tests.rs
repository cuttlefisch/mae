//! A2 driven step by step: an edit, a share or a restore racing the freeze
//! never yields frozen content and a cutoff that disagree, and the history
//! version the freeze cuts covers exactly that cutoff. Two orders for each
//! writer: the freeze paused between its content and high-water reads with
//! the writer let loose, and the writer paused mid-way with the freeze let
//! loose. Kills a freeze that releases the document's lock (or never takes
//! its replace gate) before reading the high-water mark.

use std::time::Duration;

use super::*;
use crate::doc_store::{FreezeError, Frozen};

#[derive(Clone, Copy, Debug)]
enum Writer {
    Edit,
    Share,
    Restore,
}

const WRITERS: [Writer; 3] = [Writer::Edit, Writer::Share, Writer::Restore];
const WINDOW: Duration = Duration::from_millis(200);

/// The node `n1` titled `title`, as a whole-document replacement.
fn replacement(title: &str) -> Vec<u8> {
    KbNodeDoc::new("n1", title, "Body of n1.", &[]).encode_state()
}

/// Run `w`, titling n1 "NEW"; returns its WAL id.
async fn write(store: Arc<DocStore>, w: Writer) -> u64 {
    let doc = mae_sync::kb_node_doc_name(KB, "n1");
    let by = Attribution::Principal("bob");
    match w {
        Writer::Edit => edit_with(&store, KB, "n1", "bob", |d| d.set_title("NEW")).await,
        Writer::Share => {
            store
                .share_doc(&doc, &replacement("NEW"), by)
                .await
                .unwrap()
                .wal_seq
        }
        Writer::Restore => {
            store
                .restore_doc(&doc, &replacement("NEW"), by)
                .await
                .unwrap()
                .wal_seq
        }
    }
}

/// An env whose n1 is titled "OLD", with that write's WAL id and version.
async fn old_node() -> (Env, u64, String) {
    let env = env().await;
    let w_old = retitle(&env, "n1", "OLD").await;
    let v = version(&env, "n1").await;
    (env, w_old, v)
}

fn freeze(env: &Env, v: &str) -> tokio::task::JoinHandle<Result<Frozen, FreezeError>> {
    let (s, v) = (Arc::clone(&env.store), v.to_string());
    tokio::spawn(async move { s.review_freeze(KB, "n1", &v).await })
}

/// The frozen content and cutoff agree with one of the two states the
/// document passed through, and the freeze's history version covers exactly
/// that cutoff and holds exactly that content.
async fn assert_consistent(env: &Env, f: &Frozen, w_old: u64, w_new: u64) {
    let pair = (f.node.title.as_str(), f.cutoff_wal);
    assert!(
        [("OLD", w_old), ("NEW", w_new)].contains(&pair),
        "froze {pair:?}; the document was OLD at {w_old} and NEW at {w_new}"
    );
    let h = env.store.history_list(KB, "n1").await.unwrap();
    let v = h.iter().find(|e| e.seq == f.history_seq).unwrap();
    assert_eq!(v.wal_to, f.cutoff_wal);
    assert_eq!(v.node().unwrap().unwrap(), f.node);
}

#[tokio::test]
async fn a_writer_cannot_land_inside_a_paused_freeze() {
    for w in WRITERS {
        let (env, w_old, v) = old_node().await;
        let pause = env.store.pause_next(|p| &p.freeze);
        let frozen = freeze(&env, &v);
        pause.reached.notified().await;
        let mut writer = tokio::spawn(write(Arc::clone(&env.store), w));
        let finished_inside = tokio::time::timeout(WINDOW, &mut writer).await.is_ok();
        pause.proceed.notify_one();
        let f = frozen.await.unwrap().unwrap();
        let w_new = writer.await.unwrap();
        assert!(!finished_inside, "{w:?} landed inside the freeze");
        assert_eq!(
            (f.node.title.as_str(), f.cutoff_wal),
            ("OLD", w_old),
            "{w:?}"
        );
        assert!(w_new > f.cutoff_wal);
        assert_consistent(&env, &f, w_old, w_new).await;
    }
}

#[tokio::test]
async fn a_freeze_racing_a_paused_writer_freezes_one_whole_state() {
    for w in WRITERS {
        let (env, w_old, v) = old_node().await;
        let pause = match w {
            Writer::Edit => env.store.pause_next(|p| &p.apply),
            Writer::Share | Writer::Restore => env.store.pause_next(|p| &p.replace),
        };
        let writer = tokio::spawn(write(Arc::clone(&env.store), w));
        pause.reached.notified().await;
        let mut frozen = freeze(&env, &v);
        let early = tokio::time::timeout(WINDOW, &mut frozen).await;
        pause.proceed.notify_one();
        let w_new = writer.await.unwrap();
        let result = match early {
            Ok(r) => r.unwrap(),
            Err(_) => frozen.await.unwrap(),
        };
        match result {
            Ok(f) => assert_consistent(&env, &f, w_old, w_new).await,
            // The writer won: the version the requester saw is gone, and
            // the freeze says so rather than freezing the new content.
            Err(FreezeError::Stale { current }) => {
                assert_eq!(current, version(&env, "n1").await, "{w:?}")
            }
            Err(e) => panic!("{w:?}: {e:?}"),
        }
    }
}

/// Two requests for one node, the first paused inside its freeze while the
/// second passes its own check for an open revision and waits on the node:
/// exactly one opens, and the other is refused `invalid_transition` under
/// the writer, writing no revision. Kills a request recorded without
/// re-checking under the writer.
#[tokio::test]
async fn two_requests_racing_for_one_node_open_one() {
    let env = env().await;
    enable(&env).await;
    let v = version(&env, "n1").await;
    let ask = |who: String| {
        let (store, v) = (Arc::clone(&env.store), v.clone());
        tokio::spawn(async move {
            call_on(&store, &who, "kb/review.request",
                    serde_json::json!({"kb_id": KB, "nodes": [{"node_id": "n1", "version": v}], "reason": "r"}))
                .await
        })
    };
    let pause = env.store.pause_next(|p| &p.freeze);
    let first = ask(env.editor.clone());
    pause.reached.notified().await;
    let mut second = ask(env.owner.clone());
    let finished_inside = tokio::time::timeout(WINDOW, &mut second).await.is_ok();
    pause.proceed.notify_one();
    assert!(
        !finished_inside,
        "the second request froze the node inside the first's freeze"
    );
    first.await.unwrap().unwrap();
    assert_eq!(err_kind(second.await.unwrap()), "invalid_transition");
    let open = env
        .store
        .review_read(|c| crate::review::store::node_revisions(c, KB, "n1"))
        .await
        .unwrap();
    assert_eq!(open.len(), 1);
}
