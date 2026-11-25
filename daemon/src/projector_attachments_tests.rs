//! ADR-115 D4: the projection's `node_attachments` rows equal the node
//! document's map after every kind of edit, a malformed entry from a hostile
//! peer never reaches them, the drift check notices rows changed behind the
//! projector's back, and a node removed from the collection takes its rows.

use super::*;

use crate::storage::SqliteBackend;
use mae_sync::kb::{AttachmentRef, KbCollectionDoc, KbNodeDoc, ATTACHMENTS_ROOT};
use mae_sync::yrs::{self, Map, Transact};
use std::sync::Mutex as StdMutex;

const TEST_BY: crate::doc_editors::Attribution<'static> =
    crate::doc_editors::Attribution::Principal("test-editor");
const KB: &str = "kb";

struct MemStores(StdMutex<HashMap<String, Arc<CozoKbStore>>>);
#[async_trait::async_trait]
impl ProjectionStores for MemStores {
    async fn store_for(&self, kb_id: &str) -> Result<Arc<CozoKbStore>, String> {
        let mut m = self.0.lock().unwrap();
        Ok(Arc::clone(m.entry(kb_id.to_string()).or_insert_with(
            || Arc::new(CozoKbStore::open_mem().unwrap()),
        )))
    }
}

struct Fixture {
    doc_store: Arc<DocStore>,
    stores: Arc<MemStores>,
    projector: Projector,
}

fn png(seed: u8) -> AttachmentRef {
    AttachmentRef::new(hex::encode([seed; 32]), 10 + seed as u64, "image/png").unwrap()
}

impl Fixture {
    /// Nodes `ids` in KB `KB`, without attachments, projected.
    async fn new(ids: &[&str]) -> Self {
        let doc_store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let stores = Arc::new(MemStores(StdMutex::new(HashMap::new())));
        let projector = Projector::new(Arc::clone(&doc_store), stores.clone());
        let f = Self {
            doc_store,
            stores,
            projector,
        };
        let mut coll = KbCollectionDoc::new(KB, "owner");
        for id in ids {
            let node = KbNodeDoc::new(id, "Title", "[[attachment:d.png]]", &[]);
            f.doc_store
                .apply_update(
                    &mae_sync::kb_node_doc_name(KB, id),
                    &node.encode(),
                    None,
                    TEST_BY,
                )
                .await
                .unwrap();
            coll.add_node(id, "Title");
        }
        f.put_collection(&coll).await;
        f
    }

    async fn put_collection(&self, coll: &KbCollectionDoc) {
        self.doc_store
            .apply_update(&format!("kbc:{KB}"), &coll.encode_state(), None, TEST_BY)
            .await
            .unwrap();
        self.projector
            .project_doc(&format!("kbc:{KB}"))
            .await
            .unwrap();
    }

    async fn store(&self) -> Arc<CozoKbStore> {
        self.stores.store_for(KB).await.unwrap()
    }

    async fn doc(&self, id: &str) -> KbNodeDoc {
        let state = self
            .doc_store
            .encode_state(&mae_sync::kb_node_doc_name(KB, id))
            .await
            .unwrap();
        KbNodeDoc::from_bytes_with_client_id(&state, 4242).unwrap()
    }

    /// Apply `edit` to node `id` as a real update, and project it.
    async fn edit(&self, id: &str, edit: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>) {
        let name = mae_sync::kb_node_doc_name(KB, id);
        let mut doc = self.doc(id).await;
        let update = edit(&mut doc);
        self.doc_store
            .apply_update(&name, &update, None, TEST_BY)
            .await
            .unwrap();
        self.projector.project_doc(&name).await.unwrap();
    }

    /// The projection's rows against every node document's own map.
    async fn assert_rows_are_the_maps(&self, ids: &[&str], when: &str) {
        let mut expected = Vec::new();
        for id in ids {
            for (name, a) in self.doc(id).await.attachments() {
                expected.push((id.to_string(), name, a.sha256));
            }
        }
        expected.sort();
        assert_eq!(
            self.store().await.attachment_refs().unwrap(),
            expected,
            "{when}"
        );
    }
}

