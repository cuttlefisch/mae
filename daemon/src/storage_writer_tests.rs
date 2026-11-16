//! The store's one writer and its background checkpointer
//! (`storage_writer.rs`): writers queue rather than meet a locked database,
//! a commit never checkpoints below the backstop, the WAL stays bounded, the
//! checkpointer empties it when idle, and an acknowledged write survives a
//! kill.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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
    backstop_pages: CheckpointPolicy::PRODUCTION.backstop_pages,
};

/// An update big enough to take a few pages, so the WAL grows quickly.
const UPDATE: [u8; 3000] = [7; 3000];

fn wal_path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-wal");
    PathBuf::from(p)
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

const WRITE_KINDS: usize = 11;

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
        9 => b.seed_set_node_wal(&kb, "n", 1).await,
        _ => b.seed_complete(&kb, 1).await,
    }
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
    drop(backend);
    assert!(!wal_path(&db).exists(), "the WAL outlived the store");
    let copy = tempfile::tempdir().unwrap();
    std::fs::copy(&db, copy.path().join("state.db")).unwrap();
    let reopened = SqliteBackend::open(&copy.path().join("state.db")).unwrap();
    let state = reopened.load_document("doc").await.unwrap().unwrap();
    assert_eq!(state.wal_tail.len(), 50);
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
    for i in 0..200u64 {
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
    let emptied = backend.pool.writer.emptied();
    assert!(
        emptied >= 10,
        "only {emptied} truncates ran among the writes: the test raced nothing"
    );
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

/// A compaction's scrub (ADR-037: TRUNCATE so no pre-image lingers at rest)
/// waits for a background PASSIVE rather than being refused by it: SQLite
/// refuses a second concurrent checkpoint without calling the busy handler.
/// The checkpointer runs every millisecond, so over many rounds a scrub meets
/// one. Kills a scrub that does not take the checkpoint mutex.
#[tokio::test]
async fn a_scrub_waits_for_a_background_checkpoint_instead_of_being_refused() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("state.db");
    let policy = CheckpointPolicy {
        interval: Duration::from_millis(1),
        ..PAUSED
    };
    let backend = SqliteBackend::open_with_policy(&db, 4, policy).unwrap();
    for i in 0..300 {
        let canary = format!("SCRUB-CANARY-{i:04}-must-not-survive-compaction");
        let id = backend
            .wal_append("kbc:x", canary.as_bytes(), None, &stamp())
            .await
            .unwrap();
        backend.compact("kbc:x", b"clean", id).await.unwrap();
        assert!(
            !dir_contains(dir.path(), canary.as_bytes()),
            "round {i}: the compacted update survived on disk"
        );
    }
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
