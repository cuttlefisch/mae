//! ADR-036 Amendment 1 F2, the owner's ruling "log wins, made visible": where a
//! KB's signed op-log governs membership, a member-list entry the log does not
//! admit has no access — and is never lost silently. It is reported when the
//! anchor is recorded, by `mae-daemon doctor` and by `kb anchor`, and no writer
//! adds or removes a member on the member list alone where the log governs.
//!
//! Also here: a join ticket never replaces an anchor (F1), a session without
//! identity skips only F4, and the epoch fence and an owner's key rotation on
//! the anchored, log-governed path.

use super::collab_handler_error_kind_tests::expect_kind;
use super::collab_handler_hub_verify_genesis_tests::append;
use super::collab_handler_hub_verify_tests::{
    hv, key, op, write, Hv, Key, Keys, Method, KB, METHODS,
};
use super::*;
use crate::doc_store::AnchorRefused;
use crate::storage::AnchorSource;
use mae_mcp::identity::Identity;

/// The owner's self-admit as the log's root: the KB's signed log now governs
/// once it is anchored to the owner's key.
async fn owner_genesis(h: &Hv, k: &Keys) {
    append(h, &k.owner, MembershipAction::Admit, &k.owner.fp).await;
}

fn sorted(fps: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = fps.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

async fn source(h: &Hv) -> MembershipSource {
    kb_membership_source(&h.store, KB).await.expect("a KB")
}

/// A KB shared before the amendment with Bob and Carol on its member list and
/// an owner genesis in its log: anchoring it hands membership to the log, which
/// admits neither — and both the operator's command and the owner's own
/// connection say so. Without a genesis the member list still governs and
/// nobody is reported.
#[tokio::test]
async fn anchoring_reports_the_members_the_signed_log_leaves_out() {
    let (mut h, k) = hv(false, false, false).await;
    owner_genesis(&h, &k).await;
    assert_eq!(
        source(&h).await,
        MembershipSource::default(),
        "precondition"
    );
    let lost = sorted(&[&k.bob.fp, &k.carol.fp]);
    let done = operator_anchor_kb(&h.store, KB, k.owner.pk).await;
    assert_eq!(
        done,
        Ok(OperatorAnchor::Recorded {
            roster_only: lost.clone()
        })
    );
    let src = source(&h).await;
    assert!(src.log_governed, "the log governs once anchored");
    assert_eq!(
        src.roster_only, lost,
        "and `kb anchor` shows who it leaves out"
    );
    // Log wins: they really have no access.
    let epoch = 0;
    let (upd, signed) = op(&k.bob, epoch, "bob after anchoring");
    let msg = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
    expect_kind(&h.call(k.bob.mtls(), msg).await, "not_member", "bob");

    // The owner's own connection anchors the same way, and reports the same.
    let (h, k) = hv(false, false, false).await;
    owner_genesis(&h, &k).await;
    let reported = adopt_owner_anchor(&h.store, KB, None, Some(&k.owner.pk)).await;
    assert_eq!(reported, sorted(&[&k.bob.fp, &k.carol.fp]), "adopt");
    let again = adopt_owner_anchor(&h.store, KB, None, Some(&k.owner.pk)).await;
    assert!(again.is_empty(), "reported once, when recorded");

    // Through the dispatcher: the owner's write anchors it.
    let (mut h, k) = hv(false, false, false).await;
    owner_genesis(&h, &k).await;
    let (upd, _) = op(&k.owner, 0, "the owner writes");
    let r = h
        .call(k.owner.mtls(), write(Method::NodeUpdate, &upd, None))
        .await;
    assert!(r.error.is_none(), "{:?}", r.error);
    assert_eq!(
        source(&h).await.roster_only,
        sorted(&[&k.bob.fp, &k.carol.fp])
    );

    // No genesis: the member list governs, nobody is left out.
    let (h, k) = hv(false, false, false).await;
    let done = operator_anchor_kb(&h.store, KB, k.owner.pk).await;
    assert_eq!(
        done,
        Ok(OperatorAnchor::Recorded {
            roster_only: vec![]
        })
    );
    assert_eq!(source(&h).await, MembershipSource::default());
}

/// `mae-daemon doctor`'s report lists every anchored KB whose log leaves a
/// listed member out, and only those: a KB whose daemon signed every member
/// into its log has nothing to report.
#[tokio::test]
async fn the_doctor_report_lists_members_the_signed_log_leaves_out() {
    let (h, k) = hv(false, false, false).await;
    owner_genesis(&h, &k).await;
    assert!(operator_anchor_kb(&h.store, KB, k.owner.pk).await.is_ok());
    let report = roster_only_members_report(&h.store).await;
    let lost = sorted(&[&k.bob.fp, &k.carol.fp]);
    assert_eq!(report, vec![(KB.to_string(), Ok(lost))]);

    let (h, _) = hv(false, true, true).await;
    assert!(
        source(&h).await.log_governed,
        "precondition: the log governs"
    );
    assert_eq!(roster_only_members_report(&h.store).await, vec![]);
}

/// Where the log governs and this daemon cannot sign into it, no member-list
/// writer changes membership: add, remove, approve and a permissive join's
/// auto-admit are all refused, and the member list is untouched. Where the
/// member list governs, or the daemon signs as the anchor's owner, they work.
#[tokio::test]
async fn a_member_list_change_is_refused_where_the_signed_log_governs() {
    let dave = key("dave");
    let mallory = key("mallory");

    // Log-governed, editor-owned: this daemon holds no signer for it.
    let (mut h, k) = hv(false, false, true).await;
    owner_genesis(&h, &k).await;
    assert!(source(&h).await.log_governed, "precondition");
    let owner = k.owner.bearer();
    let add = kb_member_msg("kb/add_member", KB, &dave.fp, Some("editor"));
    expect_kind(&h.call(owner, add).await, "signer_unavailable", "add");
    let remove = kb_member_msg("kb/remove_member", KB, &k.bob.fp, None);
    expect_kind(&h.call(owner, remove).await, "signer_unavailable", "remove");
    let approve = kb_approve_msg(KB, &dave.fp, Some("editor"));
    expect_kind(
        &h.call(owner, approve).await,
        "signer_unavailable",
        "approve",
    );
    let r = h.call(owner, kb_policy_msg(KB, "permissive")).await;
    assert!(r.error.is_none(), "policy: {:?}", r.error);
    let join = h.call(mallory.bearer(), kb_join_msg(KB)).await;
    expect_kind(&join, "signer_unavailable", "permissive auto-join");
    let coll = load_coll(&h.store, KB).await;
    assert_eq!(coll.role_of(&dave.fp), None, "nothing added");
    assert_eq!(coll.role_of(&mallory.fp), None, "nobody auto-admitted");
    assert_eq!(
        coll.role_of(&k.bob.fp),
        Some(SyncRole::Editor),
        "nothing removed"
    );

    // The member list governs: every writer works.
    let (mut h, k) = hv(false, false, true).await;
    assert!(!source(&h).await.log_governed, "precondition");
    let owner = k.owner.bearer();
    let add = kb_member_msg("kb/add_member", KB, &dave.fp, Some("editor"));
    assert!(h.call(owner, add).await.error.is_none(), "roster add");
    assert!(h
        .call(owner, kb_policy_msg(KB, "permissive"))
        .await
        .error
        .is_none());
    let join = h.call(mallory.bearer(), kb_join_msg(KB)).await;
    assert!(join.error.is_none(), "roster auto-join: {:?}", join.error);

    // The daemon signs as the anchor's owner: an add lands in the log, so the
    // new member writes; an auto-admit never signs, so it is still refused.
    let (mut h, k) = hv(false, true, true).await;
    assert!(source(&h).await.log_governed, "precondition");
    let owner = k.owner.bearer();
    let add = kb_member_msg("kb/add_member", KB, &dave.fp, Some("editor"));
    assert!(h.call(owner, add).await.error.is_none(), "signed add");
    let epoch = h.epoch(&dave).await;
    let (upd, signed) = op(&dave, epoch, "dave, admitted by the log");
    let msg = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
    let r = h.call(dave.mtls(), msg).await;
    assert!(r.error.is_none(), "dave writes: {:?}", r.error);
    assert!(h
        .call(owner, kb_policy_msg(KB, "permissive"))
        .await
        .error
        .is_none());
    let join = h.call(mallory.bearer(), kb_join_msg(KB)).await;
    expect_kind(&join, "signer_unavailable", "an auto-admit never signs");
}

/// `mae-daemon kb member add` signs as the daemon, the KB's owner: it works on
/// a member-list KB and on a log-governed KB anchored to the daemon's key (the
/// admit lands in the log), and is refused where the log is rooted at another
/// key, since its admit would reach the member list only.
#[tokio::test]
async fn the_operator_member_add_is_refused_where_it_cannot_sign_into_the_log() {
    let dave = key("dave");
    fn add(m: &str) -> OperatorMemberOp<'_> {
        OperatorMemberOp::Add {
            member: m,
            role: SyncRole::Editor,
            label: "dave",
        }
    }
    // Member list governs (shared without a key: no anchor).
    let (h, k) = hv(false, true, false).await;
    assert!(!source(&h).await.log_governed, "precondition");
    let r = operator_change_membership(&h.store, &k.owner.id, KB, add(&dave.fp)).await;
    assert!(r.is_ok(), "roster KB: {r:?}");

    // The log governs, rooted at the daemon's own key.
    let (h, k) = hv(false, true, true).await;
    assert!(source(&h).await.log_governed, "precondition");
    let r = operator_change_membership(&h.store, &k.owner.id, KB, add(&dave.fp)).await;
    assert!(r.is_ok(), "the daemon signs into its log: {r:?}");
    assert!(
        source(&h).await.roster_only.is_empty(),
        "dave is in the log"
    );

    // The log governs, rooted at another key.
    let (h, k) = hv(false, true, false).await;
    h.store.set_kb_anchor(KB, key("other").pk).await.unwrap();
    assert!(source(&h).await.log_governed, "precondition");
    let r = operator_change_membership(&h.store, &k.owner.id, KB, add(&dave.fp)).await;
    let e = r.expect_err("refused");
    assert!(e.contains("signed op-log"), "{e}");
    assert_eq!(load_coll(&h.store, KB).await.role_of(&dave.fp), None);
}

