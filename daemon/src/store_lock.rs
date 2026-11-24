//! The collab store's exclusive lock: one process at a time may open
//! `state.db` to write it.
//!
//! The serving daemon takes it when it opens its doc store and holds it for its
//! life; an offline operator command that writes the store (`kb seed`, `kb
//! export`, `restore`) takes it before opening the store, so neither can write
//! behind the other's back. It is an OS advisory lock on `state.db.lock`
//! (`File::try_lock`): the kernel drops it when the holder exits, however it
//! exits, so a crash never leaves a stale lock and a reused PID cannot hold one.
//!
//! @ai-caution: [daemon-state] The operator side ALSO checks that no daemon
//! answers on the control socket, after taking the lock: a daemon built before
//! this lock existed does not take it. Lock first, then check, and the daemon
//! takes the lock before it serves, so whichever starts second sees the other.
//!
//! @ai-caution: [daemon-state] A dropped lock is not always free at once. The
//! lock belongs to the open file description, and a child forked by any
//! thread of the holder's process shares that description until it `exec`s.
//! So a lock dropped while another thread is spawning can stay held for that
//! window (measured: about 150 µs typical, 0.4 ms worst, idle; longer under
//! load). [`StoreLock::acquire`] therefore retries for [`GRACE`] before it
//! refuses (#1008).

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long [`StoreLock::acquire`] keeps trying a held lock before refusing.
/// It must outlast a child's fork-to-exec window, which holds a just-released
/// lock (see the module's caution), by a wide margin even on a loaded machine:
/// 250 ms is over 600 times the worst window measured. It must stay short,
/// because a store that really is held is refused only after it: an operator
/// waits this long to hear that a daemon is running. Pinned by
/// `a_lock_released_while_other_threads_spawn_is_acquired`.
pub const GRACE: Duration = Duration::from_millis(250);

/// The pause between attempts while waiting out [`GRACE`].
const RETRY: Duration = Duration::from_millis(5);

/// The lock file, next to the store it guards.
pub fn lock_path(collab_data_dir: &Path) -> PathBuf {
    collab_data_dir.join("state.db.lock")
}

/// A held store lock; released when dropped (or when the process exits).
#[derive(Debug)]
pub struct StoreLock {
    _file: File,
}

impl StoreLock {
    /// Take the lock, trying for up to [`GRACE`] while it is held. `Err` says
    /// why, naming the lock file: another process holds it, or it could not
    /// be opened (never "proceed without it"). Blocks the calling thread for
    /// at most [`GRACE`], and only while the lock is held.
    pub fn acquire(collab_data_dir: &Path) -> Result<StoreLock, String> {
        Self::acquire_with(collab_data_dir, GRACE, || std::thread::sleep(RETRY))
    }

    /// [`acquire`](Self::acquire) with its grace and its pause between
    /// attempts given: `wait` runs after each refused attempt made before
    /// `grace` is up. Tests drive it without a clock.
    pub(crate) fn acquire_with(
        collab_data_dir: &Path,
        grace: Duration,
        mut wait: impl FnMut(),
    ) -> Result<StoreLock, String> {
        let path = lock_path(collab_data_dir);
        std::fs::create_dir_all(collab_data_dir)
            .map_err(|e| format!("create {}: {e}", collab_data_dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        let start = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(StoreLock { _file: file }),
                Err(TryLockError::WouldBlock) if start.elapsed() < grace => wait(),
                Err(TryLockError::WouldBlock) => {
                    return Err(format!(
                        "the store is in use: another process holds {} (a running daemon, or \
                         an operator command such as `kb seed` or `restore`)",
                        path.display()
                    ))
                }
                Err(TryLockError::Error(e)) => return Err(format!("lock {}: {e}", path.display())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Kills: a lock that does not exclude (e.g. `acquire` returning Ok
    /// without `try_lock`). A second open file description is refused while
    /// the first is held, and admitted once it is dropped.
    #[test]
    fn a_held_lock_refuses_a_second_holder_until_released() {
        let dir = tempfile::tempdir().unwrap();
        let first = StoreLock::acquire(dir.path()).unwrap();
        let err = StoreLock::acquire(dir.path()).unwrap_err();
        assert!(err.contains("in use"), "{err}");
        drop(first);
        StoreLock::acquire(dir.path()).unwrap();
    }

    /// The retry, without a clock: a holder that lets go after the first
    /// refused attempt is waited for; one that never does is refused once the
    /// grace is up. Kills: no retry, and a retry that ignores the grace.
    #[test]
    fn a_held_lock_is_waited_for_within_the_grace_and_refused_after_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut holder = Some(StoreLock::acquire(dir.path()).unwrap());
        let mut waits = 0;
        StoreLock::acquire_with(dir.path(), Duration::from_secs(3600), || {
            waits += 1;
            holder = None;
        })
        .expect("released after the first refusal: acquired on the next try");
        assert_eq!(waits, 1);

        let _held = StoreLock::acquire(dir.path()).unwrap();
        let mut waits = 0;
        let err = StoreLock::acquire_with(dir.path(), Duration::ZERO, || {
            waits += 1;
            assert!(waits < 1000, "the grace is never up");
        })
        .unwrap_err();
        assert!(err.contains("in use"), "{err}");
        assert_eq!(waits, 0, "a grace of zero tries once");
    }

    /// The real mechanism (#1008): other threads spawning processes keep a
    /// just-dropped lock held until their child execs, which refused about a
    /// third of immediate re-acquires with one spawning thread. With the grace,
    /// none is refused. The children are `true`, run through `sh` so the test
    /// is the same on Linux and macOS.
    #[cfg(unix)]
    #[test]
    fn a_lock_released_while_other_threads_spawn_is_acquired() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let spawners: Vec<_> = (0..2)
            .map(|_| {
                let stop = Arc::clone(&stop);
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        let _ = std::process::Command::new("sh")
                            .args(["-c", "true"])
                            .status();
                    }
                })
            })
            .collect();
        let mut refused = Vec::new();
        for i in 0..300 {
            match StoreLock::acquire(dir.path()) {
                Ok(lock) => drop(lock),
                Err(e) => refused.push((i, e)),
            }
        }
        stop.store(true, Ordering::Relaxed);
        for s in spawners {
            s.join().unwrap();
        }
        assert!(
            refused.is_empty(),
            "{} refused: {:?}",
            refused.len(),
            refused.first()
        );
    }
}
