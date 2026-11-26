//! The node-list guard at the store, on both write paths, by writer kind.

use super::super::DocStore;
use crate::doc_editors::{Attribution, InternalWriter, ManifestChange, Writer};
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

type Change = (&'static str, Vec<u8>, ManifestChange<'static>);

/// An add, a removal and a retitle, each from the stored state, with the
/// manifest change each is.
fn changes(state: &[u8]) -> Vec<Change> {
    let mut add = KbCollectionDoc::from_bytes(state).unwrap();
    let mut rm = KbCollectionDoc::from_bytes(state).unwrap();
    let mut retitle = KbCollectionDoc::from_bytes(state).unwrap();
    let change = |node_id, title| ManifestChange { node_id, title };
    vec![
        (
            "add",
            add.add_node("concept:new", "New"),
            change("concept:new", Some("New")),
        ),
        (
            "remove",
            rm.remove_node("concept:kept"),
            change("concept:kept", None),
        ),
        (
            "retitle",
            retitle.add_node("concept:kept", "Renamed"),
            change("concept:kept", Some("Renamed")),
        ),
    ]
}

/// Switch on: a session's add, removal or retitle is refused on apply and on
/// replace, and leaves the list as it was; the same change applied as a
/// verified manifest op lands.
#[tokio::test]
async fn with_the_switch_on_only_a_manifest_op_changes_the_list() {
    for (what, update, change) in changes(&fresh(true).await.1) {
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
        let signed = Writer::manifest_op(PERSON, "fp-person", change);
        store
            .apply_update(DOC, &update, None, signed)
            .await
            .unwrap();
    }
}

/// A manifest-op write changes the list by its signed change and nothing
/// else: an update that also removes another node is refused whatever it
/// signed, and so is one whose signed change is not the one it makes.
#[tokio::test]
async fn a_manifest_op_write_makes_only_its_signed_change() {
    let (store, state) = fresh(true).await;
    let mut more = KbCollectionDoc::from_bytes(&state).unwrap();
    let sv = more.state_vector();
    let _ = more.add_node("concept:new", "New");
    let _ = more.remove_node("concept:kept");
    let update = more.encode_diff(&sv).unwrap();
    let wrong = [
        ManifestChange {
            node_id: "concept:new",
            title: Some("New"),
        },
        ManifestChange {
            node_id: "concept:other",
            title: Some("New"),
        },
        ManifestChange {
            node_id: "concept:new",
            title: Some("Other"),
        },
    ];
    for change in wrong {
        let by = Writer::manifest_op(PERSON, "fp-person", change);
        let r = store.apply_update(DOC, &update, None, by).await;
        assert!(
            matches!(r, Err(StorageError::UnsignedNodeListChange { .. })),
            "{change:?}: {r:?}"
        );
    }
    assert_eq!(nodes(&store).await, vec!["concept:kept"]);
}

/// The second review's attack: a raw change held pending on an origin that
/// the daemon's own manifest-op update creates is refused when that update
/// releases it, and nothing lands.
#[tokio::test]
async fn a_held_change_released_by_a_manifest_op_write_is_refused() {
    let (store, state) = fresh(true).await;
    let mut daemon = KbCollectionDoc::from_bytes(&state).unwrap();
    let signed_add = daemon.add_node("concept:signed", "Signed");
    let mut attacker = KbCollectionDoc::from_bytes(&state).unwrap();
    attacker.apply_update(&signed_add).unwrap();
    let sv = attacker.state_vector();
    // Its origin is the daemon's entry, so yrs holds it until that lands. (A
    // fresh key has no origin and would land, and be refused, at once.)
    let _ = attacker.add_node("concept:signed", "Hijacked");
    let held = attacker.encode_diff(&sv).unwrap();
    store.apply_update(DOC, &held, None, PERSON).await.unwrap();
    assert_eq!(nodes(&store).await, vec!["concept:kept"], "not held back");
    let change = ManifestChange {
        node_id: "concept:signed",
        title: Some("Signed"),
    };
    let by = Writer::manifest_op(PERSON, "fp-person", change);
    let r = store.apply_update(DOC, &signed_add, None, by).await;
    assert!(
        matches!(r, Err(StorageError::UnsignedNodeListChange { .. })),
        "{r:?}"
    );
    assert_eq!(nodes(&store).await, vec!["concept:kept"]);
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
        let (_, add, _) = changes(&state).remove(0);
        store.apply_update(DOC, &add, None, by).await.unwrap();
        assert!(
            nodes(&store).await.contains(&"concept:new".to_string()),
            "{by:?}"
        );
    }
}
