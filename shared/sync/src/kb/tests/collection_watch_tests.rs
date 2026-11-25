//! `KbCollectionDoc::watch_encryption_inputs` (mirrors `collection_watch.rs`):
//! a store skips reading the collection's encryption mode when the watch is
//! not touched, so every way a write can change that mode must touch it, and
//! an ordinary write must not.

use super::*;
use crate::membership::MembershipAction;
use yrs::updates::decoder::Decode;
use yrs::{Map, Transact};

/// The owner's collection, its signed genesis included, and a replica of it.
fn pair() -> (
    KbCollectionDoc,
    KbCollectionDoc,
    ([u8; 32], [u8; 32], String),
) {
    let keys = oplog_keypair(41);
    let (secret, pubkey, fp) = &keys;
    let mut owner = KbCollectionDoc::new_owned("kb", fp, "owner");
    let g = owner.build_membership_op(
        "kb",
        MembershipAction::Admit,
        fp,
        Some(Role::Owner),
        true,
        fp,
        0,
        None,
        0,
    );
    let sig = g.sign(secret);
    owner.append_signed_op(&g, &sig, pubkey);
    let replica = KbCollectionDoc::from_bytes(&owner.encode_state()).unwrap();
    (owner, replica, keys)
}

/// Apply `update` to `replica` under a fresh watch; whether it was touched.
fn touched_by(replica: &KbCollectionDoc, update: &[u8]) -> bool {
    let watch = KbCollectionDoc::watch_encryption_inputs(&replica.doc);
    let mut txn = replica.doc.transact_mut();
    txn.apply_update(yrs::Update::decode_v1(update).unwrap())
        .unwrap();
    drop(txn);
    watch.touched()
}

fn signed(
    owner: &mut KbCollectionDoc,
    (secret, pubkey, fp): &([u8; 32], [u8; 32], String),
    action: MembershipAction,
    subject: &str,
) -> Vec<u8> {
    let op = owner.build_membership_op("kb", action, subject, None, false, fp, 0, None, 0);
    let sig = op.sign(secret);
    owner.append_signed_op(&op, &sig, pubkey)
}

#[test]
fn ordinary_collection_writes_do_not_touch_the_encryption_inputs() {
    let (mut owner, replica, _) = pair();
    let ordinary = [
        owner.add_node("n1", "One"),
        owner.upsert_member("SHA256:bob", "bob", Role::Editor),
        owner.set_transport_policy(TransportPolicy::Hub),
        owner.remove_node("n1"),
    ];
    for (i, up) in ordinary.iter().enumerate() {
        assert!(!touched_by(&replica, up), "write {i} touched the inputs");
    }
}

#[test]
fn every_change_to_an_encryption_input_touches_the_watch() {
    // A signed op appended to the log (here the switch itself).
    let (mut owner, replica, keys) = pair();
    let up = signed(&mut owner, &keys, MembershipAction::SetEncryption, "e2e");
    assert!(touched_by(&replica, &up), "op appended");

    // The display flag.
    let up = owner.set_encryption(Encryption::E2e);
    assert!(touched_by(&replica, &up), "flag set");

    // The owner (whose key a store owning the KB anchors with).
    let up = {
        let root = owner.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = owner.doc.transact_mut();
        root.insert(&mut txn, COLL_OWNER_KEY, "SHA256:someone-else");
        txn.encode_update_v1()
    };
    assert!(touched_by(&replica, &up), "owner replaced");

    // An op deleted from the log, and a field inside an op's record.
    let first = owner.oplog_ops()[0].chain_hash();
    let edit = |key: Option<&str>| {
        let root = owner.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = owner.doc.transact_mut();
        let Some(yrs::Out::YMap(log)) = root.get(&txn, COLL_OPLOG_KEY) else {
            panic!("no op-log");
        };
        match key {
            Some(k) => {
                let Some(yrs::Out::YMap(rec)) = log.get(&txn, first.as_str()) else {
                    panic!("no record");
                };
                rec.insert(&mut txn, k, "x");
            }
            None => {
                log.remove(&mut txn, first.as_str());
            }
        }
        txn.encode_update_v1()
    };
    let up = edit(Some(OP_SUBJECT_KEY));
    assert!(touched_by(&replica, &up), "field inside a record");
    let up = edit(None);
    assert!(touched_by(&replica, &up), "op deleted");
}

/// Blocks held pending: a change inside an op's record that arrives before the
/// op. It integrates nothing (its parent is missing), so the watch is
/// untouched and the log unchanged; the op's own update then integrates both,
/// and the watch reports that write. (This yrs integrates a block whose only
/// gap is in its client's clock, so only a missing origin or parent pends.)
#[test]
fn a_pending_change_is_reported_by_the_write_that_integrates_it() {
    let (mut owner, replica, keys) = pair();
    let first = signed(&mut owner, &keys, MembershipAction::SetEncryption, "e2e");
    let head = owner
        .oplog_ops()
        .into_iter()
        .find(|o| o.op.action == MembershipAction::SetEncryption)
        .unwrap()
        .chain_hash();
    let second = {
        let root = owner.doc.get_or_insert_map(COLLECTION_MAP);
        let mut txn = owner.doc.transact_mut();
        let Some(yrs::Out::YMap(log)) = root.get(&txn, COLL_OPLOG_KEY) else {
            panic!("no op-log");
        };
        let Some(yrs::Out::YMap(rec)) = log.get(&txn, head.as_str()) else {
            panic!("no record");
        };
        rec.insert(&mut txn, "late", "x");
        txn.encode_update_v1()
    };
    let t = touched_by(&replica, &second);
    assert_eq!(replica.oplog_ops().len(), 1, "held pending");
    assert!(!t, "pending: nothing integrated");
    assert!(touched_by(&replica, &first), "the integrating write");
    assert_eq!(replica.oplog_ops().len(), 2);
    assert_eq!(replica.encode_state().len(), owner.encode_state().len());
}
