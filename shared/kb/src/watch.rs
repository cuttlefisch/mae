//! File watcher for org directories.
//!
//! Wraps the `notify` crate in a channel-based API so the editor's
//! main loop can drain events without owning a background thread or
//! dealing with Send/Sync concerns. Typical use:
//!
//! ```no_run
//! use mae_kb::{KnowledgeBase, watch::OrgDirWatcher};
//!
//! let mut kb = KnowledgeBase::new();
//! kb.ingest_org_dir("/tmp/notes");
//! let watcher = OrgDirWatcher::new("/tmp/notes").unwrap();
//! // Later, in the main loop tick:
//! for ev in watcher.drain() {
//!     match ev {
//!         mae_kb::watch::OrgChange::Upserted(path) => {
//!             let ids = kb.ingest_org_file(&path);
//!             watcher.record_ids(path, ids);
//!         }
//!         mae_kb::watch::OrgChange::Removed(ids) => {
//!             for id in ids { kb.remove(&id); }
//!         }
//!     }
//! }
//! drop(watcher);
//! ```
//!
//! The watcher only surfaces events for `.org` files, and coalesces
//! file-remove events using the last-known id map so callers don't
//! need to re-walk the filesystem to learn what was removed. The
//! watcher itself does not parse files — the caller's `ingest_org_file`
//! already produces the id list, so callers feed it back via
//! `record_ids` to keep the removal map warm without a double read.
//!
//! # One OS watcher per process, many watched paths
//!
//! @ai-caution: [resource-exhaustion] NEVER construct a fresh
//! `notify::recommended_watcher()` per watched directory. Every
//! `RecommendedWatcher` costs one `inotify_init` fd on Linux, and
//! `fs.inotify.max_user_instances` is **128 per user** while
//! `max_user_watches` is ~250,000 — instances are the scarce resource by
//! three orders of magnitude. A watcher-per-KB design made MAE consume 70% of
//! the machine-wide budget (89 of 128) and starve every other application; see
//! `docs/INOTIFY_INSTANCE_EXHAUSTION.md`. Both public handles in this module
//! ([`OrgDirWatcher`], [`StoreWatcher`]) are therefore thin registrations on
//! one process-wide [`SharedDirWatcher`], which spends *watches* (cheap and
//! plentiful) instead of *instances*. Emacs has always worked this way. If you
//! need a new kind of watcher, register it here too — do not add a third
//! parallel watcher implementation (CLAUDE.md #8/#15).

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

/// A coalesced change event relative to the KB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgChange {
    /// File was added or modified — caller should re-ingest it.
    Upserted(PathBuf),
    /// File was removed — caller should remove these node ids from the KB.
    /// A single org file may host multiple org-roam nodes (one per
    /// heading with an `:ID:` drawer), so removal is a list.
    Removed(Vec<String>),
}

/// Identifies one registration on the [`SharedDirWatcher`].
type RegId = u64;

/// One handle's slice of the shared watcher: the root it cares about, the
/// events routed to it since its last drain, and the errors attributed to it.
struct Registration {
    /// Normalized root this registration owns (a directory for a recursive
    /// registration, a single file for a non-recursive one).
    root: PathBuf,
    /// `root`'s component count, cached — the key used for longest-prefix
    /// routing, so a nested registration wins over its ancestor.
    depth: usize,
    /// Events routed here, awaiting this registration's handle draining them.
    /// Unbounded, exactly like the per-watcher `mpsc` channel it replaces: a
    /// handle that is never drained accumulates the same way it always did.
    queue: VecDeque<Event>,
    /// Cumulative watcher errors attributed to this registration. Per
    /// registration (not global) so a fresh handle always starts at zero.
    errors: u64,
}

/// Routing table for the one shared watcher.
#[derive(Default)]
struct Routes {
    next_id: RegId,
    regs: HashMap<RegId, Registration>,
    /// Refcount per watched root, so two KBs registering the same directory
    /// share one `watch()` and the first to drop doesn't unwatch the other.
    /// `recursive` is the effective (OR'd) mode currently applied to the root.
    roots: HashMap<PathBuf, RootState>,
}

struct RootState {
    refs: usize,
    recursive: bool,
}

