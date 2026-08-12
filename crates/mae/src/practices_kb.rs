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
//!
//! This module is now a thin instantiation of the shared
//! [`crate::guidance_kb_engine`] (generalized for ADR-076 / issue #514's
//! DevPractices sibling, `devpractices_kb.rs`) — all location/copy/register
//! logic lives there, not here.

use crate::guidance_kb_engine::{self, BundledGuidanceKb};

/// The federated KB instance name auto-registration uses, and the value
/// the shipped `init.scm` template points `ai_guidance_kb` at by default.
pub const INSTANCE_NAME: &str = "MaePractices";

const DESCRIPTOR: BundledGuidanceKb = BundledGuidanceKb {
    instance_name: INSTANCE_NAME,
    asset_filename: "mae-practices.cozo",
    env_override: "MAE_PRACTICES_KB_PATH",
};

/// Ensure the federation registry has a [`INSTANCE_NAME`] entry pointing at
/// the installed practices KB, if one is found and no entry with that name
/// already exists. Safe to call on every startup — additive-only, and a
/// no-op if nothing is found or an entry already exists (whether ours from
/// a prior run, or a contributor's own customized one). See
/// `guidance_kb_engine::ensure_registered` for the full rationale (copy-first
/// into the canonical data-dir location, never opening a git-tracked source
/// asset live).
pub fn ensure_registered(data_dir: &std::path::Path) {
    guidance_kb_engine::ensure_registered(&DESCRIPTOR, data_dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // `MAE_PRACTICES_KB_PATH` is process-global; without serializing this
    // one env-var-touching test against itself across parallel runs, a
    // `set_var`/`remove_var` race could corrupt another concurrent instance
    // of it (same hazard, same fix, as `guidance.rs`'s `ENV_LOCK`). The
    // other tests below exercise `guidance_kb_engine::ensure_registered_with_path`
    // directly and don't touch the environment at all.

    #[test]
    fn locate_returns_env_override_when_set() {
        let _lock = mae_effect_sandbox::lock_env();
        let prev = std::env::var("MAE_PRACTICES_KB_PATH").ok();
        let tmp = tempfile::tempdir().unwrap();
        let kb_path = tmp.path().join("fake-practices.cozo");
        std::fs::write(&kb_path, b"not a real store, just needs to exist").unwrap();
        std::env::set_var("MAE_PRACTICES_KB_PATH", &kb_path);

        assert_eq!(
            guidance_kb_engine::locate(&DESCRIPTOR, tmp.path()),
            Some(kb_path)
        );

        match prev {
            Some(v) => std::env::set_var("MAE_PRACTICES_KB_PATH", v),
            None => std::env::remove_var("MAE_PRACTICES_KB_PATH"),
        }
    }

    // The remaining tests exercise `guidance_kb_engine::ensure_registered_with_path`
    // directly (bypassing `locate()`'s real filesystem/exe-ancestors
    // resolution entirely) — on any machine that has run `make practices-kb`,
    // `ensure_registered` finds the built `assets/mae-practices.cozo` via
    // `locate()`'s exe-ancestor probe, so a "nothing located" scenario is not
    // reliably constructible here and would add no coverage beyond the trivial
    // `Option` early-return in `ensure_registered` itself.
    //
    // Note the asymmetry that implies: whether `locate()` finds anything
    // depends on whether that `make` target has been run, so these tests
    // deliberately do not exercise it. The tests that DO want real content
    // build it from the tracked `assets/practices/*.org` corpus instead —
    // see `bootstrap.rs::build_real_guidance_kb`.

    #[test]
    fn ensure_registered_with_path_adds_entry_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let kb_path = tmp.path().join("fake-practices.cozo");

        guidance_kb_engine::ensure_registered_with_path(&DESCRIPTOR, tmp.path(), kb_path.clone());

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
                project_root: None,
                kind: mae_kb::federation::KbInstanceKind::default(),
                priority: 0,
                remote_hub: None,
            });
        });

        guidance_kb_engine::ensure_registered_with_path(
            &DESCRIPTOR,
            tmp.path(),
            tmp.path().join("fake-practices.cozo"),
        );

        let registry = mae_kb::federation::KbRegistry::load(tmp.path());
        let inst = registry.find(INSTANCE_NAME).unwrap();
        assert_eq!(
            inst.db_path, custom_path,
            "must not clobber a contributor's own existing entry"
        );
    }
}
