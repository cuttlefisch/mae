//! The write-path refusal kinds, and what `kb/join` tells a client so it never
//! has to guess the fence epoch.
//!
//! The `kb/join` oracle is the fence itself: the epoch `kb/join` returns must
//! be the one `kb/node_update` accepts, and the one it names when it refuses a
//! stale write — checked across a role change and a removal/re-admission, the
//! two events that move a member's epoch (ADR-023, #72).

use super::collab_handler_error_kind_tests::{
    expect_kind, fixture, rpc, update_as, Fixture, KB, NODE,
};
use super::*;
use mae_mcp::identity::Identity;
use mae_sync::content_ops::{ContentOp, SignedContentOp};

fn write_as(principal: &str, epoch: u64, text: &str) -> serde_json::Value {
    rpc(
        "kb/node_update",
        serde_json::json!({"kb_id": KB, "node_id": NODE, "update": update_as(principal, epoch, text)}),
    )
}

/// `kb/join` as `principal`: its result's `principal` and `epoch`.
async fn join(f: &mut Fixture, principal: Option<&str>) -> (serde_json::Value, serde_json::Value) {
    let r = f.call_as(principal, kb_join_msg(KB)).await;
    let result = r
        .result
        .unwrap_or_else(|| panic!("join failed: {:?}", r.error));
    assert!(
        result["nodes"].is_array(),
        "the existing fields stay: {result}"
    );
    (result["principal"].clone(), result["epoch"].clone())
}

async fn set_member(f: &mut Fixture, method: &str, who: &str, role: Option<&str>) {
    let r = f
        .call(Some("owner"), kb_member_msg(method, KB, who, role))
        .await;
    assert!(r.error.is_none(), "{method} {who}: {:?}", r.error);
}

#[tokio::test]
async fn kb_join_reports_the_principal_and_the_epoch_the_fence_enforces() {
    let mut f = fixture().await;
    let member = fp("viewer");
    let (principal, e0) = join(&mut f, Some(&member)).await;
    assert_eq!(principal, serde_json::json!(member));
    assert_eq!(
        e0.as_u64(),
        Some(load_coll(&f.store, KB).await.epoch_of(&member))
    );

    // A role change moves the epoch, and join reports the new one.
    set_member(&mut f, "kb/add_member", &member, Some("editor")).await;
    let e1 = join(&mut f, Some(&member))
        .await
        .1
        .as_u64()
        .expect("an epoch");
    assert_ne!(
        e0.as_u64(),
        Some(e1),
        "a grant must move the epoch (ADR-023)"
    );

    // The old epoch is fenced, and the refusal names the current one.
    let stale = f
        .call_as(Some(&member), write_as(&member, e0.as_u64().unwrap(), "s"))
        .await;
    let data = expect_kind(&stale, "epoch_fenced", "write under the pre-grant epoch");
    assert_eq!(data["current_epoch"], e1);
    assert!(
        data.get("caller_epoch").is_none(),
        "a bare update names no epoch: {data}"
    );
    // Authoring under the epoch join reported is accepted.
    let fresh = f.call_as(Some(&member), write_as(&member, e1, "f")).await;
    assert!(
        fresh.error.is_none(),
        "write under join's epoch: {:?}",
        fresh.error
    );

    // Removal and re-admission issue a fresh epoch (#72); join reports it too.
    set_member(&mut f, "kb/remove_member", &member, None).await;
    set_member(&mut f, "kb/add_member", &member, Some("editor")).await;
    let e2 = join(&mut f, Some(&member))
        .await
        .1
        .as_u64()
        .expect("an epoch");
    assert_ne!(e2, e1, "re-admission must not reuse the pre-removal epoch");
    let stale = f.call_as(Some(&member), write_as(&member, e1, "s2")).await;
    assert_eq!(
        expect_kind(&stale, "epoch_fenced", "pre-removal epoch")["current_epoch"],
        e2
    );
}

/// Without authentication there is no principal and no per-identity fence, so
/// both are `null` — never a made-up `0` a client would author under.
#[tokio::test]
async fn an_unauthenticated_join_reports_no_principal_and_no_epoch() {
    let mut f = fixture().await;
    let (principal, epoch) = join(&mut f, None).await;
    assert!(
        principal.is_null() && epoch.is_null(),
        "{principal} {epoch}"
    );
}

