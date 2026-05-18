//! Client connection handler for the state server.
//!
//! Each TCP (or Unix) client gets its own tokio task running this handler.
//! Uses `mae_mcp::read_message` for framing and `mae_mcp::write_framed`
//! for responses. Protocol methods (initialize, ping, subscribe) are
//! delegated to `mae_mcp::handle_request`. Sync methods are handled locally
//! by dispatching to the DocStore.

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

/// Run the client handler loop for a single connection.
///
/// Generic over reader/writer — works with TCP, Unix, or any async stream.
pub async fn handle_client<R, W>(
    reader: R,
    mut writer: W,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
) where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = reader;
    let write_timeout = std::time::Duration::from_secs(5);

    let mut session = ClientSession::new();
    let session_id = session.id;
    info!(session = session_id, "state-server client connected");

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
                let response = if is_doc_method(&msg) {
                    handle_doc_request(&msg, &doc_store, &broadcaster).await
                } else {
                    mae_mcp::handle_request(
                        &msg, &tool_defs, &tool_tx, &mut session, &broadcaster,
                    ).await
                };

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
                    if consecutive_write_failures >= 3 {
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

    // Unsubscribe on disconnect.
    broadcaster.lock().unwrap().unsubscribe(session_id);
    info!(session = session_id, "state-server client session ended");
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
        || msg.contains("\"$/debug\"")
}

/// Handle document-level methods directly (without editor tool dispatch).
async fn handle_doc_request(
    msg: &str,
    doc_store: &DocStore,
    _broadcaster: &SharedBroadcaster,
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
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
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
            let client_id = params["client_id"].as_u64();

            match doc_store
                .apply_update(&doc_name, &update_bytes, client_id)
                .await
            {
                Ok(result) => {
                    // Broadcast to other subscribers.
                    {
                        let mut bc = _broadcaster.lock().unwrap();
                        bc.broadcast(&EditorEvent::SyncUpdate {
                            buffer_name: doc_name.clone(),
                            update_base64: update_to_base64(&result.update),
                            wal_seq: result.wal_seq,
                        });
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
            match doc_store.encode_diff(&doc_name, &sv_bytes).await {
                Ok(diff) => {
                    let diff_b64 = update_to_base64(&diff);
                    let server_sv = doc_store.state_vector(&doc_name).await.unwrap_or_default();
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
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            match doc_store.encode_state(&doc_name).await {
                Ok(state) => {
                    let sv = doc_store.state_vector(&doc_name).await.unwrap_or_default();
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
            // Acknowledge that a save completed. Currently a no-op stub —
            // can be extended to update metadata, trigger hooks, etc.
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "doc": doc_name, "committed": true }),
            )
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
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "documents": names.len(),
                    "doc_stats": doc_stats,
                    "version": env!("CARGO_PKG_VERSION"),
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
        let resp = handle_doc_request(&msg.to_string(), &store, &bc).await;
        assert!(resp.error.is_none(), "sync/update failed: {:?}", resp.error);
        assert!(resp.result.unwrap()["wal_seq"].as_u64().unwrap() > 0);

        // docs/content
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "docs/content",
            "params": { "doc": "test" }
        });
        let resp = handle_doc_request(&msg.to_string(), &store, &bc).await;
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
        let resp = handle_doc_request(&msg.to_string(), &store, &bc).await;
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
        let resp = handle_doc_request(&msg.to_string(), &store, &bc).await;
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
        let resp = handle_doc_request(&msg.to_string(), &store, &bc).await;
        let docs = resp.result.unwrap()["documents"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(docs.len(), 2);
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
            handle_client(server_reader, server_write, store_clone, bc_clone).await;
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
}
