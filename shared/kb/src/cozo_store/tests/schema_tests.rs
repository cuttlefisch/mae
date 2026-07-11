use super::*;

#[test]
fn schema_creates_all_relations() {
    let (_tmp, store) = make_store();
    // Verify all Phase B relations exist by querying them
    let relations = [
        "node_types",
        "rel_types",
        "blocks",
        "meta_members",
        "node_versions",
        "views",
        "hygiene_suggestions",
        "instance_meta",
        "embeddings",
    ];
    // Verify all Phase B relations exist by doing a count query on each.
    // Each relation has a different key column, so use :columns introspection.
    for rel in relations {
        let query = format!("::columns {rel}");
        let result = store.run_immut(&query);
        assert!(result.is_ok(), "relation {rel} should exist: {result:?}");
    }
}

#[test]
fn instance_id_generated_on_open() {
    let (_tmp, store) = make_store();
    let id = store.instance_id().unwrap();
    assert!(!id.is_empty());
    assert!(id.contains('-'), "should be UUID format: {id}");
    // Idempotent — second call returns same ID
    let id2 = store.instance_id().unwrap();
    assert_eq!(id, id2);
}

#[test]
fn seed_type_system_populates_metadata() {
    let (_tmp, store) = make_store();
    store.seed_type_system().unwrap();

    // Check node_types
    let (headers, rows) = store
        .raw_query("?[kind, label] := *node_types{kind, label}")
        .unwrap();
    assert!(headers.contains(&"kind".to_string()));
    assert!(
        rows.len() >= 14,
        "should have at least 14 node types, got {}",
        rows.len()
    );

    // Check rel_types
    let (_, rel_rows) = store
        .raw_query("?[name, inverse] := *rel_types{name, inverse_name: inverse}")
        .unwrap();
    assert!(
        rel_rows.len() >= 20,
        "should have at least 20 rel types, got {}",
        rel_rows.len()
    );

    // Idempotent — re-seeding doesn't duplicate
    store.seed_type_system().unwrap();
    let (_, rows2) = store.raw_query("?[kind] := *node_types{kind}").unwrap();
    assert_eq!(rows.len(), rows2.len());
}

#[test]
fn seed_views_creates_view_nodes() {
    let (_tmp, store) = make_store();
    store.seed_views().unwrap();

    // Views should be in the views relation
    let result = store
        .run_immut("?[id, title, kind] := *views{id, title, kind}")
        .unwrap();
    assert!(
        result.rows.len() >= 6,
        "should have at least 6 seeded views, got {}",
        result.rows.len()
    );

    // View nodes should also exist as regular KB nodes
    let kanban = store.get_node("view:kanban").unwrap();
    assert!(kanban.is_some(), "kanban view should exist as a node");
    assert_eq!(kanban.unwrap().title, "Kanban Board");

    // Idempotent: seeding again should not error
    store.seed_views().unwrap();
}
