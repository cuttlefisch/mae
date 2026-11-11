//! A KB read or write aimed at a NAMED KB answers from that KB or fails (the
//! read half of #861's "a write lands in the KB it names").
//!
//! Two real instances, each with a durable store AND an in-memory mirror, both
//! hold an id (`dup`) with a DIFFERENT title, body and links, so every oracle
//! can tell which KB answered. A third id exists in one KB only.

use mae_core::Editor;
use mae_kb::federation::KbInstance;
use mae_kb::{CozoKbStore, KbStore, Node, NodeKind};
use std::sync::Arc;

use super::kb::*;

const DUP: &str = "dup";

fn add_instance(editor: &mut Editor, uuid: &str, name: &str, extra_only_here: &str) {
    editor.kb.registry.instances.push(KbInstance::local(
        uuid.into(),
        name.into(),
        Default::default(),
        Default::default(),
    ));
    let target = format!("{}-target", name.to_lowercase());
    let nodes = [
        Node::new(
            DUP,
            format!("{name} dup"),
            NodeKind::Note,
            format!("{name} body [[{target}]]"),
        ),
        Node::new(&target, format!("{name} target"), NodeKind::Note, "t"),
        Node::new(extra_only_here, "only here", NodeKind::Note, "o"),
    ];
    let store = CozoKbStore::open_mem().unwrap();
    store.seed_type_system().unwrap();
    let mut mirror = mae_core::KnowledgeBase::new();
    for n in nodes {
        store.insert_node(&n).unwrap();
        mirror.insert(n);
    }
    editor
        .kb
        .instance_stores
        .insert(uuid.into(), Arc::new(store));
    editor.kb.instances.insert(uuid.into(), mirror);
}

fn two_kbs() -> Editor {
    let mut editor = Editor::new();
    add_instance(&mut editor, "uuid-alpha", "Alpha", "only-a");
    add_instance(&mut editor, "uuid-beta", "Beta", "only-b");
    editor
}

fn json(r: Result<String, String>) -> serde_json::Value {
    serde_json::from_str(&r.unwrap()).unwrap()
}

fn ids(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|o| o[key].as_str().unwrap().to_string())
        .collect()
}

/// The refusals every kb-addressed read shares, whatever its output.
fn assert_target_errors(f: &dyn Fn(&Editor, serde_json::Value) -> Result<String, String>) {
    let editor = two_kbs();
    let e = f(
        &editor,
        serde_json::json!({"id": DUP, "from": DUP, "to": DUP}),
    )
    .unwrap_err();
    assert!(e.contains("Alpha") && e.contains("Beta"), "ambiguous: {e}");
    let e = f(
        &editor,
        serde_json::json!({"id": DUP, "from": DUP, "to": DUP, "kb": "Nope"}),
    )
    .unwrap_err();
    assert!(e.contains("Nope"), "unknown KB: {e}");
    let e = f(
        &editor,
        serde_json::json!({"id": "only-b", "from": "only-b", "to": "only-b", "kb": "Alpha"}),
    )
    .unwrap_err();
    assert!(e.contains("Alpha") && e.contains("only-b"), "not held: {e}");
}

#[test]
fn links_from_and_to_answer_from_the_named_kb() {
    let editor = two_kbs();
    for (kb, target) in [("Alpha", "alpha-target"), ("Beta", "beta-target")] {
        let from = json(execute_kb_links_from(
            &editor,
            &serde_json::json!({"id": DUP, "kb": kb}),
            None,
        ));
        assert_eq!(ids(&from, "dst"), vec![target.to_string()], "{kb}");
        let to = json(execute_kb_links_to(
            &editor,
            &serde_json::json!({"id": target, "kb": kb}),
            None,
        ));
        assert_eq!(ids(&to, "src"), vec![DUP.to_string()], "{kb}");
    }
    // An id held by exactly one KB resolves unqualified, as before.
    let one = json(execute_kb_links_from(
        &editor,
        &serde_json::json!({"id": "only-a"}),
        None,
    ));
    assert!(one.as_array().unwrap().is_empty());
    assert_target_errors(&|e, a| execute_kb_links_from(e, &a, None));
    assert_target_errors(&|e, a| execute_kb_links_to(e, &a, None));
}

