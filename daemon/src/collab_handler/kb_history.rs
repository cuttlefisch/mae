//! ADR-116 D7: the version-history methods. `kb/history.checkpoint` cuts a
//! named version (D2.3).

use super::method_authz::Method;
use super::*;
use crate::storage::StorageError;
use mae_sync::kb::history::{verify_chain, HistoryEntry, HistoryKey, VersionKind};
use mae_sync::kb::MaterializedNode;

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
async fn checkpoint(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    params: &serde_json::Value,
) -> Result<serde_json::Value, McpError> {
    let (kb_id, node_id, name) = checkpoint_params(params)?;
    gate(doc_store, &kb_id, auth_principal, transport, KbOp::Edit).await?;
    let summary = Some(format!(
        "named by {}",
        auth_principal.unwrap_or("unauthenticated")
    ));
    match doc_store
        .history_checkpoint(&kb_id, &node_id, VersionKind::Named, Some(name), summary)
        .await
    {
        Ok(Some(entry)) => Ok(entry_json(&entry)),
        Ok(None) => Err(McpError::internal_fault(
            "a named version was not written".into(),
        )),
        Err(StorageError::DurableDocMissing(_)) => Err(McpError::doc_not_found(format!(
            "KB '{kb_id}' has no node '{node_id}'"
        ))),
        Err(e) => Err(McpError::internal_fault(format!("checkpoint: {e}"))),
    }
}

/// The history methods, routed from one dispatch arm (ADR-116 D7).
pub(super) async fn dispatch(
    method: Method,
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    id: serde_json::Value,
    params: &serde_json::Value,
) -> JsonRpcResponse {
    let result = match method {
        Method::KbHistoryCheckpoint => {
            checkpoint(doc_store, auth_principal, transport, params).await
        }
        Method::KbHistoryList => list(doc_store, auth_principal, transport, params).await,
        Method::KbHistoryGet => get(doc_store, auth_principal, transport, params).await,
        Method::KbHistoryDiff => diff(doc_store, auth_principal, transport, params).await,
        Method::KbHistoryVerify => verify(doc_store, auth_principal, transport, params).await,
        other => Err(McpError::method_not_found(format!(
            "{other:?} is not a history method"
        ))),
    };
    match result {
        Ok(v) => JsonRpcResponse::success(id, v),
        Err(e) => JsonRpcResponse::error(id, e),
    }
}

/// What every history method needs: `op` on the KB, and a KB whose content
/// the daemon can read. An E2E KB's history is client-side (D7).
async fn gate(
    doc_store: &DocStore,
    kb_id: &str,
    auth_principal: Option<&str>,
    transport: Transport,
    op: KbOp,
) -> Result<(), McpError> {
    let coll = load_collection(doc_store, kb_id).await.ok();
    let decision = kb_access_with_coll(
        doc_store,
        kb_id,
        auth_principal,
        op,
        transport,
        coll.as_ref(),
    )
    .await;
    require_allow(decision, || {
        format!("cannot use the history of KB '{kb_id}'")
    })
    .map_err(Into::<McpError>::into)?;
    if let Some(c) = &coll {
        if kb_is_e2e(doc_store, kb_id, c).await {
            return Err(McpError::e2e_unsupported(format!(
                "KB '{kb_id}' is end-to-end encrypted: its history is not on the daemon"
            )));
        }
    }
    Ok(())
}

fn text(params: &serde_json::Value, key: &str) -> Result<String, McpError> {
    params[key]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| McpError::invalid_params(key, format!("{key} is required")))
}

fn seq(params: &serde_json::Value, key: &str) -> Result<u64, McpError> {
    params[key]
        .as_u64()
        .filter(|s| *s > 0)
        .ok_or_else(|| McpError::invalid_params(key, format!("{key} is a version seq (1 or more)")))
}

async fn load(
    doc_store: &DocStore,
    kb_id: &str,
    node_id: &str,
) -> Result<Vec<HistoryEntry>, McpError> {
    doc_store
        .history_list(kb_id, node_id)
        .await
        .map_err(|e| McpError::internal_fault(format!("history of '{node_id}' unreadable: {e}")))
}