/// F1: a join ticket never replaces an anchor. A hub-recorded anchor keeps its
/// key and its source against a ticket naming another key or its own key; a
/// joined anchor accepts its own key again (a redial) and refuses another —
/// in memory and on disk.
#[tokio::test]
async fn a_join_ticket_never_replaces_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.db");
    let (owner, other) = (key("owner"), key("ticket"));
    let hub = Some((owner.pk, AnchorSource::HubRecorded));
    let joined = Some((owner.pk, AnchorSource::Joined));
    {
        let store = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
        let recorded = store.record_kb_anchor_if_absent(KB, owner.pk).await;
        assert_eq!(recorded.ok(), Some(true));
        for pk in [other.pk, owner.pk] {
            let r = store.set_kb_anchor(KB, pk).await;
            assert_eq!(r, Err(AnchorRefused::HubRecorded));
            assert_eq!(store.kb_anchor_record(KB).await, hub, "kept");
        }
        assert_eq!(store.set_kb_anchor("kb-j", owner.pk).await, Ok(()));
        let redial = store.set_kb_anchor("kb-j", owner.pk).await;
        assert_eq!(redial, Ok(()), "a redial");
        let r = store.set_kb_anchor("kb-j", other.pk).await;
        assert_eq!(r, Err(AnchorRefused::OtherKey));
        assert_eq!(store.kb_anchor_record("kb-j").await, joined, "kept");
    }
    let reopened = DocStore::new(Arc::new(SqliteBackend::open(&path).unwrap()), 500);
    reopened.load_local_state().await;
    assert_eq!(reopened.kb_anchor_record(KB).await, hub, "on disk too");
    assert_eq!(reopened.kb_anchor_record("kb-j").await, joined);

    // And the KB keeps its membership: not flipped to fail-closed `Joined`.
    let (mut h, k) = hv(false, false, true).await;
    let r = h.store.set_kb_anchor(KB, other.pk).await;
    assert_eq!(r, Err(AnchorRefused::HubRecorded));
    let epoch = h.epoch(&k.bob).await;
    let (upd, signed) = op(&k.bob, epoch, "bob still writes");
    let msg = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
    let r = h.call(k.bob.mtls(), msg).await;
    assert!(r.error.is_none(), "{:?}", r.error);
}

