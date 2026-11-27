//! ADR-036 Amendment 2 (#727 part 2): a change to a KB's node list is a signed
//! manifest op. Through the real dispatcher, with freshly generated keys: G1's
//! verification, G2's switch, and G3's refusal of a raw write that changes the
//! node list.

use super::collab_handler_error_kind_tests::expect_kind;
use super::collab_handler_hub_verify_tests::{hv, key, Key, KB};
use super::*;
use mae_sync::content_ops::ContentOp;
use mae_sync::manifest_op::{ManifestAction, ManifestOp};

/// `kb/collection_node_add|remove` of `node` (`title` on an add), with the
/// header `author` signs over `signed` (the fields as signed, which a test may
/// make differ from the request's), or unsigned.
fn node_msg(
    add: bool,
    node: &str,
    title: &str,
    header: Option<serde_json::Value>,
) -> serde_json::Value {
    let mut params = serde_json::json!({"kb_id": KB, "node_id": node, "title": title});
    if let Some(h) = header {
        for (k, v) in h.as_object().unwrap() {
            params[k] = v.clone();
        }
    }
    let method = if add {
        "kb/collection_node_add"
    } else {
        "kb/collection_node_remove"
    };
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
}

fn signed(author: &Key, op: &ManifestOp) -> serde_json::Value {
    serde_json::json!({
        "author": op.author, "epoch": op.epoch, "issued_at": op.issued_at,
        "author_pubkey": update_to_base64(&author.pk),
        "sig": update_to_base64(&op.sign(&author.id.secret_bytes())),
    })
}

fn manifest(author: &Key, node: &str, add: bool, title: &str, epoch: u64) -> ManifestOp {
    ManifestOp {
        kb_id: KB.to_string(),
        node_id: node.to_string(),
        action: if add {
            ManifestAction::Add
        } else {
            ManifestAction::Remove
        },
        title: if add {
            title.to_string()
        } else {
            String::new()
        },
        author: author.fp.clone(),
        epoch,
        issued_at: 1_700_000_000,
    }
}

async fn listed(store: &DocStore) -> Vec<(String, String)> {
    let state = store.encode_state(&format!("kbc:{KB}")).await.unwrap();
    let mut l = KbCollectionDoc::from_bytes(&state).unwrap().list_nodes();
    l.sort();
    l
}

/// G1, G2: with the switch on, an editor's signed add and remove apply, and
/// the same change unsigned is refused. Kills a switch that skips the
/// manifest path.
#[tokio::test]
async fn with_the_switch_on_a_node_list_change_must_be_signed() {
    let (mut h, k) = hv(true, false, true).await;
    let epoch = h.epoch(&k.bob).await;
    for session in [k.bob.mtls(), k.bob.bearer()] {
        let r = h
            .call(session, node_msg(true, "concept:u", "Unsigned", None))
            .await;
        expect_kind(&r, "signature_required", "unsigned add");
        let add = manifest(&k.bob, "concept:a", true, "A", epoch);
        let r = h
            .call(
                session,
                node_msg(true, "concept:a", "A", Some(signed(&k.bob, &add))),
            )
            .await;
        assert!(r.error.is_none(), "signed add: {:?}", r.error);
        assert!(listed(&h.store)
            .await
            .contains(&("concept:a".into(), "A".into())));
        let r = h
            .call(session, node_msg(false, "concept:a", "", None))
            .await;
        expect_kind(&r, "signature_required", "unsigned remove");
        let rm = manifest(&k.bob, "concept:a", false, "", epoch);
        let r = h
            .call(
                session,
                node_msg(false, "concept:a", "", Some(signed(&k.bob, &rm))),
            )
            .await;
        assert!(r.error.is_none(), "signed remove: {:?}", r.error);
        assert!(!listed(&h.store)
            .await
            .iter()
            .any(|(id, _)| id == "concept:a"));
    }
    assert_eq!(h.store.content_op_policy().unsigned_accepted(), 0);
}

/// G2: with the switch off, an unsigned change is accepted and counted with
/// the content ops. Kills a manifest path that is never counted.
#[tokio::test]
async fn with_the_switch_off_an_unsigned_change_is_counted() {
    let (mut h, k) = hv(false, false, true).await;
    let before = h.store.content_op_policy().unsigned_accepted();
    let r = h
        .call(k.bob.mtls(), node_msg(true, "concept:u", "U", None))
        .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(h.store.content_op_policy().unsigned_accepted(), before + 1);
}

