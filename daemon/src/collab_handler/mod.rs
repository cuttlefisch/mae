//! Client connection handler for the collab server.
//!
//! Each TCP (or Unix) client gets its own tokio task running this handler.
//! Uses `mae_mcp::read_message` for framing and `mae_mcp::write_framed`
//! for responses. Protocol methods (initialize, ping, subscribe) are
//! delegated to `mae_mcp::handle_request`. Sync methods are handled locally
//! by dispatching to the DocStore.
//!
//! @ai-caution: [architecture-debt] JSON-RPC router. `handle_doc_request_inner`
//! is now a thin ~340-line dispatcher — its `sync/*`, `docs/*`, and `kb/*` match arms
//! live in the sibling `sync_methods`/`docs_methods`/`kb_membership`/
//! `kb_content`/`kb_governance` modules, grouped by domain (same pattern as
//! `crates/core/src/editor/kb_ops/`). This file still went from 3,821 to
//! ~1,934 lines: the residual is ~30 individually-reasonable auth/session/
//! access-control functions (`run_session`, `verify_content_op`, `kb_access`,
//! `verify_member_self_service_update`, etc.) that collectively exceed the
//! ceiling — a candidate for a further domain-grouping split, not attempted
//! in the 2026-07 pass. Its test module was split into `tests/` (per-feature
//! files, all under the 500-line ceiling). Tracked in
//! `docs/AUDIT_BASELINE.json` (machine-checked) and ROADMAP.md's
//! "Architecture Debt" section.

mod access;
mod docs_methods;
mod kb_artifacts;
mod kb_content;
mod kb_governance;
mod kb_id_guard;
pub mod kb_lease;
mod kb_membership;
mod kb_share_ownership;
mod method_authz;
mod sync_methods;

pub(crate) use access::*;

use std::collections::HashSet;
use std::sync::Arc;

use mae_mcp::broadcast::{EditorEvent, SharedBroadcaster};
use mae_mcp::identity::{AuthorizedKeys, PeerIdentity};
use mae_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpError, ToolInfo};
use mae_mcp::session::ClientSession;
use mae_mcp::{McpToolRequest, McpToolResult};
use mae_sync::content_ops::SignedContentOp;
use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::kb::{
    derive_kb_client_id, update_new_op_authors, JoinPolicy, KbCollectionDoc, Role as SyncRole,
    Transport,
};
use mae_sync::membership::{
    fingerprint_of, is_recovery_rebind, recovery_registry, Governance, MembershipAction,
    ReplicationPolicy,
};
use tokio::io::{AsyncBufRead, AsyncWrite};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::doc_store::DocStore;
use mae_mcp::auth::AuthProvider;

/// Write timeout for event notifications to clients (seconds).
const WRITE_TIMEOUT_SECS: u64 = 5;
/// Disconnect client after this many consecutive write failures.
const MAX_CONSECUTIVE_WRITE_FAILURES: u32 = 3;
/// #342: deadline for a client to complete its auth handshake (TLS handshake, or
/// the plaintext JSON `KeyAuth`/`PskAuth` exchange) after the TCP connection is
/// accepted. A real client completes this near-instantly; an accepted-but-silent
/// connection (deliberate, or just a stalled network) would otherwise park a
/// task+socket forever with nothing to reclaim it. `pub` — also used by the TLS
/// accept call in `main.rs`, which happens before this module's own handshake
/// logic runs for the plaintext auth paths.
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 10;

/// Run the client handler with an authentication handshake before the main loop.
///
/// The auth handshake runs on the raw stream before JSON-RPC `initialize`.
/// If auth fails, the connection is dropped without entering the main loop.
#[allow(clippy::too_many_arguments)]
pub async fn handle_client_with_auth<R, W, A>(
    mut reader: R,
    mut writer: W,
    auth: &A,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
    transport: Transport,
    artifact_store: Arc<dyn crate::artifact_store::ArtifactStore>,
    quota: Arc<dyn crate::quota::QuotaCharger>,
    kb_query_limits: crate::kb_query::KbQueryLimits,
    self_issue: Option<crate::oauth_self_issue::SelfIssueConfig>,
) where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send,
    A: AuthProvider,
{
    let handshake = tokio::time::timeout(
        std::time::Duration::from_secs(HANDSHAKE_TIMEOUT_SECS),
        auth.server_handshake(&mut reader, &mut writer),
    )
    .await;
    let peer = match handshake {
        Ok(Ok(result)) => {
            info!(
                auth = auth.name(),
                client = %result.client_label,
                "auth handshake succeeded"
            );
            // The JSON handshake proves a credential but carries no public key,
            // so bind a synthetic identity from the authenticated label.
            PeerIdentity::synthetic(&result.client_label)
        }
        Err(_elapsed) => {
            warn!(
                auth = auth.name(),
                timeout_secs = HANDSHAKE_TIMEOUT_SECS,
                "auth handshake timed out, dropping connection"
            );
            return;
        }
        Ok(Err(e)) => {
            warn!(auth = auth.name(), error = %e, "auth handshake failed, dropping connection");
            return;
        }
    };
    handle_client_authenticated(
        reader,
        writer,
        peer,
        doc_store,
        broadcaster,
        start_time,
        transport,
        artifact_store,
        quota,
        kb_query_limits,
        self_issue,
    )
    .await;
}

/// Anonymous (no-auth) connection — used for the loopback/`none` mode.
#[allow(clippy::too_many_arguments)]
pub async fn handle_client<R, W>(
    reader: R,
    writer: W,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
    transport: Transport,
    artifact_store: Arc<dyn crate::artifact_store::ArtifactStore>,
    quota: Arc<dyn crate::quota::QuotaCharger>,
    kb_query_limits: crate::kb_query::KbQueryLimits,
    self_issue: Option<crate::oauth_self_issue::SelfIssueConfig>,
) where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    run_session(
        ClientSession::new(),
        reader,
        writer,
        doc_store,
        broadcaster,
        start_time,
        transport,
        artifact_store,
        quota,
        kb_query_limits,
        self_issue,
    )
    .await;
}

/// Authenticated connection — binds `peer` (from mTLS or the JSON handshake) to
/// the session so attribution + KB membership reflect the verified identity.
#[allow(clippy::too_many_arguments)]
pub async fn handle_client_authenticated<R, W>(
    reader: R,
    writer: W,
    peer: PeerIdentity,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
    transport: Transport,
    artifact_store: Arc<dyn crate::artifact_store::ArtifactStore>,
    quota: Arc<dyn crate::quota::QuotaCharger>,
    kb_query_limits: crate::kb_query::KbQueryLimits,
    self_issue: Option<crate::oauth_self_issue::SelfIssueConfig>,
) where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    run_session(
        ClientSession::with_identity(peer),
        reader,
        writer,
        doc_store,
        broadcaster,
        start_time,
        transport,
        artifact_store,
        quota,
        kb_query_limits,
        self_issue,
    )
    .await;
}

