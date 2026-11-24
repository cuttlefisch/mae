//! The store's one writer and its background checkpointer
//! (`storage_writer.rs`): writers queue rather than meet a locked database,
//! a commit never checkpoints below the backstop, the WAL stays bounded while
//! no reader pins it and is reported when one does, the checkpointer empties
//! it when idle, every ADR-037 scrub completes once readers let go, even under
//! sustained writes, a panicking tick does not stop the checkpointer, and an
//! acknowledged write survives a kill.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::doc_editors::{Attribution, InternalWriter};
use mae_sync::kb::history::VersionKind;

fn stamp() -> EditStamp {
    EditStamp::new(Attribution::Internal(InternalWriter::Scheduler).into(), 0)
}

/// A checkpointer that never wakes during a test: only the commit path's
/// backstop can checkpoint.
const PAUSED: CheckpointPolicy = CheckpointPolicy {
    interval: Duration::from_secs(3600),
    idle_truncate_after: Duration::from_secs(3600),
    ..CheckpointPolicy::PRODUCTION
};

/// Close a store, failing rather than hanging if the close waits for its
/// checkpointer's next tick, an hour away under [`PAUSED`]: every test whose
/// store ticks that rarely closes it here, so a stop that stopped waking the
/// checkpointer at once fails each of them in seconds (the regression
/// `closing_a_store_does_not_wait_for_the_checkpointers_tick` pins).
fn close(backend: SqliteBackend) {
    let (closed, done) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(backend);
        let _ = closed.send(());
    });
    match done.recv_timeout(Duration::from_secs(30)) {
        Ok(()) => {}
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("closing the store hung: its checkpointer did not stop at once")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            panic!("closing the store panicked")
        }
    }
}

/// An update big enough to take a few pages, so the WAL grows quickly.
const UPDATE: [u8; 3000] = [7; 3000];

fn wal_path(db: &Path) -> PathBuf {
    writer::wal_path(db)
}

/// WAL frames the file holds room for (its high-water mark since the last
/// truncate: SQLite reuses a restarted WAL from its start without shrinking).
fn wal_frames(backend: &SqliteBackend, db: &Path) -> u64 {
    let page = {
        let conn = backend.pool.lock_primary();
        let mut stmt = conn.prepare("PRAGMA page_size").unwrap();
        stmt.next().unwrap();
        stmt.read::<i64, _>(0).unwrap() as u64
    };
    let len = std::fs::metadata(wal_path(db)).map_or(0, |m| m.len());
    len.saturating_sub(32) / (page + 24)
}

/// With `busy_timeout=0`, a write that meets another connection's write lock
/// fails at once with "database is locked" instead of sleeping.
fn fail_fast_on_a_locked_database(backend: &SqliteBackend) {
    for shard in &backend.pool.shards {
        shard
            .lock()
            .unwrap()
            .execute("PRAGMA busy_timeout=0")
            .unwrap();
    }
}

async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

const WRITE_KINDS: usize = 12;

/// One write of each kind the backend has, by `kind`; `(t, i)` keeps keys apart.
async fn write_one(b: &SqliteBackend, kind: usize, t: usize, i: usize) -> Result<(), StorageError> {
    let doc = format!("doc-{t}-{}", i % 8);
    let kb = format!("kb-{t}-{i}");
    match kind {
        0 => b.wal_append(&doc, &UPDATE, None, &stamp()).await.map(drop),
        1 => b.compact(&doc, b"state", 0).await,
        2 => b.delete_document(&doc).await,
        3 => b.forget_editors(&doc).await,
        4 => b.add_block(&kb, "SHA256:p").await,
        5 => b.remove_block(&kb, "SHA256:p").await,
        6 => b.history_cut(version(t, i), 1).await.map(drop),
        7 => b.history_append_head(&head(&kb)).await,
        8 => b.seed_insert(&seed(&kb), &[]).await,
        9 => b.seed_set_node_wal(&kb, "n", 1, "v").await,
        10 => b.seed_complete(&kb, 1).await,
        _ => seed_update_writes(b, &kb).await,
    }
}

/// Every write a seed update makes, in its order: the record it updates, the
/// intent, a node done, the finish.
async fn seed_update_writes(b: &SqliteBackend, kb: &str) -> Result<(), StorageError> {
    b.seed_insert(&seed(kb), &[]).await?;
    let row = crate::storage::SeedNodeRow {
        node_id: "n".into(),
        path: "n.org".into(),
        blob: "b".into(),
        normalised_sha256: "s".into(),
        created_at: 1,
        created_source: "git".into(),
        modified_at: 1,
        modified_source: "git".into(),
        seed_wal_id: None,
        seed_version: None,
    };
    let node = crate::storage::SeedUpdateNode {
        row,
        remove: false,
        base_version: None,
        done: false,
    };
    let u = crate::storage::SeedUpdate {
        kb_id: kb.into(),
        plan_hash: "p2".into(),
        repo: "r".into(),
        rev: "v2".into(),
        commit: "c".into(),
        tree: "t".into(),
        excluded: Vec::new(),
        started_at: 2,
    };
    b.seed_update_begin(&u, &[node]).await?;
    b.seed_update_node_done(kb, "n", Some(1), Some("v")).await?;
    b.seed_update_finish(kb, 3).await
}

