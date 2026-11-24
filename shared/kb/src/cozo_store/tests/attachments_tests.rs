//! ADR-115 D4: the `node_attachments` relation is the node's map, through
//! insert, replace, delete, a load-edit-store cycle and a bulk import; and the
//! CRDT bridge carries the map without letting a `Node` that lost it delete
//! it from a shared document.

use super::*;
use crate::NodeAttachment;

fn att(seed: u8) -> NodeAttachment {
    NodeAttachment {
        sha256: format!("{seed:02x}").repeat(32),
        size: 100 + seed as u64,
        media_type: "image/png".into(),
    }
}

fn node_with(id: &str, entries: &[(&str, u8)]) -> Node {
    let mut n = Node::new(id, "Title", NodeKind::Note, "[[attachment:a.png]]");
    n.attachments = entries
        .iter()
        .map(|(name, seed)| (name.to_string(), att(*seed)))
        .collect();
    n
}

fn refs(entries: &[(&str, &str, u8)]) -> Vec<(String, String, String)> {
    let mut v: Vec<_> = entries
        .iter()
        .map(|(n, name, seed)| (n.to_string(), name.to_string(), att(*seed).sha256))
        .collect();
    v.sort();
    v
}

#[test]
fn the_relation_follows_the_node_through_insert_replace_and_delete() {
    let (_tmp, store) = make_store();
    store
        .insert_node(&node_with("other", &[("o.png", 9)]))
        .unwrap();

    store
        .insert_node(&node_with("n", &[("a.png", 1), ("b.png", 2)]))
        .unwrap();
    assert_eq!(
        store.attachment_refs().unwrap(),
        refs(&[("n", "a.png", 1), ("n", "b.png", 2), ("other", "o.png", 9)])
    );

    // Replace one image, drop the other: the rows are the new map exactly.
    store.insert_node(&node_with("n", &[("a.png", 3)])).unwrap();
    assert_eq!(
        store.attachment_refs().unwrap(),
        refs(&[("n", "a.png", 3), ("other", "o.png", 9)])
    );
    let loaded = store.get_node("n").unwrap().unwrap();
    assert_eq!(
        loaded.attachments,
        node_with("n", &[("a.png", 3)]).attachments
    );

    // An emptied map leaves no row.
    store.insert_node(&node_with("n", &[])).unwrap();
    assert_eq!(
        store.attachment_refs().unwrap(),
        refs(&[("other", "o.png", 9)])
    );

    store.insert_node(&node_with("n", &[("a.png", 4)])).unwrap();
    store.delete_node("n").unwrap();
    assert_eq!(
        store.attachment_refs().unwrap(),
        refs(&[("other", "o.png", 9)]),
        "a deleted node's rows go with it, and only its rows"
    );
}

/// A node read back carries its map, so storing it again after an edit to
/// something else keeps the rows: through `get_node` and through `load_all`.
#[test]
fn a_load_edit_store_cycle_keeps_the_map() {
    let (_tmp, store) = make_store();
    let original = node_with("n", &[("a.png", 1), ("ñ b.svg", 2)]);
    store.insert_node(&original).unwrap();
    let before = store.attachment_refs().unwrap();

    let mut n = store.get_node("n").unwrap().unwrap();
    n.title = "Edited".into();
    store.insert_node(&n).unwrap();
    assert_eq!(store.attachment_refs().unwrap(), before);

    let mut all = store.load_all().unwrap();
    assert_eq!(all[0].attachments, original.attachments);
    all[0].body = "edited body".into();
    store.insert_node(&all[0]).unwrap();
    assert_eq!(store.attachment_refs().unwrap(), before);
    assert_eq!(
        store.get_node_light("n").unwrap().unwrap().attachments,
        original.attachments
    );
}

#[test]
fn a_bulk_import_carries_the_map() {
    let (_tmp, store) = make_store();
    let nodes = [
        node_with("x", &[("a.png", 1)]),
        node_with("y", &[]),
        node_with("z", &[("b.png", 2), ("c.png", 3)]),
    ];
    store.bulk_import(&nodes, &[]).unwrap();
    assert_eq!(
        store.attachment_refs().unwrap(),
        refs(&[("x", "a.png", 1), ("z", "b.png", 2), ("z", "c.png", 3)])
    );
}

