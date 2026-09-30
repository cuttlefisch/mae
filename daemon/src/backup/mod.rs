//! `mae-daemon backup create|restore|verify`: whole-instance backup.
//!
//! One archive holds every KB store, the collab store, the KB registry and the
//! identity, plus a manifest with each file's hash and each store's node count.
//! `restore` extracts it into an empty directory; `verify` proves what was
//! restored matches the manifest and prints `key=number` lines.
//!
//! Distinct from `checkpoint`/`restore` (ADR-032), which export and replay ONE
//! KB's CRDT documents, and from `mae_kb::backup`'s per-KB slug layout. This
//! reuses that module's WAL-safe `snapshot` rather than adding a third way to
//! copy a store.
//!
//! @ai-caution: [secrets] The archive contains the daemon's private identity
//! key (that is what makes a restore complete). It is written 0600, but it is
//! NOT encrypted: whatever holds the archive holds the key.

mod archive;
mod create;
mod manifest;
mod verify;

#[cfg(test)]
pub(crate) use manifest::{check_archive_path, hash_file, store_key, Manifest, MANIFEST_PATH};

/// The verification result itself, for tests that assert on its parts.
#[cfg(test)]
pub(crate) fn verify_report(dir: &Path) -> Result<verify::Report, String> {
    verify::verify(dir)
}

use std::path::{Path, PathBuf};

use crate::config::DaemonConfig;

const USAGE: &str = "Usage:\n  mae-daemon backup create <out.tar>\n  \
                     mae-daemon backup restore <archive.tar> <empty-dir>\n  \
                     mae-daemon backup verify <restored-dir>";

/// Dispatch `mae-daemon backup <sub> …`. Returns the process exit code:
/// 0 ok, 1 failure, 2 usage.
pub(crate) fn run(config: &DaemonConfig, rest: &[String]) -> i32 {
    let args: Vec<&str> = rest.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        ["create", out] => run_create(config, Path::new(out)),
        ["restore", archive, dir] => run_restore(Path::new(archive), Path::new(dir)),
        ["verify", dir] => return run_verify(Path::new(dir)),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    }
}

/// Removes the staging directory however `create` ends.
struct Staging(PathBuf);
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn run_create(config: &DaemonConfig, out: &Path) -> Result<(), String> {
    if out.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite a backup",
            out.display()
        ));
    }
    let mut staging = out.as_os_str().to_owned();
    staging.push(format!(".staging-{}", std::process::id()));
    let staging = Staging(PathBuf::from(staging));
    std::fs::create_dir_all(&staging.0).map_err(|e| format!("{}: {e}", staging.0.display()))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let m = create::stage_all(&create::sources(config), &staging.0, now)?;
    archive::write(&m, &staging.0, out)?;
    for s in &m.stores {
        println!("{}={}", s.key, s.nodes);
    }
    println!("identity={}", u8::from(m.identity));
    eprintln!("backup: {} files -> {}", m.files.len() + 1, out.display());
    Ok(())
}

pub(crate) fn run_restore(archive_path: &Path, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let occupied = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .next()
        .is_some();
    if occupied {
        return Err(format!(
            "{} is not empty; restore only into an empty directory (it never overwrites)",
            dir.display()
        ));
    }
    let m = archive::extract(archive_path, dir)?;
    println!(
        "restored: {} files, {} KB stores, identity={} -> {}",
        m.files.len(),
        m.stores.len(),
        u8::from(m.identity),
        dir.display()
    );
    Ok(())
}

/// Prints the `key=number` lines even when verification fails, so an operator
/// sees what WAS restored next to what disagrees.
pub(crate) fn run_verify(dir: &Path) -> i32 {
    match verify::verify(dir) {
        Ok(report) => {
            for line in &report.lines {
                println!("{line}");
            }
            for p in &report.problems {
                eprintln!("FAIL: {p}");
            }
            i32::from(!report.problems.is_empty())
        }
        Err(e) => {
            eprintln!("Error: {e}");
            1
        }
    }
}
