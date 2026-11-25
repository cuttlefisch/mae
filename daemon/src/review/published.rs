//! ADR-113 D5's `view` on `kb/query.get` and `kb/query.list`: `"working"`
//! (the default, unchanged) or `"published"`, which serves each node's
//! in-force revision from its frozen source. A reader builds what it shows
//! from published revisions alone: a node with none (never approved, or
//! derogated) is not in this view.
//!
//! With review off, and on an end-to-end encrypted KB (which review never
//! serves), the published view is the working copy: these functions answer
//! `None` and `kb/query` serves the working view.

use mae_mcp::protocol::McpError;
use mae_sync::kb::history::decode_content;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::reads::revision_reply;
use super::store::{self, Revision};
use super::{gate, storage_fault, time, Gated};
use crate::doc_store::DocStore;

/// Whether `params` asks for the published view. `view` is `"working"`
/// (the default) or `"published"`; anything else is refused.
pub fn wants_published(params: &Value) -> Result<bool, McpError> {
    match params.get("view") {
        None | Some(Value::Null) => Ok(false),
        Some(Value::String(v)) if v == "working" => Ok(false),
        Some(Value::String(v)) if v == "published" => Ok(true),
        Some(_) => Err(McpError::invalid_params(
            "view",
            "view is \"working\" or \"published\"".into(),
        )),
    }
}

/// The gate, for the published view: `None` when the view is the working
/// copy (review off, or an E2E KB, which review never serves).
async fn published_gate(
    store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
) -> Result<Option<Gated>, McpError> {
    let g = match gate(store, kb_id, principal, false).await {
        Ok(g) => g,
        Err(e) if kind_of(&e) == "e2e_unsupported" => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(g.policy.review_enabled.then_some(g))
}

fn kind_of(e: &McpError) -> &str {
    e.data
        .as_deref()
        .and_then(|d| d["kind"].as_str())
        .unwrap_or("")
}

/// `kb/query.get` with `view: "published"`: the node's in-force revision, as
/// a revision read returns it. A node with none is `node_not_in_kb`.
pub async fn get(
    store: &DocStore,
    kb_id: &str,
    node_id: &str,
    principal: Option<&str>,
    max_body_bytes: usize,
) -> Result<Option<Value>, McpError> {
    let Some(g) = published_gate(store, kb_id, principal).await? else {
        return Ok(None);
    };
    let (kb, node) = (kb_id.to_string(), node_id.to_string());
    let in_force = store
        .review_read(move |c| store::node_revisions_in(c, &kb, &node, &["in_force"]))
        .await
        .map_err(storage_fault)?;
    let Some(r) = in_force.last() else {
        return Err(McpError::node_not_in_kb(format!(
            "node '{node_id}' has no published revision in KB '{kb_id}'"
        )));
    };
    let reply = revision_reply(store, &g, kb_id, &r.revision_id, None, max_body_bytes).await?;
    Ok(Some(reply))
}

/// `kb/query.list` with `view: "published"`: the nodes with an in-force
/// revision, in node-id byte order after `after`, `limit` at a time, each with
/// the working view's summary fields taken from that revision, its `version`
/// and a revision summary. A frozen source that no longer matches its record
/// is listed `unavailable`, never left out.
pub async fn list(
    store: &DocStore,
    kb_id: &str,
    after: Option<&str>,
    limit: usize,
    principal: Option<&str>,
) -> Result<Option<Value>, McpError> {
    let Some(g) = published_gate(store, kb_id, principal).await? else {
        return Ok(None);
    };
    let (kb, from) = (kb_id.to_string(), after.unwrap_or("").to_string());
    let page = store
        .review_read(move |c| {
            let mut out = Vec::new();
            for r in store::in_force_after(c, &kb, &from, limit + 1)? {
                let src = store::source(c, &kb, &r.source_sha256)?;
                out.push((r, src));
            }
            Ok(out)
        })
        .await
        .map_err(storage_fault)?;
    let more = page.len() > limit;
    let format = g.coll.body_format();
    let nodes: Vec<Value> = page
        .into_iter()
        .take(limit)
        .map(|(r, src)| entry(&r, src.as_deref(), format.as_deref()))
        .collect();
    let next_after = more
        .then(|| {
            nodes
                .last()
                .and_then(|n| n["id"].as_str())
                .map(str::to_string)
        })
        .flatten();
    Ok(Some(json!({
        "kb_id": kb_id,
        "encryption": "none",
        "view": "published",
        "nodes": nodes,
        "next_after": next_after,
    })))
}

/// One node of the published list, from its in-force revision `r`.
fn entry(r: &Revision, src: Option<&[u8]>, body_format: Option<&str>) -> Value {
    let unavailable = || json!({"id": r.node_id, "unavailable": true});
    let Some(src) = src else {
        return unavailable();
    };
    if hex::encode(Sha256::digest(src)) != r.source_sha256 {
        return unavailable();
    }
    let Ok(node) = decode_content(src) else {
        return unavailable();
    };
    if hex::encode(&node.digest()[..16]) != r.version {
        return unavailable();
    }
    let mut m: Map<String, Value> = crate::kb_query::content_fields(&node, body_format);
    m.insert("id".into(), json!(r.node_id));
    m.insert("version".into(), json!(r.version));
    m.insert(
        "revision".into(),
        json!({
            "revision_id": r.revision_id,
            "doc_code": r.doc_code,
            "rev": r.rev_no,
            "state": r.state,
            "approved_at": r.approved_at.map(time),
            "in_force_at": r.in_force_since.map(time),
        }),
    );
    Value::Object(m)
}
