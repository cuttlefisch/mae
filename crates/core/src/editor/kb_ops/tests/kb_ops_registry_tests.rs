use super::*;

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
