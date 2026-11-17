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
//! **Head-of-line.** There is one slot for the whole store, not one per KB or
//! per shard, and it is strictly FIFO: a write waits for every write queued
//! before it, whatever document or KB each touches. So the slot's latency is
//! the sum of the transactions ahead of it, and the long holders set the tail:
//! - a compaction (snapshot plus WAL trim, every `compact_threshold` updates
//!   of one document; its cost grows with the document, up to the 10 MB
//!   `max_document_size_bytes`) and a document delete;
//! - a seed insert (one transaction for a KB's whole node list);
//! - the checkpointer's TRUNCATE, idle or scrub (below): a backfill of what
//!   the WAL still holds, with its fsyncs.
//!
//! A `wal_append` holds it for one small transaction (47 µs p50, 159 µs p99,
//! one writer). Compaction and delete hold it for their transaction only; they
//! no longer run the ADR-037 scrub themselves (until 2026-11-17 they ran a
//! TRUNCATE before releasing the slot, which could hold it for the whole
//! 5 s `busy_timeout` behind a reader).
//!
//! The lock is in-process only. Another process that opens the same file (an
//! operator command) is still excluded by SQLite's own file lock and
//! `busy_timeout`, exactly as before; `doctor` opens it read-only and writes
//! nothing (`inspect_store`).
//!
//! **Checkpoints off the commit path.** SQLite's default checkpoints on the
//! commit that takes the WAL past 1000 pages, so every ~160th write paid a
//! checkpoint's fsyncs; that was the whole of the measured single-writer p99.
//! Here a background thread with its own connection checkpoints instead, once
//! per [`CheckpointPolicy::interval`] tick:
//! - while a scrub is pending (below), TRUNCATE, waiting in line for the
//!   writer slot;
//! - otherwise, if a write committed since the last tick, PASSIVE: it never
//!   waits for, and is never waited on by, a writer or reader;
//! - after [`CheckpointPolicy::idle_truncate_after`] with no write, TRUNCATE,
//!   which empties the WAL file. It takes the writer slot only if it is free
//!   (`try_lock`, never queueing behind writers).
//!
//! Its connection has `busy_timeout=0`, so a TRUNCATE that meets a reader
//! gives up at once rather than holding the slot while it waits, and is
//! retried on a later tick.
//!
//! **The ADR-037 scrub.** A deleted or compacted-away document's plaintext
//! must not linger at rest. `secure_delete=ON` zeroes the freed pages in the
//! database file, but the pre-images stay in the WAL file until it is
//! truncated: PASSIVE backfills frames, it never zeroes or truncates them. So
//! a compaction or delete marks a scrub pending ([`WriteConn::mark_scrub`]),
//! and the checkpointer TRUNCATEs on every tick until one succeeds. It waits
//! for the slot (FIFO) rather than trying it, so sustained writes cannot
//! starve it the way they starve the idle TRUNCATE; and with `busy_timeout=0`
//! a reader pinning the WAL (a backup's `VACUUM INTO`, a read on another
//! shard) costs one immediate BUSY per tick, not a slot held for seconds. A
//! failed attempt is logged at debug; a scrub still pending after
//! [`CheckpointPolicy::scrub_warn_after`] logs a warning. The mark is in
//! memory only, which is enough: a clean close checkpoints and deletes the
//! WAL file, and every open marks a scrub pending, so a crash's leftover WAL
//! is truncated on the next start.
//!
//! **What bounds the WAL, and what does not.** The commit path keeps SQLite's
//! own checkpoint only as a backstop, at [`CheckpointPolicy::backstop_pages`]:
//! a commit checkpoints only when the background checkpointer has fallen that
//! far behind. With no reader holding an old snapshot, the WAL stays under
//! the backstop plus one transaction: the backstop backfills everything and
//! SQLite restarts a fully checkpointed WAL from its start. Below saturation
//! a PASSIVE completes between writes, so the file stays near one interval's
//! worth of commits, and it returns to zero bytes once idle.
//!
//! **That bound does not hold against readers.** No checkpoint, PASSIVE or
//! backstop, can backfill past the oldest snapshot a reader still holds, and
//! the WAL cannot restart while any reader uses it. So a read transaction held
//! open (a `VACUUM INTO` backup) pins it, and reads that overlap one another
//! on several shards can starve it the same way: each tick finds some reader
//! in it. The WAL then grows past the backstop without limit. That is made
//! visible rather than prevented: past twice the backstop the checkpointer
//! logs a warning (at most once per [`CheckpointPolicy::warn_every`]), and
//! the WAL size and any pending scrub are reported by `daemon/status` and
//! `mae-daemon doctor` ([`WriterLock::health`], `inspect_store`).
//!
//! **Durability is unchanged.** `synchronous=NORMAL` as before: a commit is
//! written to the WAL, not fsynced, so it survives a process crash, and it is
//! fsynced by the next checkpoint. That checkpoint now comes within about one
//! interval of the write, where before it came only every ~1000 pages.
//!
//! **A panic in a tick is caught**, logged at error, and the next tick runs:
//! a checkpointer that died silently would leave the WAL to the backstop and
//! every scrub pending forever.
//!
//! @ai-caution: [concurrency] Lock order, outermost first. Never take an
//! earlier lock while holding a later one:
//! 1. `DocStore`'s replace gate, then its per-document lock (the backend's
//!    callers);
//! 2. [`WriterLock`], by every write, and by the checkpointer's TRUNCATE
//!    (idle or scrub) only;
//! 3. the pending-scrub mark (`Shared::scrub`), held only to read or set it;
//! 4. a shard connection's mutex (`SqlitePool::lock_shard`/`lock_primary`);
//! 5. SQLite's own file locks, contended only by other processes.
//!
//! The checkpointer never holds a shard mutex: it has its own connection, and
//! it is the only code in the process that checkpoints, so no two checkpoints
//! of ours meet (SQLite would refuse the second without calling the busy
//! handler). A commit's backstop checkpoint can still meet a PASSIVE; one of
//! them is refused and the next tick or commit retries. A write that skips
//! [`WriterLock`] is not caught by any type: it collides with the queued
//! writers at step 5 and sleeps in the busy handler again.

