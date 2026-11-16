//! ADR-036 Amendment 1 F2, "verified genesis or fallback": a KB's membership
//! is read from its signed op-log only when that log is rooted at a genesis
//! its anchor signed; otherwise from its member list. No member op can make a
//! KB lose its members.

use super::collab_handler_error_kind_tests::expect_kind;
use super::collab_handler_hub_verify_tests::{hv, key, op, write, Hv, Key, Keys, Method, KB};
use super::*;

/// Every member — owner included — still writes: the owner unsigned, Bob and
/// Carol with their own signed ops, through `kb_access` and op admission alike.
async fn everyone_still_writes(h: &mut Hv, k: &Keys, ctx: &str) {
    let (upd, _) = op(&k.owner, 0, &format!("owner {ctx}"));
    let r = h
        .call(k.owner.mtls(), write(Method::NodeUpdate, &upd, None))
        .await;
    assert!(r.error.is_none(), "{ctx}: the owner writes: {:?}", r.error);
    for who in [&k.bob, &k.carol] {
        let epoch = h.epoch(who).await;
        let (upd, signed) = op(who, epoch, &format!("{} {ctx}", who.fp));
        let msg = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
        let r = h.call(who.mtls(), msg).await;
        assert!(r.error.is_none(), "{ctx}: {} writes: {:?}", who.fp, r.error);
    }
}

/// Apply `update` to the KB's collection as raw state (no handler gate), as a
/// relay or an old daemon could have left it.
pub(super) async fn raw_collection(h: &Hv, update: &[u8]) {
    let doc = format!("kbc:{KB}");
    h.store
        .apply_update(&doc, update, None, TEST_BY)
        .await
        .unwrap();
}

/// A member's own self-`Rebind` on an anchored KB with no genesis — accepted
/// by the self-service gate — leaves the log headed by a record no anchor
/// signed. Every member keeps access; nobody is locked out.
#[tokio::test]
async fn a_log_starting_with_a_members_rebind_keeps_every_member() {
    let (mut h, k) = hv(false, false, true).await;
    let successor = rotor_keys(91);
    let bob_secret = k.bob.id.secret_bytes();
    let mut coll = load_coll(&h.store, KB).await;
    let (new_pk, new_fp, new_wrap) = (successor.1, successor.2.clone(), successor.3);
    let rebind = coll.author_rebind(
        KB,
        &k.bob.fp,
        &new_fp,
        &new_pk,
        &new_wrap,
        &bob_secret,
        &k.bob.pk,
        1000,
    );
    let r = h
        .call(k.bob.mtls(), kb_collection_op_msg(KB, &rebind))
        .await;
    assert!(
        r.error.is_none(),
        "bob's self-rebind is accepted: {:?}",
        r.error
    );
    let coll = load_coll(&h.store, KB).await;
    assert!(
        coll.oplog_head().is_some(),
        "precondition: the KB now has a log"
    );
    assert_eq!(
        h.store.kb_anchor(KB).await,
        Some(k.owner.pk),
        "precondition: anchored"
    );
    everyone_still_writes(&mut h, &k, "after a member's rebind").await;
}

/// A log rooted at the anchor's genesis is the source: a member list entry the
/// signed log never admitted grants nothing.
#[tokio::test]
async fn a_log_with_the_anchors_genesis_is_the_source() {
    let (mut h, k) = hv(false, true, true).await;
    let dave = key("dave");
    let mut coll = load_coll(&h.store, KB).await;
    raw_collection(&h, &coll.upsert_member(&dave.fp, "dave", SyncRole::Editor)).await;
    assert_eq!(
        load_coll(&h.store, KB).await.role_of(&dave.fp),
        Some(SyncRole::Editor)
    );
    let (upd, _) = op(&dave, 0, "dave on the member list only");
    let r = h
        .call(dave.mtls(), write(Method::NodeUpdate, &upd, None))
        .await;
    expect_kind(
        &r,
        "not_member",
        "a member-list-only principal on a signed KB",
    );
    everyone_still_writes(&mut h, &k, "signed KB").await;
}

/// Append a membership op `action` on `subject`, authored and signed by
/// `signer`, after whatever the log's head is now.
pub(super) async fn append(h: &Hv, signer: &Key, action: MembershipAction, subject: &str) {
    let mut coll = load_coll(&h.store, KB).await;
    let role = Some(SyncRole::Owner);
    let o = coll.build_membership_op(KB, action, subject, role, true, &signer.fp, 0, None, 0);
    let sig = o.sign(&signer.id.secret_bytes());
    raw_collection(h, &coll.append_signed_op(&o, &sig, &signer.pk)).await;
}

/// The owner's self-admit genesis, carrying the owner's key but signed by
/// `forger` — what a relay that can append to the log could add.
async fn forged_genesis(h: &Hv, owner: &Key, forger: &Key) {
    let mut coll = load_coll(&h.store, KB).await;
    let (admit, role) = (MembershipAction::Admit, Some(SyncRole::Owner));
    let o = coll.build_membership_op(KB, admit, &owner.fp, role, true, &owner.fp, 0, None, 0);
    let sig = o.sign(&forger.id.secret_bytes());
    raw_collection(h, &coll.append_signed_op(&o, &sig, &owner.pk)).await;
}

