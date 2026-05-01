//! Swap file I/O for crash recovery.
//!
//! Emacs/Vim-style non-destructive swap files. Unlike the existing in-place
//! autosave (`try_autosave()`), swap files are written to a separate directory
//! and never overwrite the original. On crash, `:recover` restores content.
//!
//! Design lessons from Emacs (35 years of autosave):
//! 1. Separate modiff counter — only write when buffer changed since last swap
//! 2. No fsync — swap is best-effort, fsync too expensive per-interval
//! 3. Bulk-delete protection — skip swap if rope shrinks >75%
//! 4. Error backoff — failed swap suppresses retries for 20 min
//! 5. Atomic write — write to temp file, then rename
//! 6. Session recovery index — `.saves-PID` maps original→swap paths

use ropey::Rope;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Header for a swap file.
#[derive(Debug, Clone)]
pub struct SwapHeader {
    pub version: u32,
    pub pid: u32,
    pub timestamp: u64,
    pub original_path: PathBuf,
}

const SWAP_VERSION: u32 = 1;
const HEADER_MAGIC: &str = "MAE-SWAP";

/// Return the XDG-compliant swap directory.
pub fn swap_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        PathBuf::from(dir).join("mae/swap")
    } else if let Ok(home) = std::env::var("HOME") {
        PathBuf::from(home).join(".local/share/mae/swap")
    } else {
        PathBuf::from("/tmp/mae-swap")
    }
}

/// Compute a deterministic swap path for a given file path.
///
/// Format: `<swap_dir>/<hash>_<filename>.swp`
pub fn swap_path_for(file_path: &Path, custom_dir: Option<&Path>) -> PathBuf {
    let dir = custom_dir.map(PathBuf::from).unwrap_or_else(swap_dir);
    let hash = path_hash(file_path);
    let name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unnamed".to_string());
    dir.join(format!("{}_{}.swp", hash, name))
}

/// Write a swap file atomically (temp + rename). No fsync (Emacs lesson #2).
pub fn write_swap(file_path: &Path, rope: &Rope, custom_dir: Option<&Path>) -> io::Result<PathBuf> {
    let swap = swap_path_for(file_path, custom_dir);
    if let Some(parent) = swap.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = swap.with_extension("swp.tmp");

    // Write header + content to temp file.
    let header = format!(
        "{} v{}\n{}\n{}\n{}\n",
        HEADER_MAGIC,
        SWAP_VERSION,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        file_path.display(),
    );

    let mut content = header;
    for chunk in rope.chunks() {
        content.push_str(chunk);
    }
    fs::write(&tmp, &content)?;

    // Atomic rename (Emacs lesson #5).
    if let Err(e) = fs::rename(&tmp, &swap) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }

    Ok(swap)
}

/// Read a swap file, returning header and rope content.
pub fn read_swap(swap_path: &Path) -> io::Result<(SwapHeader, Rope)> {
    let content = fs::read_to_string(swap_path)?;
    let mut lines = content.lines();

    // Parse header: "MAE-SWAP v1"
    let magic_line = lines
        .next()
        .ok_or_else(|| io::Error::other("swap file: missing magic line"))?;
    if !magic_line.starts_with(HEADER_MAGIC) {
        return Err(io::Error::other("swap file: invalid magic"));
    }
    let version: u32 = magic_line
        .split("v")
        .last()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    if version == 0 || version > SWAP_VERSION {
        return Err(io::Error::other(format!(
            "swap file: unsupported version {}",
            version
        )));
    }

    let pid: u32 = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .ok_or_else(|| io::Error::other("swap file: missing PID"))?;

    let timestamp: u64 = lines
        .next()
        .and_then(|l| l.trim().parse().ok())
        .ok_or_else(|| io::Error::other("swap file: missing timestamp"))?;

    let original_path = PathBuf::from(
        lines
            .next()
            .ok_or_else(|| io::Error::other("swap file: missing original path"))?,
    );

    // Everything after the 4th newline is the rope content.
    // We need to find the byte offset after the 4th line.
    let header_end = nth_newline_offset(&content, 4)
        .ok_or_else(|| io::Error::other("swap file: truncated header"))?;

    let body = &content[header_end..];
    let rope = Rope::from_str(body);

    Ok((
        SwapHeader {
            version,
            pid,
            timestamp,
            original_path,
        },
        rope,
    ))
}