/// An anchored KB: membership derives from the signed op-log, and this daemon
/// is the owner's signer. Returns the fixture, the owner and an editor identity.
pub(super) async fn anchored() -> (Fixture, Arc<Identity>, Identity) {
    let mut f = Fixture {
        store: test_doc_store(),
        bc: test_broadcaster(),
        docs: HashSet::new(),
    };
    let owner = Arc::new(Identity::generate("owner"));
    let editor = Identity::generate("editor");
    let owner_pk = owner.public().to_bytes();
    f.store.set_signer(Arc::clone(&owner));
    let owner_fp = owner.fingerprint();
    let shared = kb_share_as(
        &f.store,
        &f.bc,
        Some("owner"),
        Some(&owner_fp),
        KB,
        "owner",
        &mut f.docs,
    )
    .await;
    assert!(shared.error.is_none(), "{:?}", shared.error);
    let add = kb_member_msg("kb/add_member", KB, &editor.fingerprint(), Some("editor"));
    assert!(f.call_as(Some(&owner_fp), add).await.error.is_none());
    f.store.set_kb_anchor(KB, owner_pk).await;
    (f, owner, editor)
}

/// A content op authored by `author`, claiming `epoch`, signed by `author`.
fn signed_by(author: &Identity, epoch: u64) -> serde_json::Value {
    let (upd, signed) = signed_op(author, epoch);
    signed_node_update_msg(KB, NODE, &upd, &signed)
}

fn signed_op(author: &Identity, epoch: u64) -> (Vec<u8>, SignedContentOp) {
    let mut ts = TextSync::with_client_id("", derive_kb_client_id(&author.fingerprint(), epoch));
    let upd = ts.insert(0, "signed");
    let op = ContentOp {
        kb_id: KB.to_string(),
        node_id: NODE.to_string(),
        base_sv: vec![],
        author: author.fingerprint(),
        epoch,
        issued_at: 1_700_000_000,
    };
    let sig = op.sign(&author.secret_bytes(), &upd);
    let signed = SignedContentOp {
        op,
        payload: upd.clone(),
        sig,
        author_pubkey: author.public().to_bytes(),
    };
    (upd, signed)
}

#[tokio::test]
async fn signed_write_refusals_carry_their_kinds() {
    let (mut f, _owner, editor) = anchored().await;
    let me = editor.fingerprint();
    let epoch = join(&mut f, Some(&me)).await.1.as_u64().expect("an epoch");
    let ok = f.call_as(Some(&me), signed_by(&editor, epoch)).await;
    assert!(
        ok.error.is_none(),
        "a signed op at join's epoch is admitted: {:?}",
        ok.error
    );

    // A signed op names its epoch, so the fence reports both sides.
    let stale = f.call_as(Some(&me), signed_by(&editor, epoch + 1)).await;
    let data = expect_kind(&stale, "epoch_fenced", "signed op at a wrong epoch");
    assert_eq!(
        (
            data["current_epoch"].as_u64(),
            data["caller_epoch"].as_u64()
        ),
        (Some(epoch), Some(epoch + 1))
    );

    let (upd, mut forged) = signed_op(&editor, epoch);
    forged.sig[0] ^= 0xff;
    let forged = signed_node_update_msg(KB, NODE, &upd, &forged);
    expect_kind(
        &f.call_as(Some(&me), forged).await,
        "signature_invalid",
        "forged signature",
    );

    // A member relaying an op whose signed AUTHOR is not a member.
    let outsider = Identity::generate("outsider");
    let relayed = f.call_as(Some(&me), signed_by(&outsider, 0)).await;
    expect_kind(&relayed, "not_member", "op authored by a non-member");
}

#[tokio::test]
async fn signed_membership_refusals_carry_their_kinds() {
    let (mut f, owner, editor) = anchored().await;
    let owner_fp = owner.fingerprint();
    let revoke = f
        .call_as(Some(&owner_fp), kb_revoke_msg(KB, &fp("nobody")))
        .await;
    expect_kind(&revoke, "subject_not_member", "revoke of a non-member");

    let editor_fp = editor.fingerprint();
    let (secret, public) = (owner.secret_bytes(), owner.public().to_bytes());
    set_replication_query_only(&f.store, KB, &secret, &public, &owner_fp, &editor_fp).await;
    let join = f.call_as(Some(&editor_fp), kb_join_msg(KB)).await;
    expect_kind(
        &join,
        "replication_restricted",
        "query-only member replicating",
    );
}
