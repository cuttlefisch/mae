use super::*;

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
