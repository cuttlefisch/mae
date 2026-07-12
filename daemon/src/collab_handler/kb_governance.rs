//! KB governance doc-request handlers (`kb/set_policy`, `kb/set_governance`,
//! `kb/block_principal`/`kb/unblock_principal`, `kb/blocklist`, `kb/revoke`),
//! split out of `collab_handler/mod.rs`'s `handle_doc_request_inner` match
//! (pure code motion — see that module's doc comment).

use super::*;

pub(super) async fn handle_kb_set_policy(
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
    let policy = match params["policy"].as_str().and_then(JoinPolicy::parse) {
        Some(p) => p,
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("policy must be restrictive|invite|permissive".to_string()),
            )
        }
    };
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(m)) | Err(m) => {
            return JsonRpcResponse::error(id, McpError::internal_error(m))
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
            )
        }
    }
    let mut coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
    };
    let update = coll.set_join_policy(policy);
    match persist_and_broadcast_collection(doc_store, broadcaster, session_id, &kb_id, &update)
        .await
    {
        Ok(_) => {
            info!(session = session_id, kb_id = %kb_id, policy = policy.as_str(), "kb/set_policy: complete");
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "policy": policy.as_str() }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
    }
}

// ADR-039 A2 (#162): manage this daemon's LOCAL self-protection blocklist for a KB.
// Deliberately NOT owner-gated and NOT routed through `kb_access`:
//  - blocking even the owner is the explicit capability (you reach for a local
//    block precisely when you *cannot* get a principal globally removed — e.g. you
//    lack quorum — but want THIS daemon to stop trusting them);
//  - it mutates only local trust, never the synced `kbc:` collection, and is never
//    broadcast — so there is nothing for a peer to observe and no membership to gate on.
// The block is honored at every membership-derivation site (`membership_view_for`),
// so once set it fences the principal at the access gate AND the content paths AND
// the removal derivation (complete mediation). Authz floor: it changes only this
// daemon's local state, so any trusted operator of this daemon may set it (a
// loopback `none` session — already fully trusted by `kb_access` — or an
// authenticated client). Trade-off: an authenticated remote client of a *shared*
// daemon could set a local block (a bounded, reversible local nuisance — never a
// privilege-escalation or data-exposure). Tightening this to the Unix operator
// socket only is a documented hardening follow-up (the handler isn't told the
// socket kind today; `Transport` is Hub/P2p, not Unix/Tcp).
pub(super) async fn handle_kb_block_unblock_principal(
    doc_store: &DocStore,
    session_id: u64,
    auth_principal: Option<&str>,
    method: &str,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let block = method == "kb/block_principal";
    let kb_id = match params["kb_id"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            )
        }
    };
    // Accept `fingerprint` (the canonical name) or `principal` as an alias.
    let principal = match params["fingerprint"]
        .as_str()
        .or_else(|| params["principal"].as_str())
    {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error(
                    "missing 'fingerprint' field (the principal to block)".to_string(),
                ),
            )
        }
    };
    let result = if block {
        doc_store.add_kb_block(&kb_id, &principal).await
    } else {
        doc_store.remove_kb_block(&kb_id, &principal).await
    };
    match result {
        Ok(()) => {
            info!(
                session = session_id,
                operator = ?auth_principal,
                kb_id = %kb_id,
                principal = %principal,
                block,
                "kb local blocklist updated (ADR-039 A2; local-only, not propagated)"
            );
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "kb_id": kb_id,
                    "fingerprint": principal,
                    "blocked": block,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            id,
            McpError::internal_error(format!("blocklist update failed: {e}")),
        ),
    }
}

// ADR-039 A2 (#162): read this daemon's LOCAL blocklist. Read-only introspection
// for the operator's `*KB Sharing*` Blocked view — the ONLY way a client learns
// the local blocklist, since it is never in the synced `kbc:` collection. Returns
// `{ blocklist: { kb_id: [fingerprint, ...] } }`; with `kb_id` it scopes to one.
pub(super) async fn handle_kb_blocklist(
    doc_store: &DocStore,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let all = doc_store.all_kb_blocklists().await;
    let payload = match params["kb_id"].as_str() {
        Some(kb_id) => {
            let one = all.get(kb_id).cloned().unwrap_or_default();
            serde_json::json!({ "blocklist": { kb_id: one } })
        }
        None => serde_json::json!({ "blocklist": all }),
    };
    JsonRpcResponse::success(id, payload)
}

// ADR-026 §A4: set the KB's governance rule (`single-owner` | `quorum:N`),
// recorded as an owner-signed `SetGovernance` op in the membership log so
// every peer derives the same rule. Genesis-owner-only: changing the rule is
// anchored to the trust root (`derive_governance` counts only the anchored
// owner's op), so a non-owner daemon — which can only join, never sign as the
// owner — is rejected with a clear error rather than a silent no-op.
pub(super) async fn handle_kb_set_governance(
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
    let governance = match params["governance"]
        .as_str()
        .and_then(Governance::parse_spec)
    {
        Some(g) => g,
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error(
                    "governance must be single-owner|quorum:N (N>=1)".to_string(),
                ),
            )
        }
    };
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(m)) | Err(m) => {
            return JsonRpcResponse::error(id, McpError::internal_error(m))
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
            )
        }
    }
    let mut coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
    };
    // Only the genesis owner's signature counts toward `derive_governance`;
    // reject up front so the caller never sees a false success.
    match doc_store.signer() {
        Some(s) if s.fingerprint() == coll.owner() && !coll.owner().is_empty() => {}
        _ => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!(
                    "only the owner of KB '{kb_id}' may set its governance"
                )),
            )
        }
    }
    append_signed_membership(
        doc_store,
        broadcaster,
        session_id,
        &kb_id,
        &mut coll,
        MembershipAction::SetGovernance,
        &governance.to_spec(),
        None,
        false,
        None,
    )
    .await;
    info!(session = session_id, kb_id = %kb_id, governance = %governance.to_spec(), "kb/set_governance");
    JsonRpcResponse::success(
        id,
        serde_json::json!({ "kb_id": kb_id, "governance": governance.to_spec() }),
    )
}

// ADR-026 §A4: strong-removal of a member, the m-of-n quorum co-sign
// primitive. Each current owner's daemon contributes one distinct-author
// `Revoke`; under `Quorum{m}` the member is removed once m distinct owners
// have co-signed (under `SingleOwner` a single owner's revoke suffices).
// Authorized at Manage (any current owner) — distinct from `kb/remove_member`
// (genesis-owner-only owned-KB housekeeping).
pub(super) async fn handle_kb_revoke(
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
    let member = match params["member"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'member' field".to_string()),
            )
        }
    };
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(m)) | Err(m) => {
            return JsonRpcResponse::error(id, McpError::internal_error(m))
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
            )
        }
    }
    let mut coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
    };
    match append_signed_revoke(
        doc_store,
        broadcaster,
        session_id,
        &kb_id,
        &mut coll,
        &member,
    )
    .await
    {
        Ok(()) => {
            info!(session = session_id, kb_id = %kb_id, member = %member, "kb/revoke: signed co-removal appended");
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "member": member, "revoked": true }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
    }
}
