//! ADR-116 D7: the version-history methods. `kb/history.checkpoint` cuts a
//! named version (D2.3).

use super::*;
use crate::storage::StorageError;
use mae_sync::kb::history::{HistoryEntry, VersionKind};

/// The longest name a version may carry, in characters.
const MAX_NAME_CHARS: usize = 200;

/// A version as the history methods report it.
pub(super) fn entry_json(e: &HistoryEntry) -> serde_json::Value {
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(e.created_at.max(0) as u64);
    serde_json::json!({
        "kb_id": e.kb_id,
        "node_id": e.node_id,
        "seq": e.seq,
        "kind": e.kind.as_str(),
        "created_at": humantime::format_rfc3339_seconds(at).to_string(),
        "editors": e.editors.iter()
            .map(|ed| serde_json::json!({"kind": ed.kind.as_str(), "principal": ed.principal}))
            .collect::<Vec<_>>(),
        "wal_from": e.wal_from,
        "wal_to": e.wal_to,
        "summary": e.summary,
        "name": e.name,
        "restored_from": e.restored_from,
        "pruned": e.is_pruned(),
        "content_hash": hex::encode(e.content_hash),
        "entry_hash": hex::encode(e.entry_hash),
    })
}

fn checkpoint_params(params: &serde_json::Value) -> Result<(String, String, String), McpError> {
    let text = |key: &str| {
        params[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| McpError::invalid_params(key, format!("{key} is required")))
    };
    let (kb_id, node_id, name) = (text("kb_id")?, text("node_id")?, text("name")?);
    if name.trim().is_empty() || name.chars().any(char::is_control) {
        return Err(McpError::invalid_params(
            "name",
            "a version name is visible text, with no control characters".into(),
        ));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(McpError::invalid_params(
            "name",
            format!("a version name is at most {MAX_NAME_CHARS} characters"),
        ));
    }
    Ok((kb_id, node_id, name))
}

/// `kb/history.checkpoint {kb_id, node_id, name}`: cut a named version of the
/// node as it stands. Needs `Edit` on the KB; refused for an E2E KB, whose
/// content the daemon cannot read. The requester is recorded in the
/// version's summary: naming is an act, not an edit, so it does not make them
/// one of the version's editors.
pub(super) async fn handle_kb_history_checkpoint(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let (kb_id, node_id, name) = match checkpoint_params(params) {
        Ok(p) => p,
        Err(e) => return JsonRpcResponse::error(id, e),
    };
    let coll = load_collection(doc_store, &kb_id).await.ok();
    let decision = kb_access_with_coll(
        doc_store,
        &kb_id,
        auth_principal,
        KbOp::Edit,
        transport,
        coll.as_ref(),
    )
    .await;
    if let Err(msg) = require_allow(decision, || format!("cannot edit KB '{kb_id}'")) {
        return JsonRpcResponse::error(id, msg.into());
    }
    if let Some(c) = &coll {
        if kb_is_e2e(doc_store, &kb_id, c).await {
            return JsonRpcResponse::error(
                id,
                McpError::e2e_unsupported(format!(
                    "KB '{kb_id}' is end-to-end encrypted: the daemon cannot read its content to version it"
                )),
            );
        }
    }
    let summary = Some(format!(
        "named by {}",
        auth_principal.unwrap_or("unauthenticated")
    ));
    match doc_store
        .history_checkpoint(&kb_id, &node_id, VersionKind::Named, Some(name), summary)
        .await
    {
        Ok(Some(entry)) => JsonRpcResponse::success(id, entry_json(&entry)),
        Ok(None) => JsonRpcResponse::error(
            id,
            McpError::internal_fault("a named version was not written".into()),
        ),
        Err(StorageError::DurableDocMissing(_)) => JsonRpcResponse::error(
            id,
            McpError::doc_not_found(format!("KB '{kb_id}' has no node '{node_id}'")),
        ),
        Err(e) => JsonRpcResponse::error(id, McpError::internal_fault(format!("checkpoint: {e}"))),
    }
}
