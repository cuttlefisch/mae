use super::*;
use tempfile::TempDir;

fn create_test_org_dir() -> TempDir {
    let dir = TempDir::new().unwrap();
    // File with :ID:
    std::fs::write(
        dir.path().join("note1.org"),
        ":PROPERTIES:\n:ID: test-note-1\n:END:\n#+title: Note One\n\nBody of note one.\n",
    )
    .unwrap();
    // File with :ID: in subdir
    let sub = dir.path().join("subdir");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(
            sub.join("note2.org"),
            ":PROPERTIES:\n:ID: test-note-2\n:END:\n#+title: Note Two\n\nLinks to [[id:test-note-1][Note One]].\n",
        )
        .unwrap();
    // File without :ID: (should be skipped)
    std::fs::write(
        dir.path().join("no-id.org"),
        "#+title: No ID\n\nJust a note without an ID property.\n",
    )
    .unwrap();
    dir
}

/// Set config/data dir overrides to a tempdir so tests never touch
/// real user directories (~/.config/mae, ~/.local/share/mae).
fn with_test_dirs(editor: &mut Editor) -> TempDir {
    let tmp = TempDir::new().unwrap();
    editor.config_dir_override = Some(tmp.path().join("config"));
    editor.data_dir_override = Some(tmp.path().join("data"));
    tmp
}

/// B5 / B-6 (CLAUDE.md #13): the primary KB data dir — the parent of the
/// `primary.cozo` store the editor opens at startup — MUST be XDG-first on
/// EVERY platform: `XDG_DATA_HOME/mae`, else `$HOME/.local/share/mae` — never
/// `dirs::data_dir()` (which is `~/Library/Application Support` on macOS and
/// would (a) break `XDG_DATA_HOME` test isolation and (b) split data from the
/// ADR-019 registry markers, breaking restart survival). This locks the
/// cf673b7c fix so a future change can't silently reintroduce `dirs::data_dir`.
#[test]
fn mae_data_dir_is_xdg_first_not_platform_native() {
    let mut editor = Editor::new();
    editor.data_dir_override = None; // exercise the real env-based resolution

    let orig_xdg = std::env::var_os("XDG_DATA_HOME");
    let orig_home = std::env::var_os("HOME");
    let tmp = TempDir::new().unwrap();

    // 1) XDG_DATA_HOME set → honored verbatim (joined with "mae").
    std::env::set_var("XDG_DATA_HOME", tmp.path());
    assert_eq!(
        editor.mae_data_dir(),
        Some(tmp.path().join("mae")),
        "XDG_DATA_HOME must be honored on all platforms"
    );

    // 2) No XDG_DATA_HOME → ~/.local/share/mae (NOT a platform-native dir).
    std::env::remove_var("XDG_DATA_HOME");
    std::env::set_var("HOME", tmp.path());
    let resolved = editor.mae_data_dir().expect("HOME-based path");
    assert_eq!(
        resolved,
        tmp.path().join(".local").join("share").join("mae"),
        "fallback must be ~/.local/share/mae, never ~/Library/Application Support"
    );
    assert!(
        !resolved
            .to_string_lossy()
            .contains("Library/Application Support"),
        "must never resolve to the macOS platform-native data dir"
    );

    // Restore env so sibling tests are unaffected.
    match orig_xdg {
        Some(v) => std::env::set_var("XDG_DATA_HOME", v),
        None => std::env::remove_var("XDG_DATA_HOME"),
    }
    match orig_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
}

#[test]
fn open_file_at_path_detects_language() {
    let dir = TempDir::new().unwrap();
    let org_path = dir.path().join("test-daily.org");
    std::fs::write(&org_path, "#+title: Test\n* Heading\n").unwrap();

    let mut editor = Editor::new();
    editor.open_file_at_path(&org_path);

    let idx = editor.buffers.len() - 1;
    assert_eq!(
        editor.syntax.language_of(idx),
        Some(crate::syntax::Language::Org),
        "open_file_at_path must set Language::Org for .org files"
    );
}

#[test]
fn kb_register_creates_instance() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path());
    assert!(result.is_some());
    let result = result.unwrap();
    assert_eq!(result.name, "TestNotes");
    assert_eq!(result.report.nodes_imported, 2);
    assert_eq!(result.report.nodes_skipped, 1); // no-id.org
    assert!(result.report.links_created >= 1); // note2 links to note1
    assert!(!result.uuid.is_empty());
    assert!(editor.kb.instances.contains_key(&result.uuid));
    assert_eq!(editor.kb.instances[&result.uuid].len(), 2);
}

#[test]
fn kb_register_handles_subdirs() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    // note2.org is in subdir/ — must be found
    assert_eq!(result.report.nodes_imported, 2);
    let kb = &editor.kb.instances[&result.uuid];
    assert!(kb.get("test-note-2").is_some());
}

#[test]
fn kb_unregister_removes_instance() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();
    assert!(editor.kb.instances.contains_key(&uuid));

    editor.kb_unregister("TestNotes");
    assert!(!editor.kb.instances.contains_key(&uuid));
    assert!(editor.kb.registry.find("TestNotes").is_none());
}

#[test]
fn kb_set_role_stamps_properties_and_is_independent_of_kind() {
    let mut editor = Editor::new();
    editor
        .kb_create_node(
            "note:molecular-test",
            "Test",
            "body",
            mae_kb::NodeKind::Concept,
        )
        .unwrap();

    let result = editor.kb_set_role("note:molecular-test", "atom").unwrap();
    assert!(result.contains("atom"), "result was: {result}");

    let node = editor.kb.primary.get("note:molecular-test").unwrap();
    assert_eq!(node.properties.get("role"), Some(&"atom".to_string()));
    // Orthogonal to NodeKind — setting :role: must not disturb :kind:.
    assert_eq!(node.kind, mae_kb::NodeKind::Concept);
}

#[test]
fn kb_set_role_is_case_insensitive_and_overwritable() {
    let mut editor = Editor::new();
    editor
        .kb_create_node("note:role-case", "Test", "body", mae_kb::NodeKind::Note)
        .unwrap();

    editor.kb_set_role("note:role-case", "MOLECULE").unwrap();
    assert_eq!(
        editor
            .kb
            .primary
            .get("note:role-case")
            .unwrap()
            .properties
            .get("role"),
        Some(&"molecule".to_string())
    );

    // Freely overwritable — reclassifying a note as understanding matures is
    // the whole point (a source can be distilled into an atom, etc).
    editor.kb_set_role("note:role-case", "hub").unwrap();
    assert_eq!(
        editor
            .kb
            .primary
            .get("note:role-case")
            .unwrap()
            .properties
            .get("role"),
        Some(&"hub".to_string())
    );
}

#[test]
fn kb_set_role_rejects_unknown_role() {
    let mut editor = Editor::new();
    editor
        .kb_create_node("note:bad-role", "Test", "body", mae_kb::NodeKind::Note)
        .unwrap();
    let err = editor
        .kb_set_role("note:bad-role", "not-a-real-role")
        .unwrap_err();
    assert!(err.contains("Invalid role"), "err was: {err}");
    // Rejected before touching the node — must not have mutated anything.
    assert!(!editor
        .kb
        .primary
        .get("note:bad-role")
        .unwrap()
        .properties
        .contains_key("role"));
}

#[test]
fn kb_set_role_unknown_node_errors() {
    let mut editor = Editor::new();
    let err = editor
        .kb_set_role("note:does-not-exist", "atom")
        .unwrap_err();
    assert!(err.contains("No KB node"), "err was: {err}");
}

#[test]
fn kb_instance_defaults_to_open_ai_residency() {
    // Backward-compat guard (ADR-048): a freshly registered instance — and any
    // instance loaded from a pre-existing registry file that never had this field —
    // must default to Open, not silently become restricted.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path()).unwrap();
    let inst = editor.kb.registry.find("TestNotes").unwrap();
    assert_eq!(inst.ai_residency, mae_kb::federation::AiResidency::Open);
    assert_eq!(
        editor.kb.registry.primary_ai_residency,
        mae_kb::federation::AiResidency::Open
    );
}

#[test]
fn kb_set_ai_residency_updates_named_instance() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path()).unwrap();

    let msg = editor
        .kb_set_ai_residency(
            "TestNotes",
            mae_kb::federation::AiResidency::LocalModelsOnly,
        )
        .expect("set_ai_residency should succeed for a registered instance");
    assert!(msg.contains("local_models_only"), "msg was: {msg}");
    assert_eq!(
        editor.kb.registry.find("TestNotes").unwrap().ai_residency,
        mae_kb::federation::AiResidency::LocalModelsOnly
    );

    // Freely toggleable back to Open — no anti-downgrade for a local, non-shared KB.
    editor
        .kb_set_ai_residency("TestNotes", mae_kb::federation::AiResidency::Open)
        .expect("toggling back to Open should succeed");
    assert_eq!(
        editor.kb.registry.find("TestNotes").unwrap().ai_residency,
        mae_kb::federation::AiResidency::Open
    );
}

#[test]
fn kb_set_ai_residency_updates_primary() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor
        .kb_set_ai_residency("primary", mae_kb::federation::AiResidency::LocalModelsOnly)
        .expect("set_ai_residency should succeed for the primary KB");
    assert_eq!(
        editor.kb.registry.primary_ai_residency,
        mae_kb::federation::AiResidency::LocalModelsOnly
    );
    // Case-insensitive "primary" per the implementation's eq_ignore_ascii_case.
    editor
        .kb_set_ai_residency("PRIMARY", mae_kb::federation::AiResidency::Open)
        .expect("case-insensitive primary should also succeed");
    assert_eq!(
        editor.kb.registry.primary_ai_residency,
        mae_kb::federation::AiResidency::Open
    );
}

#[test]
fn kb_set_ai_residency_via_command_line() {
    // Exercises the full `:kb-set-ai-residency <kb> <policy>` command-line path
    // (execute_command → dispatch_builtin → dispatch_kb), not just the Editor method
    // directly — the human/AI-tool/Scheme-primitive parity this command exists for is
    // only real if the command-line surface actually works end to end.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path()).unwrap();

    assert!(editor.execute_command("kb-set-ai-residency TestNotes local_models_only"));
    assert_eq!(
        editor.kb.registry.find("TestNotes").unwrap().ai_residency,
        mae_kb::federation::AiResidency::LocalModelsOnly
    );

    // Bad policy token -> usage message, no mutation, still "handled" (true).
    assert!(editor.execute_command("kb-set-ai-residency TestNotes not-a-real-policy"));
    assert_eq!(
        editor.kb.registry.find("TestNotes").unwrap().ai_residency,
        mae_kb::federation::AiResidency::LocalModelsOnly,
        "an invalid policy token must not silently change the stored policy"
    );
}

