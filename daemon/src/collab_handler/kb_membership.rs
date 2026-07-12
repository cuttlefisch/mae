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
                McpError::parse_error("missing 'kb_id' field".to_string()),
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

pub(super) async fn handle_kb_list(doc_store: &DocStore, id: serde_json::Value) -> JsonRpcResponse {
    let kbs = doc_store.list_kb_metas().await;
    JsonRpcResponse::success(id, serde_json::json!({ "kbs": kbs }))
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
                McpError::parse_error("missing 'kb_id' field".to_string()),
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
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
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
                let _ = persist_and_broadcast_collection(
                    doc_store,
                    broadcaster,
                    session_id,
                    &kb_id,
                    &update,
                )
                .await;
                info!(session = session_id, kb_id = %kb_id, member = %fp, "kb/join: recorded a forwarded pending member (#255 mesh key delivery)");
            }
        }
    }

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
                let _ = persist_and_broadcast_collection(
                    doc_store,
                    broadcaster,
                    session_id,
                    &kb_id,
                    &update,
                )
                .await;
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
                let _ = persist_and_broadcast_collection(
                    doc_store,
                    broadcaster,
                    session_id,
                    &kb_id,
                    &update,
                )
                .await;
            }
            info!(session = session_id, kb_id = %kb_id, principal = ?auth_principal, "kb/join: pending");
            return JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "status": "pending" }),
            );
        }
        Ok(AccessDecision::Deny(msg)) | Err(msg) => {
            warn!(session = session_id, kb_id = %kb_id, reason = %msg, "kb/join denied");
            return JsonRpcResponse::error(id, McpError::internal_error(msg));
        }
    }

    // Read the collection doc.
    let collection_doc = format!("kbc:{kb_id}");
    let (collection_state, _sv) = match doc_store.encode_state_and_sv(&collection_doc).await {
        Ok(pair) => pair,
        Err(e) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("kb not found or failed to read collection: {e}")),
            );
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

    // Parse collection to get the list of node IDs belonging to this KB.
    let node_ids: Vec<String> = match KbCollectionDoc::from_bytes(&collection_state) {
        Ok(coll) => coll
            .list_nodes()
            .into_iter()
            .map(|(id, _title)| id)
            .collect(),
        Err(e) => {
            warn!(session = session_id, kb_id = %kb_id, error = %e,
                        "kb/join: failed to parse collection doc, falling back to empty node list");
            Vec::new()
        }
    };

    // ADR-022: parse the member's per-node state vectors (optional). When
    // present, reply with an incremental DIFF per node so the member can
    // reconcile (merge, no clobber) instead of adopting a full snapshot.
    // Absent (old editor, or a first-ever join) → full state, as before.
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
    let reconcile_mode = !member_svs.is_empty();

    // Fetch only the nodes listed in the collection (not all kb: docs).
    let mut nodes = Vec::new();
    let mut diff_count = 0usize;
    for node_id in &node_ids {
        let doc_name = format!("kb:{node_id}");
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
        match encoded {
            Ok((diff, state, sv)) => {
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
                    diff_count += 1;
                    obj.insert("diff".into(), serde_json::json!(update_to_base64(&diff)));
                }
                if let Some(state) = state {
                    obj.insert("state".into(), serde_json::json!(update_to_base64(&state)));
                }
                nodes.push(serde_json::Value::Object(obj));
            }
            Err(e) => {
                warn!(session = session_id, doc = %doc_name, error = %e, "kb/join: failed to read node doc");
            }
        }
    }

    info!(
        session = session_id, kb_id = %kb_id, node_count = nodes.len(),
        reconcile_mode, diff_count,
        "kb/join: complete"
    );
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "kb_id": kb_id,
            "collection_state": update_to_base64(&collection_state),
            "nodes": nodes,
        }),
    )
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
    let add = method == "kb/add_member";
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            );
        }
    };
    // `member` is now a PRINCIPAL (key fingerprint), not a label.
    let member = match params["member"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'member' field".to_string()),
            );
        }
    };
    let role = params["role"]
        .as_str()
        .and_then(SyncRole::parse)
        .unwrap_or(SyncRole::Editor);
    // ADR-018: owner-only (Manage). The verified principal is the subject.
    match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
        Ok(AccessDecision::Allow) => {}
        Ok(AccessDecision::Deny(m)) | Err(m) => {
            return JsonRpcResponse::error(id, McpError::internal_error(m));
        }
        Ok(_) => {
            return JsonRpcResponse::error(
                id,
                McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
            );
        }
    }
    let mut coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
    };
    if !add && coll.owner() == member {
        return JsonRpcResponse::error(
            id,
            McpError::internal_error(format!("cannot remove the owner of KB '{kb_id}'")),
        );
    }
    let update = if add {
        coll.upsert_member(&member, params["label"].as_str().unwrap_or(""), role)
    } else {
        coll.remove_principal(&member)
    };
    match persist_and_broadcast_collection(doc_store, broadcaster, session_id, &kb_id, &update)
        .await
    {
        Ok(_) => {
            // ADR-026: mirror the change into the signed op-log (owned KBs).
            let (action, signed_role) = if add {
                (MembershipAction::Admit, Some(role))
            } else {
                (MembershipAction::Remove, None)
            };
            append_signed_membership(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &mut coll,
                action,
                &member,
                signed_role,
                false,
                None,
            )
            .await;
            info!(session = session_id, kb_id = %kb_id, member = %member, add, role = role.as_str(), "kb membership change");
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "member": member, "added": add }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
    }
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
                McpError::parse_error("missing 'kb_id' field".to_string()),
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
    let coll = match load_collection(doc_store, &kb_id).await {
        Ok(c) => c,
        Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
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
    let kb_id = match params["kb_id"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'kb_id' field".to_string()),
            )
        }
    };
    let principal = match params["principal"].as_str() {
        Some(s) => s.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'principal' field".to_string()),
            )
        }
    };
    let role = params["role"]
        .as_str()
        .and_then(SyncRole::parse)
        .unwrap_or(SyncRole::Editor);
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
    let update = coll.approve(&principal, role);
    match persist_and_broadcast_collection(doc_store, broadcaster, session_id, &kb_id, &update)
        .await
    {
        Ok(_) => {
            // ADR-026: an approval is a signed Admit in the op-log.
            append_signed_membership(
                doc_store,
                broadcaster,
                session_id,
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
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "principal": principal, "role": role.as_str() }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
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
                McpError::parse_error("missing 'kb_id' field".to_string()),
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
        let doc_name = format!("kb:{node_id}");
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