fn version(t: usize, i: usize) -> Checkpoint {
    let node = format!("n{t}");
    Checkpoint {
        kb_id: "kb".into(),
        content: mae_sync::kb::KbNodeDoc::new_created_at(&node, &format!("v{i}"), "", &[], 1)
            .materialize(),
        node_id: node,
        kind: VersionKind::Edit,
        name: None,
        summary: None,
        restored_from: None,
    }
}

fn head(kb: &str) -> SignedHead {
    SignedHead {
        kb_id: kb.into(),
        at: 1,
        tips: Vec::new(),
        digest: [1; 32],
        signer: [2; 32],
        signature: [3; 64],
    }
}

fn seed(kb: &str) -> SeedRecord {
    SeedRecord {
        kb_id: kb.into(),
        plan_hash: "p".into(),
        contract: "c".into(),
        repo: "r".into(),
        rev: "v".into(),
        commit: "c".into(),
        tree: "t".into(),
        excluded: Vec::new(),
        started_at: 1,
        applied_at: None,
    }
}

/// Every kind of write, from several tasks at once on every shard, queues for
/// the writer slot: none meets SQLite's write lock held by another shard
/// connection. Kills any write method that skips the slot: with
/// `busy_timeout=0` its first collision is a "database is locked" error, on
/// it or on the writer it collided with.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn every_kind_of_write_queues_instead_of_meeting_a_locked_database() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Arc::new(SqliteBackend::open(&dir.path().join("state.db")).unwrap());
    fail_fast_on_a_locked_database(&backend);
    // Two tasks per kind, plus a steady load of WAL appends.
    let jobs = (0..WRITE_KINDS)
        .flat_map(|kind| [kind, kind])
        .chain([0; 6])
        .enumerate();
    let tasks: Vec<_> = jobs
        .map(|(t, kind)| {
            let b = Arc::clone(&backend);
            tokio::spawn(async move {
                for i in 0..150 {
                    if let Err(e) = write_one(&b, kind, t, i).await {
                        return Err(format!("write kind {kind}, task {t}, #{i}: {e}"));
                    }
                }
                Ok(())
            })
        })
        .collect();
    let mut failed = Vec::new();
    // A deadlock between the writer slot and a shard lock fails, not hangs.
    let joined = tokio::time::timeout(Duration::from_secs(120), async {
        for task in tasks {
            failed.extend(task.await.unwrap().err());
        }
    });
    joined.await.expect("the writers deadlocked");
    assert!(
        failed.is_empty(),
        "{} writes failed: {failed:?}",
        failed.len()
    );
}