#[test]
fn kb_set_role_via_command_line() {
    // Exercises the full `:kb-set-role <node-id> <role>` command-line path
    // (execute_command → dispatch_builtin → dispatch_kb), matching the
    // kb-set-ai-residency parity test above.
    let mut editor = Editor::new();
    editor
        .kb_create_node(
            "note:role-cmdline-test",
            "Test",
            "body",
            mae_kb::NodeKind::Note,
        )
        .unwrap();

    assert!(editor.execute_command("kb-set-role note:role-cmdline-test atom"));
    assert_eq!(
        editor
            .kb
            .primary
            .get("note:role-cmdline-test")
            .unwrap()
            .properties
            .get("role"),
        Some(&"atom".to_string())
    );
}

#[test]
fn kb_set_ai_residency_unknown_kb_errors() {
    // Adversarial case: an unknown KB name must error, not silently no-op or succeed —
    // a caller relying on a false "success" for a typo'd KB name would believe a
    // sensitive KB is now protected when nothing was actually changed.
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_set_ai_residency(
        "does-not-exist",
        mae_kb::federation::AiResidency::LocalModelsOnly,
    );
    assert!(result.is_err());
}

#[test]
fn kb_reimport_refreshes_nodes() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();

    // Add a new file
    std::fs::write(
        dir.path().join("note3.org"),
        ":PROPERTIES:\n:ID: test-note-3\n:END:\n#+title: Note Three\n\nNew note.\n",
    )
    .unwrap();

    let result2 = editor.kb_reimport("TestNotes", None).unwrap();
    // Total nodes = imported (new) + updated (changed/existing)
    let total = result2.report.nodes_imported + result2.report.nodes_updated;
    assert_eq!(
        total, 3,
        "expected 3 total nodes (imported={}, updated={})",
        result2.report.nodes_imported, result2.report.nodes_updated
    );
    assert!(editor.kb.instances[&uuid].get("test-note-3").is_some());
}

#[test]
fn kb_reimport_refreshes_query_layer() {
    // Regression: kb_reimport used to update `self.kb.instances` but never
    // call `rebuild_query_layer()` (unlike kb_register/kb_unregister), so
    // kb-find — which reads through the query layer whenever one is active
    // (e.g. once a primary CozoDB store is open) — never saw reimported
    // nodes from federated instances until the process restarted.
    let mut editor = Editor::new();
    let primary = mae_kb::CozoKbStore::open_mem().unwrap();
    primary.seed_type_system().unwrap();
    let primary = std::sync::Arc::new(primary);
    editor.kb.primary_cozo = Some(primary.clone());
    editor.kb.store = Some(primary);
    editor.kb.rebuild_query_layer();
    assert!(
        editor.kb.query_layer().is_some(),
        "query layer must be active"
    );

    let dir = create_test_org_dir();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path()).unwrap();

    // New file added after registration, picked up by reimport.
    std::fs::write(
        dir.path().join("note3.org"),
        ":PROPERTIES:\n:ID: test-note-3\n:END:\n#+title: Note Three\n\nNew note.\n",
    )
    .unwrap();
    editor.kb_reimport("TestNotes", None).unwrap();

    let triples = editor
        .kb
        .query_layer()
        .unwrap()
        .id_title_body_triples(None, 500);
    assert!(
        triples.iter().any(|(id, _, _)| id == "test-note-3"),
        "reimported node must be visible through the query layer (kb-find's read path), got: {:?}",
        triples.iter().map(|(id, _, _)| id).collect::<Vec<_>>()
    );
}

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
    // non-persistent in-memory import — the exact bug reported against the
    // "arisnova" KB). With sqlite as the engine, a second handle must succeed
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
    assert_eq!(names, vec!["all", "local", "remote"]);
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

// --- #303: kb_promote_node (interim promote-to-native bridge) ---

#[test]
fn kb_promote_node_copies_into_primary() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();
    assert!(!editor.kb.primary.contains("test-note-1"));

    let result = editor.kb_promote_node("test-note-1").unwrap();
    assert_eq!(result.node_id, "test-note-1");
    assert_eq!(result.promoted_from_uuid, reg.uuid);

    let promoted = editor
        .kb
        .primary
        .get("test-note-1")
        .expect("node should now live in primary");
    assert_eq!(promoted.title, "Note One");
    assert!(promoted.body.contains("Body of note one."));
    assert!(
        promoted.source_file.is_none(),
        "promoted node must not carry the ephemeral source_file forward"
    );

    // Federated copy is left in place — no dedup-on-promote in v1.
    assert!(
        editor.kb.instances[&reg.uuid].get("test-note-1").is_some(),
        "the federated instance's own copy must remain discoverable"
    );
}

#[test]
fn kb_promote_node_preserves_provenance() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();
    editor.kb_promote_node("test-note-1").unwrap();

    let promoted = editor.kb.primary.get("test-note-1").unwrap();
    assert_eq!(
        promoted.properties.get("promoted_from_uuid"),
        Some(&reg.uuid)
    );
    assert_eq!(
        promoted.properties.get("promoted_from_org_dir"),
        Some(&dir.path().canonicalize().unwrap().display().to_string())
    );
    assert!(
        promoted
            .properties
            .get("promoted_from_path")
            .is_some_and(|p| p.ends_with("note1.org")),
        "promoted_from_path should point at the original file: {:?}",
        promoted.properties.get("promoted_from_path")
    );
    assert!(
        promoted
            .properties
            .get("promoted_at")
            .is_some_and(|s| !s.is_empty()),
        "promoted_at should be stamped"
    );
}

#[test]
fn kb_promote_node_leaves_org_file_untouched() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path()).unwrap();

    let file_path = dir.path().join("note1.org");
    let before = std::fs::read(&file_path).unwrap();
    editor.kb_promote_node("test-note-1").unwrap();
    let after = std::fs::read(&file_path).unwrap();

    assert_eq!(
        before, after,
        "promotion must never touch the original org file on disk"
    );
}

#[test]
fn kb_promote_node_rejects_already_primary() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path()).unwrap();
    editor.kb_promote_node("test-note-1").unwrap();
    let first_promoted_at = editor
        .kb
        .primary
        .get("test-note-1")
        .unwrap()
        .properties
        .get("promoted_at")
        .cloned();

    // Idempotency: a second promote call must not double-insert or
    // silently overwrite the first promotion's provenance.
    let second = editor.kb_promote_node("test-note-1");
    assert!(
        second.is_err(),
        "promoting an already-primary node must fail"
    );
    assert!(second.unwrap_err().contains("already in the primary KB"));
    assert_eq!(
        editor
            .kb
            .primary
            .get("test-note-1")
            .unwrap()
            .properties
            .get("promoted_at")
            .cloned(),
        first_promoted_at,
        "a rejected re-promote must not overwrite the original provenance"
    );
}

#[test]
fn kb_promote_node_rejects_unknown_id() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_promote_node("user:does-not-exist");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("No KB node"));
    assert!(!editor.kb.primary.contains("user:does-not-exist"));
}

#[test]
fn kb_import_result_json() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let json = result.to_json();
    assert!(json.contains("\"name\": \"TestNotes\""));
    assert!(json.contains("\"nodes_imported\": 2"));
}

#[test]
fn kb_create_node_inserts_into_local_kb() {
    let mut editor = Editor::new();
    let result = editor.kb_create_node(
        "user:test-note",
        "Test Note",
        "Hello",
        mae_kb::NodeKind::Note,
    );
    assert!(result.is_ok());
    let node = editor.kb.primary.get("user:test-note").unwrap();
    assert_eq!(node.title, "Test Note");
    assert_eq!(node.body, "Hello");
    assert_eq!(node.source, Some(mae_kb::NodeSource::Manual));
}

#[test]
fn kb_reimport_file_persists_to_instance_store() {
    // Phase 0b regression: kb_reimport_file must write THROUGH to the durable
    // instance store, not just the in-memory instance mirror — else a save-driven
    // reimport of a federated KB is lost on restart (same class as the :kb-ingest
    // durability bug). Oracle = the DURABLE store read, not the mirror.
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _td = with_test_dirs(&mut editor);
    let uuid = editor.kb_register("TestNotes", dir.path()).unwrap().uuid;
    // Write an org file AFTER registration so the reimport is what ingests it.
    let f = dir.path().join("fresh.org");
    std::fs::write(
        &f,
        ":PROPERTIES:\n:ID: reimport-durable-id\n:END:\n#+title: Reimport Me\n* H\nbody\n",
    )
    .unwrap();
    editor.kb_reimport_file(&f);
    // In-memory instance mirror has it...
    assert!(editor
        .kb
        .instances
        .get(&uuid)
        .unwrap()
        .get("reimport-durable-id")
        .is_some());
    // ...AND the durable instance store has it (the regression oracle).
    let durable = editor
        .kb
        .instance_stores
        .get(&uuid)
        .unwrap()
        .get_node("reimport-durable-id")
        .unwrap();
    assert!(
        durable.is_some(),
        "reimported node must be persisted to the durable instance store"
    );
    assert_eq!(durable.unwrap().title, "Reimport Me");
}

#[test]
fn kb_reimport_file_retracts_id_dropped_by_in_place_rename() {
    // Reproduces the reported bug end-to-end through the real editor path:
    // jenkinsp.org gets its :ID: hand-edited (jenkinsp -> jenkins) across
    // saves. Each save re-triggers kb_reimport_file (file_ops.rs); it must
    // retract the id the file no longer produces, in both the in-memory
    // instance mirror AND the durable instance store — not just upsert the
    // new one and leave the old as a ghost.
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _td = with_test_dirs(&mut editor);
    let uuid = editor.kb_register("TestNotes", dir.path()).unwrap().uuid;

    let f = dir.path().join("jenkinsp.org");
    std::fs::write(
        &f,
        ":PROPERTIES:\n:ID: user:t-jenkinsp\n:END:\n#+title: jenkinsp\n\nJenkins\n",
    )
    .unwrap();
    editor.kb_reimport_file(&f);
    assert!(editor
        .kb
        .instances
        .get(&uuid)
        .unwrap()
        .contains("user:t-jenkinsp"));

    // In-place rename, same path, then reimport again (as a save would trigger).
    std::fs::write(
        &f,
        ":PROPERTIES:\n:ID: user:t-jenkins\n:END:\n#+title: jenkins\n\nJenkins\n",
    )
    .unwrap();
    editor.kb_reimport_file(&f);

    let mirror = editor.kb.instances.get(&uuid).unwrap();
    assert!(
        !mirror.contains("user:t-jenkinsp"),
        "old id must be retracted from the in-memory mirror"
    );
    assert!(mirror.contains("user:t-jenkins"));

    let store = editor.kb.instance_stores.get(&uuid).unwrap();
    assert!(
        store.get_node("user:t-jenkinsp").unwrap().is_none(),
        "old id must be retracted from the durable instance store too"
    );
    assert!(store.get_node("user:t-jenkins").unwrap().is_some());
}

