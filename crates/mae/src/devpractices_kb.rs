//! DevPractices KB location + auto-registration (issue #514, ADR-076).
//!
//! The generic sibling of `practices_kb.rs`'s MaePractices: curated
//! developer-guidance content for anyone using MAE as their editor to build
//! *other* software, not MAE itself (forked from `~/Projects/dev-practices-kb`
//! into `assets/devpractices/*.org` -> `build-devpractices-kb` ->
//! `assets/mae-devpractices.cozo`, per ADR-076 D2/D3). Since ADR-076 D6, this
//! is the default `ai_guidance_kb` in the shipped `init.scm` template for
//! fresh installs; MaePractices remains auto-registered and available
//! alongside it for MAE contributors.
//!
//! This module shares its entire location/copy/register engine with
//! `practices_kb.rs` via [`crate::guidance_kb_engine`] rather than
//! reimplementing it — see ADR-076 D4. Auto-registration is additive-only
//! and idempotent: it never overwrites an existing entry with this name (a
//! user's own manually-registered `DevPractices` KB always wins), and it's a
//! silent no-op if no pre-built KB file is found.

use crate::guidance_kb_engine::{self, BundledGuidanceKb};

/// The federated KB instance name auto-registration uses.
pub const INSTANCE_NAME: &str = "DevPractices";

const DESCRIPTOR: BundledGuidanceKb = BundledGuidanceKb {
    instance_name: INSTANCE_NAME,
    asset_filename: "mae-devpractices.cozo",
    env_override: "MAE_DEVPRACTICES_KB_PATH",
};

/// Ensure the federation registry has a [`INSTANCE_NAME`] entry pointing at
/// the installed DevPractices KB, if one is found and no entry with that
/// name already exists. Safe to call on every startup — additive-only, and
/// a no-op if nothing is found or an entry already exists.
pub fn ensure_registered(data_dir: &std::path::Path) {
    guidance_kb_engine::ensure_registered(&DESCRIPTOR, data_dir);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // `MAE_DEVPRACTICES_KB_PATH` is process-global; serialize this one
    // env-var-touching test against itself across parallel runs (same
    // hazard/fix as `practices_kb`'s `ENV_LOCK`).

    #[test]
    fn locate_returns_env_override_when_set() {
        let _lock = mae_effect_sandbox::lock_env();
        let prev = std::env::var("MAE_DEVPRACTICES_KB_PATH").ok();
        let tmp = tempfile::tempdir().unwrap();
        let kb_path = tmp.path().join("fake-devpractices.cozo");
        std::fs::write(&kb_path, b"not a real store, just needs to exist").unwrap();
        std::env::set_var("MAE_DEVPRACTICES_KB_PATH", &kb_path);

        assert_eq!(
            guidance_kb_engine::locate(&DESCRIPTOR, tmp.path()),
            Some(kb_path)
        );

        match prev {
            Some(v) => std::env::set_var("MAE_DEVPRACTICES_KB_PATH", v),
            None => std::env::remove_var("MAE_DEVPRACTICES_KB_PATH"),
        }
    }

    // The remaining tests exercise `guidance_kb_engine::ensure_registered_with_path`
    // directly (bypassing `locate()`'s real filesystem/exe-ancestors
    // resolution entirely) — same rationale as `practices_kb`'s equivalent
    // tests: once `assets/mae-devpractices.cozo` is committed,
    // `ensure_registered` would always find the real file from within this
    // repo's own test suite.

    #[test]
    fn ensure_registered_with_path_adds_entry_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let kb_path = tmp.path().join("fake-devpractices.cozo");

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

        // Simulate a user's own pre-existing, differently-pathed entry —
        // ADR-076 D4: this must always win over the bundled default.
        let custom_path = tmp.path().join("my-own-devpractices.cozo");
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
            tmp.path().join("fake-devpractices.cozo"),
        );

        let registry = mae_kb::federation::KbRegistry::load(tmp.path());
        let inst = registry.find(INSTANCE_NAME).unwrap();
        assert_eq!(
            inst.db_path, custom_path,
            "must not clobber a user's own existing entry"
        );
    }
}