/// F4 runs on every identified session. Only a session with no identity at
/// all (`auth.mode = none`) has no principal: it relays a member's op checked
/// for signature and membership, as a mesh peer would. A psk session's
/// principal is `psk:<keyid>`, so it cannot send a member's op. And a key peer
/// whose key is the all-zero encoding still has a principal.
#[tokio::test]
async fn a_session_without_identity_skips_only_authorship() {
    let (mut h, k) = hv(false, false, true).await;
    let none: Session<'_> = (None, None, Transport::Hub);
    let epoch = h.epoch(&k.carol).await;
    for m in METHODS {
        let (upd, signed) = op(&k.carol, epoch, &format!("carol via none {m:?}"));
        let r = h
            .call(none, write(m, &upd, Some(signed.header_params())))
            .await;
        assert!(r.error.is_none(), "{m:?}: {:?}", r.error);
        let (upd, mut signed) = op(&k.carol, epoch, &format!("forged {m:?}"));
        signed.sig[0] ^= 1;
        let r = h
            .call(none, write(m, &upd, Some(signed.header_params())))
            .await;
        expect_kind(&r, "signature_invalid", &format!("{m:?}"));
    }
    let mallory = key("mallory");
    let (upd, signed) = op(&mallory, 0, "mallory via none");
    let r = h
        .call(
            none,
            write(Method::SyncUpdate, &upd, Some(signed.header_params())),
        )
        .await;
    expect_kind(&r, "not_member", "non-member via none");

    let psk = "psk:team";
    let add = kb_member_msg("kb/add_member", KB, psk, Some("editor"));
    assert!(h.call(k.owner.bearer(), add).await.error.is_none());
    let (upd, signed) = op(&k.carol, epoch, "carol via psk");
    let msg = write(Method::NodeUpdate, &upd, Some(signed.header_params()));
    let r = h.call((Some(psk), None, Transport::Hub), msg).await;
    expect_kind(&r, "not_author", "psk");

    let zero = mae_mcp::identity::PublicKey::from_bytes(&[0u8; 32], None).expect("decodes");
    let peer = mae_mcp::identity::PeerIdentity {
        label: "zero".into(),
        fingerprint: zero.fingerprint(),
        pubkey: [0u8; 32],
        bearer: false,
    };
    assert_eq!(
        peer.principal(),
        Some(zero.fingerprint().as_str()),
        "an identified session is never loopback-trusted"
    );
}