/// Run the client handler loop for a single connection.
///
/// Generic over reader/writer — works with TCP, TLS, Unix, or any async stream.
///
/// CANCEL-SAFETY: `read_message` uses `read_line` / `read_exact` internally,
/// which are NOT cancel-safe — if a `tokio::select!` cancels them mid-read the
/// BufReader is left in a corrupted state (header consumed, body still pending).
/// To avoid this, we spawn a dedicated reader task that feeds complete messages
/// into an mpsc channel, so `read_message` always runs to completion.
#[allow(clippy::too_many_arguments)]
async fn run_session<R, W>(
    mut session: ClientSession,
    reader: R,
    mut writer: W,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
    transport: Transport,
    artifact_store: Arc<dyn crate::artifact_store::ArtifactStore>,
    quota: Arc<dyn crate::quota::QuotaCharger>,
    kb_query_limits: crate::kb_query::KbQueryLimits,
    self_issue: Option<crate::oauth_self_issue::SelfIssueConfig>,
) where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin,
{
    let write_timeout = std::time::Duration::from_secs(WRITE_TIMEOUT_SECS);

    let session_id = session.id;
    // The authoritative access-control **principal** (ADR-018): the key fingerprint
    // (or psk:<keyid>), never the mutable label. KB ownership/membership key on this.
    let auth_principal: Option<String> = session.authenticated_principal().map(str::to_string);
    // The display label (key/TLS sessions) — logging/attribution only.
    let auth_label: Option<String> = session.authenticated_label().map(str::to_string);
    // ADR-038: the authenticated peer's Ed25519 public key — captured here so a `kb/join`
    // can record it in the pending request for the owner to wrap the content key to.
    let auth_pubkey: Option<[u8; 32]> = session.peer_identity.as_ref().map(|p| p.pubkey);
    if let Some((principal, label)) = session.principal_and_label() {
        info!(session = session_id, principal, peer = %label, "authenticated peer");
    }
    info!(session = session_id, "collab client connected");

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
        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
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
                // ADR-087 / audit #594: JSON-RPC message content can carry
                // arbitrary UTF-8 (e.g. synced document text); a fixed byte
                // cut can land mid-character and panic. `daemon` is a
                // separate workspace (ADR-014) that doesn't depend on
                // `mae-core`, so this uses the stable stdlib equivalent
                // directly rather than pull in a cross-workspace dependency
                // for one debug-log preview.
                let preview_end = msg.floor_char_boundary(msg.len().min(120));
                debug!(session = session_id, msg_len = msg.len(),
                    is_doc, is_notif,
                    preview = &msg[..preview_end],
                    "dispatch: message classified");

                // Check if this is a sync/* method we handle differently.
                // WU1: Detect notifications (no `id`) before dispatching.
                // Notifications must not generate a response — handle and continue.
                if is_doc && is_notif {
                    debug!(session = session_id, "notification detected, handling without response");
                    handle_doc_notification_inner(&msg, &doc_store, &broadcaster, session_id, auth_label.as_deref(), auth_principal.as_deref(), &mut session_docs).await;
                    continue;
                }

                let mut response = if is_doc {
                    handle_doc_request_inner(&msg, &doc_store, quota.as_ref(), &broadcaster, start_time, session_id, auth_label.as_deref(), auth_principal.as_deref(), auth_pubkey.as_ref(), &mut session_docs, transport, artifact_store.as_ref(), kb_query_limits, self_issue.clone()).await
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
                            let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
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
            let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
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
        "collab client session ended"
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
        || msg.contains("\"kb/")
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
#[cfg(test)]
async fn handle_doc_notification(
    msg: &str,
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    session_docs: &mut HashSet<String>,
) {
    handle_doc_notification_inner(
        msg,
        doc_store,
        broadcaster,
        session_id,
        None,
        None,
        session_docs,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn handle_doc_notification_inner(
    msg: &str,
    _doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    auth_label: Option<&str>,
    // Used by the raw-`kbc:`/`kb:` sync/update owner gate (membership-smuggling
    // defense); wired with the abuse tests.
    _auth_principal: Option<&str>,
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
                let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                bc.subscribe_doc(session_id, &doc_name);
                bc.broadcast_except(
                    &EditorEvent::AwarenessUpdate {
                        doc_id: doc_name,
                        client_id: session_id,
                        // Strict binding: an authenticated peer's cursor label is
                        // its verified identity, not a self-claimed name.
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
        }
        // Methods the daemon only handles as REQUESTS (apply/persist/respond). If one
        // arrives here it was sent without an `id` — a client protocol bug that would
        // otherwise be silently dropped (exactly the ADR-020 B-8 kb/node_update bug).
        // Make it LOUD so the next such regression is caught immediately, not chased.
        "sync/update"
        | "sync/full_state"
        | "sync/state_vector"
        | "sync/share"
        | "sync/resync"
        | "kb/node_update"
        | "kb/node_fetch"
        | "kb/share"
        | "kb/join"
        | "kb/leave"
        | "kb/collection_node_add"
        | "kb/collection_node_remove"
        | "kb/add_member"
        | "kb/remove_member"
        | "kb/approve_member"
        | "kb/collection_op"
        | "kb/claim_lease"
        | "kb/fetch_artifact"
        | "kb/set_policy"
        | "kb/set_governance"
        | "kb/block_principal"
        | "kb/unblock_principal"
        | "kb/blocklist"
        | "kb/revoke" => {
            warn!(
                session = session_id,
                method,
                "DROPPED: request-only doc method received as a notification (missing `id`) — \
                 the client must send this as a JSON-RPC request; nothing was applied"
            );
        }
        _ => {
            debug!(session = session_id, method, "unhandled doc notification");
        }
    }
}

/// Anonymous wrapper used by the test suite (no authenticated identity).
#[cfg(test)]
async fn handle_doc_request(
    msg: &str,
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    start_time: std::time::Instant,
    session_id: u64,
    session_docs: &mut HashSet<String>,
) -> JsonRpcResponse {
    handle_doc_request_inner(
        msg,
        doc_store,
        &crate::quota::NoQuota,
        broadcaster,
        start_time,
        session_id,
        None,
        None,
        None,
        session_docs,
        Transport::Hub,
        &crate::artifact_store::NoArtifactStore,
        crate::kb_query::KbQueryLimits::default(),
        None,
    )
    .await
}

/// A KB operation, for the access engine (ADR-018).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KbOp {
    Join,
    Read,
    Edit,
    Manage,
}

/// The access decision (ADR-018). `AllowAutoJoin` = a permissive-policy non-member
/// the caller must add as a viewer; `Pending` = an invite-policy non-member to be
/// recorded for owner approval. `pub` (ADR-053/Phase G, #382) so
/// `check_kb_read_access` can return it across the `oauth` module boundary — `KbOp`
/// itself stays private, since nothing outside this module needs to construct one.
#[derive(Debug, PartialEq, Eq)]
pub enum AccessDecision {
    Allow,
    AllowAutoJoin,
    Pending,
    Deny(String),
}

/// Load the collection doc for `kb_id` (`kbc:{kb_id}`).
///
/// ADR-018 (#73): a legacy v1 (label-based) collection is migrated to the v2
/// fingerprint-anchored schema right here, via `authorized_keys` label resolution —
/// not only on owner re-share (`set_owner`) as before. `migrate_if_legacy` is a
/// no-op once the collection is already v2, so this is safe to run on every load.
///
/// `pub` (ADR-053/Phase G, #382): the `kb_query` module (bin crate) needs its own
/// collection-doc snapshot for node listing/encryption resolution after
/// `check_kb_read_access` has already gated the call — a second small load of the
/// same doc, not a second gate; simpler than threading a `_with_coll` variant
/// through the public wrapper for a call path that isn't hot.
pub async fn load_collection(doc_store: &DocStore, kb_id: &str) -> Result<KbCollectionDoc, String> {
    let collection_doc = format!("kbc:{kb_id}");
    // ADR-105: a KB that does not exist must NOT be brought into existence by
    // someone asking about it. `encode_state_and_sv` goes through `get_or_create`,
    // so without this every caller — `kb_access` included — materialized an empty
    // `kbc:{kb_id}` for any id handed to it, and the "not found" error just below
    // was unreachable. That enabled a pre-squat which DEFEATS D5 rather than being
    // caught by it; the chain is spelled out in
    // `collab_handler_kb_share_ownership_tests`.
    //
    // `has_durable_doc`, never `has_doc`: a collection is memory-evicted when idle
    // and lazy-reloaded (ADR-032 A2), so the memory-only check would report "not
    // found" for a live KB and turn this guard into an intermittent outage.
    if !doc_store.has_durable_doc(&collection_doc).await {
        return Err(format!("KB '{kb_id}' not found"));
    }
    let (state, _sv) = doc_store
        .encode_state_and_sv(&collection_doc)
        .await
        .map_err(|e| format!("KB '{kb_id}' not found: {e}"))?;
    let mut coll =
        KbCollectionDoc::from_bytes(&state).map_err(|e| format!("bad collection: {e}"))?;
    if let Some(ak_path) = doc_store.authorized_keys_path() {
        // I-10: re-read fresh so a since-authorized label resolves to its key.
        let authorized = AuthorizedKeys::load(ak_path);
        let resolver = |label: &str| {
            authorized
                .lookup_by_label(label)
                .map(|pk| (pk.fingerprint(), label.to_string()))
        };
        if let Some(update) = coll.migrate_if_legacy(resolver) {
            if let Err(e) = doc_store.apply_update(&collection_doc, &update, None).await {
                warn!(kb_id, error = %e, "failed to persist legacy-collection migration (ADR-018 #73)");
            } else {
                info!(
                    kb_id,
                    "migrated legacy v1 collection to v2 identity-anchored schema (ADR-018 #73)"
                );
            }
        }
    }
    Ok(coll)
}

/// ADR-036 §D3: verify a signed content op against this peer's **derived, anchored**
/// membership — the content author (from the signed header, not the connection) must
/// be a current Editor+ member at the op's epoch. Shared by the editor→daemon
/// `kb/node_update` path and the daemon→daemon dialer relay path so there is ONE
/// authorship check, not two that could drift (principle #8). `anchor` is the KB's
/// registered trust root; the caller has already established the KB is anchored and
/// decided its policy for *unsigned* ops (the hub accepts them as legacy; the mesh
/// rejects them — ADR-036 migration). Returns `Err` with a human reason on any
/// failure, for the caller to surface (ADR-024). `pub` so the `dialer` (bin crate)
/// shares this exact check on the relay path.
pub async fn verify_content_op(
    doc_store: &DocStore,
    kb_id: &str,
    anchor: &[u8; 32],
    signed: &SignedContentOp,
) -> Result<(), String> {
    verify_content_op_with_coll(doc_store, kb_id, anchor, signed, None).await
}

/// As [`verify_content_op`], but accepts a pre-loaded collection snapshot so a
/// handler that runs several gates on one request loads `kbc:{kb_id}` once. When
/// `coll` is `None` it loads itself — identical to [`verify_content_op`].
pub async fn verify_content_op_with_coll(
    doc_store: &DocStore,
    kb_id: &str,
    anchor: &[u8; 32],
    signed: &SignedContentOp,
    coll: Option<&KbCollectionDoc>,
) -> Result<(), String> {
    let loaded;
    let coll = match coll {
        Some(c) => c,
        None => {
            loaded = load_collection(doc_store, kb_id).await?;
            &loaded
        }
    };
    let dm = doc_store
        .derived_membership(kb_id, coll, anchor, now_unix())
        .await;
    signed
        .admit(&dm.members)
        .map_err(|e| format!("signed content op rejected: {e:?}"))
}

/// Resolve a KB's content trust anchor (the genesis owner pubkey): the registered
/// external anchor for a JOINED KB, else this daemon's own signer key when it is the
/// collection's owner (an OWNED KB — A is its own authority). `None` when neither
/// holds (un-anchored + not ours), in which case the caller applies the legacy gate.
pub async fn resolve_content_anchor(doc_store: &DocStore, kb_id: &str) -> Option<[u8; 32]> {
    resolve_content_anchor_with_coll(doc_store, kb_id, None).await
}

/// As [`resolve_content_anchor`], but accepts a pre-loaded collection snapshot
/// (used only in the owned-fallback branch). `None` loads itself — identical.
pub async fn resolve_content_anchor_with_coll(
    doc_store: &DocStore,
    kb_id: &str,
    coll: Option<&KbCollectionDoc>,
) -> Option<[u8; 32]> {
    if let Some(a) = doc_store.kb_anchor(kb_id).await {
        return Some(a);
    }
    let signer = doc_store.signer()?;
    let loaded;
    let coll = match coll {
        Some(c) => c,
        None => {
            loaded = load_collection(doc_store, kb_id).await.ok()?;
            &loaded
        }
    };
    if !coll.owner().is_empty() && coll.owner() == signer.fingerprint() {
        Some(signer.public().to_bytes())
    } else {
        None
    }
}

/// ADR-036 §D3 relay-receive verification — the single check shared by the dialer
/// (a joiner receiving the owner's pushes) and the `sync/update` handler (the owner
/// receiving a joiner's relayed edit). `header` is the wire `content_header` (if
/// any). For a `kb:{node}` doc on a KB with a resolvable anchor it reconstructs the
/// signed op (re-binding `kb_id`/`node_id` from trusted local context, so a header
/// signed for a different node fails) and verifies the author is a current Editor+
/// member at the op's epoch. On success returns the header to carry onward; an
/// unsigned op errors when `require_signed` (the mesh policy) and is otherwise
/// accepted as legacy (`Ok(None)`). Non-KB / un-anchored docs pass through (`Ok(None)`).
pub async fn verify_relayed_content_op(
    doc_store: &DocStore,
    kb_id: &str,
    doc: &str,
    update: &[u8],
    header: Option<&serde_json::Value>,
    require_signed: bool,
) -> Result<Option<serde_json::Value>, String> {
    // @ai-caution: [kb-scoping] (ADR-105 D1) Address TYPE, not string prefix — a node
    // doc that stops matching here returns `Ok(None)` ("not a content op"), which
    // SKIPS ADR-036 signature verification entirely.
    let Some(mae_sync::DocAddress::KbNode { node_id, .. }) = mae_sync::DocAddress::parse(doc)
    else {
        return Ok(None); // collection/non-KB doc — not a content op
    };
    let node_id = &node_id;
    let Some(anchor) = resolve_content_anchor(doc_store, kb_id).await else {
        return Ok(None); // un-anchored + not ours — legacy gate applies
    };
    let header = match header {
        Some(h) if h.get("sig").is_some() => h,
        _ => {
            return if require_signed {
                Err(format!(
                    "unsigned content op for KB '{kb_id}' node '{node_id}' rejected on the mesh (ADR-036 require-signed)"
                ))
            } else {
                Ok(None) // hub migration: accept legacy unsigned
            };
        }
    };
    let mut params = header.clone();
    params["kb_id"] = serde_json::json!(kb_id);
    params["node_id"] = serde_json::json!(node_id);
    let signed = SignedContentOp::from_params(&params, update.to_vec())
        .ok_or_else(|| "malformed signed content op header".to_string())?;
    verify_content_op(doc_store, kb_id, &anchor, &signed).await?;
    Ok(Some(signed.header_params()))
}

/// Persist a collection update + broadcast it to other subscribers. Returns wal_seq.
// KLUDGE(#246): persist-then-broadcast is not atomic, and membership propagation is eventually
// consistent — a peer may honor an op from someone just removed until the removal op reaches it.
// This is inherent to CRDT/eventual-consistency (not a fixable bug), but it means access decisions
// are "correct at the derivation point," not globally instantaneous. Security rests on every honest
// peer converging to deny; a hostile peer is handled by the local blocklist (ADR-039), not timing.
async fn persist_and_broadcast_collection(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    kb_id: &str,
    update: &[u8],
) -> Result<u64, String> {
    let collection_doc = format!("kbc:{kb_id}");
    let result = doc_store
        .apply_update(&collection_doc, update, None)
        .await
        .map_err(|e| format!("failed to persist collection: {e}"))?;
    broadcaster
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .broadcast_except(
            &EditorEvent::SyncUpdate {
                buffer_name: collection_doc,
                update_base64: update_to_base64(update),
                wal_seq: result.wal_seq,
                content_header: None,
            },
            session_id,
        );
    Ok(result.wal_seq)
}

/// A coarse monotonic-ish timestamp (unix seconds) for pending-request ordering.
fn now_stamp() -> String {
    now_unix().to_string()
}

/// Unix seconds (0 on a pre-epoch clock).
fn now_unix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Mirror a membership mutation into the KB's **signed op-log** (ADR-026), so peers
/// can verify membership without trusting a relay. A no-op unless this daemon owns
/// the KB — i.e. its key-mode signer's fingerprint equals the collection owner; the
/// relay/hub (psk/none) path stays unsigned. Seeds the genesis owner self-admit
/// first if the log is empty, then appends the op for `subject`, persisting +
/// broadcasting each. The `epoch` is read back from the legacy `member_roles`
/// mutation the caller already applied, so derived and legacy epochs agree.
///
/// Returns [`SignedAppend`] describing what actually happened. A signing/persist
/// failure is still non-fatal *here* — the legacy `member_roles` map remains
/// authoritative until `kb_access` switches to derived membership (slice 2b-6c)
/// — but the outcome is no longer swallowed.
///
/// @ai-caution: [architecture-debt] Callers MUST act on the return value (audit
/// #589.4). Every caller previously discarded it and reported unconditional
/// success, which meant a peer-verifiable membership op could silently fail to
/// reach the signed log while the RPC said it landed. For a caller whose *only*
/// effect is this append (`kb/set_governance`), a swallowed failure is a
/// completely fabricated success; for callers that already persisted a legacy
/// mutation, the honest report is success plus an explicit divergence warning.
#[must_use]
#[allow(clippy::too_many_arguments)]
async fn append_signed_membership(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    kb_id: &str,
    coll: &mut KbCollectionDoc,
    action: MembershipAction,
    subject: &str,
    role: Option<SyncRole>,
    can_invite: bool,
    expires_at: Option<u64>,
) -> SignedAppend {
    let Some(signer) = doc_store.signer() else {
        return SignedAppend::NotOwned;
    };
    let owner = coll.owner();
    if owner.is_empty() || signer.fingerprint() != owner {
        // not an owned KB — the relay/hub path stays unsigned
        return SignedAppend::NotOwned;
    }
    let secret = signer.secret_bytes();
    let pubkey = signer.public().to_bytes();
    let now = now_unix();

    // Seed the genesis owner self-admit (the anchored root) if the log is empty.
    if coll.oplog_head().is_none() {
        let g = coll.build_membership_op(
            kb_id,
            MembershipAction::Admit,
            &owner,
            Some(SyncRole::Owner),
            true,
            &owner,
            now,
            None,
            0,
        );
        let gsig = g.sign(&secret);
        let gupdate = coll.append_signed_op(&g, &gsig, &pubkey);
        if let Err(e) =
            persist_and_broadcast_collection(doc_store, broadcaster, session_id, kb_id, &gupdate)
                .await
        {
            warn!(kb_id = %kb_id, error = %e, "failed to persist membership genesis op");
            return SignedAppend::Failed(format!("membership genesis op not persisted: {e}"));
        }
    }

    // The op mirroring this mutation, authored by the owner (the daemon signs as
    // owner). Epoch = the value the legacy mutation just assigned to `subject`.
    let epoch = coll.epoch_of(subject);
    let op = coll.build_membership_op(
        kb_id, action, subject, role, can_invite, &owner, now, expires_at, epoch,
    );
    let sig = op.sign(&secret);
    let update = coll.append_signed_op(&op, &sig, &pubkey);
    if let Err(e) =
        persist_and_broadcast_collection(doc_store, broadcaster, session_id, kb_id, &update).await
    {
        warn!(kb_id = %kb_id, error = %e, "failed to persist signed membership op");
        return SignedAppend::Failed(format!("signed membership op not persisted: {e}"));
    }
    SignedAppend::Appended
}

/// Outcome of [`append_signed_membership`] — see its `@ai-caution`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SignedAppend {
    /// The op was signed, persisted, and broadcast.
    Appended,
    /// This daemon is not the KB's genesis owner (relay/hub path) — there is no
    /// signed log to append to and nothing failed.
    NotOwned,
    /// Signing or persistence failed; the signed op-log now diverges from the
    /// legacy `member_roles` map.
    Failed(String),
}

impl SignedAppend {
    /// The failure message, if this append was supposed to happen and didn't.
    fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(e) => Some(e),
            Self::Appended | Self::NotOwned => None,
        }
    }
}

