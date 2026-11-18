//! ADR-036 Amendment 1 (#727): the hub verifies every content op it accepts.
//!
//! Through the real dispatcher, on both write methods (`kb/node_update` and
//! `sync/update`), as a keyed (mTLS) session and as a `/ws` bearer session
//! (principal, no key), and as a mesh peer on the P2P transport. Every key is
//! freshly generated; the store's signing policy is its own, so no test changes
//! another's.

use super::collab_handler_error_kind_tests::expect_kind;
use super::*;
use mae_mcp::identity::Identity;
use mae_sync::content_ops::{ContentOp, SignedContentOp};

pub(super) const KB: &str = "kb-hv";
const NODE: &str = "concept:n";

/// A real key: identity, fingerprint (the principal) and public key.
pub(super) struct Key {
    pub id: Arc<Identity>,
    pub fp: String,
    pub pk: [u8; 32],
}

pub(super) fn key(name: &str) -> Key {
    let id = Arc::new(Identity::generate(name));
    let (fp, pk) = (id.fingerprint(), id.public().to_bytes());
    Key { id, fp, pk }
}

impl Key {
    /// An mTLS / key-auth session: its principal is its key's fingerprint.
    pub(super) fn mtls(&self) -> Session<'_> {
        (Some(self.fp.as_str()), Some(&self.pk), Transport::Hub)
    }
    /// A `/ws` bearer session: the principal, and no key.
    pub(super) fn bearer(&self) -> Session<'_> {
        (Some(self.fp.as_str()), None, Transport::Hub)
    }
    /// A mesh peer on the P2P listener.
    fn mesh(&self) -> Session<'_> {
        (Some(self.fp.as_str()), Some(&self.pk), Transport::P2p)
    }
}

/// `text` authored under `author`'s `epoch` client id, signed by `author`.
pub(super) fn op(author: &Key, epoch: u64, text: &str) -> (Vec<u8>, SignedContentOp) {
    let cid = derive_kb_client_id(&author.fp, epoch);
    let upd = TextSync::with_client_id("", cid).insert(0, text);
    let op = ContentOp {
        kb_id: KB.to_string(),
        node_id: NODE.to_string(),
        base_sv: vec![],
        author: author.fp.clone(),
        epoch,
        issued_at: 1_700_000_000,
    };
    let sig = op.sign(&author.id.secret_bytes(), &upd);
    let signed = SignedContentOp {
        op,
        payload: upd.clone(),
        sig,
        author_pubkey: author.pk,
    };
    (upd, signed)
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Method {
    NodeUpdate,
    SyncUpdate,
}
pub(super) const METHODS: [Method; 2] = [Method::NodeUpdate, Method::SyncUpdate];

/// A write of `upd` by `method`, carrying `header` (or none: unsigned).
pub(super) fn write(
    method: Method,
    upd: &[u8],
    header: Option<serde_json::Value>,
) -> serde_json::Value {
    let update = update_to_base64(upd);
    let mut params = match method {
        Method::NodeUpdate => serde_json::json!({"kb_id": KB, "node_id": NODE, "update": update}),
        Method::SyncUpdate => serde_json::json!({"doc": mae_sync::kb_node_doc_name(KB, NODE),
            "kb_id": KB, "update": update}),
    };
    match (method, header) {
        (Method::NodeUpdate, Some(h)) => {
            for (k, v) in h.as_object().unwrap() {
                params[k] = v.clone();
            }
        }
        (Method::SyncUpdate, Some(h)) => params["content_header"] = h,
        (_, None) => {}
    }
    let name = match method {
        Method::NodeUpdate => "kb/node_update",
        Method::SyncUpdate => "sync/update",
    };
    serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": name, "params": params})
}

pub(super) struct Hv {
    pub store: Arc<DocStore>,
    pub bc: SharedBroadcaster,
    pub docs: HashSet<String>,
}

/// The KB's owner and its two editors.
pub(super) struct Keys {
    pub owner: Key,
    pub bob: Key,
    pub carol: Key,
}

/// A KB owned by `owner`, shared over hub and mesh, with editors Bob and Carol.
/// `require_signed`: the switch. `oplog`: this daemon signs as the owner, so
/// membership changes write a signed op-log (an anchored KB with a log).
/// `keyed_share`: shared on a keyed session (anchored at first share), or on a
/// session without a key, as a KB shared before the amendment was.
pub(super) async fn hv(require_signed: bool, oplog: bool, keyed_share: bool) -> (Hv, Keys) {
    let backend = Arc::new(SqliteBackend::open_memory().unwrap());
    let store = DocStore::new(backend, 500).with_require_signed_content_ops(require_signed);
    let mut h = Hv {
        store: Arc::new(store),
        bc: test_broadcaster(),
        docs: HashSet::new(),
    };
    let k = Keys {
        owner: key("owner"),
        bob: key("bob"),
        carol: key("carol"),
    };
    if oplog {
        h.store.set_signer(Arc::clone(&k.owner.id));
    }
    let coll = KbCollectionDoc::new_owned(KB, "", "owner");
    let share = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "kb/share",
        "params": {"kb_id": KB, "name": KB, "transport": "both", "nodes": [],
                   "collection_state": update_to_base64(&coll.encode_state())}});
    let sharer = if keyed_share {
        k.owner.mtls()
    } else {
        k.owner.bearer()
    };
    let r = h.call(sharer, share).await;
    assert!(r.error.is_none(), "share: {:?}", r.error);
    for m in [&k.bob.fp, &k.carol.fp] {
        let add = kb_member_msg("kb/add_member", KB, m, Some("editor"));
        let r = h.call(k.owner.bearer(), add).await;
        assert!(r.error.is_none(), "admit: {:?}", r.error);
    }
    (h, k)
}