/// The epoch fence on the anchored path: the signed log governs, Bob is removed
/// and re-admitted, and the log moves his epoch. An op signed under the old
/// epoch is fenced; one under the new is admitted, on both methods.
#[tokio::test]
async fn re_admission_moves_the_epoch_on_the_signed_log() {
    let (mut h, k) = hv(false, true, true).await;
    assert!(
        source(&h).await.log_governed,
        "precondition: the log governs"
    );
    let log_epoch = |h: &Hv| {
        let store = Arc::clone(&h.store);
        let bob = k.bob.fp.clone();
        async move {
            let coll = load_coll(&store, KB).await;
            let anchor = store.kb_anchor(KB).await.expect("anchored");
            let dm = store
                .derived_membership(KB, &coll, &anchor, now_unix())
                .await;
            dm.members.get(&bob).map(|m| m.epoch)
        }
    };
    let before = log_epoch(&h).await.expect("bob is in the log");
    for (method, role) in [
        ("kb/remove_member", None),
        ("kb/add_member", Some("editor")),
    ] {
        let msg = kb_member_msg(method, KB, &k.bob.fp, role);
        let r = h.call(k.owner.bearer(), msg).await;
        assert!(r.error.is_none(), "{method}: {:?}", r.error);
    }
    let now = log_epoch(&h).await.expect("bob is in the log again");
    assert_ne!(before, now, "precondition: the log moved the epoch");
    for m in METHODS {
        let (upd, signed) = op(&k.bob, before, &format!("stale {m:?}"));
        let r = h
            .call(k.bob.mtls(), write(m, &upd, Some(signed.header_params())))
            .await;
        let data = expect_kind(&r, "epoch_fenced", &format!("old epoch, {m:?}"));
        assert_eq!(data["current_epoch"].as_u64(), Some(now));
        let (upd, signed) = op(&k.bob, now, &format!("fresh {m:?}"));
        let r = h
            .call(k.bob.mtls(), write(m, &upd, Some(signed.header_params())))
            .await;
        assert!(r.error.is_none(), "new epoch, {m:?}: {:?}", r.error);
    }
}

