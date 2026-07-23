//! Practices KB location + auto-registration (issue #370).
//!
//! Ships alongside the binary exactly like the built-in help/manual KB does
//! — `assets/practices/*.org` -> `build-practices-kb` -> `assets/mae-practices.cozo`
//! (`crates/mae/src/bin/build_practices_kb.rs`), installed by `make install`
//! to the same well-known locations `manual_kb.rs` already resolves from.
//!
//! Unlike the manual KB (which is loaded directly into an in-memory store
//! for the help system), the practices KB is registered as a real federated
//! KB instance named [`INSTANCE_NAME`] so `ai_guidance_kb`
//! (`crates/ai/src/guidance.rs`) can find it through the normal
//! `KbRegistry::find` lookup — the same mechanism any contributor's own
//! manually-registered guidance KB would use. Auto-registration is
//! additive-only and idempotent: it never overwrites an existing entry with
//! this name (a contributor may have deliberately repointed or customized
//! it), and it's a silent no-op if no pre-built KB file is found (e.g. a
//! terminal-only install that skipped `manual-kb`/`practices-kb`).

use std::path::{Path, PathBuf};

/// The federated KB instance name auto-registration uses, and the value
/// the shipped `init.scm` template points `ai_guidance_kb` at by default.
pub const INSTANCE_NAME: &str = "MaePractices";

/// Well-known install locations for the pre-built practices KB, checked in
/// priority order. Mirrors `manual_kb::well_known_paths` exactly (same
/// binary-relative / dev-build-assets / XDG-data / system-path resolution),
/// since this ships through the identical install pipeline.
fn well_known_paths(data_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            paths.push(exe_dir.join("mae-practices.cozo"));
        }
        // Source/dev builds: the prebuilt KB lives at `<workspace>/assets/mae-practices.cozo`.
        for ancestor in exe.ancestors() {
            paths.push(ancestor.join("assets/mae-practices.cozo"));
        }
    }

    paths.push(data_dir.join("mae-practices.cozo"));
    paths.push(PathBuf::from("/usr/share/mae/mae-practices.cozo"));
    paths.push(PathBuf::from("/usr/local/share/mae/mae-practices.cozo"));
    paths.push(PathBuf::from("/opt/homebrew/share/mae/mae-practices.cozo"));
    paths.push(PathBuf::from(
        "/home/linuxbrew/.linuxbrew/share/mae/mae-practices.cozo",
    ));

    paths
}

/// Locate the installed practices KB file, if any. `MAE_PRACTICES_KB_PATH`
/// overrides everything, mirroring `manual_kb`'s `MAE_MANUAL_PATH` convention.
pub fn locate(data_dir: &Path) -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MAE_PRACTICES_KB_PATH") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }
    well_known_paths(data_dir).into_iter().find(|p| p.exists())
}

