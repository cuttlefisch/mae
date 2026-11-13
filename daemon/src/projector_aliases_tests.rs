//! The projector derives a node's aliases by `mae_sync::kb::aliases`: in an
//! `org-literal-v1` KB from the `ROAM_ALIASES` property, elsewhere from the
//! array. These pin that the Cozo row (and so FTS) carries the derived value,
//! that a property edit and a format change both re-derive it, and that the
//! drift check compares against the same derivation.

use super::*;

use crate::storage::SqliteBackend;
use mae_kb::store::SearchHit;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, BODY_FORMAT_ORG_LITERAL_V1};
use std::sync::Mutex as StdMutex;

const TEST_BY: crate::doc_editors::Attribution<'static> =
    crate::doc_editors::Attribution::Principal("test-editor");

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

impl Fixture {
    fn new() -> Self {
        let doc_store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open_memory().unwrap()),
            500,
        ));
        let stores = Arc::new(MemStores(StdMutex::new(HashMap::new())));
        let projector = Projector::new(Arc::clone(&doc_store), stores.clone());
        Self {
            doc_store,
            stores,
            projector,
        }
    }

    /// A KB `kb` with one node `n1` whose `ROAM_ALIASES` is `roam` and whose
    /// array is `array`, projected.
    async fn seed(&self, kb: &str, format: Option<&str>, roam: &str, array: &[&str]) {
        let mut node = KbNodeDoc::new("n1", "Plain title", "plain body", &[]);
        let props = HashMap::from([
            ("ID".to_string(), "n1".to_string()),
            ("ROAM_ALIASES".to_string(), roam.to_string()),
        ]);
        let _ = node.set_properties(&props);
        let array: Vec<String> = array.iter().map(|s| s.to_string()).collect();
        let _ = node.set_aliases(&array);
        self.doc_store
            .apply_update(
                &mae_sync::kb_node_doc_name(kb, "n1"),
                &node.encode(),
                None,
                TEST_BY,
            )
            .await
            .unwrap();
        let mut coll = KbCollectionDoc::new(kb, "owner");
        coll.add_node("n1", "Plain title");
        if let Some(f) = format {
            let _ = coll.set_body_format(f);
        }
        self.doc_store
            .apply_update(&format!("kbc:{kb}"), &coll.encode_state(), None, TEST_BY)
            .await
            .unwrap();
        self.projector
            .project_doc(&format!("kbc:{kb}"))
            .await
            .unwrap();
    }

    async fn store(&self, kb: &str) -> Arc<CozoKbStore> {
        self.stores.store_for(kb).await.unwrap()
    }

    async fn aliases(&self, kb: &str) -> Vec<String> {
        self.store(kb)
            .await
            .get_node("n1")
            .unwrap()
            .unwrap()
            .aliases
    }

    /// Apply `edit` to the stored node doc as a real update, and project it.
    async fn edit_node(&self, kb: &str, edit: impl FnOnce(&mut KbNodeDoc) -> Vec<u8>) {
        let name = mae_sync::kb_node_doc_name(kb, "n1");
        let state = self.doc_store.encode_state(&name).await.unwrap();
        let mut doc = KbNodeDoc::from_bytes_with_client_id(&state, 4242).unwrap();
        let update = edit(&mut doc);
        self.doc_store
            .apply_update(&name, &update, None, TEST_BY)
            .await
            .unwrap();
        self.projector.project_doc(&name).await.unwrap();
    }
}

const LIT: Option<&str> = Some(BODY_FORMAT_ORG_LITERAL_V1);

/// The org-literal row carries the property's aliases, and FTS finds the node
/// by one. `w\157mbat` reads as `wombat` (an octal escape), a word that is in
/// neither the title, the body nor the raw property text, so only the derived
/// alias can match it.
#[tokio::test]
async fn org_literal_projection_derives_aliases_and_fts_finds_them() {
    let f = Fixture::new();
    f.seed("lit", LIT, "foo \"bar baz\" \"w\\157mbat\"", &[])
        .await;
    assert_eq!(f.aliases("lit").await, vec!["foo", "bar baz", "wombat"]);
    let hits: Vec<SearchHit> = f.store("lit").await.fts_search("wombat", 10).unwrap();
    assert!(hits.iter().any(|h| h.id == "n1"), "{hits:?}");
}

/// Editing the property re-derives the aliases on the node's own change.
#[tokio::test]
async fn editing_roam_aliases_changes_the_projected_aliases() {
    let f = Fixture::new();
    f.seed("lit", LIT, "foo", &[]).await;
    assert_eq!(f.aliases("lit").await, vec!["foo"]);
    f.edit_node("lit", |d| {
        let mut p = d.properties();
        p.insert("ROAM_ALIASES".into(), "\"new one\" two".into());
        d.set_properties(&p)
    })
    .await;
    assert_eq!(f.aliases("lit").await, vec!["new one", "two"]);
}

/// A native KB: the array rules and the property means nothing.
#[tokio::test]
async fn native_projection_keeps_the_array() {
    let f = Fixture::new();
    f.seed("nat", None, "from-property", &["from-array"]).await;
    assert_eq!(f.aliases("nat").await, vec!["from-array"]);
}

/// An org-literal node whose array is (illegally) set does not project the
/// array's values.
#[tokio::test]
async fn org_literal_projection_ignores_an_illegal_array() {
    let f = Fixture::new();
    f.seed("lit", LIT, "real", &["smuggled"]).await;
    assert_eq!(f.aliases("lit").await, vec!["real"]);
}

/// The body format arriving AFTER the node was projected re-derives that node,
/// though the manifest did not change.
#[tokio::test]
async fn a_body_format_change_reprojects_existing_nodes() {
    let f = Fixture::new();
    f.seed("kb", None, "from-property", &["from-array"]).await;
    assert_eq!(f.aliases("kb").await, vec!["from-array"]);

    let state = f.doc_store.encode_state("kbc:kb").await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    let update = coll.set_body_format(BODY_FORMAT_ORG_LITERAL_V1);
    f.doc_store
        .apply_update("kbc:kb", &update, None, TEST_BY)
        .await
        .unwrap();
    f.projector.project_doc("kbc:kb").await.unwrap();
    assert_eq!(f.aliases("kb").await, vec!["from-property"]);
}

/// The drift check derives the same way the projector writes: a freshly
/// projected org-literal KB is clean, and a row holding the raw array instead
/// is drift.
#[tokio::test]
async fn reconcile_compares_against_the_derived_aliases() {
    let f = Fixture::new();
    f.seed("lit", LIT, "foo \"bar baz\"", &["smuggled"]).await;
    let report = f.projector.reconcile_kb("lit", false).await.unwrap();
    assert!(report.is_clean(), "{report:?}");

    let store = f.store("lit").await;
    let mut row = store.get_node("n1").unwrap().unwrap();
    row.aliases = vec!["smuggled".into()];
    store.insert_node(&row).unwrap();
    let report = f.projector.reconcile_kb("lit", false).await.unwrap();
    assert_eq!(report.differing, vec!["n1".to_string()]);
}
