//! KB membership/lifecycle doc-request handlers (`kb/register`, `kb/list`,
//! `kb/unregister`, `kb/join`, `kb/leave`, `kb/add_member`/`kb/remove_member`,
//! `kb/approve_member`, `kb/list_pending`), split out of
//! `collab_handler/mod.rs`'s `handle_doc_request_inner` match (pure code
//! motion — see that module's doc comment).

use super::*;

// --- KB protocol methods ---
pub(super) async fn handle_kb_register(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("kb_id", "missing 'kb_id' field".to_string()),
            );
        }
    };
    let name = params["name"].as_str().unwrap_or("").to_string();
    let node_count = params["node_count"].as_u64().unwrap_or(0);
    info!(session = session_id, kb_id = %kb_id, name = %name, node_count, "kb/register");

    // Store KB metadata in a collection doc address.
    let doc_name = format!("kbc:{kb_id}");
    session_docs.insert(doc_name.clone());
    broadcaster
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .subscribe_doc(session_id, &doc_name);

    // Store metadata as a simple JSON doc (not a full CRDT — lightweight registry).
    let meta = serde_json::json!({
        "kb_id": kb_id,
        "name": name,
        "node_count": node_count,
        "registered_by": session_id,
    });
    doc_store.set_kb_meta(&kb_id, meta.clone()).await;
    JsonRpcResponse::success(
        id,
        serde_json::json!({ "kb_id": kb_id, "registered": true }),
    )
}

/// List the KBs this caller may read.
///
/// @ai-caution: [kb-scoping] (#653) This MUST stay principal-filtered. It used to
/// return `list_kb_metas()` wholesale with no principal and no `kb_access` call, so
/// any client authenticated to the collab listener learned the id, name and node
/// count of **every** KB co-hosted on the daemon — including tenants it has no
/// membership in. On the multi-tenant daemon ADR-060 describes, that is a roster
/// leak, and it also hands an attacker the exact `kb_id` values the cross-KB
/// content paths key on (#571, #718).
///
/// Only `Allow` is listed. `AllowAutoJoin` (a permissive-policy KB the caller is
/// not yet a member of) is deliberately **excluded**: making every permissive KB
/// on a shared host enumerable is the same leak in a smaller costume, and nothing
/// needs it — a client that knows a `kb_id` can still `kb/join` directly. If
/// discovery of joinable KBs is wanted later, add it as its own method with its own
/// decision, rather than widening this one.
///
/// Unauthenticated callers are unaffected: `kb_access` returns `Allow` for a `None`
/// principal *before* it loads any collection, so a single-user daemon with auth off
/// sees exactly what it saw before, at no extra cost.
pub(super) async fn handle_kb_list(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    id: serde_json::Value,
) -> JsonRpcResponse {
    let kbs = doc_store.list_kb_metas().await;
    let mut visible = Vec::with_capacity(kbs.len());
    for meta in kbs {
        // A meta row with no `kb_id` cannot be authorized, so it cannot be listed.
        // Fail closed rather than treating an unreadable row as public.
        let Some(kb_id) = meta.get("kb_id").and_then(|v| v.as_str()) else {
            warn!("kb/list: skipping KB meta with no kb_id");
            continue;
        };
        match kb_access(doc_store, kb_id, auth_principal, KbOp::Read, transport).await {
            Ok(AccessDecision::Allow) => visible.push(meta),
            Ok(_) => {}
            Err(e) => {
                // Denied-by-error is still denied: omit it.
                warn!(kb_id, error = %e, "kb/list: access check failed, omitting KB");
            }
        }
    }
    JsonRpcResponse::success(id, serde_json::json!({ "kbs": visible }))
}

