//! Locating a pre-built manual KB file.
//!
//! # What used to be here
//!
//! This module also carried checksum "validation" — `KNOWN_CHECKSUMS`,
//! `ManualValidation::{Valid, Historical, Unknown}`, and a `validate_checksum`
//! consulted at every startup. All of it is gone, because none of it did
//! anything: `KNOWN_CHECKSUMS` was an empty array ("populated by the release
//! process", which never happened), so `validate_checksum` returned `Valid`
//! unconditionally and the `Historical`/`Unknown` arms were unreachable. A
//! security-shaped mechanism that always says yes is worse than none, because
//! `install.sh` advertised it ("SHA-256 checksum stored (validated at
//! runtime)").
//!
//! It could not have been made to work in that form either. A sled store is
//! rewritten in place the first time it is opened and is **not byte-reproducible**
//! (`release.yml` records measured hashes differing between a fresh build and
//! the committed sidecar), so an installed store can never be checksum-verified
//! against a fixed constant. Download integrity is covered where it actually
//! works: `SHA256SUMS` over the release tarball.
//!
//! # What is left
//!
//! Finding a pre-built store, for `mae upgrade`'s preflight report. The manual's
//! own content no longer comes from here — `bootstrap` builds a version-keyed
//! projection from the corpus instead.

use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// Locate a pre-built manual KB, if one is installed.
///
/// Resolution: `$MAE_MANUAL_PATH`, then the `manual_kb_path` config override,
/// then the well-known install locations.
pub fn locate(data_dir: &Path, manual_kb_path_override: Option<&str>) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MAE_MANUAL_PATH") {
        let path = PathBuf::from(path);
        if path.exists() {
            debug!(path = %path.display(), "using manual KB from MAE_MANUAL_PATH");
            return Some(path);
        }
        warn!(path = %path.display(), "MAE_MANUAL_PATH set but file not found");
    }

    if let Some(cfg_path) = manual_kb_path_override {
        if !cfg_path.is_empty() {
            let path = PathBuf::from(cfg_path);
            if path.exists() {
                debug!(path = %path.display(), "using manual KB from config");
                return Some(path);
            }
            warn!(path = %path.display(), "manual_kb_path configured but file not found");
        }
    }

    let found = well_known_paths(data_dir)
        .into_iter()
        .find(|candidate| candidate.exists());
    if found.is_none() {
        debug!("no pre-built manual KB found");
    }
    found
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
