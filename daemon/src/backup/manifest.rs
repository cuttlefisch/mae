//! The manifest a backup archive carries: what is in it, what each file hashes
//! to, and how many KB nodes each store held when it was taken.
//!
//! It is the FIRST entry of the archive, so `restore` knows the complete list of
//! files before it extracts any, and extracts nothing the manifest does not name.

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// `format` field value. Anything else is not our archive.
pub(crate) const FORMAT: &str = "mae-daemon-backup";
/// Layout version. Bump on any change a v1 reader would misread.
pub(crate) const VERSION: u32 = 1;
/// Archive path of the manifest itself.
pub(crate) const MANIFEST_PATH: &str = "manifest.json";
/// Archive directory holding the identity files.
pub(crate) const IDENTITY_DIR: &str = "identity";
/// Upper bound on a manifest's size, checked before it is read into memory.
pub(crate) const MANIFEST_MAX_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct FileEntry {
    /// Archive-relative path, `/`-separated.
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct StoreEntry {
    /// The validation key this store reports under, e.g. `kb.notes`.
    pub key: String,
    /// Archive-relative path of the store file (also listed in `files`).
    pub file: String,
    /// KB node count of the archived copy, counted when the backup was taken.
    pub nodes: u64,
    /// Registry uuid, when the store belongs to a registered KB.
    pub uuid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(crate) struct Manifest {
    pub format: String,
    pub version: u32,
    pub created_unix: u64,
    pub daemon_version: String,
    /// Every file in the archive except the manifest.
    pub files: Vec<FileEntry>,
    /// The KB stores among `files`.
    pub stores: Vec<StoreEntry>,
    /// Whether the daemon's private identity key is among `files`.
    pub identity: bool,
}

impl Manifest {
    /// Parse and check a manifest: right format and version, every path safe,
    /// no duplicates, every store listed as a file.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, String> {
        let m: Manifest =
            serde_json::from_slice(bytes).map_err(|e| format!("manifest is not valid: {e}"))?;
        if m.format != FORMAT {
            return Err(format!("not a {FORMAT} archive (format {:?})", m.format));
        }
        if m.version != VERSION {
            return Err(format!(
                "archive layout version {} is not supported (this build reads {VERSION})",
                m.version
            ));
        }
        let mut seen = std::collections::HashSet::new();
        for f in &m.files {
            check_archive_path(&f.path)?;
            if !seen.insert(f.path.as_str()) {
                return Err(format!("manifest lists {} twice", f.path));
            }
        }
        for s in &m.stores {
            if !seen.contains(s.file.as_str()) {
                return Err(format!(
                    "store {} names a file the manifest does not list",
                    s.key
                ));
            }
        }
        Ok(m)
    }

    pub(crate) fn file(&self, path: &str) -> Option<&FileEntry> {
        self.files.iter().find(|f| f.path == path)
    }
}

/// An archive path must be relative, `/`-separated, made only of plain names,
/// and must not be the manifest. Anything else could write outside the restore
/// directory or shadow the manifest.
pub(crate) fn check_archive_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    let plain = !path.is_empty()
        && !path.contains('\\')
        && p.components().all(|c| matches!(c, Component::Normal(_)));
    if !plain || path == MANIFEST_PATH {
        return Err(format!("unsafe or reserved archive path {path:?}"));
    }
    Ok(())
}

/// The validation key for a KB name: `kb.` plus the name with every character
/// outside `[A-Za-z0-9_.-]` replaced by `_`, so it is a valid `key=number` key.
pub(crate) fn store_key(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("kb.{}", if clean.is_empty() { "_" } else { &clean })
}

/// SHA-256 (hex) and length of a file, streamed.
pub(crate) fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), bytes))
}