#[test]
fn kb_create_note_from_title_persists_durably_to_the_matching_instance() {
    // Reproduces the reported bug: a node created via SPC n f ("create new
    // node") must reach the durable instance store immediately, not just
    // the in-memory mirror -- otherwise there's no file-write for THIS
    // process's own instance-scoped search to see (until some later event
    // happens to reimport it) and nothing for any OTHER process sharing
    // this KB directory to ever pick up via its filesystem watcher.
    let dir = TempDir::new().unwrap();
    let mut editor = Editor::new();
    let _td = with_test_dirs(&mut editor);
    let uuid = editor.kb_register("TestNotes", dir.path()).unwrap().uuid;
    editor
        .set_option("kb_notes_dir", dir.path().to_str().unwrap())
        .unwrap();

    let (id, path) = editor.kb_create_note_from_title("My New Node").unwrap();
    let path = path.expect("kb_notes_dir is set, so a real file must be written");

    assert!(
        path.exists(),
        "the note must be written as a real .org file on disk"
    );
    assert!(
        editor.kb.instances.get(&uuid).unwrap().get(&id).is_some(),
        "in-memory instance mirror must have the node"
    );
    let durable = editor
        .kb
        .instance_stores
        .get(&uuid)
        .unwrap()
        .get_node(&id)
        .unwrap();
    assert!(
        durable.is_some(),
        "the durable instance store must have the node immediately, not just the mirror"
    );
}

#[test]
fn kb_create_note_from_title_visible_to_a_second_process_after_reimport() {
    // Simulates two independent mae processes sharing one KB directory:
    // two separate Editors, each registering the SAME directory. A node
    // created in "process A" must become visible in "process B" once B
    // re-ingests the file A wrote -- i.e. the write must actually be a
    // real file, findable by kb_find_candidates once picked up.
    let dir = TempDir::new().unwrap();

    let mut editor_a = Editor::new();
    let _td_a = with_test_dirs(&mut editor_a);
    editor_a.kb_register("Shared", dir.path()).unwrap();
    editor_a
        .set_option("kb_notes_dir", dir.path().to_str().unwrap())
        .unwrap();
    let (id, path) = editor_a
        .kb_create_note_from_title("Cross Process Node")
        .unwrap();
    let path = path.unwrap();

    let mut editor_b = Editor::new();
    let _td_b = with_test_dirs(&mut editor_b);
    let uuid_b = editor_b.kb_register("Shared", dir.path()).unwrap().uuid;
    // Process B's registration walk happened AFTER A's write, so a plain
    // register (not even a reimport) should already have it -- but drive
    // it through kb_reimport_file too, mirroring a watcher-driven pickup
    // of a file that changed after B's initial import.
    editor_b.kb_reimport_file(&path);

    assert!(
        editor_b
            .kb
            .instances
            .get(&uuid_b)
            .unwrap()
            .get(&id)
            .is_some(),
        "a node created in process A must become visible in process B \
             once B re-ingests the file -- it must have actually been written"
    );
}

#[test]
fn kb_mutations_refuse_when_store_unavailable() {
    // Phase 0c: when the durable store failed to open, mutations must refuse with
    // an actionable error instead of silently writing to a mirror that never
    // persists. The negative case that MUST fail (principle #14).
    let mut editor = Editor::new();
    editor.kb.store_unavailable = true;
    let e = editor
        .kb_create_node("user:x", "X", "b", mae_kb::NodeKind::Note)
        .unwrap_err();
    assert!(e.contains("unavailable"), "create must refuse: {e}");
    let e = editor
        .kb_update_node("user:x", Some("Y"), None, None)
        .unwrap_err();
    assert!(e.contains("unavailable"), "update must refuse: {e}");
    let e = editor.kb_delete_node("user:x").unwrap_err();
    assert!(e.contains("unavailable"), "delete must refuse: {e}");
    // And nothing leaked into the mirror.
    assert!(editor.kb.primary.get("user:x").is_none());
}

#[test]
fn drain_kb_preload_populates_mirror_from_background_channel() {
    // Phase 1a: the idle-tick drain consumes the background loader's node set and
    // populates the mirror, then clears the pending channel.
    let mut editor = Editor::new();
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(Ok(vec![mae_kb::Node::new(
        "preload:1",
        "One",
        mae_kb::NodeKind::Note,
        "b",
    )]))
    .unwrap();
    editor.kb.pending_preload = Some(rx);
    assert!(editor.kb.primary.get("preload:1").is_none());
    editor.drain_kb_preload();
    assert!(
        editor.kb.primary.get("preload:1").is_some(),
        "preload must populate the mirror"
    );
    assert!(
        editor.kb.pending_preload.is_none(),
        "channel cleared once drained"
    );
}

#[test]
fn drain_kb_preload_is_noop_while_still_loading() {
    // Empty channel = loader still running: drain must be a no-op and keep the
    // pending handle so the next tick retries.
    let mut editor = Editor::new();
    let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<mae_kb::Node>, String>>();
    editor.kb.pending_preload = Some(rx);
    editor.drain_kb_preload();
    assert!(
        editor.kb.pending_preload.is_some(),
        "still-loading must remain pending"
    );
    drop(tx);
}

#[test]
fn sqlite_multi_instance_concurrent_writes_converge() {
    // Phase 2 hard gate (adversarial, #14): two CozoKbStore handles on the SAME
    // sqlite file — two DbInstances → two independent process-local locks, the same
    // lock topology as two daemon-less processes. cozo 0.7 sets no busy_timeout, so
    // without the busy-retry ~14% of concurrent writes fail with SQLITE_BUSY. This
    // asserts that with the retry, N-way concurrent writers ALL succeed and the
    // store converges to the union of their writes (no lost writes, no corruption).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.sqlite");
    let a = std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    a.seed_type_system().unwrap();
    let b = std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());

    // Cross-visibility of sequential writes.
    a.insert_node(&mae_kb::Node::new(
        "a:seq",
        "A",
        mae_kb::NodeKind::Note,
        "x",
    ))
    .unwrap();
    b.insert_node(&mae_kb::Node::new(
        "b:seq",
        "B",
        mae_kb::NodeKind::Note,
        "x",
    ))
    .unwrap();
    assert!(
        a.get_node("b:seq").unwrap().is_some(),
        "A must see B's write"
    );
    assert!(
        b.get_node("a:seq").unwrap().is_some(),
        "B must see A's write"
    );

    // Concurrent writers on disjoint id sets — every write MUST succeed.
    let n = 50;
    let mk = |store: std::sync::Arc<mae_kb::CozoKbStore>, prefix: &'static str| {
        std::thread::spawn(move || {
            for i in 0..n {
                store
                    .insert_node(&mae_kb::Node::new(
                        format!("{prefix}:{i}"),
                        prefix,
                        mae_kb::NodeKind::Note,
                        "x",
                    ))
                    .unwrap_or_else(|e| {
                        panic!("{prefix}:{i} write must not fail under contention: {e}")
                    });
            }
        })
    };
    let ta = mk(a.clone(), "wa");
    let tb = mk(b.clone(), "wb");
    ta.join().unwrap();
    tb.join().unwrap();

    // Convergence: both writers' full id sets are present + readable from either handle.
    for i in 0..n {
        assert!(
            a.get_node(&format!("wa:{i}")).unwrap().is_some()
                && a.get_node(&format!("wb:{i}")).unwrap().is_some(),
            "store must converge to the union of both writers' nodes"
        );
    }
}