/// G1's checks, each alone, at any setting: another session's op (F4), a
/// changed field, a content-op signature presented as a manifest op, a stale
/// epoch. Kills each check of `verify_manifest_op`.
#[tokio::test]
async fn a_signed_change_must_be_the_sessions_exact_and_current() {
    let (mut h, k) = hv(false, false, true).await;
    let epoch = h.epoch(&k.bob).await;
    let add = manifest(&k.bob, "concept:b", true, "Bob's", epoch);
    let header = signed(&k.bob, &add);
    let r = h
        .call(
            k.carol.mtls(),
            node_msg(true, "concept:b", "Bob's", Some(header.clone())),
        )
        .await;
    expect_kind(&r, "not_author", "carol sends bob's op");
    let r = h
        .call(
            k.bob.mtls(),
            node_msg(true, "concept:b", "Renamed", Some(header.clone())),
        )
        .await;
    expect_kind(&r, "signature_invalid", "a changed title");
    let r = h
        .call(
            k.bob.mtls(),
            node_msg(true, "concept:other", "Bob's", Some(header)),
        )
        .await;
    expect_kind(&r, "signature_invalid", "another node");

    let content = ContentOp {
        kb_id: KB.into(),
        node_id: "concept:b".into(),
        base_sv: vec![],
        author: k.bob.fp.clone(),
        epoch,
        issued_at: 1_700_000_000,
    };
    let mut forged = signed(&k.bob, &add);
    forged["sig"] = serde_json::json!(update_to_base64(
        &content.sign(&k.bob.id.secret_bytes(), b"")
    ));
    let r = h
        .call(
            k.bob.mtls(),
            node_msg(true, "concept:b", "Bob's", Some(forged)),
        )
        .await;
    expect_kind(&r, "signature_invalid", "a content-op signature");

    let stale = manifest(&k.bob, "concept:b", true, "Bob's", epoch.wrapping_add(1));
    let r = h
        .call(
            k.bob.mtls(),
            node_msg(true, "concept:b", "Bob's", Some(signed(&k.bob, &stale))),
        )
        .await;
    expect_kind(&r, "epoch_fenced", "another epoch");
    assert!(!listed(&h.store)
        .await
        .iter()
        .any(|(id, _)| id == "concept:b"));
}

/// G3: with the switch on, a raw collection write by the owner that adds or
/// removes a node is refused and changes nothing, on `sync/update`,
/// `sync/share` and `kb/collection_op`; a raw write that leaves the node list
/// alone is accepted. Kills each of the three guards and one that refuses
/// every raw write.
#[tokio::test]
async fn with_the_switch_on_a_raw_write_may_not_change_the_node_list() {
    let (mut h, k) = hv(true, false, true).await;
    let kbc = format!("kbc:{KB}");
    let state = h.store.encode_state(&kbc).await.unwrap();
    let mut coll = KbCollectionDoc::from_bytes(&state).unwrap();
    let added = coll.add_node("concept:raw", "Raw");
    let mut whole = KbCollectionDoc::from_bytes(&state).unwrap();
    let _ = whole.add_node("concept:raw", "Raw");
    let msgs = [
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "sync/update",
            "params": {"doc": kbc, "kb_id": KB, "update": update_to_base64(&added)}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "kb/collection_op",
            "params": {"kb_id": KB, "update": update_to_base64(&added)}}),
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "sync/share",
            "params": {"doc": kbc, "update": update_to_base64(&whole.encode_state())}}),
    ];
    for m in msgs {
        let label = m["method"].as_str().unwrap().to_string();
        let r = h.call(k.owner.mtls(), m).await;
        // `sync/share` never reaches a KB document at all (method_authz).
        let want = if label == "sync/share" {
            "doc_unavailable"
        } else {
            "signature_required"
        };
        expect_kind(&r, want, &label);
        assert!(
            !listed(&h.store)
                .await
                .iter()
                .any(|(id, _)| id == "concept:raw"),
            "{label}"
        );
    }
    let mut plain = KbCollectionDoc::from_bytes(&state).unwrap();
    let policy = plain.set_transport_policy(mae_sync::kb::TransportPolicy::Hub);
    let r = h
        .call(
            k.owner.mtls(),
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "kb/collection_op",
                "params": {"kb_id": KB, "update": update_to_base64(&policy)}}),
        )
        .await;
    assert!(
        r.error.is_none(),
        "a raw write leaving the node list alone: {:?}",
        r.error
    );
}