pub(super) async fn handle_kb_unregister(
    doc_store: &DocStore,
    session_id: u64,
    session_docs: &mut HashSet<String>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("kb_id", "missing 'kb_id' field".to_string()),
            );
        }
    };
    info!(session = session_id, kb_id = %kb_id, "kb/unregister");
    doc_store.remove_kb_meta(&kb_id).await;
    let doc_name = format!("kbc:{kb_id}");
    session_docs.remove(&doc_name);
    JsonRpcResponse::success(
        id,
        serde_json::json!({ "kb_id": kb_id, "unregistered": true }),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_kb_join(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    auth_pubkey: Option<&[u8; 32]>,
    session_docs: &mut HashSet<String>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let who = (session_id, Attribution::session(auth_principal, transport));
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("kb_id", "missing 'kb_id' field".to_string()),
            );
        }
    };
    info!(session = session_id, kb_id = %kb_id, "kb/join");

    // ADR-041 (#158 I1): the joiner's PUBLISHED X25519 wrap key (hex), sent in the
    // request — the daemon can't derive it (it's not the ed25519 session key), so
    // the joiner publishes it for the owner to wrap the content key to on approval.
    let join_wrap_pubkey: Option<[u8; 32]> = params["wrap_pubkey"]
        .as_str()
        .and_then(|h| hex::decode(h).ok())
        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());

    // #255: a mesh relay (an authorized peer) may FORWARD its local editor members' pending
    // join requests here, each with the wrap pubkey that member published to it — so the
    // owner can wrap the E2E content key to them on approval (the member can't reach the
    // owner directly over the mesh). A pending is a self-service request the owner still
    // explicitly approves, so recording a forwarded one is not privileged. Idempotent: skip
    // anyone already a member or already pending WITH a wrap key (don't re-broadcast on the
    // dialer's retry loop).
    record_forwarded_pending(doc_store, broadcaster, who, &kb_id, params).await;

    // ADR-018 access gate (complete mediation): member → join; non-member
    // resolved by the KB's join policy (restrictive/invite/permissive).
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Join, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::AllowAutoJoin) => {
            // permissive: auto-grant the least-privilege role (viewer).
            if let (Some(principal), Ok(mut coll)) =
                (auth_principal, load_collection(doc_store, &kb_id).await)
            {
                let update = coll.upsert_member(
                    principal,
                    auth_label.unwrap_or(principal),
                    SyncRole::Viewer,
                );
                if let Err(e) =
                    persist_and_broadcast_collection(doc_store, broadcaster, who, &kb_id, &update)
                        .await
                {
                    warn!(session = session_id, kb_id = %kb_id, principal = %principal, error = %e, "kb/join: failed to persist the auto-join membership — the join response below still proceeds against the pre-upsert collection state");
                }
            }
        }
        Ok(AccessDecision::Pending) => {
            if let (Some(principal), Ok(mut coll)) =
                (auth_principal, load_collection(doc_store, &kb_id).await)
            {
                let update = coll.add_pending(
                    principal,
                    auth_label.unwrap_or(principal),
                    &now_stamp(),
                    auth_pubkey,
                    join_wrap_pubkey.as_ref(),
                );
                // A client told "status: pending" believes their join request is durably
                // recorded and will show up for the owner to approve. If the persist
                // fails, that belief is false — the request only lived in this
                // in-memory `coll` and is gone the moment this session ends. Report the
                // failure instead of a false pending success.
                if let Err(e) =
                    persist_and_broadcast_collection(doc_store, broadcaster, who, &kb_id, &update)
                        .await
                {
                    warn!(session = session_id, kb_id = %kb_id, principal = %principal, error = %e, "kb/join: failed to persist the pending join request");
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_fault(format!(
                            "failed to record the pending join request: {e}"
                        )),
                    );
                }
            }
            info!(session = session_id, kb_id = %kb_id, principal = ?auth_principal, "kb/join: pending");
            // No `epoch`: a pending requester is not a member, so it has none.
            return JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "kb_id": kb_id, "status": "pending", "principal": auth_principal,
                }),
            );
        }
        Ok(AccessDecision::Deny(msg)) | Err(msg) => {
            warn!(session = session_id, kb_id = %kb_id, reason = %msg, "kb/join denied");
            return JsonRpcResponse::error(id, msg.into());
        }
    }

    // Read the collection doc.
    let collection_doc = format!("kbc:{kb_id}");
    let (collection_state, _sv) = match doc_store.encode_state_and_sv(&collection_doc).await {
        Ok(pair) => pair,
        Err(e) => {
            let kind = match e {
                crate::storage::StorageError::DurableDocMissing(_) => RefusalKind::KbNotFound,
                _ => RefusalKind::CollectionUnavailable,
            };
            let message = format!("kb not found or failed to read collection: {e}");
            return JsonRpcResponse::error(id, Refusal::new(kind, message).into());
        }
    };
    session_docs.insert(collection_doc.clone());
    // ADR-020 liveness: the joining member is a connected client of the docs.
    let _ = doc_store.track_client_connect(&collection_doc).await;
    {
        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
        bc.subscribe_doc(session_id, &collection_doc);
        // Subscribe to sync_update AS OF the join snapshot, so the owner's
        // edits made between this snapshot and the member's separate
        // notifications/subscribe are still pushed (no missed-edit window).
        bc.add_event_sub(session_id, "sync_update");
    }

    // Parse collection to get the list of node IDs belonging to this KB, and the
    // caller's authorization epoch in it.
    let (node_ids, epoch) = match KbCollectionDoc::from_bytes(&collection_state) {
        Ok(coll) => {
            let ids: Vec<String> = coll.list_nodes().into_iter().map(|(id, _)| id).collect();
            (
                ids,
                join_epoch(doc_store, &kb_id, &coll, auth_principal).await,
            )
        }
        Err(e) => {
            warn!(session = session_id, kb_id = %kb_id, error = %e,
                        "kb/join: failed to parse collection doc, falling back to empty node list");
            (Vec::new(), None)
        }
    };

    let joined = join_node_states(
        doc_store,
        broadcaster,
        session_id,
        session_docs,
        &kb_id,
        &node_ids,
        params,
    )
    .await;

    info!(
        session = session_id, kb_id = %kb_id, node_count = joined.nodes.len(),
        reconcile_mode = joined.reconcile_mode, diff_count = joined.diff_count,
        "kb/join: complete"
    );
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "kb_id": kb_id,
            "collection_state": update_to_base64(&collection_state),
            "nodes": joined.nodes,
            "principal": auth_principal,
            "epoch": epoch,
        }),
    )
}