#[test]
fn external_store_change_arms_a_background_reload() {
    // Phase 4: when another process commits to the shared sqlite store, the store
    // watcher fires and `drain_kb_store_watch` arms a background mirror reload.
    let mut editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.cozo");
    let store =
        std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    store.seed_type_system().unwrap();
    editor.kb.primary_cozo = Some(store.clone());
    editor.kb.store_watcher = Some(mae_kb::watch::StoreWatcher::new(&path).unwrap());
    assert!(
        editor.kb.last_local_store_write.is_none(),
        "no cooldown active"
    );

    // Another "process" commits to the store (modifies the file).
    store
        .insert_node(&mae_kb::Node::new(
            "user:ext",
            "Ext",
            mae_kb::NodeKind::Note,
            "b",
        ))
        .unwrap();

    // Poll: the external change must arm a background reload (notify is async).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut armed = false;
    while std::time::Instant::now() < deadline {
        editor.drain_kb_store_watch();
        if editor.kb.pending_preload.is_some() {
            armed = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        armed,
        "external store change must arm a background mirror reload"
    );
}

#[test]
fn store_watch_reload_suppressed_within_local_write_cooldown() {
    // Phase 4: a reload must NOT fire when WE just wrote (cooldown) — otherwise
    // local edits would churn the mirror. With a fresh local-write timestamp and a
    // changed store, drain must leave `pending_preload` unset.
    let mut editor = Editor::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("primary.cozo");
    let store =
        std::sync::Arc::new(mae_kb::CozoKbStore::open_with_engine(&path, "sqlite").unwrap());
    store.seed_type_system().unwrap();
    editor.kb.primary_cozo = Some(store.clone());
    editor.kb.store_watcher = Some(mae_kb::watch::StoreWatcher::new(&path).unwrap());
    // Pretend WE just wrote.
    editor.kb.last_local_store_write = Some(std::time::Instant::now());

    store
        .insert_node(&mae_kb::Node::new(
            "user:x",
            "X",
            mae_kb::NodeKind::Note,
            "b",
        ))
        .unwrap();

    // Give notify time to deliver, draining each tick; the cooldown must keep the
    // reload from arming for the whole window.
    for _ in 0..20 {
        editor.drain_kb_store_watch();
        assert!(
            editor.kb.pending_preload.is_none(),
            "reload must be suppressed within the local-write cooldown"
        );
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

#[test]
fn external_registry_change_adopts_new_instance() {
    // Live-refresh (section D): another mae process registers a KB
    // org-dir directly against the shared kb-registry.toml; this
    // process's registry watcher must pick it up on a later idle tick
    // without any local KB operation being run first.
    let mut editor = Editor::new();
    let data_dir = tempfile::tempdir().unwrap();
    editor.data_dir_override = Some(data_dir.path().to_path_buf());

    // Seed an empty registry file so StoreWatcher has something to watch
    // (mirrors the real startup wiring in main.rs).
    mae_kb::federation::KbRegistry::default()
        .save(data_dir.path())
        .unwrap();
    editor.kb.registry_watcher =
        Some(mae_kb::watch::StoreWatcher::new(data_dir.path().join("kb-registry.toml")).unwrap());
    assert!(
        editor.kb.last_local_registry_write.is_none(),
        "no cooldown active"
    );

    // "Another process" registers a KB directly against the same data dir
    // — this editor's in-memory registry never saw it.
    let org_dir = tempfile::tempdir().unwrap();
    let (_, uuid, saved) = mae_kb::federation::KbRegistry::update(data_dir.path(), |reg| {
        reg.register(
            "External".to_string(),
            org_dir.path().to_path_buf(),
            data_dir.path(),
            None,
        )
    });
    saved.unwrap();

    // Poll: the external change must get adopted (notify is async).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut adopted = false;
    while std::time::Instant::now() < deadline {
        editor.drain_kb_registry_watch();
        if editor.kb.instances.contains_key(&uuid) {
            adopted = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        adopted,
        "external KB registration must be adopted via the registry watcher"
    );
    assert!(
        editor.kb.registry.find(&uuid).is_some(),
        "in-memory registry must also reflect the externally-registered instance"
    );
}

#[test]
fn registry_watch_reload_suppressed_within_local_write_cooldown() {
    // A reload must NOT fire when WE just wrote (cooldown) — otherwise a
    // command that just registered/unregistered a KB would immediately
    // re-adopt/re-scan against its own fresh write.
    let mut editor = Editor::new();
    let data_dir = tempfile::tempdir().unwrap();
    editor.data_dir_override = Some(data_dir.path().to_path_buf());
    mae_kb::federation::KbRegistry::default()
        .save(data_dir.path())
        .unwrap();
    editor.kb.registry_watcher =
        Some(mae_kb::watch::StoreWatcher::new(data_dir.path().join("kb-registry.toml")).unwrap());
    // Pretend WE just wrote.
    editor.kb.last_local_registry_write = Some(std::time::Instant::now());

    let org_dir = tempfile::tempdir().unwrap();
    let (_, uuid, saved) = mae_kb::federation::KbRegistry::update(data_dir.path(), |reg| {
        reg.register(
            "External".to_string(),
            org_dir.path().to_path_buf(),
            data_dir.path(),
            None,
        )
    });
    saved.unwrap();

    for _ in 0..20 {
        editor.drain_kb_registry_watch();
        assert!(
            !editor.kb.instances.contains_key(&uuid),
            "reload must be suppressed within the local-write cooldown"
        );
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

#[test]
fn kb_agenda_routes_through_query_layer() {
    // Phase 3: :kb-agenda must resolve via the query layer (uniform read path in
    // both daemon modes), returning the same TODO set as a direct store query.
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    let mut n = mae_kb::Node::new("user:task1", "Do the thing", mae_kb::NodeKind::Note, "b");
    n.todo_state = Some("TODO".to_string());
    store.insert_node(&n).unwrap();
    let arc = std::sync::Arc::new(store);
    editor.kb.primary_cozo = Some(arc.clone());
    editor.kb.store = Some(arc.clone());
    editor.kb.rebuild_query_layer();

    let direct = arc.agenda_query(&mae_kb::AgendaFilter::Todo(None)).unwrap();
    let via_ql = editor
        .kb
        .query_layer()
        .unwrap()
        .agenda(&mae_kb::AgendaFilter::Todo(None));
    let direct_ids: Vec<String> = direct.iter().map(|n| n.id.clone()).collect();
    let ql_ids: Vec<String> = via_ql.iter().map(|n| n.id.clone()).collect();
    assert_eq!(
        direct_ids,
        vec!["user:task1".to_string()],
        "store has the TODO"
    );
    assert_eq!(
        direct_ids, ql_ids,
        "query-layer agenda must match the store's agenda"
    );
}

#[test]
fn kb_history_routes_through_query_layer() {
    // Phase 3: :kb-history routing parity — the query layer returns the same
    // version set as a direct store query (routing property, whatever the count).
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    let n = mae_kb::Node::new("user:h1", "V1", mae_kb::NodeKind::Note, "b1");
    store.insert_node(&n).unwrap();
    let mut n2 = n.clone();
    n2.body = "b2".to_string();
    store.update_node(&n2).unwrap();
    let arc = std::sync::Arc::new(store);
    editor.kb.primary_cozo = Some(arc.clone());
    editor.kb.store = Some(arc.clone());
    editor.kb.rebuild_query_layer();

    let direct = arc.node_history("user:h1", 50).unwrap();
    let via_ql = editor.kb.query_layer().unwrap().history("user:h1", 50);
    assert_eq!(
        via_ql.iter().map(|v| v.version).collect::<Vec<_>>(),
        direct.iter().map(|v| v.version).collect::<Vec<_>>(),
        "query-layer history must match the store's history"
    );
}

#[test]
fn kb_create_node_rejects_seed_overwrite() {
    let mut editor = Editor::new();
    // "index" is a seed node
    let result = editor.kb_create_node("index", "Override", "bad", mae_kb::NodeKind::Note);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("seed node"));
}

// #165: a node whose id is prefixed with a REGISTERED instance's name must be created
// in THAT federated instance, not the primary KB. Before the fix `kb_create_node`
// hard-coded owner=None, so every create landed in primary — its `kb_collab_id_of`
// resolved to None, the broadcast gate never fired, and a node added to a shared
// instance never synced to the daemon.
#[test]
fn kb_create_node_routes_an_instance_prefixed_id_to_that_instance() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let uuid = editor.kb_register("TestNotes", dir.path()).unwrap().uuid;

    editor
        .kb_create_node("TestNotes:fresh", "Fresh", "hi", mae_kb::NodeKind::Note)
        .unwrap();
    assert!(
        editor
            .kb
            .instances
            .get(&uuid)
            .unwrap()
            .get("TestNotes:fresh")
            .is_some(),
        "instance-prefixed create lands in the registered instance"
    );
    assert!(
        editor.kb.primary.get("TestNotes:fresh").is_none(),
        "and NOT in primary (the #165 bug: owner=None → primary → never syncs)"
    );
    assert_eq!(
        editor.kb_owner_of("TestNotes:fresh"),
        Some(Some(uuid.clone())),
        "owner resolves to the instance (vs None before — which left the gate dead)"
    );

    // An unregistered prefix (a primary namespace like `concept:`) stays in primary.
    editor
        .kb_create_node("concept:x", "C", "c", mae_kb::NodeKind::Note)
        .unwrap();
    assert!(
        editor.kb.primary.get("concept:x").is_some(),
        "an unregistered prefix stays in the primary KB"
    );
    assert!(editor
        .kb
        .instances
        .get(&uuid)
        .unwrap()
        .get("concept:x")
        .is_none());
}

#[test]
fn kb_delete_node_removes_from_local_kb() {
    let mut editor = Editor::new();
    editor
        .kb_create_node("user:del-me", "Delete Me", "bye", mae_kb::NodeKind::Note)
        .unwrap();
    assert!(editor.kb.primary.get("user:del-me").is_some());
    let result = editor.kb_delete_node("user:del-me");
    assert!(result.is_ok());
    assert!(editor.kb.primary.get("user:del-me").is_none());
}

#[test]
fn kb_delete_node_rejects_seed_deletion() {
    let mut editor = Editor::new();
    let result = editor.kb_delete_node("index");
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("seed node"));
    // Confirm the node still exists
    assert!(editor.kb.primary.get("index").is_some());
}

#[test]
fn kb_update_node_merges_fields() {
    let mut editor = Editor::new();
    editor
        .kb_create_node(
            "user:upd",
            "Original",
            "original body",
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    let result = editor.kb_update_node(
        "user:upd",
        Some("Updated Title"),
        None,
        Some(vec!["tag1".into()]),
    );
    assert!(result.is_ok());
    let node = editor.kb.primary.get("user:upd").unwrap();
    assert_eq!(node.title, "Updated Title");
    assert_eq!(node.body, "original body"); // unchanged
    assert_eq!(node.tags, vec!["tag1".to_string()]);
}

/// I-9: a node that lives in a federated *instance* (not `primary`) — the
/// shape on the host that registered a shared KB — must be editable via
/// `kb_update_node`, not rejected with "No KB node" (the original
/// primary-only resolution bug).
#[test]
fn kb_update_node_resolves_federated_instance() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    let mut n = mae_kb::Node::new(
        "collabtest:overview",
        "Overview",
        mae_kb::NodeKind::Note,
        "body",
    );
    n.source = Some(mae_kb::NodeSource::Federation);
    inst.insert(n);
    editor.kb.instances.insert("uuid-collabtest".into(), inst);

    // Not in primary — only in the instance.
    assert!(editor.kb.primary.get("collabtest:overview").is_none());
    let res = editor.kb_update_node(
        "collabtest:overview",
        Some("Overview v2"),
        Some("new body"),
        None,
    );
    assert!(
        res.is_ok(),
        "instance node must resolve for update: {res:?}"
    );
    let updated = editor
        .kb
        .instances
        .get("uuid-collabtest")
        .and_then(|kb| kb.get("collabtest:overview"))
        .expect("node still in instance");
    assert_eq!(updated.title, "Overview v2");
    assert_eq!(updated.body, "new body");
}

/// ADR-019: editing an instance node whose KB carries a DURABLE share marker
/// must queue a CRDT update for broadcast — **even with `shared_kbs` empty**
/// (the exact editor-restart scenario: the transient cache is gone but the
/// registry marker survives, so the emit gate still fires).
#[test]
fn kb_update_node_shared_instance_queues_crdt_update() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    let mut n = mae_kb::Node::new(
        "collabtest:alpha",
        "Alpha",
        mae_kb::NodeKind::Note,
        "alpha body",
    );
    n.source = Some(mae_kb::NodeSource::Federation);
    inst.insert(n);
    editor.kb.instances.insert("uuid-collabtest".into(), inst);

    // Durable marker only (registry), NOT the transient shared_kbs cache.
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: "uuid-collabtest".into(),
            name: "collabtest".into(),
            org_dir: std::path::PathBuf::from("/tmp/collabtest"),
            db_path: std::path::PathBuf::from("/tmp/collabtest.db"),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: Some("collabtest".into()),
            shared: true,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
        });
    editor.collab.kb_sync_mode = "on_save".into();
    assert!(
        editor.collab.shared_kbs.is_empty(),
        "gate must fire from the durable marker, not the cache"
    );

    assert!(editor.collab.pending_kb_updates.is_empty());
    editor
        .kb_update_node("collabtest:alpha", None, Some("edited"), None)
        .unwrap();
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "edit to a durably-shared instance node must queue a kb/node_update"
    );
    let (kb_id, node_id, _bytes) = &editor.collab.pending_kb_updates[0];
    assert_eq!(kb_id, "collabtest");
    assert_eq!(node_id, "collabtest:alpha");
}

