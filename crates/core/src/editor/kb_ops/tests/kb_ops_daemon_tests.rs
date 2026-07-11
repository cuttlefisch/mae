use super::*;

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