#[tokio::test]
async fn the_rows_equal_the_map_after_set_replace_rename_and_remove() {
    let ids = ["n1", "n2"];
    let f = Fixture::new(&ids).await;
    f.assert_rows_are_the_maps(&ids, "no attachments").await;

    f.edit("n1", |d| d.set_attachment("d.png", &png(1)).unwrap())
        .await;
    f.edit("n2", |d| d.set_attachment("e.png", &png(2)).unwrap())
        .await;
    f.assert_rows_are_the_maps(&ids, "after set").await;
    assert_eq!(f.store().await.attachment_refs().unwrap().len(), 2);

    f.edit("n1", |d| d.set_attachment("d.png", &png(3)).unwrap())
        .await;
    f.assert_rows_are_the_maps(&ids, "after replace").await;

    f.edit("n1", |d| d.rename_attachment("d.png", "f.png").unwrap())
        .await;
    f.assert_rows_are_the_maps(&ids, "after rename").await;

    f.edit("n1", |d| d.remove_attachment("f.png")).await;
    f.assert_rows_are_the_maps(&ids, "after remove").await;
    assert_eq!(
        f.store().await.attachment_refs().unwrap(),
        vec![("n2".into(), "e.png".into(), png(2).sha256)]
    );
    // The drift check agrees: what is projected is what truth derives.
    assert!(f
        .projector
        .reconcile_kb(KB, false)
        .await
        .unwrap()
        .is_clean());
}

#[tokio::test]
async fn a_malformed_entry_never_reaches_the_projection() {
    let ids = ["n1"];
    let f = Fixture::new(&ids).await;
    f.edit("n1", |d| d.set_attachment("ok.png", &png(1)).unwrap())
        .await;
    f.edit("n1", |d| {
        let sv = d.state_vector();
        let map = d.doc().get_or_insert_map(ATTACHMENTS_ROOT);
        {
            let mut txn = d.doc().transact_mut();
            map.insert(&mut txn, "a/b.png", yrs::Any::from("not an entry"));
            map.insert(&mut txn, "short.png", yrs::Any::Null);
        }
        d.encode_diff(&sv).unwrap()
    })
    .await;
    assert_eq!(
        f.store().await.attachment_refs().unwrap(),
        vec![("n1".into(), "ok.png".into(), png(1).sha256)]
    );
    assert!(f
        .projector
        .reconcile_kb(KB, false)
        .await
        .unwrap()
        .is_clean());
}

#[tokio::test]
async fn rows_changed_behind_the_projectors_back_are_drift_and_heal() {
    let ids = ["n1"];
    let f = Fixture::new(&ids).await;
    f.edit("n1", |d| d.set_attachment("d.png", &png(1)).unwrap())
        .await;
    let store = f.store().await;
    let mut stale = store.get_node("n1").unwrap().unwrap();
    stale.attachments.clear();
    store.insert_node(&stale).unwrap();
    let drift = f.projector.reconcile_kb(KB, true).await.unwrap();
    assert_eq!(drift.differing, vec!["n1".to_string()]);
    f.assert_rows_are_the_maps(&ids, "after heal").await;
    assert!(f
        .projector
        .reconcile_kb(KB, false)
        .await
        .unwrap()
        .is_clean());
}

#[tokio::test]
async fn a_node_removed_from_the_collection_takes_its_rows() {
    let ids = ["n1", "n2"];
    let f = Fixture::new(&ids).await;
    f.edit("n1", |d| d.set_attachment("d.png", &png(1)).unwrap())
        .await;
    f.edit("n2", |d| d.set_attachment("e.png", &png(2)).unwrap())
        .await;
    let state = f
        .doc_store
        .encode_state(&format!("kbc:{KB}"))
        .await
        .unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    coll.remove_node("n1");
    f.put_collection(&coll).await;
    assert_eq!(
        f.store().await.attachment_refs().unwrap(),
        vec![("n2".into(), "e.png".into(), png(2).sha256)]
    );
}
