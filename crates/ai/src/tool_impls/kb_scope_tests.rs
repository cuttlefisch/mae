//! Scope membership is decided by `KbScope`, never by `KbInstance.primary` (#814).
//!
//! `primary: bool` means "the first row ever registered on this machine"
//! (ADR-110 D7). Health, agenda, search and `node_matches_scope` each read it
//! as "the local store", so after the KB cutover — when the flagged KB has a
//! store of its own — that KB vanished from `all` health and agenda, and
//! scoping to it by name returned the LOCAL store's nodes. Seen live: a search
//! scoped to a real KB answered with manual pages.
//!
//! The fixture registers three KBs with the flagged one deliberately NOT first,
//! gives each a durable store AND an in-memory mirror (agenda reads the former,
//! health the latter, as in production), and puts a distinguishable node in the
//! local store, so every oracle can tell "its own node" from "the local one".

use mae_core::Editor;
use mae_kb::federation::KbInstance;
use mae_kb::{CozoKbStore, KbScope, KbStore, Node, NodeKind};
use std::sync::Arc;

use super::kb::{execute_kb_agenda, execute_kb_health};

const LOCAL_ID: &str = "localonly:scope-probe";

fn todo(id: &str) -> Node {
    let mut n = Node::new(id, "scope probe", NodeKind::Note, "scope probe body");
    n.todo_state = Some("TODO".into());
    n
}

fn add_instance(editor: &mut Editor, uuid: &str, name: &str, primary: bool, shared: bool) {
    let mut inst = KbInstance::local(
        uuid.into(),
        name.into(),
        Default::default(),
        Default::default(),
    );
    inst.primary = primary;
    inst.shared = shared;
    editor.kb.registry.instances.push(inst);

    let node = todo(&format!("{}:scope-probe", name.to_lowercase()));
    let store = CozoKbStore::open_mem().unwrap();
    store.insert_node(&node).unwrap();
    editor
        .kb
        .instance_stores
        .insert(uuid.into(), Arc::new(store));
    let mut mirror = mae_core::KnowledgeBase::new();
    mirror.insert(node);
    editor.kb.instances.insert(uuid.into(), mirror);
}

/// Alpha (plain), Flagged (`primary`, registered SECOND), Shared (remote).
fn three_kbs() -> Editor {
    let mut editor = Editor::new();
    editor.kb.primary.insert(todo(LOCAL_ID));
    add_instance(&mut editor, "uuid-alpha", "Alpha", false, false);
    add_instance(&mut editor, "uuid-flagged", "Flagged", true, false);
    add_instance(&mut editor, "uuid-shared", "Shared", false, true);
    editor
}

fn health_names(editor: &Editor, scope: &str) -> Vec<String> {
    let out = execute_kb_health(editor, &serde_json::json!({ "scope": scope }), None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let mut names: Vec<String> = v["instances"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_string())
        .collect();
    names.sort();
    names
}

fn agenda_ids(editor: &Editor, scope: &str) -> Vec<String> {
    let out = execute_kb_agenda(
        editor,
        &serde_json::json!({ "filter": "todo", "scope": scope }),
        None,
    )
    .unwrap();
    let mut ids: Vec<String> = ["id", "node_id"]
        .iter()
        .flat_map(|k| {
            let key = format!("\"{k}\": \"");
            out.match_indices(&key)
                .map(|(i, m)| {
                    let rest = &out[i + m.len()..];
                    rest[..rest.find('"').unwrap()].to_string()
                })
                .collect::<Vec<_>>()
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[test]
fn health_all_includes_the_primary_flagged_kb() {
    let editor = three_kbs();
    assert_eq!(
        health_names(&editor, "all"),
        vec!["Alpha", "Flagged", "Shared"],
        "every registered KB is in `all` — the flagged one used to be dropped"
    );
}

#[test]
fn agenda_all_includes_the_primary_flagged_kbs_todos() {
    let editor = three_kbs();
    let ids = agenda_ids(&editor, "all");
    assert!(
        ids.contains(&"flagged:scope-probe".to_string()),
        "the flagged KB's TODO must be on the agenda: {ids:?}"
    );
    assert!(
        ids.contains(&LOCAL_ID.to_string()),
        "and the local store's: {ids:?}"
    );
}

#[test]
fn scoping_to_the_flagged_kb_by_name_returns_its_own_nodes_not_the_local_stores() {
    let editor = three_kbs();
    let scope = KbScope::parse("Flagged");
    assert!(editor.kb.node_matches_scope("flagged:scope-probe", &scope));
    assert!(
        !editor.kb.node_matches_scope(LOCAL_ID, &scope),
        "the name used to be aliased to the LOCAL store"
    );

    let hits: Vec<String> = editor
        .kb_federated_search_scoped("scope probe", &scope)
        .into_iter()
        .map(|(_, n)| n.id)
        .collect();
    assert!(
        hits.contains(&"flagged:scope-probe".to_string()),
        "{hits:?}"
    );
    assert!(!hits.contains(&LOCAL_ID.to_string()), "{hits:?}");
}

/// `remote` means `is_remote()` everywhere, as documented to callers — health
/// used to read it as "every non-primary KB".
#[test]
fn remote_scope_means_the_shared_kbs_everywhere() {
    let editor = three_kbs();
    assert_eq!(health_names(&editor, "remote"), vec!["Shared"]);
    let scope = KbScope::parse("remote");
    assert!(editor.kb.node_matches_scope("shared:scope-probe", &scope));
    assert!(!editor.kb.node_matches_scope("alpha:scope-probe", &scope));
    assert!(!editor.kb.node_matches_scope(LOCAL_ID, &scope));
}

/// The aliases still address the local store — the one legitimate meaning.
#[test]
fn primary_and_default_address_the_local_store() {
    let editor = three_kbs();
    for alias in ["primary", "default", "local"] {
        let scope = KbScope::parse(alias);
        assert!(scope.includes_local(), "{alias}");
        assert!(editor.kb.node_matches_scope(LOCAL_ID, &scope), "{alias}");
        assert!(
            !editor.kb.node_matches_scope("flagged:scope-probe", &scope),
            "{alias} must not reach the flagged KB"
        );
    }
}
