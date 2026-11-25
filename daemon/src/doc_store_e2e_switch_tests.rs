//! The E2E-switch guard at the store (`doc_store_e2e_switch`): what decides
//! "is E2E" from outside the write (the KB's anchor), and what the guard costs
//! on the hot path.

use std::sync::Arc;
use std::time::Instant;

use mae_mcp::identity::Identity;
use mae_sync::kb::{Encryption, KbCollectionDoc, KbNodeDoc};
use mae_sync::membership::{MembershipAction, MembershipOp};

use crate::doc_editors::Attribution;
use crate::doc_store::DocStore;
use crate::storage::{PlaintextHeld, SqliteBackend, StorageError};

const BY: Attribution<'static> = Attribution::Principal("seed");

fn store(signer: &str) -> Arc<DocStore> {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = Arc::new(DocStore::new(backend, 500));
    store.set_signer(Arc::new(Identity::generate(signer)));
    store
}

fn kbc(kb: &str) -> String {
    format!("kbc:{kb}")
}

async fn collection(s: &DocStore, kb: &str) -> KbCollectionDoc {
    KbCollectionDoc::from_bytes(&s.encode_state(&kbc(kb)).await.unwrap()).unwrap()
}

/// Share plaintext KB `kb`, owned by `owner`, with node `n`.
async fn share_kb(s: &DocStore, kb: &str, owner: &Identity) {
    let mut coll = KbCollectionDoc::new_owned(kb, &owner.fingerprint(), "owner");
    let _ = coll.add_node("n", "N");
    s.share_doc(&kbc(kb), &coll.encode_state(), BY)
        .await
        .unwrap();
    let node = KbNodeDoc::new("n", "N", "Body.", &[]).encode_state();
    let name = mae_sync::kb_node_doc_name(kb, "n");
    s.share_doc(&name, &node, BY).await.unwrap();
}

/// Two authors in turn: the second edit's handoff cuts a history version.
async fn give_history(s: &DocStore, kb: &str) {
    let name = mae_sync::kb_node_doc_name(kb, "n");
    for (i, who) in ["alice", "bob"].into_iter().enumerate() {
        let state = s.encode_state(&name).await.unwrap();
        let mut d = KbNodeDoc::from_bytes_with_client_id(&state, 700 + i as u64).unwrap();
        let up = d.set_title(&format!("T{i}"));
        let by = Attribution::Principal(who);
        s.apply_update(&name, &up, None, by).await.unwrap();
    }
    assert!(s.history_holds_content(kb).await.unwrap());
}

/// The owner's switch to E2E as the SIGNED op-log alone: the genesis and the
/// `SetEncryption("e2e")` op, without the unsigned display flag. Read as E2E
/// only by a daemon that knows the KB's anchor.
fn signed_switch_only(coll: &mut KbCollectionDoc, kb: &str, owner: &Identity) -> Vec<u8> {
    let sv = coll.state_vector();
    let (fp, secret, pk) = (
        owner.fingerprint(),
        owner.secret_bytes(),
        owner.public().to_bytes(),
    );
    for (action, subject, role) in [
        (
            MembershipAction::Admit,
            fp.as_str(),
            Some(mae_sync::kb::Role::Owner),
        ),
        (MembershipAction::SetEncryption, "e2e", None),
    ] {
        let op = coll.build_membership_op(kb, action, subject, role, false, &fp, 0, None, 0);
        let sig = op.sign(&secret);
        coll.append_signed_op(&op, &sig, &pk);
    }
    assert!(matches!(coll.encryption(), mae_sync::kb::Encryption::None));
    coll.encode_diff(&sv).unwrap()
}

async fn is_e2e(s: &DocStore, kb: &str) -> bool {
    s.collection_is_e2e(kb, &collection(s, kb).await).await
}

