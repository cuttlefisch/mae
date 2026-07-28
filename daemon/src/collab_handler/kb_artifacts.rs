//! `kb/fetch_artifact` — the ADR-034 cross-peer artifact-sharing RPC (ADR-061
//! Phase D3). Serves this daemon's locally-cached embedding vector for a
//! given `(kb_id, content_hash, model, chunk_version)` key to a requesting
//! peer, so the peer can skip recomputing an embedding for content it
//! already trusts (KB membership) and whose model pin matches.
//!
//! Gated `KbOp::Read` (any member, including Viewer) — reading a derived
//! artifact is no more sensitive than reading the content it was derived
//! from; a KB member already has full read access to the plaintext (or the
//! wrapped content key, for an E2E KB) this vector was computed over.
//!
//! Membership-gating is complete mediation for the trust decision ADR-034
//! names ("an artifact offered by a non-member is ignored"): a non-member is
//! denied at the SAME `kb_access` gate every other read path already uses,
//! not a second, parallel check that could drift out of sync with it.
//!
//! Model-pin-mismatch handling (ADR-034: "a peer fetching with a mismatched
//! pin recomputes locally") is deliberately the REQUESTER's own decision —
//! this daemon simply reports what it has cached under the requested
//! `(model, chunk_version)` key; a peer that gets `has_artifact: false` (or
//! a value under a key it doesn't trust) falls back to local recompute on
//! its own, no server-side special-casing needed.

use super::*;

pub(super) async fn handle_kb_fetch_artifact(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    artifact_store: &dyn crate::artifact_store::ArtifactStore,
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
    let content_hash = match params["content_hash"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'content_hash' field".to_string()),
            )
        }
    };
    let model = match params["model"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'model' field".to_string()),
            )
        }
    };
    let chunk_version = params["chunk_version"].as_i64().unwrap_or(0);

    // ADR-034: membership-gated, same as every other read path. A non-member
    // (or a KB this daemon doesn't even have a collection doc for) is denied
    // here — no artifact is ever served past this point without it.
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Read, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(m)) | Err(m) => {
            return JsonRpcResponse::error(id, McpError::internal_error(m))
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("not authorized to read KB '{kb_id}'")),
            )
        }
    }

    // ADR-034: only served at all if this KB's owner/coordinator opted in.
    let coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
    };
    if !coll.share_derived_artifacts() {
        return JsonRpcResponse::success(
            id,
            serde_json::json!({
                "kb_id": kb_id,
                "has_artifact": false,
                "reason": "share_derived_artifacts is disabled for this KB",
            }),
        );
    }

    match artifact_store
        .get_cached_embedding(&kb_id, &content_hash, &model, chunk_version)
        .await
    {
        Ok(Some(vector)) => JsonRpcResponse::success(
            id,
            serde_json::json!({
                "kb_id": kb_id,
                "has_artifact": true,
                "model": model,
                "chunk_version": chunk_version,
                "vector": vector,
            }),
        ),
        Ok(None) => JsonRpcResponse::success(
            id,
            serde_json::json!({
                "kb_id": kb_id,
                "has_artifact": false,
                "reason": "no cached artifact for this content_hash/model/chunk_version",
            }),
        ),
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
    }
}