/// Append a signed strong-removal (`Revoke`) of `subject`, authored by THIS
/// daemon's own signer — the m-of-n quorum co-sign primitive (ADR-026 §A4).
///
/// Unlike [`append_signed_membership`] (genesis-owner-only, owned-KB housekeeping
/// that mirrors a legacy `member_roles` mutation), any **current `Role::Owner`**
/// member may co-sign here: on a joined/anchored KB each admin's own daemon
/// contributes one *distinct-author* `Revoke`, and `derive_valid_members_governed`
/// tallies them against the `Quorum{threshold}` (a lone admin never reaches it).
/// Op-log-only — the derived gate ([`kb_access`]) is what enforces the removal, so
/// there is no legacy `member_roles` write. Returns `Err` (defense-in-depth behind
/// the `kb_access(Manage)` check) if there is no signer or the signer is not a
/// current owner / `subject` is not a current member.
async fn append_signed_revoke(
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    session_id: u64,
    kb_id: &str,
    coll: &mut KbCollectionDoc,
    subject: &str,
) -> Result<(), String> {
    let signer = doc_store
        .signer()
        .ok_or_else(|| "no signing identity (psk/none mode cannot revoke)".to_string())?;
    if coll.oplog_head().is_none() {
        return Err(format!("KB '{kb_id}' has no signed membership log"));
    }
    // The genesis trust anchor: the external anchor registered for a JOINED KB,
    // else this daemon's own key (it IS the genesis owner of a KB it hosts).
    let anchor = match doc_store.kb_anchor(kb_id).await {
        Some(a) => a,
        None => signer.public().to_bytes(),
    };
    let now = now_unix();
    let dm = doc_store
        .derived_membership(kb_id, coll, &anchor, now)
        .await;
    let signer_fp = signer.fingerprint();
    if dm.members.get(&signer_fp).map(|m| m.role) != Some(SyncRole::Owner) {
        return Err(format!(
            "signer is not a current owner of KB '{kb_id}'; cannot revoke"
        ));
    }
    if !dm.members.contains_key(subject) {
        return Err(format!(
            "'{subject}' is not a current member of KB '{kb_id}'"
        ));
    }
    // Author = THIS signer (so distinct admins tally as distinct co-signatures).
    let secret = signer.secret_bytes();
    let pubkey = signer.public().to_bytes();
    let epoch = coll.epoch_of(subject);
    let op = coll.build_membership_op(
        kb_id,
        MembershipAction::Revoke,
        subject,
        None,
        false,
        &signer_fp,
        now,
        None,
        epoch,
    );
    let sig = op.sign(&secret);
    let update = coll.append_signed_op(&op, &sig, &pubkey);
    persist_and_broadcast_collection(doc_store, broadcaster, session_id, kb_id, &update)
        .await
        .map(|_| ())
}

