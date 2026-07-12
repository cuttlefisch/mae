use super::*;

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
