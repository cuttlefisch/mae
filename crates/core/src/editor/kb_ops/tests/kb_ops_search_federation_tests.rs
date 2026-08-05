use super::*;

/// ADR-058 Phase B adversarial test: a project root that doesn't exist (deleted, or a
/// TOCTOU window where it vanished between detection and provisioning) must fail cleanly
/// with an `Err`, never panic.
#[test]
fn kb_init_project_fails_cleanly_on_nonexistent_root_no_panic() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let nonexistent = std::path::PathBuf::from("/tmp/mae-adr058-does-not-exist-xyz-12345");
    let result = editor.kb_init_project(Some(nonexistent));
    assert!(
        result.is_err(),
        "a nonexistent project root must fail cleanly with Err, not panic"
    );
}

/// ADR-058 Phase E adversarial test: once a user declines project-KB provisioning, the
/// decline must survive both repeated triggering (50 subsequent KB actions in the same
/// session) and a process restart (a fresh `Editor` reloading the same on-disk registry) —
/// never re-prompting either way.
#[test]
fn kb_decline_project_provisioning_persists_across_actions_and_restart() {
    let shared_tmp = tempfile::tempdir().unwrap();
    let data_dir = shared_tmp.path().join("data");
    let project_root = shared_tmp.path().join("project");
    std::fs::create_dir_all(&project_root).unwrap();

    let mut editor = Editor::new();
    editor.data_dir_override = Some(data_dir.clone());
    editor.project = Some(crate::project::Project::from_root(project_root.clone()));

    // Trigger once to raise the suggestion, then decline it.
    editor.maybe_suggest_project_kb_provisioning();
    assert!(
        editor.notifications.active_sorted().iter().any(|n| n
            .key
            .as_deref()
            .is_some_and(|k| k.starts_with("kb-init-project:"))),
        "the suggestion notification must be raised before any decline"
    );
    editor
        .kb_decline_project_provisioning(Some(project_root.clone()))
        .unwrap();

    // 50 subsequent triggers in the same session: no re-prompt.
    for _ in 0..50 {
        editor.maybe_suggest_project_kb_provisioning();
    }
    let still_registered_project = editor
        .kb
        .registry
        .instances
        .iter()
        .any(|i| i.effective_kind() == mae_kb::federation::KbInstanceKind::Project);
    assert!(
        !still_registered_project,
        "sanity: this test declines, it must not have actually provisioned anything"
    );

    // Fresh Editor (simulated restart) pointed at the same data_dir: still no re-prompt.
    let mut restarted = Editor::new();
    restarted.data_dir_override = Some(data_dir);
    restarted.project = Some(crate::project::Project::from_root(project_root));
    restarted.kb.registry =
        mae_kb::federation::KbRegistry::load(restarted.data_dir_override.as_ref().unwrap());
    restarted.maybe_suggest_project_kb_provisioning();
    assert!(
        !restarted.notifications.active_sorted().iter().any(|n| n
            .key
            .as_deref()
            .is_some_and(|k| k.starts_with("kb-init-project:"))),
        "a decline recorded before restart must suppress the suggestion after restart too"
    );
}

// The genuine 3-way-concurrent registry-convergence adversarial test lives in
// `shared/kb/src/federation.rs` (`kb_registry_register_converges_under_a_three_way_race`),
// exercising `KbRegistry::register`/`KbRegistry::update` directly rather than through
// `Editor::kb_init_project`. Deliberately scoped there and not here: routing all 3 threads
// through the full `kb_init_project` (which also opens/imports into a real CozoDB store per
// call via `kb_adopt_instance`) hit a separate, pre-existing concurrent-store-open bug in
// `shared/kb/src/cozo_store/source_files.rs` unrelated to ADR-058's own registry-dedup
// contract — found while writing this test, tracked separately rather than silently masked
// by weakening the adversarial coverage down to non-concurrent calls.

#[test]
fn kb_open_instance_store_defaults_to_sqlite_not_sled() {
    // Regression: kb_register/kb_reimport/the federation loader used the bare
    // `CozoKbStore::open()`, which is hardcoded to the sled engine — ignoring
    // `kb_storage_engine` (default sqlite) entirely. Every registered federated
    // instance was permanently stuck on sled's single-writer exclusive lock, so
    // a second mae frontend could never open the same instance concurrently —
    // regardless of the option the user configured. A sled store is a
    // directory; a sqlite store is a file — that's the discriminator.
    let editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("instance.cozo");
    editor.kb_open_instance_store(&path).unwrap();
    assert!(
        path.is_file(),
        "default engine must be sqlite (a file), not sled (a directory)"
    );
}