fn version_at<'a>(
    h: &'a [HistoryEntry],
    seq: u64,
    key: &str,
) -> Result<&'a HistoryEntry, McpError> {
    h.iter()
        .find(|e| e.seq == seq)
        .ok_or_else(|| McpError::doc_not_found(format!("there is no version {seq} ({key})")))
}

/// The KB's history key: content is checked only with it.
async fn kb_key(doc_store: &DocStore, kb_id: &str) -> Result<Option<HistoryKey>, McpError> {
    doc_store
        .history_key(kb_id)
        .await
        .map_err(|e| McpError::internal_fault(format!("history key unreadable: {e}")))
}

/// The chain check of a node's whole history, as the methods report it.
fn chain_status(
    kb_id: &str,
    node_id: &str,
    h: &[HistoryEntry],
    key: Option<&HistoryKey>,
) -> serde_json::Value {
    match verify_chain(kb_id, node_id, h, None, key) {
        Ok(()) => serde_json::json!({ "ok": true }),
        Err(e) => serde_json::json!({ "ok": false, "seq": e.seq, "broken": e.broken.as_str() }),
    }
}

/// `kb/history.list {kb_id, node_id}`: every version's record, without content.
async fn list(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    params: &serde_json::Value,
) -> Result<serde_json::Value, McpError> {
    let (kb_id, node_id) = (text(params, "kb_id")?, text(params, "node_id")?);
    gate(doc_store, &kb_id, auth_principal, transport, KbOp::Read).await?;
    let h = load(doc_store, &kb_id, &node_id).await?;
    Ok(serde_json::json!({
        "kb_id": kb_id,
        "node_id": node_id,
        "versions": h.iter().map(entry_json).collect::<Vec<_>>(),
    }))
}

/// A version's content as a reader shows it, if it holds one that matches its
/// hash. `None` when pruned or when it does not verify.
fn content_json(e: &HistoryEntry, key: &HistoryKey) -> Option<serde_json::Value> {
    let node = e
        .node()?
        .ok()
        .filter(|n| key.content_hash(n) == e.content_hash)?;
    let mut m = crate::kb_query::content_fields(&node);
    m.insert("body".into(), serde_json::json!(node.body));
    m.insert("links".into(), serde_json::json!(node.links));
    m.insert(
        "version".into(),
        serde_json::json!(hex::encode(&node.digest()[..16])),
    );
    Some(serde_json::Value::Object(m))
}

/// `kb/history.get {kb_id, node_id, seq}`: one version's record and content,
/// with the verification of the node's chain.
async fn get(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    params: &serde_json::Value,
) -> Result<serde_json::Value, McpError> {
    let (kb_id, node_id, want) = (
        text(params, "kb_id")?,
        text(params, "node_id")?,
        seq(params, "seq")?,
    );
    gate(doc_store, &kb_id, auth_principal, transport, KbOp::Read).await?;
    let h = load(doc_store, &kb_id, &node_id).await?;
    let e = version_at(&h, want, "seq")?;
    let mut out = entry_json(e);
    let key = kb_key(doc_store, &kb_id).await?;
    out["content"] = key
        .as_ref()
        .and_then(|k| content_json(e, k))
        .unwrap_or(serde_json::Value::Null);
    out["verification"] = chain_status(&kb_id, &node_id, &h, key.as_ref());
    Ok(out)
}

/// `kb/history.diff {kb_id, node_id, from, to}`: the fields that differ
/// between two versions, each of which must hold content that verifies.
async fn diff(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    params: &serde_json::Value,
) -> Result<serde_json::Value, McpError> {
    let (kb_id, node_id) = (text(params, "kb_id")?, text(params, "node_id")?);
    let (from, to) = (seq(params, "from")?, seq(params, "to")?);
    gate(doc_store, &kb_id, auth_principal, transport, KbOp::Read).await?;
    let h = load(doc_store, &kb_id, &node_id).await?;
    let hkey = kb_key(doc_store, &kb_id)
        .await?
        .ok_or_else(|| McpError::internal_fault(format!("KB '{kb_id}' has no history key")))?;
    let node = |s: u64, key: &str| -> Result<MaterializedNode, McpError> {
        let e = version_at(&h, s, key)?;
        let refuse = |why: &str| McpError::invalid_params(key, format!("version {s} {why}"));
        let n = e.node().ok_or_else(|| refuse("is pruned"))?;
        n.ok()
            .filter(|n| hkey.content_hash(n) == e.content_hash)
            .ok_or_else(|| refuse("does not verify"))
    };
    let (a, b) = (node(from, "from")?, node(to, "to")?);
    let changes: Vec<serde_json::Value> = mae_sync::kb::history::diff(&a, &b)
        .into_iter()
        .map(|c| serde_json::json!({ "field": c.field, "change": c.change }))
        .collect();
    Ok(
        serde_json::json!({ "kb_id": kb_id, "node_id": node_id, "from": from, "to": to, "changes": changes }),
    )
}

