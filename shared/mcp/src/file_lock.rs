//! Advisory file locking for multi-editor file contention.
//!
//! When MAE opens a file for editing, it creates a `.mae.lock` file alongside
//! it containing the PID, hostname, and timestamp. This prevents MAE-MAE
//! conflicts when multiple instances edit the same file.
//!
//! Other editors (VS Code, etc.) won't see `.mae.lock` — those conflicts are
//! handled by the content-hash verification layer in `crates/core/src/buffer.rs`.
//!
//! Also provides [`with_locked_update`], a reload-fresh-then-mutate-then-save
//! helper for shared global state files (e.g. `kb-registry.toml`,
//! `projects.toml`) that multiple concurrently-running `mae` processes may
//! write to. Lives here (rather than in `mae-core`) so lower-level shared
//! crates like `mae-kb` — which `mae-core` depends on but which must NOT
//! depend back on `mae-core` — can reuse the same primitive instead of
//! duplicating it.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Information stored in a `.mae.lock` file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub pid: u32,
    pub hostname: String,
    pub timestamp: u64,
}

impl LockInfo {
    /// Create lock info for the current process.
    pub fn current() -> Self {
        let hostname = hostname::get()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "unknown".to_string());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        LockInfo {
            pid: std::process::id(),
            hostname,
            timestamp,
        }
    }
}

/// Compute the lock file path for a given file.
pub fn lock_path(file_path: &Path) -> PathBuf {
    let parent = file_path.parent().unwrap_or(Path::new("."));
    let name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    parent.join(format!(".{}.mae.lock", name))
}

/// Acquire an advisory lock for the given file.
/// Returns `Ok(())` if the lock was acquired, or `Err` with info about
/// the existing lock holder.
pub fn acquire_lock(file_path: &Path) -> Result<(), LockInfo> {
    let lpath = lock_path(file_path);

    // Check for existing lock.
    if let Some(existing) = read_lock(&lpath) {
        // Check if the owning process is still alive.
        if is_process_alive(existing.pid) {
            return Err(existing);
        }
        // Stale lock — remove it.
        let _ = std::fs::remove_file(&lpath);
    }

    // Write our lock.
    let info = LockInfo::current();
    if let Ok(json) = serde_json::to_string_pretty(&info) {
        let _ = std::fs::write(&lpath, json);
    }
    Ok(())
}

/// Release the advisory lock for the given file.
/// Only removes the lock if it belongs to us (same PID).
pub fn release_lock(file_path: &Path) {
    let lpath = lock_path(file_path);
    if let Some(info) = read_lock(&lpath) {
        if info.pid == std::process::id() {
            let _ = std::fs::remove_file(&lpath);
        }
    }
}

/// Check if another MAE instance holds a lock on this file.
/// Returns `Some(LockInfo)` if locked by a live process, `None` otherwise.
pub fn check_lock(file_path: &Path) -> Option<LockInfo> {
    let lpath = lock_path(file_path);
    let info = read_lock(&lpath)?;
    if info.pid == std::process::id() {
        return None; // Our own lock
    }
    if is_process_alive(info.pid) {
        Some(info)
    } else {
        // Stale lock — clean up.
        let _ = std::fs::remove_file(&lpath);
        None
    }
}

/// Read and parse a lock file, returning `None` if missing or unparseable.
fn read_lock(path: &Path) -> Option<LockInfo> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Check if a process with the given PID is alive.
fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // kill(pid, 0) checks if the process exists without sending a signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        // On non-Unix, assume alive (conservative).
        let _ = pid;
        true
    }
}

/// RAII guard: acquires on construction, releases on `Drop`. Prevents a
/// leaked lock on early-return/error/panic-unwind paths — the free-function
/// `acquire_lock`/`release_lock` pair above requires manual pairing and is
/// kept only for callers that already handle that themselves (e.g. MAE's
/// per-buffer file lock).
pub struct LockGuard {
    path: PathBuf,
    acquired: bool,
}

impl LockGuard {
    /// Try to acquire once; `acquired()` reflects whether the lock is ours.
    pub fn try_acquire(path: &Path) -> Self {
        let acquired = acquire_lock(path).is_ok();
        LockGuard {
            path: path.to_path_buf(),
            acquired,
        }
    }

    /// Bounded retry: `attempts` short-backoff tries, then give up and return
    /// a non-acquired guard. Contention on these files is expected to be a
    /// few milliseconds (another `mae` process reloading+saving the same
    /// small TOML file), so a handful of short sleeps comfortably outlasts
    /// the realistic window without introducing user-visible latency.
    pub fn try_acquire_with_retry(path: &Path, attempts: u32, backoff: Duration) -> Self {
        for attempt in 0..attempts {
            let guard = Self::try_acquire(path);
            if guard.acquired || attempt + 1 == attempts {
                return guard;
            }
            std::thread::sleep(backoff);
        }
        // Unreachable given attempts >= 1, but keep the function total.
        Self::try_acquire(path)
    }

