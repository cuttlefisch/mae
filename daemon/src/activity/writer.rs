//! The writer (ADR-120 D5): one task drains a bounded queue in group commits,
//! and every request awaits its own event's commit before it is answered.
//!
//! - **Group commit with no timer.** The writer takes everything waiting, up
//!   to [`MAX_BATCH_EVENTS`], into one transaction; while it commits, the next
//!   batch gathers.
//! - **Fail closed.** A full queue makes a request wait for space, and the
//!   wait for space and for the commit together are bounded by
//!   `commit_timeout_ms`. Past it, or with the writer failed, the caller gets
//!   [`Unavailable`] and must send nothing of its answer.
//! - **Recovery by itself (D13).** A failed batch is retried, and the first
//!   successful commit returns the record to healthy. A record whose file
//!   cannot be opened starts unhealthy and keeps trying to open it.
//!
//! @ai-caution: [security] [`ActivityRecord::commit`] returning `Ok` is the
//! ONLY permission to answer a recorded request. Answering after a timeout,
//! or before awaiting it, is an answered read with no record: the gap the
//! requirement forbids.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use super::coalesce::Coalescer;
use super::event::{Event, EventKind, OutcomeResult};
use super::session::SessionKey;
use super::store::{ActivityDb, Entry, Tip};
use super::ActivitySettings;

/// D5: a batch is everything waiting, up to this many events.
pub const MAX_BATCH_EVENTS: usize = 512;
/// The refusal kind a request gets when its event could not be committed.
pub const UNAVAILABLE_KIND: &str = "activity_record_unavailable";
/// D13's stable log messages.
pub const LOG_UNAVAILABLE: &str = "activity_record_unavailable";
pub const LOG_RECOVERED: &str = "activity_record_recovered";

/// The record could not take an event in time: refuse the request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unavailable;

enum Msg {
    Append {
        entry: Box<Entry>,
        events: usize,
        ack: Option<oneshot::Sender<bool>>,
    },
    Stop(oneshot::Sender<()>),
}

/// What `activity status` reports (D13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub healthy: bool,
    pub unavailable_total: u64,
    pub queue_depth: usize,
    pub last_commit_age_ms: Option<u64>,
    pub tips: Option<(Tip, Tip)>,
}

/// Throughput figures for the benchmark.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub batches: u64,
    pub events: u64,
    pub bytes: u64,
    pub max_batch: u64,
}

#[derive(Default)]
struct Shared {
    healthy: AtomicBool,
    unavailable_total: AtomicU64,
    queue_depth: AtomicUsize,
    pre_auth_in_flight: AtomicUsize,
    last_commit_us: AtomicI64,
    batches: AtomicU64,
    events: AtomicU64,
    bytes: AtomicU64,
    max_batch: AtomicU64,
    tips: Mutex<Option<(Tip, Tip)>>,
}

impl Shared {
    /// D13: the transition to unhealthy logs once, at ERROR.
    fn fail(&self, why: &str) {
        if self.healthy.swap(false, Ordering::SeqCst) {
            tracing::error!(reason = %why, "{LOG_UNAVAILABLE}");
        }
    }

    /// D13: the first successful commit after a failure logs once, at INFO.
    fn recover(&self) {
        if !self.healthy.swap(true, Ordering::SeqCst) {
            tracing::info!("{LOG_RECOVERED}");
        }
    }
}

/// The running record: the queue's sending end, health, and what the
/// entry points need to build their events.
pub struct ActivityRecord {
    tx: mpsc::Sender<Msg>,
    shared: Arc<Shared>,
    settings: ActivitySettings,
    session_key: SessionKey,
    coalescer: Coalescer,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for ActivityRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActivityRecord").finish_non_exhaustive()
    }
}

impl ActivityRecord {
    /// Start the writer on the record at `path`. A file that cannot be opened
    /// does not stop the daemon: the record starts unhealthy, every recorded
    /// request is refused, and the writer keeps trying to open it (D13).
    pub fn start(path: &Path, settings: ActivitySettings, session_key: SessionKey) -> Arc<Self> {
        let shared = Arc::new(Shared::default());
        let db = match ActivityDb::open(path) {
            Ok(db) => {
                shared.healthy.store(true, Ordering::SeqCst);
                *shared.tips.lock().unwrap_or_else(|e| e.into_inner()) = db.tips().ok();
                Some(db)
            }
            Err(e) => {
                tracing::error!(reason = %e, "{LOG_UNAVAILABLE}");
                None
            }
        };
        let (tx, rx) = mpsc::channel(settings.queue_capacity.max(1));
        let writer = {
            let shared = Arc::clone(&shared);
            let path = path.to_path_buf();
            std::thread::Builder::new()
                .name("activity-writer".into())
                .spawn(move || run_writer(rx, db, path, shared))
                .expect("spawning the activity writer thread")
        };
        Arc::new(ActivityRecord {
            tx,
            coalescer: Coalescer::new(&settings),
            shared,
            settings,
            session_key,
            writer: Mutex::new(Some(writer)),
        })
    }

    pub fn settings(&self) -> &ActivitySettings {
        &self.settings
    }

