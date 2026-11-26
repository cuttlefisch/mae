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