    pub fn acquired(&self) -> bool {
        self.acquired
    }
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.acquired {
            release_lock(&self.path);
        }
    }
}

/// Reload-fresh → mutate → persist, under a best-effort advisory lock on
/// `lock_target` (typically the same path being saved).
///
/// This is the fix for a whole class of bug: several MAE state files
/// (`kb-registry.toml`, `projects.toml`, ...) used to be loaded once into an
/// in-memory struct at process startup and blindly overwritten on save. Since
/// users routinely run multiple concurrent `mae` processes (one per project
/// directory), a process holding a stale in-memory copy would silently
/// clobber another process's concurrent additions. `with_locked_update`
/// always reloads the freshest on-disk state immediately before applying the
/// caller's mutation, so a save reflects "current disk state + my change"
/// rather than "my possibly-stale snapshot + my change".
///
/// If the advisory lock can't be acquired after a short bounded retry (the
/// realistic contention window here is milliseconds — another process
/// reloading+saving the same small file), this proceeds anyway with a
/// warning rather than failing the caller's operation outright: these are
/// command-triggered metadata saves, not interactive buffer edits, and the
/// reload-before-mutate step already closes most of the race even without
/// the lock — the lock only tightens the last few-millisecond gap.
///
/// Returns the mutated (freshest-plus-my-change) value, whatever `mutate`
/// returned, and the outcome of `save` as a separate `Result` — a `save`
/// failure (e.g. disk full, permissions) does NOT discard the in-memory
/// mutation, matching the existing best-effort persistence semantics these
/// callers relied on before this helper existed (mutate the in-memory state
/// unconditionally; treat the disk write as best-effort). Callers should
/// still log/surface a `saved` error rather than silently dropping it.
pub fn with_locked_update<T, R>(
    lock_target: &Path,
    load: impl FnOnce() -> T,
    mutate: impl FnOnce(&mut T) -> R,
    save: impl FnOnce(&T) -> io::Result<()>,
) -> (T, R, io::Result<()>) {
    let guard = LockGuard::try_acquire_with_retry(lock_target, 3, Duration::from_millis(15));
    if !guard.acquired() {
        tracing::warn!(
            path = %lock_target.display(),
            "proceeding without advisory lock (contended by another mae process)"
        );
    }
    let mut value = load();
    let result = mutate(&mut value);
    let saved = save(&value);
    (value, result, saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn lock_path_format() {
        let p = lock_path(Path::new("/home/user/src/main.rs"));
        assert_eq!(p, PathBuf::from("/home/user/src/.main.rs.mae.lock"));
    }

    #[test]
    fn acquire_and_release() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        assert!(acquire_lock(&file).is_ok());
        assert!(lock_path(&file).exists());

        release_lock(&file);
        assert!(!lock_path(&file).exists());
    }

    #[test]
    fn own_lock_not_reported_as_conflict() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        acquire_lock(&file).unwrap();
        assert!(check_lock(&file).is_none()); // Our own lock
        release_lock(&file);
    }

    #[test]
    fn stale_lock_is_cleaned() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();

        // Write a lock with a fake dead PID.
        let fake_lock = LockInfo {
            pid: 999_999_999, // Almost certainly not a real PID
            hostname: "test".to_string(),
            timestamp: 0,
        };
        let lpath = lock_path(&file);
        std::fs::write(&lpath, serde_json::to_string(&fake_lock).unwrap()).unwrap();

        // Should detect the stale lock and allow acquisition.
        assert!(acquire_lock(&file).is_ok());
        release_lock(&file);
    }

    #[test]
    fn lock_contention_different_pid() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();
        // Use our parent PID — guaranteed to be a live process we can signal.
        let parent_pid = unsafe { libc::getppid() } as u32;
        let fake_lock = LockInfo {
            pid: parent_pid,
            hostname: "other-host".to_string(),
            timestamp: 0,
        };
        let lpath = lock_path(&file);
        std::fs::write(&lpath, serde_json::to_string(&fake_lock).unwrap()).unwrap();
        // Should fail to acquire (parent PID is alive and not our PID)
        let result = acquire_lock(&file);
        assert!(result.is_err());
        let info = result.unwrap_err();
        assert_eq!(info.pid, parent_pid);
        // Clean up
        let _ = std::fs::remove_file(&lpath);
    }

    #[test]
    fn lock_release_only_own() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();
        // Use parent PID — guaranteed alive and not our PID
        let parent_pid = unsafe { libc::getppid() } as u32;
        let fake_lock = LockInfo {
            pid: parent_pid,
            hostname: "other".to_string(),
            timestamp: 0,
        };
        let lpath = lock_path(&file);
        std::fs::write(&lpath, serde_json::to_string(&fake_lock).unwrap()).unwrap();
        // release_lock should NOT remove it (not our PID)
        release_lock(&file);
        assert!(lpath.exists(), "Lock file should persist (not our PID)");
        // Clean up
        let _ = std::fs::remove_file(&lpath);
    }

    #[test]
    fn lock_survives_concurrent_check() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();
        acquire_lock(&file).unwrap();
        // Multiple threads call check_lock simultaneously
        let handles: Vec<_> = (0..10)
            .map(|_| {
                let f = file.clone();
                std::thread::spawn(move || check_lock(&f))
            })
            .collect();
        for h in handles {
            let result = h.join().unwrap();
            assert!(result.is_none(), "Our own lock should not be reported");
        }
        release_lock(&file);
    }

    #[test]
    fn lock_path_special_chars() {
        let p = lock_path(Path::new("/home/user/my project/hello world.rs"));
        assert_eq!(
            p,
            PathBuf::from("/home/user/my project/.hello world.rs.mae.lock")
        );
        // Unicode
        let p2 = lock_path(Path::new("/home/user/src/日本語.rs"));
        assert_eq!(p2, PathBuf::from("/home/user/src/.日本語.rs.mae.lock"));
    }

    #[test]
    fn lock_guard_releases_on_drop() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();
        {
            let guard = LockGuard::try_acquire(&file);
            assert!(guard.acquired());
            assert!(lock_path(&file).exists());
        }
        assert!(
            !lock_path(&file).exists(),
            "lock must be released when the guard drops"
        );
    }

    #[test]
    fn lock_guard_retry_gives_up_on_live_contention() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("test.txt");
        std::fs::write(&file, "hello").unwrap();
        let parent_pid = unsafe { libc::getppid() } as u32;
        let fake_lock = LockInfo {
            pid: parent_pid,
            hostname: "other".to_string(),
            timestamp: 0,
        };
        std::fs::write(lock_path(&file), serde_json::to_string(&fake_lock).unwrap()).unwrap();

        let guard = LockGuard::try_acquire_with_retry(&file, 3, Duration::from_millis(1));
        assert!(!guard.acquired(), "contended lock must not be acquired");
        // Dropping a non-acquired guard must not remove the other holder's lock.
        drop(guard);
        assert!(lock_path(&file).exists());
        let _ = std::fs::remove_file(lock_path(&file));
    }

    #[test]
    fn with_locked_update_persists_mutation() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("state.txt");
        std::fs::write(&path, "0").unwrap();

        let (final_value, doubled, saved) = with_locked_update(
            &path,
            || {
                std::fs::read_to_string(&path)
                    .unwrap()
                    .parse::<i32>()
                    .unwrap()
            },
            |v| {
                *v += 1;
                *v * 2
            },
            |v| std::fs::write(&path, v.to_string()),
        );
        saved.unwrap();

        assert_eq!(final_value, 1);
        assert_eq!(doubled, 2);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "1");
    }

    /// The literal regression scenario for the kb-registry.toml incident:
    /// two independently-loaded "processes" mutate the same backing file;
    /// the second writer's save must not clobber the first writer's change.
    #[test]
    fn with_locked_update_two_writers_both_survive() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("set.txt");
        std::fs::write(&path, "").unwrap();

        fn load(path: &Path) -> std::collections::BTreeSet<String> {
            std::fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty())
                .collect()
        }
        fn save(path: &Path, set: &std::collections::BTreeSet<String>) -> io::Result<()> {
            std::fs::write(path, set.iter().cloned().collect::<Vec<_>>().join("\n"))
        }

        // Both "processes" load their own stale snapshot before either writes.
        let _stale_a = load(&path);
        let _stale_b = load(&path);

        let (_, _, saved_a) = with_locked_update(
            &path,
            || load(&path),
            |set| {
                set.insert("A".to_string());
            },
            |set| save(&path, set),
        );
        saved_a.unwrap();

        // "Process B" saves via the same locked-update path — even though its
        // own in-memory view never saw A's write, the reload-before-mutate
        // inside with_locked_update picks it up.
        let (_, _, saved_b) = with_locked_update(
            &path,
            || load(&path),
            |set| {
                set.insert("B".to_string());
            },
            |set| save(&path, set),
        );
        saved_b.unwrap();

        let final_on_disk = load(&path);
        assert!(
            final_on_disk.contains("A"),
            "A's write must survive B's save"
        );
        assert!(final_on_disk.contains("B"));
        assert_eq!(final_on_disk.len(), 2);
    }
}