use std::ops::Deref;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tracing::{debug, error, warn};

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
    /// A scrub still pending this long logs a warning. 30 s is 30 attempts at
    /// the production interval, each of which needs only an instant with no
    /// reader in the WAL. The daemon's own reads are single statements of
    /// milliseconds, so they cannot overlap 30 attempts in a row; what can is
    /// a read transaction held open (a backup's `VACUUM INTO`, a stuck
    /// connection), which is what the warning is for. It also bounds how long
    /// purged plaintext can sit in the WAL unremarked.
    pub scrub_warn_after: Duration,
    /// The least time between two warnings of one kind (WAL past the
    /// backstop, scrub still pending), so a condition that persists through
    /// a long backup logs every five minutes rather than every second.
    pub warn_every: Duration,
}

impl CheckpointPolicy {
    pub const PRODUCTION: CheckpointPolicy = CheckpointPolicy {
        interval: Duration::from_secs(1),
        idle_truncate_after: Duration::from_secs(1),
        backstop_pages: 16384,
        scrub_warn_after: Duration::from_secs(30),
        warn_every: Duration::from_secs(300),
    };

    /// WAL frames past which the checkpointer warns: twice the backstop.
    /// Without a reader the WAL stays under the backstop plus one transaction,
    /// and no transaction the daemon writes comes near a whole backstop (the
    /// largest is a compaction, whose snapshot `max_document_size_bytes`
    /// bounds at 10 MB by default), so a WAL this long means readers are
    /// keeping checkpoints from backfilling it.
    pub fn wal_warn_frames(&self) -> u64 {
        2 * u64::from(self.backstop_pages)
    }
}

