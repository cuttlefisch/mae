//! `kb/query.*` RPC method implementations (ADR-053/Phase G, #382) — the live
//! scoped read-through KB query surface served by the OAuth HTTPS listener
//! (`daemon/src/oauth.rs`). Reuses `collab_handler`'s `check_kb_read_access`
//! (Read-only gate) and `load_collection`, and `DocStore`'s existing
//! `kbc:{kb_id}`/`kb:{node_id}` document model — the SAME data a hub-hosted,
//! collaboratively-shared KB uses. Deliberately NOT `daemon/src/handler.rs`'s
//! `KbQueryLayer`/`CozoKbStore` — that machinery serves the daemon's
//! locally-federated KB instances, a structurally different data model (see
//! ADR-053's Implementation-note addendum).
//!
//! Encryption-aware by construction: every method resolves `Encryption::{None|
//! E2e}` from the KB's SIGNED op-log (`membership::derive_encryption` — the
//! trustworthy source; the unsigned collection-doc flag is spoofable by a
//! relay and never consulted here) before touching any node content, and
//! branches accordingly. For `Encryption::E2e`, `search`/`graph` never even
//! attempt server-side plaintext operations — this is a structural property
//! of the code path taken, not a runtime check that could be bypassed.

use mae_daemon::collab_handler::{self, AccessDecision};
use mae_daemon::doc_store::DocStore;
use mae_mcp::protocol::McpError;
use mae_sync::encoding::update_to_base64;
use mae_sync::kb::{Encryption, KbCollectionDoc, KbNodeDoc, Transport};
use mae_sync::membership;
use serde_json::{json, Value};

/// Per-call caps (ADR-053 decision 3) — config-driven (principle #7), never
/// hardcoded; threaded in from `OAuthConfig`'s `kb_query_*` fields.
#[derive(Debug, Clone, Copy)]
pub struct KbQueryLimits {
    pub max_body_bytes: usize,
    pub max_scan_nodes: usize,
    pub max_search_results: usize,
}

/// Dispatch one `kb/query.*` method. `principal` is always `Some` in
/// practice — the caller has already validated a bearer token before
/// reaching here — but kept as `Option` to match `check_kb_read_access`'s
/// own signature exactly, so this boundary never silently reinterprets it.
pub async fn dispatch(
    method: &str,
    params: &Value,
    doc_store: &DocStore,
    principal: Option<&str>,
    limits: KbQueryLimits,
) -> Result<Value, McpError> {
    let kb_id = params
        .get("kb_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| McpError::invalid_request("missing 'kb_id'".to_string()))?
        .to_string();

    match method {
        "kb/query.capabilities" => capabilities(doc_store, &kb_id, principal).await,
        "kb/query.get" => {
            let node_id = params
                .get("node_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::invalid_request("missing 'node_id'".to_string()))?
                .to_string();
            get(
                doc_store,
                &kb_id,
                &node_id,
                principal,
                limits.max_body_bytes,
            )
            .await
        }
        "kb/query.search" => {
            let query = params
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::invalid_request("missing 'query'".to_string()))?
                .to_string();
            let limit = params
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize)
                .unwrap_or(limits.max_search_results)
                .min(limits.max_search_results)
                .max(1);
            search(
                doc_store,
                &kb_id,
                &query,
                limit,
                principal,
                limits.max_scan_nodes,
            )
            .await
        }
        "kb/query.graph" => graph(doc_store, &kb_id, principal, limits.max_scan_nodes).await,
        other => Err(McpError::method_not_found(format!(
            "unknown kb/query method '{other}'"
        ))),
    }
}