/// ADR-018 complete-mediation access engine: every KB operation routes through
/// ADR-039 D1 (A1, #157): the AUTHORITATIVE epoch for `principal` — the ADR-023
/// write-fence input. For an **anchored** KB it is derived from the SIGNED op-log
/// (`ValidMember.epoch`), mirroring how `kb_access` derives the *role*, so role and epoch
/// come from ONE authority. The legacy `epoch_of` (member_roles) was wrong for a
/// mesh-admitted member: that map is frozen on join (B-12) so `epoch_of`→0, and the fence
/// then rejected every valid edit by any non-epoch-0 member. Un-anchored/owned KBs keep
/// the legacy `member_roles` epoch (the daemon owns that state). Absent member ⇒ 0.
async fn kb_member_epoch(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
    principal: &str,
) -> u64 {
    match doc_store.kb_anchor(kb_id).await {
        Some(anchor) if coll.oplog_head().is_some() => doc_store
            .derived_membership(kb_id, coll, &anchor, now_unix())
            .await
            .members
            .get(principal)
            .map(|m| m.epoch)
            .unwrap_or(0),
        _ => coll.epoch_of(principal),
    }
}

/// ADR-023 (B-19) epoch fence — the security core. A granted member must author under
/// their **current-epoch** `client_id`; any NEW op (beyond the daemon's authoritative node
/// state) authored under a stale-epoch client_id is rejected — precisely a member's
/// pre-grant divergent lineage (e.g. viewer-era edits) trying to cascade after a grant.
/// `Ok(())` ⇒ passes; `Err(reason)` ⇒ fenced (the caller turns it into a rejection).
///
/// #157 N1: this is the ONE fence shared by every write path — the hub `kb/node_update`
/// AND the mesh dialer relay — so enforcement can't be present on one and absent on the
/// other (complete mediation). The epoch comes from [`kb_member_epoch`] (the signed op-log
/// for anchored KBs — #157 A1).
pub async fn enforce_epoch_fence(
    doc_store: &DocStore,
    kb_id: &str,
    node_id: &str,
    node_doc: &str,
    update_bytes: &[u8],
    principal: &str,
) -> Result<(), String> {
    enforce_epoch_fence_with_coll(
        doc_store,
        kb_id,
        node_id,
        node_doc,
        update_bytes,
        principal,
        None,
    )
    .await
}