    pub fn session_key(&self) -> &SessionKey {
        &self.session_key
    }

    pub fn coalescer(&self) -> &Coalescer {
        &self.coalescer
    }

    /// D13: `true` while the record takes events.
    pub fn healthy(&self) -> bool {
        self.shared.healthy.load(Ordering::SeqCst)
    }

    /// How many refusals made before authentication are waiting on the queue
    /// now (D6 caps them at half of it).
    pub fn pre_auth_in_flight(&self) -> usize {
        self.shared.pre_auth_in_flight.load(Ordering::SeqCst)
    }

    /// Submit `entry` and wait until it has committed, within
    /// `commit_timeout_ms` for the space and the commit together.
    pub async fn commit(&self, entry: Entry) -> Result<(), Unavailable> {
        self.commit_as(entry, false).await
    }

    /// [`commit`](Self::commit) for a refusal made before authentication,
    /// counted against D6's half of the queue while it waits.
    pub async fn commit_as(&self, entry: Entry, pre_auth: bool) -> Result<(), Unavailable> {
        let (ack, done) = oneshot::channel();
        let events = entry_events(&entry);
        let limit = Duration::from_millis(self.settings.commit_timeout_ms);
        if pre_auth {
            self.shared
                .pre_auth_in_flight
                .fetch_add(1, Ordering::SeqCst);
        }
        let result = tokio::time::timeout(limit, async {
            let msg = Msg::Append {
                entry: Box::new(entry),
                events,
                ack: Some(ack),
            };
            self.send(msg, events).await?;
            match done.await {
                Ok(true) => Ok(()),
                _ => Err(Unavailable),
            }
        })
        .await;
        if pre_auth {
            self.shared
                .pre_auth_in_flight
                .fetch_sub(1, Ordering::SeqCst);
        }
        match result {
            Ok(Ok(())) => Ok(()),
            _ => {
                self.shared.unavailable_total.fetch_add(1, Ordering::SeqCst);
                self.shared
                    .fail("an event was not committed within commit_timeout_ms");
                Err(Unavailable)
            }
        }
    }

    /// Queue `entry` without waiting for its commit: a coalesced window's
    /// count, whose refusals were already answered (D6). Waits for space.
    pub async fn submit(&self, entry: Entry) -> Result<(), Unavailable> {
        let events = entry_events(&entry);
        let msg = Msg::Append {
            entry: Box::new(entry),
            events,
            ack: None,
        };
        self.send(msg, events).await
    }

    /// Queue `msg`, counted in the depth before the writer can take it out.
    async fn send(&self, msg: Msg, events: usize) -> Result<(), Unavailable> {
        self.shared.queue_depth.fetch_add(events, Ordering::SeqCst);
        // A cancelled wait (the caller's timeout) must not leave the count up.
        let guard = DepthGuard(&self.shared, events);
        let sent = self.tx.send(msg).await.map_err(|_| Unavailable);
        if sent.is_ok() {
            std::mem::forget(guard);
        }
        sent
    }

    /// The counters D13 names.
    pub fn status(&self) -> Status {
        let last = self.shared.last_commit_us.load(Ordering::SeqCst);
        Status {
            healthy: self.healthy(),
            unavailable_total: self.shared.unavailable_total.load(Ordering::SeqCst),
            queue_depth: self.shared.queue_depth.load(Ordering::SeqCst),
            last_commit_age_ms: (last > 0).then(|| ((super::now_us() - last).max(0) / 1000) as u64),
            tips: self
                .shared
                .tips
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    pub fn stats(&self) -> Stats {
        let g = |a: &AtomicU64| a.load(Ordering::SeqCst);
        Stats {
            batches: g(&self.shared.batches),
            events: g(&self.shared.events),
            bytes: g(&self.shared.bytes),
            max_batch: g(&self.shared.max_batch),
        }
    }

    /// Flush every coalesced window, commit `daemon_stop` (D10), and stop the
    /// writer. Called once, at clean shutdown.
    pub async fn shutdown(&self, stop: Event) {
        for entry in self.coalescer.drain_all(super::now_us()) {
            let _ = self.submit(entry).await;
        }
        // Queued, not awaited with the request timeout: an abandoned wait
        // would store the daemon's own stop as a refusal. The writer takes
        // messages in order, so its acknowledgement of `Stop` covers it.
        let queued = self.submit(Entry::of(stop)).await.is_ok();
        let (tx, rx) = oneshot::channel();
        let stopped = self.tx.send(Msg::Stop(tx)).await.is_ok()
            && matches!(
                tokio::time::timeout(Duration::from_secs(5), rx).await,
                Ok(Ok(()))
            );
        if !(queued && stopped && self.healthy()) {
            tracing::error!("daemon_stop may not have been recorded");
        }
        let handle = self.writer.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(h) = handle {
            let _ = tokio::task::spawn_blocking(move || h.join()).await;
        }
    }
}

/// Takes a queued count back out unless the message was sent.
struct DepthGuard<'a>(&'a Shared, usize);

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.0.queue_depth.fetch_sub(self.1, Ordering::SeqCst);
    }
}

