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
    assert_eq!(result.dedup, crate::editor::kb_ops::PromoteDedup::Removed);

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
    assert_eq!(promoted.source, Some(mae_kb::NodeSource::Promoted));

    // The federated copy's content matched what was just promoted, so it's
    // deduplicated away immediately -- promotion converges to one copy.
    assert!(
        editor.kb.instances[&reg.uuid].get("test-note-1").is_none(),
        "the federated instance's now-redundant copy must be removed on promote"
    );
    let durable = editor
        .kb
        .instance_stores
        .get(&reg.uuid)
        .unwrap()
        .get_node("test-note-1")
        .unwrap();
    assert!(
        durable.is_none(),
        "the durable instance store copy must also be removed, not just the mirror"
    );
}

#[test]
fn kb_promote_node_resets_source_and_crdt_doc() {
    // The root-cause fix: a node promoted out of a COLLAB-JOINED instance
    // (source == Federation) must not carry that marker (or its old CRDT
    // lineage) into primary -- otherwise kb_owner_of's #76 stranded-node
    // guard mistakes the fresh primary copy for pre-ADR-019 leftover cruft
    // and keeps routing every future write back to the stale instance copy.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();

    {
        let node = editor
            .kb
            .instances
            .get_mut(&reg.uuid)
            .unwrap()
            .get_mut("test-note-1")
            .unwrap();
        node.source = Some(mae_kb::NodeSource::Federation);
        node.crdt_doc = Some(vec![1, 2, 3]);
    }

    editor.kb_promote_node("test-note-1").unwrap();

    let promoted = editor.kb.primary.get("test-note-1").unwrap();
    assert_eq!(
        promoted.source,
        Some(mae_kb::NodeSource::Promoted),
        "promotion must sever the old Federation marker"
    );
    assert!(
        promoted.crdt_doc.is_none(),
        "promotion must clear the old (unrelated-KB) CRDT lineage"
    );
}

#[test]
fn kb_owner_of_resolves_promoted_node_to_primary() {
    // Direct regression for the root-cause bug: before the fix, a node
    // promoted from a Federation-sourced instance would still resolve back
    // to the instance via kb_owner_of's stranded-node guard.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();
    {
        let node = editor
            .kb
            .instances
            .get_mut(&reg.uuid)
            .unwrap()
            .get_mut("test-note-1")
            .unwrap();
        node.source = Some(mae_kb::NodeSource::Federation);
    }

    editor.kb_promote_node("test-note-1").unwrap();

    assert_eq!(
        editor.kb_owner_of("test-note-1"),
        Some(None),
        "a promoted node must resolve to primary, not be shadowed back to its origin instance"
    );
}

#[test]
fn kb_apply_remote_update_after_promote_routes_to_primary() {
    // The consequence discovered beyond the original bug report: not just
    // local CRUD, but remote-originated CRDT updates for a promoted node's
    // id must also land on the primary copy, not a resurrected/lingering
    // instance copy.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();
    {
        let node = editor
            .kb
            .instances
            .get_mut(&reg.uuid)
            .unwrap()
            .get_mut("test-note-1")
            .unwrap();
        node.source = Some(mae_kb::NodeSource::Federation);
    }
    editor.kb_promote_node("test-note-1").unwrap();
    assert!(
        editor
            .kb
            .primary
            .get("test-note-1")
            .unwrap()
            .crdt_doc
            .is_none(),
        "starts with no lineage post-promote (Fix 1) -- the divergence trap"
    );

    // Establish + persist the canonical lineage on the promoted primary
    // node (mirrors prepare_share_lineage_persists_canonical_doc_so_owner_converges),
    // then have a "remote peer" adopt that SAME lineage and edit it.
    editor.kb_prepare_share_lineage(crate::editor::KB_DEFAULT_NAME, &[]);
    let shared_state = editor
        .kb
        .primary
        .get("test-note-1")
        .unwrap()
        .crdt_doc
        .clone()
        .expect("lineage established + persisted onto the promoted node");
    let mut remote = mae_kb::KnowledgeBase::new();
    remote
        .adopt_remote_node("test-note-1", &shared_state)
        .unwrap();
    let remote_update = {
        let mut n = remote.get("test-note-1").unwrap().clone();
        n.title = "Remotely Retitled".to_string();
        remote.upsert_with_crdt(n, 99).unwrap()
    };

    let changed = editor
        .kb_apply_remote_update("test-note-1", &remote_update, None)
        .unwrap();
    assert!(changed);
    assert_eq!(
        editor.kb.primary.get("test-note-1").unwrap().title,
        "Remotely Retitled",
        "remote update must apply to the primary copy"
    );
    assert!(
        editor.kb.instances[&reg.uuid].get("test-note-1").is_none(),
        "no instance copy should have been resurrected"
    );
}