/// As [`enforce_epoch_fence`], but accepts a pre-loaded collection snapshot.
/// `None` loads itself — identical to [`enforce_epoch_fence`].
#[allow(clippy::too_many_arguments)]
pub async fn enforce_epoch_fence_with_coll(
    doc_store: &DocStore,
    kb_id: &str,
    node_id: &str,
    node_doc: &str,
    update_bytes: &[u8],
    principal: &str,
    coll: Option<&KbCollectionDoc>,
) -> Result<(), String> {
    let loaded;
    let coll = match coll {
        Some(c) => c,
        None => {
            loaded = load_collection(doc_store, kb_id)
                .await
                .map_err(|e| format!("epoch lookup failed for KB '{kb_id}': {e}"))?;
            &loaded
        }
    };
    let epoch_now = kb_member_epoch(doc_store, kb_id, coll, principal).await;
    let c_now = derive_kb_client_id(principal, epoch_now);
    // Full authoritative state (not just the SV) so the fence detects a contiguous-clock
    // continuation of an already-canonical client (B-20) that the update's own SV hides.
    // ADR-105: a node with NO document yet has no prior ops, so nothing can be a
    // stale-epoch continuation — this is the first update bringing it into existence
    // (join and mesh relay both deliver content that way). Returning early rather
    // than substituting an empty base: `update_new_op_authors` decodes its base as
    // an ENCODED yrs state, so `Vec::new()` fails with "unexpected end of buffer"
    // and turns every first write into a decode error.
    let base_state = match doc_store.encode_state_and_sv(node_doc).await {
        Ok((state, _sv)) => state,
        Err(crate::storage::StorageError::DurableDocMissing(_)) => return Ok(()),
        Err(e) => return Err(format!("node state lookup failed for '{node_id}': {e}")),
    };
    let authors = update_new_op_authors(update_bytes, &base_state)
        .map_err(|e| format!("could not decode update: {e}"))?;
    if let Some(stale) = authors.iter().find(|a| **a != c_now) {
        return Err(format!(
            "rebase required: node '{node_id}' carries an op from stale-epoch client {stale} \
             (current-epoch author is {c_now}, epoch {epoch_now}); adopt authoritative state \
             and re-author the edit"
        ));
    }
    Ok(())
}

