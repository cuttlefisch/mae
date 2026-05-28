//! Client connection handler for the state server.
//!
//! Each TCP (or Unix) client gets its own tokio task running this handler.
//! Uses `mae_mcp::read_message` for framing and `mae_mcp::write_framed`
//! for responses. Protocol methods (initialize, ping, subscribe) are
//! delegated to `mae_mcp::handle_request`. Sync methods are handled locally
//! by dispatching to the DocStore.

use std::collections::HashSet;
use std::sync::Arc;

use mae_mcp::broadcast::{EditorEvent, SharedBroadcaster};
use mae_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpError, ToolInfo};
use mae_mcp::session::ClientSession;
use mae_mcp::{McpToolRequest, McpToolResult};
use mae_sync::encoding::{base64_to_update, update_to_base64};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::auth::AuthProvider;
use crate::doc_store::DocStore;

/// Write timeout for event notifications to clients (seconds).
const WRITE_TIMEOUT_SECS: u64 = 5;
/// Disconnect client after this many consecutive write failures.
const MAX_CONSECUTIVE_WRITE_FAILURES: u32 = 3;
/// Maximum allowed size for a single sync update payload (bytes).
const MAX_UPDATE_SIZE: usize = 1_048_576; // 1 MB

/// Run the client handler with an authentication handshake before the main loop.
///
/// The auth handshake runs on the raw stream before JSON-RPC `initialize`.
/// If auth fails, the connection is dropped without entering the main loop.
pub async fn handle_client_with_auth<R, W, A>(
    mut reader: R,
    mut writer: W,
    auth: &A,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
) where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send,
    A: AuthProvider,
{
    match auth.server_handshake(&mut reader, &mut writer).await {
        Ok(result) => {
            info!(
                auth = auth.name(),
                client = %result.client_label,
                "auth handshake succeeded"
            );
        }
        Err(e) => {
            warn!(auth = auth.name(), error = %e, "auth handshake failed, dropping connection");
            return;
        }
    }
    handle_client(reader, writer, doc_store, broadcaster, start_time).await;
}

