//! The node-list guard at the store, on both write paths, by writer kind.

use super::super::DocStore;
use crate::doc_editors::{Attribution, InternalWriter, Writer};
use crate::storage::{SqliteBackend, StorageError};
use mae_sync::kb::KbCollectionDoc;
use std::sync::Arc;

const DOC: &str = "kbc:kb-g3";
const PERSON: Attribution<'static> = Attribution::Principal("fp-person");

/// A store with the switch set, holding a collection with one node; the
/// collection's state.
async fn fresh(require: bool) -> (DocStore, Vec<u8>) {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = DocStore::new(backend, 500).with_require_signed_content_ops(require);
    // One client id, so every fresh store holds the same collection and an
    // update built against one applies to another.
    let mut c = KbCollectionDoc::new_with_client_id("kb-g3", "fp-owner", 41);
    let _ = c.add_node("concept:kept", "Kept");
    let by = Attribution::Internal(InternalWriter::CollectionMigration);
    store.share_doc(DOC, &c.encode_state(), by).await.unwrap();
    let state = store.encode_state(DOC).await.unwrap();
    (store, state)
}

async fn nodes(store: &DocStore) -> Vec<String> {
    let state = store.encode_state(DOC).await.unwrap();
    let c = KbCollectionDoc::from_bytes(&state).unwrap();
    let mut ids: Vec<String> = c.list_nodes().into_iter().map(|(id, _)| id).collect();
    ids.sort();
    ids
}

/// An add, a removal and a retitle, each from the stored state.
fn changes(state: &[u8]) -> Vec<(&'static str, Vec<u8>)> {
    let mut add = KbCollectionDoc::from_bytes(state).unwrap();
    let mut rm = KbCollectionDoc::from_bytes(state).unwrap();
    let mut retitle = KbCollectionDoc::from_bytes(state).unwrap();
    vec![
        ("add", add.add_node("concept:new", "New")),
        ("remove", rm.remove_node("concept:kept")),
        ("retitle", retitle.add_node("concept:kept", "Renamed")),
    ]
}

/// Switch on: a session's add, removal or retitle is refused on apply and on
/// replace, and leaves the list as it was; the same change applied as a
/// verified manifest op lands.
#[tokio::test]
async fn with_the_switch_on_only_a_manifest_op_changes_the_list() {
    for (what, update) in changes(&fresh(true).await.1) {
        let (store, state) = fresh(true).await;
        for by in [PERSON, Attribution::Unauthenticated] {
            let r = store.apply_update(DOC, &update, None, by).await;
            assert!(
                matches!(r, Err(StorageError::UnsignedNodeListChange { .. })),
                "{what} by {by:?}: {r:?}"
            );
            let mut whole = KbCollectionDoc::from_bytes(&state).unwrap();
            whole.apply_update(&update).unwrap();
            let r = store.share_doc(DOC, &whole.encode_state(), by).await;
            assert!(
                matches!(r, Err(StorageError::UnsignedNodeListChange { .. })),
                "{what} replaced by {by:?}: {r:?}"
            );
            assert_eq!(nodes(&store).await, vec!["concept:kept"], "{what}");
        }
        let signed = Writer::manifest_op(PERSON, "fp-person");
        store
            .apply_update(DOC, &update, None, signed)
            .await
            .unwrap();
    }
}

/// Switch on: a session write that leaves the list alone lands. Switch off:
/// any session write lands. A mesh relay and the daemon itself are outside
/// G3 (#727 part 3), pinned so that changing it is a decision.
#[tokio::test]
async fn the_guard_holds_only_session_writes_with_the_switch_on() {
    let (store, state) = fresh(true).await;
    let mut plain = KbCollectionDoc::from_bytes(&state).unwrap();
    let policy = plain.set_transport_policy(mae_sync::kb::TransportPolicy::Hub);
    store
        .apply_update(DOC, &policy, None, PERSON)
        .await
        .unwrap();
    for (switch, by) in [
        (false, PERSON),
        (true, Attribution::MeshPeer("fp-peer")),
        (
            true,
            Attribution::Internal(InternalWriter::CollectionMigration),
        ),
    ] {
        let (store, state) = fresh(switch).await;
        let (_, add) = changes(&state).remove(0);
        store.apply_update(DOC, &add, None, by).await.unwrap();
        assert!(
            nodes(&store).await.contains(&"concept:new".to_string()),
            "{by:?}"
        );
    }
}
