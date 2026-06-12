//! Manual KB location, validation, and loading.
//!
//! The manual KB is a pre-built CozoDB file containing the full mae manual
//! (~400+ seed nodes). It ships alongside the binary and provides instant
//! AI context on first launch.
//!
//! Resolution order:
//! 1. `$MAE_MANUAL_PATH` env var
//! 2. Config option `manual_kb_path`
//! 3. Well-known paths: `{exe_dir}/mae-manual.cozo`, `{data_dir}/mae-manual.cozo`
//! 4. Fallback: build from seed at runtime (current behavior, slower)

use mae_kb::KbStore;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Result of locating and validating a manual KB file.
pub struct ManualKbResult {
    pub path: PathBuf,
    pub validation: ManualValidation,
}

/// Validation status of a manual KB file.
#[derive(Debug)]
pub enum ManualValidation {
    /// Checksum matches current release version.
    Valid,
    /// Checksum matches a historical release (usable but outdated).
    Historical { matched_version: String },
    /// Checksum matches no known release (untrusted).
    Unknown,
    /// User-provided custom manual (no checksum validation).
    Custom,
}

/// Known SHA-256 checksums of official mae-manual.cozo releases.
/// Updated at release time by CI. Newest first.
///
/// Note: sled-backed CozoDB stores are directories, so the checksum
/// is computed over all files sorted by relative path (see `compute_db_checksum`).
const KNOWN_CHECKSUMS: &[(&str, &str)] = &[
    // Checksums will be populated by the release process.
    // Format: ("version", "sha256hex")
];

/// Locate and validate the manual KB.
pub fn locate_and_validate(
    data_dir: &Path,
    manual_kb_path_override: Option<&str>,
) -> Option<ManualKbResult> {
    // 1. Explicit override via env var.
    if let Ok(path) = std::env::var("MAE_MANUAL_PATH") {
        let path = PathBuf::from(path);
        if path.exists() {
            info!(path = %path.display(), "using manual KB from MAE_MANUAL_PATH");
            return Some(ManualKbResult {
                path,
                validation: ManualValidation::Custom,
            });
        }
        warn!(path = %path.display(), "MAE_MANUAL_PATH set but file not found");
    }

    // 2. Config option override.
    if let Some(cfg_path) = manual_kb_path_override {
        if !cfg_path.is_empty() {
            let path = PathBuf::from(cfg_path);
            if path.exists() {
                info!(path = %path.display(), "using manual KB from config");
                return Some(ManualKbResult {
                    path,
                    validation: ManualValidation::Custom,
                });
            }
            warn!(path = %path.display(), "manual_kb_path configured but file not found");
        }
    }

    // 3. Well-known paths.
    let candidates = well_known_paths(data_dir);
    for candidate in &candidates {
        if candidate.exists() {
            debug!(path = %candidate.display(), "found manual KB at well-known path");
            let checksum = compute_db_checksum(candidate);
            let validation = validate_checksum(&checksum);
            return Some(ManualKbResult {
                path: candidate.clone(),
                validation,
            });
        }
    }

    // 4. Not found — caller should fall back to seed_kb().
    debug!("no pre-built manual KB found; will seed at runtime");
    None
}

/// Recursively copy a directory tree (used to stage a throwaway manual-KB copy).
fn copy_dir_all(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Read all nodes from a pre-built manual KB **without mutating the source**.
///
/// sled (CozoDB's backend) always opens read-write and writes recovery
/// snapshots on open, which would dirty a git-tracked asset or drift an
/// installed file's checksum. We copy the store into a throwaway temp dir,
/// read from the copy, then discard it — leaving the original untouched.
pub fn load_nodes_readonly(path: &Path) -> Result<Vec<mae_kb::Node>, String> {
    let tmp = std::env::temp_dir().join(format!("mae-manual-ro-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    copy_dir_all(path, &tmp).map_err(|e| format!("staging manual KB copy: {e}"))?;
    let result = (|| {
        let store = mae_kb::CozoKbStore::open(&tmp).map_err(|e| e.to_string())?;
        store.load_all().map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Well-known paths where the manual KB might be found.
fn well_known_paths(data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    // Next to the binary.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            paths.push(exe_dir.join("mae-manual.cozo"));
        }
        // Source/dev builds: the prebuilt KB lives at `<workspace>/assets/mae-manual.cozo`.
        // Walk up from the binary (e.g. `target/release/mae`) and probe each ancestor's
        // `assets/` dir so `cargo build` / `make build` runs find it without `make install`.
        for ancestor in exe.ancestors() {
            paths.push(ancestor.join("assets/mae-manual.cozo"));
        }
    }

    // XDG data dir.
    paths.push(data_dir.join("mae-manual.cozo"));

    // System-wide (Linux).
    paths.push(PathBuf::from("/usr/share/mae/mae-manual.cozo"));
    paths.push(PathBuf::from("/usr/local/share/mae/mae-manual.cozo"));

    // Homebrew (macOS Apple Silicon).
    paths.push(PathBuf::from("/opt/homebrew/share/mae/mae-manual.cozo"));
    // Homebrew (macOS Intel / Linux Homebrew).
    paths.push(PathBuf::from(
        "/home/linuxbrew/.linuxbrew/share/mae/mae-manual.cozo",
    ));

    paths
}

/// Validate a checksum against known releases.
fn validate_checksum(checksum: &str) -> ManualValidation {
    let current_version = env!("CARGO_PKG_VERSION");

    for (version, known_hash) in KNOWN_CHECKSUMS {
        if *known_hash == checksum {
            if *version == current_version {
                return ManualValidation::Valid;
            }
            return ManualValidation::Historical {
                matched_version: version.to_string(),
            };
        }
    }

    // No match — could be a dev build or tampered file.
    // In dev builds (no checksums populated), treat as valid.
    if KNOWN_CHECKSUMS.is_empty() {
        return ManualValidation::Valid;
    }

    ManualValidation::Unknown
}

/// Compute a SHA-256 checksum for the CozoDB store.
///
/// For sled (directory-based), hashes all files sorted by relative path.
/// For single-file backends, hashes the file directly.
pub fn compute_db_checksum(path: &Path) -> String {
    let mut hasher = Sha256::new();

    if path.is_dir() {
        let mut files = Vec::new();
        collect_files_recursive(path, &mut files);
        files.sort();
        for file in &files {
            let rel = file.strip_prefix(path).unwrap_or(file);
            hasher.update(rel.to_string_lossy().as_bytes());
            if let Ok(data) = std::fs::read(file) {
                hasher.update(&data);
            }
        }
    } else if let Ok(data) = std::fs::read(path) {
        hasher.update(&data);
    }

    format!("{:x}", hasher.finalize())
}

fn collect_files_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_files_recursive(&path, out);
            } else {
                out.push(path);
            }
        }
    }
}