/// The caller's ADR-023 authorization epoch in this KB, as `kb/join` reports
/// it: the SAME value the write fence (`enforce_epoch_fence`) compares
/// against, from the same function, so a client that authors under
/// `derive_kb_client_id(principal, epoch)` is never fenced for a stale
/// epoch it was just handed. Epochs are per member, not per KB.
///
/// `None` for an unauthenticated (`auth.mode = none`) session: there is no
/// principal to derive an epoch for, and the fence does not run for one.
async fn join_epoch(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
    principal: Option<&str>,
) -> Option<u64> {
    match principal {
        Some(p) => Some(kb_member_epoch(doc_store, kb_id, coll, p).await),
        None => None,
    }
}

/// The per-node payload of a `kb/join` reply.
struct JoinedNodes {
    nodes: Vec<serde_json::Value>,
    diff_count: usize,
    reconcile_mode: bool,
}

/// Fetch the nodes the collection lists (not all `kb:` docs) and subscribe the
/// session to each. ADR-022: when the member sent per-node state vectors
/// (`node_svs`), reply with an incremental DIFF per node so it can reconcile
/// (merge, no clobber) instead of adopting a full snapshot; absent (old editor,
/// or a first-ever join) → full state, as before.
async fn join_node_states(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
    kb_id: &str,
    node_ids: &[String],
    params: &serde_json::Value,
) -> JoinedNodes {
    let member_svs: std::collections::HashMap<String, Vec<u8>> = params["node_svs"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    let id = e["id"].as_str()?;
                    let sv = base64_to_update(e["sv"].as_str()?).ok()?;
                    Some((id.to_string(), sv))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut joined = JoinedNodes {
        nodes: Vec::new(),
        diff_count: 0,
        reconcile_mode: !member_svs.is_empty(),
    };
    for node_id in node_ids {
        let doc_name = mae_sync::kb_node_doc_name(kb_id, node_id);
        // Member sent an SV for this node → send only the ops it lacks.
        let encoded = match member_svs.get(node_id) {
            Some(member_sv) => doc_store
                .encode_diff_and_sv(&doc_name, member_sv)
                .await
                .map(|(diff, sv)| (Some(diff), None, sv)),
            None => doc_store
                .encode_state_and_sv(&doc_name)
                .await
                .map(|(state, sv)| (None, Some(state), sv)),
        };
        let (diff, state, sv) = match encoded {
            Ok(triple) => triple,
            Err(e) => {
                warn!(session = session_id, doc = %doc_name, error = %e, "kb/join: failed to read node doc");
                continue;
            }
        };
        session_docs.insert(doc_name.clone());
        let _ = doc_store.track_client_connect(&doc_name).await;
        broadcaster
            .lock()
            .unwrap()
            .subscribe_doc(session_id, &doc_name);
        // Always carry the daemon's SV so the member can compute its
        // local-ahead diff; carry `diff` (incremental) or `state`
        // (full) depending on whether the member sent an SV.
        let mut obj = serde_json::Map::new();
        obj.insert("id".into(), serde_json::json!(node_id));
        obj.insert("sv".into(), serde_json::json!(update_to_base64(&sv)));
        if let Some(diff) = diff {
            joined.diff_count += 1;
            obj.insert("diff".into(), serde_json::json!(update_to_base64(&diff)));
        }
        if let Some(state) = state {
            obj.insert("state".into(), serde_json::json!(update_to_base64(&state)));
        }
        joined.nodes.push(serde_json::Value::Object(obj));
    }
    joined
}

/// #255: record the pending join requests a mesh relay forwarded on its local
/// members' behalf (see the call site in [`handle_kb_join`]). Best-effort: a
/// failure is logged and the join proceeds.
async fn record_forwarded_pending(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    who: (u64, Attribution<'_>),
    kb_id: &str,
    params: &serde_json::Value,
) {
    let session_id = who.0;
    let kb_id = kb_id.to_string();
    if let Some(arr) = params["pending_members"]
        .as_array()
        .filter(|a| !a.is_empty())
    {
        if let Ok(mut coll) = load_collection(doc_store, &kb_id).await {
            let members: HashSet<String> = coll
                .member_roles()
                .into_iter()
                .map(|m| m.fingerprint)
                .collect();
            let keyed_pending: HashSet<String> = coll
                .pending()
                .into_iter()
                .filter(|p| p.wrap_pubkey.is_some())
                .map(|p| p.fingerprint)
                .collect();
            for pm in arr {
                let Some(fp) = pm["fp"].as_str() else {
                    continue;
                };
                if members.contains(fp) || keyed_pending.contains(fp) {
                    continue;
                }
                let hex32 = |v: &serde_json::Value| -> Option<[u8; 32]> {
                    v.as_str()
                        .and_then(|h| hex::decode(h).ok())
                        .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok())
                };
                let wrap = hex32(&pm["wrap_pubkey"]);
                let pk = hex32(&pm["pubkey"]);
                let label = pm["label"].as_str().unwrap_or(fp);
                let update = coll.add_pending(fp, label, &now_stamp(), pk.as_ref(), wrap.as_ref());
                match persist_and_broadcast_collection(doc_store, broadcaster, who, &kb_id, &update)
                    .await
                {
                    Ok(_) => {
                        info!(session = session_id, kb_id = %kb_id, member = %fp, "kb/join: recorded a forwarded pending member (#255 mesh key delivery)");
                    }
                    Err(e) => {
                        warn!(session = session_id, kb_id = %kb_id, member = %fp, error = %e, "kb/join: failed to persist a forwarded pending member (#255 mesh key delivery) — not recorded");
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_kb_add_remove_member(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    transport: Transport,
    method: &str,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let who = (session_id, Attribution::session(auth_principal, transport));
    let add = method == "kb/add_member";
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("kb_id", "missing 'kb_id' field".to_string()),
            );
        }
    };
    // `member` is now a PRINCIPAL (key fingerprint), not a label.
    let member = match params["member"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("member", "missing 'member' field".to_string()),
            );
        }
    };
    let role = params["role"]
        .as_str()
        .and_then(SyncRole::parse)
        .unwrap_or(SyncRole::Editor);
    // ADR-018: owner-only (Manage). The verified principal is the subject.
    let decision = kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await;
    if let Err(r) = require_allow(decision, || {
        format!("not authorized to manage KB '{kb_id}'")
    }) {
        return JsonRpcResponse::error(id, r.into());
    }
    let label = params["label"].as_str().unwrap_or("");
    let change = MembershipChange {
        member: &member,
        add,
        role,
        label,
    };
    match apply_membership_change(doc_store, broadcaster, who, &kb_id, change).await {
        Ok(result) => {
            info!(session = session_id, kb_id = %kb_id, member = %member, add, role = role.as_str(), "kb membership change");
            JsonRpcResponse::success(id, result)
        }
        Err(e) => JsonRpcResponse::error(id, e),
    }
}

/// One membership change, as `kb/add_member` / `kb/remove_member` make it once
/// access is granted.
pub struct MembershipChange<'a> {
    pub member: &'a str,
    pub add: bool,
    pub role: SyncRole,
    pub label: &'a str,
}

/// The single membership write path (the RPC and the operator command both
/// use it): update the collection's `member_roles`, persist and broadcast it,
/// then mirror the change into the signed op-log when this daemon owns the KB.
/// The caller has already decided the change is allowed.
async fn apply_membership_change(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    who: (u64, Attribution<'_>),
    kb_id: &str,
    ch: MembershipChange<'_>,
) -> Result<serde_json::Value, McpError> {
    let mut coll = load_collection(doc_store, kb_id)
        .await
        .map_err(McpError::from)?;
    if !ch.add && coll.owner() == ch.member {
        return Err(McpError::cannot_remove_owner(format!(
            "cannot remove the owner of KB '{kb_id}'"
        )));
    }
    let update = if ch.add {
        coll.upsert_member(ch.member, ch.label, ch.role)
    } else {
        coll.remove_principal(ch.member)
    };
    persist_and_broadcast_collection(doc_store, broadcaster, who, kb_id, &update)
        .await
        .map_err(McpError::internal_fault)?;
    // ADR-026: mirror the change into the signed op-log (owned KBs).
    let (action, signed_role) = if ch.add {
        (MembershipAction::Admit, Some(ch.role))
    } else {
        (MembershipAction::Remove, None)
    };
    let appended = append_signed_membership(
        doc_store,
        broadcaster,
        who,
        kb_id,
        &mut coll,
        action,
        ch.member,
        signed_role,
        false,
        None,
    )
    .await;
    // The legacy `member_roles` mutation above DID persist, so this is a real
    // success — but if the peer-verifiable signed op-log did not get the
    // matching op, say so rather than implying both landed (audit #589.4).
    let mut result = serde_json::json!({ "kb_id": kb_id, "member": ch.member, "added": ch.add });
    result["signed_oplog"] = serde_json::json!(appended.failure().is_none());
    if let Some(e) = appended.failure() {
        result["warning"] = serde_json::json!(format!(
            "membership applied locally, but the signed op-log was not updated \
             ({e}) — peers verifying membership from the op-log will not see this \
             change"
        ));
    }
    Ok(result)
}

/// What the operator's `mae-daemon kb member` command asks for.
pub enum OperatorMemberOp<'a> {
    Add {
        member: &'a str,
        role: SyncRole,
        label: &'a str,
    },
    Remove {
        member: &'a str,
    },
}

/// `mae-daemon kb member add|remove`: the operator changes membership of a KB
/// THIS daemon owns (a seeded KB, until ADR-098's binding), through the same
/// write path as `kb/add_member`, signed as the daemon (the owner).
///
/// Refused, with nothing written: a KB another identity owns (its owner
/// manages it), a role other than viewer or editor (ownership is not
/// transferred here), the owner itself as the member, a member that is not a
/// key fingerprint, and a store without the daemon's signer (the op-log would
/// silently miss the change).
///
/// @ai-caution: [security] Operator-only, like `token mint` and `kb seed`: it
/// acts as the KB's owner. It must never become an MCP tool or an RPC an agent
/// principal can reach (CLAUDE.md principle #16).
pub async fn operator_change_membership(
    doc_store: &DocStore,
    operator: &mae_mcp::identity::Identity,
    kb_id: &str,
    op: OperatorMemberOp<'_>,
) -> Result<serde_json::Value, String> {
    let coll = load_collection(doc_store, kb_id)
        .await
        .map_err(|e| format!("KB '{kb_id}': {}", McpError::from(e).message))?;
    let owner = coll.owner();
    if owner != operator.fingerprint() {
        return Err(format!(
            "KB '{kb_id}' is owned by {owner}, not this daemon ({}); its owner manages its \
             membership",
            operator.fingerprint()
        ));
    }
    if doc_store.signer().map(|s| s.fingerprint()) != Some(operator.fingerprint()) {
        return Err("the store has no signer for this daemon's identity".into());
    }
    let (member, add, role, label) = match op {
        OperatorMemberOp::Add {
            member,
            role,
            label,
        } => (member, true, role, label),
        OperatorMemberOp::Remove { member } => (member, false, SyncRole::Viewer, ""),
    };
    if !member.starts_with("SHA256:") {
        return Err(format!("'{member}' is not a key fingerprint (SHA256:…)"));
    }
    if member == owner {
        return Err("the owner's own membership is not changed here".into());
    }
    if add && !matches!(role, SyncRole::Viewer | SyncRole::Editor) {
        return Err("the role must be viewer or editor".into());
    }
    let broadcaster: SharedBroadcaster = Default::default();
    let who = (0, Attribution::Internal(InternalWriter::OperatorMembership));
    let change = MembershipChange {
        member,
        add,
        role,
        label,
    };
    let result = apply_membership_change(doc_store, &broadcaster, who, kb_id, change)
        .await
        .map_err(|e| e.message)?;
    if result["signed_oplog"] != true {
        return Err(format!(
            "membership applied to the roles, but NOT to the signed op-log: {}",
            result["warning"]
        ));
    }
    Ok(result)
}

/// `mae-daemon kb member list`: the KB's owner and its members with roles.
pub async fn operator_list_members(
    doc_store: &DocStore,
    kb_id: &str,
) -> Result<(String, Vec<(String, String)>), String> {
    let coll = load_collection(doc_store, kb_id)
        .await
        .map_err(|e| format!("KB '{kb_id}': {}", McpError::from(e).message))?;
    let mut members: Vec<(String, String)> = coll
        .member_roles()
        .into_iter()
        .map(|m| (m.fingerprint, m.role.as_str().to_string()))
        .collect();
    members.sort();
    Ok((coll.owner(), members))
}

pub(super) async fn handle_kb_list_pending(
    doc_store: &DocStore,
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
                McpError::invalid_params("kb_id", "missing 'kb_id' field".to_string()),
            )
        }
    };
    let decision = kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await;
    if let Err(r) = require_allow(decision, || {
        format!("not authorized to manage KB '{kb_id}'")
    }) {
        return JsonRpcResponse::error(id, r.into());
    }
    let coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, e.into()),
    };
    let pending: Vec<_> = coll
        .pending()
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "fingerprint": p.fingerprint,
                "label": p.label,
                "requested_at": p.requested_at,
            })
        })
        .collect();
    JsonRpcResponse::success(
        id,
        serde_json::json!({ "kb_id": kb_id, "pending": pending }),
    )
}

