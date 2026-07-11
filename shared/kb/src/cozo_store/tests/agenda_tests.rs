use super::*;

#[test]
fn agenda_orphan_query() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "linked:1",
            "Linked",
            NodeKind::Note,
            "See [[linked:2]]",
        ))
        .unwrap();
    store
        .insert_node(&Node::new("linked:2", "Also Linked", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new(
            "orphan:1",
            "Orphan",
            NodeKind::Note,
            "No links here",
        ))
        .unwrap();

    let orphans = store.agenda_query(&AgendaFilter::Orphan).unwrap();
    let orphan_ids: Vec<&str> = orphans.iter().map(|n| n.id.as_str()).collect();
    assert!(
        orphan_ids.contains(&"orphan:1"),
        "orphan:1 should be found: {orphan_ids:?}"
    );
    assert!(
        !orphan_ids.contains(&"linked:1"),
        "linked:1 should not be orphan"
    );
}

#[test]
fn agenda_todo_filter() {
    let (_tmp, store) = make_store();
    let mut todo = Node::new("task:1", "Fix Bug", NodeKind::Task, "");
    todo.todo_state = Some("TODO".to_string());
    store.insert_node(&todo).unwrap();

    let mut done = Node::new("task:2", "Done Task", NodeKind::Task, "");
    done.todo_state = Some("DONE".to_string());
    store.insert_node(&done).unwrap();

    store
        .insert_node(&Node::new("note:1", "Regular", NodeKind::Note, ""))
        .unwrap();

    // All todos
    let all_todos = store.agenda_query(&AgendaFilter::Todo(None)).unwrap();
    assert_eq!(all_todos.len(), 2);

    // Only TODO state
    let just_todo = store
        .agenda_query(&AgendaFilter::Todo(Some("TODO".into())))
        .unwrap();
    assert_eq!(just_todo.len(), 1);
    assert_eq!(just_todo[0].id, "task:1");
}

#[test]
fn agenda_missing_role_filter() {
    let (_tmp, store) = make_store();

    let mut has_role = Node::new("note:has-role", "Has Role", NodeKind::Note, "");
    has_role
        .properties
        .insert("role".to_string(), "atom".to_string());
    store.insert_node(&has_role).unwrap();

    let mut other_role = Node::new("note:other-role", "Other Role", NodeKind::Note, "");
    other_role
        .properties
        .insert("role".to_string(), "hub".to_string());
    store.insert_node(&other_role).unwrap();

    store
        .insert_node(&Node::new(
            "note:no-role",
            "No Role",
            NodeKind::Note,
            "Unclassified",
        ))
        .unwrap();

    // A node with an unrelated property (but no role) must still count as missing.
    let mut other_prop = Node::new("note:other-prop", "Other Prop", NodeKind::Note, "");
    other_prop
        .properties
        .insert("assignee".to_string(), "alice".to_string());
    store.insert_node(&other_prop).unwrap();

    let missing = store.agenda_query(&AgendaFilter::MissingRole).unwrap();
    let missing_ids: std::collections::HashSet<&str> =
        missing.iter().map(|n| n.id.as_str()).collect();

    assert!(
        missing_ids.contains("note:no-role"),
        "note:no-role should be missing a role: {missing_ids:?}"
    );
    assert!(
        missing_ids.contains("note:other-prop"),
        "note:other-prop should be missing a role: {missing_ids:?}"
    );
    assert!(
        !missing_ids.contains("note:has-role"),
        "note:has-role has a role set, should be excluded"
    );
    assert!(
        !missing_ids.contains("note:other-role"),
        "note:other-role has a role set, should be excluded"
    );
}

#[test]
fn agenda_weakly_linked_filter() {
    let (_tmp, store) = make_store();

    // Zero outgoing links.
    store
        .insert_node(&Node::new("note:zero", "Zero Links", NodeKind::Note, ""))
        .unwrap();
    // One outgoing link.
    store
        .insert_node(&Node::new("note:one", "One Link", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new(
            "note:one-target",
            "One Target",
            NodeKind::Note,
            "",
        ))
        .unwrap();
    store
        .add_typed_link("note:one", "note:one-target", "related_to", 1.0)
        .unwrap();
    // Two outgoing links — well-linked, should not appear for threshold 2.
    store
        .insert_node(&Node::new("note:two", "Two Links", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new(
            "note:two-target-a",
            "Target A",
            NodeKind::Note,
            "",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "note:two-target-b",
            "Target B",
            NodeKind::Note,
            "",
        ))
        .unwrap();
    store
        .add_typed_link("note:two", "note:two-target-a", "related_to", 1.0)
        .unwrap();
    store
        .add_typed_link("note:two", "note:two-target-b", "related_to", 1.0)
        .unwrap();

    // Threshold 2: nodes with fewer than 2 outgoing links (0 or 1).
    let weak = store.agenda_query(&AgendaFilter::WeaklyLinked(2)).unwrap();
    let weak_ids: std::collections::HashSet<&str> = weak.iter().map(|n| n.id.as_str()).collect();

    assert!(
        weak_ids.contains("note:zero"),
        "note:zero has 0 outgoing links: {weak_ids:?}"
    );
    assert!(
        weak_ids.contains("note:one"),
        "note:one has 1 outgoing link: {weak_ids:?}"
    );
    assert!(
        !weak_ids.contains("note:two"),
        "note:two has 2 outgoing links, should not be weakly linked at threshold 2"
    );

    // Threshold 0: nothing can have fewer than 0 links.
    let none_weak = store.agenda_query(&AgendaFilter::WeaklyLinked(0)).unwrap();
    assert!(
        none_weak.is_empty(),
        "no node should have fewer than 0 outgoing links: {:?}",
        none_weak.iter().map(|n| &n.id).collect::<Vec<_>>()
    );

    // Threshold 1 == DeadEnd's "no outgoing links" semantics.
    let dead_end_equiv = store.agenda_query(&AgendaFilter::WeaklyLinked(1)).unwrap();
    let dead_end = store.agenda_query(&AgendaFilter::DeadEnd).unwrap();
    let mut equiv_ids: Vec<&str> = dead_end_equiv.iter().map(|n| n.id.as_str()).collect();
    let mut dead_end_ids: Vec<&str> = dead_end.iter().map(|n| n.id.as_str()).collect();
    equiv_ids.sort_unstable();
    dead_end_ids.sort_unstable();
    assert_eq!(
        equiv_ids, dead_end_ids,
        "WeaklyLinked(1) should match DeadEnd's node set exactly"
    );
}
