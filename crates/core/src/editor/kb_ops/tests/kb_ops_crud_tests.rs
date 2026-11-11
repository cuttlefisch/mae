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
        .agenda(&mae_kb::AgendaFilter::Todo(None))
        .expect("agenda query must succeed against a healthy store");
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
    let via_ql = editor
        .kb
        .query_layer()
        .unwrap()
        .history("user:h1", 50)
        .expect("history query must succeed against a healthy store");
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

/// The ADR KB's generator never stamped `NodeSource`, so its nodes carried
/// `source == None` — and this guard refuses only `Some(Seed)`. An in-editor
/// or MCP `kb_update` on an installed ADR node therefore **succeeded**, wrote
/// into the derived store, and was silently destroyed by the next
/// `make adr-kb` / install. Fixed by stamping at the single write boundary
/// (`kb_build::insert_nodes`) rather than at each generator's discretion.
///
/// Both halves are asserted deliberately. The second one pins the *mechanism*:
/// an identically-shaped node without the stamp is still editable, which is
/// what makes the stamp — not the id, not the `concept:adr-` namespace — the
/// thing doing the protecting. If someone "fixes" this by special-casing the
/// namespace in the guard, the second assertion should be what makes them
/// stop and think.
#[test]
fn kb_update_node_rejects_a_seed_stamped_adr_node_and_the_stamp_is_what_does_it() {
    let mut editor = Editor::new();

    let stamped = mae_kb::Node::new(
        "concept:adr-076-bundled-kb-system",
        "ADR-076",
        mae_kb::NodeKind::Concept,
        "Original body.",
    )
    .with_source(mae_kb::NodeSource::Seed, 1);
    editor.kb.primary.insert(stamped);

    let err = editor
        .kb_update_node(
            "concept:adr-076-bundled-kb-system",
            Some("Hijacked"),
            None,
            None,
        )
        .expect_err("a shipped ADR node must not be editable");
    assert!(err.contains("seed node"), "unexpected error: {err}");
    assert_eq!(
        editor
            .kb
            .primary
            .get("concept:adr-076-bundled-kb-system")
            .unwrap()
            .title,
        "ADR-076",
        "the refused edit must not have partially applied"
    );

    // The same shape WITHOUT the stamp — exactly what the ADR builder used to
    // emit — is editable. That is the bug, pinned rather than described.
    let unstamped = mae_kb::Node::new(
        "concept:adr-999-unstamped",
        "ADR-999",
        mae_kb::NodeKind::Concept,
        "Original body.",
    );
    assert_eq!(unstamped.source, None, "fixture must start unstamped");
    editor.kb.primary.insert(unstamped);
    editor
        .kb_update_node("concept:adr-999-unstamped", Some("Hijacked"), None, None)
        .expect("an unstamped node IS editable -- which is precisely why the stamp matters");
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
            project_root: None,
            project_key: None,
            kind: mae_kb::federation::KbInstanceKind::default(),
            ingest_policy: Default::default(),
            import_record: None,
            priority: 0,
            remote_hub: None,
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
            project_root: None,
            project_key: None,
            kind: mae_kb::federation::KbInstanceKind::default(),
            ingest_policy: Default::default(),
            import_record: None,
            priority: 0,
            remote_hub: None,
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

// ---- Instance targeting (a write aimed at a named KB lands there or fails
// loudly; it never lands in the primary KB). ----

/// An editor with two loaded, non-primary instances.
fn two_instances() -> (Editor, String, String, Vec<tempfile::TempDir>) {
    let a = create_test_org_dir();
    let b = create_test_org_dir();
    let mut editor = Editor::new();
    let dirs = with_test_dirs(&mut editor);
    let ua = editor.kb_register("Alpha", a.path()).unwrap().uuid;
    let ub = editor.kb_register("Beta", b.path()).unwrap().uuid;
    (editor, ua, ub, vec![a, b, dirs])
}

fn in_instance(editor: &Editor, uuid: &str, id: &str) -> bool {
    editor
        .kb
        .instances
        .get(uuid)
        .is_some_and(|kb| kb.get(id).is_some())
}

#[test]
fn a_create_aimed_at_an_unloaded_instance_fails_and_never_lands_in_primary() {
    let mut editor = Editor::new();
    let _dirs = with_test_dirs(&mut editor);
    let r = editor.kb_create_node_in(
        Some("uuid-not-loaded".into()),
        "plain-id",
        "T",
        "b",
        mae_kb::NodeKind::Note,
    );
    assert!(
        r.is_err(),
        "a create aimed at an unloaded instance succeeded"
    );
    assert!(
        editor.kb.primary.get("plain-id").is_none(),
        "it landed in the primary KB"
    );
}

#[test]
fn a_create_names_its_kb_whatever_its_id() {
    let (mut editor, ua, ub, _d) = two_instances();
    editor
        .kb_create_node_target(
            Some("Beta"),
            "3f9c-uuid-like",
            "T",
            "b",
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    assert!(
        in_instance(&editor, &ub, "3f9c-uuid-like"),
        "not in the named KB"
    );
    assert!(!in_instance(&editor, &ua, "3f9c-uuid-like"));
    assert!(
        editor.kb.primary.get("3f9c-uuid-like").is_none(),
        "landed in primary"
    );
    let err = editor
        .kb_create_node_target(Some("NoSuchKb"), "x", "T", "b", mae_kb::NodeKind::Note)
        .unwrap_err();
    assert!(err.contains("NoSuchKb"), "{err}");
    assert!(editor.kb.primary.get("x").is_none());
}

#[test]
fn an_update_or_delete_of_an_id_held_by_two_kbs_needs_a_kb() {
    let (mut editor, ua, ub, _d) = two_instances();
    for kb in ["Alpha", "Beta"] {
        editor
            .kb_create_node_target(Some(kb), "dup", kb, "b", mae_kb::NodeKind::Note)
            .unwrap();
    }
    let err = editor
        .kb_update_node("dup", Some("new"), None, None)
        .unwrap_err();
    assert!(err.contains("Alpha") && err.contains("Beta"), "{err}");
    editor
        .kb_update_node_target(Some("Beta"), "dup", Some("new"), None, None)
        .unwrap();
    let title = |u: &str| editor.kb.instances[u].get("dup").unwrap().title.clone();
    assert_eq!(
        (title(&ua), title(&ub)),
        ("Alpha".to_string(), "new".to_string())
    );
    assert!(
        editor.kb_delete_node("dup").is_err(),
        "ambiguous delete succeeded"
    );
    editor.kb_delete_node_target(Some("Alpha"), "dup").unwrap();
    assert!(!in_instance(&editor, &ua, "dup") && in_instance(&editor, &ub, "dup"));
}

#[test]
fn a_targeted_update_of_an_id_the_kb_does_not_hold_fails() {
    let (mut editor, _ua, _ub, _d) = two_instances();
    editor
        .kb_create_node_target(Some("Alpha"), "only-a", "T", "b", mae_kb::NodeKind::Note)
        .unwrap();
    let err = editor
        .kb_update_node_target(Some("Beta"), "only-a", Some("x"), None, None)
        .unwrap_err();
    assert!(err.contains("Beta"), "{err}");
}

// ---- Follow-ups to the above: the remaining writes (add-link, set-role, the
// ex commands, promote) and the node-addressed reads name their KB too. ----

/// `dup` in both Alpha and Beta, with different titles.
fn two_instances_with_dup() -> (Editor, String, String, Vec<tempfile::TempDir>) {
    let (mut editor, ua, ub, d) = two_instances();
    for kb in ["Alpha", "Beta"] {
        editor
            .kb_create_node_target(Some(kb), "dup", kb, "body", mae_kb::NodeKind::Note)
            .unwrap();
    }
    (editor, ua, ub, d)
}

fn body_in(editor: &Editor, uuid: &str) -> String {
    editor.kb.instances[uuid].get("dup").unwrap().body.clone()
}

fn role_in(editor: &Editor, uuid: &str) -> Option<String> {
    editor.kb.instances[uuid]
        .get("dup")
        .unwrap()
        .properties
        .get("role")
        .cloned()
}

#[test]
fn an_add_link_lands_in_the_named_kbs_node_or_fails() {
    let (mut editor, ua, ub, _d) = two_instances_with_dup();
    let err = editor
        .kb_add_link_target(None, "dup", "somewhere", "refs", 1.0)
        .unwrap_err();
    assert!(err.contains("Alpha") && err.contains("Beta"), "{err}");
    assert_eq!(
        (body_in(&editor, &ua), body_in(&editor, &ub)),
        ("body".into(), "body".into())
    );

    editor
        .kb_add_link_target(Some("Beta"), "dup", "somewhere", "refs", 1.0)
        .unwrap();
    assert!(
        body_in(&editor, &ub).contains("[[somewhere?rel=refs"),
        "not in Beta"
    );
    assert_eq!(body_in(&editor, &ua), "body", "Alpha's copy was changed");

    let err = editor
        .kb_add_link_target(Some("Nope"), "dup", "x", "refs", 1.0)
        .unwrap_err();
    assert!(err.contains("Nope"), "{err}");
}

#[test]
fn the_set_role_command_and_the_ex_writes_take_a_kb() {
    let (mut editor, ua, ub, _d) = two_instances_with_dup();
    // Unnamed, an id both KBs hold is refused and nothing changes.
    editor.ex_kb_set_role("dup hub");
    assert!(editor.status_msg.contains("Alpha"), "{}", editor.status_msg);
    assert_eq!((role_in(&editor, &ua), role_in(&editor, &ub)), (None, None));
    editor.ex_kb_set_role("--kb=Nope dup hub");
    assert!(editor.status_msg.contains("Nope"), "{}", editor.status_msg);
    assert_eq!((role_in(&editor, &ua), role_in(&editor, &ub)), (None, None));

    editor.ex_kb_set_role("--kb=Beta dup hub");
    assert_eq!(role_in(&editor, &ub).as_deref(), Some("hub"));
    assert_eq!(role_in(&editor, &ua), None, "Alpha's copy was changed");

    editor.ex_kb_update("--kb=Alpha dup new body text");
    assert_eq!(body_in(&editor, &ua), "new body text");
    assert_eq!(body_in(&editor, &ub), "body");

    editor.ex_kb_delete("dup");
    assert!(
        in_instance(&editor, &ua, "dup") && in_instance(&editor, &ub, "dup"),
        "ambiguous delete ran"
    );
    editor.ex_kb_delete("--kb=Alpha dup");
    assert!(!in_instance(&editor, &ua, "dup") && in_instance(&editor, &ub, "dup"));

    editor.ex_kb_create("--kb=Beta 3f9c-x A title");
    assert!(in_instance(&editor, &ub, "3f9c-x"), "not in the named KB");
    assert!(editor.kb.primary.get("3f9c-x").is_none());
    editor.ex_kb_create("--kb=Nope 3f9c-y A title");
    assert!(editor.kb.primary.get("3f9c-y").is_none() && !in_instance(&editor, &ub, "3f9c-y"));
    editor.ex_kb_create("--kb=Beta onlyword");
    assert!(editor.status_msg.contains("--kb"), "{}", editor.status_msg);
}

#[test]
fn a_promote_names_the_instance_it_copies_from() {
    let (mut editor, _ua, ub, _d) = two_instances_with_dup();
    editor
        .kb_set_role_target(Some("Beta"), "dup", "atom")
        .unwrap();
    let err = editor.kb_promote_node("dup").unwrap_err();
    assert!(err.contains("Alpha") && err.contains("Beta"), "{err}");
    assert!(editor.kb.primary.get("dup").is_none());
    let err = editor
        .kb_promote_node_from(Some("Alpha"), "nope")
        .unwrap_err();
    assert!(err.contains("Alpha"), "{err}");

    let r = editor.kb_promote_node_from(Some("Beta"), "dup").unwrap();
    assert_eq!(r.promoted_from_uuid, ub);
    assert_eq!(editor.kb.primary.get("dup").unwrap().title, "Beta");
}

#[test]
fn a_read_names_its_kb_or_is_refused_when_ambiguous() {
    let (mut editor, ua, ub, _d) = two_instances_with_dup();
    editor
        .kb_create_node_target(Some("Beta"), "only-b", "T", "b", mae_kb::NodeKind::Note)
        .unwrap();
    let err = editor.kb_read_scope("dup", None).unwrap_err();
    assert!(err.contains("Alpha") && err.contains("Beta"), "{err}");
    assert_eq!(
        editor.kb_read_scope("dup", Some("Beta")).unwrap(),
        Some(Some(ub))
    );
    assert_eq!(
        editor.kb_read_scope("dup", Some("Alpha")).unwrap(),
        Some(Some(ua))
    );
    assert!(editor
        .kb_read_scope("dup", Some("Nope"))
        .unwrap_err()
        .contains("Nope"));
    assert!(editor
        .kb_read_scope("dup", Some("local"))
        .unwrap_err()
        .contains("local"));
    // An id held by one KB resolves unqualified, as before.
    assert_eq!(editor.kb_read_scope("only-b", None).unwrap(), None);
}

#[test]
fn a_named_local_read_answers_from_the_primary_not_an_instance() {
    let (mut editor, _ua, _ub, _d) = two_instances_with_dup();
    editor
        .kb_create_node_target(
            Some("local"),
            "dup",
            "Local copy",
            "b",
            mae_kb::NodeKind::Note,
        )
        .unwrap();
    let got = editor.kb_get_in("local", "dup").unwrap().unwrap();
    assert_eq!(got.node.title, "Local copy");
    let got = editor.kb_get_in("Beta", "dup").unwrap().unwrap();
    assert_eq!(got.node.title, "Beta");
    let err = editor.kb_read_scope("dup", None).unwrap_err();
    assert!(
        err.contains("local") && err.contains("Alpha") && err.contains("Beta"),
        "{err}"
    );
}

/// `:kb-restore` (and so Scheme `kb-restore`, which queues it) restores in
/// the KB it names, through the same resolver as the `kb_restore` tool. Kills:
/// restoring the primary's store whatever the flag says, or a silent fallback
/// for an unknown KB.
#[test]
fn the_restore_command_acts_on_the_named_kbs_store() {
    let (mut editor, _ua, ub, _d) = two_instances_with_dup();
    // History is written by authored ingest (`insert_node_with_history`), not
    // by an ex update; snapshot Beta's original directly.
    let v = editor.kb.instance_stores[&ub]
        .snapshot_version("dup", "before the edit")
        .unwrap();
    editor.ex_kb_update("--kb=Beta dup edited in beta");
    editor.ex_kb_update("--kb=Alpha dup edited in alpha");
    let body = |e: &Editor, kb: &str| {
        e.kb_store_named(kb)
            .unwrap()
            .get_node("dup")
            .unwrap()
            .unwrap()
            .body
    };
    assert_eq!(body(&editor, "Beta"), "edited in beta");

    editor.ex_kb_restore(&format!("--kb=Nope dup {v}"));
    assert!(editor.status_msg.contains("Nope"), "{}", editor.status_msg);
    assert_eq!(
        body(&editor, "Beta"),
        "edited in beta",
        "an unknown KB fell back"
    );

    editor.ex_kb_restore(&format!("--kb=Beta dup {v}"));
    assert_eq!(body(&editor, "Beta"), "body", "{}", editor.status_msg);
    assert_eq!(
        body(&editor, "Alpha"),
        "edited in alpha",
        "Alpha's store was touched"
    );
}