/// The process-wide watcher: ONE `notify` watcher (one `inotify_init` fd on
/// Linux, one FSEvents stream on macOS) with one path per registration.
///
/// Every event is routed to the registration with the **longest** matching
/// root prefix. Longest-prefix rather than first-match is what makes a KB
/// registered *inside* another KB's directory correct: the file belongs to the
/// innermost KB that claims it. (Ties — two registrations on the same
/// directory — all receive the event, matching the old one-watcher-each
/// behavior for that case.)
struct SharedDirWatcher {
    /// Lock order: `routes` may be taken while holding nothing, and `watcher`
    /// only while already holding `routes`. `rx` is only ever taken alone,
    /// before `routes`. No path takes them in any other order, so no cycle.
    watcher: Mutex<RecommendedWatcher>,
    rx: Mutex<mpsc::Receiver<notify::Result<Event>>>,
    routes: Mutex<Routes>,
}

/// The process-wide instance, created on first use.
///
/// A `Weak`, not an `Arc`: the OS watcher lives exactly as long as at least one
/// handle holds it, so a process that watches nothing holds no instance at all
/// (strictly better than the old design's zero-when-idle, never worse) and a
/// test binary doesn't strand one for its whole run.
///
/// Creation is deliberately re-tried on failure rather than memoized as a
/// permanent error: the one failure mode that matters here (the per-user
/// instance limit being momentarily exhausted by *other* processes) is
/// transient, and a caller registering a KB minutes later should get a working
/// watcher.
static SHARED: Mutex<std::sync::Weak<SharedDirWatcher>> = Mutex::new(std::sync::Weak::new());

impl SharedDirWatcher {
    fn get() -> notify::Result<Arc<Self>> {
        let mut slot = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(existing) = slot.upgrade() {
            return Ok(existing);
        }
        let (tx, rx) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        let shared = Arc::new(Self {
            watcher: Mutex::new(watcher),
            rx: Mutex::new(rx),
            routes: Mutex::new(Routes::default()),
        });
        *slot = Arc::downgrade(&shared);
        Ok(shared)
    }