#[test]
fn kb_promote_node_dedup_keeps_both_on_divergence() {
    // Adversarial/defensive case (principle #14): if the instance's copy
    // ever diverges from what was just promoted (not reachable through
    // kb_promote_node's own synchronous call sequence in v1, but must stay
    // correct if something races), neither copy is silently deleted.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();

    let mut promoted = editor
        .kb
        .instances
        .get(&reg.uuid)
        .unwrap()
        .get("test-note-1")
        .cloned()
        .unwrap();
    promoted.source = Some(mae_kb::NodeSource::Promoted);
    promoted.body = "content that has since diverged".to_string();

    let dedup = editor.kb_dedup_promoted_instance_copy("test-note-1", &reg.uuid, &promoted);
    assert_eq!(dedup, crate::editor::kb_ops::PromoteDedup::KeptDiverged);
    assert!(
        editor.kb.instances[&reg.uuid].contains("test-note-1"),
        "a diverged instance copy must be preserved, never silently deleted"
    );
    let notes = editor.notifications.active_sorted();
    let hit = notes
        .iter()
        .find(|n| n.source == "kb" && n.title.contains("test-note-1"));
    assert!(
        hit.is_some(),
        "divergence must be surfaced via notification for manual review"
    );
    assert_eq!(
        hit.unwrap().severity,
        crate::notifications::Severity::ActionRequired
    );
}

#[test]
fn kb_promote_node_retracts_manifest_for_shared_origin() {
    // If the origin instance is actively shared, promotion must retract the
    // node from THAT KB's manifest -- otherwise peers/daemon still believe
    // the node belongs to the old shared collection.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();
    {
        let inst = editor
            .kb
            .registry
            .instances
            .iter_mut()
            .find(|i| i.uuid == reg.uuid)
            .unwrap();
        inst.shared = true;
        inst.collab_id = Some("collab-kb-id".to_string());
    }

    editor.kb_promote_node("test-note-1").unwrap();

    assert!(
        editor
            .collab
            .pending_kb_manifest
            .iter()
            .any(|(kb_id, node_id, _title, add)| kb_id == "collab-kb-id"
                && node_id == "test-note-1"
                && !add),
        "promotion must enqueue a manifest retraction for the shared origin instance, got: {:?}",
        editor.collab.pending_kb_manifest
    );
}

