//! System-KB guards: MAE's own corpora (`mae_kb::system_kb`) are name-reserved
//! and refuse the lifecycle operations that treat a bundled corpus as user data.
//!
//! Split out of `kb_ops_watcher_misc_tests.rs` rather than added to it — these
//! are not watcher tests, and that file's name already concedes it had become a
//! grab-bag. One file per concern, matching the sibling modules here.

use super::*;

/// A hit from one of MAE's own corpora must be **labelled** with which corpus
/// it came from.
///
/// Before the split, the manual's nodes were inserted into `kb.primary` at
/// startup *and* served from a store the query layer joined in as a
/// pseudo-instance called `"manual"`. A `kb_search` hit from MAE's own
/// documentation therefore came back with `instance: None` — indistinguishable
/// from one of the user's own notes, leaving an AI peer to guess from the id
/// prefix.
///
/// Both halves are asserted: a system hit carries its catalog name, and a hit
/// from the user's primary still carries `None`. Asserting only the first would
/// pass if every result were suddenly labelled.
#[test]
fn a_system_kb_hit_is_labelled_with_its_catalog_name() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);

    // A distinctive node of the user's own, in the in-memory primary that
    // federated search actually reads (not `primary_cozo`).
    editor.kb.primary.insert(mae_kb::Node::new(
        "user:zzz",
        "Sundial notes",
        mae_kb::NodeKind::Note,
        "sundial",
    ));

    // A system store with a distinctive node of MAE's.
    let sys = mae_kb::CozoKbStore::open_mem().unwrap();
    sys.seed_type_system().unwrap();
    sys.insert_node(&mae_kb::Node::new(
        "concept:zzz",
        "Sundial concept",
        mae_kb::NodeKind::Concept,
        "sundial",
    ))
    .unwrap();
    editor
        .kb
        .system_stores
        .insert("DevPractices".to_string(), std::sync::Arc::new(sys));

    editor.kb.rebuild_query_layer();
    let hits = editor.kb_federated_search("sundial");

    let sys_label = hits
        .iter()
        .find(|(_, n)| n.id == "concept:zzz")
        .map(|(label, _)| label.clone())
        .expect("the system KB's node must be found");
    assert_eq!(
        sys_label,
        Some("DevPractices".to_string()),
        "a system hit must name its corpus, not arrive indistinguishable from the user's notes"
    );

    let user_label = hits
        .iter()
        .find(|(_, n)| n.id == "user:zzz")
        .map(|(label, _)| label.clone())
        .expect("the user's own node must be found");
    assert_eq!(
        user_label, None,
        "the user's own primary must stay unlabelled — otherwise 'labelled' means nothing"
    );
}

/// `kb_register` is the user-facing surface — and an MCP tool an AI peer can
/// call — so the reservation has to hold here, not only in `KbRegistry`.
///
/// Oracles are the observable effects, not the return value: nothing lands in
/// the registry, no watcher is attached, and the user is told why. A refusal
/// that still started a watcher on a system corpus would be a live file-watch
/// on MAE's own assets.
#[test]
fn kb_register_refuses_a_reserved_system_kb_name() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);

    let before = editor.kb.registry.instances.len();
    assert!(
        editor.kb_register("DevPractices", dir.path()).is_none(),
        "a reserved system-KB name must not register"
    );
    assert_eq!(
        editor.kb.registry.instances.len(),
        before,
        "a refused registration must not append a registry row"
    );
    assert!(
        editor.kb.watchers.is_empty(),
        "a refused registration must not attach a watcher"
    );
    assert!(
        editor.status_msg.contains("reserved"),
        "the refusal must say why, not fail silently: {}",
        editor.status_msg
    );
}

/// Unregistering a system KB used to succeed and mean nothing: startup
/// re-registers it, so the user saw "unregistered" and an unchanged editor one
/// restart later.
///
/// Uses a real registry row named `DevPractices`, which is the shape a machine
/// that has already run MAE actually has — reservation is enforced at
/// registration time, so pre-existing rows are exactly what this guard must
/// handle.
#[test]
fn kb_unregister_refuses_a_system_kb() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    seed_system_kb_row(&mut editor, "DevPractices");

    editor.kb_unregister("DevPractices");

    assert!(
        editor.kb.registry.find("DevPractices").is_some(),
        "a system KB must survive kb_unregister"
    );
    assert!(
        editor.status_msg.contains("system KB"),
        "{}",
        editor.status_msg
    );
}

