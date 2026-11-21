use super::*;

#[test]
fn links_from_and_to() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "a",
            "A",
            NodeKind::Note,
            "See [[b]] for details.",
        ))
        .unwrap();
    store
        .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
        .unwrap();

    let from_a = store.links_from("a").unwrap();
    assert_eq!(from_a.len(), 1);
    assert_eq!(from_a[0].dst, "b");

    let to_b = store.links_to("b").unwrap();
    assert_eq!(to_b.len(), 1);
    assert_eq!(to_b[0].src, "a");
}

#[test]
fn typed_links() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("impl:1", "Implementation", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("spec:1", "Specification", NodeKind::Concept, ""))
        .unwrap();

    store
        .add_typed_link("impl:1", "spec:1", "implements", 1.0)
        .unwrap();
    store
        .add_typed_link("impl:1", "spec:1", "references", 0.5)
        .unwrap();

    let impl_links = store.links_typed("impl:1", "implements").unwrap();
    assert_eq!(impl_links.len(), 1);
    assert_eq!(impl_links[0].rel_type, "implements");

    let ref_links = store.links_typed("impl:1", "references").unwrap();
    assert_eq!(ref_links.len(), 1);
}

#[test]
fn insert_node_projects_adr030_typed_link_grammar_from_body() {
    // Regression for the real bug this fix closes: update_links_for_node (the
    // single-user insert_node/update_node path, NOT the daemon projector) used to
    // call the untyped parse_links, which doesn't strip a link's `?query` string and
    // hardcoded every edge to rel_type="related_to" -- so an ADR-030-grammar typed
    // link written into a node's body via ordinary single-user kb_update produced a
    // dangling edge to a literal "target?rel=...&w=..." id, always typed
    // "related_to", regardless of what was actually authored.
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new(
            "concept:buffer",
            "Buffer",
            NodeKind::Concept,
            "",
        ))
        .unwrap();
    store
        .insert_node(&Node::new(
            "note:1",
            "Note",
            NodeKind::Note,
            "See [[concept:buffer?rel=teaches&w=0.8][the buffer]] for details.",
        ))
        .unwrap();

    let links = store.links_from("note:1").unwrap();
    assert_eq!(links.len(), 1);
    // The query string must be stripped from the target -- not a dangling
    // "concept:buffer?rel=teaches&w=0.8" id.
    assert_eq!(links[0].dst, "concept:buffer");
    assert_eq!(
        links[0].rel_type, "teaches",
        "authored rel_type must survive, not be forced to related_to"
    );
    assert_eq!(links[0].weight, 0.8);

    // Also queryable via the typed-link API, proving it's the same projection
    // the daemon's project_node uses.
    let typed = store.links_typed("note:1", "teaches").unwrap();
    assert_eq!(typed.len(), 1);
    assert_eq!(typed[0].dst, "concept:buffer");
}

#[test]
fn link_confidence_round_trips() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("a", "A", NodeKind::Note, ""))
        .unwrap();
    store
        .insert_node(&Node::new("b", "B", NodeKind::Note, ""))
        .unwrap();

    store
        .add_typed_link_with_confidence("a", "b", "implements", 0.8, 0.6)
        .unwrap();

    let links = store.links_from("a").unwrap();
    assert_eq!(links.len(), 1);
    assert!((links[0].weight - 0.8).abs() < 0.01);
    assert!((links[0].confidence - 0.6).abs() < 0.01);
    assert_eq!(links[0].rel_type, "implements");
}

#[test]
fn seed_typed_relationships_creates_links() {
    let (_tmp, store) = make_store();
    let count = store.seed_typed_relationships().unwrap();
    // Only 6 code-generated relationships remain (index categorizes).
    // Content relationships are now inline typed links in org files.
    assert_eq!(count, 6, "should seed exactly 6 relationships, got {count}");

    // Verify index categorizes concept:buffer
    let links = store.links_typed("index", "categorizes").unwrap();
    assert!(
        links.iter().any(|l| l.dst == "concept:buffer"),
        "index should categorize concept:buffer"
    );

    // Verify idempotency
    let count2 = store.seed_typed_relationships().unwrap();
    assert_eq!(count, count2);
    // Count should not double
    let all_links = store
        .run_immut("?[src, dst, rt] := *links{src, dst, rel_type: rt}, rt != 'related_to'")
        .unwrap();
    assert_eq!(
        all_links.rows.len(),
        count,
        "idempotent: link count should match"
    );
}

/// The stored edges of one node with `body` (#941 PR2: `mae-org` derives them).
fn stored_edges(body: &str) -> Vec<(String, String, f64, f64)> {
    let store = CozoKbStore::open_mem().unwrap();
    store
        .insert_node(&Node::new("src", "S", NodeKind::Note, body))
        .unwrap();
    let mut v: Vec<_> = store
        .links_from("src")
        .unwrap()
        .into_iter()
        .map(|l| (l.dst, l.rel_type, l.weight, l.confidence))
        .collect();
    v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    v
}

#[test]
fn a_typed_link_is_stored_with_its_relation_weight_and_confidence() {
    // #854 reaching stored data: the relation, weight and confidence written
    // in the text are the stored edge's, in raw org and in the store form.
    for body in [
        "[[id:b?rel=answers&w=0.5&c=0.7][b]]",
        "[[b?rel=answers&w=0.5&c=0.7|b]]",
    ] {
        assert_eq!(
            stored_edges(body),
            vec![("b".to_string(), "answers".to_string(), 0.5, 0.7)],
            "{body}"
        );
    }
}

#[test]
fn a_stored_weight_is_clamped_to_the_unit_interval() {
    assert_eq!(
        stored_edges("[[b?w=5&c=-2]]"),
        vec![("b".to_string(), "references".to_string(), 1.0, 0.0)]
    );
}

#[test]
fn one_edge_per_target_and_relation_the_last_in_body_order() {
    assert_eq!(
        stored_edges("[[b?rel=r&w=0.2]] [[b?rel=r&w=0.6]] [[b?rel=s&w=0.3]]"),
        vec![
            ("b".to_string(), "r".to_string(), 0.6, 1.0),
            ("b".to_string(), "s".to_string(), 0.3, 1.0),
        ]
    );
}

#[test]
fn links_that_name_no_node_are_not_stored_edges() {
    let body = "[[https://e.org][web]] [[mailto:x@e.org]] [[file:a.org][f]] \
                [[help:x]] [[#custom]] [[*Heading]] [[(ref)]] [[kept]]\n\
                # [[in-comment]]\n\
                #+begin_src sh\n[[in-src]]\n#+end_src\n";
    assert_eq!(
        stored_edges(body),
        vec![("kept".to_string(), "references".to_string(), 1.0, 1.0)]
    );
}

#[test]
fn bare_and_namespaced_ids_stay_edges() {
    // An include-list of KB namespaces would drop an org-roam bare id: the
    // edge filter is "names a node", not "looks like MAE's".
    let ids = [
        "1F0A-BEEF-4C21-9A33",
        "20260824T101500",
        "concept:buffer",
        "my-note-without-a-namespace",
        "project:some/path",
        "user:20260824-my-note",
    ];
    let body: String = ids.iter().map(|i| format!("[[{i}]] ")).collect();
    let got: Vec<String> = stored_edges(&body).into_iter().map(|e| e.0).collect();
    assert_eq!(got, ids.map(String::from).to_vec());
}