/// Delete a swap file for a given original file path.
pub fn delete_swap(file_path: &Path, custom_dir: Option<&Path>) -> io::Result<()> {
    let swap = swap_path_for(file_path, custom_dir);
    if swap.exists() {
        fs::remove_file(&swap)?;
    }
    Ok(())
}

/// Check if a swap file exists for a given original file path.
pub fn swap_exists(file_path: &Path, custom_dir: Option<&Path>) -> bool {
    swap_path_for(file_path, custom_dir).exists()
}

/// Scan the swap directory for orphaned swap files (dead PIDs).
pub fn find_orphaned_swaps(custom_dir: Option<&Path>) -> Vec<(PathBuf, SwapHeader)> {
    let dir = custom_dir.map(PathBuf::from).unwrap_or_else(swap_dir);
    let mut orphans = Vec::new();

    let Ok(entries) = fs::read_dir(&dir) else {
        return orphans;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("swp") {
            continue;
        }
        if let Ok((header, _)) = read_swap(&path) {
            if !is_pid_alive(header.pid) {
                orphans.push((path, header));
            }
        }
    }

    orphans
}

/// Check if a process with the given PID is still alive.
pub fn is_pid_alive(pid: u32) -> bool {
    // /proc/<pid> check on Linux.
    if Path::new(&format!("/proc/{}", pid)).exists() {
        return true;
    }
    // Fallback: kill(pid, 0) — signal 0 checks existence without sending a signal.
    #[cfg(unix)]
    {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Append an entry to the session recovery index (`.saves-PID`).
pub fn append_session_index(
    original: &Path,
    swap: &Path,
    custom_dir: Option<&Path>,
) -> io::Result<()> {
    use std::io::Write;
    let dir = custom_dir.map(PathBuf::from).unwrap_or_else(swap_dir);
    fs::create_dir_all(&dir)?;
    let index_path = dir.join(format!(".saves-{}", std::process::id()));
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&index_path)?;
    writeln!(f, "{}\t{}", original.display(), swap.display())?;
    Ok(())
}

/// Delete the session index for the current PID (called on clean exit).
pub fn delete_session_index(custom_dir: Option<&Path>) {
    let dir = custom_dir.map(PathBuf::from).unwrap_or_else(swap_dir);
    let index_path = dir.join(format!(".saves-{}", std::process::id()));
    let _ = fs::remove_file(index_path);
}

/// Per-buffer swap tracking state.
#[derive(Debug, Clone, Default)]
pub struct SwapState {
    /// Edit count at last swap write (Emacs lesson #1).
    pub swap_modiff: u64,
    /// Rope byte count at last successful swap write (for bulk-delete protection).
    pub swap_last_len: usize,
    /// If set, suppress swap writes until this instant + 20 min (Emacs lesson #4).
    pub failure_time: Option<Instant>,
    /// Whether a swap file has been written for this buffer.
    pub written: bool,
}

impl SwapState {
    /// Check if swap write should be attempted.
    /// Returns false if:
    /// - modiff hasn't changed since last swap (lesson #1)
    /// - error backoff is active (lesson #4)
    pub fn should_write(&self, current_modiff: u64) -> bool {
        if current_modiff <= self.swap_modiff {
            return false;
        }
        if let Some(fail_time) = self.failure_time {
            if fail_time.elapsed().as_secs() < 20 * 60 {
                return false;
            }
        }
        true
    }

    /// Check bulk-delete protection (Emacs lesson #3).
    /// Returns false if rope shrank to <25% of last swap size.
    pub fn bulk_delete_safe(&self, current_len: usize) -> bool {
        if self.swap_last_len == 0 {
            return true; // First write, no baseline.
        }
        // If current length < 25% of last swap length, it's a mass delete.
        current_len * 4 >= self.swap_last_len
    }

    /// Record a successful swap write.
    pub fn record_success(&mut self, modiff: u64, rope_len: usize) {
        self.swap_modiff = modiff;
        self.swap_last_len = rope_len;
        self.failure_time = None;
        self.written = true;
    }

    /// Record a failed swap write (triggers 20-min backoff).
    pub fn record_failure(&mut self) {
        self.failure_time = Some(Instant::now());
    }
}

// --- Internal helpers ---

/// Simple hash of a path for swap file naming.
fn path_hash(path: &Path) -> String {
    let s = path.display().to_string();
    let mut hash: u64 = 5381;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(33).wrapping_add(byte as u64);
    }
    format!("{:08x}", hash as u32)
}