fn entry_events(entry: &Entry) -> usize {
    entry.events.len()
        + usize::from(entry.sign_in.is_some())
        + usize::from(entry.exchange.is_some())
        + usize::from(entry.session_start.is_some())
}

/// One pending submission inside the writer.
struct Pending {
    entry: Entry,
    events: usize,
    ack: Option<oneshot::Sender<bool>>,
}

fn run_writer(
    mut rx: mpsc::Receiver<Msg>,
    mut db: Option<ActivityDb>,
    path: PathBuf,
    shared: Arc<Shared>,
) {
    let mut stop: Option<oneshot::Sender<()>> = None;
    while stop.is_none() {
        let Some(first) = rx.blocking_recv() else {
            break;
        };
        let mut batch = Vec::new();
        let mut events = 0;
        let mut next = Some(first);
        while let Some(msg) = next.take() {
            match msg {
                Msg::Append {
                    entry,
                    events: n,
                    ack,
                } => {
                    events += n;
                    batch.push(Pending {
                        entry: *entry,
                        events: n,
                        ack,
                    });
                }
                Msg::Stop(s) => stop = Some(s),
            }
            if stop.is_none() && events < MAX_BATCH_EVENTS {
                next = rx.try_recv().ok();
            }
        }
        commit_batch(&mut db, &path, &shared, batch, stop.is_some());
    }
    if let Some(s) = stop {
        let _ = s.send(());
    }
}

/// Commit `batch`, retrying until it commits, with a growing pause. At stop,
/// a batch that still fails after a few tries is given up and its waiters
/// are told so.
fn commit_batch(
    db: &mut Option<ActivityDb>,
    path: &Path,
    shared: &Shared,
    mut batch: Vec<Pending>,
    stopping: bool,
) {
    let mut pause = Duration::from_millis(50);
    let mut tries = 0;
    loop {
        tries += 1;
        let result = match db {
            // Abandonment is checked once the lock is held: a waiter that gave
            // up while the writer waited for it is stored as refused.
            Some(db) => db
                .append_with(|| {
                    mark_abandoned(&mut batch);
                    batch.iter().map(|p| p.entry.clone()).collect()
                })
                .map_err(|e| e.to_string()),
            None => match ActivityDb::open(path) {
                Ok(opened) => {
                    *db = Some(opened);
                    continue;
                }
                Err(e) => Err(e.to_string()),
            },
        };
        match result {
            Ok(done) => {
                shared
                    .last_commit_us
                    .store(super::now_us(), Ordering::SeqCst);
                shared.batches.fetch_add(1, Ordering::SeqCst);
                shared.events.fetch_add(done.events, Ordering::SeqCst);
                shared.bytes.fetch_add(done.bytes, Ordering::SeqCst);
                shared.max_batch.fetch_max(done.events, Ordering::SeqCst);
                if let Some(db) = db.as_ref() {
                    *shared.tips.lock().unwrap_or_else(|e| e.into_inner()) = db.tips().ok();
                }
                shared.recover();
                let n: usize = batch.iter().map(|p| p.events).sum();
                shared.queue_depth.fetch_sub(n, Ordering::SeqCst);
                for p in batch {
                    if let Some(ack) = p.ack {
                        let _ = ack.send(true);
                    }
                }
                return;
            }
            Err(e) => {
                shared.fail(&e);
                if stopping && tries >= 3 {
                    let n: usize = batch.iter().map(|p| p.events).sum();
                    shared.queue_depth.fetch_sub(n, Ordering::SeqCst);
                    for p in batch {
                        if let Some(ack) = p.ack {
                            let _ = ack.send(false);
                        }
                    }
                    return;
                }
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_secs(1));
            }
        }
    }
}

/// A waiter that gave up has already answered its caller with the record's
/// refusal: its event is committed as that refusal, never as the success it
/// was built as, so the record states what the caller received. Checked once
/// the write lock is held; a waiter that gives up during the insert and the
/// fsync themselves is the one window left, and its event says `ok`.
fn mark_abandoned(batch: &mut [Pending]) {
    for p in batch.iter_mut() {
        let gave_up = p.ack.as_ref().is_some_and(|a| a.is_closed());
        if gave_up {
            for ev in &mut p.entry.events {
                ev.result = OutcomeResult::Refused;
                ev.outcome_kind = Some(UNAVAILABLE_KIND.to_string());
            }
        }
    }
}

/// `daemon_start` (D10): the version, the configuration digest, the
/// retention, and the tips as they stood.
pub fn daemon_start_event(
    settings: &ActivitySettings,
    version: &str,
    tips: Option<&(Tip, Tip)>,
) -> Event {
    let mut ev = Event::daemon(EventKind::DaemonStart, super::now_us());
    let tip =
        |t: Option<&Tip>| t.map(|t| serde_json::json!({"seq": t.seq, "entry_hash": t.entry_hash}));
    ev.detail = serde_json::json!({
        "version": version,
        "config_digest": settings.digest(),
        "retention": settings.retention(),
        "tips": {
            "long": tip(tips.map(|t| &t.0)),
            "short": tip(tips.map(|t| &t.1)),
        },
    });
    ev
}