/// G3 against a split update (the independent review's attack): the
/// node-list change goes first, its origin (an earlier entry for the same
/// key) missing, so yrs holds it pending and the list does not change; then
/// the update holding that origin, which on its own adds and removes the
/// entry and so changes nothing either. The second write integrates the held
/// change, so it is refused, and the node never appears. Kills a guard that
/// judges anything but the document the write leaves.
#[tokio::test]
async fn a_node_list_change_held_pending_is_refused_when_it_lands() {
    let (mut h, k) = hv(true, false, true).await;
    let kbc = format!("kbc:{KB}");
    let state = h.store.encode_state(&kbc).await.unwrap();
    let mut attacker = KbCollectionDoc::from_bytes(&state).unwrap();
    let sv = attacker.state_vector();
    let _ = attacker.add_node("concept:split", "Gone");
    let _ = attacker.remove_node("concept:split");
    let releaser = attacker.encode_diff(&sv).unwrap();
    let change = attacker.add_node("concept:split", "Split");
    let send = |update: &[u8]| {
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "sync/update",
            "params": {"doc": kbc, "kb_id": KB, "update": update_to_base64(update)}})
    };
    let listed_split = |l: Vec<(String, String)>| l.iter().any(|(id, _)| id == "concept:split");
    let first = h.call(k.owner.mtls(), send(&change)).await;
    assert!(
        first.error.is_none(),
        "held pending, it changes nothing yet: {:?}",
        first.error
    );
    assert!(
        !listed_split(listed(&h.store).await),
        "the change was not held back"
    );
    let second = h.call(k.owner.mtls(), send(&releaser)).await;
    expect_kind(&second, "signature_required", "the write that lands it");
    assert!(
        !listed_split(listed(&h.store).await),
        "the held-back node landed"
    );
    // Another member's signed add still applies beside what is held.
    let epoch = h.epoch(&k.bob).await;
    let add = manifest(&k.bob, "concept:ok", true, "Ok", epoch);
    let r = h
        .call(
            k.bob.mtls(),
            node_msg(true, "concept:ok", "Ok", Some(signed(&k.bob, &add))),
        )
        .await;
    assert!(r.error.is_none(), "signed add after: {:?}", r.error);
    assert!(
        !listed_split(listed(&h.store).await),
        "a later write landed it"
    );
}

/// The key a test session would use is real; a sanity check that the
/// helpers sign what the verifier checks.
#[test]
fn the_helper_signs_what_the_verifier_checks() {
    let a = key("a");
    let op = manifest(&a, "concept:x", true, "X", 3);
    let h = signed(&a, &op);
    let sig = mae_sync::encoding::base64_to_update(h["sig"].as_str().unwrap()).unwrap();
    assert!(op.verify(&sig, &a.pk));
}

/// A member removed while connected stops receiving the KB's changes on the
/// sessions they already have open; another member's session keeps them.
/// Before, the removed member's open connection kept receiving until it
/// disconnected. Kills the removal not cutting subscriptions off.
#[tokio::test]
async fn a_removed_members_open_sessions_stop_receiving_the_kb() {
    let (mut h, k) = hv(false, false, true).await;
    let node = mae_sync::kb_node_doc_name(KB, "concept:shared");
    let (bob_sid, carol_sid) = (601, 602);
    let mut bob_rx = register_session(&h.bc, bob_sid, Some(&k.bob.fp));
    let mut carol_rx = register_session(&h.bc, carol_sid, Some(&k.carol.fp));
    {
        let mut b = h.bc.lock().unwrap();
        for sid in [bob_sid, carol_sid] {
            b.add_event_sub(sid, "sync_update");
            b.subscribe_doc(sid, &node);
        }
    }
    let rm = kb_member_msg("kb/remove_member", KB, &k.bob.fp, None);
    let r = h.call(k.owner.bearer(), rm).await;
    assert!(r.error.is_none(), "{:?}", r.error);
    while bob_rx.try_recv().is_ok() {}
    while carol_rx.try_recv().is_ok() {}
    h.bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
        buffer_name: node.clone(),
        update_base64: "AA==".into(),
        wal_seq: 1,
        content_header: None,
    });
    assert!(bob_rx.try_recv().is_err(), "the removed member is cut off");
    assert!(carol_rx.try_recv().is_ok(), "a member still receives");
}
