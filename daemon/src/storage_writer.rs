//! One in-process writer for `state.db`, and checkpoints off the commit path
//! (ADR-074 Amendment 1, open question 2: the two prerequisites of the change
//! index, decided 2026-11-15; evidence in
//! `docs/research/074a-write-cost-measurement.md`).
//!
//! **One writer.** SQLite has one write lock per database file, so the pool's
//! shard connections always wrote one at a time. What this changes is how a
//! collision between them is resolved. SQLite's busy handler resolves it by
//! sleeping (1, 2, 5, 10 … ms, or whole seconds in a build without
//! `HAVE_USLEEP`), so a longer transaction loses disproportionately: with 32
//! writers, throughput fell 34% and p99 reached 392 ms. Every write now takes
//! [`WriterLock`] before its shard connection, and tokio's `Mutex` is FIFO, so
//! writers queue instead of sleeping. Reads take only their shard: in WAL mode
//! a reader never waits for the writer.
//!
//! The lock is in-process only. Another process that opens the same file (an
//! operator command, `doctor`) is still excluded by SQLite's own file lock and
//! `busy_timeout`, exactly as before.
//!
//! **Checkpoints off the commit path.** SQLite's default checkpoints on the
//! commit that takes the WAL past 1000 pages, so every ~160th write paid a
//! checkpoint's fsyncs; that was the whole of the measured single-writer p99.
//! Here a background thread with its own connection checkpoints instead:
//! - every [`CheckpointPolicy::interval`] in which a write committed, PASSIVE:
//!   it never waits for, and is never waited on by, a writer or reader;
//! - after [`CheckpointPolicy::idle_truncate_after`] with no write, TRUNCATE,
//!   which empties the WAL file. It takes the writer lock (`try_lock`, never
//!   queueing behind writers) so a writer that arrives meanwhile queues rather
//!   than busy-sleeps, and its connection has `busy_timeout=0`, so it never
//!   holds the lock waiting for a reader: it gives up and retries after
//!   another idle period.
//!
//! The commit path keeps SQLite's own checkpoint only as a backstop, at
//! [`CheckpointPolicy::backstop_pages`]: a commit checkpoints only when the
//! background checkpointer has fallen that far behind.
//!
//! **The bound on WAL growth.** The WAL file never exceeds `backstop_pages`
//! plus one transaction's pages (each frame is a page plus 24 bytes): the
//! backstop checkpoints on the commit that crosses it, under the writer slot,
//! so it backfills everything, and SQLite restarts a fully checkpointed WAL
//! from its start. Below saturation, a PASSIVE completes in a gap between
//! writes and the next write restarts the WAL, so the file stays near one
//! interval's worth of commits; it returns to zero bytes once idle. At
//! saturation (writes back to back) a PASSIVE never quite catches up, the file
//! grows to the backstop, and one commit in about `backstop_pages` worth pays
//! for copying the tail the background checkpointer had not reached. The one
//! exception no checkpoint can help: a read transaction held open for long (a
//! `VACUUM INTO` backup in progress) pins the WAL until it ends.
//!
//! **Durability is unchanged.** `synchronous=NORMAL` as before: a commit is
//! written to the WAL, not fsynced, so it survives a process crash, and it is
//! fsynced by the next checkpoint. That checkpoint now comes within about one
//! interval of the write, where before it came only every ~1000 pages.
//!
//! @ai-caution: [concurrency] Lock order, outermost first. Never take an
//! earlier lock while holding a later one:
//! 1. `DocStore`'s replace gate, then its per-document lock (the backend's
//!    callers);
//! 2. [`WriterLock`], by every write, and by the checkpointer's TRUNCATE only;
//! 3. the checkpoint mutex (`Shared::checkpointing`), by every checkpoint
//!    this process starts;
//! 4. a shard connection's mutex (`SqlitePool::lock_shard`/`lock_primary`);
//! 5. SQLite's own file locks, contended only by other processes.
//!
//! The checkpointer never holds a shard mutex: it has its own connection. A
//! write that skips [`WriterLock`] is not caught by any type: it collides with
//! the queued writers at step 5 and sleeps in the busy handler again.

use std::ops::Deref;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, MutexGuard};
use std::time::{Duration, Instant};

use tracing::warn;

use super::StorageError;

