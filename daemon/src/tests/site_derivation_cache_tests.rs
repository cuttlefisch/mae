//! The live site's derivation cache (ADR-119 D6, first step) through the real
//! `site::page` and a real `DocStore`: an unchanged KB is derived once, and
//! every kind of change to it shows on the next request.

use mae_daemon::site::{self, SitePage};
use mae_sync::kb::{KbCollectionDoc, KbNodeDoc};

use super::site_tests::{kb, seed, BY, LIMITS, READER, STRANGER};

async fn body_of(store: &mae_daemon::doc_store::DocStore, id: &str) -> Option<String> {
    match site::page(store, "kb-site", id, Some(READER), LIMITS).await {
        SitePage::Found { html, .. } => Some(html),
        _ => None,
    }
}

/// Repeat requests, for the same page or another, derive the KB once.
#[tokio::test]
async fn an_unchanged_kb_is_derived_once() {
    let store = kb().await;
    for id in ["alpha", "alpha", "beta", "alpha"] {
        assert!(body_of(&store, id).await.is_some(), "{id}");
    }
    assert_eq!(store.site_cache().derivations(), 1);
}

/// An edit to a node's body, through the store's write path, shows on the
/// next request.
#[tokio::test]
async fn an_edit_shows_on_the_next_request() {
    let store = kb().await;
    assert!(body_of(&store, "beta")
        .await
        .unwrap()
        .contains("Beta's body"));
    let name = mae_sync::kb_node_doc_name("kb-site", "beta");
    let (state, _) = store.encode_state_and_sv(&name).await.unwrap();
    let mut node = KbNodeDoc::from_bytes(&state).unwrap();
    let edit = node.set_body("A newer body.\n");
    store.apply_update(&name, &edit, None, BY).await.unwrap();
    let html = body_of(&store, "beta").await.unwrap();
    assert!(html.contains("A newer body"), "{html}");
    assert!(!html.contains("Beta's body"), "{html}");
    assert_eq!(store.site_cache().derivations(), 2);
}

/// A node removed from the collection, and a node document deleted while
/// still listed, are gone on the next request.
#[tokio::test]
async fn a_removed_or_deleted_node_is_gone_on_the_next_request() {
    let store = kb().await;
    assert!(body_of(&store, "beta").await.is_some());
    let (state, _) = store.encode_state_and_sv("kbc:kb-site").await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    let rm = coll.remove_node("beta");
    store
        .apply_update("kbc:kb-site", &rm, None, BY)
        .await
        .unwrap();
    assert!(body_of(&store, "beta").await.is_none(), "removed");

    assert!(body_of(&store, "alpha").await.is_some());
    store
        .delete_doc(&mae_sync::kb_node_doc_name("kb-site", "alpha"))
        .await
        .unwrap();
    assert!(body_of(&store, "alpha").await.is_none(), "deleted");
}

/// The cache never stands in for the read gate: with the KB derived for a
/// reader, a stranger is still refused, and so is the reader once removed.
#[tokio::test]
async fn the_read_gate_runs_on_every_request() {
    let store = kb().await;
    assert!(body_of(&store, "alpha").await.is_some());
    assert_eq!(
        site::page(&store, "kb-site", "alpha", Some(STRANGER), LIMITS).await,
        SitePage::NotFound
    );
    let (state, _) = store.encode_state_and_sv("kbc:kb-site").await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    let out = coll.remove_principal(READER);
    store
        .apply_update("kbc:kb-site", &out, None, BY)
        .await
        .unwrap();
    assert!(body_of(&store, "alpha").await.is_none(), "removed reader");
}

/// Two KBs in one store never answer for each other.
#[tokio::test]
async fn one_kbs_site_never_answers_for_another() {
    let store = kb().await;
    seed(
        &store,
        "kb-other",
        &[("alpha", "Other alpha", "The other KB.\n")],
    )
    .await;
    let mine = body_of(&store, "alpha").await.unwrap();
    let SitePage::Found { html: theirs, .. } =
        site::page(&store, "kb-other", "alpha", Some(READER), LIMITS).await
    else {
        panic!("the other KB's page");
    };
    assert!(mine.contains("Alpha links"), "{mine}");
    assert!(theirs.contains("The other KB"), "{theirs}");
    assert!(!theirs.contains("Alpha links"), "{theirs}");
}