pub(super) async fn handle_kb_approve_member(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let who = (session_id, Attribution::session(auth_principal, transport));
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("kb_id", "missing 'kb_id' field".to_string()),
            )
        }
    };
    let principal = match params["principal"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("principal", "missing 'principal' field".to_string()),
            )
        }
    };
    let role = params["role"]
        .as_str()
        .and_then(SyncRole::parse)
        .unwrap_or(SyncRole::Editor);
    let decision = kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await;
    if let Err(r) = require_allow(decision, || {
        format!("not authorized to manage KB '{kb_id}'")
    }) {
        return JsonRpcResponse::error(id, r.into());
    }
    let mut coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, e.into()),
    };
    let update = coll.approve(&principal, role);
    match persist_and_broadcast_collection(doc_store, broadcaster, who, &kb_id, &update).await {
        Ok(_) => {
            // ADR-026: an approval is a signed Admit in the op-log.
            let appended = append_signed_membership(
                doc_store,
                broadcaster,
                who,
                &kb_id,
                &mut coll,
                MembershipAction::Admit,
                &principal,
                Some(role),
                false,
                None,
            )
            .await;
            info!(session = session_id, kb_id = %kb_id, principal = %principal, role = role.as_str(), "kb/approve_member: complete");
            // Same split as `kb/set_member` above (audit #589.4): the approval
            // itself persisted, the signed mirror may not have.
            let mut result = serde_json::json!({
                "kb_id": kb_id, "principal": principal, "role": role.as_str()
            });
            result["signed_oplog"] = serde_json::json!(appended.failure().is_none());
            if let Some(e) = appended.failure() {
                result["warning"] = serde_json::json!(format!(
                    "approval applied locally, but the signed op-log was not updated ({e}) \
                     — peers verifying membership from the op-log will not see this change"
                ));
            }
            JsonRpcResponse::success(id, result)
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_fault(e)),
    }
}