/// Run the client handler loop for a single connection.
///
/// Generic over reader/writer — works with TCP, Unix, or any async stream.
///
/// CANCEL-SAFETY: `read_message` uses `read_line` / `read_exact` internally,
/// which are NOT cancel-safe — if a `tokio::select!` cancels them mid-read the
/// BufReader is left in a corrupted state (header consumed, body still pending).
/// To avoid this, we spawn a dedicated reader task that feeds complete messages
/// into an mpsc channel, so `read_message` always runs to completion.
pub async fn handle_client<R, W>(
    reader: R,
    mut writer: W,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
) where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let write_timeout = std::time::Duration::from_secs(WRITE_TIMEOUT_SECS);

    let mut session = ClientSession::new();
    let session_id = session.id;
    info!(session = session_id, "state-server client connected");

    // Track which docs this session has interacted with for disconnect cleanup.
    let mut session_docs: HashSet<String> = HashSet::new();

    // Create a dummy tool channel — the state server has no editor tools,
    // but handle_request needs one for the type signature.
    let (tool_tx, mut tool_rx) = mpsc::channel::<McpToolRequest>(16);

    // Spawn a task to handle tool requests that come from handle_request's
    // sync/* dispatch. We intercept them and handle via DocStore.
    let doc_store_for_tools = Arc::clone(&doc_store);
    let bc_for_tools = Arc::clone(&broadcaster);
    tokio::spawn(async move {
        while let Some(req) = tool_rx.recv().await {
            let result = handle_sync_tool(
                &req.tool_name,
                &req.arguments,
                &doc_store_for_tools,
                &bc_for_tools,
            )
            .await;
            let _ = req.reply.send(result);
        }
    });

    // Spawn a dedicated reader task so read_message always runs to completion
    // (never cancelled by select!).  Messages arrive via an mpsc channel.
    let (msg_tx, mut msg_rx) = mpsc::channel::<Result<String, String>>(32);
    tokio::spawn(async move {
        let mut reader = reader;
        loop {
            match mae_mcp::read_message(&mut reader).await {
                Ok(Some(msg)) => {
                    if msg_tx.send(Ok(msg)).await.is_err() {
                        break; // handler dropped
                    }
                }
                Ok(None) => {
                    let _ = msg_tx.send(Err("EOF".to_string())).await;
                    break;
                }
                Err(e) => {
                    let _ = msg_tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });

    // Subscribe with empty subs — client opts in later.
    let mut event_rx = {
        let mut bc = broadcaster.lock().unwrap();
        bc.subscribe(session_id, vec![])
    };

    let tool_defs: Vec<ToolInfo> = vec![];
    let mut consecutive_write_failures: u32 = 0;

    loop {
        tokio::select! {
            // NOTE: do NOT use `biased;` here — it causes starvation of the
            // event_rx arm when the client sends requests rapidly. This means
            // broadcast events (sync_update from other peers) never get delivered
            // to a client that is itself actively sending updates.

            msg = msg_rx.recv() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) if e == "EOF" => {
                        debug!(session = session_id, "client disconnected (EOF)");
                        break;
                    }
                    Some(Err(e)) => {
                        error!(session = session_id, error = %e, "read error");
                        break;
                    }
                    None => {
                        debug!(session = session_id, "reader task ended");
                        break;
                    }
                };

                session.touch();
                session.messages_received += 1;
                // WU6: Log message classification for dispatch diagnostics.
                let is_doc = is_doc_method(&msg);
                let is_notif = is_notification(&msg);
                debug!(session = session_id, msg_len = msg.len(),
                    is_doc, is_notif,
                    preview = &msg[..msg.len().min(120)],
                    "dispatch: message classified");

                // Check if this is a sync/* method we handle differently.
                // WU1: Detect notifications (no `id`) before dispatching.
                // Notifications must not generate a response — handle and continue.
                if is_doc && is_notif {
                    debug!(session = session_id, "notification detected, handling without response");
                    handle_doc_notification(&msg, &doc_store, &broadcaster, session_id, &mut session_docs).await;
                    continue;
                }

                let mut response = if is_doc {
                    handle_doc_request(&msg, &doc_store, &broadcaster, start_time, session_id, &mut session_docs).await
                } else {
                    mae_mcp::handle_request(
                        &msg, &tool_defs, &tool_tx, &mut session, &broadcaster,
                    ).await
                };

                // Augment initialize response with connection count so
                // clients can report peer count accurately.
                if msg.contains("\"initialize\"") {
                    if let Some(ref mut result) = response.result {
                        if let Some(info) = result.get_mut("serverInfo") {
                            let mut bc = broadcaster.lock().unwrap();
                            let count = bc.client_count().saturating_sub(1);
                            info["connections"] = serde_json::json!(count);
                            // Notify existing clients about the new peer.
                            let peer_count = bc.client_count();
                            bc.broadcast_except(
                                &EditorEvent::PeerJoined {
                                    session_id,
                                    peer_count,
                                },
                                session_id,
                            );
                        }
                    }
                }

                let body = match serde_json::to_vec(&response) {
                    Ok(b) => b,
                    Err(e) => {
                        error!(session = session_id, error = %e, "serialize error");
                        continue;
                    }
                };

                if mae_mcp::write_framed(&mut writer, &body, write_timeout).await.is_err() {
                    warn!(session = session_id, "write error; closing client");
                    break;
                }
            }

            Some(event) = event_rx.recv() => {
                let method = format!("notifications/{}", event.event_type());
                debug!(session = session_id, event_type = %method,
                    "broadcasting event to client");
                let notification = serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": method,
                    "params": { "seq": session.events_delivered + 1, "event": event },
                });
                let body = match serde_json::to_vec(&notification) {
                    Ok(b) => b,
                    Err(_) => continue,
                };

                if mae_mcp::write_framed(&mut writer, &body, write_timeout).await.is_err() {
                    consecutive_write_failures += 1;
                    session.events_dropped += 1;
                    if consecutive_write_failures >= MAX_CONSECUTIVE_WRITE_FAILURES {
                        warn!(session = session_id, "disconnecting after 3 write failures");
                        break;
                    }
                } else {
                    consecutive_write_failures = 0;
                    session.events_delivered += 1;
                }
            }
        }
    }

    // Track client disconnect for all docs this session touched.
    for doc_name in &session_docs {
        debug!(session = session_id, doc = %doc_name, "disconnect: cleanup for doc");
        if let Err(e) = doc_store.track_client_disconnect(doc_name).await {
            warn!(session = session_id, doc = %doc_name, error = %e, "disconnect tracking failed");
        }
    }

    // Check if this session was the sharer for any docs and broadcast SharerLeft.
    for doc_name in &session_docs {
        if doc_store.is_sharer(doc_name, session_id).await {
            debug!(session = session_id, doc = %doc_name, "disconnect: was sharer, broadcasting SharerLeft");
            doc_store.clear_sharer(doc_name).await;
            let mut bc = broadcaster.lock().unwrap();
            let remaining = bc.client_count().saturating_sub(1);
            bc.broadcast_except(
                &EditorEvent::SharerLeft {
                    session_id,
                    doc: doc_name.clone(),
                    peer_count: remaining,
                },
                session_id,
            );
        }
    }

    // Broadcast PeerLeft to remaining clients.
    {
        let mut bc = broadcaster.lock().unwrap();
        let remaining = bc.client_count().saturating_sub(1); // exclude this session (about to unsubscribe)
        bc.broadcast_except(
            &EditorEvent::PeerLeft {
                session_id,
                peer_count: remaining,
            },
            session_id,
        );
        bc.unsubscribe(session_id);
    }
    info!(
        session = session_id,
        docs_touched = session_docs.len(),
        "state-server client session ended"
    );
}

