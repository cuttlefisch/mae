//! A node's title comes from its own document, never from the collection
//! manifest. The manifest's title is written once, when a node is added, and no
//! update path refreshes it, so after a rename the two disagree; each
//! `kb/query.*` method must give the document's title.

use std::sync::Arc;

use mae_mcp::identity::Identity;
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role, TransportPolicy};
use serde_json::{json, Value};

use mae_daemon::doc_store::DocStore;
use mae_daemon::kb_query;

use super::kb_query_tests::{fresh_doc_store, generous_limits, LIST_VIEWER};

/// Every test write is attributed; the editor-record tests use their own.
const TEST_BY: mae_daemon::doc_editors::Attribution<'static> =
    mae_daemon::doc_editors::Attribution::Principal("test-editor");

const KB: &str = "titles-kb";

/// `n1` is listed in the manifest as "Manifiesto" but its document says
/// "Nodo", as after a rename; `n2` is listed and has no document.
async fn seed_renamed(doc_store: &DocStore) {
    let owner = Arc::new(Identity::generate("owner"));
    doc_store.set_signer(Arc::clone(&owner));
    let mut coll = KbCollectionDoc::new_owned(KB, &owner.fingerprint(), "owner");
    coll.set_transport_policy(TransportPolicy::Hub);
    coll.upsert_member(LIST_VIEWER, "member", Role::Viewer);
    let _ = coll.add_node("n1", "Manifiesto");
    let _ = coll.add_node("n2", "Fantasma");
    doc_store
        .share_doc(&format!("kbc:{KB}"), &coll.encode_state(), TEST_BY)
        .await
        .unwrap();
    let node = KbNodeDoc::new("n1", "Nodo", "body", &[]);
    doc_store
        .share_doc(
            &mae_sync::kb_node_doc_name(KB, "n1"),
            &node.encode_state(),
            TEST_BY,
        )
        .await
        .unwrap();
}

async fn call(doc_store: &DocStore, method: &str, params: Value) -> Value {
    kb_query::dispatch(
        method,
        &params,
        doc_store,
        Some(LIST_VIEWER),
        generous_limits(),
    )
    .await
    .unwrap_or_else(|e| panic!("{method}: {e:?}"))
}

#[tokio::test]
async fn titles_and_list_give_the_documents_title_not_the_manifests() {
    let doc_store = fresh_doc_store().await;
    seed_renamed(&doc_store).await;

    let titles = call(&doc_store, "kb/query.titles", json!({"kb_id": KB})).await;
    let pairs = titles["pairs"].as_array().unwrap();
    assert!(pairs.contains(&json!(["n1", "Nodo"])), "{titles}");

    let list = call(&doc_store, "kb/query.list", json!({"kb_id": KB})).await;
    let n1 = &list["nodes"].as_array().unwrap()[0];
    assert_eq!(n1["title"], "Nodo");

    let get = call(
        &doc_store,
        "kb/query.get",
        json!({"kb_id": KB, "node_id": "n1"}),
    )
    .await;
    assert_eq!(get["title"], "Nodo");

    for text in [&titles, &list, &get] {
        assert!(
            !text.to_string().contains("Manifiesto"),
            "stale title served: {text}"
        );
    }
}

/// A listed node with no readable document: `titles` gives it an empty title
/// and names it in `unavailable`; `list` reports exactly `{id, unavailable}`.
/// Neither falls back to the manifest's title.
#[tokio::test]
async fn an_unreadable_node_never_gets_the_manifests_title() {
    let doc_store = fresh_doc_store().await;
    seed_renamed(&doc_store).await;

    let titles = call(&doc_store, "kb/query.titles", json!({"kb_id": KB})).await;
    assert!(
        titles["pairs"]
            .as_array()
            .unwrap()
            .contains(&json!(["n2", ""])),
        "{titles}"
    );
    assert_eq!(titles["unavailable"], json!(["n2"]));

    let list = call(&doc_store, "kb/query.list", json!({"kb_id": KB})).await;
    let n2 = list["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == "n2")
        .expect("n2 left out");
    assert_eq!(*n2, json!({"id": "n2", "unavailable": true}));
    assert!(!list.to_string().contains("Fantasma"), "{list}");
}
