//! Advisory file locking for multi-editor file contention.
//!
//! When MAE opens a file for editing, it creates a `.mae.lock` file alongside
//! it containing the PID, hostname, and timestamp. This prevents MAE-MAE
//! conflicts when multiple instances edit the same file.
//!
//! Other editors (VS Code, etc.) won't see `.mae.lock` — those conflicts are
//! handled by the content-hash verification layer in `buffer.rs`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    fn content_hash_on_buffer() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("hash_test.txt");
        std::fs::write(&file, "hello world").unwrap();

        let buf = crate::buffer::Buffer::from_file(&file).unwrap();
        assert!(buf.content_hash.is_some());
        assert!(!buf.content_hash.as_ref().unwrap().is_empty());
    }
}
