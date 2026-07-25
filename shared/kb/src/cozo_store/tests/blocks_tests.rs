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
fn transclusion_authored_via_kb_create_update_stays_in_sync_on_every_write() {
    // ADR-065 item 4: `#+TRANSCLUDE:` directives previously only got parsed
    // into `meta_members` at file-import time — a node written directly via
    // `kb_create`/`kb_update` (both funnel through `CozoKbStore::insert_node`,
    // exercised here the same way) never re-derived them, unlike the sibling
    // typed-link directive path. This is the adversarial test the ADR names
    // verbatim: author via the MCP write path (`insert_node`), edit the
    // transcluded MEMBER via a *second, separate* write, re-read — the
    // composed body must reflect the edit, not just the creation-time state.
    let (_tmp, store) = make_store();

    // The member exists first (order doesn't matter — compose_meta_body reads
    // it live at call time regardless of insertion order).
    store
        .insert_node(&Node::new(
            "feat:transcluded",
            "Feature",
            NodeKind::Note,
            "Original feature body.",
        ))
        .unwrap();

    // Author the meta node via the same path `kb_create` uses — no explicit
    // `add_meta_member` call, only a `#+TRANSCLUDE:` directive in the body.
    store
        .insert_node(&Node::new(
            "meta:authored",
            "Authored Meta",
            NodeKind::Meta,
            "#+TRANSCLUDE: feat:transcluded content\n#+TRANSCLUDE: feat:transcluded reference",
        ))
        .unwrap();

    let members = store.meta_members("meta:authored").unwrap();
    assert_eq!(
        members.len(),
        2,
        "both TRANSCLUDE directives in the authored body must be reflected \
         in meta_members without any explicit add_meta_member call"
    );
    let body = store.compose_meta_body("meta:authored").unwrap();
    assert!(
        body.contains("Original feature body."),
        "compose_meta_body must include the transcluded member's content \
         immediately after MCP-style authoring, got: {body:?}"
    );

    // Edit the transcluded MEMBER via a second, separate write — the actual
    // adversarial case the ADR names: does the composed body reflect the
    // edit, not just the creation-time snapshot?
    store
        .insert_node(&Node::new(
            "feat:transcluded",
            "Feature",
            NodeKind::Note,
            "Edited feature body.",
        ))
        .unwrap();
    let body_after_edit = store.compose_meta_body("meta:authored").unwrap();
    assert!(
        body_after_edit.contains("Edited feature body."),
        "compose_meta_body must reflect the member's edit, got: {body_after_edit:?}"
    );
    assert!(!body_after_edit.contains("Original feature body."));

    // Removing the directive from the meta node's own body (a second write to
    // the meta node itself) must clear its meta_members — full "re-derive
    // from source on every write" symmetry with the typed-link directive.
    store
        .insert_node(&Node::new(
            "meta:authored",
            "Authored Meta",
            NodeKind::Meta,
            "No transclusion directives anymore.",
        ))
        .unwrap();
    assert!(
        store.meta_members("meta:authored").unwrap().is_empty(),
        "removing the TRANSCLUDE directive from the body must clear \
         meta_members on the next write, not leave stale membership behind"
    );
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