/// ADR-019: with the durable marker ABSENT (instance not shared), an edit
/// must NOT broadcast — even if a stale `shared_kbs` cache entry exists.
#[test]
fn kb_update_node_unshared_instance_does_not_queue() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    let mut n = mae_kb::Node::new("local:x", "X", mae_kb::NodeKind::Note, "b");
    n.source = Some(mae_kb::NodeSource::Federation);
    inst.insert(n);
    editor.kb.instances.insert("uuid-local".into(), inst);
    // Registry instance exists but is NOT shared.
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: "uuid-local".into(),
            name: "local".into(),
            org_dir: std::path::PathBuf::from("/tmp/local"),
            db_path: std::path::PathBuf::from("/tmp/local.db"),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
        });
    editor.collab.kb_sync_mode = "on_save".into();
    // A stale cache entry must NOT be trusted as authority.
    let mut nodes = std::collections::HashSet::new();
    nodes.insert("local:x".to_string());
    editor.collab.shared_kbs.insert("local".into(), nodes);

    editor
        .kb_update_node("local:x", None, Some("edited"), None)
        .unwrap();
    assert!(
        editor.collab.pending_kb_updates.is_empty(),
        "unshared KB must not broadcast even with a stale cache entry"
    );
}

/// Phase D (ADR-029): when the daemon hosts the primary, a primary-node edit
/// must queue a CRDT update under the canonical "default" collab id — even
/// though the user never ran `kb-share` (durable `primary_shared` stays false).
#[test]
fn kb_update_node_daemon_hosted_primary_queues_under_default() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:alpha",
        "Alpha",
        mae_kb::NodeKind::Note,
        "body",
    ));
    editor.collab.kb_sync_mode = "on_save".into();
    // Daemon hosts the primary at runtime; no durable peer-share marker.
    editor.kb.set_daemon_hosts_primary(true);
    assert!(!editor.kb.registry.primary_shared);

    assert!(editor.collab.pending_kb_updates.is_empty());
    editor
        .kb_update_node("note:alpha", None, Some("edited"), None)
        .unwrap();
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "daemon-hosted primary edit must queue a kb/node_update"
    );
    let (kb_id, node_id, _bytes) = &editor.collab.pending_kb_updates[0];
    assert_eq!(kb_id, crate::editor::KB_DEFAULT_NAME);
    assert_eq!(node_id, "note:alpha");
    // Hosting is runtime-only — it must NOT have stamped the durable marker.
    assert!(
        !editor.kb.registry.primary_shared,
        "daemon-hosting must not durably mark the primary as peer-shared"
    );
}

/// Phase D: with the daemon NOT hosting and no durable share, a primary edit
/// stays local (no broadcast) — today's embedded behavior is unchanged.
#[test]
fn kb_update_node_unhosted_primary_does_not_queue() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:beta",
        "Beta",
        mae_kb::NodeKind::Note,
        "body",
    ));
    editor.collab.kb_sync_mode = "on_save".into();
    assert!(!editor.kb.daemon_hosts_primary());
    assert!(!editor.kb.registry.primary_shared);

    editor
        .kb_update_node("note:beta", None, Some("edited"), None)
        .unwrap();
    assert!(
        editor.collab.pending_kb_updates.is_empty(),
        "un-hosted, un-shared primary edit must not queue"
    );
}

/// Phase D: `refresh_daemon_host_state` is the single writer of the runtime
/// flag and requires BOTH the opt-in option and a live daemon connection.
#[test]
fn refresh_daemon_host_state_requires_optin_and_connection() {
    let mut editor = Editor::new();
    // Force the flag on, then prove refresh clears it without the preconditions.
    editor.kb.set_daemon_hosts_primary(true);
    editor.kb.daemon_default = false;
    editor.refresh_daemon_host_state();
    assert!(!editor.kb.daemon_hosts_primary(), "no opt-in ⇒ not hosting");

    // Opt in, but with no daemon read layer / not Connected ⇒ still not hosting.
    editor.kb.daemon_default = true;
    editor.refresh_daemon_host_state();
    assert!(
        !editor.kb.daemon_hosts_primary(),
        "opt-in without a connected daemon ⇒ not hosting"
    );
}

/// I-9: deleting an instance node must resolve it (not "No KB node").
#[test]
fn kb_delete_node_resolves_federated_instance() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    let mut n = mae_kb::Node::new("collabtest:beta", "Beta", mae_kb::NodeKind::Note, "b");
    n.source = Some(mae_kb::NodeSource::Federation);
    inst.insert(n);
    editor.kb.instances.insert("uuid-collabtest".into(), inst);

    let res = editor.kb_delete_node("collabtest:beta");
    assert!(
        res.is_ok(),
        "instance node must resolve for delete: {res:?}"
    );
    assert!(editor
        .kb
        .instances
        .get("uuid-collabtest")
        .and_then(|kb| kb.get("collabtest:beta"))
        .is_none());
}

/// Phase D1.1: creating a node on a daemon-hosted primary must emit the node
/// doc AND a collection-manifest add (so the projector materializes it — not
/// just on first edit).
#[test]
fn kb_create_node_daemon_hosted_emits_doc_and_manifest_add() {
    let mut editor = Editor::new();
    editor.collab.kb_sync_mode = "on_save".into();
    editor.kb.set_daemon_hosts_primary(true);

    assert!(editor.collab.pending_kb_updates.is_empty());
    assert!(editor.collab.pending_kb_manifest.is_empty());
    editor
        .kb_create_node("note:new", "New", "body", mae_kb::NodeKind::Note)
        .unwrap();

    // Node doc enqueued (transient queue — no durable store in a unit test).
    assert_eq!(editor.collab.pending_kb_updates.len(), 1);
    assert_eq!(
        editor.collab.pending_kb_updates[0].0,
        crate::editor::KB_DEFAULT_NAME
    );
    assert_eq!(editor.collab.pending_kb_updates[0].1, "note:new");
    // Manifest add enqueued (kb_id, node_id, title, add=true).
    assert_eq!(editor.collab.pending_kb_manifest.len(), 1);
    let (kb_id, node_id, title, add) = &editor.collab.pending_kb_manifest[0];
    assert_eq!(kb_id, crate::editor::KB_DEFAULT_NAME);
    assert_eq!(node_id, "note:new");
    assert_eq!(title, "New");
    assert!(*add);
    // And the node exists in the in-memory primary KB.
    assert!(editor.kb.primary.get("note:new").is_some());
}

/// Phase D1.1: with no daemon hosting, a create stays local — no CRDT/manifest
/// traffic (embedded behavior unchanged).
#[test]
fn kb_create_node_unhosted_stays_local() {
    let mut editor = Editor::new();
    editor.collab.kb_sync_mode = "on_save".into();
    editor
        .kb_create_node("note:loc", "Local", "body", mae_kb::NodeKind::Note)
        .unwrap();
    assert!(editor.collab.pending_kb_updates.is_empty());
    assert!(editor.collab.pending_kb_manifest.is_empty());
    assert!(editor.kb.primary.get("note:loc").is_some());
}

/// Phase D1.1: deleting a node on a daemon-hosted primary enqueues a
/// collection-manifest remove (so the projector drops it from cozo).
#[test]
fn kb_delete_node_daemon_hosted_enqueues_manifest_remove() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "note:del",
        "Del",
        mae_kb::NodeKind::Note,
        "b",
    ));
    editor.collab.kb_sync_mode = "on_save".into();
    editor.kb.set_daemon_hosts_primary(true);

    editor.kb_delete_node("note:del").unwrap();
    assert_eq!(editor.collab.pending_kb_manifest.len(), 1);
    let (kb_id, node_id, _title, add) = &editor.collab.pending_kb_manifest[0];
    assert_eq!(kb_id, crate::editor::KB_DEFAULT_NAME);
    assert_eq!(node_id, "note:del");
    assert!(!*add, "delete must enqueue a manifest REMOVE");
    assert!(editor.kb.primary.get("note:del").is_none());
}

/// Phase D3: on a thin startup (mirror NOT preloaded) the daemon-hosted edit
/// path must lazily load the node — with its persisted CRDT lineage — from the
/// open store, so the edit resolves + chains onto the shared lineage.
#[test]
fn kb_update_node_lazily_loads_from_store_when_daemon_hosted() {
    let mut editor = Editor::new();
    // A store holding a node that is NOT in the in-memory mirror.
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    store
        .insert_node(&mae_kb::Node::new(
            "note:lazy",
            "Lazy",
            mae_kb::NodeKind::Note,
            "orig body",
        ))
        .unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));
    editor.kb.set_primary_thin(true);
    editor.kb.set_daemon_hosts_primary(true);
    editor.collab.kb_sync_mode = "on_save".into();
    // Thin startup: the mirror is empty.
    assert!(editor.kb.primary.get("note:lazy").is_none());

    // Editing must lazily load the node from the store, then apply the edit.
    editor
        .kb_update_node("note:lazy", None, Some("edited body"), None)
        .unwrap();
    let n = editor
        .kb
        .primary
        .get("note:lazy")
        .expect("node lazily loaded into mirror");
    assert_eq!(n.body, "edited body");
}

/// Phase D (#118): on a thin primary the in-memory mirror is empty, so federated
/// search must source the primary's ranked hits + owned nodes from the query layer
/// (daemon LRU), not from `kb.primary`. Without the routing the agenda's sibling
/// surface — search — silently returns nothing under a daemon-hosted primary.
#[test]
fn federated_search_routes_primary_via_query_layer_when_thin() {
    let mut editor = Editor::new();
    let store = std::sync::Arc::new(mae_kb::CozoKbStore::open_mem().unwrap());
    store.seed_type_system().unwrap();
    store
        .insert_node(&mae_kb::Node::new(
            "note:thin",
            "Findable Thin Node",
            mae_kb::NodeKind::Note,
            "body",
        ))
        .unwrap();
    // Inject the store as the daemon query layer + mark the primary thin.
    editor
        .kb
        .set_daemon_query_layer(Some(std::sync::Arc::new(mae_kb::CozoQueryLayer::new(
            store,
        ))));
    editor.kb.set_primary_thin(true);

    // The in-memory mirror is empty...
    assert!(editor.kb.primary.get("note:thin").is_none());
    // ...but federated search still finds the node, routed via the query layer.
    let results = editor.kb_federated_search("findable");
    assert!(
        results.iter().any(|(_, n)| n.id == "note:thin"),
        "thin-primary search must route through the query layer"
    );
}