/// The owner rotates its key on an anchored, log-governed KB. The anchor stays
/// the key that signed the genesis — the log's root — and the log's `Rebind`
/// carries ownership to the new key, so the owner does nothing more: it
/// writes, it never re-anchors, and no path (its own connection, the
/// operator's command, a join ticket naming it) replaces the anchor. The old
/// key is retired.
#[tokio::test]
async fn an_owner_key_rotation_keeps_the_anchor() {
    let (mut h, k) = hv(false, true, true).await;
    let id = Arc::new(Identity::from_seed(&[93u8; 32], "owner-rotated"));
    let new = Key {
        fp: id.fingerprint(),
        pk: id.public().to_bytes(),
        id,
    };
    let wrap = mae_sync::content_crypto::wrap_public_for(&new.id.secret_bytes());
    let mut coll = load_coll(&h.store, KB).await;
    let secret = k.owner.id.secret_bytes();
    let rebind = coll.author_rebind(
        KB,
        &k.owner.fp,
        &new.fp,
        &new.pk,
        &wrap,
        &secret,
        &k.owner.pk,
        1000,
    );
    let r = h
        .call(k.owner.mtls(), kb_collection_op_msg(KB, &rebind))
        .await;
    assert!(r.error.is_none(), "the owner's rebind: {:?}", r.error);

    let epoch = h.epoch(&new).await;
    let (upd, signed) = op(&new, epoch, "the rotated owner writes");
    let msg = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
    let r = h.call(new.mtls(), msg).await;
    assert!(r.error.is_none(), "the new key writes: {:?}", r.error);
    assert!(operator_anchor_kb(&h.store, KB, new.pk).await.is_err());
    let ticket = h.store.set_kb_anchor(KB, new.pk).await;
    assert_eq!(ticket, Err(AnchorRefused::HubRecorded));
    let kept = Some((k.owner.pk, AnchorSource::HubRecorded));
    assert_eq!(h.store.kb_anchor_record(KB).await, kept, "the genesis key");
    assert!(source(&h).await.log_governed, "the log still governs");

    let (upd, _) = op(&k.owner, 0, "the retired key");
    let r = h
        .call(k.owner.mtls(), write(Method::NodeUpdate, &upd, None))
        .await;
    expect_kind(&r, "not_member", "the retired key");
}

/// The attacker's test for ADR-040 §4 on the anchored, log-governed path: a
/// rotation retires the old key, and a session still authenticated by it gets
/// NOTHING — whoever signed what it carries. Its own op, the successor's op,
/// another member's op, an op naming the successor without a signature, and
/// the successor's self re-wrap are all refused there. That is why the editor
/// reconnects under the new key once its `Rebind` is accepted, and sends the
/// re-wrap on that session (`collab_bridge::finish_rotation`). The control: the
/// SAME successor op and re-wrap are accepted on the new key's own session, so
/// every refusal above is the retired session's and nothing else's.
#[tokio::test]
async fn a_retired_keys_session_gets_nothing_after_rotation() {
    for m in METHODS {
        let (mut h, k, new) = rotated_owner().await;
        // The node exists first, so a refusal is not a node's first write.
        let bob_epoch = h.epoch(&k.bob).await;
        let (upd, signed) = op(&k.bob, bob_epoch, "bob was here");
        let r = h
            .call(k.bob.mtls(), write(m, &upd, Some(signed.header_params())))
            .await;
        assert!(
            r.error.is_none(),
            "bob seeds the node, {m:?}: {:?}",
            r.error
        );

        let epoch = h.epoch(&new).await;
        let text = format!("rotated owner, {m:?}");
        let (succ_upd, succ_signed) = op(&new, epoch, &text);
        let (own_upd, own_signed) = op(&k.owner, 0, "the retired key's own op");
        let (bob_upd, bob_signed) = op(&k.bob, h.epoch(&k.bob).await, "bob's op");
        let mut unsigned = succ_signed.header_params();
        unsigned.as_object_mut().unwrap().remove("sig");
        let attempts = [
            (
                &succ_upd,
                Some(succ_signed.header_params()),
                "the successor's op",
            ),
            (
                &own_upd,
                Some(own_signed.header_params()),
                "the retired key's own op",
            ),
            (&bob_upd, Some(bob_signed.header_params()), "bob's op"),
            (&succ_upd, Some(unsigned), "unsigned, naming the successor"),
            (&own_upd, None, "unsigned"),
        ];
        for (upd, header, ctx) in attempts {
            let r = h.call(k.owner.mtls(), write(m, upd, header)).await;
            expect_kind(
                &r,
                "not_member",
                &format!("{ctx} on the retired session, {m:?}"),
            );
        }
        let node_doc = mae_sync::kb_node_doc_name(KB, "concept:n");
        let applied = |state: &[u8]| state.windows(text.len()).any(|w| w == text.as_bytes());
        let (state, _) = h.store.encode_state_and_sv(&node_doc).await.expect("node");
        assert!(
            !applied(&state),
            "nothing the retired session sent landed, {m:?}"
        );

        rewrap_lands_only_on_the_new_session(&mut h, &k, &new, m).await;

        // Control: the successor's op itself is fine on its own session.
        let r = h
            .call(
                new.mtls(),
                write(m, &succ_upd, Some(succ_signed.header_params())),
            )
            .await;
        assert!(
            r.error.is_none(),
            "the successor's op on its session, {m:?}: {:?}",
            r.error
        );
        let (state, _) = h.store.encode_state_and_sv(&node_doc).await.expect("node");
        assert!(applied(&state), "and it is applied, {m:?}");
    }
}