/// Item 1, the order the write guard cannot see: the signed switch is written
/// while the KB's anchor is unknown (so it reads as plaintext and is
/// admitted), and the anchor is registered after. Registering it would make a
/// KB holding plaintext history read as E2E with no write passing the guard,
/// so the registration is refused, on both registration paths, and the KB
/// stays readable as plaintext. With nothing held, the same registration is
/// accepted and the KB reads as E2E.
#[tokio::test]
async fn an_anchor_that_would_switch_a_kb_holding_plaintext_to_e2e_is_refused() {
    for hub_recorded in [true, false] {
        let (s, owner) = (store("daemon"), Identity::generate("owner"));
        share_kb(&s, "kb", &owner).await;
        give_history(&s, "kb").await;
        let mut coll = collection(&s, "kb").await;
        let switch = signed_switch_only(&mut coll, "kb", &owner);
        s.apply_update(&kbc("kb"), &switch, None, BY).await.unwrap();
        assert!(!is_e2e(&s, "kb").await, "unanchored: read as plaintext");

        let pk = owner.public().to_bytes();
        let refused = if hub_recorded {
            match s.record_kb_anchor_if_absent("kb", pk).await {
                Err(StorageError::E2eSwitchRefused { held, .. }) => held,
                other => panic!("hub-recorded anchor: {other:?}"),
            }
        } else {
            match s.set_kb_anchor("kb", pk).await {
                Err(crate::doc_store::AnchorRefused::E2eSwitch(held)) => held,
                other => panic!("joined anchor: {other:?}"),
            }
        };
        assert_eq!(refused, PlaintextHeld::History);
        assert_eq!(s.kb_anchor("kb").await, None, "nothing recorded");
        assert!(!is_e2e(&s, "kb").await, "the KB still reads as plaintext");
        assert_eq!(s.e2e_switches_refused(), 1);
    }

    // Nothing held: the same registration is accepted and switches the KB.
    let (s, owner) = (store("daemon"), Identity::generate("owner"));
    share_kb(&s, "kb", &owner).await;
    let mut coll = collection(&s, "kb").await;
    let switch = signed_switch_only(&mut coll, "kb", &owner);
    s.apply_update(&kbc("kb"), &switch, None, BY).await.unwrap();
    let pk = owner.public().to_bytes();
    assert!(matches!(
        s.record_kb_anchor_if_absent("kb", pk).await,
        Ok(true)
    ));
    assert!(is_e2e(&s, "kb").await);
    assert_eq!(s.e2e_switches_refused(), 0);
}

/// Item 1, the other order: the anchor first (a plaintext KB holding history
/// anchors normally), then the signed switch, which the write guard refuses.
#[tokio::test]
async fn with_the_anchor_known_first_the_signed_switch_is_refused_at_the_write() {
    let (s, owner) = (store("daemon"), Identity::generate("owner"));
    share_kb(&s, "kb", &owner).await;
    give_history(&s, "kb").await;
    let pk = owner.public().to_bytes();
    assert!(matches!(
        s.record_kb_anchor_if_absent("kb", pk).await,
        Ok(true)
    ));
    let mut coll = collection(&s, "kb").await;
    let switch = signed_switch_only(&mut coll, "kb", &owner);
    let r = s.apply_update(&kbc("kb"), &switch, None, BY).await;
    assert!(
        matches!(r, Err(StorageError::E2eSwitchRefused { .. })),
        "{r:?}"
    );
    assert!(!is_e2e(&s, "kb").await);
    assert_eq!(s.kb_anchor("kb").await, Some(pk));
}

/// Item 2 at the store: a client or peer that keeps offering a refused switch
/// is refused every time and counted every time, but warned about once per
/// KB per interval; another KB gets its own warning.
#[tokio::test]
async fn refusals_are_all_counted_and_their_warnings_rate_limited_per_kb() {
    let (s, owner) = (store("daemon"), Identity::generate("owner"));
    for kb in ["kb", "kb2"] {
        share_kb(&s, kb, &owner).await;
        give_history(&s, kb).await;
    }
    for i in 0..5u64 {
        let flip = collection(&s, "kb").await.set_encryption(Encryption::E2e);
        let r = s.apply_update(&kbc("kb"), &flip, None, BY).await;
        assert!(
            matches!(r, Err(StorageError::E2eSwitchRefused { .. })),
            "{r:?}"
        );
        assert_eq!(s.e2e_switches_refused(), i + 1);
    }
    assert_eq!(s.e2e_switch_refusals_logged(), 1, "one warning for five");
    let flip = collection(&s, "kb2").await.set_encryption(Encryption::E2e);
    assert!(s.apply_update(&kbc("kb2"), &flip, None, BY).await.is_err());
    assert_eq!(s.e2e_switch_refusals_logged(), 2, "per KB");
    assert!(!is_e2e(&s, "kb").await && !is_e2e(&s, "kb2").await);
}

/// Item 3's pre-filter on the store: a write that changes nothing the
/// encryption reading depends on is not checked, but the write that
/// integrates a held-pending change to the op-log is. The switch's record is
/// edited before it arrives (accepted, pending); the switch then arrives and
/// is refused, whole.
#[tokio::test]
async fn the_write_that_integrates_a_pending_switch_is_checked() {
    let (s, owner) = (store("daemon"), Identity::generate("owner"));
    share_kb(&s, "kb", &owner).await;
    give_history(&s, "kb").await;
    let mut coll = collection(&s, "kb").await;
    let switch = signed_switch_only(&mut coll, "kb", &owner);
    let late = edit_switch_record(&coll);
    s.apply_update(&kbc("kb"), &late, None, BY).await.unwrap();
    assert_eq!(collection(&s, "kb").await.oplog_ops().len(), 0, "pending");
    let pk = owner.public().to_bytes();
    assert!(matches!(
        s.record_kb_anchor_if_absent("kb", pk).await,
        Ok(true)
    ));
    let r = s.apply_update(&kbc("kb"), &switch, None, BY).await;
    assert!(
        matches!(r, Err(StorageError::E2eSwitchRefused { .. })),
        "{r:?}"
    );
    assert!(!is_e2e(&s, "kb").await);
    assert_eq!(collection(&s, "kb").await.oplog_ops().len(), 0);
}