#[cfg(feature = "crdt")]
mod bridge {
    use std::collections::BTreeMap;

    use super::*;
    use crate::KnowledgeBase;
    use mae_sync::kb::{AttachmentRef, KbNodeDoc};

    fn sync_ref(seed: u8) -> AttachmentRef {
        let a = att(seed);
        AttachmentRef::new(a.sha256, a.size, a.media_type).unwrap()
    }

    fn expected(entries: &[(&str, u8)]) -> BTreeMap<String, NodeAttachment> {
        entries
            .iter()
            .map(|(n, s)| (n.to_string(), att(*s)))
            .collect()
    }

    /// An org re-ingest produces a `Node` with no map (the text cannot carry
    /// one). Upserting it onto a shared node must not delete the document's map.
    #[test]
    fn an_upsert_of_a_node_without_its_map_does_not_delete_it_from_the_document() {
        let mut doc = KbNodeDoc::new_with_client_id("n", "Title", "body", &[], 7);
        let _ = doc.set_attachment("a.png", &sync_ref(1)).unwrap();
        let mut kb = KnowledgeBase::new();
        kb.adopt_remote_node("n", &doc.encode()).unwrap();
        assert_eq!(kb.get("n").unwrap().attachments, expected(&[("a.png", 1)]));

        let mut reingested = kb.get("n").unwrap().clone();
        reingested.attachments.clear();
        reingested.body = "edited body".into();
        let state = kb.upsert_with_crdt(reingested, 7).unwrap();
        assert_eq!(kb.get("n").unwrap().attachments, expected(&[("a.png", 1)]));
        let back = KbNodeDoc::from_bytes(&state).unwrap();
        assert_eq!(back.attachment("a.png"), Some(sync_ref(1)));
        assert_eq!(back.body(), "edited body");
    }

    #[test]
    fn an_attachments_only_remote_update_reaches_the_node() {
        let mut owner = KbNodeDoc::new_with_client_id("n", "Title", "body", &[], 7);
        let mut kb = KnowledgeBase::new();
        kb.adopt_remote_node("n", &owner.encode()).unwrap();
        let update = owner.set_attachment("a.png", &sync_ref(1)).unwrap();
        assert!(kb.apply_remote_update("n", &update).unwrap());
        assert_eq!(kb.get("n").unwrap().attachments, expected(&[("a.png", 1)]));
        let update = owner.remove_attachment("a.png");
        assert!(kb.apply_remote_update("n", &update).unwrap());
        assert!(kb.get("n").unwrap().attachments.is_empty());
    }

    /// Minting a fresh lineage from a node that has a map (no CRDT bytes yet)
    /// carries it: construction, not a write to a shared document.
    #[test]
    fn a_fresh_lineage_carries_the_map() {
        let n = node_with("n", &[("a.png", 1)]);
        let doc = n.to_crdt_doc().unwrap();
        assert_eq!(doc.attachment("a.png"), Some(sync_ref(1)));
        let back = Node::from_crdt_doc(&doc, NodeKind::Note, crate::NodeSource::Federation);
        assert_eq!(back.attachments, n.attachments);
    }

    /// A node whose map holds two NAMEs that fold to one key (two peers bound
    /// them concurrently; every writer refuses the second) mints a lineage
    /// with both: the copy is not a new binding, and dropping one would delete
    /// an image.
    #[test]
    fn a_fresh_lineage_carries_a_concurrent_collision_pair() {
        let n = node_with("n", &[("Photo.png", 1), ("photo.png", 2)]);
        let doc = n.to_crdt_doc().unwrap();
        assert_eq!(doc.attachment("Photo.png"), Some(sync_ref(1)));
        assert_eq!(doc.attachment("photo.png"), Some(sync_ref(2)));
    }
}
