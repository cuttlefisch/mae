//! Split from the monolithic `collab_bridge_tests.rs`: Phase 4 continuous KB sync: share/join/left tracking, CRDT-update generation, manual-sync-mode.

use super::*;

#[test]
fn collab_kb_shared_populates_tracking() {
    let mut editor = Editor::new();
    // Isolate the registry save (handler stamps the primary-shared marker).
    let tmp = std::env::temp_dir().join(format!("mae-adr019-prim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    editor.data_dir_override = Some(tmp.clone());
    // Insert some nodes into the primary KB.
    editor.kb.primary.insert(mae_kb::Node::new(
        "node-1".to_string(),
        "Title 1".to_string(),
        mae_kb::NodeKind::Note,
        "body 1".to_string(),
    ));
    editor.kb.primary.insert(mae_kb::Node::new(
        "node-2".to_string(),
        "Title 2".to_string(),
        mae_kb::NodeKind::Note,
        "body 2".to_string(),
    ));

    // Simulate KbShared event.
    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "default".to_string(),
            node_count: 2,
            collection_state: Vec::new(),
        },
    );

    assert!(
        editor.collab.shared_kbs.contains_key("default"),
        "shared_kbs should track the shared KB"
    );
    let tracked = &editor.collab.shared_kbs["default"];
    assert!(
        tracked.contains("node-1") && tracked.contains("node-2"),
        "shared_kbs should contain all node IDs: {:?}",
        tracked
    );
    // ADR-019: primary-share durable marker stamped.
    assert!(editor.kb.registry.primary_shared);
    assert_eq!(
        editor.kb.registry.primary_collab_id.as_deref(),
        Some("default")
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
/// Phase D (ADR-029): a *daemon-host* share (auto-hosting the primary) must NOT
/// stamp the durable `primary_shared` marker — hosting is runtime-only, so it
/// never implies peer-share and never leaks into a later daemon-less launch. The
/// in-flight `daemon_host_pending` marker routes the KbShared down the host path.
#[test]
fn collab_kb_shared_daemon_host_skips_durable_marker() {
    let mut editor = Editor::new();
    let tmp = std::env::temp_dir().join(format!("mae-phased-host-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    editor.data_dir_override = Some(tmp.clone());

    editor.kb.primary.insert(mae_kb::Node::new(
        "node-1",
        "Title 1",
        mae_kb::NodeKind::Note,
        "body 1",
    ));
    // Mark the host share in flight (as the Connected handler does before the share).
    editor
        .collab
        .daemon_host_pending
        .insert("default".to_string());

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "default".to_string(),
            node_count: 1,
            collection_state: Vec::new(),
        },
    );

    // The pending marker is consumed (host path taken)...
    assert!(
        !editor.collab.daemon_host_pending.contains("default"),
        "host-only KbShared must consume the in-flight marker"
    );
    // ...and the DURABLE peer-share marker is NOT stamped or persisted.
    assert!(
        !editor.kb.registry.primary_shared,
        "daemon-host share must NOT durably mark the primary as peer-shared"
    );
    assert!(
        editor.kb.registry.primary_collab_id.is_none(),
        "daemon-host share must not set a durable collab id"
    );
    assert!(
        !tmp.join("kb-registry.toml").exists(),
        "runtime-only hosting must not persist a registry marker"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
/// I-9 + ADR-019: sharing a *named federated instance* tracks its node IDs by
/// resolving name→uuid (cache) AND stamps the DURABLE registry marker
/// (`shared`/`collab_id`) so the share survives editor restart.
#[test]
fn collab_kb_shared_named_instance_tracks_nodes_by_uuid() {
    let mut editor = Editor::new();
    // Isolate the registry save to a temp dir (the handler persists markers).
    let tmp = std::env::temp_dir().join(format!("mae-adr019-share-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    editor.data_dir_override = Some(tmp.clone());

    let uuid = "uuid-collabtest".to_string();
    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "collabtest:overview",
        "Overview",
        mae_kb::NodeKind::Note,
        "b",
    ));
    inst.insert(mae_kb::Node::new(
        "collabtest:alpha",
        "Alpha",
        mae_kb::NodeKind::Note,
        "b",
    ));
    editor.kb.instances.insert(uuid.clone(), inst);
    // Registry maps the human name → uuid, NOT yet shared (handler stamps it).
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: uuid.clone(),
            name: "collabtest".into(),
            org_dir: std::path::PathBuf::from("/tmp/collabtest"),
            db_path: std::path::PathBuf::from("/tmp/collabtest.db"),
            primary: false,
            enabled: true,
            last_import: None,
            collab_id: None,
            shared: false,
            remote_peers: Vec::new(),
            last_sync: None,
            ai_residency: mae_kb::federation::AiResidency::default(),
        });
    // The handler now reloads the registry fresh from disk before stamping
    // the durable marker (KbRegistry::update) — persist the fixture instance
    // first so it's actually there to find, matching the real precondition
    // (an instance can only be shared once it's already registered+persisted).
    editor.kb.registry.save(&tmp).unwrap();

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "collabtest".to_string(),
            node_count: 2,
            collection_state: Vec::new(),
        },
    );

    let tracked = &editor.collab.shared_kbs["collabtest"];
    assert!(
        tracked.contains("collabtest:overview") && tracked.contains("collabtest:alpha"),
        "named-instance share must track nodes via uuid resolution, got: {:?}",
        tracked
    );
    // Durable marker stamped (survives restart).
    let inst = editor.kb.registry.find("collabtest").unwrap();
    assert!(inst.shared, "share must stamp durable shared=true");
    assert_eq!(inst.collab_id.as_deref(), Some("collabtest"));
    // And persisted to the isolated registry file.
    assert!(
        tmp.join("kb-registry.toml").exists(),
        "registry marker must be persisted"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
/// ADR-019 restart-survival (the bug): the durable share marker must survive
/// a registry SAVE→LOAD round-trip, so a freshly-started editor's emit gate
/// fires without any live event. This is the persistence crux of "edits keep
/// propagating across editor restart".
#[test]
fn adr019_share_marker_survives_registry_reload() {
    let tmp = std::env::temp_dir().join(format!("mae-adr019-reload-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut editor = Editor::new();
    editor.data_dir_override = Some(tmp.clone());

    let mut inst = mae_kb::KnowledgeBase::new();
    inst.insert(mae_kb::Node::new(
        "collabtest:overview",
        "O",
        mae_kb::NodeKind::Note,
        "b",
    ));
    editor.kb.instances.insert("uuid-ct".into(), inst);
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: "uuid-ct".into(),
            name: "collabtest".into(),
            org_dir: std::path::PathBuf::new(),
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
    // The handler reloads the registry fresh from disk before stamping the
    // durable marker (KbRegistry::update) — persist the fixture instance
    // first so it's actually there to find (an instance can only be shared
    // once it's already registered+persisted).
    editor.kb.registry.save(&tmp).unwrap();

    handle_collab_event(
        &mut editor,
        CollabEvent::KbShared {
            kb_id: "collabtest".to_string(),
            node_count: 1,
            collection_state: Vec::new(),
        },
    );

    // Simulate restart: load the registry fresh from disk.
    let reloaded = mae_kb::federation::KbRegistry::load(&tmp);
    let inst = reloaded
        .find("collabtest")
        .expect("instance survives reload");
    assert!(
        inst.shared && inst.collab_id.as_deref() == Some("collabtest"),
        "durable share marker must survive a registry save→load round-trip"
    );

    // A restarted editor (empty cache) with the reloaded registry: the emit
    // gate fires from the durable marker → edits still queue for broadcast.
    let mut restarted = Editor::new();
    restarted.kb.registry = reloaded;
    let mut inst2 = mae_kb::KnowledgeBase::new();
    let mut n = mae_kb::Node::new("collabtest:overview", "O", mae_kb::NodeKind::Note, "b");
    n.source = Some(mae_kb::NodeSource::Federation);
    inst2.insert(n);
    restarted.kb.instances.insert("uuid-ct".into(), inst2);
    restarted.collab.kb_sync_mode = "on_save".into();
    assert!(restarted.collab.shared_kbs.is_empty());

    restarted
        .kb_update_node(
            "collabtest:overview",
            Some("edited after restart"),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        restarted.collab.pending_kb_updates.len(),
        1,
        "post-restart edit must still queue a kb/node_update (durable gate)"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}
#[test]
fn collab_kb_joined_populates_tracking() {
    let mut editor = Editor::new();
    let tmp = std::env::temp_dir().join(format!("mae-adr019-join-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    editor.data_dir_override = Some(tmp.clone());

    // Create a CRDT node state + SV for the join event (ADR-022 reconcile).
    let doc = mae_sync::kb::KbNodeDoc::new("join-node-1", "Joined Title", "joined body", &[]);
    let state = doc.encode_state();
    let sv = doc.state_vector();

    handle_collab_event(
        &mut editor,
        CollabEvent::KbJoined {
            kb_id: "remote-kb".to_string(),
            collection_state: vec![],
            nodes: vec![JoinedNode {
                id: "join-node-1".to_string(),
                bytes: state,
                daemon_sv: Some(sv),
            }],
        },
    );

    assert!(
        editor.collab.shared_kbs.contains_key("remote-kb"),
        "shared_kbs should track the joined KB"
    );
    assert!(
        editor.collab.shared_kbs["remote-kb"].contains("join-node-1"),
        "shared_kbs should contain the joined node ID"
    );
    // ADR-019: joined KB is a FIRST-CLASS instance with durable markers, NOT
    // dumped into primary (fixes B-3).
    let inst = editor
        .kb
        .registry
        .find_by_collab_id("remote-kb")
        .expect("joined KB must be a registered instance");
    assert!(inst.shared && inst.collab_id.as_deref() == Some("remote-kb"));
    let uuid = inst.uuid.clone();
    assert!(
        editor.kb.instances[&uuid].get("join-node-1").is_some(),
        "joined node must live in the instance"
    );
    assert!(
        editor.kb.primary.get("join-node-1").is_none(),
        "joined node must NOT be dumped into primary"
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
#[test]
fn collab_kb_left_removes_tracking() {
    let mut editor = Editor::new();
    editor
        .collab
        .shared_kbs
        .insert("test-kb".to_string(), HashSet::from(["n1".to_string()]));

    handle_collab_event(
        &mut editor,
        CollabEvent::KbLeft {
            kb_id: "test-kb".to_string(),
        },
    );

    assert!(
        !editor.collab.shared_kbs.contains_key("test-kb"),
        "shared_kbs should be cleared after leaving"
    );
}
#[test]
fn collab_kb_update_node_generates_crdt_update_for_shared_node() {
    let mut editor = Editor::new();
    // Insert a node and mark it as shared.
    editor.kb.primary.insert(mae_kb::Node::new(
        "shared-node".to_string(),
        "Original Title".to_string(),
        mae_kb::NodeKind::Note,
        "original body".to_string(),
    ));
    // ADR-019: the durable primary-share marker is the gate authority.
    editor.kb.registry.primary_shared = true;
    editor.kb.registry.primary_collab_id = Some("my-kb".to_string());
    editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();

    // Update the node.
    editor
        .kb_update_node("shared-node", Some("New Title"), Some("new body"), None)
        .unwrap();

    // Should have a pending KB update.
    assert_eq!(
        editor.collab.pending_kb_updates.len(),
        1,
        "should generate one pending KB update"
    );
    let (kb_id, node_id, update_bytes) = &editor.collab.pending_kb_updates[0];
    assert_eq!(kb_id, "my-kb");
    assert_eq!(node_id, "shared-node");
    assert!(
        !update_bytes.is_empty(),
        "CRDT update bytes should be non-empty"
    );
}
#[test]
fn collab_kb_update_node_no_update_for_unshared_node() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "local-only".to_string(),
        "Title".to_string(),
        mae_kb::NodeKind::Note,
        "body".to_string(),
    ));
    editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();
    // No shared_kbs entry for this node.

    editor
        .kb_update_node("local-only", Some("Updated"), None, None)
        .unwrap();

    assert!(
        editor.collab.pending_kb_updates.is_empty(),
        "unshared node should not generate KB updates"
    );
}
#[test]
fn collab_kb_manual_sync_mode_suppresses_auto_update() {
    let mut editor = Editor::new();
    editor.kb.primary.insert(mae_kb::Node::new(
        "shared-node".to_string(),
        "Title".to_string(),
        mae_kb::NodeKind::Note,
        "body".to_string(),
    ));
    editor.collab.shared_kbs.insert(
        "my-kb".to_string(),
        HashSet::from(["shared-node".to_string()]),
    );
    editor.collab.kb_sync_mode = "manual".to_string();

    editor
        .kb_update_node("shared-node", Some("New Title"), None, None)
        .unwrap();

    assert!(
        editor.collab.pending_kb_updates.is_empty(),
        "manual sync mode should not auto-generate KB updates"
    );
}
