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
///
/// **Atomic exclusive-create, not read-then-write (ADR-058 Phase B fix).** The original
/// implementation checked "does a lock file exist" via a separate read, then wrote its own
/// lock file — a classic TOCTOU window between the check and the write that two contenders
/// arriving within microseconds of each other can both slip through, each concluding it
/// acquired the lock. A real 3-way-concurrent adversarial test
/// (`kb_init_project_converges_to_one_instance_under_a_three_way_race`) caught this directly:
/// widening the retry *budget* alone (a separate fix, see `with_locked_update`) did not help,
/// because the bug isn't about retrying long enough — it's that the acquisition itself could
/// succeed for more than one caller at once. `OpenOptions::create_new(true)` maps to
/// `O_CREAT|O_EXCL` on Unix — the OS guarantees this fails if the file already exists, with
/// no window for two callers to both "win" — closing the race at its actual root rather than
/// papering over the symptom with more retries.
pub fn acquire_lock(file_path: &Path) -> Result<(), LockInfo> {
    let lpath = lock_path(file_path);
    // Ensure the parent directory exists before the atomic create below — otherwise a
    // missing directory (e.g. the very first write on a fresh machine/container, or a
    // test that creates its data_dir lazily) surfaces as a generic I/O error that the
    // loop below treats as "proceed without a lock" (reserved for genuinely exceptional
    // errors), silently skipping real exclusion for what's actually the common,
    // first-ever-write case — exactly when multiple simultaneously-starting processes
    // racing to create that same directory need the lock most.
    if let Some(parent) = lpath.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let info = LockInfo::current();
    let json = serde_json::to_string_pretty(&info).unwrap_or_default();

    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lpath)
        {
            Ok(mut f) => {
                use std::io::Write;
                let _ = f.write_all(json.as_bytes());
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match read_lock(&lpath) {
                    Some(existing) => {
                        if is_process_alive(existing.pid) {
                            return Err(existing);
                        }
                        // Stale lock (owning process is gone) — reclaim it and retry the
                        // atomic create. Another contender could win this same race, in
                        // which case our own retry will correctly see THEIR fresh lock
                        // and back off via the `is_process_alive` branch above.
                        let _ = std::fs::remove_file(&lpath);
                    }
                    None => {
                        // The file exists but isn't readable as a valid lock yet — another
                        // caller's create_new just won and hasn't finished writing.
                        // Genuinely contended; do not busy-loop indefinitely on it.
                        return Err(LockInfo {
                            pid: 0,
                            hostname: "unknown".to_string(),
                            timestamp: 0,
                        });
                    }
                }
            }
            Err(_) => {
                // Some other I/O error (e.g. parent directory missing/permissions) —
                // best-effort fall through rather than hard-failing the caller, matching
                // this module's existing best-effort persistence semantics elsewhere.
                return Ok(());
            }
        }
    }
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