/// Gate Read access, then load a collection-doc snapshot + resolve its
/// (signed, trustworthy) encryption mode — the common prefix every method
/// needs, encryption resolved BEFORE any node content is touched. Two small
/// `DocStore` reads of `kbc:{kb_id}` (one inside the gate, one here) rather
/// than a `_with_coll` variant threaded through the public wrapper — this
/// isn't a hot path, and simplicity wins over saving one cheap read.
async fn load_gated(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
) -> Result<(KbCollectionDoc, Encryption), McpError> {
    match collab_handler::check_kb_read_access(doc_store, kb_id, principal, Transport::Hub).await {
        Ok(AccessDecision::Allow) | Ok(AccessDecision::AllowAutoJoin) => {}
        Ok(AccessDecision::Deny(msg)) | Err(msg) => return Err(McpError::internal_error(msg)),
        Ok(_) => {
            return Err(McpError::internal_error(format!(
                "cannot read KB '{kb_id}'"
            )))
        }
    }

    let coll = collab_handler::load_collection(doc_store, kb_id)
        .await
        .map_err(McpError::internal_error)?;

    // Trustworthy encryption mode: derived from the SIGNED op-log, never the
    // unsigned collection-doc flag (a key-blind relay could spoof that).
    // `resolve_content_anchor` mirrors `kb_access`'s own owned-vs-joined
    // anchor resolution exactly (principle #8).
    let encryption = match collab_handler::resolve_content_anchor(doc_store, kb_id).await {
        Some(anchor) => membership::derive_encryption(&coll.oplog_ops(), &anchor),
        // No anchor at all (never anchored, not owned by this daemon either)
        // means no signed genesis exists to latch E2e from.
        None => Encryption::None,
    };

    Ok((coll, encryption))
}

async fn capabilities(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
) -> Result<Value, McpError> {
    let (_coll, encryption) = load_gated(doc_store, kb_id, principal).await?;
    Ok(json!({
        "kb_id": kb_id,
        "encryption": encryption.as_str(),
        "searchable": matches!(encryption, Encryption::None),
    }))
}

