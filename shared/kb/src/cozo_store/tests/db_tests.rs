use super::*;
use crate::NodeSource;

#[test]
fn node_source_round_trips_through_the_store_for_every_variant() {
    // Locks in the two exhaustive NodeSource<->str match arms (db.rs,
    // util.rs) added for NodeSource::Promoted (#303) -- every variant,
    // including the new one, must persist and reload exactly.
    let (_tmp, store) = make_store();
    let variants = [
        NodeSource::Seed,
        NodeSource::UserOrg,
        NodeSource::Manual,
        NodeSource::Federation,
        NodeSource::Promoted,
    ];
    for (i, source) in variants.iter().enumerate() {
        let id = format!("user:round-trip-{i}");
        let node = Node::new(&id, "T", NodeKind::Note, "b").with_source(*source, 0);
        store.insert_node(&node).unwrap();
        let reloaded = store.get_node(&id).unwrap().expect("node must reload");
        assert_eq!(
            reloaded.source,
            Some(*source),
            "NodeSource::{source:?} must round-trip exactly"
        );
    }
}

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
