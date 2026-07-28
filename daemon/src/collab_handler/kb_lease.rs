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

    match claim_lease_and_report(
        doc_store,
        broadcaster,
        session_id,
        &kb_id,
        &op_kind,
        &holder_fp,
        ttl_secs,
    )
    .await
    {
        Ok(lease) => {
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
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
    }
}

/// Shared claim logic: load the collection, attempt `claim_lease`, persist +
/// broadcast if it produced a delta, then return the ACTUAL current holder
/// (the caller may have lost the tiebreak, in which case the returned claim's
/// `holder_fp` differs from `holder_fp` passed in). Used by both the external
/// `kb/claim_lease` RPC (above, after its `kb_access` gate) and the enrichment
/// scheduler's internal claim (`claim_lease_for_scheduler`, below).
async fn claim_lease_and_report(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    kb_id: &str,
    op_kind: &str,
    holder_fp: &str,
    ttl_secs: u64,
) -> Result<mae_sync::kb::LeaseClaim, String> {
    let mut coll = load_collection(doc_store, kb_id).await?;
    let now = now_unix();
    let update = coll.claim_lease(op_kind, holder_fp, ttl_secs, now);
    if !update.is_empty() {
        persist_and_broadcast_collection(doc_store, broadcaster, session_id, kb_id, &update)
            .await?;
    }
    coll.current_lease(op_kind, now)
        .ok_or_else(|| "lease claim produced no current holder".to_string())
}

/// ADR-061 Phase D2: the enrichment scheduler's own claim, called from
/// `daemon/src/scheduler.rs`'s `run_maintenance_tick` before sweeping a KB
/// that IS collab-shared (a KB with no `collab_id` has nothing to coordinate
/// and never reaches this function at all). Deliberately does **not** go
/// through the `kb_access` gate `handle_kb_claim_lease` uses: that gate
/// authenticates an EXTERNAL network caller, but this is the daemon's own
/// internal maintenance tick operating on data it already has full local
/// access to via `CozoKbStore` directly — there is no "connection" to
/// authenticate (mirrors `kb_governance.rs`'s `handle_kb_block_unblock_
/// principal` precedent for a local, already-trusted-daemon operation).
/// `session_id: 0` is a safe sentinel for `persist_and_broadcast_collection`'s
/// `broadcast_except` — real session ids are allocated from 1 upward
/// (`mae_mcp::session::NEXT_SESSION_ID`), so 0 never excludes a real client.
pub async fn claim_lease_for_scheduler(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    kb_id: &str,
    op_kind: &str,
    holder_fp: &str,
    ttl_secs: u64,
) -> Result<mae_sync::kb::LeaseClaim, String> {
    claim_lease_and_report(
        doc_store,
        broadcaster,
        0,
        kb_id,
        op_kind,
        holder_fp,
        ttl_secs,
    )
    .await
}

/// ADR-033 Decision item 1 ("checked at write time, not only at acquisition") —
/// mirrors `enforce_epoch_fence_with_coll`'s contract at the lease-generation
/// dimension instead of the per-member authorization epoch. A caller captures
/// `LeaseClaim::generation` at claim time; before committing the bulk operation's
/// results, it re-checks here. `Err` ⇒ someone else was granted the lease in the
/// meantime (this daemon's TTL lapsed and lost the race, or a higher-fingerprint
/// peer preempted it) — the caller must discard its batch, not commit it.
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

/// ADR-061 Phase D2: the production [`crate::lease_fence::LeaseFence`] —
/// re-reads the real collection doc from `doc_store` and re-checks the
/// generation fence, right before `run_enrichment_sweep` (the binary crate's
/// `enrichment` module) commits. Constructed by the scheduler with the
/// generation captured at claim time (`claim_lease_for_scheduler`'s returned
/// `LeaseClaim::generation`). Lives here, not in `enrichment.rs`, because this
/// IS library-crate code (`enrichment` is binary-only) — see
/// `crate::lease_fence`'s doc comment for the full crate-boundary rationale.
pub struct DaemonLeaseFence {
    pub doc_store: std::sync::Arc<DocStore>,
    pub kb_id: String,
    pub op_kind: String,
    pub holder_fp: String,
    pub generation: u64,
}

#[async_trait::async_trait]
impl crate::lease_fence::LeaseFence for DaemonLeaseFence {
    async fn check(&self) -> Result<(), String> {
        let coll = load_collection(&self.doc_store, &self.kb_id).await?;
        enforce_lease_generation_fence(
            &coll,
            &self.op_kind,
            &self.holder_fp,
            self.generation,
            now_unix(),
        )
    }
}