/// Membership-smuggling defense (ADR-018): a raw `sync/update` to a collection
/// doc (`kbc:{kb}`) mutates owner/members/policy and is therefore owner-only. The
/// editor only ever touches collections via the gated `kb/*` methods, so a raw
/// `kbc:` write from a non-owner is rejected. Non-collection docs are unaffected.
async fn deny_collection_smuggling(
    doc_store: &DocStore,
    doc_name: &str,
    principal: Option<&str>,
    transport: Transport,
) -> Result<(), String> {
    // @ai-caution: [kb-scoping] (ADR-105 D1) Address TYPE, not string prefix — a
    // collection doc that stops matching here falls through ungated, and this is the
    // ADR-018 membership-smuggling defense (owner-only raw collection writes).
    match mae_sync::DocAddress::parse(doc_name) {
        Some(mae_sync::DocAddress::KbCollection { kb_id }) => {
            match kb_access(doc_store, &kb_id, principal, KbOp::Manage, transport).await? {
                AccessDecision::Allow => Ok(()),
                _ => Err(format!(
                    "only the owner may write the collection doc for KB '{kb_id}'"
                )),
            }
        }
        // Node docs are gated by their own path (`kb/node_update`'s `kb_access` +
        // epoch fence, and `sync/update`'s #169 M1 arm); buffer collab and
        // unrecognized names are not collection docs.
        Some(mae_sync::DocAddress::KbNode { .. })
        | Some(mae_sync::DocAddress::File { .. })
        | Some(mae_sync::DocAddress::Shared { .. })
        | None => Ok(()),
    }
}

/// The single denial message for a node-scope refusal. One function so every
/// call site is byte-identical — see [`require_node_in_kb`] for why that matters.
fn node_scope_denial(kb_id: &str, node_id: &str) -> String {
    format!("node '{node_id}' is not in KB '{kb_id}'")
}

/// Complete-mediation for a CALLER-SUPPLIED `node_id` (#571).
///
/// The `DocStore` doc namespace is FLAT — `kb:{node_id}`, with no `kb_id`
/// component — so gating on `kb_id` authorizes a *KB* while the read then hits
/// a *globally addressed document*. Without this check, a principal with Read
/// on any one KB can read nodes of any other KB co-hosted on the daemon by
/// passing `kb_id = <theirs>, node_id = <someone else's>`.
///
/// The `kbc:{kb_id}` manifest is the authoritative belongs-to relation, and
/// `kb/join` already scopes itself with it (`kb_membership.rs`, "Fetch only the
/// nodes listed in the collection"). The single-node read paths simply never
/// consulted it.
///
/// **MUST be called BEFORE the doc store is touched.** Reading a node also
/// `get_or_create`s it (`doc_store.rs`), so a check placed after the fetch
/// would still let a caller materialize — and pre-squat — arbitrary node ids;
/// and on `kb/node_fetch` the read additionally subscribes the session to that
/// doc's future updates.
///
/// Fail-CLOSED and NON-DISCRIMINATING: a node in another KB, a node that does
/// not exist at all, and a collection that fails to load all produce the SAME
/// message via the SAME error variant the KB gate already uses. Distinguishing
/// them would hand the caller an oracle for probing node ids elsewhere on this
/// daemon. The real reason is logged server-side.
pub async fn require_node_in_kb(
    doc_store: &DocStore,
    kb_id: &str,
    node_id: &str,
    coll: Option<&KbCollectionDoc>,
) -> Result<(), String> {
    let loaded;
    let coll = match coll {
        Some(c) => c,
        None => match load_collection(doc_store, kb_id).await {
            Ok(c) => {
                loaded = c;
                &loaded
            }
            Err(e) => {
                warn!(
                    kb_id,
                    node_id,
                    error = %e,
                    "node-scope check: collection load failed — refusing (fail-closed)"
                );
                return Err(node_scope_denial(kb_id, node_id));
            }
        },
    };
    if coll.has_node(node_id) {
        Ok(())
    } else {
        warn!(
            kb_id,
            node_id, "node-scope check: node is not in this KB's manifest — refused"
        );
        Err(node_scope_denial(kb_id, node_id))
    }
}