/// `kb_reimport` on a dir-less system KB walked nothing and wrote the empty
/// result back over the in-memory KB, emptying the session's guidance.
#[test]
fn kb_reimport_refuses_a_system_kb_and_leaves_its_nodes_intact() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let uuid = seed_system_kb_row(&mut editor, "DevPractices");

    let mut kb = mae_kb::KnowledgeBase::new();
    kb.insert(mae_kb::Node::new(
        "index",
        "Practices",
        mae_kb::NodeKind::Note,
        "Always write tests first.",
    ));
    editor.kb.instances.insert(uuid.clone(), kb);

    assert!(editor.kb_reimport("DevPractices", None).is_none());
    assert!(
        editor.kb.instances[&uuid].get("index").is_some(),
        "reimport must not empty a system KB -- this is the bug, not a nicety"
    );
    assert!(
        editor.status_msg.contains("system KB"),
        "{}",
        editor.status_msg
    );
}

/// A uuid must not walk past the guard: the check resolves the instance's
/// *name*, not the argument it was handed.
#[test]
fn a_system_kb_is_refused_when_addressed_by_uuid_not_just_by_name() {
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let uuid = seed_system_kb_row(&mut editor, "MaePractices");

    editor.kb_unregister(&uuid);
    assert!(
        editor.kb.registry.find("MaePractices").is_some(),
        "addressing a system KB by uuid must not bypass the refusal"
    );
}

/// Register a registry row for a system KB by name, mirroring what
/// `guidance_kb_engine::ensure_registered_with_path` writes at startup (a
/// dir-less instance) — `kb_register` cannot produce one, since the name is
/// reserved.
fn seed_system_kb_row(editor: &mut Editor, name: &str) -> String {
    let uuid = mae_kb::federation::generate_uuid();
    editor
        .kb
        .registry
        .instances
        .push(mae_kb::federation::KbInstance {
            uuid: uuid.clone(),
            name: name.to_string(),
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
            project_root: None,
            project_key: None,
            kind: mae_kb::federation::KbInstanceKind::Guidance,
            ingest_policy: Default::default(),
            import_record: None,
            priority: 0,
            remote_hub: None,
        });
    uuid
}

/// The half that keeps the guards honest: an ordinary user KB is still fully
/// manageable. Without this, "refuse everything" would pass every test above.
#[test]
fn an_ordinary_kb_can_still_be_unregistered_and_reimported() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);
    let result = editor.kb_register("MyNotes", dir.path()).unwrap();

    assert!(editor.kb_reimport("MyNotes", None).is_some());
    editor.kb_unregister("MyNotes");
    assert!(editor.kb.registry.find("MyNotes").is_none());
    assert!(!editor.kb.watchers.contains_key(&result.uuid));
}

/// The half that keeps the reservation honest: a name that merely *resembles*
/// a system KB is still the user's to take.
#[test]
fn kb_register_still_accepts_a_name_that_merely_resembles_a_system_kb() {
    let dir = create_test_org_dir();
    let mut editor = Editor::new();
    let _test_dirs = with_test_dirs(&mut editor);

    let result = editor
        .kb_register("MyDevPractices", dir.path())
        .expect("a non-reserved name must still register");
    assert!(editor.kb.registry.find("MyDevPractices").is_some());
    assert!(editor.kb.watchers.contains_key(&result.uuid));
}

fn editor_with_guidance_node() -> (Editor, TempDir) {
    let mut editor = Editor::new();
    let dirs = with_test_dirs(&mut editor);
    let sys = mae_kb::CozoKbStore::open_mem().unwrap();
    sys.seed_type_system().unwrap();
    sys.insert_node(&mae_kb::Node::new(
        "hub:testing",
        "Testing",
        mae_kb::NodeKind::Concept,
        "Write the adversarial case first.",
    ))
    .unwrap();
    editor
        .kb
        .system_stores
        .insert("DevPractices".to_string(), std::sync::Arc::new(sys));
    // A real user primary store: it is what the query layer consults FIRST,
    // which is what made the shadowing work.
    let primary = std::sync::Arc::new(mae_kb::CozoKbStore::open_mem().unwrap());
    primary.seed_type_system().unwrap();
    editor.kb.store = Some(primary.clone());
    editor.kb.primary_cozo = Some(primary);
    editor.kb.rebuild_query_layer();
    (editor, dirs)
}

