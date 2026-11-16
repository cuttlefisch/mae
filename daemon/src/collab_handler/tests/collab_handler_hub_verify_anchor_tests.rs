//! ADR-036 Amendment 1 F1: a KB's anchor is the key of the session that first
//! shared it, recorded durably and once; a KB shared before is anchored by its
//! owner's key at its next share or write, or by the operator — and by no key
//! whose fingerprint the collection does not name as owner.

use super::collab_handler_hub_verify_tests::{hv, key, op, write, Method, KB};
use super::*;

/// A KB shared now carries its sharer's key as anchor, and still does after the
/// store is closed and opened again.
#[tokio::test]
async fn a_kb_shared_now_is_anchored_to_its_sharer_durably() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let owner = key("owner");
    {
        let store = Arc::new(DocStore::new(
            Arc::new(SqliteBackend::open(&path).unwrap()),
            500,
        ));
        let coll = KbCollectionDoc::new_owned(KB, "", "owner");
        let share = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {"kb_id": KB, "name": KB, "nodes": [],
                       "collection_state": update_to_base64(&coll.encode_state())}});
        let (bc, mut docs) = (test_broadcaster(), HashSet::new());
        let r = dispatch_session(&store, &bc, owner.mtls(), share, &mut docs).await;
        assert!(r.error.is_none(), "share: {:?}", r.error);
        assert_eq!(store.kb_anchor(KB).await, Some(owner.pk));
        // A mesh join's anchor, for the provenance half of the round trip.
        store.set_kb_anchor("kb-joined", owner.pk).await.unwrap();
    }
    let reopened = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
    assert_eq!(reopened.kb_anchor(KB).await, None, "not yet hydrated");
    reopened.load_local_state().await;
    let hub = Some((owner.pk, crate::storage::AnchorSource::HubRecorded));
    let joined = Some((owner.pk, crate::storage::AnchorSource::Joined));
    assert_eq!(
        reopened.kb_anchor_record(KB).await,
        hub,
        "key and source survive"
    );
    assert_eq!(reopened.kb_anchor_record("kb-joined").await, joined);
}

/// A KB shared before (no key at share) is not anchored by a member's key, nor
/// by a session naming the owner's principal with another key; the owner's
/// own key anchors it at its next write.
#[tokio::test]
async fn a_kb_shared_before_is_anchored_by_its_owners_key_and_no_other() {
    let (mut h, k) = hv(false, false, false).await;
    assert_eq!(
        h.store.kb_anchor(KB).await,
        None,
        "precondition: shared without a key"
    );
    let epoch = h.epoch(&k.bob).await;
    let (upd, signed) = op(&k.bob, epoch, "a member writes");
    let r = h
        .call(
            k.bob.mtls(),
            write(Method::SyncUpdate, &upd, Some(signed.header_params())),
        )
        .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        h.store.kb_anchor(KB).await,
        None,
        "a member's key is not the owner's"
    );
    let impostor = (Some(k.owner.fp.as_str()), Some(&k.carol.pk), Transport::Hub);
    let (upd, _) = op(&k.owner, 0, "owner's principal, carol's key");
    let _ = h
        .call(impostor, write(Method::NodeUpdate, &upd, None))
        .await;
    assert_eq!(
        h.store.kb_anchor(KB).await,
        None,
        "a key not hashing to the owner"
    );
    let (upd, _) = op(&k.owner, 0, "the owner writes");
    let r = h
        .call(k.owner.mtls(), write(Method::NodeUpdate, &upd, None))
        .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        h.store.kb_anchor(KB).await,
        Some(k.owner.pk),
        "the owner's next write"
    );
}

/// The owner re-sharing a KB shared before anchors it; `sync/update` does too.
#[tokio::test]
async fn an_owners_reshare_or_sync_update_anchors_a_kb_shared_before() {
    let (mut h, k) = hv(false, false, false).await;
    let coll = KbCollectionDoc::new_owned(KB, "", "owner");
    let share = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {"kb_id": KB, "name": KB, "nodes": [],
                   "collection_state": update_to_base64(&coll.encode_state())}});
    assert!(h.call(k.owner.mtls(), share).await.error.is_none());
    assert_eq!(h.store.kb_anchor(KB).await, Some(k.owner.pk), "re-share");

    let (mut h, k) = hv(false, false, false).await;
    let (upd, _) = op(&k.owner, 0, "owner over sync/update");
    let r = h
        .call(k.owner.mtls(), write(Method::SyncUpdate, &upd, None))
        .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(h.store.kb_anchor(KB).await, Some(k.owner.pk), "sync/update");
}

/// A recorded anchor is stored once: the owner's connection never replaces it.
#[tokio::test]
async fn an_anchor_is_never_replaced_by_the_owners_connection() {
    let (mut h, k) = hv(false, false, false).await;
    let other = key("joined-from");
    h.store.set_kb_anchor(KB, other.pk).await.unwrap();
    let (upd, _) = op(&k.owner, 0, "owner writes");
    let _ = h
        .call(k.owner.mtls(), write(Method::NodeUpdate, &upd, None))
        .await;
    assert_eq!(h.store.kb_anchor(KB).await, Some(other.pk));
    let recorded = h.store.record_kb_anchor_if_absent(KB, k.owner.pk).await;
    assert_eq!(recorded.ok(), Some(false), "the store itself records once");
    assert_eq!(h.store.kb_anchor(KB).await, Some(other.pk));
}

/// `mae-daemon kb anchor`: only the owner's key, once.
#[tokio::test]
async fn the_operator_anchors_only_with_the_owners_key_and_only_once() {
    let (h, k) = hv(false, false, false).await;
    let refused = operator_anchor_kb(&h.store, KB, k.bob.pk).await;
    assert!(refused.is_err(), "a member's key: {refused:?}");
    assert_eq!(h.store.kb_anchor(KB).await, None);
    let missing = operator_anchor_kb(&h.store, "no-such-kb", k.owner.pk).await;
    assert!(missing.is_err(), "an unknown KB: {missing:?}");
    let done = operator_anchor_kb(&h.store, KB, k.owner.pk).await;
    assert_eq!(
        done,
        Ok(OperatorAnchor::Recorded {
            roster_only: vec![]
        })
    );
    assert_eq!(h.store.kb_anchor(KB).await, Some(k.owner.pk));
    let again = operator_anchor_kb(&h.store, KB, k.owner.pk).await;
    assert_eq!(again, Ok(OperatorAnchor::AlreadyAnchored));

    let (h, k) = hv(false, false, false).await;
    h.store.set_kb_anchor(KB, k.carol.pk).await.unwrap();
    let replaced = operator_anchor_kb(&h.store, KB, k.owner.pk).await;
    assert!(replaced.is_err(), "anchored to another key: {replaced:?}");
    assert_eq!(h.store.kb_anchor(KB).await, Some(k.carol.pk));
}
