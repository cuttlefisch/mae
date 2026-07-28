//! `kb/claim_lease` — the ADR-033 advisory lease RPC (issue #420 / ADR-061 Phase
//! D1). Coordinates an expensive, KB-wide bulk operation (enrichment sweeps,
//! embedding rebuilds — namespaced by `op_kind`) so only one daemon runs it at a
//! time. Gated by `KbOp::Edit` (not `KbOp::Manage`, unlike `kb/collection_op`) —
//! any Editor-role member should be able to claim the lease, not only the owner.
//! Rides the EXISTING `kbc:{kb_id}` collection-doc sync pipe
//! (`persist_and_broadcast_collection`) — no new transport.

use super::*;

/// A conservative default TTL for a claim that doesn't specify one. `daemon/src/
/// enrichment.rs`'s scheduler wiring (ADR-061 Phase D2) is the intended real
/// caller and will pass its own configured `lease_ttl_secs`; this default only
/// covers a manually-issued or test claim that omits it.
const DEFAULT_LEASE_TTL_SECS: u64 = 120;

pub(super) async fn handle_kb_claim_lease(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            )
        }
    };
    let op_kind = match params["op_kind"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'op_kind' field".to_string()),
            )
        }
    };
    let ttl_secs = params["ttl_secs"]
        .as_u64()
        .unwrap_or(DEFAULT_LEASE_TTL_SECS);

    // Any Editor may claim the lease — not owner-only (unlike `kb/collection_op`'s
    // Manage gate), since enrichment/embedding work is an ordinary editing
    // capability, not KB governance.
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Edit, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(m)) | Err(m) => {
            return JsonRpcResponse::error(id, McpError::internal_error(m))
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("not authorized to edit KB '{kb_id}'")),
            )
        }
    }
    let holder_fp = auth_principal.unwrap_or("").to_string();

    let mut coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
    };
    let now = now_unix();
    let update = coll.claim_lease(&op_kind, &holder_fp, ttl_secs, now);
    if !update.is_empty() {
        if let Err(e) =
            persist_and_broadcast_collection(doc_store, broadcaster, session_id, &kb_id, &update)
                .await
        {
            return JsonRpcResponse::error(id, McpError::internal_error(e));
        }
    }

    // Report who ACTUALLY holds the lease now, not an assumed "you got it" — the
    // caller may have lost the tiebreak (empty delta above) or already renewed.
    match coll.current_lease(&op_kind, now) {
        Some(lease) => {
            info!(
                session = session_id,
                kb_id = %kb_id,
                op_kind = %op_kind,
                held = (lease.holder_fp == holder_fp),
                "kb/claim_lease: complete"
            );
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "kb_id": kb_id,
                    "op_kind": op_kind,
                    "held": lease.holder_fp == holder_fp,
                    "holder_fp": lease.holder_fp,
                    "claimed_at": lease.claimed_at,
                    "ttl_secs": lease.lease_ttl_secs,
                    "generation": lease.generation,
                }),
            )
        }
        None => JsonRpcResponse::error(
            id,
            McpError::internal_error("lease claim produced no current holder".to_string()),
        ),
    }
}

/// ADR-033 Decision item 1 ("checked at write time, not only at acquisition") —
/// mirrors `enforce_epoch_fence_with_coll`'s contract at the lease-generation
/// dimension instead of the per-member authorization epoch. A caller captures
/// `LeaseClaim::generation` at claim time; before committing the bulk operation's
/// results, it re-checks here. `Err` ⇒ someone else was granted the lease in the
/// meantime (this daemon's TTL lapsed and lost the race, or a higher-fingerprint
/// peer preempted it) — the caller must discard its batch, not commit it.
///
/// `#[allow(dead_code)]`: ships in this PR (ADR-061 Phase D1) fully implemented
/// and directly unit-tested (`collab_handler_lease_race_tests.rs`), but its real
/// caller — `run_enrichment_sweep`'s commit path — is ADR-061 Phase D2 (issue
/// #420's second half), landing as a follow-up PR. Named explicitly rather than
/// silently deferred, matching this codebase's own precedent for a correctly-wired
/// but not-yet-connected scaffold (see ADR-034's Phase D3 relationship-baking note).
#[allow(dead_code)]
pub(super) fn enforce_lease_generation_fence(
    coll: &KbCollectionDoc,
    op_kind: &str,
    my_holder_fp: &str,
    my_generation: u64,
    now: u64,
) -> Result<(), String> {
    match coll.current_lease(op_kind, now) {
        Some(lease) if lease.holder_fp == my_holder_fp && lease.generation == my_generation => {
            Ok(())
        }
        Some(lease) => Err(format!(
            "lease for '{op_kind}' has moved on: now generation {} held by {} \
             (this batch was authored under generation {my_generation} as {my_holder_fp}); \
             discard, do not commit",
            lease.generation, lease.holder_fp
        )),
        None => Err(format!(
            "lease for '{op_kind}' expired with no current holder; discard, do not commit"
        )),
    }
}