#[test]
fn kb_open_instance_store_migrates_an_existing_sled_instance() {
    // A pre-existing legacy sled federated instance (e.g. registered before
    // Phase 2c, or hand-created) must be auto-migrated to sqlite on next open —
    // matching the primary store's behavior — not opened as sled forever.
    let editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("instance.cozo");

    {
        let sled = mae_kb::CozoKbStore::open_with_engine(&path, "sled").unwrap();
        sled.seed_type_system().unwrap();
        sled.insert_node(&mae_kb::Node::new(
            "user:legacy",
            "Legacy",
            mae_kb::NodeKind::Note,
            "pre-migration content",
        ))
        .unwrap();
    }
    assert!(path.is_dir(), "sanity: sled store is a directory");

    let migrated = editor.kb_open_instance_store(&path).unwrap();
    assert!(path.is_file(), "path must be a sqlite file after migration");
    assert!(
        migrated.get_node("user:legacy").unwrap().is_some(),
        "migration must preserve existing nodes, not drop them"
    );
}

#[test]
fn kb_register_allows_a_second_concurrent_frontend_to_open_the_same_instance() {
    // The actual user-facing bug: two mae GUI frontends both pointed at the same
    // registered KB instance. Before the fix, the FIRST frontend's kb_register
    // opened the instance as sled and kept the handle open for the process
    // lifetime; a SECOND frontend's attempt to open the same instance store hit
    // sled's exclusive dir lock and failed (silently falling back to a
    // non-persistent in-memory import — the exact bug originally reported
    // against a registered KB). With sqlite as the engine, a second handle
    // must succeed
    // while the first is still open — the same topology that already lets N
    // daemon-less processes share the primary store.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();
    // The first "frontend"'s handle is still held open here (in instance_stores).
    assert!(editor.kb.instance_stores.contains_key(&uuid));

    let db_path = editor.kb.registry.find(&uuid).unwrap().db_path.clone();
    let second_frontend = mae_kb::CozoKbStore::open_with_engine(&db_path, "sqlite");
    assert!(
        second_frontend.is_ok(),
        "a second frontend must be able to open the same registered instance \
             concurrently: {:?}",
        second_frontend.err()
    );
    assert!(
        second_frontend
            .unwrap()
            .get_node("test-note-1")
            .unwrap()
            .is_some(),
        "the second frontend must see the first frontend's imported nodes"
    );
}

#[test]
fn kb_federated_search_finds_across_instances() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());

    // Search should find nodes from federated instance
    let results = editor.kb_federated_search("Note");
    let federated: Vec<_> = results.iter().filter(|(name, _)| name.is_some()).collect();
    assert!(!federated.is_empty());
}

#[test]
fn kb_federated_search_scope_filters_instances() {
    use mae_kb::KbScope;
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());

    let count_federated =
        |r: &[(Option<String>, mae_kb::Node)]| r.iter().filter(|(name, _)| name.is_some()).count();

    // All: includes the federated TestNotes instance.
    let all = editor.kb_federated_search_scoped("Note", &KbScope::All);
    assert!(count_federated(&all) > 0, "All should include federated");

    // LocalOnly: drops every federated result.
    let local = editor.kb_federated_search_scoped("Note", &KbScope::LocalOnly);
    assert_eq!(count_federated(&local), 0, "LocalOnly excludes federated");

    // Named: selects exactly the named instance's results.
    let named = editor.kb_federated_search_scoped("Note", &KbScope::Named("TestNotes".into()));
    assert!(count_federated(&named) > 0, "Named selects the instance");
    assert!(
        named
            .iter()
            .all(|(name, _)| name.is_none() || name.as_deref() == Some("TestNotes")),
        "Named yields only that instance (+ local)"
    );

    // RemoteOnly: TestNotes is a local import (not shared), so no results.
    let remote = editor.kb_federated_search_scoped("Note", &KbScope::RemoteOnly);
    assert_eq!(
        count_federated(&remote),
        0,
        "RemoteOnly excludes non-shared local imports"
    );
}

#[test]
fn resolve_kb_scope_project_token_resolves_current_root_or_falls_back_to_all() {
    let mut editor = Editor::new();

    // No project detectable at all: "project" gracefully degrades to All (Phase E) —
    // never an unusable/empty scope, and never a panic.
    assert_eq!(editor.resolve_kb_scope("project"), mae_kb::KbScope::All);

    // A project IS set: "project"/"project-only" resolve to Project(root).
    let root = std::path::PathBuf::from("/tmp/mae-adr058-resolve-test");
    editor.project = Some(crate::project::Project::from_root(root.clone()));
    assert_eq!(
        editor.resolve_kb_scope("project"),
        mae_kb::KbScope::Project(root.clone())
    );
    assert_eq!(
        editor.resolve_kb_scope("PROJECT-ONLY"),
        mae_kb::KbScope::Project(root)
    );

    // Every other token still delegates to KbScope::parse unaffected.
    assert_eq!(editor.resolve_kb_scope("all"), mae_kb::KbScope::All);
    assert_eq!(editor.resolve_kb_scope("local"), mae_kb::KbScope::LocalOnly);
    assert_eq!(
        editor.resolve_kb_scope("SomeInstance"),
        mae_kb::KbScope::Named("SomeInstance".into())
    );
}

