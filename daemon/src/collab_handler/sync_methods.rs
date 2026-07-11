//! `sync/*` doc-request handlers, split out of `collab_handler/mod.rs`'s
//! `handle_doc_request_inner` match (pure code motion — see that module's
//! doc comment). Each function is one former match arm, taking only the
//! subset of the shared fixed parameter list (doc_store, broadcaster,
//! start_time, session_id, auth_label/auth_principal/auth_pubkey,
//! session_docs, transport, id, params) it actually reads.

use super::*;

pub(super) async fn handle_sync_state_vector(
    doc_store: &DocStore,
    session_id: u64,
    auth_principal: Option<&str>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    if let Err(msg) = deny_kb_doc_read(doc_store, &doc_name, auth_principal, transport).await {
        warn!(session = session_id, doc = %doc_name, reason = %msg, "sync/state_vector denied");
        return JsonRpcResponse::error(id, McpError::internal_error(msg));
    }
    match doc_store.state_vector(&doc_name).await {
        Ok(sv) => {
            let sv_b64 = update_to_base64(&sv);
            JsonRpcResponse::success(id, serde_json::json!({ "doc": doc_name, "sv": sv_b64 }))
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_sync_update(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    session_docs: &mut HashSet<String>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    info!(session = session_id, "sync/update: processing");
    let doc_name = match params["doc"].as_str() {
        Some(d) => d.to_string(),
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'doc' field".to_string()),
            );
        }
    };
    // ADR-018 membership-smuggling defense: a raw write to a collection doc
    // (`kbc:`) is owner-only.
    if let Err(e) = deny_collection_smuggling(doc_store, &doc_name, auth_principal, transport).await
    {
        warn!(session = session_id, doc = %doc_name, reason = %e, "sync/update denied (collection smuggling)");
        return JsonRpcResponse::error(id, McpError::internal_error(e));
    }
    // Track this doc for disconnect cleanup and doc-scoped broadcast filtering.
    if session_docs.insert(doc_name.clone()) {
        // First interaction — track client connect + subscribe to doc events.
        let _ = doc_store.track_client_connect(&doc_name).await;
        broadcaster
            .lock()
            .unwrap()
            .subscribe_doc(session_id, &doc_name);
        debug!(session = session_id, doc = %doc_name, "sync/update: first interaction, tracking connect");
    }
    let update_b64 = match params["update"].as_str() {
        Some(s) => s,
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'update' field".to_string()),
            );
        }
    };
    let update_bytes = match base64_to_update(update_b64) {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error(format!("invalid base64: {e}")),
            );
        }
    };
    let max_update = doc_store.max_update_size();
    if update_bytes.len() > max_update {
        return JsonRpcResponse::error(
            id,
            McpError::parse_error(format!(
                "update too large: {} bytes (max {})",
                update_bytes.len(),
                max_update
            )),
        );
    }
    let client_id = params["client_id"].as_u64();

    // ADR-036 §D3 (B→A): a content op arriving over the mesh — a peer daemon's
    // forwarded edit — is re-verified against THIS KB's membership before
    // apply, because the relaying peer is untrusted (even a member's *daemon*
    // could lie). The joiner's dialer sends `kb_id`; require-signed on P2p, the
    // hub accepts legacy unsigned during migration. Same shared check as the
    // dialer's inbound path. On success the verified header is carried onward.
    let mut sync_content_header: Option<serde_json::Value> = None;
    if let Some(kb_id) = params.get("kb_id").and_then(|v| v.as_str()) {
        match verify_relayed_content_op(
            doc_store,
            kb_id,
            &doc_name,
            &update_bytes,
            params.get("content_header"),
            matches!(transport, Transport::P2p),
        )
        .await
        {
            Ok(verified) => sync_content_header = verified,
            Err(e) => {
                warn!(session = session_id, doc = %doc_name, reason = %e, "sync/update: SIGNED CONTENT OP REJECTED (ADR-036)");
                return JsonRpcResponse::error(id, McpError::internal_error(e));
            }
        }
    }

    // #169 M1: a `kb:{node}` write arriving via `sync/update` must carry its `kb_id`
    // and clear the ADR-023 epoch fence — the SAME mediation as `kb/node_update` and
    // the mesh dialer. Before this, a bare `kb:` `sync/update` (no `kb_id`) skipped
    // `verify_relayed_content_op` (it's gated on `kb_id`), `kb_access`, AND the fence,
    // writing the node CRDT with no role/signature/epoch check, then broadcasting.
    // Text buffers (non-`kb:` docs) are unaffected.
    if let Some(node_id) = doc_name.strip_prefix("kb:") {
        match params.get("kb_id").and_then(|v| v.as_str()) {
            Some(kb_id) => {
                // Fence on the VERIFIED header author (a relayed op's true author —
                // never the connection principal), mirroring the dialer's #157 N1 path.
                if let Some(author) = sync_content_header
                    .as_ref()
                    .and_then(|h| h.get("author").and_then(|a| a.as_str()))
                {
                    if let Err(reason) = enforce_epoch_fence(
                        doc_store,
                        kb_id,
                        node_id,
                        &doc_name,
                        &update_bytes,
                        author,
                    )
                    .await
                    {
                        warn!(session = session_id, doc = %doc_name, %reason, "sync/update: kb node REJECTED (epoch fence — #169 M1)");
                        return JsonRpcResponse::error(id, McpError::internal_error(reason));
                    }
                }
            }
            None => {
                warn!(session = session_id, doc = %doc_name, "sync/update: kb node write WITHOUT kb_id — REJECTED (would bypass membership + epoch fence, #169 M1)");
                return JsonRpcResponse::error(
                    id,
                    McpError::internal_error(
                        "kb node writes via sync/update must carry kb_id".to_string(),
                    ),
                );
            }
        }
    }

    match doc_store
        .apply_update(&doc_name, &update_bytes, client_id)
        .await
    {
        Ok(result) => {
            // Broadcast to other subscribers (skip sender to avoid echo).
            {
                let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                bc.broadcast_except(
                    &EditorEvent::SyncUpdate {
                        buffer_name: doc_name.clone(),
                        update_base64: update_to_base64(&result.update),
                        wal_seq: result.wal_seq,
                        content_header: sync_content_header,
                    },
                    session_id,
                );
            }
            info!(session = session_id, doc = %doc_name, wal_seq = result.wal_seq, update_len = result.update.len(), "sync/update: applied and broadcast");
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "doc": doc_name,
                    "wal_seq": result.wal_seq,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_sync_awareness(
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_label: Option<&str>,
    session_docs: &mut HashSet<String>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    // Pure relay: broadcast awareness to all other clients on same doc.
    // No persistence — awareness is ephemeral.
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    let state = &params["state"];
    debug!(
        session = session_id,
        doc = %doc_name,
        "sync/awareness: relaying"
    );
    // Track doc for cleanup and doc-scoped broadcast filtering.
    session_docs.insert(doc_name.clone());
    {
        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
        bc.subscribe_doc(session_id, &doc_name);
        bc.broadcast_except(
            &EditorEvent::AwarenessUpdate {
                doc_id: doc_name.clone(),
                client_id: session_id,
                // Strict binding: authenticated peer's cursor label wins.
                user_name: auth_label.map(str::to_string).unwrap_or_else(|| {
                    state
                        .get("user_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string()
                }),
                cursor_row: state
                    .get("cursor_row")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
                cursor_col: state
                    .get("cursor_col")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize,
                selection: state.get("selection").and_then(|v| {
                    let arr = v.as_array()?;
                    if arr.len() == 4 {
                        Some((
                            arr[0].as_u64()? as usize,
                            arr[1].as_u64()? as usize,
                            arr[2].as_u64()? as usize,
                            arr[3].as_u64()? as usize,
                        ))
                    } else {
                        None
                    }
                }),
                mode: state
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("normal")
                    .to_string(),
            },
            session_id,
        );
    }
    // Awareness is a notification (no `id` field), so if it has an id,
    // respond with a simple ack.
    JsonRpcResponse::success(id, serde_json::json!({ "doc": doc_name }))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn handle_sync_full_state(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_principal: Option<&str>,
    session_docs: &mut HashSet<String>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    if let Err(msg) = deny_kb_doc_read(doc_store, &doc_name, auth_principal, transport).await {
        warn!(session = session_id, doc = %doc_name, reason = %msg, "sync/full_state denied");
        return JsonRpcResponse::error(id, McpError::internal_error(msg));
    }
    // Track this doc for disconnect cleanup and doc-scoped broadcast filtering.
    if session_docs.insert(doc_name.clone()) {
        let _ = doc_store.track_client_connect(&doc_name).await;
        broadcaster
            .lock()
            .unwrap()
            .subscribe_doc(session_id, &doc_name);
        debug!(session = session_id, doc = %doc_name, "sync/full_state: first interaction, tracking connect");
    }
    match doc_store.encode_state(&doc_name).await {
        Ok(state) => {
            let state_b64 = update_to_base64(&state);
            debug!(session = session_id, doc = %doc_name, state_len = state.len(), "sync/full_state: returning state");
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "doc": doc_name, "state": state_b64 }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_sync_diff(
    doc_store: &DocStore,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    let sv_b64 = match params["sv"].as_str() {
        Some(s) => s,
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'sv' field".to_string()),
            );
        }
    };
    let sv_bytes = match base64_to_update(sv_b64) {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error(format!("invalid base64: {e}")),
            );
        }
    };
    // BUG C fix: atomic diff + sv under single lock (INV-2).
    match doc_store.encode_diff_and_sv(&doc_name, &sv_bytes).await {
        Ok((diff, server_sv)) => {
            let diff_b64 = update_to_base64(&diff);
            let server_sv_b64 = update_to_base64(&server_sv);
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "doc": doc_name,
                    "update": diff_b64,
                    "server_sv": server_sv_b64,
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_sync_resync(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    // Full resync: returns full state + state vector for a document.
    // BUG C fix: atomic state + sv under single lock (INV-2).
    let raw_name = params["doc"].as_str().unwrap_or("default").to_string();
    info!(session = session_id, doc = %raw_name, "sync/resync: processing");
    // Resolve bare filenames via suffix matching (e.g. "test.txt" finds "file:no-project/test.txt").
    let doc_name = if doc_store.has_doc(&raw_name).await {
        raw_name
    } else if let Some(found) = doc_store.find_doc_by_suffix(&raw_name).await {
        info!(requested = %raw_name, resolved = %found, "resolved doc by suffix match");
        found
    } else {
        raw_name // fall through — will create new empty doc
    };
    // Track this doc for disconnect cleanup and doc-scoped broadcast filtering.
    if session_docs.insert(doc_name.clone()) {
        let _ = doc_store.track_client_connect(&doc_name).await;
        broadcaster
            .lock()
            .unwrap()
            .subscribe_doc(session_id, &doc_name);
    }
    match doc_store.encode_state_and_sv(&doc_name).await {
        Ok((state, sv)) => {
            info!(session = session_id, doc = %doc_name, state_len = state.len(), sv_len = sv.len(), "sync/resync: returning state");
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "doc": doc_name,
                    "state": update_to_base64(&state),
                    "sv": update_to_base64(&sv),
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_sync_share(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    // BUG D fix: use atomic share_doc (delete + create + connected_clients=1).
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    info!(session = session_id, doc = %doc_name, "sync/share: processing");
    // Track this doc for disconnect cleanup and doc-scoped broadcast filtering.
    session_docs.insert(doc_name.clone());
    broadcaster
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .subscribe_doc(session_id, &doc_name);
    let update_b64 = match params["update"].as_str() {
        Some(s) => s,
        None => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error("missing 'update' field".to_string()),
            );
        }
    };
    let update_bytes = match base64_to_update(update_b64) {
        Ok(b) => b,
        Err(e) => {
            return JsonRpcResponse::error(
                id,
                McpError::parse_error(format!("invalid base64: {e}")),
            );
        }
    };

    match doc_store.share_doc(&doc_name, &update_bytes).await {
        Ok(result) => {
            info!(session = session_id, doc = %doc_name, wal_seq = result.wal_seq,
                        update_len = result.update.len(), "sync/share: accepted");
            // Record this session as the sharer for disconnect notifications.
            doc_store.set_sharer_session(&doc_name, session_id).await;
            // Broadcast to all OTHER subscribers (not the sharer).
            {
                let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                bc.broadcast_except(
                    &EditorEvent::SyncUpdate {
                        buffer_name: doc_name.clone(),
                        update_base64: update_to_base64(&result.update),
                        wal_seq: result.wal_seq,
                        content_header: None,
                    },
                    session_id,
                );
                let subscriber_count = bc.client_count().saturating_sub(1);
                debug!(session = session_id, doc = %doc_name, subscriber_count, "sync/share: broadcast sent");
            }
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "doc": doc_name, "wal_seq": result.wal_seq }),
            )
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}
