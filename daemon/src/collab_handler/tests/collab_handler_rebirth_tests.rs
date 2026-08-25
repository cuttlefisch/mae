//! ADR-107: the daemon's rebirth gate.
//!
//! A rebirth REPLACES a node document, discarding its operation history. That is
//! the growth bound — and it is also the most destructive thing the KB protocol
//! can be asked to do, so the daemon must refuse every version of it that is not
//! exactly what the owner signed.
//!
//! `kb_access(Manage)` proves the caller may rebirth *something*. The binding
//! check proves they are rebirthing *what the owner signed*. Both are required:
//! authority without binding turns "discard this node's history" into "replace
//! this node with anything", and the destroyed history is precisely what would
//! have revealed the substitution.

use super::*;
use mae_mcp::identity::Identity;
use mae_sync::kb::{KbNodeDoc, Role};
use mae_sync::membership::{MembershipAction, MembershipOp, ReplicationPolicy};

/// A KB owned by a real signing identity, with one node in its manifest.
async fn anchored_kb_with_node(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    docs: &mut HashSet<String>,
) -> (Arc<Identity>, String) {
    let owner = Arc::new(Identity::generate("owner"));
    let owner_fp = owner.fingerprint();
    store.set_signer(Arc::clone(&owner));

    kb_share_as(
        store,
        bc,
        Some("owner"),
        Some(&owner_fp),
        "kb1",
        "owner",
        docs,
    )
    .await;
    // The signed owner-genesis: `derive_rebirths` resolves the owner from it, the
    // same way `derive_encryption` does. `kb/share` does not seed one (only
    // `p2p/share_kb` does), so an anchored-KB fixture has to author it.
    append_op(
        store,
        bc,
        &owner,
        &owner_fp,
        MembershipAction::Admit,
        &owner_fp,
        Some(Role::Owner),
        None,
        docs,
    )
    .await;
    dispatch_as(
        store,
        bc,
        Some("owner"),
        Some(&owner_fp),
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/collection_node_add",
            "params":{"kb_id":"kb1","node_id":"concept:n","title":"N"}}),
        docs,
    )
    .await;
    (owner, owner_fp)
}

/// A node document with real accumulated history — what a rebirth is FOR.
fn grown_node() -> KbNodeDoc {
    let mut d = KbNodeDoc::new_with_client_id("concept:n", "N", "body", &[], 7);
    for i in 0..30 {
        let _ = d.set_body(&format!("revision {i}"));
    }
    d
}

fn rebirth_msg(state: &[u8]) -> serde_json::Value {
    serde_json::json!({
    "jsonrpc":"2.0","id":1,"method":"kb/node_update",
    "params":{
        "kb_id":"kb1","node_id":"concept:n",
        "update": update_to_base64(state),
        "rebirth": true,
    }})
}

/// **A rebirth with no signed op is refused.**
///
/// Without this, `rebirth: true` is a bare "replace this document" verb available
/// to any owner session — a silent local truncation with no record peers can see.
/// The signed op is what makes a rebirth *observable*.
#[tokio::test]
async fn a_rebirth_with_no_signed_op_is_refused() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (_owner, owner_fp) = anchored_kb_with_node(&store, &bc, &mut docs).await;

    let reborn = grown_node().reborn(99);
    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        rebirth_msg(&reborn.encode_state()),
        &mut docs,
    )
    .await;

    let err = format!("{resp:?}");
    assert!(
        err.contains("no owner-signed Rebirth op"),
        "an unsigned rebirth must be refused with a reason, got: {err}"
    );
}

/// **The attack the hash exists to stop.** A caller with legitimate authority
/// ships content that is NOT what the owner signed.
///
/// The owner authorised discarding this node's history. They did not authorise
/// replacing its content — and once the history is gone, nothing remains to show
/// the difference.
#[tokio::test]
async fn a_rebirth_whose_state_does_not_match_the_signed_hash_is_refused() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (owner, owner_fp) = anchored_kb_with_node(&store, &bc, &mut docs).await;

    // The owner signs a rebirth of the REAL content...
    let genuine = grown_node().reborn(99);
    append_rebirth_op(&store, &bc, &owner, &owner_fp, &genuine, &mut docs).await;

    // ...but a substituted document is shipped instead.
    let mut forged = grown_node();
    let _ = forged.set_body("content the owner never signed");
    let forged = forged.reborn(99);

    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        rebirth_msg(&forged.encode_state()),
        &mut docs,
    )
    .await;

    let err = format!("{resp:?}");
    assert!(
        err.contains("the owner did not sign"),
        "a substituted rebirth must be refused, got: {err}"
    );
}