/// An edit inside the record of `coll`'s `SetEncryption` op: an update that
/// depends on the op, so a store without it holds it pending. (Raw yrs on the
/// collection's own keys, which mae-sync keeps private.)
fn edit_switch_record(coll: &KbCollectionDoc) -> Vec<u8> {
    use mae_sync::yrs::{Map, Out, Transact};
    let head = coll
        .oplog_ops()
        .into_iter()
        .find(|o| o.op.action == MembershipAction::SetEncryption)
        .unwrap()
        .chain_hash();
    let sync = mae_sync::text::TextSync::from_state(&coll.encode_state()).unwrap();
    let root = sync.doc().get_or_insert_map("collection");
    let mut txn = sync.doc().transact_mut();
    let Some(Out::YMap(log)) = root.get(&txn, "membership_oplog") else {
        panic!("no op-log under the collection's keys");
    };
    let Some(Out::YMap(rec)) = log.get(&txn, head.as_str()) else {
        panic!("no record");
    };
    rec.insert(&mut txn, "late", "x");
    txn.encode_update_v1()
}

/// A KB owned by `owner` whose op-log holds `ops` signed entries (a genesis
/// and admits, chained), E2E or not, shared into a fresh store with its
/// anchor recorded. Built without `build_membership_op` past the genesis,
/// which reads the whole log for its head (quadratic at this size).
async fn big_kb(ops: usize, e2e: bool) -> (Arc<DocStore>, KbCollectionDoc) {
    let s = store("daemon");
    let owner = Identity::generate("owner");
    let (fp, secret, pk) = (
        owner.fingerprint(),
        owner.secret_bytes(),
        owner.public().to_bytes(),
    );
    let mut coll = KbCollectionDoc::new_owned("big", &fp, "owner");
    let genesis = coll.build_membership_op(
        "big",
        MembershipAction::Admit,
        &fp,
        Some(mae_sync::kb::Role::Owner),
        true,
        &fp,
        0,
        None,
        0,
    );
    let mut sig = genesis.sign(&secret);
    coll.append_signed_op(&genesis, &sig, &pk);
    let mut prev = genesis.chain_hash(&sig);
    let mut push = |coll: &mut KbCollectionDoc, op: MembershipOp| {
        let mut op = op;
        op.prev_hash = std::mem::take(&mut prev);
        sig = op.sign(&secret);
        coll.append_signed_op(&op, &sig, &pk);
        prev = op.chain_hash(&sig);
    };
    for i in 1..ops {
        let mut op: MembershipOp = genesis.clone();
        op.subject = format!("SHA256:member-{i}");
        op.role = Some(mae_sync::kb::Role::Editor);
        op.can_invite = false;
        push(&mut coll, op);
    }
    if e2e {
        let mut op = genesis.clone();
        op.action = MembershipAction::SetEncryption;
        op.subject = "e2e".into();
        op.role = None;
        push(&mut coll, op);
        coll.set_encryption(mae_sync::kb::Encryption::E2e);
    }
    s.share_doc(&kbc("big"), &coll.encode_state(), BY)
        .await
        .unwrap();
    assert!(matches!(
        s.record_kb_anchor_if_absent("big", pk).await,
        Ok(true)
    ));
    (s, coll)
}

/// Item 3's measurement: the mean cost of one ordinary collection write (a
/// node added to the manifest) through `apply_update`, on a plaintext and an
/// E2E KB at 1,000 and 50,000 op-log entries. Run with
/// `cargo test -p mae-daemon --release --lib e2e_switch_cost -- --ignored --nocapture`.
#[tokio::test]
#[ignore = "benchmark: prints timings, asserts nothing about them"]
async fn e2e_switch_cost() {
    for (ops, writes) in [(1_000usize, 20usize), (50_000, 5)] {
        for e2e in [false, true] {
            let (s, mut coll) = big_kb(ops, e2e).await;
            let started = Instant::now();
            for i in 0..writes {
                let up = coll.add_node(&format!("bench-{i}"), "B");
                s.apply_update(&kbc("big"), &up, None, BY).await.unwrap();
            }
            let mean = started.elapsed() / writes as u32;
            println!("e2e_switch_cost ops={ops} e2e={e2e} writes={writes} mean_per_write={mean:?}");
        }
    }
}