/// When the checkpointer runs, and the bound the commit path enforces.
#[derive(Clone, Copy, Debug)]
pub struct CheckpointPolicy {
    /// How often the checkpointer wakes. A wake with no new commit costs one
    /// atomic load. 1 s, measured (`docs/research/074a-write-cost-measurement.md`):
    /// at the hosted rate a checkpoint's fsyncs slow the commits that overlap
    /// them, and a 100 ms cadence took p50 from 3.1 ms to 11.6 ms against 1 s.
    /// At that rate (about 1,000 WAL pages a second) 1 s is also how often
    /// SQLite's own 1000-page checkpoint ran before.
    pub interval: Duration,
    /// With no commit for this long, TRUNCATE the WAL to zero bytes.
    pub idle_truncate_after: Duration,
    /// `wal_autocheckpoint` on the shard connections: the commit path
    /// checkpoints only when the WAL holds this many pages. 16384 pages (64 MiB
    /// at 4 KiB pages) is more than a second of writing at the measured
    /// saturation (about 2,000 commits/s at 6–8 pages each), so the background
    /// pass normally comes first.
    pub backstop_pages: u32,
}

impl CheckpointPolicy {
    pub const PRODUCTION: CheckpointPolicy = CheckpointPolicy {
        interval: Duration::from_secs(1),
        idle_truncate_after: Duration::from_secs(1),
        backstop_pages: 16384,
    };
}

#[derive(Default)]
struct Shared {
    /// FIFO: tokio's `Mutex` hands the lock to waiters in arrival order.
    queue: tokio::sync::Mutex<()>,
    /// Writes finished (committed or not); the checkpointer's "anything new?".
    writes: AtomicU64,
    /// The `writes` count read before the last PASSIVE that backfilled the
    /// whole WAL: every write up to it is in the database file.
    covered: AtomicU64,
    /// TRUNCATE checkpoints that emptied the WAL file.
    emptied: AtomicU64,
    /// Held by every checkpoint this process starts itself (the checkpointer's,
    /// and the TRUNCATE that scrubs a purge or compaction from the WAL): SQLite
    /// refuses a second concurrent checkpoint at once, without calling the busy
    /// handler, so the scrub would otherwise be skipped whenever it met a
    /// background PASSIVE. Taken after the writer slot, never before it.
    checkpointing: std::sync::Mutex<()>,
}

/// The store's one writer slot. Cheap to clone: clones share the slot.
#[derive(Clone, Default)]
pub struct WriterLock(Arc<Shared>);

impl WriterLock {
    /// Wait in line for the writer slot, THEN lock the shard connection (lock
    /// order steps 2 and 3). The slot is held until the returned guard drops.
    pub async fn write<'a>(
        &'a self,
        shard: impl FnOnce() -> MutexGuard<'a, sqlite::Connection>,
    ) -> WriteConn<'a> {
        let queued = self.0.queue.lock().await;
        WriteConn {
            conn: shard(),
            _queued: queued,
            shared: &self.0,
        }
    }

    /// The checkpoint mutex (see `Shared::checkpointing`), poison-recovering.
    pub fn checkpointing(&self) -> MutexGuard<'_, ()> {
        self.0
            .checkpointing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Writes finished so far, committed or not.
    pub fn writes(&self) -> u64 {
        self.0.writes.load(Ordering::Acquire)
    }

    /// The write count the last complete PASSIVE covered; tests wait on it.
    #[cfg(test)]
    pub fn covered(&self) -> u64 {
        self.0.covered.load(Ordering::Acquire)
    }

    /// TRUNCATE checkpoints that emptied the WAL so far.
    #[cfg(test)]
    pub fn emptied(&self) -> u64 {
        self.0.emptied.load(Ordering::Acquire)
    }
}

/// A shard connection held as the store's writer. Derefs to the connection.
pub struct WriteConn<'a> {
    // Declared first, so dropped first: the shard is free before the next
    // writer is admitted.
    conn: MutexGuard<'a, sqlite::Connection>,
    _queued: tokio::sync::MutexGuard<'a, ()>,
    shared: &'a Shared,
}