/// Truncate `s` to at most `max_bytes` bytes without splitting a multi-byte
/// UTF-8 character (QA-pass finding: `max_body_bytes` is documented and
/// configured as a byte cap, but the original implementation truncated by
/// `.chars().take(n)` — up to 4x overshoot on multi-byte content, e.g. a
/// body of all 4-byte emoji would produce up to `4 * max_body_bytes` bytes
/// on the wire, silently defeating the cap's actual purpose).
fn truncate_to_byte_boundary(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

async fn get(
    doc_store: &DocStore,
    kb_id: &str,
    node_id: &str,
    principal: Option<&str>,
    max_body_bytes: usize,
) -> Result<Value, McpError> {
    let (_coll, encryption) = load_gated(doc_store, kb_id, principal).await?;

    let node_doc = format!("kb:{node_id}");
    let (state, _sv) = doc_store
        .encode_state_and_sv(&node_doc)
        .await
        .map_err(|e| McpError::internal_error(format!("node fetch failed for '{node_id}': {e}")))?;

    match encryption {
        Encryption::None => {
            let doc = KbNodeDoc::from_bytes(&state)
                .map_err(|e| McpError::internal_error(format!("bad node doc: {e}")))?;
            let body = doc.body();
            let truncated = body.len() > max_body_bytes;
            let body = if truncated {
                truncate_to_byte_boundary(&body, max_body_bytes)
            } else {
                body
            };
            Ok(json!({
                "kb_id": kb_id,
                "node_id": node_id,
                "encryption": "none",
                "title": doc.title(),
                "body": body,
                "body_truncated": truncated,
                "tags": doc.tags(),
                "links": doc.links(),
            }))
        }
        Encryption::E2e => {
            // The daemon cannot decrypt this -- `state` IS the op-set
            // ciphertext (a yrs YMap<op_id, encrypted_blob>), not a
            // KbNodeDoc. Returned as-is for a genuine KB member to decrypt
            // client-side with a key only they hold (a non-member never
            // reaches this branch -- `load_gated` already denied them).
            Ok(json!({
                "kb_id": kb_id,
                "node_id": node_id,
                "encryption": "e2e",
                "ciphertext_b64": update_to_base64(&state),
            }))
        }
    }
}

async fn search(
    doc_store: &DocStore,
    kb_id: &str,
    query: &str,
    limit: usize,
    principal: Option<&str>,
    max_scan_nodes: usize,
) -> Result<Value, McpError> {
    let (coll, encryption) = load_gated(doc_store, kb_id, principal).await?;

    if !matches!(encryption, Encryption::None) {
        // Structural refusal, checked before touching a single node -- never
        // an attempt that silently returns empty results.
        return Err(McpError::invalid_request(format!(
            "server-side search is unavailable for E2E-encrypted KB '{kb_id}'; use \
             kb/query.get with member-side decryption instead"
        )));
    }

    let needle = query.to_lowercase();
    let nodes = coll.list_nodes(); // Vec<(node_id, title)>
    let scanned = nodes.len().min(max_scan_nodes);
    let mut results = Vec::new();
    for (node_id, _manifest_title) in nodes.into_iter().take(max_scan_nodes) {
        if results.len() >= limit {
            break;
        }
        let node_doc = format!("kb:{node_id}");
        let Ok((state, _sv)) = doc_store.encode_state_and_sv(&node_doc).await else {
            continue; // a manifest entry with no materialized doc yet -- skip, not an error
        };
        let Ok(doc) = KbNodeDoc::from_bytes(&state) else {
            continue;
        };
        let title = doc.title();
        let body = doc.body();
        if title.to_lowercase().contains(&needle) || body.to_lowercase().contains(&needle) {
            let excerpt: String = body.chars().take(200).collect();
            results.push(json!({
                "id": node_id,
                "title": title,
                "excerpt": excerpt,
            }));
        }
    }

    Ok(json!({
        "kb_id": kb_id,
        "results": results,
        "scanned": scanned,
    }))
}

async fn graph(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    max_scan_nodes: usize,
) -> Result<Value, McpError> {
    let (coll, encryption) = load_gated(doc_store, kb_id, principal).await?;
    let nodes = coll.list_nodes();
    let node_ids: Vec<String> = nodes
        .iter()
        .map(|(id, _)| id.clone())
        .take(max_scan_nodes)
        .collect();

    match encryption {
        Encryption::None => {
            // QA-pass finding: `node_ids` is already capped by
            // `max_scan_nodes`, but a single "hub" node can hold an
            // unbounded number of outgoing links -- without a separate cap
            // on the total edge count, a densely-linked KB could still
            // produce a full-dump-sized response through this path alone.
            // Reuses `max_scan_nodes` as the edge budget too (same "capped,
            // never unbounded" property the whole surface already commits
            // to, no new config knob needed for a v1 cap) rather than a
            // fresh magic number.
            let mut edges = Vec::new();
            let mut edges_truncated = false;
            'scan: for id in &node_ids {
                let node_doc = format!("kb:{id}");
                if let Ok((state, _sv)) = doc_store.encode_state_and_sv(&node_doc).await {
                    if let Ok(doc) = KbNodeDoc::from_bytes(&state) {
                        for link in doc.links() {
                            if edges.len() >= max_scan_nodes {
                                edges_truncated = true;
                                break 'scan;
                            }
                            edges.push(json!([id, link]));
                        }
                    }
                }
            }
            Ok(json!({
                "kb_id": kb_id,
                "encryption": "none",
                "nodes": node_ids,
                "edges": edges,
                "edges_truncated": edges_truncated,
            }))
        }
        Encryption::E2e => {
            // Node existence only -- `links` lives inside the same encrypted
            // KbNodeDoc schema as title/body, so the daemon has no separate
            // plaintext link registry to read instead. No edges, ever, for
            // an E2E KB through this surface.
            Ok(json!({
                "kb_id": kb_id,
                "encryption": "e2e",
                "nodes": node_ids,
                "edges": [],
                "edges_truncated": false,
            }))
        }
    }
}
