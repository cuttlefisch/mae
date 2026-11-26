//! ADR-036 Amendment 2: the editor signs each change to a KB's node list. The
//! request it builds carries a signature that verifies over exactly the change
//! it asks for, with this editor's key and epoch; a removal signs no title.

use super::*;
use mae_sync::manifest_op::{ManifestAction, ManifestOp};

fn rebuilt(req: &serde_json::Value, action: ManifestAction) -> (ManifestOp, Vec<u8>, [u8; 32]) {
    let p = &req["params"];
    let b64 = |k: &str| mae_sync::encoding::base64_to_update(p[k].as_str().unwrap()).unwrap();
    let op = ManifestOp {
        kb_id: p["kb_id"].as_str().unwrap().into(),
        node_id: p["node_id"].as_str().unwrap().into(),
        action,
        title: match action {
            ManifestAction::Add => p["title"].as_str().unwrap().into(),
            ManifestAction::Remove => String::new(),
        },
        author: p["author"].as_str().unwrap().into(),
        epoch: p["epoch"].as_u64().unwrap(),
        issued_at: p["issued_at"].as_u64().unwrap(),
    };
    (op, b64("sig"), b64("author_pubkey").try_into().unwrap())
}

/// The add and the remove the editor sends verify, under its own key and the
/// epoch it was given; changing the node or title breaks them. Kills a builder
/// that signs other fields than it sends, or signs a removal's title.
#[test]
fn the_editors_node_list_requests_are_signed_over_what_they_ask() {
    let id = std::sync::Arc::new(mae_mcp::identity::Identity::generate("editor"));
    let add = kb_collection_node_request(1, ("kb-x", "concept:n", "Title", true), 42, Some(&id));
    assert_eq!(add["method"], "kb/collection_node_add");
    let (op, sig, pk) = rebuilt(&add, ManifestAction::Add);
    assert_eq!(
        (op.author.as_str(), op.epoch),
        (id.fingerprint().as_str(), 42)
    );
    assert!(op.verify(&sig, &pk));
    let renamed = ManifestOp {
        title: "Other".into(),
        ..op
    };
    assert!(!renamed.verify(&sig, &pk));

    let rm = kb_collection_node_request(2, ("kb-x", "concept:n", "Title", false), 42, Some(&id));
    assert_eq!(rm["method"], "kb/collection_node_remove");
    let (op, sig, pk) = rebuilt(&rm, ManifestAction::Remove);
    assert!(
        op.title.is_empty() && op.verify(&sig, &pk),
        "a removal signs no title"
    );
    let other = ManifestOp {
        node_id: "concept:m".into(),
        ..op
    };
    assert!(!other.verify(&sig, &pk));

    let unsigned = kb_collection_node_request(3, ("kb-x", "concept:n", "T", true), 42, None);
    assert!(
        unsigned["params"].get("sig").is_none(),
        "no key, no signature"
    );
}
