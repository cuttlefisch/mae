//! Shared engine for auto-registering a bundled guidance KB at startup.
//!
//! Extracted from `practices_kb.rs` (issue #370) when a second bundled
//! guidance KB — DevPractices (issue #514, ADR-076 D4) — needed the exact
//! same location/copy/register logic. `practices_kb.rs` and
//! `devpractices_kb.rs` are both thin instantiations of this engine via a
//! [`BundledGuidanceKb`] descriptor; nothing here is MaePractices- or
//! DevPractices-specific, and neither of those modules reimplements any of
//! this.
//!
//! See ADR-076 for the full design rationale. In particular, the "your own
//! registration under the same name always wins" behavior baked into
//! [`ensure_registered_with_path`] is a documented, load-bearing property
//! (ADR-076 D4's Consequences), not incidental: a contributor who wants to
//! override a shipped default guidance KB with their own live copy does so
//! simply by registering something under the same name first — no override
//! flag or special config needed.

use std::path::{Path, PathBuf};

/// Describes one bundled, auto-registered guidance KB: the federated
/// instance name it registers under, the shipped asset's filename (resolved
/// against the same well-known set of install locations every bundled KB
/// uses), and the env var that overrides that resolution.
pub struct BundledGuidanceKb {
    /// The federated KB instance name auto-registration uses.
    pub instance_name: &'static str,
    /// The shipped asset's filename, e.g. `"mae-practices.cozo"`. Joined
    /// onto each well-known install location in turn.
    pub asset_filename: &'static str,
    /// Env var that overrides all well-known-path resolution, e.g.
    /// `"MAE_PRACTICES_KB_PATH"`.
    pub env_override: &'static str,
}

/// Well-known install locations for `descriptor`'s pre-built KB asset,
/// checked in priority order. Mirrors `manual_kb::well_known_paths` exactly
/// (same binary-relative / dev-build-assets / XDG-data / system-path
/// resolution), since every bundled KB ships through the identical install
/// pipeline.
fn well_known_paths(descriptor: &BundledGuidanceKb, data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            paths.push(exe_dir.join(descriptor.asset_filename));
        }
        // Source/dev builds: the prebuilt KB lives at `<workspace>/assets/<asset_filename>`.
        for ancestor in exe.ancestors() {
            paths.push(ancestor.join("assets").join(descriptor.asset_filename));
        }
    }

    paths.push(data_dir.join(descriptor.asset_filename));
    paths.push(PathBuf::from("/usr/share/mae").join(descriptor.asset_filename));
    paths.push(PathBuf::from("/usr/local/share/mae").join(descriptor.asset_filename));
    paths.push(PathBuf::from("/opt/homebrew/share/mae").join(descriptor.asset_filename));
    paths.push(
        PathBuf::from("/home/linuxbrew/.linuxbrew/share/mae").join(descriptor.asset_filename),
    );

    paths
}

/// Locate `descriptor`'s installed KB asset, if any. `descriptor.env_override`
/// overrides everything, mirroring `manual_kb`'s `MAE_MANUAL_PATH` convention.
pub fn locate(descriptor: &BundledGuidanceKb, data_dir: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var(descriptor.env_override) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    well_known_paths(descriptor, data_dir)
        .into_iter()
        .find(|p| p.exists())
}

/// Ensure the federation registry has a `descriptor.instance_name` entry
/// pointing at the installed KB asset, if one is found and no entry with
/// that name already exists. Safe to call on every startup — additive-only,
/// and a no-op if nothing is found or an entry already exists (whether ours
/// from a prior run, or a contributor's own customized one).
///
/// If `locate()` resolved to anything other than this data dir's own
/// canonical copy (`data_dir/<asset_filename>`) — i.e. the binary-relative
/// or dev-checkout `assets/` fallback in `well_known_paths` — copies it into
/// that canonical location FIRST and registers the copy instead. This is
/// NOT the same read-only precaution `manual_kb.rs` takes (that one loads
/// nodes into an in-memory store and never opens the source file live
/// again): a federated instance's `db_path` gets opened LIVE — and
/// potentially sled->sqlite migrated in place — every time
/// `init_kb_federation` imports it. Registering a git-tracked source asset
/// directly would let any dev/test run in this checkout silently mutate it
/// (hit for real once already: an early version of this auto-registration
/// path did exactly that, leaving `.sled.bak-*` migration debris alongside
/// the committed `assets/mae-practices.cozo`).
pub fn ensure_registered(descriptor: &BundledGuidanceKb, data_dir: &Path) {
    let Some(found) = locate(descriptor, data_dir) else {
        return;
    };
    let canonical = data_dir.join(descriptor.asset_filename);
    let path = if found == canonical {
        found
    } else if copy_kb_asset(&found, &canonical).is_ok() {
        canonical
    } else {
        return;
    };
    ensure_registered_with_path(descriptor, data_dir, path);
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

/// The registry-mutation half of [`ensure_registered`], with the located KB
/// path passed in directly rather than resolved from real filesystem/env
/// state. Split out so tests can exercise the additive/no-overwrite
/// invariants without depending on `locate()`'s exe-relative fallback paths
/// (which — once a KB's asset is committed under `assets/`, the same way
/// `assets/mae-manual.cozo` already is — would always resolve to the real
/// checked-in file from within this repo's own test suite, making a
/// "nothing located" scenario otherwise untestable here).
pub fn ensure_registered_with_path(descriptor: &BundledGuidanceKb, data_dir: &Path, path: PathBuf) {
    let registry = mae_kb::federation::KbRegistry::load(data_dir);
    if registry.find(descriptor.instance_name).is_some() {
        return;
    }
    let instance = mae_kb::federation::KbInstance {
        uuid: mae_kb::federation::generate_uuid(),
        name: descriptor.instance_name.to_string(),
        org_dir: PathBuf::new(),
        db_path: path,
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: None,
        shared: false,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: mae_kb::federation::AiResidency::default(),
        project_root: None,
        kind: mae_kb::federation::KbInstanceKind::Guidance,
        priority: 0,
        remote_hub: None,
    };
    let _ = mae_kb::federation::KbRegistry::update(data_dir, |reg| {
        // Re-check against the freshly-reloaded registry: another mae
        // process may have already added this since we loaded ours above.
        if reg.find(descriptor.instance_name).is_none() {
            reg.instances.push(instance.clone());
        }
    });
}