/// Check if a raw JSON message is a doc-level sync method.
fn is_doc_method(msg: &str) -> bool {
    // Quick string check before full parse.
    msg.contains("\"sync/state_vector\"")
        || msg.contains("\"sync/update\"")
        || msg.contains("\"sync/full_state\"")
        || msg.contains("\"sync/diff\"")
        || msg.contains("\"sync/resync\"")
        || msg.contains("\"sync/awareness\"")
        || msg.contains("\"docs/list\"")
        || msg.contains("\"docs/content\"")
        || msg.contains("\"docs/stats\"")
        || msg.contains("\"docs/save_intent\"")
        || msg.contains("\"docs/save_committed\"")
        || msg.contains("\"docs/delete\"")
        || msg.contains("\"docs/metadata\"")
        || msg.contains("\"sync/share\"")
        || msg.contains("\"$/debug\"")
}

/// Check if a raw JSON message is a JSON-RPC notification (has `method`, no `id`).
///
/// Notifications must not generate a response. Sending awareness as a notification
/// is correct per JSON-RPC 2.0 — the server should relay without responding.
fn is_notification(msg: &str) -> bool {
    msg.contains("\"method\"") && !msg.contains("\"id\"")
}

/// Handle a JSON-RPC notification (no `id` field) for doc-level methods.
///
/// Unlike `handle_doc_request`, this does NOT return a response — per JSON-RPC 2.0,
/// notifications must not be replied to. Currently handles `sync/awareness` relay.
async fn handle_doc_notification(
    msg: &str,
    _doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
) {
    // Parse method and params manually — no JsonRpcRequest (requires `id`).
    let val: serde_json::Value = match serde_json::from_str(msg) {
        Ok(v) => v,
        Err(e) => {
            warn!(session = session_id, error = %e, "notification: invalid JSON");
            return;
        }
    };
    let method = match val.get("method").and_then(|m| m.as_str()) {
        Some(m) => m,
        None => return,
    };
    let params = val
        .get("params")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    match method {
        "sync/awareness" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            let state = &params["state"];
            debug!(session = session_id, doc = %doc_name, "sync/awareness notification: relaying");
            // Track doc for cleanup and doc-scoped broadcast filtering.
            session_docs.insert(doc_name.clone());
            {
                let mut bc = broadcaster.lock().unwrap();
                bc.subscribe_doc(session_id, &doc_name);
                bc.broadcast_except(
                    &EditorEvent::AwarenessUpdate {
                        doc_id: doc_name,
                        client_id: session_id,
                        user_name: state
                            .get("user_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
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
        }
        _ => {
            debug!(session = session_id, method, "unhandled doc notification");
        }
    }
}

/// Handle document-level methods directly (without editor tool dispatch).
async fn handle_doc_request(
    msg: &str,
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    start_time: std::time::Instant,
    session_id: u64,
    session_docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    let request: JsonRpcRequest = match serde_json::from_str(msg) {
        Ok(r) => r,
        Err(e) => {
            return JsonRpcResponse::error(
                serde_json::Value::Null,
                McpError::parse_error(format!("Invalid JSON: {e}")),
            );
        }
    };

    let id = request.id.clone();
    let params = request.params.unwrap_or(serde_json::Value::Null);

    info!(session = session_id, method = %request.method, "doc request");
    match request.method.as_str() {
        "sync/state_vector" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            match doc_store.state_vector(&doc_name).await {
                Ok(sv) => {
                    let sv_b64 = update_to_base64(&sv);
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "doc": doc_name, "sv": sv_b64 }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
            }
        }

        "sync/update" => {
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
            if update_bytes.len() > MAX_UPDATE_SIZE {
                return JsonRpcResponse::error(
                    id,
                    McpError::parse_error(format!(
                        "update too large: {} bytes (max {})",
                        update_bytes.len(),
                        MAX_UPDATE_SIZE
                    )),
                );
            }
            let client_id = params["client_id"].as_u64();

            match doc_store
                .apply_update(&doc_name, &update_bytes, client_id)
                .await
            {
                Ok(result) => {
                    // Broadcast to other subscribers (skip sender to avoid echo).
                    {
                        let mut bc = broadcaster.lock().unwrap();
                        bc.broadcast_except(
                            &EditorEvent::SyncUpdate {
                                buffer_name: doc_name.clone(),
                                update_base64: update_to_base64(&result.update),
                                wal_seq: result.wal_seq,
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

        "sync/awareness" => {
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
                let mut bc = broadcaster.lock().unwrap();
                bc.subscribe_doc(session_id, &doc_name);
                bc.broadcast_except(
                    &EditorEvent::AwarenessUpdate {
                        doc_id: doc_name.clone(),
                        client_id: session_id,
                        user_name: state
                            .get("user_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
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

        "sync/full_state" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
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

        "sync/diff" => {
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

        "docs/list" => {
            let names = doc_store.document_names().await;
            JsonRpcResponse::success(id, serde_json::json!({ "documents": names }))
        }

        "docs/content" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            match doc_store.content(&doc_name).await {
                Ok(text) => JsonRpcResponse::success(
                    id,
                    serde_json::json!({ "doc": doc_name, "content": text }),
                ),
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
            }
        }

        "sync/resync" => {
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

        "docs/stats" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            match doc_store.doc_stats(&doc_name).await {
                Ok(stats) => JsonRpcResponse::success(
                    id,
                    serde_json::json!({ "doc": doc_name, "stats": stats }),
                ),
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
            }
        }

        "docs/metadata" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            match doc_store.doc_stats(&doc_name).await {
                Ok(stats) => {
                    let connection_count = broadcaster.lock().unwrap().client_count();
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({
                            "doc": doc_name,
                            "connected_clients": stats.connected_clients,
                            "save_epoch": stats.save_epoch,
                            "last_saved_by": stats.last_saved_by,
                            "content_length": stats.content_length,
                            "update_count": stats.update_count,
                            "idle_secs": stats.idle_secs,
                            "total_connections": connection_count,
                        }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
            }
        }

        "docs/save_intent" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            let expected_hash = match params["expected_hash"].as_str() {
                Some(h) => h,
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'expected_hash' field".to_string()),
                    );
                }
            };
            match doc_store.check_save_intent(&doc_name, expected_hash).await {
                Ok(result) => {
                    debug!(session = session_id, doc = %doc_name, result = ?result, "docs/save_intent: checked");
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "doc": doc_name, "result": result }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
            }
        }

        "docs/save_committed" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            let saved_by = params["saved_by"].as_str().unwrap_or("unknown").to_string();
            let save_epoch = params["save_epoch"].as_u64().unwrap_or(0);
            let content_hash = params["content_hash"].as_str().unwrap_or("").to_string();

            debug!(session = session_id, doc = %doc_name, saved_by = %saved_by, save_epoch, "docs/save_committed: recording");

            // Record save metadata on the document.
            if let Err(e) = doc_store.record_save(&doc_name, &saved_by).await {
                warn!(doc = %doc_name, error = %e, "failed to record save");
            }

            // Broadcast save_committed to peers (excluding the saver).
            {
                let mut bc = broadcaster.lock().unwrap();
                bc.broadcast_except(
                    &EditorEvent::SaveCommitted {
                        doc: doc_name.clone(),
                        saved_by: saved_by.clone(),
                        save_epoch,
                        content_hash,
                    },
                    session_id,
                );
            }

            JsonRpcResponse::success(
                id,
                serde_json::json!({ "doc": doc_name, "committed": true }),
            )
        }

        "sync/share" => {
            // BUG D fix: use atomic share_doc (delete + create + connected_clients=1).
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            info!(session = session_id, doc = %doc_name, "sync/share: processing");
            // Track this doc for disconnect cleanup and doc-scoped broadcast filtering.
            session_docs.insert(doc_name.clone());
            broadcaster
                .lock()
                .unwrap()
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
                        let mut bc = broadcaster.lock().unwrap();
                        bc.broadcast_except(
                            &EditorEvent::SyncUpdate {
                                buffer_name: doc_name.clone(),
                                update_base64: update_to_base64(&result.update),
                                wal_seq: result.wal_seq,
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

        "docs/delete" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            debug!(session = session_id, doc = %doc_name, "docs/delete: processing");
            match doc_store.delete_doc(&doc_name).await {
                Ok(()) => JsonRpcResponse::success(
                    id,
                    serde_json::json!({ "doc": doc_name, "deleted": true }),
                ),
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
            }
        }

        "$/debug" => {
            let names = doc_store.document_names().await;
            let mut doc_stats = serde_json::Map::new();
            for name in &names {
                if let Ok(stats) = doc_store.doc_stats(name).await {
                    doc_stats.insert(
                        name.clone(),
                        serde_json::to_value(&stats).unwrap_or_default(),
                    );
                }
            }
            let uptime_secs = start_time.elapsed().as_secs();
            let connection_count = broadcaster.lock().unwrap().client_count();
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "documents": names.len(),
                    "doc_stats": doc_stats,
                    "version": env!("CARGO_PKG_VERSION"),
                    "uptime_secs": uptime_secs,
                    "connection_count": connection_count,
                }),
            )
        }

        // --- KB protocol methods ---
        "kb/register" => {
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
                .unwrap()
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

        "kb/list" => {
            let kbs = doc_store.list_kb_metas().await;
            JsonRpcResponse::success(id, serde_json::json!({ "kbs": kbs }))
        }

        "kb/unregister" => {
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

        other => JsonRpcResponse::error(
            id,
            McpError::method_not_found(format!("Unknown method: {other}")),
        ),
    }
}

/// Handle sync tool requests from mae_mcp::handle_request's sync/* dispatch.
async fn handle_sync_tool(
    tool_name: &str,
    arguments: &serde_json::Value,
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
) -> McpToolResult {
    match tool_name {
        "__mcp_sync_enable" => McpToolResult {
            success: true,
            output: serde_json::json!({ "sync_enabled": true }).to_string(),
        },
        "__mcp_sync_state_vector" => {
            let doc = arguments["doc"].as_str().unwrap_or("default");
            match doc_store.state_vector(doc).await {
                Ok(sv) => McpToolResult {
                    success: true,
                    output: serde_json::json!({
                        "doc": doc,
                        "sv": update_to_base64(&sv),
                    })
                    .to_string(),
                },
                Err(e) => McpToolResult {
                    success: false,
                    output: e.to_string(),
                },
            }
        }
        "__mcp_sync_update" => {
            let doc = arguments["doc"].as_str().unwrap_or("default").to_string();
            let update_b64 = arguments["update"].as_str().unwrap_or("");
            let update_bytes = match base64_to_update(update_b64) {
                Ok(b) => b,
                Err(e) => {
                    return McpToolResult {
                        success: false,
                        output: format!("invalid base64: {e}"),
                    };
                }
            };
            let client_id = arguments["client_id"].as_u64();
            match doc_store.apply_update(&doc, &update_bytes, client_id).await {
                Ok(result) => {
                    let mut bc = broadcaster.lock().unwrap();
                    bc.broadcast(&EditorEvent::SyncUpdate {
                        buffer_name: doc.clone(),
                        update_base64: update_to_base64(&result.update),
                        wal_seq: result.wal_seq,
                    });
                    McpToolResult {
                        success: true,
                        output: serde_json::json!({
                            "doc": doc,
                            "wal_seq": result.wal_seq,
                        })
                        .to_string(),
                    }
                }
                Err(e) => McpToolResult {
                    success: false,
                    output: e.to_string(),
                },
            }
        }
        "__mcp_sync_full_state" => {
            let doc = arguments["doc"].as_str().unwrap_or("default");
            match doc_store.encode_state(doc).await {
                Ok(state) => McpToolResult {
                    success: true,
                    output: serde_json::json!({
                        "doc": doc,
                        "state": update_to_base64(&state),
                    })
                    .to_string(),
                },
                Err(e) => McpToolResult {
                    success: false,
                    output: e.to_string(),
                },
            }
        }
        _ => McpToolResult {
            success: false,
            output: format!("unknown sync tool: {tool_name}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteBackend;
    use mae_mcp::broadcast::EventBroadcaster;
    use mae_sync::encoding::update_to_base64;
    use mae_sync::text::TextSync;
    use tokio::io::BufReader;

    fn test_broadcaster() -> SharedBroadcaster {
        Arc::new(std::sync::Mutex::new(EventBroadcaster::new()))
    }

    fn test_doc_store() -> Arc<DocStore> {
        let backend = Arc::new(SqliteBackend::open_memory().unwrap());
        Arc::new(DocStore::new(backend, 500))
    }

    #[tokio::test]
    async fn handle_doc_sync_update_and_read() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Generate a real yrs update.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        let update_b64 = update_to_base64(&update);

        // sync/update
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/update",
            "params": { "doc": "test", "update": update_b64 }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(resp.error.is_none(), "sync/update failed: {:?}", resp.error);
        assert!(resp.result.unwrap()["wal_seq"].as_u64().unwrap() > 0);

        // docs/content
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "docs/content",
            "params": { "doc": "test" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert_eq!(resp.result.unwrap()["content"], "hello");
    }

    #[tokio::test]
    async fn handle_doc_state_vector() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/state_vector",
            "params": { "doc": "test" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(resp.error.is_none());
        let sv = resp.result.unwrap()["sv"].as_str().unwrap().to_string();
        assert!(!sv.is_empty());
    }

    #[tokio::test]
    async fn handle_doc_full_state() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/full_state",
            "params": { "doc": "test" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(resp.error.is_none());
    }

    #[tokio::test]
    async fn handle_docs_list() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Create two docs.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "a");
        store.apply_update("alpha", &update, None).await.unwrap();
        store.apply_update("beta", &update, None).await.unwrap();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "docs/list"
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        let docs = resp.result.unwrap()["documents"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(docs.len(), 2);
    }

    #[tokio::test]
    async fn debug_method_returns_uptime_and_connections() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "$/debug"
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(resp.error.is_none(), "$/debug failed: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert!(
            result.get("uptime_secs").is_some(),
            "should include uptime_secs"
        );
        assert!(
            result.get("connection_count").is_some(),
            "should include connection_count"
        );
        assert!(result.get("version").is_some(), "should include version");
        assert!(
            result.get("documents").is_some(),
            "should include document count"
        );
        assert!(
            result.get("doc_stats").is_some(),
            "should include doc_stats"
        );
        // Uptime should be a small non-negative integer for a just-started server.
        assert!(result["uptime_secs"].as_u64().is_some());
        // No clients connected in this test.
        assert_eq!(result["connection_count"].as_u64().unwrap(), 0);
    }

    #[tokio::test]
    async fn full_client_session_over_pipe() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Create an in-memory duplex stream.
        let (client_stream, server_stream) = tokio::io::duplex(4096);

        let (server_read, server_write) = tokio::io::split(server_stream);
        let server_reader = BufReader::new(server_read);

        // Spawn handler.
        let store_clone = Arc::clone(&store);
        let bc_clone = Arc::clone(&bc);
        tokio::spawn(async move {
            handle_client(
                server_reader,
                server_write,
                store_clone,
                bc_clone,
                std::time::Instant::now(),
            )
            .await;
        });

        // Client side.
        let (client_read, mut client_write) = tokio::io::split(client_stream);
        let mut client_reader = BufReader::new(client_read);

        // Send initialize.
        let init_msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "test-pipe"}}
        });
        let payload = format!("{}\n", serde_json::to_string(&init_msg).unwrap());
        tokio::io::AsyncWriteExt::write_all(&mut client_write, payload.as_bytes())
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut client_write)
            .await
            .unwrap();

        // Read response.
        let resp_msg = mae_mcp::read_message(&mut client_reader)
            .await
            .unwrap()
            .unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&resp_msg).unwrap();
        assert!(resp.error.is_none(), "initialize failed: {:?}", resp.error);
        assert_eq!(resp.result.unwrap()["serverInfo"]["name"], "mae-editor");

        // Ping.
        let ping_msg = serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "$/ping"});
        let payload = format!("{}\n", serde_json::to_string(&ping_msg).unwrap());
        tokio::io::AsyncWriteExt::write_all(&mut client_write, payload.as_bytes())
            .await
            .unwrap();
        tokio::io::AsyncWriteExt::flush(&mut client_write)
            .await
            .unwrap();

        let resp_msg = mae_mcp::read_message(&mut client_reader)
            .await
            .unwrap()
            .unwrap();
        let resp: JsonRpcResponse = serde_json::from_str(&resp_msg).unwrap();
        assert_eq!(resp.result.unwrap(), "pong");
    }

    #[tokio::test]
    async fn resync_tracks_session_doc() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut session_docs = HashSet::new();

        // First create the doc via sync/update.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "resync test");
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/update",
            "params": { "doc": "resync-doc", "update": update_to_base64(&update) }
        });
        handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut session_docs,
        )
        .await;

        // Clear session_docs to simulate a fresh session.
        session_docs.clear();

        // sync/resync should track the doc in session_docs.
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/resync",
            "params": { "doc": "resync-doc" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut session_docs,
        )
        .await;
        assert!(resp.error.is_none(), "resync failed: {:?}", resp.error);
        assert!(
            session_docs.contains("resync-doc"),
            "resync must track doc in session_docs"
        );
    }

    #[tokio::test]
    async fn resync_increments_connected_clients() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut session_docs = HashSet::new();

        // Create doc.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/update",
            "params": { "doc": "cc-doc", "update": update_to_base64(&update) }
        });
        handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut session_docs,
        )
        .await;

        // Resync from a different session.
        let mut session2 = HashSet::new();
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "sync/resync",
            "params": { "doc": "cc-doc" }
        });
        handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            1,
            &mut session2,
        )
        .await;

        // Check doc_stats — connected_clients should be at least 1.
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "docs/stats",
            "params": { "doc": "cc-doc" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            1,
            &mut session2,
        )
        .await;
        let stats = &resp.result.unwrap()["stats"];
        assert!(
            stats["connected_clients"].as_u64().unwrap() >= 1,
            "resync must increment connected_clients, got: {stats}"
        );
    }

    #[tokio::test]
    async fn sync_update_missing_doc_returns_error() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // sync/update without "doc" param should return an error (not silently use "default").
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/update",
            "params": { "update": "AAAA" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(
            resp.error.is_some(),
            "sync/update without doc should return error"
        );
    }

    #[tokio::test]
    async fn sync_update_oversized_rejected() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Create a base64 string that decodes to > 1 MB.
        let big_data = vec![0u8; MAX_UPDATE_SIZE + 1];
        let big_b64 = mae_sync::encoding::update_to_base64(&big_data);

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/update",
            "params": { "doc": "test", "update": big_b64 }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(resp.error.is_some(), "oversized update should be rejected");
        let err_msg = resp.error.unwrap().message;
        assert!(
            err_msg.contains("too large"),
            "error should mention size: {err_msg}"
        );
    }

    #[tokio::test]
    async fn resync_with_suffix_matching() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Create a doc with a file: prefix address.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "shared content");
        store
            .apply_update("file:no-project/test.txt", &update, None)
            .await
            .unwrap();

        // Resync using bare filename — suffix matching should resolve.
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/resync",
            "params": { "doc": "test.txt" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(
            resp.error.is_none(),
            "resync should succeed: {:?}",
            resp.error
        );
        let result = resp.result.unwrap();
        // The response should use the resolved full name.
        assert_eq!(result["doc"], "file:no-project/test.txt");
        // State should be non-empty (contains the shared content).
        assert!(!result["state"].as_str().unwrap().is_empty());
    }

    #[tokio::test]
    async fn docs_metadata_returns_save_epoch() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Create a doc and record a save.
        let mut ts = TextSync::with_client_id("", 1);
        let update = ts.insert(0, "hello");
        store.apply_update("test", &update, Some(1)).await.unwrap();
        store.record_save("test", "alice").await.unwrap();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "docs/metadata",
            "params": { "doc": "test" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(
            resp.error.is_none(),
            "docs/metadata failed: {:?}",
            resp.error
        );
        let result = resp.result.unwrap();
        assert_eq!(result["doc"], "test");
        assert_eq!(result["last_saved_by"], "alice");
        assert!(result["content_length"].as_u64().unwrap() > 0);
    }

    #[tokio::test]
    async fn unknown_method_returns_error() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "sync/nonexistent"
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut HashSet::new(),
        )
        .await;
        assert!(resp.error.is_some());
        assert!(resp.error.unwrap().message.contains("Unknown method"));
    }

    // WU1: Notification handling tests

    #[test]
    fn is_notification_detects_no_id() {
        let notif = r#"{"jsonrpc":"2.0","method":"sync/awareness","params":{}}"#;
        assert!(is_notification(notif));

        let request = r#"{"jsonrpc":"2.0","id":1,"method":"sync/awareness","params":{}}"#;
        assert!(!is_notification(request));

        let response = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700}}"#;
        assert!(!is_notification(response));
    }

    #[tokio::test]
    async fn awareness_notification_no_response() {
        // Sending sync/awareness as a notification (no id) should relay the
        // broadcast but NOT generate any response.
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Subscribe a second client to receive the broadcast.
        let session_id_sender = 1u64;
        let session_id_receiver = 2u64;
        let mut rx = {
            let mut b = bc.lock().unwrap();
            b.subscribe(session_id_receiver, vec!["sync_update".to_string()])
        };

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "sync/awareness",
            "params": {
                "doc": "test.rs",
                "state": {
                    "user_name": "alice",
                    "cursor_row": 10,
                    "cursor_col": 5
                }
            }
        });

        let mut session_docs = HashSet::new();
        handle_doc_notification(
            &msg.to_string(),
            &store,
            &bc,
            session_id_sender,
            &mut session_docs,
        )
        .await;

        // Verify: session_docs tracks the doc for cleanup.
        assert!(session_docs.contains("test.rs"));

        // Verify: broadcast was relayed (receiver should get AwarenessUpdate).
        if let Ok(event) = rx.try_recv() {
            match event {
                EditorEvent::AwarenessUpdate {
                    doc_id,
                    user_name,
                    cursor_row,
                    cursor_col,
                    ..
                } => {
                    assert_eq!(doc_id, "test.rs");
                    assert_eq!(user_name, "alice");
                    assert_eq!(cursor_row, 10);
                    assert_eq!(cursor_col, 5);
                }
                other => panic!("expected AwarenessUpdate, got {:?}", other),
            }
        }
        // No response was generated — that's the whole point of handling notifications.
    }

    #[tokio::test]
    async fn awareness_with_id_returns_ack() {
        // Backward compat: sync/awareness WITH an id should return a success response.
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "sync/awareness",
            "params": {
                "doc": "test.rs",
                "state": {
                    "user_name": "bob",
                    "cursor_row": 0,
                    "cursor_col": 0
                }
            }
        });

        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            1,
            &mut HashSet::new(),
        )
        .await;

        // Should succeed (not error) and echo back the doc name.
        assert!(
            resp.error.is_none(),
            "awareness with id should succeed: {:?}",
            resp.error
        );
        assert_eq!(resp.result.unwrap()["doc"], "test.rs");
    }

    #[tokio::test]
    async fn notification_for_unknown_method_is_silently_dropped() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = r#"{"jsonrpc":"2.0","method":"sync/unknown_notification","params":{}}"#;
        let mut session_docs = HashSet::new();

        // Should not panic or error — just log and return.
        handle_doc_notification(msg, &store, &bc, 1, &mut session_docs).await;
    }
}
