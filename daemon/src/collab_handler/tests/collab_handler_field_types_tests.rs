//! A node update that leaves a known field mistyped is refused with
//! `invalid_field`, naming the field, on both write paths (`kb/node_update`
//! and `sync/update`), and nothing is applied; a well-typed update is not.

use super::*;
use mae_sync::yrs::{updates::decoder::Decode, Doc, Map, ReadTxn, Transact, Update};

const KB: &str = "kra";
const NODE: &str = "concept:n";

fn node_doc() -> String {
    mae_sync::kb_node_doc_name(KB, NODE)
}

/// Another client's update on top of `base`: `title` set to a plain string.
fn mistyped_title(base: &[u8]) -> Vec<u8> {
    // The writer's own client id (principal + epoch), as the fence requires.
    let doc = Doc::with_client_id(derive_kb_client_id(&fp("alice"), 0));
    doc.transact_mut()
        .apply_update(Update::decode_v1(base).unwrap())
        .unwrap();
    let sv = doc.transact().state_vector();
    let root = doc.get_or_insert_map("node");
    root.insert(&mut doc.transact_mut(), "title", "a plain string");
    let out = doc.transact().encode_state_as_update_v1(&sv);
    out
}

fn update_msg(method: &str, update: &[u8]) -> serde_json::Value {
    let b64 = update_to_base64(update);
    let params = if method == "sync/update" {
        serde_json::json!({"doc": node_doc(), "kb_id": KB, "update": b64})
    } else {
        serde_json::json!({"kb_id": KB, "node_id": NODE, "update": b64})
    };
    serde_json::json!({"jsonrpc": "2.0", "id": 9, "method": method, "params": params})
}

async fn title_now(store: &Arc<DocStore>) -> String {
    let state = store.encode_state(&node_doc()).await.unwrap();
    mae_sync::kb::KbNodeDoc::from_bytes(&state).unwrap().title()
}

/// Kills: no check on either path, or a check that applies before refusing.
#[tokio::test]
async fn a_mistyped_node_update_is_refused_on_both_paths_and_applies_nothing() {
    for method in ["kb/node_update", "sync/update"] {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        let owner = fp("alice");
        let r = kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&owner),
            KB,
            "alice",
            &mut docs,
        )
        .await;
        assert!(r.error.is_none(), "{:?}", r.error);

        // A well-typed node, created through the same path.
        let cid = derive_kb_client_id(&owner, 0);
        let good =
            mae_sync::kb::KbNodeDoc::new_with_client_id(NODE, "Real title", "body\n", &[], cid)
                .encode_state();
        let r = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&owner),
            update_msg(method, &good),
            &mut docs,
        )
        .await;
        assert!(
            r.error.is_none(),
            "{method}: a well-typed node is refused: {:?}",
            r.error
        );
        assert_eq!(title_now(&store).await, "Real title");

        let base = store.encode_state(&node_doc()).await.unwrap();
        let bad = mistyped_title(&base);
        let r = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&owner),
            update_msg(method, &bad),
            &mut docs,
        )
        .await;
        let err = r
            .error
            .unwrap_or_else(|| panic!("{method}: a mistyped title was accepted"));
        let data = err.data.expect("kinded");
        assert_eq!(data["kind"], "invalid_field", "{method}");
        assert_eq!(data["field"], "title", "{method}");
        assert_eq!(
            title_now(&store).await,
            "Real title",
            "{method}: something was applied"
        );
    }
}

/// The review's case: a node evicted from memory (idle) is still checked
/// against its stored state, not as a new document. Kills a base taken from
/// memory only, which let a delta overwriting a field pass on a cold node.
#[tokio::test]
async fn an_evicted_node_is_checked_against_its_stored_state() {
    for method in ["kb/node_update", "sync/update"] {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        let owner = fp("alice");
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&owner),
            KB,
            "alice",
            &mut docs,
        )
        .await;
        let cid = derive_kb_client_id(&owner, 0);
        let good =
            mae_sync::kb::KbNodeDoc::new_with_client_id(NODE, "Real title", "body\n", &[], cid)
                .encode_state();
        let r = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&owner),
            update_msg(method, &good),
            &mut docs,
        )
        .await;
        assert!(r.error.is_none(), "{:?}", r.error);
        let base = store.encode_state(&node_doc()).await.unwrap();
        for _ in 0..4 {
            store.track_client_disconnect(&node_doc()).await.unwrap(); // saturates at 0
        }
        let evicted = store.evict_idle(0).await;
        assert!(
            evicted.contains(&node_doc()),
            "precondition: evicted ({evicted:?})"
        );
        assert!(!store.has_doc(&node_doc()).await);

        let r = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&owner),
            update_msg(method, &mistyped_title(&base)),
            &mut docs,
        )
        .await;
        let err = r
            .error
            .unwrap_or_else(|| panic!("{method}: a cold node took a mistyped title"));
        assert_eq!(err.data.unwrap()["field"], "title", "{method}");
        assert_eq!(title_now(&store).await, "Real title", "{method}");
    }
}
