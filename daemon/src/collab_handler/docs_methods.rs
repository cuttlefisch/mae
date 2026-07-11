//! `docs/*` doc-request handlers (+ `$/debug`), split out of
//! `collab_handler/mod.rs`'s `handle_doc_request_inner` match (pure code
//! motion — see that module's doc comment). `$/debug` doesn't carry a
//! `docs/`/`sync/`/`kb/` prefix but is doc/connection introspection, so it
//! lives here alongside the other read-mostly `docs/*` arms.

use super::*;

pub(super) async fn handle_docs_list(
    doc_store: &DocStore,
    id: serde_json::Value,
) -> JsonRpcResponse {
    let names = doc_store.document_names().await;
    JsonRpcResponse::success(id, serde_json::json!({ "documents": names }))
}

pub(super) async fn handle_docs_content(
    doc_store: &DocStore,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    match doc_store.content(&doc_name).await {
        Ok(text) => {
            JsonRpcResponse::success(id, serde_json::json!({ "doc": doc_name, "content": text }))
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_docs_stats(
    doc_store: &DocStore,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    match doc_store.doc_stats(&doc_name).await {
        Ok(stats) => {
            JsonRpcResponse::success(id, serde_json::json!({ "doc": doc_name, "stats": stats }))
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_docs_metadata(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    match doc_store.doc_stats(&doc_name).await {
        Ok(stats) => {
            let connection_count = broadcaster
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .client_count();
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

pub(super) async fn handle_docs_save_intent(
    doc_store: &DocStore,
    session_id: u64,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
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
            JsonRpcResponse::success(id, serde_json::json!({ "doc": doc_name, "result": result }))
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_docs_save_committed(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_label: Option<&str>,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    // Strict binding: an authenticated peer's saved_by is its verified
    // identity, not a self-claimed name.
    let saved_by = auth_label
        .map(str::to_string)
        .unwrap_or_else(|| params["saved_by"].as_str().unwrap_or("unknown").to_string());
    let save_epoch = params["save_epoch"].as_u64().unwrap_or(0);
    let content_hash = params["content_hash"].as_str().unwrap_or("").to_string();

    debug!(session = session_id, doc = %doc_name, saved_by = %saved_by, save_epoch, "docs/save_committed: recording");

    // Record save metadata on the document.
    if let Err(e) = doc_store.record_save(&doc_name, &saved_by).await {
        warn!(doc = %doc_name, error = %e, "failed to record save");
    }

    // Broadcast save_committed to peers (excluding the saver).
    {
        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
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

pub(super) async fn handle_docs_delete(
    doc_store: &DocStore,
    session_id: u64,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
    debug!(session = session_id, doc = %doc_name, "docs/delete: processing");
    match doc_store.delete_doc(&doc_name).await {
        Ok(()) => {
            JsonRpcResponse::success(id, serde_json::json!({ "doc": doc_name, "deleted": true }))
        }
        Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e.to_string())),
    }
}

pub(super) async fn handle_debug_stats(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    start_time: std::time::Instant,
    id: serde_json::Value,
) -> JsonRpcResponse {
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
    let connection_count = broadcaster
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .client_count();
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "documents": names.len(),
            "doc_stats": doc_stats,
            "version": env!("CARGO_PKG_VERSION"),
            "build": crate::BUILD_SHA,
            "uptime_secs": uptime_secs,
            "connection_count": connection_count,
        }),
    )
}