/// ADR-058 Phase C adversarial test: register a varied mix of instances (multiple distinct
/// project roots, one non-project instance, deliberately overlapping/shared vocabulary in
/// bodies so a naive substring-only bug would leak across projects) and, for every project
/// root in the mix, assert `KbScope::Project(root)` results are (a) always a non-strict
/// subset of `KbScope::All` results and (b) contain zero nodes from any *other* project or
/// the non-project instance. A single hand-picked pair of projects could pass by accident
/// (e.g. if the filter only happened to work for exactly two); this exercises several.
#[test]
fn kb_federated_search_scope_project_never_leaks_across_projects() {
    use mae_kb::federation::{KbInstance, KbInstanceKind};
    use mae_kb::KbScope;

    let mut editor = Editor::new();

    struct Fixture {
        uuid: &'static str,
        name: &'static str,
        root: Option<&'static str>,
        kind: KbInstanceKind,
        node_id: &'static str,
    }
    let fixtures = [
        Fixture {
            uuid: "u-alpha",
            name: "alpha",
            root: Some("/tmp/mae-adr058-fixture/project-alpha"),
            kind: KbInstanceKind::Project,
            node_id: "proj:alpha-note",
        },
        Fixture {
            uuid: "u-beta",
            name: "beta",
            root: Some("/tmp/mae-adr058-fixture/project-beta"),
            kind: KbInstanceKind::Project,
            node_id: "proj:beta-note",
        },
        Fixture {
            uuid: "u-gamma",
            name: "gamma",
            root: Some("/tmp/mae-adr058-fixture/project-gamma"),
            kind: KbInstanceKind::Project,
            node_id: "proj:gamma-note",
        },
        Fixture {
            uuid: "u-unscoped",
            name: "unscoped",
            root: None,
            kind: KbInstanceKind::UserRegistered,
            node_id: "proj:unscoped-note",
        },
    ];

    for f in &fixtures {
        let mut kb = mae_kb::KnowledgeBase::new();
        // Shared "widget" vocabulary across every fixture's body — a substring-only or
        // relevance-only filter bug would leak these across projects; only the scope filter
        // (by instance, not by content) should prevent that.
        kb.insert(mae_kb::Node::new(
            f.node_id,
            "Widget notes",
            mae_kb::NodeKind::Note,
            "shared widget vocabulary present in every fixture",
        ));
        editor.kb.instances.insert(f.uuid.to_string(), kb);
        editor.kb.registry.instances.push(KbInstance {
            uuid: f.uuid.to_string(),
            name: f.name.to_string(),
            org_dir: std::path::PathBuf::from(format!("/tmp/mae-adr058-fixture/{}", f.name)),
            db_path: std::path::PathBuf::from(format!("/tmp/mae-adr058-fixture/{}.db", f.name)),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
            project_root: f.root.map(std::path::PathBuf::from),
            kind: f.kind,
            priority: 0,
            remote_hub: None,
        });
    }

    let all = editor.kb_federated_search_scoped("widget", &KbScope::All);
    let all_ids: std::collections::HashSet<&str> = all.iter().map(|(_, n)| n.id.as_str()).collect();
    for f in &fixtures {
        assert!(
            all_ids.contains(f.node_id),
            "sanity: All scope must see fixture {}'s node {}, got {all_ids:?}",
            f.name,
            f.node_id
        );
    }

    for f in fixtures.iter().filter(|f| f.root.is_some()) {
        let root = std::path::PathBuf::from(f.root.unwrap());
        let scoped = editor.kb_federated_search_scoped("widget", &KbScope::Project(root));
        let scoped_ids: std::collections::HashSet<&str> =
            scoped.iter().map(|(_, n)| n.id.as_str()).collect();

        assert!(
            scoped_ids.contains(f.node_id),
            "Project scope for {} must include its own node",
            f.name
        );
        assert!(
            scoped_ids.is_subset(&all_ids),
            "Project scope for {} must be a subset of All: {scoped_ids:?} vs {all_ids:?}",
            f.name
        );
        for other in fixtures.iter().filter(|o| o.node_id != f.node_id) {
            assert!(
                !scoped_ids.contains(other.node_id),
                "Project scope for {} must NOT leak {}'s node {}",
                f.name,
                other.name,
                other.node_id
            );
        }
    }
}

