use super::*;

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