impl Hv {
    pub(super) async fn call(&mut self, s: Session<'_>, msg: serde_json::Value) -> JsonRpcResponse {
        dispatch_session(&self.store, &self.bc, s, msg, &mut self.docs).await
    }
    /// `who`'s epoch as `kb/join` reports it — the one the fence enforces.
    pub(super) async fn epoch(&mut self, who: &Key) -> u64 {
        let r = self.call(who.bearer(), kb_join_msg(KB)).await;
        r.result.expect("join")["epoch"].as_u64().expect("an epoch")
    }
}

/// Every KB kind × both methods × mTLS and `/ws`: an unsigned op is refused,
/// and the same member's signed op on the same path is admitted, so the
/// refusal is the signature's and nothing else's.
#[tokio::test]
async fn with_the_switch_on_an_unsigned_op_is_refused_on_every_path() {
    for (oplog, keyed) in [(false, true), (true, true), (false, false)] {
        let (mut h, k) = hv(true, oplog, keyed).await;
        let (bob, epoch) = (&k.bob, h.epoch(&k.bob).await);
        for (si, session) in [bob.mtls(), bob.bearer()].into_iter().enumerate() {
            for m in METHODS {
                let ctx = format!("{m:?} oplog={oplog} keyed_share={keyed} session={si}");
                let (upd, _) = op(bob, epoch, &format!("unsigned {ctx}"));
                expect_kind(
                    &h.call(session, write(m, &upd, None)).await,
                    "signature_required",
                    &ctx,
                );
                let (upd, signed) = op(bob, epoch, &format!("signed {ctx}"));
                let r = h
                    .call(session, write(m, &upd, Some(signed.header_params())))
                    .await;
                assert!(
                    r.error.is_none(),
                    "{ctx}: a signed op is admitted: {:?}",
                    r.error
                );
            }
        }
        assert_eq!(h.store.content_op_policy().unsigned_accepted(), 0);
    }
}

/// With the switch off an unsigned op is accepted on both methods and counted
/// once each; one refused before it is applied is not counted.
#[tokio::test]
async fn with_the_switch_off_an_unsigned_op_is_accepted_and_counted() {
    let (mut h, k) = hv(false, false, true).await;
    let epoch = h.epoch(&k.bob).await;
    let stranger = key("stranger");
    for (i, m) in METHODS.into_iter().enumerate() {
        let (upd, _) = op(&k.bob, epoch, &format!("unsigned {i}"));
        let r = h.call(k.bob.mtls(), write(m, &upd, None)).await;
        assert!(r.error.is_none(), "{m:?}: {:?}", r.error);
        let (upd, _) = op(&stranger, 0, "stranger");
        expect_kind(
            &h.call(stranger.mtls(), write(m, &upd, None)).await,
            "not_member",
            "stranger",
        );
    }
    assert_eq!(h.store.content_op_policy().unsigned_accepted(), 2);
}