/// Check if a process with the given PID is alive. `pub` because this is the one
/// correct cross-platform liveness check in the workspace -- other call sites (e.g.
/// `mae-agent-cli`'s connection discovery) should use this rather than growing their
/// own platform-specific implementation (principle #8: no duplicated logic).
pub fn is_process_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // `pid` may not fit `pid_t` (e.g. u32::MAX) -- a bare `as` cast would wrap
        // around to a small negative value (`-1` has its own special "every process
        // I can signal" meaning to `kill(2)`, not "PID 4294967295"), so reject
        // out-of-range values up front rather than letting the cast silently lie.
        let Ok(pid_t) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // kill(pid, 0) checks if the process exists without sending a signal. A
        // `-1` return alone is not enough: EPERM means the process exists but we
        // lack permission to signal it (e.g. PID 1, owned by root) -- still alive.
        // Only ESRCH (or any other errno) means no such process.
        if unsafe { libc::kill(pid_t, 0) } == 0 {
            return true;
        }
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        // No libc::kill equivalent on Windows -- ask sysinfo (already a proven
        // dependency elsewhere in this workspace) whether the PID is a live process.
        // A stale-lock left by a dead process must be detected here too, not treated
        // as permanently alive -- see this fn's Cargo.toml dependency comment.
        let mut sys = sysinfo::System::new();
        sys.refresh_processes(
            sysinfo::ProcessesToUpdate::Some(&[sysinfo::Pid::from_u32(pid)]),
            true,
        );
        sys.process(sysinfo::Pid::from_u32(pid)).is_some()
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
/// If the advisory lock still can't be acquired after a bounded retry, this
/// proceeds anyway with a warning rather than failing the caller's operation
/// outright — these are command-triggered metadata saves, not interactive
/// buffer edits, and it's better to risk a rare lost update than to hang or
/// error out a user-visible action over a lock that (per `acquire_lock`'s own
/// `is_process_alive` check) can only be genuinely held by a live process.
///
/// **The retry budget was widened (ADR-058 Phase B) from an original
/// `3 × 15ms` (≈45ms total) after a real 3-way-concurrent adversarial test
/// (`kb_init_project_converges_to_one_instance_under_a_three_way_race`)
/// caught it silently losing/duplicating a registration under genuine tight
/// contention: the original budget's doc comment reasoned "the reload-before-
/// mutate step already closes most of the race even without the lock," but
/// that reasoning only holds when one caller's `load()` happens after
/// another's `save()` completes — under real concurrent starts (e.g. a GUI
/// session, a headless instance, and a paired external editor all
/// provisioning the same project within the same second) multiple callers
/// can `load()` before any of them `save()`, and the loser's save then
/// silently overwrites the winner's rather than merging with it.** `30 ×
/// 20ms` (≈600ms worst case) comfortably outlasts a small-TOML-file
/// load+mutate+save cycle for every current caller (`kb-registry.toml`,
/// `projects.toml`, package-lockfile, config known-set) while staying well
/// under user-noticeable latency for what's already a discrete,
/// explicitly-triggered action, not a hot path.
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
    let guard = LockGuard::try_acquire_with_retry(lock_target, 30, Duration::from_millis(20));
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

/// How long [`locked_update`] waits for a contended lock before failing.
///
/// An update holds the lock for one load + mutate + save of a small file
/// (milliseconds). Five seconds outlasts any realistic burst of concurrent
/// updates (ADR-058's three-way race) while still surfacing a lock that is
/// never going to be released.
pub const LOCKED_UPDATE_WAIT: Duration = Duration::from_secs(5);

/// Why a [`locked_update`] did not happen. Nothing was saved in any case.
#[derive(Debug)]
pub enum LockedUpdateError<E> {
    /// Another holder kept the lock past [`LOCKED_UPDATE_WAIT`]. `holder` is
    /// what its lock file says (`None` if it could not be read), `lock_file`
    /// where it is.
    Contended {
        holder: Option<LockInfo>,
        lock_file: PathBuf,
    },
    /// The current state could not be loaded. Saving "nothing + my change"
    /// over it would destroy it, so nothing is saved.
    Load(E),
    /// The mutated state could not be saved.
    Save(io::Error),
    /// The lock itself could not be taken (an I/O error, not contention).
    Lock(io::Error),
}

impl<E: std::fmt::Display + std::fmt::Debug> std::error::Error for LockedUpdateError<E> {}

impl<E: std::fmt::Display> std::fmt::Display for LockedUpdateError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Contended { holder, lock_file } => {
                match holder {
                    Some(h) => {
                        let now = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                        write!(
                            f,
                            "locked by another MAE process (pid {} on {}, for {}s)",
                            h.pid,
                            h.hostname,
                            now.saturating_sub(h.timestamp)
                        )?;
                    }
                    None => write!(f, "locked by another MAE process")?,
                }
                write!(
                    f,
                    "; nothing was changed — try again ({})",
                    lock_file.display()
                )
            }
            Self::Load(e) => write!(f, "could not be read ({e}); nothing was changed"),
            Self::Save(e) => write!(f, "could not be saved: {e}"),
            Self::Lock(e) => write!(f, "could not be locked ({e}); nothing was changed"),
        }
    }
}