/// ADR-058 Phase D adversarial test (the negative case the ADR names verbatim): path
/// *identity*, not raw string equality, must govern whether the current project matches a
/// registered `Project`-kind instance. Two sub-cases, both against real directories on disk
/// (not synthetic path strings — a canonicalization bug can hide behind a path that was never
/// actually resolved against the filesystem):
/// 1. A symlink-aliased path (differently *spelled*, same real directory) must still match —
///    proving canonicalization doesn't produce false negatives.
/// 2. A genuinely different real directory (even one an uncanonicalized comparison might
///    confuse, e.g. via a same-named symlink) must NOT match — proving it doesn't produce
///    false positives / silent collisions either, which is the specific failure mode the ADR
///    names: two projects "colliding by string path but differing by canonicalized path" must
///    not silently merge.
// #[cfg(unix)]: relies on std::os::unix::fs::symlink (no portable equivalent --
// Windows symlink creation needs elevated privileges by default, see ADR-066's
// bootstrap.rs precedent). Tracked as a known Windows test-coverage gap in issue
// #455 (Gap 1's kb_ops cluster) rather than ported/skipped ad hoc.
#[cfg(unix)]
#[test]
fn kb_scope_project_path_identity_not_string_equality() {
    use mae_kb::federation::{KbInstance, KbInstanceKind};
    use mae_kb::KbScope;

    let tmp = tempfile::tempdir().unwrap();
    let real_a = tmp.path().join("real-project-a");
    let real_b = tmp.path().join("real-project-b");
    std::fs::create_dir_all(&real_a).unwrap();
    std::fs::create_dir_all(&real_b).unwrap();
    let link_to_a = tmp.path().join("alias-to-a");
    std::os::unix::fs::symlink(&real_a, &link_to_a).unwrap();

    let canonical_a = real_a.canonicalize().unwrap();

    let mut editor = Editor::new();
    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(mae_kb::Node::new(
        "proj:a-note",
        "A note",
        mae_kb::NodeKind::Note,
        "widget content in project a",
    ));
    editor.kb.instances.insert("uuid-a".into(), kb);
    editor.kb.registry.instances.push(KbInstance {
        uuid: "uuid-a".into(),
        name: "project-a".into(),
        org_dir: real_a.clone(),
        db_path: tmp.path().join("a.db"),
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: None,
        shared: false,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: mae_kb::federation::AiResidency::default(),
        project_root: Some(canonical_a.clone()),
        kind: KbInstanceKind::Project,
        priority: 0,
        remote_hub: None,
    });

    // Case 1 (must match): resolve via the symlink alias — a differently-*spelled* path to
    // the SAME real directory. `resolve_kb_scope` canonicalizes before constructing the
    // scope, so this must still find project-a's node.
    editor.project = Some(crate::project::Project::from_root(link_to_a));
    let scope = editor.resolve_kb_scope("project");
    assert_eq!(
        scope,
        KbScope::Project(canonical_a.clone()),
        "a symlink-aliased path to the same real directory must canonicalize to an \
         identical KbScope::Project, not a textually-different one"
    );
    let results = editor.kb_federated_search_scoped("widget", &scope);
    assert!(
        results.iter().any(|(_, n)| n.id == "proj:a-note"),
        "resolving through a symlink alias must still find the registered project's node"
    );

    // Case 2 (must NOT match): a genuinely different real directory. Even though nothing
    // here makes its *string* collide with project-a's, this is the actual negative
    // property under test — canonical identity governs matching, so an unrelated directory
    // must never be treated as project-a no matter how its path is spelled.
    editor.project = Some(crate::project::Project::from_root(real_b.clone()));
    let scope_b = editor.resolve_kb_scope("project");
    assert_ne!(
        scope_b,
        KbScope::Project(canonical_a),
        "a genuinely different real directory must not resolve to the same KbScope::Project"
    );
    let results_b = editor.kb_federated_search_scoped("widget", &scope_b);
    assert!(
        results_b.iter().all(|(_, n)| n.id != "proj:a-note"),
        "an unrelated project's scope must never silently include project-a's node: {results_b:?}"
    );
}