/// F2: an anchored KB without a signed op-log reads membership from its member
/// list, as `kb_access` does — it does not lose every member to an empty
/// derivation.
#[tokio::test]
async fn an_anchored_kb_without_an_op_log_admits_its_members_signed_ops() {
    let (mut h, k) = hv(true, false, true).await;
    let coll = load_coll(&h.store, KB).await;
    assert_eq!(
        h.store.kb_anchor(KB).await,
        Some(k.owner.pk),
        "precondition: anchored"
    );
    assert!(
        coll.oplog_head().is_none(),
        "precondition: no signed op-log"
    );
    let epoch = h.epoch(&k.carol).await;
    for m in METHODS {
        let (upd, signed) = op(&k.carol, epoch, &format!("{m:?}"));
        let r = h
            .call(k.carol.mtls(), write(m, &upd, Some(signed.header_params())))
            .await;
        assert!(r.error.is_none(), "{m:?}: {:?}", r.error);
    }
}

/// F1: the key must hash to the author and the signature verify. A header with
/// a signature that does not parse is invalid — never downgraded to unsigned,
/// even with the switch off.
#[tokio::test]
async fn a_key_or_signature_that_does_not_bind_the_author_is_signature_invalid() {
    let (mut h, k) = hv(false, false, true).await;
    let epoch = h.epoch(&k.bob).await;
    for m in METHODS {
        let (upd, signed) = op(&k.bob, epoch, &format!("{m:?}"));
        let mut wrong_key = signed.header_params();
        wrong_key["author_pubkey"] = serde_json::json!(update_to_base64(&k.carol.pk));
        let mut bad_sig = signed.clone();
        bad_sig.sig[0] ^= 0xff;
        let mut malformed = signed.header_params();
        malformed.as_object_mut().unwrap().remove("author_pubkey");
        // Bob's key and signature, naming Carol: refused for the key, before
        // the session's authorship is even asked.
        let mut names_carol = signed.header_params();
        names_carol["author"] = serde_json::json!(k.carol.fp);
        for (what, header) in [
            ("key of another", wrong_key),
            ("flipped signature", bad_sig.header_params()),
            ("signature without a key", malformed),
            ("another author named", names_carol),
        ] {
            let r = h.call(k.bob.mtls(), write(m, &upd, Some(header))).await;
            expect_kind(&r, "signature_invalid", &format!("{m:?}: {what}"));
        }
    }
    assert_eq!(
        h.store.content_op_policy().unsigned_accepted(),
        0,
        "none counted as unsigned"
    );
}

/// The signed op is bound to the KB and node the daemon writes, not to ones
/// its header names: an op Bob signed for another node (or KB) does not verify
/// on this one, even when the header says so.
#[tokio::test]
async fn a_header_naming_another_node_or_kb_does_not_verify_here() {
    let (mut h, k) = hv(false, false, true).await;
    let epoch = h.epoch(&k.bob).await;
    let (upd, mut signed) = op(&k.bob, epoch, "signed for elsewhere");
    for (field, value) in [("node_id", "concept:other"), ("kb_id", "kb-other")] {
        let mut elsewhere = signed.op.clone();
        match field {
            "node_id" => elsewhere.node_id = value.to_string(),
            _ => elsewhere.kb_id = value.to_string(),
        }
        signed.sig = elsewhere.sign(&k.bob.id.secret_bytes(), &upd);
        let mut header = signed.header_params();
        header[field] = serde_json::json!(value);
        let r = h
            .call(k.bob.mtls(), write(Method::SyncUpdate, &upd, Some(header)))
            .await;
        expect_kind(
            &r,
            "signature_invalid",
            &format!("signed for another {field}"),
        );
    }
}

