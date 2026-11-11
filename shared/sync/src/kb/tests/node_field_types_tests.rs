//! Field types on write: an update that leaves a known field mistyped is
//! named; a well-typed one, an unknown key, and a legacy document's existing
//! mistype are not. Each mistyped update is a real yrs update made by a second
//! client from the node's own state, as a careless or hostile writer would.

use super::*;
use yrs::{
    updates::decoder::Decode, ArrayPrelim, Doc, Map, MapPrelim, ReadTxn, StateVector, Transact,
    Update,
};

/// A seeded-shape node: every field set the way MAE writes it.
fn node() -> KbNodeDoc {
    let mut n = KbNodeDoc::new_created_at("n", "Title", "body\n", &["a".into()], 1_700_000_000);
    let _ = n.set_kind(Some("Concept"));
    let _ = n.set_todo_state(Some("TODO"));
    let _ = n.set_aliases(&["alias".into()]);
    let mut props = std::collections::HashMap::new();
    props.insert("ID".to_string(), "n".to_string());
    let _ = n.set_properties(&props);
    let _ = n.set_org_header(&OrgHeader {
        property_order: vec!["ID".into()],
        has_title_line: true,
        has_filetags_line: false,
    });
    n
}

/// An update made by another client on top of `base`, doing `edit` to the
/// node map.
fn foreign(base: &[u8], edit: impl FnOnce(&yrs::MapRef, &mut yrs::TransactionMut)) -> Vec<u8> {
    let doc = Doc::with_client_id(4242);
    doc.transact_mut()
        .apply_update(Update::decode_v1(base).unwrap())
        .unwrap();
    let sv = doc.transact().state_vector();
    let root = doc.get_or_insert_map("node");
    {
        let mut txn = doc.transact_mut();
        edit(&root, &mut txn);
    }
    let out = doc.transact().encode_state_as_update_v1(&sv);
    out
}

#[test]
fn a_well_typed_node_has_no_violation() {
    assert!(node().field_type_violations().is_empty());
    // And a real edit through the API introduces none.
    let base = node().encode_state();
    let mut edited = KbNodeDoc::from_bytes(&base).unwrap();
    let up = edited.set_kind(Some("Note"));
    assert_eq!(KbNodeDoc::field_type_introduced(&base, &up).unwrap(), None);
}

/// Every known field, set to a wrong type, is named. Kills a check that misses
/// any one of them.
#[test]
fn every_known_field_set_to_a_wrong_type_is_named() {
    let base = node().encode_state();
    type Edit = Box<dyn Fn(&yrs::MapRef, &mut yrs::TransactionMut)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "title",
            Box::new(|m, t| {
                m.insert(t, "title", "a plain string");
            }),
        ),
        (
            "body",
            Box::new(|m, t| {
                m.insert(t, "body", 42_i64);
            }),
        ),
        (
            "tags",
            Box::new(|m, t| {
                m.insert(t, "tags", ArrayPrelim::from([1_i64, 2]));
            }),
        ),
        (
            "links",
            Box::new(|m, t| {
                m.insert(t, "links", "x");
            }),
        ),
        (
            "aliases",
            Box::new(|m, t| {
                m.insert(t, "aliases", MapPrelim::default());
            }),
        ),
        (
            "props",
            Box::new(|m, t| {
                let p = m.insert(t, "props", MapPrelim::default());
                p.insert(t, "ID", 7_i64);
            }),
        ),
        (
            "kind",
            Box::new(|m, t| {
                m.insert(t, "kind", true);
            }),
        ),
        (
            "todo",
            Box::new(|m, t| {
                m.insert(t, "todo", ArrayPrelim::default());
            }),
        ),
        (
            "created",
            Box::new(|m, t| {
                m.insert(t, "created", 1_700_000_000_i64);
            }),
        ),
        (
            "org_header",
            Box::new(|m, t| {
                let h = m.insert(t, "org_header", MapPrelim::default());
                h.insert(t, "has_title_line", "yes");
            }),
        ),
        (
            "org_header",
            Box::new(|m, t| {
                m.insert(t, "org_header", "flat");
            }),
        ),
    ];
    for (field, edit) in cases {
        let up = foreign(&base, |m, t| edit(m, t));
        assert_eq!(
            KbNodeDoc::field_type_introduced(&base, &up).unwrap(),
            Some(field),
            "{field}"
        );
    }
}

/// An unknown key is left alone (a newer client's additive field), and a
/// legacy document already mistyped stays writable: only a violation the
/// update INTRODUCES is named. Kills a check on unknown keys, and one that
/// locks a legacy document.
#[test]
fn unknown_keys_and_existing_mistypes_are_left_alone() {
    let base = node().encode_state();
    let up = foreign(&base, |m, t| {
        m.insert(t, "a_future_field", 3_i64);
    });
    assert_eq!(KbNodeDoc::field_type_introduced(&base, &up).unwrap(), None);

    let legacy = {
        let mut b = KbNodeDoc::from_bytes(&base).unwrap();
        let up = foreign(&base, |m, t| {
            m.insert(t, "title", "legacy plain title");
        });
        b.apply_update(&up).unwrap();
        b.encode_state()
    };
    assert!(KbNodeDoc::from_bytes(&legacy)
        .unwrap()
        .field_type_violations()
        .contains("title"));
    let mut edit = KbNodeDoc::from_bytes(&legacy).unwrap();
    let up = edit.set_kind(Some("Note"));
    assert_eq!(
        KbNodeDoc::field_type_introduced(&legacy, &up).unwrap(),
        None
    );
}

/// A new document (empty base) is checked too: a create that writes a
/// mistyped field is named.
#[test]
fn a_new_document_is_checked_from_nothing() {
    let doc = Doc::with_client_id(7);
    let root = doc.get_or_insert_map("node");
    {
        let mut t = doc.transact_mut();
        root.insert(&mut t, "id", "n");
        root.insert(&mut t, "title", 5_i64);
    }
    let up = {
        let t = doc.transact();
        t.encode_state_as_update_v1(&StateVector::default())
    };
    assert_eq!(
        KbNodeDoc::field_type_introduced(&[], &up).unwrap(),
        Some("title")
    );
}
