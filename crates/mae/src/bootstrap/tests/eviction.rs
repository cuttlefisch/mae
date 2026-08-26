//! Bootstrap tests: evicting MAE-provisioned rows from `kb-registry.toml`.
//!
//! Split out of `kb_federation.rs` when the guidance-delivery oracle pushed
//! that file past the 500-line test ceiling. Eviction is a coherent subject of
//! its own — the one-time migration off the pre-ADR-104 registry layout — so it
//! splits here rather than the federation tests being carved arbitrarily.
//!
//! No `use super::super::*` here, unlike its sibling modules: these tests reach
//! `guidance_kb_engine::evict_system_rows_from_registry` by its full crate path
//! and touch nothing else private to `bootstrap`.

/// The migration: a registry that already carries MAE-provisioned rows —
/// the state of any machine that has run MAE before this change — is
/// cleaned up, while the user's own KBs are left exactly alone.
///
/// The `kind` values here are the ones observed on a real long-running
/// install: `MaePractices` stamped `UserRegistered` and `DevPractices`
/// `Guidance`, though MAE wrote both. That is precisely why the classifier
/// keys on shape instead.
#[test]
fn init_kb_federation_evicts_mae_provisioned_rows_and_keeps_the_users_own() {
    let _lock = mae_effect_sandbox::lock_env();
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path();

    let row = |name: &str, org_dir: std::path::PathBuf, db_path: std::path::PathBuf, kind| {
        mae_kb::federation::KbInstance {
            uuid: mae_kb::federation::generate_uuid(),
            name: name.to_string(),
            org_dir,
            db_path,
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
            project_root: None,
            project_key: None,
            kind,
            ingest_policy: Default::default(),
            priority: 0,
            remote_hub: None,
        }
    };

    let user_dir = tempfile::tempdir().unwrap();
    let mut registry = mae_kb::federation::KbRegistry::default();
    registry.instances.push(row(
        "MaePractices",
        std::path::PathBuf::new(),
        data_dir.join("mae-practices.cozo"),
        mae_kb::federation::KbInstanceKind::UserRegistered,
    ));
    registry.instances.push(row(
        "DevPractices",
        std::path::PathBuf::new(),
        data_dir.join("mae-devpractices.cozo"),
        mae_kb::federation::KbInstanceKind::Guidance,
    ));
    registry.instances.push(row(
        "MyNotes",
        user_dir.path().to_path_buf(),
        data_dir.join("kb/local/mynotes/kb.sqlite"),
        mae_kb::federation::KbInstanceKind::UserRegistered,
    ));
    registry.save(data_dir).unwrap();

    let (removed, kept) = crate::guidance_kb_engine::evict_system_rows_from_registry(data_dir);

    assert_eq!(removed.len(), 2, "both MAE-provisioned rows: {removed:?}");
    assert!(
        kept.is_empty(),
        "no reserved-name row was the user's: {kept:?}"
    );

    let after = mae_kb::federation::KbRegistry::load(data_dir);
    assert!(after.find("MaePractices").is_none());
    assert!(after.find("DevPractices").is_none());
    assert!(
        after.find("MyNotes").is_some(),
        "the user's own KB must survive untouched"
    );
    assert_eq!(after.instances.len(), 1);

    // Idempotent: a second pass removes nothing and still leaves the user's.
    let (again, _) = crate::guidance_kb_engine::evict_system_rows_from_registry(data_dir);
    assert!(again.is_empty(), "second pass must be a no-op: {again:?}");
    assert_eq!(
        mae_kb::federation::KbRegistry::load(data_dir)
            .instances
            .len(),
        1
    );
}

/// The half that stops the migration being a licence to delete: a reserved
/// name pointing at the user's OWN org directory is their content, and the
/// only record of where it lives. It is kept, and reported.
#[test]
fn eviction_never_removes_a_reserved_name_that_holds_the_users_own_content() {
    let _lock = mae_effect_sandbox::lock_env();
    let tmp = tempfile::tempdir().unwrap();
    let user_dir = tempfile::tempdir().unwrap();

    let mut registry = mae_kb::federation::KbRegistry::default();
    registry.instances.push(mae_kb::federation::KbInstance {
        uuid: mae_kb::federation::generate_uuid(),
        name: "DevPractices".to_string(),
        org_dir: user_dir.path().to_path_buf(),
        db_path: tmp.path().join("kb/local/mine/kb.sqlite"),
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: None,
        shared: false,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: mae_kb::federation::AiResidency::default(),
        project_root: None,
        project_key: None,
        kind: mae_kb::federation::KbInstanceKind::UserRegistered,
        ingest_policy: Default::default(),
        priority: 0,
        remote_hub: None,
    });
    registry.save(tmp.path()).unwrap();

    let (removed, kept) = crate::guidance_kb_engine::evict_system_rows_from_registry(tmp.path());

    assert!(
        removed.is_empty(),
        "must not delete the user's content: {removed:?}"
    );
    assert_eq!(kept, vec!["DevPractices".to_string()]);
    assert!(mae_kb::federation::KbRegistry::load(tmp.path())
        .find("DevPractices")
        .is_some());
}
