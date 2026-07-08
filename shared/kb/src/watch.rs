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

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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

/// Recursive watcher for a directory of org files. Keeps the
/// `RecommendedWatcher` alive for the lifetime of the struct and tracks
/// path→id mappings so removals can be reported by id.
pub struct OrgDirWatcher {
    // The watcher must stay alive to keep receiving events. It owns an
    // internal thread; dropping this field tears the thread down.
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<Event>>,
    path_to_ids: Arc<Mutex<HashMap<PathBuf, Vec<String>>>>,
    /// Cumulative count of watcher errors (channel recv errors).
    errors: Arc<AtomicU64>,
}

impl OrgDirWatcher {
    /// Start watching `dir` recursively. The caller is expected to have
    /// already called `kb.ingest_org_dir(dir)` so the id map is warm —
    /// but the watcher will also populate it lazily on events.
    pub fn new(dir: impl AsRef<Path>) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        watcher.watch(dir.as_ref(), RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
            path_to_ids: Arc::new(Mutex::new(HashMap::new())),
            errors: Arc::new(AtomicU64::new(0)),
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

    /// Cumulative count of watcher errors since creation.
    pub fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// Drain all pending events and return coalesced `OrgChange`s.
    /// Non-blocking: returns an empty vec if nothing has happened.
    pub fn drain(&self) -> Vec<OrgChange> {
        let mut changes: Vec<OrgChange> = Vec::new();
        let mut seen_upsert: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
        while let Ok(ev) = self.rx.try_recv() {
            let Ok(ev) = ev else {
                self.errors.fetch_add(1, Ordering::Relaxed);
                continue;
            };
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
pub struct StoreWatcher {
    // Owns the watcher thread; dropping tears it down.
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<notify::Result<Event>>,
    errors: Arc<AtomicU64>,
}

impl StoreWatcher {
    /// Start watching the store `file` (non-recursive). The file must exist.
    pub fn new(file: impl AsRef<Path>) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })?;
        watcher.watch(file.as_ref(), RecursiveMode::NonRecursive)?;
        Ok(Self {
            _watcher: watcher,
            rx,
            errors: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Cumulative watcher errors since creation.
    pub fn error_count(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// Drain all pending events; return true if the store changed (create/modify/
    /// remove). Non-blocking. Always consumes the queued events so a caller that
    /// chooses NOT to act (e.g. within its own-write cooldown) doesn't reprocess them.
    pub fn drain_changed(&self) -> bool {
        let mut changed = false;
        while let Ok(res) = self.rx.try_recv() {
            match res {
                Ok(ev) => {
                    if matches!(
                        ev.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        changed = true;
                    }
                }
                Err(_) => {
                    self.errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        changed
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const SAMPLE: &str = ":PROPERTIES:\n:ID: abc-123\n:END:\n#+title: Test\nbody [[id:xyz]]\n";

    fn wait_for<F: Fn() -> bool>(cond: F) -> bool {
        // notify is eventually-consistent on most platforms. Poll briefly.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if cond() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn store_watcher_detects_external_modification() {
        // The basis of cross-instance freshness: another process modifying the shared
        // store file must be observable via drain_changed().
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("primary.cozo");
        std::fs::write(&path, b"v1").unwrap();
        let w = StoreWatcher::new(&path).unwrap();

        // Simulate another process committing to the store.
        std::fs::write(&path, b"v2-committed-by-another-process").unwrap();
        assert!(
            wait_for(|| w.drain_changed()),
            "store watcher must detect an external modification of the store file"
        );
    }

    #[test]
    fn watcher_reports_upsert_on_file_create() {
        let tmp = TempDir::new().unwrap();
        let w = OrgDirWatcher::new(tmp.path()).unwrap();

        let path = tmp.path().join("a.org");
        std::fs::write(&path, SAMPLE).unwrap();
        // The watcher emits normalized (canonical) paths so they match across
        // the /var → /private/var symlink on macOS; compare against canonical.
        let expected = path.canonicalize().unwrap();

        let got = wait_for(|| {
            w.drain()
                .iter()
                .any(|c| matches!(c, OrgChange::Upserted(p) if p == &expected))
        });
        assert!(got, "did not observe upsert for newly-created file");
    }

    #[test]
    fn watcher_ignores_non_org_files() {
        let tmp = TempDir::new().unwrap();
        let w = OrgDirWatcher::new(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("notes.txt"), "ignore me").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let changes = w.drain();
        assert!(
            changes
                .iter()
                .all(|c| !matches!(c, OrgChange::Upserted(p) if p.extension().and_then(|e| e.to_str()) != Some("org"))),
            "non-org change leaked through: {changes:?}"
        );
    }

    #[test]
    fn watcher_reports_removed_with_ids_from_seed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.org");
        std::fs::write(&path, SAMPLE).unwrap();
        let w = OrgDirWatcher::new(tmp.path()).unwrap();
        w.seed([(path.clone(), vec!["abc-123".to_string()])]);
        std::fs::remove_file(&path).unwrap();
        let got = wait_for(|| {
            w.drain().iter().any(
                |c| matches!(c, OrgChange::Removed(ids) if ids.contains(&"abc-123".to_string())),
            )
        });
        assert!(got, "did not observe Removed event with seeded id");
    }
}
