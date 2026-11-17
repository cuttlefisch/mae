//! Kill safety. Before a file is mutated, its original bytes are written to a
//! journal and synced; the next start restores whatever the journal names, so
//! even a SIGKILL or an OOM kill of the tool itself leaves no mutant behind.
//! SIGINT and SIGTERM restore at once (see `signals`). A lock file stops a
//! second run in the same worktree from restoring under a live one.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const JOURNAL: &str = "journal.toml";
const LOCK: &str = "lock";

#[derive(Serialize, Deserialize)]
struct Journal {
    file: Vec<Original>,
}

#[derive(Serialize, Deserialize)]
struct Original {
    path: String,
    original: String,
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut f = fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    f.write_all(bytes)
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Record the originals durably. Only after this returns may a file change.
pub fn record(state: &Path, root: &Path, files: &[(String, String, String)]) -> Result<(), String> {
    let j = Journal {
        file: files
            .iter()
            .map(|(p, orig, _)| Original {
                path: root.join(p).to_string_lossy().into_owned(),
                original: orig.clone(),
            })
            .collect(),
    };
    let text = toml::to_string(&j).map_err(|e| format!("journal: {e}"))?;
    let tmp = state.join("journal.tmp");
    write_synced(&tmp, text.as_bytes())?;
    fs::rename(&tmp, state.join(JOURNAL)).map_err(|e| format!("journal: {e}"))?;
    // The rename itself must survive a crash too.
    if let Ok(d) = fs::File::open(state) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// Put every journalled file back and drop the journal. Idempotent: a kill
/// between the two steps only repeats the restore next time.
pub fn restore(state: &Path) -> Result<Vec<String>, String> {
    let path = state.join(JOURNAL);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let j: Journal = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut done = Vec::new();
    for f in &j.file {
        write_file(Path::new(&f.path), &f.original)?;
        done.push(f.path.clone());
    }
    fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(done)
}

/// Write a source file. Its mtime is whatever the write gives it; the build
/// step checks that cargo really recompiled (see `run::build_step`).
pub fn write_file(path: &Path, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Push a file's mtime past any build reference cargo can hold, so a build
/// that skipped it as fresh recompiles it. Two seconds covers filesystems
/// whose timestamps have a one- or two-second granularity.
pub fn bump_mtime(path: &Path) -> Result<(), String> {
    let t = SystemTime::now() + Duration::from_secs(2);
    fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(t))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// One run per worktree at a time. Released on drop.
pub struct Lock(PathBuf);

impl Lock {
    pub fn acquire(state: &Path) -> Result<Lock, String> {
        let path = state.join(LOCK);
        for _ in 0..2 {
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    let _ = write!(f, "{}", std::process::id());
                    return Ok(Lock(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let pid = fs::read_to_string(&path).unwrap_or_default();
                    if alive(pid.trim()) {
                        return Err(format!(
                            "another mae-mutate run (pid {}) holds {}",
                            pid.trim(),
                            path.display()
                        ));
                    }
                    // Left by a run that was killed: take it over.
                    let _ = fs::remove_file(&path);
                }
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Err(format!("could not take {}", path.display()))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(unix)]
fn alive(pid: &str) -> bool {
    if pid.is_empty() || !pid.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(unix))]
fn alive(pid: &str) -> bool {
    // No portable liveness probe without a dependency: a stale lock on such a
    // platform is removed by hand, which the refusal message names.
    !pid.is_empty()
}

/// SIGINT and SIGTERM set a flag; the command loop polls it, stops the child,
/// restores the files and exits. Nothing unsafe runs in the handler itself.
pub mod signals {
    use std::sync::atomic::{AtomicBool, Ordering};

    static INTERRUPTED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(_: i32) {
        INTERRUPTED.store(true, Ordering::SeqCst);
    }

    pub fn interrupted() -> bool {
        INTERRUPTED.load(Ordering::SeqCst)
    }

    #[cfg(unix)]
    pub fn install() {
        extern "C" {
            fn signal(sig: i32, handler: extern "C" fn(i32)) -> usize;
        }
        const SIGINT: i32 = 2;
        const SIGTERM: i32 = 15;
        // SAFETY: `signal` is libc's, the handler only stores to an atomic.
        unsafe {
            signal(SIGINT, on_signal);
            signal(SIGTERM, on_signal);
        }
    }

    #[cfg(not(unix))]
    pub fn install() {}
}
