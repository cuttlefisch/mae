//! ADR-105 D3: a KB id must be one a document address can unambiguously carry.
//!
//! Its own module rather than an inline block in `handle_doc_request_inner`, for
//! the same reason the check is centralised in the first place: it is one rule
//! covering every `kb/*` method, and a rule that lives inside the dispatch match
//! is a rule the next method added there can quietly sit beside.

use super::*;

/// Refuse any request whose `kb_id` cannot appear in a node document address.
///
/// Returns `Some(error_response)` to refuse, `None` to continue.
///
/// `kbn:{kb_id}:{node_id}` splits on the FIRST colon, and node ids routinely
/// contain colons (`concept:architecture`), so the address parses back
/// unambiguously only if the KB id contains none. This is not hygiene — it is the
/// same defect ADR-105 exists to remove: kb_id "a:b" + node "c" and kb_id "a" +
/// node "b:c" both spell `kbn:a:b:c`, so two tenants collide on one document
/// exactly as in #718. Measured with the guard removed, the collision is a
/// cross-tenant read in some runs and silent data loss in others, decided by CRDT
/// merge order. `kb_id` is client-supplied on `kb/share`, so anyone could reach it.
///
/// Keyed on the PRESENCE of a `kb_id` param rather than on a list of method names,
/// so a method added later is covered without anyone remembering to add it — the
/// enumerate-the-sites approach is what finding C showed fails open.
///
/// A KB whose id already contains a colon is refused rather than grandfathered:
/// its nodes are already mis-addressed, so failing loudly is strictly better than
/// continuing to collide silently.
pub(super) fn refuse_unaddressable_kb_id(
    session_id: u64,
    method: &str,
    params: &serde_json::Value,
    id: &serde_json::Value,
) -> Option<JsonRpcResponse> {
    let kb_id = params.get("kb_id").and_then(|v| v.as_str())?;
    if mae_sync::kb_id_is_addressable(kb_id) {
        return None;
    }
    warn!(
        session = session_id,
        method, kb_id, "rejected: KB id is not addressable (empty or contains ':')"
    );
    Some(JsonRpcResponse::error(
        id.clone(),
        McpError::invalid_request(format!(
            "KB id {kb_id:?} is not addressable: it must be non-empty and must not \
             contain ':' (it forms part of each node's document address, which \
             splits on the first colon)"
        )),
    ))
}
