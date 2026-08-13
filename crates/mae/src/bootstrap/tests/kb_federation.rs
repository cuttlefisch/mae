//! Bootstrap tests: `init_kb_federation` — system-KB catalog serving, guidance provisioning,
//! registry eviction, and the failure paths that must surface to the user.

use super::super::*;

/// #79 third slice: a primary-KB-store-open failure used to be a clobberable
/// status-line message fired once during the startup burst — easy to miss, yet
/// every subsequent KB edit is silently discarded until the user notices. Must
/// land as a durable notification. Uses a REAL failure (a garbage regular file
/// where a valid CozoDB store is expected), not a synthetic one.
#[test]
#[cfg(unix)]
fn init_kb_federation_notifies_on_a_real_store_open_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let kb_dir = tmp.path().join("kb");
    std::fs::create_dir_all(&kb_dir).unwrap();
    // primary.cozo as an unreadable regular file: not a directory (so the
    // sled->sqlite migration check short-circuits to NotNeeded) and permission
    // denied at the OS level — CozoKbStore::open_with_engine must fail on it for
    // real, without going through cozo's own file-format parsing (which panics
    // internally on garbage content rather than returning a clean Err).
    let cozo_path = kb_dir.join("primary.cozo");
    std::fs::write(&cozo_path, b"").unwrap();
    std::fs::set_permissions(&cozo_path, std::fs::Permissions::from_mode(0o000)).unwrap();

    let mut editor = mae_core::Editor::new();
    editor.data_dir_override = Some(tmp.path().to_path_buf());

    init_kb_federation(&mut editor, false);

    // Restore permissions so tempdir cleanup can remove the file.
    std::fs::set_permissions(&cozo_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        editor.kb.store_unavailable,
        "a real store-open failure must flag store_unavailable"
    );
    let notes = editor.notifications.active_sorted();
    let hit = notes
        .iter()
        .find(|n| n.source == "kb" && n.title.contains("KB store unavailable"));
    assert!(
        hit.is_some(),
        "a durable notification must be raised for a real store-open failure, \
         not just a status-line toast; got: {:?}",
        notes.iter().map(|n| &n.title).collect::<Vec<_>>()
    );
    assert_eq!(
        hit.unwrap().severity,
        mae_core::notifications::Severity::Error
    );
}