/// The writer slot is per process; another process on the same file (an
/// operator command, `doctor`) is still arbitrated by SQLite's own lock and
/// `busy_timeout`. Two stores on one file stand in for two processes: they
/// share no slot, collide, and every write still lands. Kills a shard
/// connection that no longer waits on SQLite's lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_second_store_on_the_same_file_still_waits_on_sqlites_lock() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let stores = [
        Arc::new(SqliteBackend::open(&db).unwrap()),
        Arc::new(SqliteBackend::open(&db).unwrap()),
    ];
    let tasks: Vec<_> = (0..8)
        .map(|t| {
            let b = Arc::clone(&stores[t % 2]);
            tokio::spawn(async move {
                for i in 0..100 {
                    let doc = format!("doc-{t}");
                    let written = b.wal_append(&doc, &UPDATE, Some(i), &stamp()).await;
                    written.map_err(|e| format!("store {}, write {i}: {e}", t % 2))?;
                }
                Ok::<(), String>(())
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    for t in 0..8 {
        let state = stores[0].load_document(&format!("doc-{t}")).await.unwrap();
        assert_eq!(state.unwrap().wal_tail.len(), 100, "doc-{t}");
    }
}

/// A reader that holds the WAL open never makes a writer wait on a
/// checkpoint: the idle TRUNCATE, which holds the writer slot, gives up at
/// once on meeting the reader instead of waiting for it. Kills a checkpointer
/// connection that waits on readers (`busy_timeout` above 0).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reader_never_makes_a_writer_wait_on_a_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(1),
        idle_truncate_after: Duration::from_millis(1),
        ..CheckpointPolicy::PRODUCTION
    };
    let backend = SqliteBackend::open_with_policy(&db, 4, policy).unwrap();
    backend
        .wal_append("doc", &UPDATE, None, &stamp())
        .await
        .unwrap();
    // A read transaction left open, as a long backup's `VACUUM INTO` holds one.
    let reader = sqlite::Connection::open(&db).unwrap();
    reader.execute("BEGIN; SELECT COUNT(*) FROM wal;").unwrap();
    for i in 0..40 {
        tokio::time::sleep(Duration::from_millis(20)).await; // idle: a TRUNCATE is tried
        let started = Instant::now();
        backend
            .wal_append("doc", &UPDATE, None, &stamp())
            .await
            .unwrap();
        let waited = started.elapsed();
        assert!(
            waited < Duration::from_secs(1),
            "write #{i} waited {waited:?} behind a checkpoint"
        );
    }
    reader.execute("COMMIT").unwrap();
}

/// Closing the store leaves every write in the database file itself: the
/// checkpointer is stopped and its connection closed by the time the drop
/// returns, so SQLite's checkpoint-on-last-close runs and removes the WAL.
/// A copy of `state.db` alone, as an operator takes after stopping the
/// daemon, then holds everything. Kills a checkpointer that is not joined.
#[tokio::test]
async fn closing_the_store_leaves_everything_in_the_database_file() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let backend = SqliteBackend::open_with_policy(&db, 4, PAUSED).unwrap();
    for i in 0..50 {
        backend
            .wal_append("doc", &UPDATE, Some(i), &stamp())
            .await
            .unwrap();
    }
    close(backend);
    assert!(!wal_path(&db).exists(), "the WAL outlived the store");
    let copy = tempfile::tempdir().unwrap();
    std::fs::copy(&db, copy.path().join("state.db")).unwrap();
    let reopened = SqliteBackend::open(&copy.path().join("state.db")).unwrap();
    let state = reopened.load_document("doc").await.unwrap().unwrap();
    assert_eq!(state.wal_tail.len(), 50);
}

/// Closing a store stops its checkpointer at once, not at its next tick: the
/// stop disconnects the channel the thread waits on. Every store a test opens,
/// and every daemon shutdown, closes one, so a stop that waited out the tick
/// would cost up to a production interval (1 s) each. Production-policy
/// stores, each closed just after its open: a stop that waited for the tick
/// would take about `CLOSES` intervals in all, four times the bound, while a
/// close takes milliseconds (its own checkpoint of one write). Kills a
/// checkpointer that sleeps through its interval before looking for the stop.
#[tokio::test]
async fn closing_a_store_does_not_wait_for_the_checkpointers_tick() {
    const CLOSES: u32 = 20;
    let dir = tempfile::tempdir().unwrap();
    let mut closing = Duration::ZERO;
    for i in 0..CLOSES {
        let backend = SqliteBackend::open(&dir.path().join(format!("{i}.db"))).unwrap();
        backend
            .wal_append("doc", &UPDATE, None, &stamp())
            .await
            .unwrap();
        let started = Instant::now();
        drop(backend);
        closing += started.elapsed();
    }
    let bound = CheckpointPolicy::PRODUCTION.interval * CLOSES / 4;
    assert!(
        closing < bound,
        "{CLOSES} closes took {closing:?} (bound {bound:?}): a close waits for the tick"
    );
}

/// The checkpointer's TRUNCATE takes the writer slot: a write that arrives
/// while it runs queues behind it rather than meeting its lock. The policy
/// makes the checkpointer empty the WAL after every write; kills a TRUNCATE
/// run without the slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_truncate_never_meets_a_writer() {
    let dir = tempfile::tempdir().unwrap();
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(1),
        idle_truncate_after: Duration::from_millis(1),
        ..CheckpointPolicy::PRODUCTION
    };
    let backend = SqliteBackend::open_with_policy(&dir.path().join("state.db"), 4, policy).unwrap();
    fail_fast_on_a_locked_database(&backend);
    backend.wait_for_scrub().await; // the open's own TRUNCATE is not counted
    let base = backend.pool.writer.emptied();
    // At least 200 writes, and on until 10 idle truncates have run among them:
    // under I/O load a PASSIVE and a TRUNCATE (an fsync each) may not fit the
    // gaps, so a fixed count of writes once saw only 7.
    for i in 0u64.. {
        if i >= 200 && backend.pool.writer.emptied() >= base + 10 {
            break;
        }
        assert!(
            i < 3000,
            "only {} truncates ran among {i} writes: the test raced nothing",
            backend.pool.writer.emptied() - base
        );
        let doc = format!("doc{}", i % 8);
        backend
            .wal_append(&doc, &UPDATE, None, &stamp())
            .await
            .unwrap_or_else(|e| panic!("write #{i} met the checkpointer: {e}"));
        // Gaps of 0 to 40 ms, in a scattered order: the long ones leave time
        // for a PASSIVE and then a TRUNCATE where each fsync takes
        // milliseconds; the rest land a write anywhere inside one.
        tokio::time::sleep(Duration::from_millis(i * 7 % 41)).await;
    }
    // The collision above needs a write to land inside a TRUNCATE lasting
    // microseconds, so it is not certain to happen; this check is: no
    // TRUNCATE ran with the slot free.
    assert_eq!(backend.pool.writer.unslotted_truncates(), 0);
}