#[test]
fn kb_search_recency_floats_visited_to_top() {
    let mut editor = Editor::new();
    editor.kb.search_sort = "recency".to_string();

    // Pick two nodes that both match a common query but aren't the top
    // relevance hit, then visit the second one and confirm it leads.
    let baseline = editor.kb_federated_search("buffer");
    assert!(baseline.len() >= 2, "need ≥2 matches for the query");
    // A match that is NOT already first under relevance.
    let promote = baseline[1].1.id.clone();

    // No visits yet → recency order == relevance order (stable).
    let ids_before: Vec<String> = editor
        .kb_federated_search("buffer")
        .iter()
        .map(|(_, n)| n.id.clone())
        .collect();
    assert_eq!(ids_before.first(), Some(&baseline[0].1.id.clone()));

    // Visit the promoted node; it should now sort first.
    editor.kb.record_visit(&promote);
    let ids_after: Vec<String> = editor
        .kb_federated_search("buffer")
        .iter()
        .map(|(_, n)| n.id.clone())
        .collect();
    assert_eq!(
        ids_after.first(),
        Some(&promote),
        "visited node should float to the top under recency sort"
    );
}

#[test]
fn kb_search_sort_option_accepts_recency() {
    let mut editor = Editor::new();
    assert!(editor.set_option("kb_search_sort", "recency").is_ok());
    assert_eq!(editor.kb.search_sort, "recency");
    assert_eq!(
        editor.get_option("kb_search_sort").map(|(v, _)| v),
        Some("recency".to_string())
    );
    // Invalid value is rejected and leaves the setting unchanged.
    assert!(editor.set_option("kb_search_sort", "bogus").is_err());
    assert_eq!(editor.kb.search_sort, "recency");
}

#[test]
fn kb_search_scope_option_round_trip() {
    let mut editor = Editor::new();
    // Keywords always validate.
    for kw in ["all", "local", "remote"] {
        assert!(editor.set_option("kb_search_scope", kw).is_ok());
        assert_eq!(editor.kb.search_scope, kw);
    }
    // An unknown instance name is rejected (no instance registered).
    assert!(editor.set_option("kb_search_scope", "NoSuchKB").is_err());
    // A registered instance name validates.
    let dir = create_test_org_dir();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());
    assert!(editor.set_option("kb_search_scope", "TestNotes").is_ok());
    assert_eq!(
        editor.get_option("kb_search_scope").map(|(v, _)| v),
        Some("TestNotes".to_string())
    );
}

#[test]
fn kb_search_scope_option_accepts_project_and_project_only() {
    // A6 config-gap fix: `resolve_kb_scope` already fully supports (and has
    // its own passing test for) the "project"/"project-only" token, but the
    // setter's keyword allowlist omitted both spellings -- a user could not
    // actually `:set kb_search_scope project` (or `(set-option! "kb_search_scope"
    // "project")`) despite it being a real, working, documented scope value.
    // This previously returned Err("Invalid kb_search_scope: ..."); both
    // spellings, case-insensitively, must now validate.
    let mut editor = Editor::new();
    for kw in ["project", "project-only", "PROJECT", "Project-Only"] {
        assert!(
            editor.set_option("kb_search_scope", kw).is_ok(),
            "kb_search_scope must accept '{kw}'"
        );
        assert_eq!(editor.kb.search_scope, kw);
        assert_eq!(
            editor.get_option("kb_search_scope").map(|(v, _)| v),
            Some(kw.to_string())
        );
    }
}

#[test]
fn ai_guidance_kb_option_round_trip() {
    let mut editor = Editor::new();
    // Empty (disabled, the default) always validates.
    assert!(editor.set_option("ai_guidance_kb", "").is_ok());
    assert_eq!(editor.ai_guidance_kb, "");
    // "primary" always validates.
    assert!(editor.set_option("ai_guidance_kb", "primary").is_ok());
    assert_eq!(editor.ai_guidance_kb, "primary");
    // Issue #370 drift fix: unlike `kb_search_scope`, an unknown/not-yet-registered
    // instance name is intentionally ACCEPTED, not rejected -- init.scm evaluates
    // BEFORE KB federation populates `self.kb.registry`, so the shipped default
    // ("DevPractices") would always fail eager validation here even though it
    // resolves correctly moments later. Resolution is deliberately deferred to
    // read time (`crates/ai/src/guidance.rs::read_guidance_kb_context`, which is
    // already best-effort and silently no-ops for an unresolvable name).
    assert!(editor.set_option("ai_guidance_kb", "no-such-kb").is_ok());
    assert_eq!(editor.ai_guidance_kb, "no-such-kb");
    // A registered instance name also validates, same as before.
    let dir = create_test_org_dir();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("dev-practices", dir.path());
    assert!(editor.set_option("ai_guidance_kb", "dev-practices").is_ok());
    assert_eq!(
        editor.get_option("ai_guidance_kb").map(|(v, _)| v),
        Some("dev-practices".to_string())
    );
}

#[test]
fn kb_find_candidates_small_kb_returns_all() {
    let editor = Editor::new();
    // The seed manual is well under the lazy threshold.
    assert!(editor.kb_loaded_node_count() <= Editor::KB_FIND_LAZY_THRESHOLD);
    let all = editor.kb_all_node_triples();
    let cands = editor.kb_find_candidates("");
    assert_eq!(cands.len(), all.len(), "small KB should return every node");
}