/// #79 third slice: a sled->sqlite migration failure used to be a clobberable
/// status-line message only. Uses a REAL failure — a `primary.cozo` directory
/// that LOOKS like a legacy sled store (triggers the migration attempt) but
/// contains no valid sled database, so the migration's own open genuinely fails.
#[test]
#[cfg(unix)]
fn init_kb_federation_notifies_on_a_real_migration_failure() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().unwrap();
    let kb_dir = tmp.path().join("kb");
    // primary.cozo as a DIRECTORY (looks like a legacy sled store, so
    // migrate_sled_to_sqlite attempts the migration) but permission-denied —
    // the migration's own sled open must fail for real, without relying on
    // sabotaging sled's on-disk format (undocumented, fragile to depend on).
    let cozo_dir = kb_dir.join("primary.cozo");
    std::fs::create_dir_all(&cozo_dir).unwrap();
    std::fs::set_permissions(&cozo_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    let mut editor = mae_core::Editor::new();
    editor.data_dir_override = Some(tmp.path().to_path_buf());
    assert_eq!(
        editor.kb.storage_engine, "sqlite",
        "default engine must be sqlite for the migration path to even attempt"
    );

    init_kb_federation(&mut editor, false);

    // Restore permissions so tempdir cleanup can remove the directory.
    std::fs::set_permissions(&cozo_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

    let notes = editor.notifications.active_sorted();
    let hit = notes
        .iter()
        .find(|n| n.source == "kb" && n.title.contains("KB migration failed"));
    assert!(
        hit.is_some(),
        "a durable notification must be raised for a real migration failure, \
         not just a status-line toast; got: {:?}",
        notes.iter().map(|n| &n.title).collect::<Vec<_>>()
    );
    assert_eq!(
        hit.unwrap().severity,
        mae_core::notifications::Severity::Warning
    );
}

/// Build a guidance KB from its REAL tracked org corpus (`assets/practices`
/// or `assets/devpractices`) into a throwaway tempdir, and return that dir
/// plus the store path inside it.
///
/// Replaces a helper that copied the pre-built `assets/mae-*.cozo`
/// artifact. That helper existed only to dodge a hazard: CozoDB (sled in
/// particular) always opens read-write and would migrate/compact a
/// git-tracked asset in place — hit for real once while writing this very
/// test, `.sled.bak-*` debris and all, the moment `init_kb_federation`'s
/// normal import path opened it. Building a fresh sqlite store from the
/// tracked `.org` source removes the hazard instead of tiptoeing around
/// it, works on a clone where the artifact was never built (it is
/// gitignored, and CI's test leg does not build it), and cannot validate a
/// stale artifact from whenever `make practices-kb` last ran.
///
/// Still the REAL shipped content: `assets/practices/*.org` is the tracked
/// source of truth, and this is the same `build_org_kb` the shipped
/// `build-practices-kb` binary calls.
fn build_real_guidance_kb(corpus: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .join(corpus);
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join(format!("mae-{corpus}.cozo"));
    mae_kb::kb_build::build_org_kb(
        &src,
        &db_path,
        &mae_kb::kb_build::OrgKbBuildOptions {
            engine: "sqlite",
            ..Default::default()
        },
    )
    .unwrap_or_else(|e| panic!("failed to build {corpus} KB from {}: {e}", src.display()));
    (tmp, db_path)
}

/// Issue #370 / #514, end-to-end, restated for the system-KB split:
/// `init_kb_federation` must make MAE's own corpora available **without**
/// putting them in `kb-registry.toml`.
///
/// The oracles are deliberately both-sided. Asserting only "it loaded"
/// would still pass if the corpus were auto-registered exactly as before;
/// asserting only "no registry row" would pass if it failed to load at all.
/// Content comes from the tracked `assets/*/**.org` corpora via
/// `build_real_guidance_kb`, which is what the shipped asset is built from.
#[test]
fn init_kb_federation_serves_system_kbs_from_the_catalog_not_the_registry() {
    let _lock = mae_effect_sandbox::lock_env();
    let prev_p = std::env::var("MAE_PRACTICES_KB_PATH").ok();
    let prev_d = std::env::var("MAE_DEVPRACTICES_KB_PATH").ok();

    let (_bp, practices) = build_real_guidance_kb("practices");
    let (_bd, devpractices) = build_real_guidance_kb("devpractices");
    std::env::set_var("MAE_PRACTICES_KB_PATH", &practices);
    std::env::set_var("MAE_DEVPRACTICES_KB_PATH", &devpractices);

    let tmp = tempfile::tempdir().unwrap();
    let mut editor = mae_core::Editor::new();
    editor.data_dir_override = Some(tmp.path().to_path_buf());

    init_kb_federation(&mut editor, false);

    match prev_p {
        Some(v) => std::env::set_var("MAE_PRACTICES_KB_PATH", v),
        None => std::env::remove_var("MAE_PRACTICES_KB_PATH"),
    }
    match prev_d {
        Some(v) => std::env::set_var("MAE_DEVPRACTICES_KB_PATH", v),
        None => std::env::remove_var("MAE_DEVPRACTICES_KB_PATH"),
    }

    for name in ["MaePractices", "DevPractices"] {
        let store = editor
            .kb
            .system_stores
            .get(name)
            .unwrap_or_else(|| panic!("{name} must be served as a system store"));
        assert!(
            mae_kb::KbStore::get_node(store.as_ref(), "index")
                .ok()
                .flatten()
                .is_some(),
            "{name}'s real index node must have loaded"
        );
        assert!(
            editor.kb.registry.find(name).is_none(),
            "{name} must NOT appear in kb-registry.toml — that is what made every \
             registry consumer treat a bundled corpus as the user's data"
        );
    }
}

/// **The invariant.** After `init_kb_federation`, MAE's own documentation is
/// reachable through the query layer — in every mode, from the first tick.
///
/// This is the gate whose absence let a real regression ship. An earlier
/// attempt made the manual a durable cache that was absent until built, and
/// absent forever under `--headless`. Every test still passed, because they
/// all asserted the projection *worked*; none asserted documentation stayed
/// reachable while it did not yet exist. The observable damage was that
/// `kb-find`/`kb-insert-link` contained zero `cmd:`/`concept:`/`option:`
/// nodes, and `kb_list`/`kb_links_to`/`kb_graph` returned empty for MAE's
/// own docs, on first launch and after every upgrade.
///
/// The oracle is deliberately the QUERY LAYER, not `kb.primary`. The query
/// layer's primary is the user's own store; MAE's docs reach it only as the
/// `"manual"` pseudo-instance, so asserting on `kb.primary` would pass even
/// with the manual missing from every federated read.
#[test]
fn mae_documentation_is_reachable_through_the_query_layer_from_the_first_tick() {
    let _lock = mae_effect_sandbox::lock_env();
    let prev_cache = std::env::var("XDG_CACHE_HOME").ok();
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CACHE_HOME", cache.path());

    let tmp = tempfile::tempdir().unwrap();
    let mut editor = mae_core::Editor::new();
    editor.data_dir_override = Some(tmp.path().to_path_buf());

    init_kb_federation(&mut editor, false);

    match prev_cache {
        Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
        None => std::env::remove_var("XDG_CACHE_HOME"),
    }

    assert!(
        editor
            .kb
            .system_stores
            .contains_key(mae_kb::system_kb::MANUAL),
        "the manual store must be present immediately, in every mode"
    );

    let q = editor
        .kb
        .query_layer()
        .expect("a query layer must exist once the manual store is present");

    // A code-generated node and an org-only node: the two halves that must
    // both land. `concept:scheme-api` exists ONLY in `assets/manual/`, so it
    // proves the corpus was ingested rather than just the seed nodes.
    for id in ["index", "cmd:save", "concept:scheme-api"] {
        assert!(
            q.get(id).is_some(),
            "{id} must be reachable through the query layer"
        );
    }

    // ...and the palettes, which is where the regression was user-visible.
    let pairs = editor.kb_all_node_pairs();
    assert!(
        pairs.iter().any(|(id, _)| id == "cmd:save"),
        "kb-find / kb-insert-link candidates must include MAE help nodes"
    );
}

/// The Windows / Docker / `cargo install` case: **no pre-built store
/// installed**, and guidance must still work.
///
/// Those three ship zero KB corpora today, so `ai_guidance_kb`'s shipped
/// default resolves to nothing and the AI peer runs with no standing
/// practices. Building from source is what fixes that, and this asserts the
/// chain end to end: no installed store -> corpus -> built store -> the real
/// `index` node, in the cache rather than among the user's data.
///
/// **What this does NOT cover, verified rather than assumed:** the corpus it
/// builds from resolves ON-DISK here, not from the embedded copy.
/// `system_corpus::resolve` prefers `assets/<corpus>` found by walking the
/// executable's ancestors, and in this checkout the test binary's ancestors
/// genuinely contain it — `current_exe()` is not something a test can
/// redirect. The embedded half is covered separately by
/// `system_corpus::tests::the_embedded_corpus_materialises_into_a_walkable_directory`.
/// Claiming otherwise here would be exactly the "passes for the wrong
/// reason" this test exists to avoid.
#[test]
fn guidance_is_built_from_the_embedded_corpus_when_no_store_is_installed() {
    let _lock = mae_effect_sandbox::lock_env();
    let prev_d = std::env::var("MAE_DEVPRACTICES_KB_PATH").ok();
    let prev_cache = std::env::var("XDG_CACHE_HOME").ok();

    let tmp = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CACHE_HOME", cache.path());
    // A path that does not exist, so `locate()` finds no installed STORE
    // and the build-from-source branch is the only way through.
    std::env::set_var(
        "MAE_DEVPRACTICES_KB_PATH",
        tmp.path().join("definitely-absent.cozo"),
    );

    let kb = mae_kb::system_kb::find("DevPractices").unwrap();
    let built = build_guidance_from_embedded_corpus(kb, tmp.path());

    match prev_d {
        Some(v) => std::env::set_var("MAE_DEVPRACTICES_KB_PATH", v),
        None => std::env::remove_var("MAE_DEVPRACTICES_KB_PATH"),
    }
    match prev_cache {
        Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
        None => std::env::remove_var("XDG_CACHE_HOME"),
    }

    let path = built.expect("must build DevPractices from the embedded corpus");
    assert!(
        path.exists(),
        "the built store must exist at {}",
        path.display()
    );
    assert!(
        path.starts_with(cache.path()),
        "a rebuildable store belongs in cache, not among the user's data: {}",
        path.display()
    );

    let store = mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").expect("open");
    let index = mae_kb::KbStore::get_node(&store, "index")
        .expect("query")
        .expect("the built KB must carry the real index node");
    assert!(
        index.body.contains("DevPractices"),
        "expected the real DevPractices corpus, got: {}",
        index.body
    );
}

/// The oracle the whole guidance mechanism was missing: **what the pipeline
/// builds must be what the reader finds.**
///
/// [`guidance_is_built_from_the_embedded_corpus_when_no_store_is_installed`]
/// above proves the store gets built, and asserts it lands in cache. It never
/// asks whether anything can then *read* it — and nothing could, because
/// `resolve_guidance_db_path` derived `<data dir>/<asset_filename>` while the
/// builder wrote `<cache>/kb/<name>-<version>.cozo`. Two functions, two
/// answers to "where does this KB live", no test connecting them (principle
/// #8). The consequence was total: on a clean install `ai_guidance_kb`
/// delivered nothing, so MCP `initialize.instructions` and `mae-agent-cli`'s
/// system prompt carried no practices block at all.
///
/// `crates/mae/tests/guidance_delivery_e2e.rs` could not catch it either — it
/// hand-seeds `<data dir>/<asset_filename>`, i.e. it constructs by hand the
/// exact artifact the pipeline stopped producing, then asserts delivery works.
/// A fixture that builds the thing under test cannot fail when the thing under
/// test stops being built.
///
/// So this asserts the *join*, with no hand-seeding: build the way a real
/// first run builds, then read the way a real MCP `initialize` reads.
#[test]
fn a_freshly_built_guidance_store_is_found_by_the_guidance_reader() {
    let _lock = mae_effect_sandbox::lock_env();
    let prev_d = std::env::var("MAE_DEVPRACTICES_KB_PATH").ok();
    let prev_cache = std::env::var("XDG_CACHE_HOME").ok();

    let data_dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CACHE_HOME", cache.path());
    // Point the override at a path that does not exist, so no *installed*
    // store can satisfy this and the build-from-corpus branch is the only way
    // through. Without this the dev checkout's own `assets/mae-devpractices.cozo`
    // would satisfy `locate()` and the test would pass for the wrong reason —
    // which is precisely how a contributor's machine diverges from a user's.
    std::env::set_var(
        "MAE_DEVPRACTICES_KB_PATH",
        data_dir.path().join("definitely-absent.cozo"),
    );

    let kb = mae_kb::system_kb::find("DevPractices").unwrap();
    let built = build_guidance_from_embedded_corpus(kb, data_dir.path());
    let context = mae_ai::guidance::read_guidance_kb_context(data_dir.path(), "DevPractices");

    match prev_d {
        Some(v) => std::env::set_var("MAE_DEVPRACTICES_KB_PATH", v),
        None => std::env::remove_var("MAE_DEVPRACTICES_KB_PATH"),
    }
    match prev_cache {
        Some(v) => std::env::set_var("XDG_CACHE_HOME", v),
        None => std::env::remove_var("XDG_CACHE_HOME"),
    }

    let built = built.expect("the corpus must build");
    assert!(built.exists(), "built store missing at {}", built.display());

    let context = context.unwrap_or_else(|| {
        panic!(
            "guidance built to {} but the reader found nothing — the builder and \
             `resolve_guidance_db_path` disagree about where a system KB's store lives",
            built.display()
        )
    });
    assert!(
        context.contains("DevPractices"),
        "the delivered guidance must be the real corpus, got: {context}"
    );
}

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
            kind,
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
        kind: mae_kb::federation::KbInstanceKind::UserRegistered,
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
