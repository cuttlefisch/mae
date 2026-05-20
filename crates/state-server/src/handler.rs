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

use crate::doc_store::DocStore;

/// Write timeout for event notifications to clients (seconds).
const WRITE_TIMEOUT_SECS: u64 = 5;
/// Disconnect client after this many consecutive write failures.
const MAX_CONSECUTIVE_WRITE_FAILURES: u32 = 3;
/// Maximum allowed size for a single sync update payload (bytes).
const MAX_UPDATE_SIZE: usize = 1_048_576; // 1 MB

/// Run the client handler loop for a single connection.
///
/// Generic over reader/writer — works with TCP, Unix, or any async stream.
pub async fn handle_client<R, W>(
    reader: R,
    mut writer: W,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
) where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = reader;
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

    // Subscribe with empty subs — client opts in later.
    let mut event_rx = {
        let mut bc = broadcaster.lock().unwrap();
        bc.subscribe(session_id, vec![])
    };

    let tool_defs: Vec<ToolInfo> = vec![];
    let mut consecutive_write_failures: u32 = 0;

    loop {
        tokio::select! {
            biased;

            msg = mae_mcp::read_message(&mut reader) => {
                let msg = match msg {
                    Ok(Some(msg)) => msg,
                    Ok(None) => {
                        debug!(session = session_id, "client disconnected (EOF)");
                        break;
                    }
                    Err(e) => {
                        error!(session = session_id, error = %e, "read error");
                        break;
                    }
                };

                session.touch();
                session.messages_received += 1;

                // Check if this is a sync/* method we handle differently.
                let mut response = if is_doc_method(&msg) {
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
        if let Err(e) = doc_store.track_client_disconnect(doc_name).await {
            warn!(session = session_id, doc = %doc_name, error = %e, "disconnect tracking failed");
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
        || msg.contains("\"docs/list\"")
        || msg.contains("\"docs/content\"")
        || msg.contains("\"docs/stats\"")
        || msg.contains("\"docs/save_intent\"")
        || msg.contains("\"docs/save_committed\"")
        || msg.contains("\"docs/delete\"")
        || msg.contains("\"sync/share\"")
        || msg.contains("\"$/debug\"")
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
            let doc_name = match params["doc"].as_str() {
                Some(d) => d.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'doc' field".to_string()),
                    );
                }
            };
            // Track this doc for disconnect cleanup.
            if session_docs.insert(doc_name.clone()) {
                // First interaction — track client connect.
                let _ = doc_store.track_client_connect(&doc_name).await;
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

        "sync/full_state" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            // Track this doc for disconnect cleanup (joiners use full_state).
            if session_docs.insert(doc_name.clone()) {
                let _ = doc_store.track_client_connect(&doc_name).await;
            }
            match doc_store.encode_state(&doc_name).await {
                Ok(state) => {
                    let state_b64 = update_to_base64(&state);
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
            // Resolve bare filenames via suffix matching (e.g. "test.txt" finds "file:no-project/test.txt").
            let doc_name = if doc_store.has_doc(&raw_name).await {
                raw_name
            } else if let Some(found) = doc_store.find_doc_by_suffix(&raw_name).await {
                info!(requested = %raw_name, resolved = %found, "resolved doc by suffix match");
                found
            } else {
                raw_name // fall through — will create new empty doc
            };
            // Track this doc for disconnect cleanup (same as sync/full_state).
            if session_docs.insert(doc_name.clone()) {
                let _ = doc_store.track_client_connect(&doc_name).await;
            }
            match doc_store.encode_state_and_sv(&doc_name).await {
                Ok((state, sv)) => JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "doc": doc_name,
                        "state": update_to_base64(&state),
                        "sv": update_to_base64(&sv),
                    }),
                ),
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
                Ok(result) => JsonRpcResponse::success(
                    id,
                    serde_json::json!({ "doc": doc_name, "result": result }),
                ),
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
            }
        }

        "docs/save_committed" => {
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            let saved_by = params["saved_by"].as_str().unwrap_or("unknown").to_string();
            let save_epoch = params["save_epoch"].as_u64().unwrap_or(0);
            let content_hash = params["content_hash"].as_str().unwrap_or("").to_string();

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
            // Track this doc for disconnect cleanup.
            session_docs.insert(doc_name.clone());
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
}
