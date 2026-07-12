use super::*;

#[test]
fn id_title_pairs_basic() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&Node::new("concept:a", "Alpha", NodeKind::Concept, ""))
        .unwrap();
    store
        .insert_node(&Node::new("lesson:b", "Beta", NodeKind::Lesson, ""))
        .unwrap();

    let all = store.id_title_pairs(None).unwrap();
    assert_eq!(all.len(), 2);

    let concepts = store.id_title_pairs(Some("concept:")).unwrap();
    assert_eq!(concepts.len(), 1);
    assert_eq!(concepts[0].0, "concept:a");
    assert_eq!(concepts[0].1, "Alpha");
}