/// A scrub the checkpointer still owes (see the module doc).
struct Scrub {
    since: Instant,
    /// Compactions and deletes since the last successful scrub (0 for the one
    /// every open schedules). A count, never names: a document's name can
    /// carry a node id, and this is logged.
    docs: u64,
    warned_at: Option<Instant>,
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
    /// The pending scrub, if any. Set only by a writer holding the slot, and
    /// cleared only by a TRUNCATE holding it, so a scrub can never clear a
    /// mark set after the TRUNCATE it reports on.
    scrub: Mutex<Option<Scrub>>,
    /// WAL frames the last checkpoint found (`log`), for `health`.
    wal_frames: AtomicU64,
    #[cfg(test)]
    test: TestHooks,
}

/// Counters and fault injection the tests read and set.
#[cfg(test)]
#[derive(Default)]
struct TestHooks {
    /// TRUNCATEs the checkpointer ran while the writer slot was free: always
    /// 0, since it holds the slot itself.
    unslotted_truncates: AtomicU64,
    /// Scrub attempts, failed or not.
    scrub_attempts: AtomicU64,
    /// Warnings logged: a scrub still pending, the WAL past the backstop.
    scrub_warnings: AtomicU64,
    wal_warnings: AtomicU64,
    /// Ticks to panic at the start of, and ticks that panicked.
    panic_next: AtomicU64,
    tick_panics: AtomicU64,
}

#[cfg(test)]
thread_local! {
    /// Checkpointers this thread started; `doctor`'s inspection must start none.
    static SPAWNED_HERE: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// How a running store's WAL stands, for `daemon/status` (and from there
/// `doctor`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageHealth {
    /// The WAL file's length now.
    pub wal_bytes: u64,
    /// Frames in the WAL when the checkpointer last looked.
    pub wal_frames: u64,
    /// The frame count past which the checkpointer warns.
    pub wal_warn_frames: u64,
    /// A scrub still owed: documents it covers, and seconds pending.
    pub scrub_pending: Option<(u64, u64)>,
}

/// The store's one writer slot. Cheap to clone: clones share the slot.
#[derive(Clone, Default)]
pub struct WriterLock(Arc<Shared>);

impl WriterLock {
    /// Wait in line for the writer slot, THEN lock the shard connection (lock
    /// order steps 2 and 5). The slot is held until the returned guard drops.
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

    /// Writes finished so far, committed or not.
    pub fn writes(&self) -> u64 {
        self.0.writes.load(Ordering::Acquire)
    }

    /// Whether a scrub is still owed.
    pub fn scrub_pending(&self) -> bool {
        relock(&self.0.scrub).is_some()
    }

    /// How the WAL stands: the file `wal` now, the frames as of the
    /// checkpointer's last tick.
    pub fn health(&self, policy: &CheckpointPolicy, wal: &Path) -> StorageHealth {
        StorageHealth {
            wal_bytes: std::fs::metadata(wal).map_or(0, |m| m.len()),
            wal_frames: self.0.wal_frames.load(Ordering::Acquire),
            wal_warn_frames: policy.wal_warn_frames(),
            scrub_pending: relock(&self.0.scrub)
                .as_ref()
                .map(|s| (s.docs, s.since.elapsed().as_secs())),
        }
    }

    /// Owe a scrub, covering `docs` more documents.
    fn mark_scrub(&self, docs: u64) {
        let mut scrub = relock(&self.0.scrub);
        match scrub.as_mut() {
            Some(s) => s.docs += docs,
            None => {
                *scrub = Some(Scrub {
                    since: Instant::now(),
                    docs,
                    warned_at: None,
                })
            }
        }
    }