/// Complete-mediation for RAW doc READS (`sync/full_state`, `sync/state_vector`). These
/// generic sync methods otherwise return a doc's yrs state for ANY caller-supplied name,
/// bypassing the `kb_access(Read)` gate that `kb/node_fetch`/`kb/join` enforce — a
/// confidentiality hole (a non-member could pull `kb:<node>` plaintext, or `kbc:<kb>` =
/// the roster + pending join pubkeys + node manifest). So: a `kbc:` collection doc is gated
/// on `Read` (members only); a `kb:` node doc is DENIED on this raw path — content is fetched
/// via the access-gated `kb/node_fetch`. Non-KB docs (text buffers / session docs) keep their
/// existing behavior. Fail-closed. The editor only force-syncs BUFFER docs here, so gating KB
/// docs breaks no legitimate flow (KB sync uses `kb/join` + `kb/node_fetch`).
async fn deny_kb_doc_read(
    doc_store: &DocStore,
    doc_name: &str,
    principal: Option<&str>,
    transport: Transport,
) -> Result<(), String> {
    // ADR-105 D1: match the ADDRESS TYPE, never a string prefix. The arms below are
    // exhaustive on purpose — adding or renaming a `DocAddress` variant must fail to
    // compile here rather than silently stop guarding. This gate previously read
    // `doc_name.starts_with("kb:")`, which would have fallen through to `Ok(())` — i.e.
    // handed raw node plaintext to any connected client — the moment the node
    // addressing scheme changed.
    match mae_sync::DocAddress::parse(doc_name) {
        Some(mae_sync::DocAddress::KbCollection { kb_id }) => {
            match kb_access(doc_store, &kb_id, principal, KbOp::Read, transport).await? {
                AccessDecision::Allow => Ok(()),
                _ => Err(format!(
                    "not authorized to read the collection doc for KB '{kb_id}' (members only)"
                )),
            }
        }
        Some(mae_sync::DocAddress::KbNode { .. }) => Err(
            "KB node content must be fetched via the access-gated `kb/node_fetch`, \
             not the raw `sync/full_state` / `sync/state_vector` path"
                .to_string(),
        ),
        // Buffer collab: not KB content, so no KB gate applies. `sync/share` lets a
        // client pick an arbitrary doc name, so an unparseable name is ordinary
        // buffer collaboration, not something suspicious — gating it here would break
        // a real feature. (Tried fail-closed first;
        // `raw_sync_read_of_a_kb_doc_is_access_gated` correctly rejected it for
        // exactly that reason.)
        //
        // The protection this rewrite buys is the EXHAUSTIVE match, not the default
        // arm: KB content can only be written under a `DocAddress::KbNode` /
        // `KbCollection` address, and adding or renaming such a variant now fails to
        // compile here instead of silently falling through to `Ok(())`.
        //
        // @ai-caution: [kb-scoping] If a legacy/compat KB address variant is added
        // during the ADR-105 Stage 4 migration, it MUST be denied here like `KbNode`
        // — a legacy name that stops parsing would otherwise land in this arm and be
        // served raw.
        Some(mae_sync::DocAddress::File { .. })
        | Some(mae_sync::DocAddress::Shared { .. })
        | None => Ok(()),
    }
}