#[test]
fn kb_promote_node_from_non_collab_instance_unaffected() {
    // Regression guard: a plain (never-joined, non-Federation-sourced)
    // instance promotion must behave exactly as before this change --
    // kb_owner_of resolution to primary was never broken for this shape.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    editor.kb_register("TestNotes", dir.path()).unwrap();

    assert_ne!(
        editor
            .kb
            .instances
            .values()
            .next()
            .unwrap()
            .get("test-note-1")
            .unwrap()
            .source,
        Some(mae_kb::NodeSource::Federation),
        "a plain org-dir import must never be Federation-sourced"
    );

    editor.kb_promote_node("test-note-1").unwrap();
    assert_eq!(editor.kb_owner_of("test-note-1"), Some(None));
    assert_eq!(
        editor.kb.primary.get("test-note-1").unwrap().source,
        Some(mae_kb::NodeSource::Promoted)
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
fn kb_promote_node_then_delete_org_dir_all_crud_paths_still_work() {
    // The user's actual goal: once promoted, a node must be a fully
    // self-sufficient graph-only citizen -- the org file it came from
    // doesn't need to exist anymore, and every CRUD path must keep working.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);

    let store = mae_kb::CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    let arc = std::sync::Arc::new(store);
    editor.kb.primary_cozo = Some(arc.clone());
    editor.kb.store = Some(arc.clone());

    editor.kb_register("TestNotes", dir.path()).unwrap();
    editor.kb_promote_node("test-note-1").unwrap();
    editor.kb_promote_node("test-note-2").unwrap();

    // Org files "do not need to exist anymore" for promoted content --
    // delete the entire backing directory and confirm nothing depends on it.
    std::fs::remove_dir_all(dir.path()).unwrap();

    // update
    editor
        .kb_update_node("test-note-1", Some("Updated Title"), None, None)
        .unwrap();
    assert_eq!(
        editor.kb.primary.get("test-note-1").unwrap().title,
        "Updated Title"
    );

    // add-link / links_from (primary store, mirrors kb_neighborhood/kb_agenda's
    // primary-only routing)
    arc.add_link("test-note-1", "test-note-2", None).unwrap();
    let links = arc.links_from("test-note-1").unwrap();
    assert!(
        links.iter().any(|l| l.dst == "test-note-2"),
        "add_link must work against a promoted node with no source_file"
    );

    // history + restore
    let v1 = arc
        .snapshot_version("test-note-1", "post-promote checkpoint")
        .unwrap();
    let history = arc.node_history("test-note-1", 10).unwrap();
    assert!(history.iter().any(|v| v.version == v1));
    editor
        .kb_update_node("test-note-1", Some("Second Update"), None, None)
        .unwrap();
    arc.restore_version("test-note-1", v1).unwrap();
    let restored = arc.get_node("test-note-1").unwrap().unwrap();
    assert_eq!(
        restored.title, "Updated Title",
        "restore must work against a promoted node"
    );

    // health
    let health = arc.health_report().unwrap();
    assert!(health.total_nodes >= 2);

    // agenda
    assert!(arc.agenda_query(&mae_kb::AgendaFilter::Orphan).is_ok());

    // neighborhood + shortest_path (primary-only, per kb_neighborhood/kb_shortest_path)
    let neighborhood = arc.neighborhood("test-note-1", 2).unwrap();
    assert!(!neighborhood.nodes.is_empty());
    let path = arc.shortest_path("test-note-1", "test-note-2").unwrap();
    assert!(
        !path.is_empty(),
        "shortest_path must find the link we added"
    );

    // graph-view/help-buffer population
    editor.open_help_at("test-note-1");
    assert_eq!(editor.kb_view().unwrap().current, "test-note-1");
    assert!(
        !editor.status_msg.to_lowercase().contains("no such"),
        "help buffer must populate for a promoted node: {}",
        editor.status_msg
    );

    // delete (exercised last)
    editor.kb_delete_node("test-note-2").unwrap();
    assert!(editor.kb.primary.get("test-note-2").is_none());
    assert!(arc.get_node("test-note-2").unwrap().is_none());
}

#[test]
fn kb_promote_node_n_way_promote_update_stranded_sweep_delete() {
    // N-way (principle #14): promote, then edit, then run the OLD
    // #76 :kb-migrate-stranded sweep (must be a correct no-op against the
    // Promoted-sourced node, since it only targets `source == Federation`),
    // then delete. The two mechanisms must not fight each other, and the
    // final state must have zero copies anywhere.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();
    {
        let node = editor
            .kb
            .instances
            .get_mut(&reg.uuid)
            .unwrap()
            .get_mut("test-note-1")
            .unwrap();
        node.source = Some(mae_kb::NodeSource::Federation);
    }

    editor.kb_promote_node("test-note-1").unwrap();
    editor
        .kb_update_node("test-note-1", Some("Edited After Promote"), None, None)
        .unwrap();

    let (removed, diverged) = editor.kb_migrate_stranded_federation_nodes();
    assert_eq!(
        removed, 0,
        "the stranded sweep must not touch a Promoted-sourced node"
    );
    assert_eq!(diverged, 0);
    assert_eq!(
        editor.kb.primary.get("test-note-1").unwrap().title,
        "Edited After Promote",
        "the sweep must not have reverted or removed the promoted node's edit"
    );

    editor.kb_delete_node("test-note-1").unwrap();
    assert!(editor.kb.primary.get("test-note-1").is_none());
    assert!(
        editor.kb.instances[&reg.uuid].get("test-note-1").is_none(),
        "no copy should remain in the origin instance either"
    );
}

#[test]
fn kb_promote_node_resave_org_file_after_promote_reappears_but_crud_stays_correct() {
    // Documents a residual, honestly-scoped limitation: re-saving the org
    // file after promotion re-materializes a cosmetic duplicate in the
    // instance (the importer doesn't know the heading was promoted), but
    // CRUD correctness is unaffected -- kb_owner_of still prefers primary
    // since the resurrected node is UserOrg-sourced, not Federation.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let reg = editor.kb_register("TestNotes", dir.path()).unwrap();
    editor.kb_promote_node("test-note-1").unwrap();
    assert!(editor.kb.instances[&reg.uuid].get("test-note-1").is_none());

    // Re-save the still-present org file (unchanged content is enough to
    // trigger re-materialization on reimport).
    let file_path = dir.path().join("note1.org");
    editor.kb_reimport_file(&file_path);

    assert!(
        editor.kb.instances[&reg.uuid].get("test-note-1").is_some(),
        "re-importing the still-present org file re-materializes the heading (cosmetic)"
    );
    assert_eq!(
        editor.kb_owner_of("test-note-1"),
        Some(None),
        "CRUD correctness is preserved -- primary still wins despite the cosmetic duplicate"
    );
    assert_eq!(
        editor.kb.primary.get("test-note-1").unwrap().source,
        Some(mae_kb::NodeSource::Promoted)
    );
}

#[test]
fn kb_prepare_share_lineage_mints_fresh_lineage_for_promoted_node() {
    // Open question 2, resolved: kb_prepare_share_lineage only mints a
    // fresh CRDT lineage when crdt_doc.is_none(). A node promoted from a
    // collab-joined instance used to keep its OLD lineage (tied to the
    // wrong KB/epoch) -- Fix 1's `node.crdt_doc = None` on promote is what
    // makes this correctly mint a fresh one instead of silently reusing it.
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    {
        let node = editor.kb.instances.entry("origin".to_string()).or_default();
        let mut n = mae_kb::Node::new("shared-id", "T", mae_kb::NodeKind::Note, "b");
        n.source = Some(mae_kb::NodeSource::Federation);
        node.insert(n);
    }
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: "origin".into(),
            name: "TestNotes".into(),
            org_dir: dir.path().to_path_buf(),
            db_path: std::path::PathBuf::new(),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
        });
    // Simulate a lingering OLD lineage from before it was federated to this
    // instance (upsert_with_crdt on the instance copy directly, bypassing
    // kb_promote_node -- we want to prove the RESET happens, so start from
    // a node that clearly has a stale crdt_doc set).
    let old_lineage = editor
        .kb
        .instances
        .get_mut("origin")
        .unwrap()
        .upsert_with_crdt(
            mae_kb::Node::new("shared-id", "T", mae_kb::NodeKind::Note, "b"),
            0xDEAD,
        )
        .unwrap();

    editor.kb_promote_node("shared-id").unwrap();
    assert!(
        editor
            .kb
            .primary
            .get("shared-id")
            .unwrap()
            .crdt_doc
            .is_none(),
        "promotion must clear the old lineage before any later share"
    );

    editor.kb_prepare_share_lineage("primary", &["shared-id".to_string()]);
    let fresh = editor
        .kb
        .primary
        .get("shared-id")
        .unwrap()
        .crdt_doc
        .clone()
        .expect("kb_prepare_share_lineage must mint a fresh lineage");
    assert_ne!(
        fresh, old_lineage,
        "the freshly-minted lineage must not be the old federated lineage"
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