/// A commit never runs a checkpoint below the backstop: the database file is
/// untouched by writes that take the WAL past SQLite's default 1000-page
/// auto-checkpoint. Kills leaving `wal_autocheckpoint` at its default.
#[tokio::test]
async fn a_commit_never_checkpoints_below_the_backstop() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let backend = SqliteBackend::open_with_policy(&db, 4, PAUSED).unwrap();
    let before = std::fs::read(&db).unwrap();
    let mut i = 0;
    while wal_frames(&backend, &db) < 1500 {
        let doc = format!("doc{}", i % 16);
        backend
            .wal_append(&doc, &UPDATE, None, &stamp())
            .await
            .unwrap();
        i += 1;
        assert!(
            i < 10_000,
            "the WAL stopped growing: something checkpointed"
        );
    }
    assert!(wal_frames(&backend, &db) < u64::from(PAUSED.backstop_pages));
    assert!(
        std::fs::read(&db).unwrap() == before,
        "a commit checkpointed: the database file changed under {i} writes"
    );
    close(backend);
}

/// Sustained writes keep the WAL under the stated bound, the backstop plus one
/// transaction, with the background checkpointer idle. Kills a commit path
/// with no backstop (`wal_autocheckpoint=0`) or SQLite's default in its place.
#[tokio::test]
async fn sustained_writes_keep_the_wal_under_the_backstop() {
    const BACKSTOP: u32 = 400;
    const WRITES: usize = 2500;
    // The updates alone fill more than three times the bound.
    const _: () = assert!(WRITES * UPDATE.len() / 4096 > 3 * BACKSTOP as usize);
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        backstop_pages: BACKSTOP,
        ..PAUSED
    };
    let backend = SqliteBackend::open_with_policy(&db, 4, policy).unwrap();
    let mut max = 0;
    for i in 0..WRITES {
        let doc = format!("doc{}", i % 16);
        backend
            .wal_append(&doc, &UPDATE, None, &stamp())
            .await
            .unwrap();
        max = max.max(wal_frames(&backend, &db));
    }
    // One of these transactions writes well under 32 pages.
    let bound = u64::from(BACKSTOP) + 32;
    assert!(max <= bound, "the WAL reached {max} frames, over {bound}");
    close(backend);
}

/// The background checkpointer, not the commit, keeps the WAL small: after
/// each write it backfills the WAL, so the next write restarts it from the
/// start, and once writes stop it empties the file. The backstop is set out
/// of reach. Kills a checkpointer that does not run, a PASSIVE that is not
/// issued, and an idle TRUNCATE that is not.
#[tokio::test]
async fn the_checkpointer_keeps_the_wal_small_and_empties_it_when_idle() {
    const WRITES: usize = 200;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(2),
        idle_truncate_after: Duration::from_secs(1),
        backstop_pages: 1_000_000,
        ..CheckpointPolicy::PRODUCTION
    };
    let backend = SqliteBackend::open_with_policy(&db, 4, policy).unwrap();
    let writer = backend.pool.writer.clone();
    // The schema's own commits; a restarted WAL reuses this room.
    let start = wal_frames(&backend, &db);
    for i in 0..WRITES {
        let doc = format!("doc{}", i % 16);
        backend
            .wal_append(&doc, &UPDATE, None, &stamp())
            .await
            .unwrap();
        let written = writer.writes();
        wait_until("a PASSIVE after the write", || writer.covered() >= written).await;
    }
    // Without a restart after each write the file would hold every write
    // (several frames each); with one it holds about one write's worth.
    let frames = wal_frames(&backend, &db);
    assert!(
        frames <= start + 32,
        "the WAL holds {frames} frames after {WRITES} writes (it began with {start})"
    );
    wait_until("the idle TRUNCATE", || wal_frames(&backend, &db) == 0).await;
    assert!(writer.emptied() >= 1);
}

fn dir_contains(dir: &Path, needle: &[u8]) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        std::fs::read(entry.unwrap().path())
            .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle))
    })
}