/// Handle document-level methods directly (without editor tool dispatch).
/// `auth_principal` (key fingerprint / psk:<keyid>) is the authoritative subject
/// for KB access control (ADR-018); `auth_label` is display/attribution only.
#[allow(clippy::too_many_arguments)]
async fn handle_doc_request_inner(
    msg: &str,
    doc_store: &DocStore,
    quota: &dyn crate::quota::QuotaCharger,
    broadcaster: &SharedBroadcaster,
    start_time: std::time::Instant,
    session_id: u64,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
    auth_pubkey: Option<&[u8; 32]>,
    session_docs: &mut HashSet<String>,
    transport: Transport,
    artifact_store: &dyn crate::artifact_store::ArtifactStore,
    kb_query_limits: crate::kb_query::KbQueryLimits,
    self_issue: Option<crate::oauth_self_issue::SelfIssueConfig>,
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

    // ADR-060 Phase C (#456): per-tenant quota enforcement for the collab/mTLS
    // surface, keyed on the authenticated principal.
    //
    // Placed here — after parsing, before any handler runs — for the same reason
    // `handler.rs::charge_tenant_or_reject` sits inside `snapshot_query_layer`: a
    // rejected request must cost only a `dashmap` lookup and a few atomics, never
    // the doc-store work the handler was about to do.
    //
    // The guard is bound for the rest of this function so a tenant's connection
    // slot is held for the request's duration and released when it returns.
    //
    // `kb/query.*` is charged HERE and not inside `kb_query::dispatch`, even though
    // the OAuth listener reaches that function directly: charging inside it would
    // bill this path twice, once here and once there. The rule is that each
    // listener charges at its own entry.
    let _quota_lease =
        match crate::quota::charge_or_reject(quota, auth_principal, &request.method, id.clone()) {
            Ok(lease) => lease,
            Err(resp) => {
                warn!(
                    session = session_id,
                    method = %request.method,
                    principal = auth_principal.unwrap_or("<none>"),
                    "collab request rejected by tenant quota"
                );
                return resp;
            }
        };

    // ADR-105 D3: a KB id becomes part of every node document's address, so it
    // must be one the address can unambiguously carry. One chokepoint here rather
    // than in each `kb/*` handler — see `kb_id_guard` for why that matters.
    if let Some(resp) =
        kb_id_guard::refuse_unaddressable_kb_id(session_id, &request.method, &params, &id)
    {
        return resp;
    }

    info!(session = session_id, method = %request.method, "doc request");

    // @ai-caution: [dispatch-authz] The match below is over `Method`, not
    // `&str`, and it is exhaustive. That is what makes a new method impossible
    // to route until it is also classified in `method_authz::DocScope::of` --
    // the property the previous two rounds of this bug lacked. See
    // `method_authz`'s header for both of them.
    let Some(method) = method_authz::Method::parse(request.method.as_str()) else {
        return JsonRpcResponse::error(
            id,
            McpError::method_not_found(format!("Unknown method: {}", request.method)),
        );
    };

    // One chokepoint, keyed on the address TYPE (ADR-105 D1), for the methods
    // that name a caller-supplied document but cannot authorize a KB one.
    if let Some(resp) = method_authz::authorize_named_doc(session_id, method, &params, &id) {
        return resp;
    }

    use method_authz::Method;
    match method {
        Method::SyncStateVector => {
            sync_methods::handle_sync_state_vector(
                doc_store,
                session_id,
                auth_principal,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::SyncUpdate => {
            sync_methods::handle_sync_update(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                session_docs,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::SyncAwareness => {
            sync_methods::handle_sync_awareness(
                broadcaster,
                session_id,
                auth_label,
                session_docs,
                id,
                &params,
            )
            .await
        }

        Method::SyncFullState => {
            sync_methods::handle_sync_full_state(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                session_docs,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::SyncDiff => {
            sync_methods::handle_sync_diff(auth_principal, transport, doc_store, id, &params).await
        }

        Method::DocsList => docs_methods::handle_docs_list(doc_store, id).await,

        Method::DocsContent => docs_methods::handle_docs_content(doc_store, id, &params).await,

        Method::SyncResync => {
            sync_methods::handle_sync_resync(
                auth_principal,
                transport,
                doc_store,
                broadcaster,
                session_id,
                session_docs,
                id,
                &params,
            )
            .await
        }

        Method::DocsStats => docs_methods::handle_docs_stats(doc_store, id, &params).await,

        Method::DocsMetadata => {
            docs_methods::handle_docs_metadata(doc_store, broadcaster, id, &params).await
        }

        Method::DocsSaveIntent => {
            docs_methods::handle_docs_save_intent(doc_store, session_id, id, &params).await
        }

        Method::DocsSaveCommitted => {
            docs_methods::handle_docs_save_committed(
                doc_store,
                broadcaster,
                session_id,
                auth_label,
                id,
                &params,
            )
            .await
        }

        Method::SyncShare => {
            sync_methods::handle_sync_share(
                doc_store,
                broadcaster,
                session_id,
                session_docs,
                id,
                &params,
            )
            .await
        }

        Method::DocsDelete => {
            docs_methods::handle_docs_delete(doc_store, session_id, id, &params).await
        }

        Method::DebugStats => {
            docs_methods::handle_debug_stats(doc_store, broadcaster, start_time, id).await
        }

        Method::KbRegister => {
            kb_membership::handle_kb_register(
                doc_store,
                broadcaster,
                session_id,
                session_docs,
                id,
                &params,
            )
            .await
        }

        Method::KbList => {
            kb_membership::handle_kb_list(doc_store, auth_principal, transport, id).await
        }

        Method::KbUnregister => {
            kb_membership::handle_kb_unregister(doc_store, session_id, session_docs, id, &params)
                .await
        }

        Method::KbShare => {
            kb_content::handle_kb_share(
                doc_store,
                broadcaster,
                session_id,
                auth_label,
                auth_principal,
                session_docs,
                id,
                &params,
            )
            .await
        }

        Method::KbJoin => {
            kb_membership::handle_kb_join(
                doc_store,
                broadcaster,
                session_id,
                auth_label,
                auth_principal,
                auth_pubkey,
                session_docs,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::KbNodeFetch => {
            kb_content::handle_kb_node_fetch(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                session_docs,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::KbNodeUpdate => {
            kb_content::handle_kb_node_update(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                session_docs,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::KbCollectionOp => {
            kb_content::handle_kb_collection_op(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                id,
                &params,
            )
            .await
        }
        Method::KbClaimLease => {
            kb_lease::handle_kb_claim_lease(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                id,
                &params,
            )
            .await
        }
        Method::KbFetchArtifact => {
            kb_artifacts::handle_kb_fetch_artifact(
                doc_store,
                auth_principal,
                transport,
                artifact_store,
                id,
                &params,
            )
            .await
        }
        Method::KbAddMember | Method::KbRemoveMember => {
            kb_membership::handle_kb_add_remove_member(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                request.method.as_str(),
                id,
                &params,
            )
            .await
        }

        Method::KbCollectionNodeAdd | Method::KbCollectionNodeRemove => {
            kb_content::handle_kb_collection_node_add_remove(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                request.method.as_str(),
                id,
                &params,
            )
            .await
        }

        Method::KbSetPolicy => {
            kb_governance::handle_kb_set_policy(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::KbBlockPrincipal | Method::KbUnblockPrincipal => {
            kb_governance::handle_kb_block_unblock_principal(
                doc_store,
                session_id,
                auth_principal,
                request.method.as_str(),
                id,
                &params,
            )
            .await
        }

        Method::KbBlocklist => kb_governance::handle_kb_blocklist(doc_store, id, &params).await,

        Method::KbSetGovernance => {
            kb_governance::handle_kb_set_governance(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::KbRevoke => {
            kb_governance::handle_kb_revoke(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::KbListPending => {
            kb_membership::handle_kb_list_pending(doc_store, auth_principal, transport, id, &params)
                .await
        }

        Method::KbApproveMember => {
            kb_membership::handle_kb_approve_member(
                doc_store,
                broadcaster,
                session_id,
                auth_principal,
                transport,
                id,
                &params,
            )
            .await
        }

        Method::KbLeave => {
            kb_membership::handle_kb_leave(
                doc_store,
                broadcaster,
                session_id,
                session_docs,
                id,
                &params,
            )
            .await
        }

        // ADR-067 Phase D2: the live scoped read-through KB query surface
        // (ADR-053), previously reachable only over the OAuth HTTPS listener
        // (`daemon/src/oauth.rs`) — closes the gap where ADR-053's own
        // Decision-1 prose claimed mTLS reachability that was never actually
        // implemented. Reuses `crate::kb_query::dispatch` UNCHANGED — same
        // access gate (`check_kb_read_access`, Read-only), same encryption-
        // aware branching, same per-call caps. `auth_principal` is already
        // the exact `SHA256:...` fingerprint-or-`psk:<keyid>` shape
        // `kb_query::dispatch`'s `principal` parameter expects (see this
        // function's own doc comment) — zero translation needed.
        Method::KbQueryCapabilities
        | Method::KbQueryGet
        | Method::KbQuerySearch
        | Method::KbQueryGraph
        | Method::KbQueryMyWrappedKey => {
            match crate::kb_query::dispatch(
                &request.method,
                &params,
                doc_store,
                auth_principal,
                kb_query_limits,
            )
            .await
            {
                Ok(result) => JsonRpcResponse::success(id, result),
                Err(e) => JsonRpcResponse::error(id, e),
            }
        }

        // ADR-067 Phase D3: mint a self-issued OAuth bearer token for THIS
        // connection's own already-mTLS-verified principal -- what lets a
        // `QueryOnly`-restricted member (or any member) obtain OAuth
        // `kb/query.*` access without an external authorization server, by
        // pointing a `RemoteHubQueryLayer` (ADR-062) at this daemon's own
        // OAuth listener. Deliberately takes NO params: the token is never
        // KB-scoped (every `kb/query.*` call independently re-checks
        // `kb_access` using the token's `sub`, exactly like an externally-
        // issued token already does), so there is nothing here for a caller
        // to smuggle a different identity through -- `auth_principal` (this
        // TLS handshake's own verified fingerprint) is the ONLY source of
        // the minted `sub`, zero additional trust decision needed.
        Method::KbQuerySelfToken => {
            let Some(principal) = auth_principal else {
                return JsonRpcResponse::error(
                    id,
                    McpError::internal_error(
                        "kb/query.self_token requires an authenticated connection".to_string(),
                    ),
                );
            };
            let Some(si) = self_issue else {
                return JsonRpcResponse::error(
                    id,
                    McpError::internal_error(
                        "self-issued tokens are not enabled on this daemon \
                         (oauth.self_issued_tokens_enabled is false, or no \
                         key-mode identity is configured)"
                            .to_string(),
                    ),
                );
            };
            match crate::oauth_self_issue::mint_self_token(
                &si.identity,
                principal,
                &si.audience,
                si.ttl_secs,
            ) {
                Ok(token) => JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "token": token,
                        "token_type": "Bearer",
                        "expires_in": si.ttl_secs,
                        "audience": si.audience,
                    }),
                ),
                Err(e) => JsonRpcResponse::error(
                    id,
                    McpError::internal_error(format!("failed to mint self-issued token: {e}")),
                ),
            }
        }
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
                    let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                    bc.broadcast(&EditorEvent::SyncUpdate {
                        buffer_name: doc.clone(),
                        update_base64: update_to_base64(&result.update),
                        wal_seq: result.wal_seq,
                        content_header: None,
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
mod tests;