#[test]
fn related_and_graph_answer_from_the_named_kb() {
    let editor = two_kbs();
    for (kb, target) in [("Alpha", "alpha-target"), ("Beta", "beta-target")] {
        let g = json(execute_kb_graph(
            &editor,
            &serde_json::json!({"id": DUP, "kb": kb}),
            None,
        ));
        let nodes = ids(&g["nodes"], "id");
        assert!(nodes.contains(&target.to_string()), "{kb}: {nodes:?}");
        let other = if kb == "Alpha" {
            "beta-target"
        } else {
            "alpha-target"
        };
        assert!(!nodes.contains(&other.to_string()), "{kb} leaked {other}");
        let titles: Vec<String> = g["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["id"] == DUP)
            .map(|n| n["title"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(titles, vec![format!("{kb} dup")]);
        let r = json(execute_kb_related(
            &editor,
            &serde_json::json!({"id": DUP, "kb": kb}),
            None,
        ));
        assert!(r.is_array());
    }
    assert_target_errors(&|e, a| execute_kb_graph(e, &a, None));
    assert_target_errors(&|e, a| execute_kb_related(e, &a, None));
}

#[test]
fn neighborhood_and_shortest_path_walk_the_named_kbs_store() {
    let editor = two_kbs();
    for (kb, target) in [("Alpha", "alpha-target"), ("Beta", "beta-target")] {
        let n = json(execute_kb_neighborhood(
            &editor,
            &serde_json::json!({"id": DUP, "kb": kb}),
            None,
        ));
        let nodes = ids(&n["nodes"], "id");
        assert!(nodes.contains(&target.to_string()), "{kb}: {nodes:?}");
        assert_eq!(nodes.len(), 2, "{kb}: only its own KB's nodes: {nodes:?}");
        let p = json(execute_kb_shortest_path(
            &editor,
            &serde_json::json!({"from": DUP, "to": target, "kb": kb}),
            None,
        ));
        assert_eq!(p, serde_json::json!([DUP, target]), "{kb}");
    }
    assert_target_errors(&|e, a| execute_kb_neighborhood(e, &a, None));
    assert_target_errors(&|e, a| execute_kb_shortest_path(e, &a, None));
}

#[test]
fn history_restore_and_raw_query_act_on_the_named_kbs_store() {
    let editor = two_kbs();
    // Alpha is edited once, Beta twice, so each KB's history has its own length.
    let edit = |u: &str, body: &str| {
        let store = &editor.kb.instance_stores[u];
        let mut n = store.get_node(DUP).unwrap().unwrap();
        store.snapshot_version(DUP, "pre-edit").unwrap();
        n.body = body.into();
        store.update_node(&n).unwrap();
    };
    edit("uuid-alpha", "Alpha v2");
    edit("uuid-beta", "Beta v2");
    edit("uuid-beta", "Beta v3");

    let versions = |kb: &str| {
        json(execute_kb_history(
            &editor,
            &serde_json::json!({"id": DUP, "kb": kb}),
        ))["version_count"]
            .as_u64()
            .unwrap()
    };
    let (a, b) = (versions("Alpha"), versions("Beta"));
    assert!(
        a > 0 && b == a + 1,
        "each KB reports its own history: {a} vs {b}"
    );
    // Unnamed, they act on the primary store, which this editor does not have.
    let e = execute_kb_history(&editor, &serde_json::json!({"id": DUP})).unwrap_err();
    assert!(e.contains("No KB store"), "{e}");

    execute_kb_restore(
        &editor,
        &serde_json::json!({"id": DUP, "version": 1, "kb": "Beta"}),
    )
    .unwrap();
    let body = |u: &str| {
        editor.kb.instance_stores[u]
            .get_node(DUP)
            .unwrap()
            .unwrap()
            .body
    };
    assert_eq!(
        body("uuid-beta"),
        "Beta body [[beta-target]]",
        "restored in Beta"
    );
    assert_eq!(body("uuid-alpha"), "Alpha v2", "Alpha untouched");

    let q = json(execute_kb_raw_query(
        &editor,
        &serde_json::json!({"query": "?[id] := *nodes{id}", "kb": "Alpha"}),
    ));
    let rows = q["rows"].to_string();
    assert!(
        rows.contains("alpha-target") && !rows.contains("beta-target"),
        "{rows}"
    );

    for f in [
        execute_kb_history(&editor, &serde_json::json!({"id": DUP, "kb": "Nope"})),
        execute_kb_restore(
            &editor,
            &serde_json::json!({"id": DUP, "version": 1, "kb": "Nope"}),
        ),
        execute_kb_raw_query(
            &editor,
            &serde_json::json!({"query": "?[id] := *nodes{id}", "kb": "Nope"}),
        ),
    ] {
        assert!(f.unwrap_err().contains("Nope"));
    }
}

#[test]
fn preview_and_graph_view_open_use_the_named_kb() {
    let mut editor = two_kbs();
    editor.open_help_at("index"); // the preview needs a KB-kind active buffer
    for kb in ["Alpha", "Beta"] {
        let out = execute_kb_preview_show(&mut editor, &serde_json::json!({"id": DUP, "kb": kb}))
            .unwrap();
        assert!(out.contains(&format!("{kb} dup")), "{kb}: {out}");
    }
    let e = execute_kb_preview_show(&mut editor, &serde_json::json!({"id": DUP})).unwrap_err();
    assert!(e.contains("Alpha") && e.contains("Beta"), "{e}");
    let e = execute_kb_preview_show(&mut editor, &serde_json::json!({"id": DUP, "kb": "Nope"}))
        .unwrap_err();
    assert!(e.contains("Nope"), "{e}");

    let scope_before = editor.kb.search_scope.clone();
    for (kb, uuid) in [("Alpha", "uuid-alpha"), ("Beta", "uuid-beta")] {
        let v = json(execute_kb_graph_view_open(
            &mut editor,
            &serde_json::json!({"id": DUP, "kb": kb}),
        ));
        assert_eq!(v["kb_instance"], uuid, "{kb}");
    }
    assert_eq!(
        editor.kb.search_scope, scope_before,
        "the persistent scope is untouched"
    );
    let e = execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": DUP})).unwrap_err();
    assert!(e.contains("Alpha") && e.contains("Beta"), "{e}");
    let e = execute_kb_graph_view_open(&mut editor, &serde_json::json!({"id": DUP, "kb": "Nope"}))
        .unwrap_err();
    assert!(e.contains("Nope"), "{e}");
}

#[test]
fn promote_takes_the_named_kbs_copy() {
    let mut editor = two_kbs();
    let e = execute_kb_promote(&mut editor, &serde_json::json!({"id": DUP})).unwrap_err();
    assert!(e.contains("Alpha") && e.contains("Beta"), "{e}");
    assert!(
        editor.kb.primary.get(DUP).is_none(),
        "a refused promote wrote nothing"
    );
    let e =
        execute_kb_promote(&mut editor, &serde_json::json!({"id": DUP, "kb": "Nope"})).unwrap_err();
    assert!(e.contains("Nope"), "{e}");
    let e = execute_kb_promote(
        &mut editor,
        &serde_json::json!({"id": "only-a", "kb": "Beta"}),
    )
    .unwrap_err();
    assert!(e.contains("Beta") && e.contains("only-a"), "{e}");
    execute_kb_promote(&mut editor, &serde_json::json!({"id": DUP, "kb": "Beta"})).unwrap();
    assert_eq!(editor.kb.primary.get(DUP).unwrap().title, "Beta dup");
}

#[test]
fn add_link_appends_to_the_named_kbs_node() {
    let mut editor = two_kbs();
    let args = |kb: Option<&str>| {
        let mut a = serde_json::json!({"src": DUP, "dst": "elsewhere", "rel_type": "refs"});
        if let Some(kb) = kb {
            a["kb"] = kb.into();
        }
        a
    };
    let e = execute_kb_add_link(&mut editor, &args(None)).unwrap_err();
    assert!(e.contains("Alpha") && e.contains("Beta"), "{e}");
    execute_kb_add_link(&mut editor, &args(Some("Beta"))).unwrap();
    let body = |u: &str| editor.kb.instances[u].get(DUP).unwrap().body.clone();
    assert!(
        body("uuid-beta").contains("[[elsewhere?rel=refs"),
        "not in Beta"
    );
    assert_eq!(
        body("uuid-alpha"),
        "Alpha body [[alpha-target]]",
        "Alpha changed"
    );
    assert!(execute_kb_add_link(&mut editor, &args(Some("Nope")))
        .unwrap_err()
        .contains("Nope"));
}
