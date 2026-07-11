use super::*;

#[test]
fn meta_node_composition() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "meta:release",
            "Release Notes",
            NodeKind::Meta,
            "",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "feat:1",
            "Feature 1",
            NodeKind::Note,
            "Added widgets.",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "feat:2",
            "Feature 2",
            NodeKind::Note,
            "Fixed bugs.",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "ref:1",
            "Reference",
            NodeKind::Note,
            "See docs.",
        ))
        .unwrap();

    store
        .add_meta_member("meta:release", "feat:1", 0, "content")
        .unwrap();
    store
        .add_meta_member("meta:release", "feat:2", 1, "content")
        .unwrap();
    store
        .add_meta_member("meta:release", "ref:1", 2, "reference")
        .unwrap();

    let members = store.meta_members("meta:release").unwrap();
    assert_eq!(members.len(), 3);
    assert_eq!(members[0].member_id, "feat:1");
    assert_eq!(members[1].member_id, "feat:2");
    assert_eq!(members[2].role, "reference");

    let body = store.compose_meta_body("meta:release").unwrap();
    assert!(body.contains("Added widgets."));
    assert!(body.contains("Fixed bugs."));
    assert!(body.contains("→ [[ref:1]]"));

    // Remove member
    store.remove_meta_member("meta:release", "feat:2").unwrap();
    assert_eq!(store.meta_members("meta:release").unwrap().len(), 2);
}

#[test]
fn block_level_addressing() {
    let (_tmp, store) = make_store();
    store.insert_node(&Node::new(
            "concept:test",
            "Test Concept",
            NodeKind::Concept,
            "First paragraph here.\n\nSecond paragraph about buffers.\n\n- A list item\n- Another item",
        )).unwrap();

    let count = store.split_into_blocks("concept:test").unwrap();
    assert_eq!(count, 3);

    let blocks = store.get_blocks("concept:test").unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(blocks[0].block_type, "paragraph");
    assert_eq!(blocks[2].block_type, "list");

    // Single block access
    let block = store.get_block("concept:test", 1).unwrap().unwrap();
    assert!(block.content.contains("buffers"));
}