/// Every compaction's scrub (ADR-037: TRUNCATE so no pre-image lingers at
/// rest) completes, among background checkpoints running every millisecond.
/// Kills a compaction that marks no scrub.
#[tokio::test]
async fn every_compactions_scrub_completes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(1),
        ..PAUSED
    };
    let backend = SqliteBackend::open_with_policy(&db, 4, policy).unwrap();
    for i in 0..100 {
        let canary = format!("SCRUB-CANARY-{i:04}-must-not-survive-compaction");
        let id = backend
            .wal_append("kbc:x", canary.as_bytes(), None, &stamp())
            .await
            .unwrap();
        backend.compact("kbc:x", b"clean", id).await.unwrap();
        backend.wait_for_scrub().await;
        assert!(
            !dir_contains(dir.path(), canary.as_bytes()),
            "round {i}: the compacted update survived on disk"
        );
    }
}

/// The shard a test pins a reader on. Not 0, the primary.
const READER: usize = 3;

fn shard_index(backend: &SqliteBackend, doc: &str) -> usize {
    let shard = backend.pool.shard_for(doc);
    let shards = &backend.pool.shards;
    shards.iter().position(|s| std::ptr::eq(s, shard)).unwrap()
}

/// A document name of the form `{tag}-n` that lives on any shard but
/// [`READER`], so writing it never meets the pinned reader's transaction.
fn doc_off_reader(backend: &SqliteBackend, tag: &str, n: usize) -> String {
    (0..)
        .map(|i| format!("{tag}-{n}-{i}"))
        .find(|d| shard_index(backend, d) != READER)
        .unwrap()
}

/// Leave a read transaction open on shard [`READER`]'s connection, as a long
/// read (a backup's `VACUUM INTO`) holds one: no checkpoint can backfill past
/// its snapshot, and the WAL cannot be restarted or truncated under it.
fn pin_reader(backend: &SqliteBackend) {
    let conn = backend.pool.shards[READER].lock().unwrap();
    conn.execute("BEGIN; SELECT COUNT(*) FROM wal;").unwrap();
}

fn release_reader(backend: &SqliteBackend) {
    let conn = backend.pool.shards[READER].lock().unwrap();
    conn.execute("COMMIT").unwrap();
}

/// Write `canary`, compact it away, and wait until the checkpointer has tried
/// the scrub three times against the pinned reader; the canary is still on
/// disk then, or the test proves nothing.
async fn compact_canary_under_reader(backend: &SqliteBackend, dir: &Path, canary: &[u8]) {
    let doc = doc_off_reader(backend, "kbc", 0);
    let id = backend
        .wal_append(&doc, canary, None, &stamp())
        .await
        .unwrap();
    backend.compact(&doc, b"clean", id).await.unwrap();
    let writer = backend.pool.writer.clone();
    let tried = writer.scrub_attempts();
    wait_until("three scrub attempts", || {
        writer.scrub_attempts() >= tried + 3
    })
    .await;
    assert!(
        writer.scrub_pending(),
        "a scrub completed past a pinned reader"
    );
    assert!(
        dir_contains(dir, canary),
        "precondition: the reader keeps the canary in the WAL (else this proves nothing)"
    );
}

/// A scrub that a reader on another shard blocks is retried, every tick, and
/// completes once the reader lets go. The idle TRUNCATE is out of reach, so
/// only the scrub can empty the WAL. Kills a scrub that is not retried (the
/// compaction's own best-effort TRUNCATE, which this replaced, gave up for
/// good on a reader) and a compaction that marks none.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_scrub_blocked_by_a_reader_completes_once_it_lets_go() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(5),
        idle_truncate_after: Duration::from_secs(3600),
        ..CheckpointPolicy::PRODUCTION
    };
    let backend = SqliteBackend::open_with_policy(&db, 4, policy).unwrap();
    backend.wait_for_scrub().await; // the one every open owes
    pin_reader(&backend);
    let canary = b"READER-SCRUB-CANARY-must-not-survive-the-reader";
    compact_canary_under_reader(&backend, dir.path(), canary).await;
    release_reader(&backend);
    backend.wait_for_scrub().await;
    assert!(
        !dir_contains(dir.path(), canary),
        "the scrub completed and the compacted update is still on disk"
    );
}