/// Reload-fresh → mutate → save under the advisory lock, and ONLY under it.
///
/// The strict sibling of [`with_locked_update`], for state whose loss is not
/// "a rare lost update" but data loss — the KB registry is the only record of
/// which KBs exist. It differs in exactly the two places that lost data:
/// - contention past [`LOCKED_UPDATE_WAIT`] is an error, never an unlocked
///   write (`with_locked_update` writes anyway after ~600 ms);
/// - a state that cannot be loaded is an error, never "default + my change"
///   saved over the unreadable file.
///
/// On any error nothing is saved and `mutate` has not run, so the caller can
/// keep its previous in-memory state and report the failure.
///
/// The lock is [`UpdateLock`]: an OS file lock, which the kernel releases when
/// its holder dies (so a crash never leaves it held), layered over the legacy
/// PID file that older MAE versions still honour.
pub fn locked_update<T, R, E>(
    lock_target: &Path,
    load: impl FnOnce() -> Result<T, E>,
    mutate: impl FnOnce(&mut T) -> R,
    save: impl FnOnce(&T) -> io::Result<()>,
) -> Result<(T, R), LockedUpdateError<E>> {
    locked_update_waiting(LOCKED_UPDATE_WAIT, lock_target, load, mutate, save)
}

fn locked_update_waiting<T, R, E>(
    wait: Duration,
    lock_target: &Path,
    load: impl FnOnce() -> Result<T, E>,
    mutate: impl FnOnce(&mut T) -> R,
    save: impl FnOnce(&T) -> io::Result<()>,
) -> Result<(T, R), LockedUpdateError<E>> {
    let deadline = std::time::Instant::now() + wait;
    let mut backoff = Duration::from_millis(5);
    let _guard = loop {
        if let Some(guard) =
            UpdateLock::try_acquire(lock_target).map_err(LockedUpdateError::Lock)?
        {
            break guard;
        }
        if std::time::Instant::now() >= deadline {
            let lock_file = lock_path(lock_target);
            return Err(LockedUpdateError::Contended {
                holder: read_lock(&lock_file),
                lock_file,
            });
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_millis(100));
    };
    let mut value = load().map_err(LockedUpdateError::Load)?;
    let result = mutate(&mut value);
    save(&value).map_err(LockedUpdateError::Save)?;
    Ok((value, result))
}

/// The exclusive lock [`locked_update`] holds for one update.
///
/// Two layers, because the two kinds of contender honour different things:
/// - an OS lock (`File::try_lock`) on `.<name>.mae.oslock`, a file that is never
///   deleted: every current MAE process serializes on it, and the kernel
///   releases it when the holder exits or crashes — no stale state, no PID
///   reuse, no empty-file-after-crash;
/// - the legacy PID file (`.<name>.mae.lock`, [`acquire_lock`]) that OLDER MAE
///   versions check. It is created while holding the OS lock, so they still back
///   off. Holding the OS lock also makes reclaiming a stale legacy file safe: no
///   current-version process can be mid-acquire, so a legacy file that is empty,
///   unreadable, names a dead PID, or names THIS process's PID (a previous
///   incarnation with the same PID — routine in containers) is leftover, not a
///   holder. Only a live FOREIGN PID (an older version) means contention.
///
/// Any I/O error is an error, never "acquired".
struct UpdateLock {
    _os: std::fs::File,
    legacy: PathBuf,
}

