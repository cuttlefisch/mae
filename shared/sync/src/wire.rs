//! Canonical JSON-RPC wire-message builders for the collaborative sync protocol.
//!
//! These constructors are the **single source of truth** for the collab wire
//! messages, shared by the editor (`mae` emit path) and the daemon (and the
//! end-to-end tests). Centralising them here makes the class of bug behind
//! ADR-020 B-8 *structurally impossible*: `kb/node_update` was hand-rolled in the
//! editor's background task **without an `id`**, so the daemon classified it as a
//! fire-and-forget notification and dropped it before the apply+broadcast handler
//! — while a parallel hand-rolled *test* client sent the correct (id-bearing)
//! shape, so no test ever caught it. With one shared builder, production and tests
//! serialise identically, and a request that the daemon must apply + acknowledge
//! can never silently degrade into a notification again.
//!
//! Convention: a message the daemon must apply/persist and reply to is a **request**
//! (carries an `id`); a relay-only, fire-and-forget message (e.g. awareness) is a
//! **notification** (no `id`). The daemon's read loop routes no-`id` messages to the
//! notification handler, which only relays — so anything needing application MUST be
//! built here as a request.

use serde_json::{json, Value};

/// Build a `kb/node_update` JSON-RPC **request**.
///
/// Carries an `id`, so the daemon dispatches it to the request handler that runs
/// access control → `apply_update` (WAL) → `broadcast_except` to peers → replies
/// `{"applied": true}`. `update_b64` is the base64-encoded yrs update
/// (see [`crate::encoding::update_to_base64`]). The `node_id` is the bare KB node
/// id (e.g. `collabtest:overview`); the daemon namespaces it to `kb:<node_id>`.
pub fn kb_node_update_request(id: u64, kb_id: &str, node_id: &str, update_b64: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "kb/node_update",
        "params": {
            "kb_id": kb_id,
            "node_id": node_id,
            "update": update_b64,
        }
    })
}

/// Build a `kb/share` JSON-RPC **request** (owner shares all nodes of a KB).
///
/// `collection_state_b64` is the base64 `KbCollectionDoc` state; `nodes` is the
/// list of `(node_id, state_b64)` pairs. The node objects are built here with the
/// canonical `{ "id", "state" }` keys so the editor and tests can never disagree on
/// the field names.
pub fn kb_share_request(
    id: u64,
    kb_id: &str,
    name: &str,
    creator: &str,
    collection_state_b64: &str,
    nodes: &[(String, String)],
) -> Value {
    let nodes_json: Vec<Value> = nodes
        .iter()
        .map(|(node_id, state_b64)| json!({ "id": node_id, "state": state_b64 }))
        .collect();
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "kb/share",
        "params": {
            "kb_id": kb_id,
            "name": name,
            "creator": creator,
            "collection_state": collection_state_b64,
            "nodes": nodes_json,
        }
    })
}

/// Build a `kb/join` JSON-RPC **request** (member joins a shared KB, pulling state).
///
/// ADR-022: `node_svs` carries the member's per-node state vectors as
/// `(node_id, sv_b64)` pairs so the daemon can reply with an SV **diff** (only
/// the ops the member lacks) per node and the member can reconcile instead of
/// blindly adopting a full snapshot — the crash-safe (re)join path. When empty,
/// the `node_svs` key is omitted entirely, producing the exact pre-ADR-022 shape
/// so an older daemon (or a first-ever join with no local nodes) still gets full
/// state. The pairs use the canonical `{ "id", "sv" }` keys.
pub fn kb_join_request(id: u64, kb_id: &str, node_svs: &[(String, String)]) -> Value {
    let mut params = json!({ "kb_id": kb_id });
    if !node_svs.is_empty() {
        let svs: Vec<Value> = node_svs
            .iter()
            .map(|(node_id, sv_b64)| json!({ "id": node_id, "sv": sv_b64 }))
            .collect();
        params["node_svs"] = Value::Array(svs);
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "kb/join",
        "params": params,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression guard for ADR-020 B-8: `kb/node_update` MUST be a request
    /// (carry an `id`) or the daemon silently drops it as a notification.
    #[test]
    fn kb_node_update_is_a_request_with_id() {
        let req = kb_node_update_request(42, "collabtest", "collabtest:overview", "AAEC");
        assert_eq!(
            req["id"], 42,
            "kb/node_update MUST carry an id (request, not notification) — \
             without it the daemon routes it to the notification sink and drops it"
        );
        assert_eq!(req["method"], "kb/node_update");
        assert_eq!(req["params"]["kb_id"], "collabtest");
        assert_eq!(req["params"]["node_id"], "collabtest:overview");
        assert_eq!(req["params"]["update"], "AAEC");
    }

    /// Every request-shaped builder here must carry a non-null `id` and a `method`.
    /// This is the mechanical net that catches a future "forgot the id" regression
    /// across the whole protocol surface.
    #[test]
    fn all_request_builders_carry_an_id() {
        let requests = [
            kb_node_update_request(1, "k", "k:n", "AAEC"),
            kb_share_request(
                2,
                "k",
                "name",
                "creator",
                "AAEC",
                &[("k:n".into(), "AAEC".into())],
            ),
            kb_join_request(3, "k", &[]),
        ];
        for req in requests {
            let method = req["method"].as_str().unwrap_or("<none>").to_string();
            assert!(
                req.get("id").is_some_and(|v| !v.is_null()),
                "request builder for method '{method}' must carry a non-null id"
            );
            assert_eq!(
                req["jsonrpc"], "2.0",
                "method '{method}' must be JSON-RPC 2.0"
            );
        }
    }

    /// ADR-022: `kb/join` omits `node_svs` when empty (exact pre-ADR-022 shape,
    /// backward-compatible) and emits the canonical `{id, sv}` objects otherwise.
    #[test]
    fn kb_join_request_node_svs_shape() {
        let empty = kb_join_request(1, "k", &[]);
        assert!(
            empty["params"].get("node_svs").is_none(),
            "empty node_svs must be omitted (backward-compat): {empty}"
        );

        let with = kb_join_request(2, "k", &[("k:n1".into(), "AAEC".into())]);
        let svs = with["params"]["node_svs"]
            .as_array()
            .expect("node_svs must be an array");
        assert_eq!(svs.len(), 1);
        assert_eq!(svs[0]["id"], "k:n1");
        assert_eq!(svs[0]["sv"], "AAEC");
    }
}