/// Phase D3c: the pre-connect window — a thin mirror with the daemon read layer
/// up but the collab WRITE channel NOT yet connected (`daemon_hosts_primary`
/// false). Hydration must still fire (gated on `primary_thin`), so an edit
/// resolves instead of failing with "No KB node".
#[test]
fn kb_update_node_hydrates_in_pre_connect_window() {
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    store
        .insert_node(&mae_kb::Node::new(
            "note:pc",
            "PC",
            mae_kb::NodeKind::Note,
            "orig",
        ))
        .unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));
    editor.kb.set_primary_thin(true); // thin mirror...
    assert!(!editor.kb.daemon_hosts_primary()); // ...but collab NOT connected yet
    editor.collab.kb_sync_mode = "on_save".into();

    editor
        .kb_update_node("note:pc", None, Some("edited"), None)
        .expect("edit must resolve in the pre-connect window");
    assert_eq!(editor.kb.primary.get("note:pc").unwrap().body, "edited");
}

/// Phase D3: when the mirror is NOT thin (full preload, no daemon), the lazy-load
/// helper is inert — a missing node stays missing (today's embedded behavior).
#[test]
fn kb_ensure_node_loaded_inert_when_mirror_not_thin() {
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    store
        .insert_node(&mae_kb::Node::new(
            "note:x",
            "X",
            mae_kb::NodeKind::Note,
            "b",
        ))
        .unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));
    // primary_thin is false (default — full preload).
    editor.kb_ensure_node_loaded("note:x");
    assert!(
        editor.kb.primary.get("note:x").is_none(),
        "no lazy load when the mirror isn't thin"
    );
}

/// Phase D3b: while the daemon hosts the primary, the per-edit local
/// write-through is RETIRED (the daemon is the source of truth); snapshot-back
/// then persists the mirror to the store for the daemon-less fallback.
#[test]
fn kb_persist_retired_when_hosted_then_snapshot_restores() {
    let mut editor = Editor::new();
    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    editor.kb.store = Some(std::sync::Arc::new(store));
    editor.kb.set_daemon_hosts_primary(true);
    editor.collab.kb_sync_mode = "on_save".into();

    // Create a node while hosted: it enters the mirror + the daemon queue, but
    // the per-edit write-through is retired ⇒ the local store does NOT have it.
    editor
        .kb_create_node("note:r", "R", "body", mae_kb::NodeKind::Note)
        .unwrap();
    assert!(editor.kb.primary.get("note:r").is_some(), "node in mirror");
    assert!(
        editor
            .kb
            .store
            .as_ref()
            .unwrap()
            .get_node("note:r")
            .unwrap()
            .is_none(),
        "retire: per-edit write-through skipped while daemon-hosted"
    );

    // Snapshot-back persists the mirror → store (the daemon-less fallback).
    editor.kb_snapshot_primary_to_store();
    assert!(
        editor
            .kb
            .store
            .as_ref()
            .unwrap()
            .get_node("note:r")
            .unwrap()
            .is_some(),
        "snapshot-back persists the mirror to the store"
    );
}

/// Helper: a registry instance marked shared (uuid = "uuid-ct", collab_id =
/// "collabtest").
fn shared_ct_instance() -> mae_kb::federation::KbInstance {
    mae_kb::federation::KbInstance {
        uuid: "uuid-ct".into(),
        name: "collabtest".into(),
        org_dir: std::path::PathBuf::new(),
        db_path: std::path::PathBuf::new(),
        primary: false,
        enabled: true,
        last_import: None,
        collab_id: Some("collabtest".into()),
        shared: true,
        remote_peers: Vec::new(),
        last_sync: None,
        ai_residency: mae_kb::federation::AiResidency::default(),
    }
}

/// ADR-019 receive-side: a remote update for a *new* node routes to the
/// owning instance (via the collab_id hint), NOT primary.
#[test]
fn kb_apply_remote_update_routes_new_node_to_instance() {
    let mut editor = Editor::new();
    editor
        .kb
        .instances
        .insert("uuid-ct".into(), mae_kb::KnowledgeBase::new());
    editor.kb.registry.instances.push(shared_ct_instance());

    // Build a remote CRDT update from a separate KB (client_id 2 = "remote").
    let mut remote = mae_kb::KnowledgeBase::new();
    let update = remote
        .upsert_with_crdt(
            mae_kb::Node::new("collabtest:newnode", "T", mae_kb::NodeKind::Note, "b"),
            2,
        )
        .unwrap();

    let changed = editor
        .kb_apply_remote_update("collabtest:newnode", &update, Some("collabtest"))
        .unwrap();
    assert!(changed, "a new remote node must be created");
    assert!(
        editor.kb.instances["uuid-ct"]
            .get("collabtest:newnode")
            .is_some(),
        "remote node must route to the owning instance"
    );
    assert!(
        editor.kb.primary.get("collabtest:newnode").is_none(),
        "remote node must NOT land in primary"
    );
}

/// ADR-019 receive-side: a remote update for an *existing* instance node is
/// applied in that instance and never copied into primary.
#[test]
fn kb_apply_remote_update_existing_node_stays_in_instance() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    let mut n = mae_kb::Node::new("collabtest:overview", "Old", mae_kb::NodeKind::Note, "old");
    n.source = Some(mae_kb::NodeSource::Federation);
    inst.insert(n);
    editor.kb.instances.insert("uuid-ct".into(), inst);
    editor.kb.registry.instances.push(shared_ct_instance());

    let mut remote = mae_kb::KnowledgeBase::new();
    let update = remote
        .upsert_with_crdt(
            mae_kb::Node::new(
                "collabtest:overview",
                "Updated",
                mae_kb::NodeKind::Note,
                "updated",
            ),
            2,
        )
        .unwrap();

    editor
        .kb_apply_remote_update("collabtest:overview", &update, None)
        .unwrap();
    assert!(
        editor.kb.instances["uuid-ct"]
            .get("collabtest:overview")
            .is_some(),
        "node stays in the owning instance"
    );
    assert!(
        editor.kb.primary.get("collabtest:overview").is_none(),
        "remote update must not copy the node into primary"
    );
}

/// ADR-020 Phase 2: a joined KB is registered + nodes MERGED via
/// apply_remote_update (not insert-overwritten). A re-join is idempotent:
/// the same instance is reused, the node is kept (merged), and the registry
/// has exactly one entry for the collab id.
#[test]
fn kb_register_joined_instance_merges_and_is_idempotent() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let mut remote = mae_kb::KnowledgeBase::new();
    let state = remote
        .upsert_with_crdt(
            mae_kb::Node::new("ct:overview", "V0", mae_kb::NodeKind::Note, "b0"),
            2,
        )
        .unwrap();

    let sv = remote.node_state_vector("ct:overview").unwrap();
    let join_node = |bytes: Vec<u8>| {
        vec![crate::editor::JoinedNode {
            id: "ct:overview".to_string(),
            bytes,
            daemon_sv: Some(sv.clone()),
        }]
    };

    let uuid = editor.kb_register_joined_instance("ct", join_node(state.clone()));
    assert!(
        editor.kb.instances[&uuid].get("ct:overview").is_some(),
        "first join creates the node in its instance"
    );
    // Joined node lives in the instance, never primary.
    assert!(editor.kb.primary.get("ct:overview").is_none());

    // Re-join with the same state — reconcile MERGES (idempotent),
    // does not crash, reuses the instance, keeps the node, no duplicate.
    let uuid2 = editor.kb_register_joined_instance("ct", join_node(state));
    assert_eq!(uuid2, uuid, "re-join reuses the same instance");
    assert!(editor.kb.instances[&uuid].get("ct:overview").is_some());
    assert_eq!(
        editor
            .kb
            .registry
            .instances
            .iter()
            .filter(|i| i.collab_id.as_deref() == Some("ct"))
            .count(),
        1,
        "exactly one registry instance for the collab id"
    );
}

/// B3 (collab test-gap plan): a joined KB instance must SURFACE to the user.
/// After `kb_register_joined_instance`, the node resolves via federated get
/// WITH its instance name attached (non-null), federated search attributes the
/// hit to the joined instance (not the primary KB), and the instance appears
/// in the user-facing `*KB Instances*` list. Guards the "joined KB is invisible
/// after join" regression class — the surfacing the live two-machine test did
/// by hand each iteration.
#[test]
fn joined_instance_surfaces_in_list_get_and_search() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);

    let mut remote = mae_kb::KnowledgeBase::new();
    let state = remote
        .upsert_with_crdt(
            mae_kb::Node::new(
                "shared:alpha",
                "Findme Title",
                mae_kb::NodeKind::Note,
                "searchable body",
            ),
            2,
        )
        .unwrap();
    let sv = remote.node_state_vector("shared:alpha").unwrap();

    let uuid = editor.kb_register_joined_instance(
        "team-kb",
        vec![crate::editor::JoinedNode {
            id: "shared:alpha".to_string(),
            bytes: state,
            daemon_sv: Some(sv),
        }],
    );

    // 1) Federated get resolves the node WITH a non-null instance attribution,
    //    and the node never leaks into the primary KB.
    let (inst_name, node) = editor
        .kb_federated_get("shared:alpha")
        .expect("joined node must resolve via federated get");
    assert_eq!(node.id, "shared:alpha");
    assert_eq!(
        inst_name.as_deref(),
        Some("team-kb"),
        "federated get must attribute the joined node to its instance"
    );
    assert!(
        editor.kb.primary.get("shared:alpha").is_none(),
        "joined nodes never pollute the primary KB"
    );

    // 2) Federated search attributes the hit to the joined instance.
    let hits = editor.kb_federated_search("Findme");
    let hit = hits
        .iter()
        .find(|(_, n)| n.id == "shared:alpha")
        .expect("joined node must be findable via federated search");
    assert_eq!(
        hit.0.as_deref(),
        Some("team-kb"),
        "search hit must carry the joined instance name, not None (local)"
    );

    // 3) The instance surfaces in the user-facing *KB Instances* list.
    editor.show_kb_instances();
    let listing = editor
        .buffers
        .iter()
        .find(|b| b.name == "*KB Instances*")
        .map(|b| b.rope().to_string())
        .expect("show_kb_instances must create the *KB Instances* buffer");
    assert!(
        listing.contains("team-kb"),
        "joined KB name must appear in *KB Instances*:\n{listing}"
    );
    assert!(
        listing.contains(&uuid),
        "the instance uuid must appear in *KB Instances*:\n{listing}"
    );
}