impl WriteConn<'_> {
    /// TRUNCATE-checkpoint the WAL, so a purged or superseded page's pre-image
    /// does not linger in it at rest (ADR-037). Best-effort, as before: a
    /// reader still holding the WAL after `busy_timeout` leaves it to the
    /// checkpointer's next idle TRUNCATE. Holds the checkpoint mutex, so it
    /// waits for a background PASSIVE instead of being refused by it.
    pub fn truncate_wal(&self) {
        let _checkpointing = self.shared.checkpointing.lock();
        let _ = self.conn.execute("PRAGMA wal_checkpoint(TRUNCATE);");
    }
}

impl Deref for WriteConn<'_> {
    type Target = sqlite::Connection;
    fn deref(&self) -> &sqlite::Connection {
        &self.conn
    }
}

impl Drop for WriteConn<'_> {
    fn drop(&mut self) {
        self.shared.writes.fetch_add(1, Ordering::Release);
    }
}

/// The background checkpointer: one thread, one connection, stopped and
/// joined on drop.
pub struct Checkpointer {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Checkpointer {
    /// Start checkpointing `path` (already open in WAL mode) under `policy`.
    pub fn spawn(
        path: &Path,
        policy: CheckpointPolicy,
        writer: WriterLock,
    ) -> Result<Checkpointer, StorageError> {
        let conn = sqlite::Connection::open(path)?;
        // `busy_timeout=0`: a checkpoint that meets a reader gives up at once
        // and is retried; it never sleeps holding the writer slot.
        conn.execute(
            "PRAGMA synchronous=NORMAL;
             PRAGMA busy_timeout=0;
             PRAGMA wal_autocheckpoint=0;",
        )?;
        let (stop, stopped) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("state-db-checkpoint".into())
            .spawn(move || run(&conn, policy, &writer, &stopped))
            .map_err(|e| StorageError::Io(format!("checkpointer thread: {e}")))?;
        Ok(Checkpointer {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for Checkpointer {
    fn drop(&mut self) {
        drop(self.stop.take()); // disconnects: the thread's next wait returns
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The checkpointer's loop: one decision per tick, until stopped.
fn run(
    conn: &sqlite::Connection,
    policy: CheckpointPolicy,
    writer: &WriterLock,
    stopped: &mpsc::Receiver<()>,
) {
    // `None` forces a first pass: a store reopened after a crash may hold a WAL.
    let mut seen = None;
    let mut quiet_since = Instant::now();
    let mut emptied = false;
    while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(policy.interval) {
        let writes = writer.writes();
        if seen != Some(writes) {
            (seen, quiet_since, emptied) = (Some(writes), Instant::now(), false);
            if checkpoint(conn, "PASSIVE", &writer.checkpointing()) {
                writer.0.covered.store(writes, Ordering::Release);
            }
        } else if !emptied && quiet_since.elapsed() >= policy.idle_truncate_after {
            emptied = truncate(conn, writer);
            if emptied {
                writer.0.emptied.fetch_add(1, Ordering::Release);
            } else {
                quiet_since = Instant::now(); // a reader held it: wait another idle period
            }
        }
    }
}

/// TRUNCATE the WAL holding the writer slot, if it is free; whether it emptied.
fn truncate(conn: &sqlite::Connection, writer: &WriterLock) -> bool {
    match writer.0.queue.try_lock() {
        Ok(_slot) => checkpoint(conn, "TRUNCATE", &writer.checkpointing()),
        Err(_) => false,
    }
}

/// Run `PRAGMA wal_checkpoint(mode)`; whether it completed: not blocked
/// (`busy` 0) and every frame of the WAL backfilled (`log` = `checkpointed`;
/// both 0 after a TRUNCATE emptied the file). The caller holds the checkpoint
/// mutex, which `_held` stands for.
fn checkpoint(conn: &sqlite::Connection, mode: &str, _held: &MutexGuard<'_, ()>) -> bool {
    let result = conn
        .prepare(format!("PRAGMA wal_checkpoint({mode})"))
        .and_then(|mut stmt| {
            stmt.next()?;
            Ok((
                stmt.read::<i64, _>(0)?,
                stmt.read::<i64, _>(1)?,
                stmt.read::<i64, _>(2)?,
            ))
        });
    match result {
        Ok((busy, log, checkpointed)) => busy == 0 && log == checkpointed,
        Err(e) => {
            warn!(error = %e, mode, "state.db checkpoint failed; retried on the next tick");
            false
        }
    }
}