/// A scrub completes while writers keep the slot busy back to back, so it is
/// never free and the WAL is never idle: the scrub has to queue for the slot.
/// The WAL is filled ahead of the canary, so the frames that later writes
/// put at the start of a restarted WAL never reach the canary's. Kills a
/// scrub that only tries the slot (`try_lock`) and one that is not retried.
#[tokio::test(flavor = "multi_thread", worker_threads = 6)]
async fn a_scrub_completes_under_sustained_writes() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(5),
        ..CheckpointPolicy::PRODUCTION
    };
    let backend = Arc::new(SqliteBackend::open_with_policy(&db, 4, policy).unwrap());
    backend.wait_for_scrub().await;
    pin_reader(&backend);
    for i in 0..1000 {
        let doc = doc_off_reader(&backend, "fill", i % 8);
        backend
            .wal_append(&doc, &UPDATE, None, &stamp())
            .await
            .unwrap();
    }
    let canary = b"BUSY-SCRUB-CANARY-must-not-survive-sustained-writes";
    compact_canary_under_reader(&backend, dir.path(), canary).await;
    let stop = Arc::new(AtomicBool::new(false));
    let writers: Vec<_> = (0..4)
        .map(|t| {
            let (b, stop) = (Arc::clone(&backend), Arc::clone(&stop));
            let doc = doc_off_reader(&backend, "busy", t);
            tokio::spawn(async move {
                while !stop.load(Ordering::Acquire) {
                    b.wal_append(&doc, b"tick", None, &stamp()).await.unwrap();
                }
            })
        })
        .collect();
    let writer = backend.pool.writer.clone();
    let before = writer.writes();
    wait_until("the writers to start", || writer.writes() > before + 100).await;
    release_reader(&backend);
    backend.wait_for_scrub().await;
    // The writers ran before the scrub and still run after it: it completed
    // among them, not in a pause. (Whether a write lands between the release
    // and the scrub is a race, so it is not asserted; the `try_lock` mutant
    // is what shows the scrub could not have waited for a free slot.)
    let done = writer.writes();
    wait_until("writes after the scrub", || writer.writes() > done + 100).await;
    stop.store(true, Ordering::Release);
    for w in writers {
        w.await.unwrap();
    }
    assert!(
        !dir_contains(dir.path(), canary),
        "the scrub completed and the compacted update is still on disk"
    );
}

/// A compaction and a delete hold the writer slot for their own transaction
/// only, so a write queued behind them waits for that and no checkpoint: they
/// owe a scrub instead of running one. Deterministic, with the checkpointer
/// paused: the database file can only change, and the WAL only shrink, if one
/// of them checkpoints. Kills a compaction or delete that TRUNCATEs inline.
#[tokio::test]
async fn a_compaction_or_delete_runs_no_checkpoint_under_the_slot() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let backend = SqliteBackend::open_with_policy(&db, 4, PAUSED).unwrap();
    let mut last = 0;
    for i in 0..20 {
        last = backend
            .wal_append(&format!("doc{}", i % 2), &UPDATE, None, &stamp())
            .await
            .unwrap();
    }
    let file = std::fs::read(&db).unwrap();
    let frames = wal_frames(&backend, &db);
    backend.compact("doc0", b"state", last).await.unwrap();
    backend.delete_document("doc1").await.unwrap();
    assert!(
        std::fs::read(&db).unwrap() == file,
        "a compaction or delete checkpointed into the database file"
    );
    assert!(
        wal_frames(&backend, &db) >= frames,
        "a compaction or delete truncated the WAL"
    );
    let owed = backend.storage_health().unwrap().scrub_pending;
    assert!(
        owed.is_some_and(|(docs, _)| docs >= 2),
        "the compaction and delete owe no scrub: {owed:?}"
    );
    close(backend);
}

/// `delete_document` is two autocommit DELETEs. If the second fails, the first
/// has already put the deleted snapshot's pre-image in the WAL, so the scrub
/// must be owed even though the delete returns an error. A trigger makes the
/// second statement fail after the first committed. Kills a mark set only
/// after both statements succeed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_that_fails_midway_still_owes_its_scrub() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let backend = SqliteBackend::open_with_policy(&db, 4, PAUSED).unwrap();
    let mut last = 0;
    for _ in 0..3 {
        last = backend
            .wal_append("doc", &UPDATE, None, &stamp())
            .await
            .unwrap();
    }
    backend.compact("doc", b"state", last).await.unwrap();
    backend
        .wal_append("doc", &UPDATE, None, &stamp())
        .await
        .unwrap();
    let before = backend
        .storage_health()
        .unwrap()
        .scrub_pending
        .map(|(d, _)| d);
    {
        let other = sqlite::open(&db).unwrap();
        other
            .execute(
                "CREATE TRIGGER refuse_wal_delete BEFORE DELETE ON wal \
                 BEGIN SELECT RAISE(ABORT, 'refused for the test'); END;",
            )
            .unwrap();
    }
    assert!(
        backend.delete_document("doc").await.is_err(),
        "precondition: the second DELETE fails"
    );
    let after = backend
        .storage_health()
        .unwrap()
        .scrub_pending
        .map(|(d, _)| d);
    assert!(
        after.unwrap_or(0) > before.unwrap_or(0),
        "a delete that failed after removing the snapshot owes no scrub: {before:?} -> {after:?}"
    );
    close(backend);
}

