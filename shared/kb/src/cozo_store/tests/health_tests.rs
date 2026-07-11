use super::*;

#[test]
fn health_report_counts() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, "See [[b]]"))
        .unwrap();
    store
        .insert_node(&Node::new("b", "B", NodeKind::Concept, ""))
        .unwrap();
    store
        .insert_node(&Node::new("c", "C", NodeKind::Note, ""))
        .unwrap();

    let report = store.health_report().unwrap();
    assert_eq!(report.total_nodes, 3);
    assert!(report.total_links >= 1);
    assert_eq!(report.orphan_ids.len(), 1); // "c" has no links
    assert!(report.by_kind.get("note").copied().unwrap_or(0) >= 2);
    assert!(report.by_kind.get("concept").copied().unwrap_or(0) >= 1);
}

#[test]
fn health_report_typed_links_not_orphans() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "lesson:nav",
            "Navigation",
            NodeKind::Lesson,
            "body",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer",
            NodeKind::Concept,
            "body",
        ))
        .unwrap();
    // Add a typed link — lesson teaches concept
    store
        .add_typed_link("lesson:nav", "concept:buffer", "teaches", 1.0)
        .unwrap();

    let report = store.health_report().unwrap();
    assert_eq!(report.total_nodes, 2);
    assert!(report.total_links >= 1);
    // Neither should be orphan since they have a typed link between them
    assert!(
        report.orphan_ids.is_empty(),
        "nodes with typed links should not be orphans: {:?}",
        report.orphan_ids
    );
    // Verify namespace counts
    assert_eq!(
        report.namespace_counts.get("lesson").copied().unwrap_or(0),
        1
    );
    assert_eq!(
        report.namespace_counts.get("concept").copied().unwrap_or(0),
        1
    );
    // Verify rel_type counts
    assert_eq!(report.by_rel_type.get("teaches").copied().unwrap_or(0), 1);
}

#[test]
fn health_report_broken_links_with_details() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
        .unwrap();
    // Add a link to a non-existent node
    store
        .add_typed_link("a", "concept:missing", "references", 1.0)
        .unwrap();

    let report = store.health_report().unwrap();
    assert_eq!(report.broken_links.len(), 1);
    assert_eq!(report.broken_links[0].source, "a");
    assert_eq!(report.broken_links[0].target, "concept:missing");
    assert_eq!(report.broken_links[0].rel_type, "references");
}

#[test]
fn health_report_hub_nodes_ranked() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("hub", "Hub", NodeKind::Concept, ""))
        .unwrap();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("c", "C", NodeKind::Note, ""))
        .unwrap();
    // All nodes link to "hub"
    store.add_typed_link("a", "hub", "references", 1.0).unwrap();
    store.add_typed_link("b", "hub", "references", 1.0).unwrap();
    store.add_typed_link("c", "hub", "references", 1.0).unwrap();

    let report = store.health_report().unwrap();
    assert!(!report.hub_nodes.is_empty());
    assert_eq!(report.hub_nodes[0].0, "hub");
    assert_eq!(report.hub_nodes[0].1, 3);
}