#[test]
fn kb_find_candidates_large_kb_is_bounded_but_query_reachable() {
    let mut editor = Editor::new();
    // Push past the lazy threshold with synthetic nodes, including one
    // distinctive node far beyond the empty-query window.
    for i in 0..(Editor::KB_FIND_LAZY_THRESHOLD + 500) {
        editor.kb.primary.insert(mae_kb::Node::new(
            format!("note:bulk{i}"),
            format!("Bulk Note {i}"),
            mae_kb::NodeKind::Note,
            "filler body",
        ));
    }
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:zebra-marker",
        "Zebra Marker",
        mae_kb::NodeKind::Note,
        "uniquely findable",
    ));
    assert!(editor.kb_loaded_node_count() > Editor::KB_FIND_LAZY_THRESHOLD);

    // Empty query: bounded window, not the whole KB.
    let empty = editor.kb_find_candidates("");
    assert!(
        empty.len() <= Editor::KB_FIND_LAZY_LIMIT,
        "large-KB window should be bounded, got {}",
        empty.len()
    );

    // A targeted query still reaches a node outside the empty window — the
    // ranker scans the whole KB, so lazy completion stays full-KB-reachable.
    let hits = editor.kb_find_candidates("zebra marker");
    assert!(
        hits.iter().any(|(id, _, _)| id == "note:zebra-marker"),
        "targeted query must find the distinctive node at scale"
    );
}

#[test]
fn kb_find_candidates_empty_query_defaults_to_activity_order_not_alphabetical() {
    let mut editor = Editor::new();
    assert_eq!(
        editor.kb.search_sort, "relevance",
        "sanity check: default sort"
    );
    // "note:zzz-recent" sorts LAST alphabetically among these three, but is
    // the most recently accessed -- an empty-query kb-find must default to
    // activity order (most-recently-active first), not let the meaningless
    // "relevance" default silently degenerate to alphabetical-by-id.
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:aaa-old",
        "Old note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:mmm-mid",
        "Mid note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    let (y, m, d) = today_ymd();
    let today = mae_kb::activity::format_date(y, m, d);
    let mut recent = mae_kb::Node::new(
        "note:zzz-recent",
        "Recent note",
        mae_kb::NodeKind::Note,
        "body",
    );
    recent.properties.insert("last-accessed".to_string(), today);
    editor.kb.primary.insert(recent);

    let cands = editor.kb_find_candidates("");
    let ids: Vec<&str> = cands.iter().map(|(id, _, _)| id.as_str()).collect();
    assert_eq!(
        ids.first(),
        Some(&"note:zzz-recent"),
        "most recently accessed node should be first for an empty query, got {ids:?}"
    );
}

#[test]
fn kb_find_candidates_respects_explicit_alphabetical_override_even_on_empty_query() {
    let mut editor = Editor::new();
    editor.set_option("kb_search_sort", "alphabetical").unwrap();
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:aaa-old",
        "Old note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    let (y, m, d) = today_ymd();
    let today = mae_kb::activity::format_date(y, m, d);
    let mut recent = mae_kb::Node::new(
        "note:zzz-recent",
        "Recent note",
        mae_kb::NodeKind::Note,
        "body",
    );
    recent.properties.insert("last-accessed".to_string(), today);
    editor.kb.primary.insert(recent);

    let cands = editor.kb_find_candidates("");
    let ids: Vec<&str> = cands.iter().map(|(id, _, _)| id.as_str()).collect();
    // Editor::new() seeds ~1000 manual-KB nodes, so "note:aaa-old" won't be
    // globally first -- check its position RELATIVE to "note:zzz-recent"
    // instead: alphabetically "aaa" sorts before "zzz", so if this held,
    // the explicit alphabetical choice was correctly left untouched. Under
    // the (wrong) activity default, zzz-recent's non-zero score would put
    // it first instead.
    let pos_old = ids.iter().position(|&id| id == "note:aaa-old").unwrap();
    let pos_recent = ids.iter().position(|&id| id == "note:zzz-recent").unwrap();
    assert!(
        pos_old < pos_recent,
        "an explicit alphabetical sort choice must stay alphabetical on an \
         empty query, not be silently overridden by the activity default \
         (note:aaa-old at {pos_old}, note:zzz-recent at {pos_recent})"
    );
}