/// F4: a session cannot send another member's op, whatever its session kind;
/// the same op relayed by a mesh peer is admitted, its author being checked
/// for membership instead.
#[tokio::test]
async fn a_session_cannot_send_another_members_op_but_a_mesh_peer_relays_it() {
    let (mut h, k) = hv(true, false, true).await;
    let epoch = h.epoch(&k.carol).await;
    let (upd, signed) = op(&k.carol, epoch, "carol's words");
    for session in [k.bob.mtls(), k.bob.bearer()] {
        for m in METHODS {
            let r = h
                .call(session, write(m, &upd, Some(signed.header_params())))
                .await;
            expect_kind(&r, "not_author", &format!("{m:?} on bob's session"));
        }
    }
    let relay = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
    let r = h.call(k.bob.mesh(), relay).await;
    assert!(
        r.error.is_none(),
        "a mesh peer relays a member's op: {:?}",
        r.error
    );
    let mallory = key("mallory");
    let (upd, signed) = op(&mallory, 0, "mallory's words");
    let relay = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
    expect_kind(
        &h.call(k.bob.mesh(), relay).await,
        "not_member",
        "relayed non-member op",
    );
}

/// Without an anchor, admission falls back to the roster, and the fence there
/// is the member's CURRENT epoch, not a default. Re-admission moves Bob's epoch
/// (#72): an op signed under the superseded epoch is fenced, and one signed
/// under the new epoch is admitted on every method — the positive control is
/// what pins the roster's epoch, since only a match is admitted.
#[tokio::test]
async fn without_an_anchor_the_fence_is_the_members_current_epoch() {
    let (mut h, k) = hv(false, false, false).await;
    assert_eq!(
        h.store.kb_anchor(KB).await,
        None,
        "precondition: not anchored"
    );
    let before = h.epoch(&k.bob).await;
    for (method, role) in [
        ("kb/remove_member", None),
        ("kb/add_member", Some("editor")),
    ] {
        let msg = kb_member_msg(method, KB, &k.bob.fp, role);
        let r = h.call(k.owner.bearer(), msg).await;
        assert!(r.error.is_none(), "{method}: {:?}", r.error);
    }
    let now = h.epoch(&k.bob).await;
    assert_ne!(before, now, "precondition: re-admission moves the epoch");
    assert_ne!(now, 0, "precondition: the current epoch is not the default");
    for m in METHODS {
        let (upd, signed) = op(&k.bob, before, &format!("stale {m:?}"));
        let r = h
            .call(k.bob.mtls(), write(m, &upd, Some(signed.header_params())))
            .await;
        let data = expect_kind(&r, "epoch_fenced", &format!("superseded epoch, {m:?}"));
        assert_eq!(data["current_epoch"].as_u64(), Some(now));
        let (upd, signed) = op(&k.bob, now, &format!("fresh {m:?}"));
        let r = h
            .call(k.bob.mtls(), write(m, &upd, Some(signed.header_params())))
            .await;
        assert!(r.error.is_none(), "current epoch, {m:?}: {:?}", r.error);
    }
}

/// #727 §1: a KB with no anchor was not verified at all — a signed op passed
/// unchecked. It is now admitted against its member list like any other.
#[tokio::test]
async fn a_kb_without_an_anchor_still_verifies_signed_ops() {
    let (mut h, k) = hv(false, false, false).await;
    assert_eq!(
        h.store.kb_anchor(KB).await,
        None,
        "precondition: not anchored"
    );
    let epoch = h.epoch(&k.bob).await;
    // Built under the CURRENT client id, so only the signed epoch is stale.
    let (upd, mut signed) = op(&k.bob, epoch, "claims another epoch");
    signed.op.epoch = epoch + 7;
    signed.sig = signed.op.sign(&k.bob.id.secret_bytes(), &upd);
    for m in METHODS {
        let r = h
            .call(k.bob.mtls(), write(m, &upd, Some(signed.header_params())))
            .await;
        let data = expect_kind(&r, "epoch_fenced", &format!("{m:?}"));
        assert_eq!(data["caller_epoch"].as_u64(), Some(epoch + 7));
    }
    let mallory = key("mallory");
    let (upd, signed) = op(&mallory, 0, "mallory's words");
    let relay = write(Method::SyncUpdate, &upd, Some(signed.header_params()));
    expect_kind(
        &h.call(k.bob.mesh(), relay).await,
        "not_member",
        "relayed non-member op",
    );
}