/// ADR-020 Phase 3 (B-10): a joined instance persists its nodes to a durable
/// CozoDB store with a real `db_path` that a fresh open + load_all reloads —
/// the foundation of restart survival (the startup loader reads this back).
#[test]
fn joined_instance_persists_to_reloadable_store() {
    let mut editor = Editor::new();
    let tmp = with_test_dirs(&mut editor);
    let dd = mae_kb::data_dir::KbDataDir::new(&tmp.path().join("data")).unwrap();
    editor.kb.data_dir = Some(dd);

    let mut remote = mae_kb::KnowledgeBase::new();
    let state = remote
        .upsert_with_crdt(
            mae_kb::Node::new("ct:overview", "Persisted", mae_kb::NodeKind::Note, "body"),
            2,
        )
        .unwrap();
    let sv = remote.node_state_vector("ct:overview").unwrap();
    let uuid = editor.kb_register_joined_instance(
        "ct",
        vec![crate::editor::JoinedNode {
            id: "ct:overview".to_string(),
            bytes: state,
            daemon_sv: Some(sv),
        }],
    );

    let db_path = {
        let inst = editor.kb.registry.find_by_uuid(&uuid).unwrap();
        assert!(
            !inst.db_path.as_os_str().is_empty() && inst.db_path.exists(),
            "joined instance must have a real, existing db_path (durable across restart)"
        );
        inst.db_path.clone()
    };

    // Drop the editor (and its live store), then open fresh from db_path
    // exactly as the startup loader does on restart (sqlite by default —
    // kb_open_instance_store — not the hardcoded-sled CozoKbStore::open()).
    drop(editor);
    let store = mae_kb::CozoKbStore::open_with_engine(&db_path, "sqlite").unwrap();
    let nodes = store.load_all().unwrap();
    assert!(
        nodes.iter().any(|n| n.id == "ct:overview"),
        "node reloads from the durable store (B-10 restart survival)"
    );
}

/// ADR-020 B-16: `kb_prepare_share_lineage` establishes + persists a canonical
/// CRDT lineage for a never-edited node, so the owner's local doc IS the lineage
/// peers adopt — and a peer's later edit converges on the owner (the bob→alice
/// direction that previously no-opped). Drives the OWNER (editor) path.
#[test]
fn prepare_share_lineage_persists_canonical_doc_so_owner_converges() {
    let mut editor = Editor::new();
    editor.collab.local_kb_client_id = 0xA11CE; // alice's stable, unique id

    // A node from org import: present locally with NO CRDT lineage.
    editor.kb.primary.insert(mae_kb::Node::new(
        "p:beta",
        "Plain",
        mae_kb::NodeKind::Note,
        "body",
    ));
    assert!(
        editor.kb.primary.get("p:beta").unwrap().crdt_doc.is_none(),
        "starts with no lineage (the divergence trap)"
    );

    // Owner prepares to share → establishes + persists the canonical lineage.
    editor.kb_prepare_share_lineage(crate::editor::KB_DEFAULT_NAME, &[]);
    let shared_state = editor
        .kb
        .primary
        .get("p:beta")
        .unwrap()
        .crdt_doc
        .clone()
        .expect("lineage established + persisted onto the local node");

    // Bob adopts the shared lineage and edits with HIS distinct client_id.
    let mut bob = mae_kb::KnowledgeBase::new();
    bob.adopt_remote_node("p:beta", &shared_state).unwrap();
    let bob_edit = {
        let mut n = bob.get("p:beta").unwrap().clone();
        n.title = "Bob Edit [REVERSE]".to_string();
        bob.upsert_with_crdt(n, 0xB0B).unwrap()
    };

    // The OWNER applies bob's edit to her local doc → converges (was a no-op).
    let changed = editor
        .kb
        .primary
        .apply_remote_update("p:beta", &bob_edit)
        .unwrap();
    assert!(
        changed,
        "owner converges to a peer's edit — local doc is now on the shared lineage (B-16)"
    );
    assert_eq!(
        editor.kb.primary.get("p:beta").unwrap().title,
        "Bob Edit [REVERSE]"
    );
}

/// ADR-019 Phase 3: after a restart the transient cache is empty, but
/// reconstruction rebuilds it from the durable registry markers (primary +
/// shared instances), and durable_shared_kb_ids lists what to re-subscribe.
#[test]
fn reconstruct_kb_sync_gate_rebuilds_from_durable_markers() {
    let mut editor = Editor::new();
    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "collabtest:overview",
        "O",
        mae_kb::NodeKind::Note,
        "b",
    ));
    editor.kb.instances.insert("uuid-ct".into(), inst);
    editor.kb.registry.instances.push(shared_ct_instance());
    editor.kb.registry.primary_shared = true;
    editor.kb.registry.primary_collab_id = Some("default".into());
    editor
        .kb
        .primary
        .insert(mae_kb::Node::new("p:1", "P", mae_kb::NodeKind::Note, "b"));

    assert!(
        editor.collab.shared_kbs.is_empty(),
        "cache empty post-restart"
    );
    editor.reconstruct_kb_sync_gate();
    assert!(editor.collab.shared_kbs["collabtest"].contains("collabtest:overview"));
    assert!(editor.collab.shared_kbs["default"].contains("p:1"));

    let mut ids = editor.durable_shared_kb_ids();
    ids.sort();
    assert_eq!(ids, vec!["collabtest".to_string(), "default".to_string()]);
}

/// ADR-019: reconnect re-subscribe SKIPS the primary KB (re-joining one's own
/// primary popped a spurious pending request → the *Collab Status* buffer on
/// launch), re-JOINS guests (empty org_dir), and re-SHARES owner instances.
#[test]
fn kb_resubscribe_intents_skips_primary_and_distinguishes_owner_guest() {
    use crate::editor::CollabIntent;
    let mut editor = Editor::new();
    // Stale primary share marker (must NOT produce a re-subscribe intent).
    editor.kb.registry.primary_shared = true;
    editor.kb.registry.primary_collab_id = Some("default".into());
    // Guest-joined instance: empty org_dir.
    let mut guest = shared_ct_instance();
    guest.name = "joined-kb".into();
    guest.collab_id = Some("joined-kb".into());
    guest.org_dir = std::path::PathBuf::new();
    editor.kb.registry.instances.push(guest);
    // Owner-shared instance: real org_dir.
    let mut owner = shared_ct_instance();
    owner.uuid = "uuid-owned".into();
    owner.name = "owned-kb".into();
    owner.collab_id = Some("owned-kb".into());
    owner.org_dir = std::path::PathBuf::from("/home/u/org");
    editor.kb.registry.instances.push(owner);

    let intents = editor.kb_resubscribe_intents();
    assert_eq!(
        intents.len(),
        2,
        "primary must be skipped; 2 instances remain"
    );
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, CollabIntent::JoinKb { kb_id, .. } if kb_id == "joined-kb")),
        "guest (empty org_dir) must re-JOIN"
    );
    assert!(
        intents
            .iter()
            .any(|i| matches!(i, CollabIntent::ShareKb { kb_name, .. } if kb_name == "owned-kb")),
        "owner (real org_dir) must re-SHARE"
    );
    assert!(
        !intents
            .iter()
            .any(|i| matches!(i, CollabIntent::JoinKb { kb_id, .. } if kb_id == "default")),
        "primary KB must NOT be re-subscribed (the launch-popup bug)"
    );
}

/// B-8 repro: register a KB via the REAL kb_register path (CozoKbStore
/// import — not a hand-inserted instance), stamp the durable share marker as
/// the share would, then edit a node. The edit MUST enqueue a CRDT update.
/// Live, this produced pending_kb_updates=0 (no emit) — reproduce it here.
#[test]
fn b8_repro_registered_kb_edit_enqueues() {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/kb/collabtest"
    );
    if !std::path::Path::new(fixture).is_dir() {
        eprintln!("fixture missing, skipping: {fixture}");
        return;
    }
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let result = editor
        .kb_register("collabtest", std::path::Path::new(fixture))
        .expect("register collabtest");
    let uuid = result.uuid.clone();
    eprintln!("registered uuid={uuid}");
    eprintln!(
        "instances keys = {:?}",
        editor.kb.instances.keys().collect::<Vec<_>>()
    );
    eprintln!(
        "node in instance? {}",
        editor
            .kb
            .instances
            .get(&uuid)
            .map(|kb| kb.contains("collabtest:overview"))
            .unwrap_or(false)
    );
    eprintln!(
        "node in primary? {}",
        editor.kb.primary.contains("collabtest:overview")
    );

    // Stamp the durable share marker (as the KbShared handler does).
    {
        let inst = editor.kb.registry.find_mut(&uuid).expect("find inst");
        inst.shared = true;
        inst.collab_id = Some("collabtest".into());
    }
    editor.collab.kb_sync_mode = "on_save".into();

    editor
        .kb_update_node("collabtest:overview", Some("EDITED"), None, None)
        .expect("update");
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "registered-KB edit must enqueue a kb/node_update (B-8)"
    );
}

#[test]
fn watcher_starts_on_register() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    assert!(
        editor.kb.watchers.contains_key(&result.uuid),
        "watcher should start on register"
    );
}

#[test]
fn watcher_removed_on_unregister() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();
    assert!(editor.kb.watchers.contains_key(&uuid));
    editor.kb_unregister("TestNotes");
    assert!(!editor.kb.watchers.contains_key(&uuid));
}

#[test]
fn watcher_drains_new_file() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();

    // Write a new org file
    std::fs::write(
        dir.path().join("new-note.org"),
        ":PROPERTIES:\n:ID: watch-test-new\n:END:\n#+title: Watched Note\n\nNew.\n",
    )
    .unwrap();

    // Poll until watcher picks it up (filesystem events are async)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        editor.drain_kb_watchers();
        if editor.kb.instances[&uuid].get("watch-test-new").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(
        editor.kb.instances[&uuid].get("watch-test-new").is_some(),
        "new org file should be auto-ingested by watcher"
    );
}

// --- W1: KB options tests ---

