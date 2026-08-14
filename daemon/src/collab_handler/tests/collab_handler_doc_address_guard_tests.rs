//! ADR-105 Stage 1: the doc-address guards, pinned as *properties of the address
//! type* rather than of a string prefix.
//!
//! These are regression tests for bypasses a **future** change could introduce, not
//! for a bug that exists today. Stage 1 is a pure refactor — five guards moved from
//! `starts_with("kb:")` / `strip_prefix("kb:")` to exhaustive `DocAddress` matching
//! — precisely so that Stage 2's node-address rename cannot silently un-guard them.
//! Each of the five failed **open or destructive** under a string rename:
//!
//! - the raw-read gate would have served node plaintext to any client
//! - `sync/update`'s node arm would have skipped signature verify, `kb_access` *and*
//!   the epoch fence
//! - `verify_relayed_content_op` would have treated node ops as "not a content op"
//! - `is_durable_doc` would have made node docs evictable-and-deletable — data loss
//! - the P2P dialer would have stopped fencing relayed writes
//!
//! So the value of this file is that it keeps failing if any of those regress, in a
//! form that does not care what the doc-name string happens to be. Every assertion
//! builds its doc name through `DocAddress::to_doc_name()` rather than a literal —
//! a literal would have to be rewritten in Stage 2, and a test you rewrite alongside
//! the change it is supposed to police is not a check.

use super::*;

fn node_doc(kb_id: &str, node_id: &str) -> String {
    // Stage 2 gave `KbNode` its `kb_id`. Only this constructor changed — every
    // assertion in this file was written against the type, so none of them moved.
    mae_sync::kb_node_doc_name(kb_id, node_id)
}

fn collection_doc(kb_id: &str) -> String {
    mae_sync::DocAddress::KbCollection {
        kb_id: kb_id.to_string(),
    }
    .to_doc_name()
}

/// A KB node doc must never be readable through the raw sync path, whatever its
/// address happens to look like.
#[tokio::test]
async fn raw_reads_of_a_node_doc_are_refused_by_address_type() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&fp("owner")),
        "kbguard",
        "owner",
        &mut docs,
    )
    .await;

    for method in ["sync/full_state", "sync/state_vector"] {
        let r = dispatch_as(
            &store,
            &bc,
            Some("owner"),
            Some(&fp("owner")),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,
                "params":{"doc": node_doc("kbguard", "concept:secret")}}),
            &mut docs,
        )
        .await;
        // Even the OWNER is refused here: the point is that node content has exactly
        // one door (`kb/node_fetch`), not that this caller lacks rights.
        let err = r
            .error
            .unwrap_or_else(|| panic!("{method} on a node doc must be refused"));
        assert!(
            err.message.contains("kb/node_fetch"),
            "the refusal must name the correct door, got: {}",
            err.message
        );
    }
}

/// The other half of the oracle: buffer collab must still work. Asserting only the
/// refusal above would pass just as well if the gate refused everything — which is
/// the mistake an earlier draft of this change actually made, caught by
/// `raw_sync_read_of_a_kb_doc_is_access_gated`'s "non-KB doc keeps its existing
/// (ungated) sync behavior".
#[tokio::test]
async fn raw_reads_of_non_kb_docs_are_still_ungated() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    for doc in [
        mae_sync::DocAddress::Shared {
            name: "scratch".to_string(),
        }
        .to_doc_name(),
        // `sync/share` lets a client pick an arbitrary name; such a doc is ordinary
        // buffer collaboration and must not be gated as KB content.
        "an-arbitrary-doc-name".to_string(),
    ] {
        let r = dispatch_as(
            &store,
            &bc,
            Some("someone"),
            Some(&fp("someone")),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/state_vector",
                "params":{"doc": doc}}),
            &mut docs,
        )
        .await;
        assert!(
            r.error.is_none(),
            "non-KB doc '{doc}' must keep ungated sync behaviour, got: {:?}",
            r.error
        );
    }
}

/// A raw `sync/update` to a *collection* doc is owner-only (ADR-018
/// membership-smuggling defense).
#[tokio::test]
async fn raw_collection_writes_stay_owner_only_by_address_type() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut owner_docs = HashSet::new();
    kb_share_as(
        &store,
        &bc,
        Some("owner"),
        Some(&fp("owner")),
        "kbguard",
        "owner",
        &mut owner_docs,
    )
    .await;

    let mut evil_docs = HashSet::new();
    let r = dispatch_as(
        &store,
        &bc,
        Some("evil"),
        Some(&fp("evil")),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
            "params":{"doc": collection_doc("kbguard"),
                      "update": update_to_base64(&[0u8; 4])}}),
        &mut evil_docs,
    )
    .await;
    let err = r
        .error
        .expect("a non-owner raw write to a collection doc must be refused");
    assert!(
        err.message.contains("owner"),
        "the refusal must say it is owner-only, got: {}",
        err.message
    );
}

/// A `sync/update` to a node doc without `kb_id` must be refused — that parameter is
/// what carries the write into `kb_access` and the epoch fence (#169 M1). If the node
/// arm ever stops matching, this write sails through unchecked.
#[tokio::test]
async fn node_writes_via_sync_update_still_require_kb_id() {
    let store = test_doc_store();
    let bc = test_broadcaster();
    let mut docs = HashSet::new();

    let r = dispatch_as(
        &store,
        &bc,
        Some("someone"),
        Some(&fp("someone")),
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
            "params":{"doc": node_doc("kbguard", "concept:x"),
                      "update": update_to_base64(&[0u8; 4])}}),
        &mut docs,
    )
    .await;
    let err = r
        .error
        .expect("a node write with no kb_id must be refused, not applied");
    assert!(
        err.message.contains("kb_id"),
        "the refusal must name the missing kb_id, got: {}",
        err.message
    );
}

/// KB docs must remain durable. This one is not a security property — it decides
/// whether idle eviction **deletes the document from storage**, so a node doc that
/// stops being recognized is silent data loss.
#[test]
fn kb_docs_are_durable_by_address_type() {
    for doc in [
        node_doc("kbguard", "concept:x"),
        node_doc("kbguard", "plain-node"),
        collection_doc("kbguard"),
    ] {
        assert!(
            crate::doc_store::is_durable_doc(&doc),
            "'{doc}' must be durable — non-durable means evicted AND deleted"
        );
    }
    for doc in [
        mae_sync::DocAddress::Shared {
            name: "scratch".to_string(),
        }
        .to_doc_name(),
        "an-arbitrary-doc-name".to_string(),
    ] {
        assert!(
            !crate::doc_store::is_durable_doc(&doc),
            "'{doc}' is transient buffer collab and must keep evict-and-delete"
        );
    }
}
