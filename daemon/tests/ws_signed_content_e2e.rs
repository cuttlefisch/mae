//! ADR-036 Amendment 1 over the real sync WebSocket, with
//! `require_signed_content_ops` on: a bearer session's unsigned write is
//! refused on both write methods, its own signed write is admitted, and another
//! member's signed op sent on it is refused `not_author` (F4, F6; ADR-099
//! Amendment 2 E4).

mod daemon_harness;
mod ws_common;

use mae_mcp::identity::Identity;
use mae_sync::content_ops::{ContentOp, SignedContentOp};
use mae_sync::encoding::update_to_base64;
use serde_json::{json, Value};
use ws_common::*;

const KB: &str = "ks";
const NODE: &str = "n1";

fn ok(resp: &Value) -> bool {
    resp["error"].is_null()
}

/// `update` as a `kb/node_update`, signed by `author` at `epoch`.
fn signed_write(author: &Identity, epoch: u64, update: &[u8]) -> Value {
    let op = ContentOp {
        kb_id: KB.to_string(),
        node_id: NODE.to_string(),
        base_sv: vec![],
        author: author.fingerprint(),
        epoch,
        issued_at: 1_700_000_000,
    };
    let sig = op.sign(&author.secret_bytes(), update);
    let signed = SignedContentOp {
        op,
        payload: update.to_vec(),
        sig,
        author_pubkey: author.public().to_bytes(),
    };
    let mut params = json!({"kb_id": KB, "node_id": NODE, "update": update_to_base64(update)});
    for (k, v) in signed.header_params().as_object().unwrap() {
        params[k] = v.clone();
    }
    params
}

#[tokio::test]
async fn a_ws_session_writes_only_ops_it_signed_when_signatures_are_required() {
    let d = spawn(Opts {
        require_signed: true,
        ..Opts::default()
    })
    .await;
    let (alice, bob) = (Identity::generate("alice"), Identity::generate("bob"));
    let mut a = connect_bearer(&d, &d.token(&alice.fingerprint(), 600)).await;
    let shared = a
        .call(1, "kb/share", kb_share(KB, &[(NODE, "one", "body")]))
        .await;
    assert!(ok(&shared), "{shared}");
    let add = json!({"kb_id": KB, "member": bob.fingerprint(), "role": "editor", "label": "bob"});
    assert!(ok(&a.call(2, "kb/add_member", add).await));

    let mut b = connect_bearer(&d, &d.token(&bob.fingerprint(), 600)).await;
    let joined = b.call(1, "kb/join", json!({"kb_id": KB})).await;
    assert!(ok(&joined), "{joined}");
    let epoch = epoch_of(&joined);
    let mut doc = node_writer(&joined, NODE, &bob.fingerprint(), epoch);
    let upd = update_to_base64(&doc.set_title("unsigned"));
    let unsigned = json!({"kb_id": KB, "node_id": NODE, "update": upd});
    let r = b.call(2, "kb/node_update", unsigned).await;
    assert_eq!(kind(&r), "signature_required", "{r}");
    let doc_name = mae_sync::kb_node_doc_name(KB, NODE);
    let sync = json!({"doc": doc_name, "kb_id": KB, "update": upd});
    let r = b.call(3, "sync/update", sync).await;
    assert_eq!(kind(&r), "signature_required", "{r}");

    let mut doc = node_writer(&joined, NODE, &bob.fingerprint(), epoch);
    let own = doc.set_title("signed by bob");
    let r = b
        .call(4, "kb/node_update", signed_write(&bob, epoch, &own))
        .await;
    assert!(ok(&r), "the session's own signed op is admitted: {r}");

    let foreign = doc.set_title("signed by alice");
    let r = b
        .call(5, "kb/node_update", signed_write(&alice, 0, &foreign))
        .await;
    assert_eq!(kind(&r), "not_author", "{r}");
}