/// `kb/history.verify {kb_id, node_id?}`: verify one node's chain, or the
/// whole KB (`crate::history_verify`, shared with the operator's command),
/// which also checks the chains against the latest signed head. A head kept
/// outside the daemon is still the only defence against a rewrite by whoever
/// holds the daemon's key (`mae-daemon kb history verify --against`).
async fn verify(
    doc_store: &DocStore,
    auth_principal: Option<&str>,
    transport: Transport,
    params: &serde_json::Value,
) -> Result<serde_json::Value, McpError> {
    let kb_id = text(params, "kb_id")?;
    gate(doc_store, &kb_id, auth_principal, transport, KbOp::Read).await?;
    let unreadable = |e: StorageError| McpError::internal_fault(format!("history unreadable: {e}"));
    let Some(node) = params["node_id"].as_str() else {
        let r = crate::history_verify::verify_kb(doc_store, &kb_id, None)
            .await
            .map_err(unreadable)?;
        let failures: Vec<serde_json::Value> = r
            .failures
            .iter()
            .map(|(n, e)| serde_json::json!({ "node_id": n, "seq": e.seq, "broken": e.broken.as_str() }))
            .collect();
        return Ok(serde_json::json!({
            "kb_id": kb_id, "ok": r.ok(), "nodes": r.nodes, "versions": r.versions,
            "failures": failures, "head": hex::encode(r.head),
            "head_checked": !matches!(r.signed, crate::history_verify::HeadCheck::Unchecked(_)),
            "signed_head": r.signed.json(),
        }));
    };
    let key = kb_key(doc_store, &kb_id).await?;
    let h = load(doc_store, &kb_id, node).await?;
    let failures: Vec<serde_json::Value> = verify_chain(&kb_id, node, &h, None, key.as_ref())
        .err()
        .map(|e| serde_json::json!({ "node_id": node, "seq": e.seq, "broken": e.broken.as_str() }))
        .into_iter()
        .collect();
    Ok(serde_json::json!({
        "kb_id": kb_id, "ok": failures.is_empty(), "nodes": 1, "versions": h.len(),
        "failures": failures, "head": null, "head_checked": false, "signed_head": null,
    }))
}

/// ADR-116 Amendment 1, until history is carried over encrypted: refuse a
/// collection update that would switch `kb_id` to E2E while its history still
/// holds plaintext content, which the switch would leave readable on a daemon
/// that must hold none (ADR-037). The daemon does not interpret the update:
/// it applies it to a copy of the collection and asks whether the KB is E2E
/// before and after. An update that does not apply is left to the write path.
/// A history that cannot be read refuses the switch rather than allowing it.
pub(super) async fn refuse_e2e_switch_over_history(
    doc_store: &DocStore,
    kb_id: &str,
    update: &[u8],
) -> Result<(), McpError> {
    let Ok(before) = load_collection(doc_store, kb_id).await else {
        return Ok(());
    };
    if kb_is_e2e(doc_store, kb_id, &before).await {
        return Ok(());
    }
    let Ok(mut after) = KbCollectionDoc::from_bytes(&before.encode_state()) else {
        return Ok(());
    };
    if after.apply_update(update).is_err() || !kb_is_e2e(doc_store, kb_id, &after).await {
        return Ok(());
    }
    match doc_store.history_holds_content(kb_id).await {
        Ok(false) => Ok(()),
        Ok(true) => Err(McpError::history_holds_plaintext(format!(
            "KB '{kb_id}' cannot switch to end-to-end encryption while its version history \
             holds plaintext content"
        ))),
        Err(e) => Err(McpError::internal_fault(format!(
            "KB '{kb_id}': its version history could not be read to check the switch: {e}"
        ))),
    }
}
