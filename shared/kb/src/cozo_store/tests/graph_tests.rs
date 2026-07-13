use super::*;

#[test]
fn shortest_path_finds_a_direct_link() {
    // Regression test for a bug where EVERY call to `shortest_path` errored:
    // the original recursive-Datalog query used `d + 1` in a rule head,
    // which CozoDB's parser rejects outright ("query parser has encountered
    // unexpected input"). Now implemented as iterative BFS (mirroring
    // `neighborhood` below), so this must actually succeed.
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
        .unwrap();
    store.add_link("a", "b", None).unwrap();

    let path = store.shortest_path("a", "b").unwrap();
    assert_eq!(path, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn shortest_path_walks_multiple_hops_within_the_depth_cap() {
    let (_tmp, store) = make_store();
    for id in ["a", "b", "c", "d"] {
        store
            .insert_node(&Node::new(id, id, NodeKind::Note, ""))
            .unwrap();
    }
    store.add_link("a", "b", None).unwrap();
    store.add_link("b", "c", None).unwrap();
    store.add_link("c", "d", None).unwrap();

    let path = store.shortest_path("a", "d").unwrap();
    assert_eq!(path, vec!["a".to_string(), "d".to_string()]);
}

#[test]
fn shortest_path_reaches_via_incoming_links_too() {
    // The walk is undirected (matches the original two-rule query, which
    // recursed through both `links{src: mid, dst: node}` and
    // `links{src: node, dst: mid}`).
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
        .unwrap();
    // Link points b -> a (incoming to a), not a -> b.
    store.add_link("b", "a", None).unwrap();

    let path = store.shortest_path("a", "b").unwrap();
    assert_eq!(path, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn shortest_path_empty_for_disconnected_nodes() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
        .unwrap();
    // No link between them.
    let path = store.shortest_path("a", "b").unwrap();
    assert!(path.is_empty());
}

#[test]
fn shortest_path_from_a_node_to_itself() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
        .unwrap();
    let path = store.shortest_path("a", "a").unwrap();
    assert_eq!(path, vec!["a".to_string(), "a".to_string()]);
}

#[test]
fn neighborhood_query() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("center", "Center", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("near1", "Near 1", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("near2", "Near 2", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("far1", "Far 1", NodeKind::Note, ""))
        .unwrap();

    store.add_link("center", "near1", None).unwrap();
    store.add_link("center", "near2", None).unwrap();
    store.add_link("near1", "far1", None).unwrap();

    // Depth 1: center + near1 + near2
    let sg = store.neighborhood("center", 1).unwrap();
    assert!(sg.nodes.len() >= 3);

    // Depth 2: should include far1 too
    let sg2 = store.neighborhood("center", 2).unwrap();
    assert!(sg2.nodes.len() >= 4);
}

#[test]
fn related_matches_graph_and_tag_signals() {
    let (_tmp, store) = make_store();
    let mut seed = Node::new("seed", "Seed", NodeKind::Note, "");
    seed.tags = vec!["topic".into()];
    let mut tagmate = Node::new("tagmate", "Tagmate", NodeKind::Note, "");
    tagmate.tags = vec!["topic".into()];
    for n in [
        &seed,
        &Node::new("coupled", "Coupled", NodeKind::Note, ""),
        &Node::new("hub", "Hub", NodeKind::Note, ""),
        &Node::new("direct", "Direct", NodeKind::Note, ""),
        &tagmate,
        &Node::new("unrelated", "Unrelated", NodeKind::Note, ""),
    ] {
        store.insert_node(n).unwrap();
    }
    // seed -> hub ; coupled -> hub (coupling) ; direct -> seed (adjacency).
    store.add_link("seed", "hub", None).unwrap();
    store.add_link("coupled", "hub", None).unwrap();
    store.add_link("direct", "seed", None).unwrap();

    let related = store.related("seed", 10).unwrap();
    let score = |id: &str| related.iter().find(|(i, _)| i == id).map(|(_, s)| *s);

    // Same ordering guarantees as the in-memory KnowledgeBase::related.
    assert!(score("hub").unwrap() > score("coupled").unwrap());
    assert!(score("direct").unwrap() > score("coupled").unwrap());
    assert!(score("coupled").unwrap() > score("tagmate").unwrap());
    assert!(score("tagmate").is_some(), "tag-only relatedness surfaces");
    assert!(score("unrelated").is_none());
    assert!(score("seed").is_none());
}
