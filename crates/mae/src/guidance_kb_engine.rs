//! Locating and opening MAE's bundled **system KBs** (`mae_kb::system_kb`).
//!
//! This module used to *register* those corpora as rows in
//! `kb-registry.toml`, driven by a per-KB `BundledGuidanceKb` descriptor that
//! `practices_kb.rs` and `devpractices_kb.rs` each instantiated. Both of those
//! are gone: the descriptors are now the compile-time catalog
//! (`mae_kb::system_kb::SYSTEM_KBS`), and a system KB no longer appears in the
//! registry at all.
//!
//! Registering them there was the root of a family of bugs. Once a bundled
//! corpus is a row next to the user's own KBs, every operation that iterates
//! the registry treats it as user data — `kb_share` could publish it (stripping
//! the `Seed` provenance on the receiving peer, since `NodeSource` is not in
//! the CRDT wire payload), `kb_reimport` could empty it, `kb_health` reported
//! its upstream link hygiene as the user's problem, and a second registration
//! under the same name silently produced a shadowed duplicate row.
//!
//! ADR-076 D4's "your own registration under the same name always wins" is
//! superseded rather than dropped. It was only ever true if you happened to
//! register *before* startup auto-registration ran, because `KbRegistry`
//! deduplicated on `org_dir` and never on name. The override survives in a form
//! that actually holds: register under your own name and point `ai_guidance_kb`
//! at it — an explicit choice, order-independent.
//!
//! @stability: experimental

use mae_kb::system_kb::SystemKb;
use std::path::{Path, PathBuf};

/// Well-known install locations for `kb`'s pre-built asset, checked in
/// priority order. Mirrors `manual_kb::well_known_paths` exactly (same
/// binary-relative / dev-build-assets / XDG-data / system-path resolution),
/// since every bundled KB ships through the identical install pipeline.
fn well_known_paths(kb: &SystemKb, data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            paths.push(exe_dir.join(kb.asset_filename));
        }
        // Source/dev builds: the prebuilt KB lives at `<workspace>/assets/<asset_filename>`.
        for ancestor in exe.ancestors() {
            paths.push(ancestor.join("assets").join(kb.asset_filename));
        }
    }

    paths.push(data_dir.join(kb.asset_filename));
    paths.push(PathBuf::from("/usr/share/mae").join(kb.asset_filename));
    paths.push(PathBuf::from("/usr/local/share/mae").join(kb.asset_filename));
    paths.push(PathBuf::from("/opt/homebrew/share/mae").join(kb.asset_filename));
    paths.push(PathBuf::from("/home/linuxbrew/.linuxbrew/share/mae").join(kb.asset_filename));

    paths
}

/// Locate `kb`'s installed asset, if any. `kb.env_override` overrides
/// everything, mirroring `manual_kb`'s `MAE_MANUAL_PATH` convention.
pub fn locate(kb: &SystemKb, data_dir: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var(kb.env_override) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    well_known_paths(kb, data_dir)
        .into_iter()
        .find(|p| p.exists())
}

/// Locate `kb`'s asset and ensure MAE has its own copy of it in `data_dir`,
/// returning the path to that copy. `None` if nothing is installed — a silent
/// no-op, which is the shipped behaviour for anyone without the asset.
///
/// **The copy is load-bearing, not tidiness.** The returned path is opened
/// read-write by the caller, and CozoDB migrates a sled store to sqlite in
/// place on first open (`kb_storage_engine` defaults to sqlite). Handing back
/// the located asset directly would rewrite it — hit for real once, leaving
/// `.sled.bak-*` migration debris alongside the then-committed
/// `assets/mae-practices.cozo`. Copying first means MAE mutates only its own
/// derived cache.
///
/// Deliberately stops at the path rather than opening the store itself. The
/// caller opens it through `Editor::kb_open_instance_store`, which honours
/// `kb_storage_engine` and performs the sled->sqlite migration. Opening it here
/// as sled instead cost **six seconds of main-thread stall** at startup — the
/// watchdog caught it — because a bundled sled store is tens of megabytes and
/// sled is the slowest of the three engines (measured in
/// `kb_provisioning_cost`). Same reason the old registry-import path went
/// through that function.
/// Copy an already-located system-KB asset into the canonical data-dir path.
///
/// Takes `found` rather than calling [`locate`] itself, and that split is
/// load-bearing: `locate` reads process env (`kb.env_override`), while system-KB
/// provisioning runs on a background thread. Looking it up inside the thread
/// made the answer depend on when the thread happened to run — a caller that
/// sets those vars around `init_kb_federation` could restore them first, which
/// passed locally and failed on CI. Resolve on the main thread, pass the result
/// here.
///
/// (There was briefly an `ensure_local_copy` wrapper doing the lookup inline.
/// It ended up with no callers, which is the honest signal that the lookup does
/// not belong bundled with the copy.)
pub fn ensure_local_copy_of(kb: &SystemKb, data_dir: &Path, found: PathBuf) -> Option<PathBuf> {
    let canonical = data_dir.join(kb.asset_filename);
    if found == canonical {
        return Some(found);
    }
    if copy_kb_asset(&found, &canonical).is_ok() {
        Some(canonical)
    } else {
        None
    }
}

/// Copy a (possibly directory-based, e.g. sled) KB asset from `src` to
/// `dst`, unless `dst` already exists (an earlier session/run already
/// copied it — don't redo the work every startup).
fn copy_kb_asset(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.exists() {
        return Ok(());
    }
    if src.is_dir() {
        copy_dir_all(src, dst)
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

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

/// Remove `kb-registry.toml` rows that MAE itself wrote for a system KB, now
/// that system KBs are served from the catalog instead. Returns
/// `(removed, kept)` — the names evicted, and the reserved names left alone
/// because they turned out to be the user's own content.
///
/// Runs on every startup rather than behind a one-shot marker: it is cheap and
/// idempotent, and a marker would be a second piece of state to get wrong.
///
/// Classification is [`mae_kb::system_kb::classify_reserved_row`] — keyed on
/// the row's *shape*, deliberately not on `KbInstanceKind`. See that function
/// for why `kind` cannot be trusted here.
pub fn evict_system_rows_from_registry(data_dir: &Path) -> (Vec<String>, Vec<String>) {
    use mae_kb::system_kb::{classify_reserved_row, ReservedRow};

    let mut removed = Vec::new();
    let mut kept = Vec::new();

    let (_registry, (), saved) = mae_kb::federation::KbRegistry::update(data_dir, |reg| {
        removed.clear();
        kept.clear();
        reg.instances.retain(|inst| {
            match classify_reserved_row(&inst.name, &inst.org_dir, &inst.db_path, data_dir) {
                Some(ReservedRow::MaeProvisioned) => {
                    removed.push(inst.name.clone());
                    false
                }
                Some(ReservedRow::UserOwned) => {
                    kept.push(inst.name.clone());
                    true
                }
                // Not a system name at all — the user's KB, untouched.
                None => true,
            }
        });
    });
    if let Err(e) = saved {
        tracing::warn!(error = %e, "failed to persist registry after system-KB eviction");
    }

    for name in &removed {
        tracing::info!(kb = %name, "removed MAE-provisioned system KB row from kb-registry.toml");
    }
    for name in &kept {
        tracing::warn!(
            kb = %name,
            "a KB of yours is registered under a reserved MAE system-KB name; MAE's own \
             corpus of that name takes precedence. Re-register yours under a different \
             name and point `ai_guidance_kb` at it."
        );
    }
    (removed, kept)
}