impl UpdateLock {
    /// `Ok(None)` when another process holds it.
    fn try_acquire(target: &Path) -> io::Result<Option<Self>> {
        let legacy = lock_path(target);
        if let Some(parent) = legacy.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let os_path = legacy.with_extension("oslock");
        let os = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&os_path)?;
        match os.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => return Err(e),
        }
        // Holding the OS lock: settle the legacy PID file.
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&legacy)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    let json = serde_json::to_string(&LockInfo::current())
                        .map_err(|e| io::Error::other(e.to_string()))?;
                    if let Err(e) = f.write_all(json.as_bytes()).and_then(|()| f.sync_all()) {
                        let _ = std::fs::remove_file(&legacy);
                        return Err(e);
                    }
                    return Ok(Some(UpdateLock { _os: os, legacy }));
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    match read_lock(&legacy) {
                        Some(h) if h.pid != std::process::id() && is_process_alive(h.pid) => {
                            // An older MAE version holds it.
                            return Ok(None);
                        }
                        // Leftover (see the type doc): reclaim and retry once.
                        _ => match std::fs::remove_file(&legacy) {
                            Ok(()) => continue,
                            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                            Err(e) => return Err(e),
                        },
                    }
                }
                Err(e) => return Err(e),
            }
        }
        // An older version re-created it between our reclaim and retry.
        Ok(None)
    }
}

impl Drop for UpdateLock {
    fn drop(&mut self) {
        // Remove the legacy file before the OS lock is released (when `_os`
        // drops right after), so no current-version process sees it as ours.
        let _ = std::fs::remove_file(&self.legacy);
    }
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
    #[cfg(unix)]
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
    #[cfg(unix)]
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
    #[cfg(unix)]
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

