//! ADR-115 D5's live set: every hash a node document or a history version of
//! the KB binds, and nothing else; a frozen version keeps the hash its node
//! has since replaced; a node dropped from the manifest still holds its own.

use super::*;

use mae_sync::kb::history::VersionKind;
use mae_sync::kb::{AttachmentRef, KbCollectionDoc, KbNodeDoc, ATTACHMENTS_ROOT};
use mae_sync::yrs::{Any, Map, Transact};

use crate::storage::SqliteBackend;
use std::sync::Arc;

const TEST_BY: crate::doc_editors::Attribution<'static> =
    crate::doc_editors::Attribution::Principal("test-editor");

fn h(seed: u8) -> String {
    hex::encode([seed; 32])
}

fn png(seed: u8) -> AttachmentRef {
    AttachmentRef::new(h(seed), 1, "image/png").unwrap()
}

fn set(seeds: &[u8]) -> BTreeSet<String> {
    seeds.iter().map(|s| h(*s)).collect()
}

async fn put_node(store: &DocStore, kb: &str, id: &str, entries: &[(&str, u8)]) {
    let mut node = KbNodeDoc::new(id, "T", "B", &[]);
    for (name, seed) in entries {
        let _ = node.set_attachment(name, &png(*seed)).unwrap();
    }
    store
        .apply_update(
            &mae_sync::kb_node_doc_name(kb, id),
            &node.encode(),
            None,
            TEST_BY,
        )
        .await
        .unwrap();
}

async fn edit(store: &DocStore, kb: &str, id: &str, f: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>) {
    let name = mae_sync::kb_node_doc_name(kb, id);
    let state = store.encode_state(&name).await.unwrap();
    let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, 4242).unwrap();
    let update = f(&mut doc);
    store
        .apply_update(&name, &update, None, TEST_BY)
        .await
        .unwrap();
}

/// A peer's raw entry at `name`, carrying hash `seed` and `fields` besides.
async fn put_raw(
    store: &DocStore,
    kb: &str,
    id: &str,
    name: &str,
    seed: u8,
    fields: &[(&str, Any)],
) {
    let mut entry: std::collections::HashMap<String, Any> = fields
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    entry.insert("sha256".into(), Any::from(h(seed).as_str()));
    edit(store, kb, id, |d| {
        let sv = d.state_vector();
        let map = d.doc().get_or_insert_map(ATTACHMENTS_ROOT);
        map.insert(&mut d.doc().transact_mut(), name, Any::Map(Arc::new(entry)));
        d.encode_diff(&sv).unwrap()
    })
    .await;
}

/// A hostile peer's entry under a bad name, carrying hash `seed` and nothing
/// else.
async fn put_malformed(store: &DocStore, kb: &str, id: &str, seed: u8) {
    put_raw(store, kb, id, "bad/name.png", seed, &[]).await;
}

