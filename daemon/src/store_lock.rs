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

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

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
    /// Take the lock without waiting. `Err` says why, naming the lock file:
    /// another process holds it, or it could not be opened (never "proceed
    /// without it").
    pub fn acquire(collab_data_dir: &Path) -> Result<StoreLock, String> {
        let path = lock_path(collab_data_dir);
        std::fs::create_dir_all(collab_data_dir)
            .map_err(|e| format!("create {}: {e}", collab_data_dir.display()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| format!("open {}: {e}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(StoreLock { _file: file }),
            Err(TryLockError::WouldBlock) => Err(format!(
                "the store is in use: another process holds {} (a running daemon, or an \
                 operator command such as `kb seed` or `restore`)",
                path.display()
            )),
            Err(TryLockError::Error(e)) => Err(format!("lock {}: {e}", path.display())),
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
}