/// Logs with no root the anchor signed fall back to the member list, which
/// still holds every member — and not a foreign root's signer:
/// - a genesis another key signed;
/// - that, followed by the owner's own self-admit (signed by the anchor, but
///   not a root: it has a predecessor);
/// - a root the anchor signed that admits someone else (not a self-admit);
/// - a root the anchor signed for itself that is not an admit;
/// - a genesis naming the anchor's key whose signature another key made.
#[tokio::test]
async fn a_log_whose_root_the_anchor_did_not_sign_as_genesis_falls_back() {
    let mallory = key("mallory");
    for case in [
        "foreign genesis",
        "owner self-admit not at the root",
        "owner admits another",
        "owner's recovery-key registration",
        "forged genesis naming the anchor's key",
    ] {
        let (mut h, k) = hv(false, false, true).await;
        match case {
            "foreign genesis" => append(&h, &mallory, MembershipAction::Admit, &mallory.fp).await,
            "owner self-admit not at the root" => {
                append(&h, &mallory, MembershipAction::Admit, &mallory.fp).await;
                append(&h, &k.owner, MembershipAction::Admit, &k.owner.fp).await;
            }
            "forged genesis naming the anchor's key" => {
                forged_genesis(&h, &k.owner, &mallory).await
            }
            "owner admits another" => {
                append(&h, &k.owner, MembershipAction::Admit, &mallory.fp).await
            }
            _ => {
                append(
                    &h,
                    &k.owner,
                    MembershipAction::RegisterRecoveryKey,
                    &k.owner.fp,
                )
                .await
            }
        }
        let coll = load_coll(&h.store, KB).await;
        assert!(coll.oplog_head().is_some(), "{case}: precondition: a log");
        everyone_still_writes(&mut h, &k, case).await;
        let (upd, _) = op(&mallory, 0, case);
        let r = h
            .call(mallory.mtls(), write(Method::NodeUpdate, &upd, None))
            .await;
        expect_kind(&r, "not_member", &format!("{case}: mallory"));
    }
}

/// The owner's own key rotation on a KB with no genesis: its `Rebind` is the
/// log's root and the anchor signed it, but it is not a genesis — the KB keeps
/// its members (#265's roster mirror) instead of deriving an empty set.
#[tokio::test]
async fn an_owner_rebind_as_the_first_record_keeps_every_member() {
    let (mut h, k) = hv(false, false, true).await;
    let successor = rotor_keys(92);
    let mut coll = load_coll(&h.store, KB).await;
    let (new_pk, new_fp, new_wrap) = (successor.1, successor.2.clone(), successor.3);
    let secret = k.owner.id.secret_bytes();
    let rebind = coll.author_rebind(
        KB,
        &k.owner.fp,
        &new_fp,
        &new_pk,
        &new_wrap,
        &secret,
        &k.owner.pk,
        1000,
    );
    let r = h
        .call(k.owner.mtls(), kb_collection_op_msg(KB, &rebind))
        .await;
    assert!(
        r.error.is_none(),
        "the owner's rebind is accepted: {:?}",
        r.error
    );
    assert!(
        load_coll(&h.store, KB).await.oplog_head().is_some(),
        "precondition: a log"
    );
    everyone_still_writes(&mut h, &k, "after the owner's rebind").await;
}

/// A JOINED KB fails closed (ADR-026): without a genesis its anchor signed, its
/// log yields no members, and the member list a relay supplied is never used —
/// with a foreign genesis or with no log at all. The same KB anchored by this
/// hub instead keeps every member.
#[tokio::test]
async fn a_joined_kb_without_the_anchors_genesis_has_no_members() {
    let mallory = key("mallory");
    for (case, joined) in [
        ("foreign genesis", true),
        ("no log", true),
        ("hub-recorded", false),
    ] {
        let (mut h, k) = hv(false, false, false).await;
        if joined {
            h.store.set_kb_anchor(KB, k.owner.pk).await.unwrap();
        } else {
            let recorded = h.store.record_kb_anchor_if_absent(KB, k.owner.pk).await;
            assert_eq!(recorded.ok(), Some(true));
        }
        if case != "no log" {
            append(&h, &mallory, MembershipAction::Admit, &mallory.fp).await;
        }
        let coll = load_coll(&h.store, KB).await;
        assert_eq!(
            coll.role_of(&k.bob.fp),
            Some(SyncRole::Editor),
            "{case}: bob is listed"
        );
        if !joined {
            everyone_still_writes(&mut h, &k, case).await;
            continue;
        }
        for who in [&k.owner, &k.bob] {
            let (upd, _) = op(who, 0, case);
            let r = h
                .call(who.mtls(), write(Method::NodeUpdate, &upd, None))
                .await;
            expect_kind(
                &r,
                "not_member",
                &format!("{case}: {} on a joined KB", who.fp),
            );
        }
    }
}