pub(super) async fn handle_kb_leave(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::invalid_params("kb_id", "missing 'kb_id' field".to_string()),
            );
        }
    };
    info!(session = session_id, kb_id = %kb_id, "kb/leave");

    // Read the collection doc to find which nodes belong to this KB.
    let collection_doc = format!("kbc:{kb_id}");
    let node_ids: Vec<String> = match doc_store.encode_state_and_sv(&collection_doc).await {
        Ok((state, _sv)) => match KbCollectionDoc::from_bytes(&state) {
            Ok(coll) => coll.list_nodes().into_iter().map(|(id, _)| id).collect(),
            Err(e) => {
                warn!(
                    kb_id = %kb_id,
                    error = %e,
                    "kb/leave: collection decode failed; no nodes to unsubscribe"
                );
                Vec::new()
            }
        },
        Err(e) => {
            debug!(
                kb_id = %kb_id,
                error = %e,
                "kb/leave: collection state unavailable; no nodes to unsubscribe"
            );
            Vec::new()
        }
    };

    // Unsubscribe from collection doc.
    session_docs.remove(&collection_doc);
    broadcaster
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unsubscribe_doc(session_id, &collection_doc);

    // Unsubscribe only from this KB's node docs.
    let mut removed_count: u64 = 0;
    for node_id in &node_ids {
        let doc_name = mae_sync::kb_node_doc_name(&kb_id, node_id);
        if session_docs.remove(&doc_name) {
            broadcaster
                .lock()
                .unwrap()
                .unsubscribe_doc(session_id, &doc_name);
            removed_count += 1;
        }
    }

    info!(session = session_id, kb_id = %kb_id, removed_count, "kb/leave: complete");
    JsonRpcResponse::success(id, serde_json::json!({ "kb_id": kb_id, "left": true }))
}