    /// See `Shared::test`.
    #[cfg(test)]
    pub fn unslotted_truncates(&self) -> u64 {
        self.0.test.unslotted_truncates.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn scrub_attempts(&self) -> u64 {
        self.0.test.scrub_attempts.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn scrub_warnings(&self) -> u64 {
        self.0.test.scrub_warnings.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn wal_warnings(&self) -> u64 {
        self.0.test.wal_warnings.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub fn tick_panics(&self) -> u64 {
        self.0.test.tick_panics.load(Ordering::Acquire)
    }

    /// Make the checkpointer's next `n` ticks panic.
    #[cfg(test)]
    pub fn panic_next_ticks(&self, n: u64) {
        self.0.test.panic_next.store(n, Ordering::Release);
    }

    /// Checkpointers the calling thread has started.
    #[cfg(test)]
    pub fn spawned_on_this_thread() -> u64 {
        SPAWNED_HERE.with(|c| c.get())
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

/// Lock a std mutex, recovering it if a panic poisoned it: a tick that
/// panicked holding one must not take the store down with it.
fn relock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A shard connection held as the store's writer. Derefs to the connection.
pub struct WriteConn<'a> {
    // Declared first, so dropped first: the shard is free before the next
    // writer is admitted.
    conn: MutexGuard<'a, sqlite::Connection>,
    _queued: tokio::sync::MutexGuard<'a, ()>,
    shared: &'a Arc<Shared>,
}

impl WriteConn<'_> {
    /// Owe the ADR-037 scrub for what this write removed: the checkpointer
    /// TRUNCATEs the WAL on its next ticks until it succeeds (module doc).
    /// Called after the commit and before the slot is released, so the
    /// TRUNCATE that clears the mark always runs after this commit.
    pub fn mark_scrub(&self) {
        WriterLock(Arc::clone(self.shared)).mark_scrub(1);
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
        // A store opened after a crash may hold a WAL with a purged document's
        // pre-image that no in-memory mark remembers.
        writer.mark_scrub(0);
        #[cfg(test)]
        SPAWNED_HERE.with(|c| c.set(c.get() + 1));
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
            // A tick's panic is caught in `run`; this one could only come from
            // outside a tick, and is logged rather than lost.
            if thread.join().is_err() {
                error!("state.db checkpointer thread panicked outside a tick");
            }
        }
    }
}

/// What the checkpointer carries from one tick to the next.
struct Ticks {
    /// `None` forces a first pass: a store reopened after a crash may hold a WAL.
    seen: Option<u64>,
    quiet_since: Instant,
    emptied: bool,
    wal_warned_at: Option<Instant>,
    failed_warned_at: Option<Instant>,
}

/// The checkpointer's loop: one tick per interval, until stopped. A panic in
/// a tick is logged and the loop goes on (module doc).
fn run(
    conn: &sqlite::Connection,
    policy: CheckpointPolicy,
    writer: &WriterLock,
    stopped: &mpsc::Receiver<()>,
) {
    let mut ticks = Ticks {
        seen: None,
        quiet_since: Instant::now(),
        emptied: false,
        wal_warned_at: None,
        failed_warned_at: None,
    };
    while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(policy.interval) {
        let ticked =
            std::panic::catch_unwind(AssertUnwindSafe(|| tick(conn, &policy, writer, &mut ticks)));
        if let Err(panic) = ticked {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            error!(panic = %what, "state.db checkpointer tick panicked; continuing");
            #[cfg(test)]
            writer.0.test.tick_panics.fetch_add(1, Ordering::Release);
        }
    }
}

/// One decision: scrub, PASSIVE, idle TRUNCATE, or nothing.
fn tick(conn: &sqlite::Connection, policy: &CheckpointPolicy, writer: &WriterLock, t: &mut Ticks) {
    #[cfg(test)]
    {
        let hooks = &writer.0.test;
        if hooks
            .panic_next
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
            .is_ok()
        {
            panic!("injected checkpointer panic");
        }
    }
    let writes = writer.writes();
    if writer.scrub_pending() {
        if scrub(conn, policy, writer, t) {
            let covered = writer.0.covered.load(Ordering::Acquire);
            (t.seen, t.quiet_since, t.emptied) = (Some(covered), Instant::now(), true);
        }
        return;
    }
    if t.seen != Some(writes) {
        (t.seen, t.quiet_since, t.emptied) = (Some(writes), Instant::now(), false);
        if checkpoint(conn, "PASSIVE", policy, writer, t).is_some_and(|c| c.complete()) {
            writer.0.covered.store(writes, Ordering::Release);
        }
    } else if !t.emptied && t.quiet_since.elapsed() >= policy.idle_truncate_after {
        t.emptied = truncate(conn, policy, writer, t);
        if !t.emptied {
            t.quiet_since = Instant::now(); // a reader held it: wait another idle period
        }
    }
}

/// The scrub: TRUNCATE holding the writer slot, waiting in line for it.
/// Whether the WAL emptied, which clears the mark.
fn scrub(
    conn: &sqlite::Connection,
    policy: &CheckpointPolicy,
    writer: &WriterLock,
    t: &mut Ticks,
) -> bool {
    // FIFO behind the writers already queued, ahead of any that come later:
    // under sustained writes the slot is never free for `try_lock`.
    let _slot = writer.0.queue.blocking_lock();
    #[cfg(test)]
    writer.0.test.scrub_attempts.fetch_add(1, Ordering::Release);
    let done = checkpoint(conn, "TRUNCATE", policy, writer, t).is_some_and(|c| c.emptied());
    let mut pending = relock(&writer.0.scrub);
    if done {
        *pending = None;
        // Exact: a write counts itself before it releases the slot, and this
        // TRUNCATE, holding the slot, put every one so far in the database file.
        writer.0.covered.store(writer.writes(), Ordering::Release);
        writer.0.emptied.fetch_add(1, Ordering::Release);
        return true;
    }
    let Some(s) = pending.as_mut() else {
        return false;
    };
    debug!(
        docs = s.docs,
        "state.db WAL scrub blocked (a reader is in the WAL); retried next tick"
    );
    let overdue = s.since.elapsed() >= policy.scrub_warn_after;
    if overdue
        && s.warned_at
            .is_none_or(|at| at.elapsed() >= policy.warn_every)
    {
        warn!(
            docs = s.docs,
            pending_secs = s.since.elapsed().as_secs(),
            "state.db WAL scrub (ADR-037) still pending: a reader has held the WAL through \
             every attempt, so removed content stays in the WAL file until it lets go"
        );
        s.warned_at = Some(Instant::now());
        #[cfg(test)]
        writer.0.test.scrub_warnings.fetch_add(1, Ordering::Release);
    }
    false
}

/// TRUNCATE the WAL holding the writer slot, if it is free; whether it emptied.
fn truncate(
    conn: &sqlite::Connection,
    policy: &CheckpointPolicy,
    writer: &WriterLock,
    t: &mut Ticks,
) -> bool {
    match writer.0.queue.try_lock() {
        Ok(_slot) => {
            let emptied =
                checkpoint(conn, "TRUNCATE", policy, writer, t).is_some_and(|c| c.emptied());
            if emptied {
                writer.0.emptied.fetch_add(1, Ordering::Release);
            }
            emptied
        }
        Err(_) => false,
    }
}

/// `PRAGMA wal_checkpoint`'s three columns.
struct Checkpointed {
    busy: i64,
    log: i64,
    done: i64,
}

impl Checkpointed {
    /// Not blocked, and every frame of the WAL backfilled.
    fn complete(&self) -> bool {
        self.busy == 0 && self.log == self.done
    }

    /// A TRUNCATE that left the WAL file empty (both counts 0).
    fn emptied(&self) -> bool {
        self.complete() && self.log == 0
    }
}

/// Run `PRAGMA wal_checkpoint(mode)`; record the WAL's length and warn if it
/// is past twice the backstop. `None` on an error.
fn checkpoint(
    conn: &sqlite::Connection,
    mode: &str,
    policy: &CheckpointPolicy,
    writer: &WriterLock,
    t: &mut Ticks,
) -> Option<Checkpointed> {
    #[cfg(test)]
    if mode == "TRUNCATE" && writer.0.queue.try_lock().is_ok() {
        writer
            .0
            .test
            .unslotted_truncates
            .fetch_add(1, Ordering::Release);
    }
    let result = conn
        .prepare(format!("PRAGMA wal_checkpoint({mode})"))
        .and_then(|mut stmt| {
            stmt.next()?;
            Ok(Checkpointed {
                busy: stmt.read::<i64, _>(0)?,
                log: stmt.read::<i64, _>(1)?,
                done: stmt.read::<i64, _>(2)?,
            })
        });
    match result {
        Ok(c) => {
            // -1 when SQLite could not read the WAL header at all.
            if let Ok(frames) = u64::try_from(c.log) {
                writer.0.wal_frames.store(frames, Ordering::Release);
                watch_wal(frames, policy, writer, t);
            }
            Some(c)
        }
        Err(e) => {
            // Retried every tick, so a persistent error is warned at most once per
            // `warn_every` and debug-logged otherwise.
            if t.failed_warned_at
                .is_none_or(|at| at.elapsed() >= policy.warn_every)
            {
                warn!(error = %e, mode, "state.db checkpoint failed; retried on the next tick");
                t.failed_warned_at = Some(Instant::now());
            } else {
                debug!(error = %e, mode, "state.db checkpoint failed again");
            }
            None
        }
    }
}

/// Warn, rate-limited, when the WAL is past twice the backstop: readers are
/// keeping every checkpoint from backfilling it (module doc).
#[cfg_attr(not(test), allow(unused_variables))]
fn watch_wal(frames: u64, policy: &CheckpointPolicy, writer: &WriterLock, t: &mut Ticks) {
    if frames <= policy.wal_warn_frames()
        || t.wal_warned_at
            .is_some_and(|at| at.elapsed() < policy.warn_every)
    {
        return;
    }
    warn!(
        wal_frames = frames,
        backstop_frames = policy.backstop_pages,
        "state.db WAL is past twice its checkpoint backstop: a read transaction held open \
         (a backup's VACUUM INTO?) or overlapping reads are keeping checkpoints from \
         backfilling it"
    );
    t.wal_warned_at = Some(Instant::now());
    #[cfg(test)]
    writer.0.test.wal_warnings.fetch_add(1, Ordering::Release);
}

/// What `doctor` reports about a store, read without writing it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreInspection {
    /// No `state.db` yet: the daemon creates it on first start.
    Absent,
    Present {
        /// The WAL file's length (a high-water mark: a restarted WAL reuses
        /// the file from its start without shrinking it).
        wal_bytes: u64,
        /// The length past which the checkpointer warns
        /// ([`CheckpointPolicy::wal_warn_frames`] frames).
        wal_warn_bytes: u64,
    },
}

impl StoreInspection {
    /// Whether the WAL file is past the warning length.
    pub fn wal_over_backstop(&self) -> bool {
        matches!(self, Self::Present { wal_bytes, wal_warn_bytes } if wal_bytes > wal_warn_bytes)
    }
}

/// Inspect the store at `path` for `doctor`, beside a daemon that may be
/// running on it: a read-only connection, no checkpointer, no schema change,
/// and no database created when the file is absent. A read-only connection
/// never checkpoints, on close or otherwise, so the database and WAL contents
/// are left as they were. One exception: with the daemon stopped and no
/// `-wal`/`-shm` present, SQLite may create those (empty) sidecar files to read
/// a WAL-mode database, and a read-only connection can't remove them on close.
/// They hold no data, and the daemon's next open adopts them.
pub fn inspect_store(
    path: &Path,
    policy: &CheckpointPolicy,
) -> Result<StoreInspection, StorageError> {
    if !path.exists() {
        return Ok(StoreInspection::Absent);
    }
    let conn =
        sqlite::Connection::open_with_flags(path, sqlite::OpenFlags::new().with_read_only())?;
    let scalar = |sql: &str| -> Result<i64, StorageError> {
        let mut stmt = conn.prepare(sql)?;
        stmt.next()?;
        Ok(stmt.read::<i64, _>(0)?)
    };
    // Reads the schema, so a file that is not a database fails here.
    scalar("SELECT COUNT(*) FROM sqlite_master")?;
    let page = u64::try_from(scalar("PRAGMA page_size")?).unwrap_or(4096);
    let wal_bytes = std::fs::metadata(wal_path(path)).map_or(0, |m| m.len());
    Ok(StoreInspection::Present {
        wal_bytes,
        wal_warn_bytes: 32 + policy.wal_warn_frames() * (page + 24),
    })
}

/// The WAL file beside `db`.
pub fn wal_path(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push("-wal");
    PathBuf::from(p)
}