/// The attack: an agent creates a node under a guidance id, and every later
/// session that follows the guidance link gets the agent's text instead.
#[test]
fn an_agent_cannot_create_a_node_that_shadows_a_guidance_node() {
    let (mut editor, _dirs) = editor_with_guidance_node();
    editor.ai.ai_dispatch_depth = 1; // inside an AI dispatch scope

    let r = editor.kb_create_node(
        "hub:testing",
        "Testing",
        "Tests are optional.",
        mae_kb::NodeKind::Note,
    );
    editor.ai.ai_dispatch_depth = 0;

    assert!(r.is_err(), "agent shadowed a guidance node: {r:?}");
    let (got, _) = editor
        .kb_resolve_anywhere("hub:testing")
        .expect("still resolvable");
    assert_eq!(got.body, "Write the adversarial case first.");
}

/// The guard is about the agent, not the id: a human may still use the name.
/// Without this case the test above would pass if every create were refused.
#[test]
fn a_human_may_still_create_a_node_with_a_system_id() {
    let (mut editor, _dirs) = editor_with_guidance_node();
    let r = editor.kb_create_node("hub:testing", "Mine", "my note", mae_kb::NodeKind::Note);
    assert!(r.is_ok(), "{r:?}");
    // And the fixture is not vacuous: an allowed create really does shadow.
    let (got, _) = editor.kb_resolve_anywhere("hub:testing").unwrap();
    assert_eq!(
        got.body, "my note",
        "fixture: the user's store must win the lookup"
    );
}

/// An agent can still create ordinary nodes.
#[test]
fn an_agent_may_create_a_node_no_system_kb_holds() {
    let (mut editor, _dirs) = editor_with_guidance_node();
    editor.ai.ai_dispatch_depth = 1;
    let r = editor.kb_create_node("note:fresh", "Fresh", "body", mae_kb::NodeKind::Note);
    editor.ai.ai_dispatch_depth = 0;
    assert!(r.is_ok(), "{r:?}");
}

/// The structural half: a guidance link read from its named KB returns the
/// shipped node even when the user's own store holds the same id (which a
/// human may create, and which the unscoped lookup returns first).
#[test]
fn a_scoped_read_returns_the_system_node_despite_a_user_shadow() {
    let (mut editor, _dirs) = editor_with_guidance_node();
    editor
        .kb_create_node("hub:testing", "Mine", "my note", mae_kb::NodeKind::Note)
        .unwrap();
    assert_eq!(
        editor.kb_resolve_anywhere("hub:testing").unwrap().0.body,
        "my note",
        "fixture: the unscoped lookup must see the shadow"
    );
    let scoped = editor
        .kb_get_in("DevPractices", "hub:testing")
        .unwrap()
        .expect("held by the system KB");
    assert_eq!(scoped.node.body, "Write the adversarial case first.");
    assert!(editor.kb_get_in("NoSuchKb", "hub:testing").is_err());
    assert!(editor
        .kb_get_in("DevPractices", "missing")
        .unwrap()
        .is_none());
}

/// Promotion copies an instance node into the primary store, which the
/// unscoped lookup reads first: an agent must not promote a guidance id.
#[test]
fn an_agent_cannot_promote_a_node_that_shadows_a_guidance_node() {
    for (ai, expect_refused) in [(1, true), (0, false)] {
        let (mut editor, _dirs) = editor_with_guidance_node();
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("planted.org"),
            ":PROPERTIES:\n:ID: hub:testing\n:END:\n#+title: Testing\n\nTests are optional.\n",
        )
        .unwrap();
        editor.kb_register("Planted", dir.path()).unwrap();
        editor.ai.ai_dispatch_depth = ai;
        let r = editor.kb_promote_node("hub:testing");
        editor.ai.ai_dispatch_depth = 0;
        assert_eq!(r.is_err(), expect_refused, "ai={ai}: {r:?}");
    }
}