/// The positive case, so the tests above are not passing on "everything is
/// refused". A rebirth matching its signed hash is applied.
#[tokio::test]
async fn a_rebirth_matching_its_signed_hash_is_applied() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (owner, owner_fp) = anchored_kb_with_node(&store, &bc, &mut docs).await;

    let reborn = grown_node().reborn(99);
    append_rebirth_op(&store, &bc, &owner, &owner_fp, &reborn, &mut docs).await;

    let resp = dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        rebirth_msg(&reborn.encode_state()),
        &mut docs,
    )
    .await;

    let out = format!("{resp:?}");
    assert!(
        out.contains("applied"),
        "a correctly signed rebirth must be applied, got: {out}"
    );
}

/// **A non-owner cannot rebirth**, even holding an Editor role — the same
/// `Manage` gate a reseal carries, for the same reason: this destroys state.
#[tokio::test]
async fn an_editor_cannot_rebirth_a_node() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    let (_owner, owner_fp) = anchored_kb_with_node(&store, &bc, &mut docs).await;

    let bob = Identity::generate("bob");
    let bob_fp = bob.fingerprint();
    dispatch_as(
        &store,
        &bc,
        Some("owner"),
        Some(&owner_fp),
        kb_member_msg("kb/add_member", "kb1", &bob_fp, Some("editor")),
        &mut docs,
    )
    .await;

    let reborn = grown_node().reborn(99);
    let mut bob_docs = HashSet::new();
    let resp = dispatch_as(
        &store,
        &bc,
        Some("bob"),
        Some(&bob_fp),
        rebirth_msg(&reborn.encode_state()),
        &mut bob_docs,
    )
    .await;

    let err = format!("{resp:?}");
    assert!(
        !err.contains("\"applied\":true") && !err.contains("applied: true"),
        "an Editor must not be able to rebirth a node: {err}"
    );
}

/// Author + ship an owner-signed `Rebirth` op for `doc` via `kb/collection_op`.
async fn append_rebirth_op(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    owner: &Arc<Identity>,
    owner_fp: &str,
    doc: &KbNodeDoc,
    docs: &mut HashSet<String>,
) {
    append_op(
        store,
        bc,
        owner,
        owner_fp,
        MembershipAction::Rebirth,
        "concept:n",
        None,
        Some(doc.rebirth_hash()),
        docs,
    )
    .await;
}

/// Append one owner-signed membership op to `kb1`'s log, chaining onto whatever
/// is already there.
#[allow(clippy::too_many_arguments)]
async fn append_op(
    store: &Arc<DocStore>,
    bc: &SharedBroadcaster,
    owner: &Arc<Identity>,
    owner_fp: &str,
    action: MembershipAction,
    subject: &str,
    role: Option<Role>,
    content_hash: Option<String>,
    docs: &mut HashSet<String>,
) {
    let coll = crate::collab_handler::load_collection(store, "kb1")
        .await
        .expect("collection loads");
    let prev = coll
        .oplog_ops()
        .last()
        .map(|o| o.op.chain_hash(&o.sig))
        .unwrap_or_default();
    let op = MembershipOp {
        kb_id: "kb1".into(),
        action,
        subject: subject.into(),
        role,
        can_invite: false,
        author: owner_fp.to_string(),
        issued_at: 1,
        expires_at: None,
        epoch: if content_hash.is_some() { 1 } else { 0 },
        prev_hash: prev,
        wrapped_key: None,
        new_pubkey: None,
        new_wrap_pubkey: None,
        recovery_pubkey: None,
        replication: ReplicationPolicy::Full,
        content_hash,
    };
    let sig = op.sign(&owner.secret_bytes());
    let mut c2 = coll;
    let update = c2.append_signed_op(&op, &sig, &owner.public().to_bytes());
    dispatch_as(
        store,
        bc,
        Some("owner"),
        Some(owner_fp),
        serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/collection_op",
            "params":{"kb_id":"kb1","update":update_to_base64(&update)}}),
        docs,
    )
    .await;
}