/// Ensure the federation registry has a [`INSTANCE_NAME`] entry pointing at
/// the installed practices KB, if one is found and no entry with that name
/// already exists. Safe to call on every startup — additive-only, and a
/// no-op if nothing is found or an entry already exists (whether ours from
/// a prior run, or a contributor's own customized one).
///
/// If `locate()` resolved to anything other than this data dir's own
/// canonical copy (`data_dir/mae-practices.cozo`) — i.e. the binary-relative
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
pub fn ensure_registered(data_dir: &Path) {
    let Some(found) = locate(data_dir) else {
        return;
    };
    let canonical = data_dir.join("mae-practices.cozo");
    let path = if found == canonical {
        found
    } else if copy_kb_asset(&found, &canonical).is_ok() {
        canonical
    } else {
        return;
    };
    ensure_registered_with_path(data_dir, path);
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
/// (which — once `assets/mae-practices.cozo` is committed, the same way
/// `assets/mae-manual.cozo` already is — would always resolve to the real
/// checked-in file from within this repo's own test suite, making a
/// "nothing located" scenario otherwise untestable here).
fn ensure_registered_with_path(data_dir: &Path, path: PathBuf) {
    let registry = mae_kb::federation::KbRegistry::load(data_dir);
    if registry.find(INSTANCE_NAME).is_some() {
        return;
    }
    let instance = mae_kb::federation::KbInstance {
        uuid: mae_kb::federation::generate_uuid(),
        name: INSTANCE_NAME.to_string(),
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
    };
    let _ = mae_kb::federation::KbRegistry::update(data_dir, |reg| {
        // Re-check against the freshly-reloaded registry: another mae
        // process may have already added this since we loaded ours above.
        if reg.find(INSTANCE_NAME).is_none() {
            reg.instances.push(instance.clone());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `MAE_PRACTICES_KB_PATH` is process-global; without serializing this
    // one env-var-touching test against itself across parallel runs, a
    // `set_var`/`remove_var` race could corrupt another concurrent instance
    // of it (same hazard, same fix, as `guidance.rs`'s `ENV_LOCK`). The
    // other tests below exercise `ensure_registered_with_path` directly and
    // don't touch the environment at all.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn locate_returns_env_override_when_set() {
        let _lock = ENV_LOCK.lock().unwrap();
        let prev = std::env::var("MAE_PRACTICES_KB_PATH").ok();
        let tmp = tempfile::tempdir().unwrap();
        let kb_path = tmp.path().join("fake-practices.cozo");
        std::fs::write(&kb_path, b"not a real store, just needs to exist").unwrap();
        std::env::set_var("MAE_PRACTICES_KB_PATH", &kb_path);

        assert_eq!(locate(tmp.path()), Some(kb_path));

        match prev {
            Some(v) => std::env::set_var("MAE_PRACTICES_KB_PATH", v),
            None => std::env::remove_var("MAE_PRACTICES_KB_PATH"),
        }
    }

    // The remaining tests exercise `ensure_registered_with_path` directly
    // (bypassing `locate()`'s real filesystem/exe-ancestors resolution
    // entirely) — `ensure_registered` itself would always find the real
    // committed `assets/mae-practices.cozo` from within this repo's own
    // test suite (same as `assets/mae-manual.cozo` already is), making a
    // "nothing located" scenario impossible to construct here, and adding
    // no coverage beyond the trivial `Option` early-return in
    // `ensure_registered` itself.

    #[test]
    fn ensure_registered_with_path_adds_entry_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let kb_path = tmp.path().join("fake-practices.cozo");

        ensure_registered_with_path(tmp.path(), kb_path.clone());

        let registry = mae_kb::federation::KbRegistry::load(tmp.path());
        let inst = registry
            .find(INSTANCE_NAME)
            .expect("entry should have been added");
        assert_eq!(inst.db_path, kb_path);
        assert!(!inst.primary);
        assert!(inst.enabled);
    }

    #[test]
    fn ensure_registered_with_path_never_overwrites_an_existing_entry() {
        let tmp = tempfile::tempdir().unwrap();

        // Simulate a contributor's own pre-existing, differently-pathed entry.
        let custom_path = tmp.path().join("my-own-practices.cozo");
        let _ = mae_kb::federation::KbRegistry::update(tmp.path(), |reg| {
            reg.instances.push(mae_kb::federation::KbInstance {
                uuid: "custom-uuid".to_string(),
                name: INSTANCE_NAME.to_string(),
                org_dir: PathBuf::new(),
                db_path: custom_path.clone(),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
                ai_residency: mae_kb::federation::AiResidency::default(),
            });
        });

        ensure_registered_with_path(tmp.path(), tmp.path().join("fake-practices.cozo"));

        let registry = mae_kb::federation::KbRegistry::load(tmp.path());
        let inst = registry.find(INSTANCE_NAME).unwrap();
        assert_eq!(
            inst.db_path, custom_path,
            "must not clobber a contributor's own existing entry"
        );
    }
}