/// A reader held open lets the WAL grow past the backstop, which no
/// checkpoint can prevent; the checkpointer warns, and the store reports it:
/// `storage_health` (`daemon/status`) and `inspect_store` (`doctor`). A scrub
/// the reader blocks past `scrub_warn_after` warns and is reported too.
/// Kills a missing warning, and a report that does not see the WAL or the
/// scrub.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_wal_a_reader_pins_past_the_backstop_is_warned_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(5),
        idle_truncate_after: Duration::from_secs(3600),
        backstop_pages: 64,
        scrub_warn_after: Duration::from_millis(50),
        warn_every: Duration::from_secs(3600),
    };
    let backend = SqliteBackend::open_with_policy(&db, 4, policy).unwrap();
    let writer = backend.pool.writer.clone();
    backend.wait_for_scrub().await;
    assert!(!inspect_store(&db, &policy).unwrap().wal_over_backstop());
    pin_reader(&backend);
    let mut i = 0;
    while !inspect_store(&db, &policy).unwrap().wal_over_backstop() {
        let doc = doc_off_reader(&backend, "grow", i % 8);
        backend
            .wal_append(&doc, &UPDATE, None, &stamp())
            .await
            .unwrap();
        i += 1;
        assert!(
            i < 5_000,
            "the WAL stayed under the backstop with a reader pinned"
        );
    }
    wait_until("the WAL warning", || writer.wal_warnings() >= 1).await;
    let health = backend.storage_health().unwrap();
    assert!(
        health.wal_frames > health.wal_warn_frames,
        "status does not see the WAL past the backstop: {health:?}"
    );
    assert!(health.wal_bytes > 0);
    assert_eq!(health.scrub_pending, None);

    compact_canary_under_reader(&backend, dir.path(), b"REPORTED-SCRUB-CANARY").await;
    wait_until("the scrub warning", || writer.scrub_warnings() >= 1).await;
    let owed = backend.storage_health().unwrap().scrub_pending;
    assert!(owed.is_some_and(|(docs, _)| docs == 1), "{owed:?}");
    assert_eq!(
        writer.wal_warnings(),
        1,
        "the WAL warning is not rate-limited"
    );
    release_reader(&backend);
    backend.wait_for_scrub().await;
    assert_eq!(backend.storage_health().unwrap().scrub_pending, None);
}

/// The scrub mark lives in memory, so a crash loses it; every open owes a
/// scrub to make up for that. The crash is the files copied from a live store
/// whose compaction's scrub never ran (its checkpointer is paused). Kills an
/// open that owes none: a PASSIVE backfills the WAL but leaves the pre-image
/// in it.
#[tokio::test]
async fn a_store_reopened_after_a_crash_scrubs_the_wal_it_finds() {
    let live = tempfile::tempdir().unwrap();
    let db = live.path().join("state.db");
    let canary = b"CRASH-SCRUB-CANARY-must-not-survive-a-restart";
    let backend = SqliteBackend::open_with_policy(&db, 4, PAUSED).unwrap();
    let id = backend
        .wal_append("kbc:x", canary, None, &stamp())
        .await
        .unwrap();
    backend.compact("kbc:x", b"clean", id).await.unwrap();
    let crashed = tempfile::tempdir().unwrap();
    for name in ["state.db", "state.db-wal"] {
        std::fs::copy(live.path().join(name), crashed.path().join(name)).unwrap();
    }
    close(backend);
    assert!(
        dir_contains(crashed.path(), canary),
        "precondition: the crashed store's WAL holds the canary"
    );
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(2),
        idle_truncate_after: Duration::from_secs(3600),
        ..CheckpointPolicy::PRODUCTION
    };
    let reopened =
        SqliteBackend::open_with_policy(&crashed.path().join("state.db"), 4, policy).unwrap();
    reopened.wait_for_scrub().await;
    assert!(
        !dir_contains(crashed.path(), canary),
        "the reopened store kept the compacted update in its WAL"
    );
}

/// A tick that panics is caught and logged, and the checkpointer goes on: the
/// write after it is still checkpointed. Kills a tick run without
/// `catch_unwind`, whose panic ends the thread (and `Drop` used to discard it).
#[tokio::test]
async fn a_panicking_tick_does_not_stop_the_checkpointer() {
    let dir = tempfile::tempdir().unwrap();
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(2),
        idle_truncate_after: Duration::from_secs(3600),
        ..CheckpointPolicy::PRODUCTION
    };
    let backend = SqliteBackend::open_with_policy(&dir.path().join("state.db"), 4, policy).unwrap();
    let writer = backend.pool.writer.clone();
    backend.wait_for_scrub().await;
    writer.panic_next_ticks(3);
    backend
        .wal_append("doc", &UPDATE, None, &stamp())
        .await
        .unwrap();
    let written = writer.writes();
    wait_until("a PASSIVE after the panics", || writer.covered() >= written).await;
    assert_eq!(writer.tick_panics(), 3);
}