    /// Add `path` to the shared watcher and return the new registration's id.
    /// Errors (missing path, exhausted OS limits) surface exactly as they did
    /// when each handle owned its own watcher.
    fn register(&self, path: &Path, recursive: bool) -> notify::Result<RegId> {
        let root = normalize_path(path);
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let effective_recursive = recursive
            || routes
                .roots
                .get(&root)
                .map(|s| s.recursive)
                .unwrap_or(false);
        let mode = if effective_recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        };
        // Always call `watch()`, even for an already-watched root: `notify`
        // treats a repeat watch of the same path as an update, and calling it
        // unconditionally is what preserves the "path must exist" error for
        // the second registrant.
        self.watcher
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .watch(&root, mode)?;
        let entry = routes.roots.entry(root.clone()).or_insert(RootState {
            refs: 0,
            recursive: effective_recursive,
        });
        entry.refs += 1;
        entry.recursive = effective_recursive;
        let id = routes.next_id;
        routes.next_id += 1;
        let depth = root.components().count();
        routes.regs.insert(
            id,
            Registration {
                root,
                depth,
                queue: VecDeque::new(),
                errors: 0,
            },
        );
        Ok(id)
    }

    /// Drop a registration; unwatch its root once no registration wants it.
    fn unregister(&self, id: RegId) {
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        let Some(reg) = routes.regs.remove(&id) else {
            return;
        };
        let drop_root = match routes.roots.get_mut(&reg.root) {
            Some(state) => {
                state.refs = state.refs.saturating_sub(1);
                state.refs == 0
            }
            None => false,
        };
        if drop_root {
            routes.roots.remove(&reg.root);
            // Best-effort: the root may already be gone from the filesystem,
            // in which case `notify` has dropped the watch itself.
            let _ = self
                .watcher
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .unwatch(&reg.root);
        }
    }

    /// Move everything the OS has delivered so far into per-registration
    /// queues. Called by every drain, so no registration depends on another
    /// handle being drained first.
    fn pump(&self) {
        let mut incoming: Vec<notify::Result<Event>> = Vec::new();
        {
            let rx = self.rx.lock().unwrap_or_else(|e| e.into_inner());
            while let Ok(res) = rx.try_recv() {
                incoming.push(res);
            }
        }
        if incoming.is_empty() {
            return;
        }
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        for res in incoming {
            match res {
                Ok(ev) => {
                    for id in Self::targets(&routes, &ev.paths) {
                        if let Some(reg) = routes.regs.get_mut(&id) {
                            reg.queue.push_back(ev.clone());
                        }
                    }
                }
                Err(err) => {
                    // A `notify` error carrying paths belongs to whoever owns
                    // those paths; a watcher-level error (no paths) affects
                    // every registration, so every registration counts it —
                    // the same number each handle would have observed back
                    // when it owned the failing watcher outright.
                    let targets = Self::targets(&routes, &err.paths);
                    if targets.is_empty() {
                        for reg in routes.regs.values_mut() {
                            reg.errors += 1;
                        }
                    } else {
                        for id in targets {
                            if let Some(reg) = routes.regs.get_mut(&id) {
                                reg.errors += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    /// Registrations owning `paths`: for each path, the registration(s) whose
    /// root is its longest matching prefix. Resolved per path independently,
    /// then unioned — a rename event carries two paths that can legitimately
    /// belong to two different KBs.
    ///
    /// O(registrations) per path, with registrations bounded by the number of
    /// KBs a process has open (single digits) — the same order of work the
    /// per-watcher design spent inside the kernel instead.
    fn targets(routes: &Routes, paths: &[PathBuf]) -> Vec<RegId> {
        let mut out: Vec<RegId> = Vec::new();
        for p in paths {
            let np = normalize_path(p);
            let mut best_depth: Option<usize> = None;
            let mut best: Vec<RegId> = Vec::new();
            for (id, reg) in routes.regs.iter() {
                if !is_prefix_of(&reg.root, &np) {
                    continue;
                }
                match best_depth {
                    Some(d) if reg.depth < d => continue,
                    Some(d) if reg.depth == d => best.push(*id),
                    _ => {
                        best.clear();
                        best.push(*id);
                        best_depth = Some(reg.depth);
                    }
                }
            }
            for id in best {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// Pump, then hand this registration everything routed to it.
    fn take_events(&self, id: RegId) -> Vec<Event> {
        self.pump();
        let mut routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        match routes.regs.get_mut(&id) {
            Some(reg) => reg.queue.drain(..).collect(),
            None => Vec::new(),
        }
    }

    fn error_count(&self, id: RegId) -> u64 {
        let routes = self.routes.lock().unwrap_or_else(|e| e.into_inner());
        routes.regs.get(&id).map(|r| r.errors).unwrap_or(0)
    }
}

/// Component-wise prefix test — `/a/bc` is NOT a prefix of `/a/bcd`, which a
/// naive string comparison would get wrong and silently misroute.
fn is_prefix_of(root: &Path, path: &Path) -> bool {
    let mut r = root.components();
    let mut p = path.components();
    loop {
        match (r.next(), p.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(a), Some(b)) if a != b => return false,
            _ => {}
        }
    }
}

/// Recursive watcher for a directory of org files. A registration on the
/// process-wide [`SharedDirWatcher`] (NOT its own OS watcher — see this
/// module's `@ai-caution`), plus the path→id mappings that let removals be
/// reported by id.
pub struct OrgDirWatcher {
    core: Arc<SharedDirWatcher>,
    id: RegId,
    path_to_ids: Mutex<HashMap<PathBuf, Vec<String>>>,
}

impl Drop for OrgDirWatcher {
    fn drop(&mut self) {
        self.core.unregister(self.id);
    }
}

impl OrgDirWatcher {
    /// Start watching `dir` recursively. The caller is expected to have
    /// already called `kb.ingest_org_dir(dir)` so the id map is warm —
    /// but the watcher will also populate it lazily on events.
    pub fn new(dir: impl AsRef<Path>) -> notify::Result<Self> {
        let core = SharedDirWatcher::get()?;
        let id = core.register(dir.as_ref(), true)?;
        Ok(Self {
            core,
            id,
            path_to_ids: Mutex::new(HashMap::new()),
        })
    }

    /// Pre-seed the path→ids map from an existing KB walk. If the caller
    /// ingested a directory and knows the mapping, calling this avoids
    /// a cold-start race where a removal event fires before the watcher
    /// has seen the initial create.
    pub fn seed(&self, mappings: impl IntoIterator<Item = (PathBuf, Vec<String>)>) {
        let mut map = self.path_to_ids.lock().unwrap();
        for (p, ids) in mappings {
            map.insert(normalize_path(&p), ids);
        }
    }

    /// The ids this path produced as of the last `record_ids`/`seed` call —
    /// i.e. what the caller should diff a fresh re-ingest against to find
    /// ids that no longer belong to this file (e.g. an in-place `:ID:` edit)
    /// and retract them. Returns `None` if the path was never recorded.
    pub fn ids_for_path(&self, path: impl AsRef<Path>) -> Option<Vec<String>> {
        let path = normalize_path(path.as_ref());
        self.path_to_ids.lock().unwrap().get(&path).cloned()
    }

    /// Record the ids a caller ingested for a given path. This keeps the
    /// removal id map warm after `OrgChange::Upserted` events without
    /// the watcher having to re-read and re-parse the file itself —
    /// the caller's `KnowledgeBase::ingest_org_file` already returned
    /// these ids. Empty id lists still clear any stale mapping so the
    /// next removal event reports no phantom ids.
    pub fn record_ids(&self, path: impl Into<PathBuf>, ids: Vec<String>) {
        let path = normalize_path(&path.into());
        let mut map = self.path_to_ids.lock().unwrap();
        if ids.is_empty() {
            map.remove(&path);
        } else {
            map.insert(path, ids);
        }
    }

    /// Cumulative count of watcher errors since creation, attributed to this
    /// watcher's own registration.
    pub fn error_count(&self) -> u64 {
        self.core.error_count(self.id)
    }

    /// Drain all pending events and return coalesced `OrgChange`s.
    /// Non-blocking: returns an empty vec if nothing has happened.
    pub fn drain(&self) -> Vec<OrgChange> {
        let mut changes: Vec<OrgChange> = Vec::new();
        let mut seen_upsert: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        for ev in self.core.take_events(self.id) {
            match ev.kind {
                EventKind::Create(_) | EventKind::Modify(_) => {
                    for p in ev.paths {
                        if !is_org(&p) {
                            continue;
                        }
                        let p = normalize_path(&p);
                        if !seen_upsert.insert(p.clone()) {
                            continue;
                        }
                        changes.push(OrgChange::Upserted(p));
                    }
                }
                EventKind::Remove(_) => {
                    for p in ev.paths {
                        if !is_org(&p) {
                            continue;
                        }
                        let ids = self.path_to_ids.lock().unwrap().remove(&normalize_path(&p));
                        if let Some(ids) = ids {
                            if !ids.is_empty() {
                                changes.push(OrgChange::Removed(ids));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        changes
    }
}

/// Watches a single durable KB store file (e.g. `primary.cozo`) for changes made by
/// OTHER processes — the basis of daemon-less cross-instance freshness. When another
/// mae process commits to the shared sqlite store, this fires so the editor can reload
/// its in-memory mirror. `drain_changed()` coalesces all pending events into one bool.
///
/// A single-file, non-recursive registration on the same process-wide
/// [`SharedDirWatcher`] the org-directory watchers use: a store file and an
/// org directory are different *paths*, not different watchers, so folding
/// them onto one instance costs nothing and keeps the "one home for one
/// concern" boundary (CLAUDE.md #8). Their events never collide — an exact
/// file path is always a strictly longer prefix match than any directory that
/// happens to contain it.
pub struct StoreWatcher {
    core: Arc<SharedDirWatcher>,
    id: RegId,
    /// File name of the store itself, e.g. `kb.sqlite`. Events are filtered to
    /// this name and its sidecars — see [`StoreWatcher::new`] for why the
    /// registration is on the parent directory rather than the file.
    stem: std::ffi::OsString,
}

impl Drop for StoreWatcher {
    fn drop(&mut self) {
        self.core.unregister(self.id);
    }
}

impl StoreWatcher {
    /// Start watching the store `file` (non-recursive).
    ///
    /// @ai-caution: [storage] Registers the store's PARENT DIRECTORY, not the
    /// store file. That is not incidental: MAE puts its sqlite stores into WAL
    /// mode (`cozo_store::wal`), and **under WAL a commit writes to
    /// `<store>-wal`, leaving the main file untouched until a checkpoint**. A
    /// registration on the file itself therefore stops seeing writes entirely —
    /// caught by `external_store_change_arms_a_background_reload`, which went red
    /// the moment WAL was enabled.
    ///
    /// Events are filtered back down to the store and its `-wal`/`-shm`
    /// sidecars in [`drain_changed`], so a directory that also holds unrelated
    /// files does not produce spurious reloads.
    pub fn new(file: impl AsRef<Path>) -> notify::Result<Self> {
        let file = file.as_ref();
        let core = SharedDirWatcher::get()?;
        let stem = file
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_default();
        let dir = file.parent().unwrap_or(file);
        let id = core.register(dir, false)?;
        Ok(Self { core, id, stem })
    }

    /// Cumulative watcher errors since creation.
    pub fn error_count(&self) -> u64 {
        self.core.error_count(self.id)
    }

    /// Drain all pending events; return true if the store changed (create/modify/
    /// remove). Non-blocking. Always consumes the queued events so a caller that
    /// chooses NOT to act (e.g. within its own-write cooldown) doesn't reprocess them.
    pub fn drain_changed(&self) -> bool {
        let stem = self.stem.as_os_str();
        self.core.take_events(self.id).into_iter().any(|ev| {
            if !matches!(
                ev.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            ) {
                return false;
            }
            // The store itself, or one of its WAL sidecars. Compared on the file
            // NAME rather than the full path so this holds under macOS FSEvents'
            // canonicalized paths too (see `normalize_path`).
            ev.paths.iter().any(|p| {
                p.file_name().is_some_and(|n| {
                    n == stem
                        || n.to_string_lossy()
                            .starts_with(&format!("{}-", stem.to_string_lossy()))
                })
            })
        })
    }
}

fn is_org(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()) == Some("org")
}

/// Normalize a path so map keys and event paths compare equal across platforms.
///
/// macOS FSEvents reports canonical paths (e.g. `/private/var/...`) while
/// callers usually hold the symlinked form (`/var/...`, `/tmp/...`). Without
/// normalizing, a removal event's path never matches the seeded key, so the
/// removed node ids are lost and stale KB nodes linger. Canonicalize when the
/// file still exists; fall back to the original path otherwise (e.g. a removal,
/// where `canonicalize()` would fail because the file is already gone — by then
/// FSEvents has already reported the canonical form anyway).
fn normalize_path(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

/// Deadline for [`wait_for`]'s poll loop. Widened from an earlier 3s (issue
/// #494): macOS's FSEvents backend delivers change notifications in
/// OS-coalesced batches regardless of the `latency: 0.0` this module already
/// requests (not further caller-tunable), so a tight deadline is more
/// marginal on macOS under CI load than on Linux's inotify backend, without
/// there being any actual debounce/config asymmetry to fix here. 10s gives
/// well over 3x headroom over the original while keeping the worst-case
/// added wall-clock time for a `continue-on-error: true`, non-blocking CI job
/// bounded and reasonable — the loop still exits immediately on success in
/// the common case; this only affects the rare slow-CI tail.
pub const WATCHER_TEST_POLL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);

/// Poll `cond` until it returns `true` or [`WATCHER_TEST_POLL_DEADLINE`]
/// elapses — `notify`-backed watchers are eventually-consistent on every
/// platform this crate supports, so a test observing a watcher side effect
/// must poll rather than assert immediately. Test/CI-only helper (not part
/// of this crate's real runtime behavior), but a normal `pub fn` rather than
/// `#[cfg(test)]`-gated: a `cfg(test)` item is only visible within its own
/// crate's test build, never to a downstream dependent crate's tests, so
/// `mae-core`'s own watcher-related tests (`kb_ops_concurrency_tests.rs`)
/// need this to be genuinely, always compiled and exported to call it at
/// all rather than hand-rolling an independent (and, per #494, silently
/// drifting) copy of the same loop. `FnMut` (not `Fn`) so a caller's
/// condition closure can mutate captured state each poll (e.g. draining a
/// pending queue via a `&mut Editor` method).
pub fn wait_for<F: FnMut() -> bool>(mut cond: F) -> bool {
    let deadline = std::time::Instant::now() + WATCHER_TEST_POLL_DEADLINE;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    false
}

/// How many inotify **instances** (`inotify_init` file descriptors) this
/// process currently holds, or `None` on a platform where that is not
/// observable.
///
/// The scarce kernel resource behind `docs/INOTIFY_INSTANCE_EXHAUSTION.md`:
/// `fs.inotify.max_user_instances` defaults to 128 per user, while
/// `max_user_watches` is typically ~250,000 — so a watcher design must spend
/// watches, not instances. This is the empirical oracle for that invariant, so
/// the regression tests measure it instead of inferring it from the code shape.
///
/// Linux-only by nature (`/proc/self/fd` + the `anon_inode:inotify` link
/// target). Returns `None` on macOS/other, where `notify` uses FSEvents/kqueue
/// and there is no equivalent per-user instance cap to exhaust — callers
/// (tests) must skip rather than assert, never silently treat `None` as 0.
/// Exported as a normal `pub fn` for the same reason as [`wait_for`]: a
/// `cfg(test)` item is invisible to a downstream crate's tests, and `mae-core`
/// needs this to assert the same invariant end-to-end through `kb_register`.
pub fn inotify_instance_count() -> Option<usize> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let entries = std::fs::read_dir("/proc/self/fd").ok()?;
    let mut count = 0usize;
    for entry in entries.flatten() {
        // A raced-away fd (the dir listing is a snapshot) just doesn't count.
        if let Ok(target) = std::fs::read_link(entry.path()) {
            if target.to_string_lossy() == "anon_inode:inotify" {
                count += 1;
            }
        }
    }
    Some(count)
}

#[cfg(test)]
#[path = "watch_tests.rs"]
mod watch_tests;