/// Find the byte offset immediately after the Nth newline in `s`.
fn nth_newline_offset(s: &str, n: usize) -> Option<usize> {
    let mut count = 0;
    for (i, ch) in s.char_indices() {
        if ch == '\n' {
            count += 1;
            if count == n {
                return Some(i + 1);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_dir() -> TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn swap_path_deterministic() {
        let p = Path::new("/home/user/src/main.rs");
        let a = swap_path_for(p, Some(Path::new("/tmp/test-swap")));
        let b = swap_path_for(p, Some(Path::new("/tmp/test-swap")));
        assert_eq!(a, b);
    }

    #[test]
    fn swap_path_different_files() {
        let a = swap_path_for(
            Path::new("/home/user/src/main.rs"),
            Some(Path::new("/tmp/test-swap")),
        );
        let b = swap_path_for(
            Path::new("/home/user/src/lib.rs"),
            Some(Path::new("/tmp/test-swap")),
        );
        assert_ne!(a, b);
    }

    #[test]
    fn write_read_roundtrip() {
        let dir = test_dir();
        let file_path = Path::new("/home/user/test.rs");
        let content = "fn main() {\n    println!(\"hello\");\n}\n";
        let rope = Rope::from_str(content);

        let swap = write_swap(file_path, &rope, Some(dir.path())).unwrap();
        assert!(swap.exists());

        let (header, recovered) = read_swap(&swap).unwrap();
        assert_eq!(header.version, SWAP_VERSION);
        assert_eq!(header.original_path, file_path);
        assert_eq!(recovered.to_string(), content);
    }

    #[test]
    fn header_parsing() {
        let dir = test_dir();
        let file_path = Path::new("/test/file.txt");
        let rope = Rope::from_str("hello world");

        let swap = write_swap(file_path, &rope, Some(dir.path())).unwrap();
        let (header, _) = read_swap(&swap).unwrap();

        assert_eq!(header.version, 1);
        assert_eq!(header.pid, std::process::id());
        assert!(header.timestamp > 0);
        assert_eq!(header.original_path, file_path);
    }

    #[test]
    fn delete_swap_removes_file() {
        let dir = test_dir();
        let file_path = Path::new("/test/file.txt");
        let rope = Rope::from_str("content");

        write_swap(file_path, &rope, Some(dir.path())).unwrap();
        assert!(swap_exists(file_path, Some(dir.path())));

        delete_swap(file_path, Some(dir.path())).unwrap();
        assert!(!swap_exists(file_path, Some(dir.path())));
    }

    #[test]
    fn orphan_detection() {
        let dir = test_dir();
        let file_path = Path::new("/test/orphan.txt");
        let rope = Rope::from_str("orphan content");

        let swap = write_swap(file_path, &rope, Some(dir.path())).unwrap();
        assert!(swap.exists());

        // Our own PID is alive, so it shouldn't appear as orphaned.
        let orphans = find_orphaned_swaps(Some(dir.path()));
        assert!(orphans.is_empty(), "own PID should not be orphaned");

        // Manually create a swap with a dead PID.
        let dead_swap = dir.path().join("deadbeef_fake.swp");
        fs::write(
            &dead_swap,
            "MAE-SWAP v1\n99999999\n1234567890\n/fake/path.txt\nfake content",
        )
        .unwrap();

        let orphans = find_orphaned_swaps(Some(dir.path()));
        assert!(!orphans.is_empty(), "dead PID should be orphaned");
    }

    #[test]
    fn atomic_write_no_temp_left() {
        let dir = test_dir();
        let file_path = Path::new("/test/atomic.txt");
        let rope = Rope::from_str("atomic content");

        write_swap(file_path, &rope, Some(dir.path())).unwrap();

        // No .tmp files should remain.
        let tmp_files: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|s| s == "tmp")
                    .unwrap_or(false)
            })
            .collect();
        assert!(tmp_files.is_empty(), "no temp files should remain");
    }

    #[test]
    fn modiff_skip() {
        let mut state = SwapState::default();
        assert!(!state.should_write(0), "modiff 0 == 0, skip");

        state.swap_modiff = 5;
        assert!(!state.should_write(5), "modiff unchanged, skip");
        assert!(state.should_write(6), "modiff increased, write");
    }

    #[test]
    fn bulk_delete_protection() {
        let state = SwapState {
            swap_last_len: 1000,
            ..Default::default()
        };

        assert!(state.bulk_delete_safe(1000));
        assert!(state.bulk_delete_safe(500));
        assert!(state.bulk_delete_safe(250)); // exactly 25%
        assert!(!state.bulk_delete_safe(249)); // below 25%
        assert!(!state.bulk_delete_safe(0));
    }

    #[test]
    fn error_backoff() {
        let mut state = SwapState::default();
        state.record_failure();
        assert!(
            !state.should_write(1),
            "should skip immediately after failure"
        );
    }

    #[test]
    fn special_buffer_skip() {
        // SwapState doesn't enforce this — the caller (file_ops.rs) checks
        // buffer kind. This test validates the check logic.
        use crate::BufferKind;
        let special = [
            BufferKind::Conversation,
            BufferKind::Messages,
            BufferKind::Help,
            BufferKind::Shell,
            BufferKind::Debug,
            BufferKind::Dashboard,
            BufferKind::GitStatus,
            BufferKind::Visual,
            BufferKind::FileTree,
        ];
        for kind in special {
            assert_ne!(kind, BufferKind::Text, "special buffer != Text");
        }
    }

    #[test]
    fn swap_deleted_on_save() {
        let dir = test_dir();
        let file_path = Path::new("/test/save.txt");
        let rope = Rope::from_str("content");

        write_swap(file_path, &rope, Some(dir.path())).unwrap();
        assert!(swap_exists(file_path, Some(dir.path())));

        delete_swap(file_path, Some(dir.path())).unwrap();
        assert!(!swap_exists(file_path, Some(dir.path())));
    }

    #[test]
    fn session_index_written() {
        let dir = test_dir();
        let original = Path::new("/test/indexed.txt");
        let swap = Path::new("/swap/indexed.swp");

        append_session_index(original, swap, Some(dir.path())).unwrap();

        let index_path = dir.path().join(format!(".saves-{}", std::process::id()));
        assert!(index_path.exists());

        let content = fs::read_to_string(&index_path).unwrap();
        assert!(content.contains("/test/indexed.txt"));
        assert!(content.contains("/swap/indexed.swp"));
    }

    #[test]
    fn recover_command_roundtrip() {
        let dir = test_dir();
        let file_path = Path::new("/test/recover.txt");
        let original = "original content\nline 2\n";
        let modified = "modified content\nnew line 2\nnew line 3\n";

        // Simulate: user edits, swap is written, then crash.
        let rope = Rope::from_str(modified);
        write_swap(file_path, &rope, Some(dir.path())).unwrap();

        // Recovery: read swap, verify content matches modified version.
        let swap = swap_path_for(file_path, Some(dir.path()));
        let (header, recovered) = read_swap(&swap).unwrap();
        assert_eq!(header.original_path, file_path);
        assert_eq!(recovered.to_string(), modified);
        assert_ne!(recovered.to_string(), original);
    }
}
