//! ADR-115 D5's live set: every hash a node document or a history version of
//! the KB binds, and nothing else; a frozen version keeps the hash its node
//! has since replaced; a node dropped from the manifest still holds its own.

use super::*;

use mae_sync::kb::history::VersionKind;
use mae_sync::kb::{AttachmentRef, KbCollectionDoc, KbNodeDoc, ATTACHMENTS_ROOT};
use mae_sync::yrs::{Map, Transact};

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

    // Removing an entry releases its hash; a malformed entry adds none.
    edit(&store, "kb", "n2", |d| d.remove_attachment("f.png")).await;
    edit(&store, "kb", "n2", |d| {
        let sv = d.state_vector();
        let map = d.doc().get_or_insert_map(ATTACHMENTS_ROOT);
        map.insert(
            &mut d.doc().transact_mut(),
            "bad/name.png",
            mae_sync::yrs::Any::Map(std::sync::Arc::new(
                [(
                    "sha256".to_string(),
                    mae_sync::yrs::Any::from(h(7).as_str()),
                )]
                .into_iter()
                .collect(),
            )),
        );
        d.encode_diff(&sv).unwrap()
    })
    .await;
    assert_eq!(
        store.referenced_blobs("kb").await.unwrap().hashes,
        set(&[1, 2, 3])
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
        set(&[1, 2, 3])
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