#[tokio::test]
async fn the_live_set_is_every_node_and_every_history_version_of_the_kb() {
    let store = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
    put_node(&store, "kb", "n1", &[("d.png", 1)]).await;
    put_node(&store, "kb", "n2", &[("e.png", 3), ("f.png", 4)]).await;
    put_node(&store, "other", "n1", &[("d.png", 9)]).await;
    let mut coll = KbCollectionDoc::new("kb", "owner");
    coll.add_node("n1", "T");
    coll.add_node("n2", "T");
    store
        .apply_update("kbc:kb", &coll.encode_state(), None, TEST_BY)
        .await
        .unwrap();
    assert_eq!(
        store.referenced_blobs("kb").await.unwrap().hashes,
        set(&[1, 3, 4])
    );

    // A named version freezes d.png at hash 1; the node then replaces it.
    store
        .history_checkpoint("kb", "n1", VersionKind::Named, Some("v1".into()), None)
        .await
        .unwrap()
        .expect("a named version is always cut");
    edit(&store, "kb", "n1", |d| {
        d.set_attachment("d.png", &png(2)).unwrap()
    })
    .await;
    let live = store.referenced_blobs("kb").await.unwrap();
    assert!(live.complete());
    assert_eq!(
        live.hashes,
        set(&[1, 2, 3, 4]),
        "the version still holds hash 1"
    );

    // The frozen version shows its own map, not the node's.
    let versions = store.history_list("kb", "n1").await.unwrap();
    let frozen = versions[0].node().unwrap().unwrap();
    assert_eq!(frozen.attachments["d.png"], png(1));

    // Removing an entry releases its hash; a malformed entry holding one
    // still counts (the sweep's reader fails safe).
    edit(&store, "kb", "n2", |d| d.remove_attachment("f.png")).await;
    put_malformed(&store, "kb", "n2", 7).await;
    assert_eq!(
        store.referenced_blobs("kb").await.unwrap().hashes,
        set(&[1, 2, 3, 7])
    );

    // A node dropped from the manifest keeps its document until it is
    // purged, so it still holds its blobs (D5's bin).
    coll.remove_node("n2");
    store
        .apply_update("kbc:kb", &coll.encode_state(), None, TEST_BY)
        .await
        .unwrap();
    assert_eq!(
        store.referenced_blobs("kb").await.unwrap().hashes,
        set(&[1, 2, 3, 7])
    );

    // The other KB's set is its own.
    assert_eq!(
        store.referenced_blobs("other").await.unwrap().hashes,
        set(&[9])
    );
    assert_eq!(
        store.referenced_blobs("none").await.unwrap(),
        ReferencedBlobs::default()
    );
}

/// The sweep reads a node document failing safe: an entry the display reader
/// rejects, for its NAME or for one field's shape, still holds its blob when
/// it carries a hash, so a newer peer's image is never collected.
#[tokio::test]
async fn an_entry_the_reader_rejects_still_holds_its_blob() {
    let store = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
    put_node(&store, "kb", "n", &[("ok.png", 1)]).await;
    let shape = [
        ("size", Any::Number(5.0)),
        ("media_type", Any::from("image/png")),
    ];
    put_raw(&store, "kb", "n", "CON.png", 2, &shape).await;
    let string_size = [
        ("size", Any::from("5")),
        ("media_type", Any::from("image/png")),
    ];
    put_raw(&store, "kb", "n", "s.png", 3, &string_size).await;

    let name = mae_sync::kb_node_doc_name("kb", "n");
    let doc = KbNodeDoc::from_bytes(&store.encode_state(&name).await.unwrap()).unwrap();
    assert_eq!(
        doc.attachments().len(),
        1,
        "the display reader rejects both"
    );
    let live = store.referenced_blobs("kb").await.unwrap();
    assert!(live.complete());
    assert_eq!(live.hashes, set(&[1, 2, 3]));
}

/// A node document the sweep's reader cannot walk in full (here, an entry
/// nested past its depth bound) is listed unreadable, so the KB's set is not
/// complete and no sweep may collect from it; it is never read as holding
/// nothing.
#[tokio::test]
async fn a_document_past_the_walk_bound_is_unreadable() {
    let store = DocStore::new(Arc::new(SqliteBackend::open_memory().unwrap()), 500);
    put_node(&store, "kb", "ok", &[("a.png", 1)]).await;
    put_node(&store, "kb", "deep", &[("b.png", 2)]).await;
    let mut value = Any::from(h(3).as_str());
    for _ in 0..mae_sync::kb::MAX_REFERENCE_DEPTH {
        let m: std::collections::HashMap<String, Any> = [("k".to_string(), value)].into();
        value = Any::Map(Arc::new(m));
    }
    edit(&store, "kb", "deep", |d| {
        let sv = d.state_vector();
        let map = d.doc().get_or_insert_map(ATTACHMENTS_ROOT);
        map.insert(&mut d.doc().transact_mut(), "deep.png", value);
        d.encode_diff(&sv).unwrap()
    })
    .await;

    let live = store.referenced_blobs("kb").await.unwrap();
    assert!(!live.complete());
    assert_eq!(
        live.unreadable,
        vec![mae_sync::kb_node_doc_name("kb", "deep")]
    );
    assert!(live.hashes.contains(&h(1)));
}