    /// `locked_update` saves only what it loaded and mutated, under the lock.
    #[test]
    fn locked_update_loads_mutates_and_saves() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("state.txt");
        std::fs::write(&file, "1").unwrap();
        let (value, doubled) = locked_update_waiting(
            Duration::from_millis(50),
            &file,
            || std::fs::read_to_string(&file).map(|s| s.trim().parse::<u32>().unwrap()),
            |v| {
                *v += 1;
                *v * 2
            },
            |v| std::fs::write(&file, v.to_string()),
        )
        .expect("uncontended update");
        assert_eq!((value, doubled), (2, 4));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "2");
        assert!(!lock_path(&file).exists(), "the lock was not released");
    }

    /// A state that cannot be loaded is never replaced: `mutate` does not run
    /// and nothing is saved (the old helper saved "default + my change").
    #[test]
    fn locked_update_saves_nothing_when_the_state_cannot_be_loaded() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("state.txt");
        std::fs::write(&file, "unparseable").unwrap();
        let mut mutated = false;
        let mut saved = false;
        let r = locked_update_waiting(
            Duration::from_millis(50),
            &file,
            || Err::<u32, _>("bad state".to_string()),
            |_| mutated = true,
            |_| {
                saved = true;
                Ok(())
            },
        );
        assert!(matches!(r, Err(LockedUpdateError::Load(_))), "{r:?}");
        assert!(!mutated && !saved, "mutated={mutated} saved={saved}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "unparseable");
        assert!(!lock_path(&file).exists(), "the lock was not released");
    }

    /// Runs a tiny `locked_update` on `file` (containing "1") with a short wait;
    /// returns the result and whether `save` ran.
    fn try_update(file: &Path) -> (Result<(u32, ()), LockedUpdateError<String>>, bool) {
        let mut saved = false;
        let r = locked_update_waiting(
            Duration::from_millis(100),
            file,
            || Ok::<u32, String>(1),
            |v| *v += 1,
            |_| {
                saved = true;
                Ok(())
            },
        );
        (r, saved)
    }

    fn state_file() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("state.txt");
        std::fs::write(&file, "1").unwrap();
        (tmp, file)
    }

    /// A current-version holder (the OS lock) blocks the update: it fails after
    /// the wait, saves nothing, and names the lock file.
    #[test]
    fn locked_update_fails_closed_while_another_update_holds_the_lock() {
        let (_tmp, file) = state_file();
        let held = UpdateLock::try_acquire(&file)
            .unwrap()
            .expect("fixture: lock taken");
        let (r, saved) = try_update(&file);
        let err = r.expect_err("a contended update must fail, not write unlocked");
        assert!(
            matches!(err, LockedUpdateError::Contended { .. }),
            "{err:?}"
        );
        assert!(
            err.to_string()
                .contains(&lock_path(&file).display().to_string()),
            "the error must name the lock file: {err}"
        );
        assert!(!saved, "a contended update saved");
        drop(held);
        let (r, saved) = try_update(&file);
        assert!(
            r.is_ok() && saved,
            "after release the update must run: {r:?}"
        );
    }

    /// An OLDER MAE version honours only the legacy PID file. A live foreign PID
    /// in it is a holder: the update fails closed and leaves its file alone.
    #[cfg(unix)]
    #[test]
    fn locked_update_respects_an_older_versions_live_legacy_lock() {
        let (_tmp, file) = state_file();
        let foreign = LockInfo {
            pid: std::os::unix::process::parent_id(),
            ..LockInfo::current()
        };
        assert!(
            is_process_alive(foreign.pid),
            "fixture: the parent process is alive"
        );
        std::fs::write(lock_path(&file), serde_json::to_string(&foreign).unwrap()).unwrap();
        let (r, saved) = try_update(&file);
        assert!(
            matches!(
                r,
                Err(LockedUpdateError::Contended {
                    holder: Some(_),
                    ..
                })
            ),
            "{r:?}"
        );
        assert!(!saved, "saved while an older version held the lock");
        assert!(
            lock_path(&file).exists(),
            "the older version's lock was removed"
        );
    }

    /// Leftovers from a crash are reclaimed, not reported as holders: a legacy
    /// file naming THIS process's PID (a restarted container reuses PIDs), an
    /// empty one (a crash between create and write, or a full disk), and one
    /// naming a dead PID. Each used to block every update (the first two
    /// forever).
    #[test]
    fn locked_update_reclaims_leftover_legacy_lock_files() {
        let own = serde_json::to_string(&LockInfo::current()).unwrap();
        let dead = serde_json::to_string(&LockInfo {
            pid: dead_pid(),
            ..LockInfo::current()
        })
        .unwrap();
        for (label, content) in [
            ("own pid", own),
            ("empty", String::new()),
            ("dead pid", dead),
        ] {
            let (_tmp, file) = state_file();
            std::fs::write(lock_path(&file), &content).unwrap();
            let (r, saved) = try_update(&file);
            assert!(
                r.is_ok() && saved,
                "{label}: a leftover lock blocked the update: {r:?}"
            );
            assert!(
                !lock_path(&file).exists(),
                "{label}: the legacy lock was not released"
            );
        }
    }

    /// A PID that is not running: spawn a process and wait for it to exit.
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!is_process_alive(pid), "fixture: pid {pid} still alive");
        pid
    }

    /// An I/O error while locking is an error, never "acquired": the old lock
    /// treated any create error other than AlreadyExists as success.
    #[cfg(unix)]
    #[test]
    fn locked_update_fails_when_the_lock_cannot_be_created() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return; // root ignores directory permissions; nothing to test
        }
        // Two places it can fail: creating the OS lock file, or (with that file
        // already present from an earlier update) creating the legacy one.
        for os_lock_exists in [false, true] {
            let (tmp, file) = state_file();
            if os_lock_exists {
                std::fs::write(lock_path(&file).with_extension("oslock"), b"").unwrap();
            }
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
            let (r, saved) = try_update(&file);
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(
                matches!(r, Err(LockedUpdateError::Lock(_))),
                "os lock file present: {os_lock_exists}: {r:?}"
            );
            assert!(
                !saved,
                "os lock file present: {os_lock_exists}: saved without a lock"
            );
        }
    }
}