#[test]
fn kb_find_candidates_nonempty_query_behavior_unchanged_by_empty_query_default() {
    // Regression guard: the empty-query activity default must only apply
    // when query.is_empty() -- a non-empty query's candidate set is
    // unaffected (same nodes as kb_all_node_triples, no filtering here;
    // ranking/filtering for non-empty queries happens client-side via the
    // palette's fuzzy filter).
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:aaa-old",
        "Old note",
        mae_kb::NodeKind::Note,
        "body",
    ));
    let (y, m, d) = today_ymd();
    let today = mae_kb::activity::format_date(y, m, d);
    let mut recent = mae_kb::Node::new(
        "note:zzz-recent",
        "Recent note",
        mae_kb::NodeKind::Note,
        "body",
    );
    recent.properties.insert("last-accessed".to_string(), today);
    editor.kb.primary.insert(recent);

    let all = editor.kb_all_node_triples();
    let queried = editor.kb_find_candidates("note");
    assert_eq!(
        all.len(),
        queried.len(),
        "a non-empty query must return the same candidate set as kb_all_node_triples \
         (small-KB path), unaffected by the empty-query activity default"
    );
}

#[test]
fn kb_find_candidates_reaches_federated_instance_nodes_at_scale() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "note:federated-zebra",
        "Federated Zebra Marker",
        mae_kb::NodeKind::Note,
        "uniquely findable in a federated instance",
    ));
    editor.kb.instances.insert("test-instance".into(), inst);

    // Push primary past the lazy threshold, same as the sibling test above.
    for i in 0..(Editor::KB_FIND_LAZY_THRESHOLD + 500) {
        editor.kb.primary.insert(mae_kb::Node::new(
            format!("note:bulk{i}"),
            format!("Bulk Note {i}"),
            mae_kb::NodeKind::Note,
            "filler body",
        ));
    }
    assert!(editor.kb_loaded_node_count() > Editor::KB_FIND_LAZY_THRESHOLD);

    // A targeted query must still reach a node that lives ONLY in a
    // federated instance, not primary — this is exactly the bug
    // kb_find_candidates had: the lazy branch searched primary alone,
    // making federated content permanently unreachable through kb-find
    // once the KB tipped past the threshold, regardless of query.
    let hits = editor.kb_find_candidates("federated zebra");
    assert!(
        hits.iter().any(|(id, _, _)| id == "note:federated-zebra"),
        "targeted query must find a federated-instance-only node at scale"
    );
}

#[test]
fn kb_find_palette_lazy_refresh_repopulates_on_query() {
    let mut editor = Editor::new();
    for i in 0..(Editor::KB_FIND_LAZY_THRESHOLD + 100) {
        editor.kb.primary.insert(mae_kb::Node::new(
            format!("note:bulk{i}"),
            format!("Bulk Note {i}"),
            mae_kb::NodeKind::Note,
            "filler",
        ));
    }
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:platypus",
        "Platypus",
        mae_kb::NodeKind::Note,
        "distinctive",
    ));

    // Open kb-find: bounded initial window.
    assert!(editor.dispatch_builtin("kb-find"));
    let initial = editor.command_palette.as_ref().unwrap().entries.len();
    assert!(initial <= Editor::KB_FIND_LAZY_LIMIT);

    // Type a query, then refresh: the distinctive node is now reachable.
    if let Some(p) = editor.command_palette.as_mut() {
        p.query = "platypus".to_string();
    }
    editor.kb_find_palette_query_changed();
    let entries: Vec<String> = editor
        .command_palette
        .as_ref()
        .unwrap()
        .entries
        .iter()
        .map(|e| e.name.clone())
        .collect();
    assert!(
        entries.iter().any(|id| id == "note:platypus"),
        "lazy refresh should surface the queried node"
    );
}

#[test]
fn kb_set_search_scope_command_opens_picker() {
    let mut editor = Editor::new();
    assert!(editor.command_palette.is_none());
    assert!(editor.dispatch_builtin("kb-set-search-scope"));
    let palette = editor.command_palette.as_ref().expect("picker should open");
    assert_eq!(
        palette.purpose,
        crate::command_palette::PalettePurpose::SetKbSearchScope
    );
    // Keyword scopes are always present (no instances registered here).
    let names: Vec<&str> = palette.entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, vec!["all", "local", "remote", "project"]);
}

#[test]
fn kb_visit_log_is_monotonic() {
    let mut editor = Editor::new();
    editor.kb.record_visit("concept:buffer");
    editor.kb.record_visit("concept:window");
    editor.kb.record_visit("concept:buffer"); // re-visit bumps ahead
    assert!(editor.kb.visit_rank("concept:buffer") > editor.kb.visit_rank("concept:window"));
    assert_eq!(editor.kb.visit_rank("never-visited"), 0);
}

#[test]
fn kb_federated_get_local_first() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path());

    // Get from federated instance
    let result = editor.kb_federated_get("test-note-1");
    assert!(result.is_some());
    let (inst_name, node) = result.unwrap();
    assert_eq!(inst_name, Some("TestNotes".to_string()));
    assert_eq!(node.title, "Note One");
}