/// `doctor`'s inspection beside a live store starts no checkpointer and writes
/// nothing, byte for byte; and where there is no store it creates none.
/// Kills an inspection that opens the store as the daemon does (a second
/// checkpointer, and schema DDL outside the writer slot).
#[tokio::test]
async fn inspecting_a_live_store_starts_no_checkpointer_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy::PRODUCTION;
    assert_eq!(
        inspect_store(&db, &policy).unwrap(),
        StoreInspection::Absent
    );
    assert!(!db.exists(), "inspecting created the store");

    let live = SqliteBackend::open_with_policy(&db, 4, PAUSED).unwrap();
    for i in 0..20 {
        live.wal_append("doc", &UPDATE, Some(i), &stamp())
            .await
            .unwrap();
    }
    let files = || {
        (
            std::fs::read(&db).unwrap(),
            std::fs::read(wal_path(&db)).unwrap(),
        )
    };
    let before = files();
    let spawned = WriterLock::spawned_on_this_thread();
    assert!(
        spawned >= 1,
        "the live store's own checkpointer is not counted"
    );
    let found = inspect_store(&db, &policy).unwrap();
    assert_eq!(
        WriterLock::spawned_on_this_thread(),
        spawned,
        "inspecting started a checkpointer"
    );
    assert!(
        matches!(found, StoreInspection::Present { wal_bytes, .. } if wal_bytes > 0),
        "{found:?}"
    );
    assert!(files() == before, "inspecting wrote to the store");
    close(live);
}

const CRASH_CHILD_DIR: &str = "MAE_STORAGE_CRASH_CHILD_DIR";
const CRASH_DOCS: u64 = 8;

/// The crash test's child: writes from four tasks until it is killed,
/// printing `ACK n` once update `n`'s `wal_append` has returned Ok. It does
/// nothing unless the parent test started it with [`CRASH_CHILD_DIR`].
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "the crash test's child process; started only by its parent test"]
async fn crash_child_writes_until_killed() {
    let Some(dir) = std::env::var_os(CRASH_CHILD_DIR) else {
        return;
    };
    // Checkpoints as often as possible, so a kill can land inside one.
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(1),
        idle_truncate_after: Duration::from_millis(1),
        backstop_pages: 64,
        ..CheckpointPolicy::PRODUCTION
    };
    let path = Path::new(&dir).join("state.db");
    let backend = Arc::new(SqliteBackend::open_with_policy(&path, 4, policy).unwrap());
    let tasks: Vec<_> = (0..4u64)
        .map(|t| {
            let b = Arc::clone(&backend);
            tokio::spawn(async move {
                for n in (t..1_000_000).step_by(4) {
                    let doc = format!("doc{}", n % CRASH_DOCS);
                    let update = format!("u{n:07}-{}", "x".repeat(500));
                    b.wal_append(&doc, update.as_bytes(), Some(n), &stamp())
                        .await
                        .unwrap();
                    println!("ACK {n}");
                }
            })
        })
        .collect();
    for task in tasks {
        task.await.unwrap();
    }
}

/// Kill the store's process while it writes, reopen it: every write that was
/// acknowledged is there. The child checkpoints every millisecond, so the
/// kill lands among writes and checkpoints alike.
#[test]
fn an_acknowledged_write_survives_a_kill_mid_write() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "storage::writer_tests::crash_child_writes_until_killed",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CRASH_CHILD_DIR, dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut acked = Vec::new();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    while acked.len() < 3000 {
        let line = lines
            .next()
            .expect("the child stopped before the kill")
            .unwrap();
        if let Some(n) = line.strip_prefix("ACK ") {
            acked.push(n.parse::<u64>().unwrap());
        }
    }
    let running = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(
        running,
        "the child had finished: the kill was not mid-write"
    );

    let found = stored_updates(&dir.path().join("state.db"));
    let lost: Vec<_> = acked.iter().filter(|n| !found.contains(n)).collect();
    assert!(
        lost.is_empty(),
        "{} acknowledged writes lost: {lost:?}",
        lost.len()
    );
}

/// Every update number the store holds, read through a fresh open.
fn stored_updates(db: &Path) -> std::collections::HashSet<u64> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let backend = SqliteBackend::open(db).unwrap();
    let mut found = std::collections::HashSet::new();
    for d in 0..CRASH_DOCS {
        let state = rt.block_on(backend.load_document(&format!("doc{d}")));
        for entry in state.unwrap().map_or(Vec::new(), |s| s.wal_tail) {
            let text = String::from_utf8(entry.update).unwrap();
            found.insert(text[1..8].parse::<u64>().unwrap());
        }
    }
    found
}