/// The successor's own self re-wrap: refused on the retired session, accepted
/// on the new key's session, where it reaches the log.
async fn rewrap_lands_only_on_the_new_session(h: &mut Hv, k: &Keys, new: &Key, m: Method) {
    let rewrap = successor_rewrap(h, k, new).await;
    let r = h
        .call(k.owner.mtls(), kb_collection_op_msg(KB, &rewrap))
        .await;
    assert!(
        r.error.is_some(),
        "the re-wrap on the retired session, {m:?}"
    );
    let before = load_coll(&h.store, KB).await.oplog_ops().len();
    let r = h.call(new.mtls(), kb_collection_op_msg(KB, &rewrap)).await;
    assert!(
        r.error.is_none(),
        "the re-wrap on the new session, {m:?}: {:?}",
        r.error
    );
    let after = load_coll(&h.store, KB).await.oplog_ops().len();
    assert_eq!(after, before + 1, "the re-wrap reached the log, {m:?}");
}

/// The rotated owner's self re-wrap of the KB's content key to its new wrap
/// key — what `collab-rotate-identity` sends after its `Rebind` (ADR-040 §3).
async fn successor_rewrap(h: &Hv, k: &Keys, new: &Key) -> Vec<u8> {
    let key = mae_sync::content_crypto::ContentKey::generate();
    let wrap_pk = mae_sync::content_crypto::wrap_public_for(&new.id.secret_bytes());
    let wrapped = mae_sync::content_crypto::wrap_to_member(&key, &wrap_pk).expect("wrap");
    let mut coll = load_coll(&h.store, KB).await;
    let secret = new.id.secret_bytes();
    coll.author_rebind_rewrap(
        KB,
        &new.fp,
        &new.pk,
        wrapped,
        &k.owner.pk,
        &new.fp,
        &secret,
        &new.pk,
        1001,
    )
}

/// [`hv`]'s KB, its signed log governing, after the owner rotated its key with
/// a `Rebind` sent on its own (old-key) session — the editor's rotation.
async fn rotated_owner() -> (Hv, Keys, Key) {
    let (mut h, k) = hv(false, true, true).await;
    let id = Arc::new(Identity::generate("owner-rotated"));
    let new = Key {
        fp: id.fingerprint(),
        pk: id.public().to_bytes(),
        id,
    };
    let wrap = mae_sync::content_crypto::wrap_public_for(&new.id.secret_bytes());
    let mut coll = load_coll(&h.store, KB).await;
    let secret = k.owner.id.secret_bytes();
    let rebind = coll.author_rebind(
        KB,
        &k.owner.fp,
        &new.fp,
        &new.pk,
        &wrap,
        &secret,
        &k.owner.pk,
        1000,
    );
    let r = h
        .call(k.owner.mtls(), kb_collection_op_msg(KB, &rebind))
        .await;
    assert!(r.error.is_none(), "the owner's rebind: {:?}", r.error);
    assert!(source(&h).await.log_governed, "the log governs");
    (h, k, new)
}