#[test]
fn kb_register_nonexistent_path() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("Bad", Path::new("/nonexistent/path"));
    assert!(result.is_none());
    assert!(editor.status_msg.contains("does not exist"));
}

#[test]
fn kb_register_canonicalizes_org_dir() {
    // #303: registering with a non-canonical path (here, a redundant
    // `subdir/..` component) must store the canonical form so a later
    // comparison/re-derivation against `org_dir` doesn't drift from
    // what was actually walked at import time.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);

    let canonical = dir.path().canonicalize().unwrap();
    let noncanonical = dir.path().join("subdir").join("..");
    assert_ne!(
        noncanonical, canonical,
        "test setup must actually be non-canonical"
    );

    let result = editor
        .kb_register("TestNotes", &noncanonical)
        .expect("registration should succeed");
    let instance = editor.kb.registry.find(&result.uuid).unwrap();
    assert_eq!(
        instance.org_dir, canonical,
        "registry must store the canonicalized org_dir, not the literal argument"
    );
}

// Issue #496: `kb_register`'s real registration path canonicalizes `org_dir`
// (see `kb_register_canonicalizes_org_dir` above), but `kb_reimport_file`/
// `kb_path_in_instance` used to compare an un-canonicalized caller `path`
// against it via `Path::starts_with` — silently mismatching wherever a
// symlink separates the two spellings of the same real directory (macOS's
// `/var` -> `/private/var` in practice; reproduced here on Linux via an
// explicit symlink, same technique as `kb_scope_project_path_identity_not_
// string_equality` above, so this isn't gated on macOS-only CI).
#[cfg(unix)]
#[test]
fn kb_reimport_file_matches_instance_via_symlinked_alias_path() {
    let tmp = tempfile::tempdir().unwrap();
    let real_dir = tmp.path().join("real-notes");
    std::fs::create_dir_all(&real_dir).unwrap();
    let alias_dir = tmp.path().join("alias-to-notes");
    std::os::unix::fs::symlink(&real_dir, &alias_dir).unwrap();

    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor
        .kb_register("AliasNotes", &real_dir)
        .expect("registration should succeed");

    // Write the org file via the ALIAS path — the reimport call below uses
    // this same alias, mirroring a real save through a symlinked project dir.
    let alias_file = alias_dir.join("note1.org");
    std::fs::write(
        &alias_file,
        ":PROPERTIES:\n:ID: aliased-note\n:END:\n#+title: Aliased Note\n\nBody.\n",
    )
    .unwrap();

    editor.kb_reimport_file(&alias_file);

    assert!(
        editor.kb.instances[&result.uuid]
            .get("aliased-note")
            .is_some(),
        "reimporting a file via a symlinked alias of the registered org_dir must still \
         match the instance and ingest the node, not silently no-op"
    );
}

#[cfg(unix)]
#[test]
fn kb_path_in_instance_true_for_symlinked_alias_path() {
    let tmp = tempfile::tempdir().unwrap();
    let real_dir = tmp.path().join("real-notes");
    std::fs::create_dir_all(&real_dir).unwrap();
    let alias_dir = tmp.path().join("alias-to-notes");
    std::os::unix::fs::symlink(&real_dir, &alias_dir).unwrap();

    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor
        .kb_register("AliasNotes", &real_dir)
        .expect("registration should succeed");

    let alias_file = alias_dir.join("note1.org");
    std::fs::write(&alias_file, "content").unwrap();

    assert!(
        editor.kb_path_in_instance(&alias_file),
        "a path reached via a symlinked alias of a registered org_dir must be recognized \
         as inside that instance"
    );
}

#[cfg(unix)]
#[test]
fn kb_path_in_instance_false_for_a_real_but_unrelated_directory() {
    // Adversarial negative case: canonicalizing the caller's path must not
    // make matching MORE permissive — a genuinely different, unrelated real
    // directory must still return false.
    let tmp = tempfile::tempdir().unwrap();
    let real_dir = tmp.path().join("real-notes");
    let unrelated_dir = tmp.path().join("unrelated-notes");
    std::fs::create_dir_all(&real_dir).unwrap();
    std::fs::create_dir_all(&unrelated_dir).unwrap();

    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor
        .kb_register("RealNotes", &real_dir)
        .expect("registration should succeed");

    let unrelated_file = unrelated_dir.join("note.org");
    std::fs::write(&unrelated_file, "content").unwrap();

    assert!(
        !editor.kb_path_in_instance(&unrelated_file),
        "a genuinely unrelated real directory must never be treated as inside the \
         registered instance"
    );
}