#[test]
fn kb_options_registered() {
    let editor = Editor::new();
    for name in &[
        "kb_watcher_enabled",
        "kb_watcher_debounce_ms",
        "kb_max_drain_events",
        "kb_search_excerpt_length",
        "kb_search_max_results",
        "kb_auto_register",
    ] {
        assert!(
            editor.option_registry.find(name).is_some(),
            "option '{}' not found in registry",
            name
        );
    }
    // Also check aliases
    assert!(editor.option_registry.find("kb-watcher-enabled").is_some());
    assert!(editor.option_registry.find("kb-max-drain-events").is_some());
}

#[test]
fn kb_options_get_set_roundtrip() {
    let mut editor = Editor::new();
    // Bool roundtrip
    assert_eq!(editor.get_option("kb_watcher_enabled").unwrap().0, "true");
    editor.set_option("kb_watcher_enabled", "false").unwrap();
    assert_eq!(editor.get_option("kb_watcher_enabled").unwrap().0, "false");
    // Int roundtrip
    editor.set_option("kb_watcher_debounce_ms", "1000").unwrap();
    assert_eq!(
        editor.get_option("kb_watcher_debounce_ms").unwrap().0,
        "1000"
    );
    editor.set_option("kb_max_drain_events", "50").unwrap();
    assert_eq!(editor.get_option("kb_max_drain_events").unwrap().0, "50");
    editor
        .set_option("kb_search_excerpt_length", "300")
        .unwrap();
    assert_eq!(
        editor.get_option("kb_search_excerpt_length").unwrap().0,
        "300"
    );
    editor.set_option("kb_search_max_results", "10").unwrap();
    assert_eq!(editor.get_option("kb_search_max_results").unwrap().0, "10");
    // Bool roundtrip
    editor.set_option("kb_auto_register", "true").unwrap();
    assert_eq!(editor.get_option("kb_auto_register").unwrap().0, "true");
}

// --- W4: Watcher hardening tests ---

#[test]
fn drain_debounce_skips_recent() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();

    // Write a file and wait for watcher to see it
    std::fs::write(
        dir.path().join("debounce-first.org"),
        ":PROPERTIES:\n:ID: debounce-first\n:END:\n#+title: First\n\ntest\n",
    )
    .unwrap();
    // Drain until first file is picked up (establishes timestamp)
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        editor.drain_kb_watchers();
        if editor.kb.last_drain.contains_key(&uuid) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(editor.kb.last_drain.contains_key(&uuid));

    // Now set a very long debounce
    editor.kb.watcher_debounce_ms = 60_000;

    // Write another file
    std::fs::write(
        dir.path().join("debounce-second.org"),
        ":PROPERTIES:\n:ID: debounce-second\n:END:\n#+title: Second\n\ntest\n",
    )
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    // This drain should be debounced — second node should NOT appear
    editor.drain_kb_watchers();
    assert!(
        editor.kb.instances[&uuid].get("debounce-second").is_none(),
        "debounce should have skipped the drain"
    );
}

#[test]
fn watcher_disabled_skips_drain() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb.watcher_enabled = false;
    // Register should skip watcher creation
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    assert!(
        !editor.kb.watchers.contains_key(&result.uuid),
        "watcher should not be created when disabled"
    );
    // drain should be a no-op
    editor.drain_kb_watchers();
}

#[test]
fn watcher_error_count_exposed() {
    let dir = create_test_org_dir();
    let watcher = mae_kb::watch::OrgDirWatcher::new(dir.path()).unwrap();
    // Initial error count should be 0
    assert_eq!(watcher.error_count(), 0);
}

#[test]
fn kb_federated_search_deduplicates() {
    let mut editor = Editor::new();
    // Insert a node locally
    editor
        .kb_create_node("dedup-test", "Dedup", "body", mae_kb::NodeKind::Note)
        .unwrap();
    // Insert same node in a federated instance
    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "dedup-test",
        "Dedup",
        mae_kb::NodeKind::Note,
        "body",
    ));
    editor.kb.instances.insert("inst-1".to_string(), inst);

    let results = editor.kb_federated_search("Dedup");
    let dedup_count = results.iter().filter(|(_, n)| n.id == "dedup-test").count();
    assert_eq!(dedup_count, 1, "same node ID should appear only once");
    // Local result should win (instance_name is None)
    let (inst_name, _) = results.iter().find(|(_, n)| n.id == "dedup-test").unwrap();
    assert!(
        inst_name.is_none(),
        "local result should win over federated"
    );
}

// --- W5: Observability tests ---

#[test]
fn kb_watcher_stats_update_on_drain() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("TestNotes", dir.path()).unwrap();
    let uuid = result.uuid.clone();

    // Write a new file and wait for watcher
    std::fs::write(
        dir.path().join("stats-test.org"),
        ":PROPERTIES:\n:ID: stats-test\n:END:\n#+title: Stats\n\ntest\n",
    )
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        editor.drain_kb_watchers();
        if editor.kb.instances[&uuid].get("stats-test").is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    assert!(
        editor.kb.watcher_stats.events_upserted > 0,
        "events_upserted should be positive after drain"
    );
}

#[test]
fn perf_stats_kb_fields_default_zero() {
    let editor = Editor::new();
    assert_eq!(editor.perf_stats.kb_search_latency_us, 0);
    assert_eq!(editor.perf_stats.kb_watcher_drain_us, 0);
    assert_eq!(editor.perf_stats.kb_watcher_events, 0);
}

#[test]
fn kb_register_does_not_clobber_user_dirs() {
    // Resolve real user dirs the same way the production code does.
    let home = std::env::var("HOME").unwrap();
    let real_config = PathBuf::from(&home).join(".config/mae/kb-registry.toml");
    let real_data = PathBuf::from(&home).join(".local/share/mae/kb-registry.toml");

    // Record mtimes before
    let config_mtime = real_config.metadata().ok().and_then(|m| m.modified().ok());
    let data_mtime = real_data.metadata().ok().and_then(|m| m.modified().ok());

    // Run a register + unregister cycle with test dirs
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("IsolationTest", dir.path()).unwrap();
    editor.kb_unregister(&result.uuid);

    // Verify mtimes unchanged
    let config_mtime_after = real_config.metadata().ok().and_then(|m| m.modified().ok());
    let data_mtime_after = real_data.metadata().ok().and_then(|m| m.modified().ok());
    assert_eq!(
        config_mtime, config_mtime_after,
        "config dir kb-registry.toml was modified by test"
    );
    assert_eq!(
        data_mtime, data_mtime_after,
        "data dir kb-registry.toml was modified by test"
    );
}

/// CF1 (SECURITY_REVIEW §6.3): enabling E2E MUST surface the honesty advisory at
/// the point of action — and a non-e2e mode MUST NOT. Selective oracle: the WARN
/// message names the *actual* caveats (no forward secrecy, metadata visible), not
/// an incidental string; and the negative `mode="none"` case must produce no
/// advisory (the failure mode that would let the label silently oversell).
#[test]
fn enabling_e2e_surfaces_the_caveat_advisory_at_point_of_action() {
    use crate::editor::KbCollabAction;

    // Enable E2E → exactly one WARN advisory, naming the real caveats.
    let mut editor = Editor::new();
    editor.queue_kb_collab_action(KbCollabAction::SetEncryption {
        kb_id: "kb-cf1".into(),
        mode: "e2e".into(),
    });
    let warns = editor
        .message_log
        .entries_filtered(crate::messages::MessageLevel::Warn);
    let advisory: Vec<_> = warns
        .iter()
        .filter(|e| e.target == "kb-encryption")
        .collect();
    assert_eq!(
        advisory.len(),
        1,
        "exactly one E2E enable advisory expected, got {}",
        advisory.len()
    );
    let msg = &advisory[0].message;
    // Selective oracle: the meaningful caveats, not an incidental token.
    assert!(
        msg.contains("No forward secrecy"),
        "advisory must disclose the no-FS caveat"
    );
    assert!(
        msg.to_lowercase().contains("metadata is visible"),
        "advisory must disclose metadata exposure"
    );
    assert!(
        msg.contains("NOT retroactive"),
        "advisory must warn enable-before-sharing"
    );
    // The intent is still queued (the advisory doesn't block the action).
    assert!(matches!(
        editor.collab.pending_intent,
        Some(crate::editor::CollabIntent::KbSetEncryption { .. })
    ));

    // Negative: a non-e2e mode must NOT emit the advisory (the oversell failure mode).
    let mut editor2 = Editor::new();
    editor2.queue_kb_collab_action(KbCollabAction::SetEncryption {
        kb_id: "kb-cf1".into(),
        mode: "none".into(),
    });
    let advisory2 = editor2
        .message_log
        .entries()
        .into_iter()
        .filter(|e| e.target == "kb-encryption")
        .count();
    assert_eq!(
        advisory2, 0,
        "no advisory should fire for a non-e2e SetEncryption mode"
    );
}

/// Pre-dogfood review: the Scheme/AI surface can lower several lifecycle
/// actions in ONE apply cycle (bulk member onboarding). The single
/// `pending_intent` slot used to keep only the LAST, silently dropping the
/// rest — an owner who scripted "add a, add b, add c" got only c, with no
/// error. Assert all N survive (1 in the slot + the rest fanned out through
/// `reconnect_intents`, the same one-per-tick queue the reconnect path drains).
#[test]
fn batched_kb_collab_actions_do_not_collapse_to_the_last() {
    use crate::editor::{CollabIntent, KbCollabAction};
    let mut editor = Editor::new();
    for fp in ["SHA256:a", "SHA256:b", "SHA256:c"] {
        editor.queue_kb_collab_action(KbCollabAction::AddMember {
            kb_id: "kb".into(),
            member: fp.into(),
            role: "editor".into(),
        });
    }
    // 1 in the active slot + 2 fanned out = 3 total, none dropped.
    assert!(
        editor.collab.pending_intent.is_some(),
        "first action in the slot"
    );
    assert_eq!(
        editor.collab.reconnect_intents.len(),
        2,
        "the other two batched actions must be queued, not overwritten"
    );

    // FIFO order preserved: slot = a, queue = [b, c].
    let members: Vec<String> = std::iter::once(editor.collab.pending_intent.clone().unwrap())
        .chain(editor.collab.reconnect_intents.iter().cloned())
        .map(|i| match i {
            CollabIntent::KbAddMember { member, .. } => member,
            other => panic!("expected KbAddMember, got {other:?}"),
        })
        .collect();
    assert_eq!(members, vec!["SHA256:a", "SHA256:b", "SHA256:c"]);
}
