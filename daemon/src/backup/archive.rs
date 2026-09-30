//! Writing and strictly reading the backup tar.
//!
//! Written: the manifest first, then exactly the files it lists, every entry a
//! regular file with mode 0600 (the archive holds a private key).
//! Read: the manifest first, then only regular files it lists, each once, and
//! all of them. So a restore can never write a `-wal`/`-shm`, a symlink, or
//! anything outside its target directory, even from a tampered archive.

use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use super::manifest::{self, Manifest, MANIFEST_MAX_BYTES, MANIFEST_PATH};

fn header(size: u64, mtime: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(size);
    h.set_mode(0o600);
    h.set_mtime(mtime);
    h
}

/// Write the archive for `m` from files staged under `staging`, to `out`
/// through `<out>.parcial` and a rename, so a reader never sees half a file.
pub(crate) fn write(m: &Manifest, staging: &Path, out: &Path) -> Result<(), String> {
    let mut partial = out.as_os_str().to_owned();
    partial.push(".parcial");
    let partial = std::path::PathBuf::from(partial);
    let result = write_to(m, staging, &partial).and_then(|_| {
        std::fs::rename(&partial, out).map_err(|e| format!("rename to {}: {e}", out.display()))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn write_to(m: &Manifest, staging: &Path, path: &Path) -> Result<(), String> {
    let file = create_private(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut builder = tar::Builder::new(file);
    let json = serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?;
    builder
        .append_data(
            &mut header(json.len() as u64, m.created_unix),
            MANIFEST_PATH,
            json.as_slice(),
        )
        .map_err(|e| format!("write manifest: {e}"))?;
    for f in &m.files {
        let src = File::open(staging.join(&f.path)).map_err(|e| format!("{}: {e}", f.path))?;
        builder
            .append_data(&mut header(f.bytes, m.created_unix), &f.path, src)
            .map_err(|e| format!("write {}: {e}", f.path))?;
    }
    let mut file = builder
        .into_inner()
        .map_err(|e| format!("finish archive: {e}"))?;
    file.flush()
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("sync: {e}"))
}

/// A file readable by its owner only (on Unix), truncated if it exists. Only
/// ever called on a `.parcial` path or inside a directory checked empty.
fn create_private(path: &Path) -> std::io::Result<File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    opts.open(path)
}

/// Extract `archive` into the EMPTY directory `dir`, returning its manifest.
pub(crate) fn extract(archive: &Path, dir: &Path) -> Result<Manifest, String> {
    let file = File::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
    let mut tar = tar::Archive::new(file);
    let mut entries = tar.entries().map_err(|e| format!("read archive: {e}"))?;
    let first = entries
        .next()
        .ok_or("the archive is empty")?
        .map_err(|e| format!("read archive: {e}"))?;
    let m = read_manifest(first)?;
    let mut expected: HashSet<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("read archive: {e}"))?;
        let path = entry_path(&entry)?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            return Err(format!("{path}: not a regular file"));
        }
        if !expected.remove(path.as_str()) {
            return Err(format!(
                "{path}: not listed in the manifest, or listed once and seen twice"
            ));
        }
        unpack_private(&mut entry, dir, &path)?;
    }
    if let Some(missing) = expected.iter().next() {
        return Err(format!(
            "the archive is missing {missing}, which its manifest lists"
        ));
    }
    std::fs::write(
        dir.join(MANIFEST_PATH),
        serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("write manifest: {e}"))?;
    Ok(m)
}

fn read_manifest(mut entry: tar::Entry<'_, File>) -> Result<Manifest, String> {
    if entry_path(&entry)? != MANIFEST_PATH {
        return Err(format!(
            "the first entry must be {MANIFEST_PATH}: not a MAE daemon backup"
        ));
    }
    if entry.header().size().unwrap_or(u64::MAX) > MANIFEST_MAX_BYTES {
        return Err("the manifest is implausibly large".into());
    }
    let mut buf = Vec::new();
    entry
        .read_to_end(&mut buf)
        .map_err(|e| format!("read manifest: {e}"))?;
    Manifest::parse(&buf)
}

fn entry_path(entry: &tar::Entry<'_, File>) -> Result<String, String> {
    let raw = entry.path_bytes();
    let path = std::str::from_utf8(&raw).map_err(|_| "archive path is not UTF-8".to_string())?;
    if path != MANIFEST_PATH {
        manifest::check_archive_path(path)?;
    }
    Ok(path.to_string())
}

/// Write the entry's bytes to `dir/path` with owner-only permissions.
fn unpack_private(entry: &mut tar::Entry<'_, File>, dir: &Path, path: &str) -> Result<(), String> {
    let dst = dir.join(path);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let mut out = create_private(&dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    std::io::copy(entry, &mut out).map_err(|e| format!("extract {path}: {e}"))?;
    Ok(())
}
