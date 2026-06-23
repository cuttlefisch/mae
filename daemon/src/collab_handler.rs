//! Client connection handler for the collab server.
//!
//! Each TCP (or Unix) client gets its own tokio task running this handler.
//! Uses `mae_mcp::read_message` for framing and `mae_mcp::write_framed`
//! for responses. Protocol methods (initialize, ping, subscribe) are
//! delegated to `mae_mcp::handle_request`. Sync methods are handled locally
//! by dispatching to the DocStore.

use std::collections::HashSet;
use std::sync::Arc;

use mae_mcp::broadcast::{EditorEvent, SharedBroadcaster};
use mae_mcp::identity::PeerIdentity;
use mae_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpError, ToolInfo};
use mae_mcp::session::ClientSession;
use mae_mcp::{McpToolRequest, McpToolResult};
use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::kb::{
    derive_kb_client_id, update_new_op_authors, JoinPolicy, KbCollectionDoc, Role as SyncRole,
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
    let peer = match auth.server_handshake(&mut reader, &mut writer).await {
        Ok(result) => {
            info!(
                auth = auth.name(),
                client = %result.client_label,
                "auth handshake succeeded"
            );
            // The JSON handshake proves a credential but carries no public key,
            // so bind a synthetic identity from the authenticated label.
            PeerIdentity::synthetic(&result.client_label)
        }
        Err(e) => {
            warn!(auth = auth.name(), error = %e, "auth handshake failed, dropping connection");
            return;
        }
    };
    handle_client_authenticated(reader, writer, peer, doc_store, broadcaster, start_time).await;
}

/// Anonymous (no-auth) connection — used for the loopback/`none` mode.
pub async fn handle_client<R, W>(
    reader: R,
    writer: W,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
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
    )
    .await;
}

/// Authenticated connection — binds `peer` (from mTLS or the JSON handshake) to
/// the session so attribution + KB membership reflect the verified identity.
pub async fn handle_client_authenticated<R, W>(
    reader: R,
    writer: W,
    peer: PeerIdentity,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
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
async fn run_session<R, W>(
    mut session: ClientSession,
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

    let session_id = session.id;
    // The authoritative access-control **principal** (ADR-018): the key fingerprint
    // (or psk:<keyid>), never the mutable label. KB ownership/membership key on this.
    let auth_principal: Option<String> = session.authenticated_principal().map(str::to_string);
    // The display label (key/TLS sessions) — logging/attribution only.
    let auth_label: Option<String> = session.authenticated_label().map(str::to_string);
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
                    handle_doc_notification_inner(&msg, &doc_store, &broadcaster, session_id, auth_label.as_deref(), auth_principal.as_deref(), &mut session_docs).await;
                    continue;
                }

                let mut response = if is_doc {
                    handle_doc_request_inner(&msg, &doc_store, &broadcaster, start_time, session_id, auth_label.as_deref(), auth_principal.as_deref(), &mut session_docs).await
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
                let mut bc = broadcaster.lock().unwrap();
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
        "sync/update" | "sync/full_state" | "sync/state_vector" | "sync/share" | "sync/resync"
        | "kb/node_update" | "kb/node_fetch" | "kb/share" | "kb/join" | "kb/leave"
        | "kb/add_member" | "kb/remove_member" | "kb/approve_member" | "kb/set_policy" => {
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
        broadcaster,
        start_time,
        session_id,
        None,
        None,
        session_docs,
    )
    .await
}

/// A KB operation, for the access engine (ADR-018).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KbOp {
    Join,
    #[allow(dead_code)]
    Read,
    Edit,
    Manage,
}

/// The access decision (ADR-018). `AllowAutoJoin` = a permissive-policy non-member
/// the caller must add as a viewer; `Pending` = an invite-policy non-member to be
/// recorded for owner approval.
#[derive(Debug, PartialEq, Eq)]
enum AccessDecision {
    Allow,
    AllowAutoJoin,
    Pending,
    Deny(String),
}

/// Load the collection doc for `kb_id` (`kbc:{kb_id}`).
async fn load_collection(doc_store: &DocStore, kb_id: &str) -> Result<KbCollectionDoc, String> {
    let collection_doc = format!("kbc:{kb_id}");
    let (state, _sv) = doc_store
        .encode_state_and_sv(&collection_doc)
        .await
        .map_err(|e| format!("KB '{kb_id}' not found: {e}"))?;
    KbCollectionDoc::from_bytes(&state).map_err(|e| format!("bad collection: {e}"))
}

/// Persist a collection update + broadcast it to other subscribers. Returns wal_seq.
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
    broadcaster.lock().unwrap().broadcast_except(
        &EditorEvent::SyncUpdate {
            buffer_name: collection_doc,
            update_base64: update_to_base64(update),
            wal_seq: result.wal_seq,
        },
        session_id,
    );
    Ok(result.wal_seq)
}

/// A coarse monotonic-ish timestamp (unix seconds) for pending-request ordering.
fn now_stamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

/// ADR-018 complete-mediation access engine: every KB operation routes through
/// here. Resolves the caller's role from its cryptographic **principal** (key
/// fingerprint — never a label), then decides by hierarchical RBAC role × the
/// KB's join policy × the operation. `principal == None` (the `none`/loopback
/// auth mode) is connection-level-trusted and blanket-allowed (dev only — real
/// per-identity policy requires `key` mode).
async fn kb_access(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    op: KbOp,
) -> Result<AccessDecision, String> {
    let principal = match principal {
        Some(p) => p,
        None => return Ok(AccessDecision::Allow),
    };
    let coll = load_collection(doc_store, kb_id).await?;
    match coll.role_of(principal) {
        Some(role) => {
            // Hierarchical RBAC: owner ⊇ editor ⊇ viewer.
            let allowed = match op {
                KbOp::Join | KbOp::Read => true,
                KbOp::Edit => role.includes(SyncRole::Editor),
                KbOp::Manage => role.includes(SyncRole::Owner),
            };
            if allowed {
                Ok(AccessDecision::Allow)
            } else {
                Ok(AccessDecision::Deny(format!(
                    "role '{}' may not {:?} KB '{kb_id}'",
                    role.as_str(),
                    op
                )))
            }
        }
        None => match op {
            // Non-member join is governed by the KB's join policy.
            KbOp::Join => match coll.join_policy() {
                JoinPolicy::Permissive => Ok(AccessDecision::AllowAutoJoin),
                JoinPolicy::Invite => Ok(AccessDecision::Pending),
                JoinPolicy::Restrictive => Ok(AccessDecision::Deny(format!(
                    "not a member of KB '{kb_id}'"
                ))),
            },
            _ => Ok(AccessDecision::Deny(format!(
                "not a member of KB '{kb_id}'"
            ))),
        },
    }
}

/// Membership-smuggling defense (ADR-018): a raw `sync/update` to a collection
/// doc (`kbc:{kb}`) mutates owner/members/policy and is therefore owner-only. The
/// editor only ever touches collections via the gated `kb/*` methods, so a raw
/// `kbc:` write from a non-owner is rejected. Non-collection docs are unaffected.
async fn deny_collection_smuggling(
    doc_store: &DocStore,
    doc_name: &str,
    principal: Option<&str>,
) -> Result<(), String> {
    if let Some(kb_id) = doc_name.strip_prefix("kbc:") {
        match kb_access(doc_store, kb_id, principal, KbOp::Manage).await? {
            AccessDecision::Allow => Ok(()),
            _ => Err(format!(
                "only the owner may write the collection doc for KB '{kb_id}'"
            )),
        }
    } else {
        Ok(())
    }
}

/// Handle document-level methods directly (without editor tool dispatch).
/// `auth_principal` (key fingerprint / psk:<keyid>) is the authoritative subject
/// for KB access control (ADR-018); `auth_label` is display/attribution only.
#[allow(clippy::too_many_arguments)]
async fn handle_doc_request_inner(
    msg: &str,
    doc_store: &DocStore,
    broadcaster: &SharedBroadcaster,
    start_time: std::time::Instant,
    session_id: u64,
    auth_label: Option<&str>,
    auth_principal: Option<&str>,
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
            // ADR-018 membership-smuggling defense: a raw write to a collection doc
            // (`kbc:`) is owner-only.
            if let Err(e) = deny_collection_smuggling(doc_store, &doc_name, auth_principal).await {
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

        "kb/share" => {
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
            // ADR-018: the client-supplied `creator` is only a DISPLAY label hint —
            // never authoritative. The daemon derives the OWNER from the verified
            // cert principal (key fingerprint) and ignores any claimed identity, so
            // there is no "creator mismatch" failure (the old I-7 reject is gone).
            let creator_hint = params["creator"].as_str().unwrap_or("").to_string();
            let owner_label = auth_label.unwrap_or(creator_hint.as_str()).to_string();
            info!(
                session = session_id, kb_id = %kb_id, name = %name,
                owner = ?auth_principal, owner_label = %owner_label, "kb/share"
            );

            // Decode the collection doc.
            let collection_b64 = match params["collection_state"].as_str() {
                Some(s) => s,
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'collection_state' field".to_string()),
                    );
                }
            };
            let collection_bytes = match base64_to_update(collection_b64) {
                Ok(b) => b,
                Err(e) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error(format!("invalid collection_state base64: {e}")),
                    );
                }
            };
            // ADR-018 strict binding: bind the OWNER = the AUTHENTICATED principal
            // (key fingerprint), overriding whatever the client baked in. A
            // self-claimed creator is simply ignored — never rejected. `none` mode
            // (no principal) keeps the client collection as-is (loopback trust).
            let collection_bytes = match auth_principal {
                Some(principal) => match KbCollectionDoc::from_bytes(&collection_bytes) {
                    Ok(mut coll) => {
                        coll.set_owner(principal, &owner_label);
                        coll.encode_state()
                    }
                    Err(_) => collection_bytes,
                },
                None => collection_bytes,
            };
            let collection_doc = format!("kbc:{kb_id}");
            // ADR-020 B-12: an owner reconnect/re-share must NOT clobber the daemon's
            // authoritative collection. It holds the durable membership — approved
            // members, roles, pending requests (set via kb/approve_member /
            // kb/add_member) — that the owner's LOCAL collection copy does not carry,
            // plus the collection's CRDT lineage. `share_doc` is destructive
            // (delete+replace), so re-sharing the owner-only copy silently revoked
            // every trusted member. PRESERVE an existing collection; create it only
            // on the FIRST share. Membership changes flow through the dedicated
            // member/approve/policy methods, never through re-share.
            if !doc_store.has_doc(&collection_doc).await {
                if let Err(e) = doc_store
                    .share_doc(&collection_doc, &collection_bytes)
                    .await
                {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("failed to share collection doc: {e}")),
                    );
                }
            } else {
                info!(
                    session = session_id,
                    kb_id = %kb_id,
                    "kb/share: collection exists — preserving daemon-side membership (B-12)"
                );
            }
            session_docs.insert(collection_doc.clone());
            // ADR-020 liveness: count the sharer as a connected client so the
            // collection doc isn't idle-evicted while the owner stays connected.
            let _ = doc_store.track_client_connect(&collection_doc).await;
            broadcaster
                .lock()
                .unwrap()
                .subscribe_doc(session_id, &collection_doc);

            // Store each node doc.
            let nodes = params["nodes"].as_array();
            let mut node_count: u64 = 0;
            if let Some(node_arr) = nodes {
                for node in node_arr {
                    let node_id = match node["id"].as_str() {
                        Some(s) => s,
                        None => continue,
                    };
                    let state_b64 = match node["state"].as_str() {
                        Some(s) => s,
                        None => continue,
                    };
                    let state_bytes = match base64_to_update(state_b64) {
                        Ok(b) => b,
                        Err(e) => {
                            warn!(session = session_id, node_id, error = %e, "kb/share: skipping node with invalid base64");
                            continue;
                        }
                    };
                    let node_doc = format!("kb:{node_id}");
                    // ADR-020 B-12: MERGE onto an existing node instead of
                    // delete+replace, so an owner re-share does not reset the node's
                    // CRDT lineage or clobber peer edits the owner hasn't seen yet.
                    // Post-B-16 the owner's lineage is stable + persisted, so this is
                    // a clean idempotent merge. Create fresh only on the first share.
                    let node_res = if doc_store.has_doc(&node_doc).await {
                        doc_store
                            .apply_update(&node_doc, &state_bytes, None)
                            .await
                            .map(|_| ())
                    } else {
                        doc_store
                            .share_doc(&node_doc, &state_bytes)
                            .await
                            .map(|_| ())
                    };
                    if let Err(e) = node_res {
                        warn!(session = session_id, node_id, error = %e, "kb/share: failed to share node doc");
                        continue;
                    }
                    session_docs.insert(node_doc.clone());
                    let _ = doc_store.track_client_connect(&node_doc).await;
                    broadcaster
                        .lock()
                        .unwrap()
                        .subscribe_doc(session_id, &node_doc);
                    node_count += 1;
                }
            }

            // Store KB metadata.
            let meta = serde_json::json!({
                "kb_id": kb_id,
                "name": name,
                "owner": auth_principal.unwrap_or(""),
                "owner_label": owner_label,
                "node_count": node_count,
                "shared_by": session_id,
            });
            doc_store.set_kb_meta(&kb_id, meta).await;

            info!(session = session_id, kb_id = %kb_id, node_count, "kb/share: complete");
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "kb_id": kb_id,
                    "shared": true,
                    "node_count": node_count,
                }),
            )
        }

        "kb/join" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    );
                }
            };
            info!(session = session_id, kb_id = %kb_id, "kb/join");

            // ADR-018 access gate (complete mediation): member → join; non-member
            // resolved by the KB's join policy (restrictive/invite/permissive).
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Join).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::AllowAutoJoin) => {
                    // permissive: auto-grant the least-privilege role (viewer).
                    if let (Some(principal), Ok(mut coll)) =
                        (auth_principal, load_collection(doc_store, &kb_id).await)
                    {
                        let update = coll.upsert_member(
                            principal,
                            auth_label.unwrap_or(principal),
                            SyncRole::Viewer,
                        );
                        let _ = persist_and_broadcast_collection(
                            doc_store,
                            broadcaster,
                            session_id,
                            &kb_id,
                            &update,
                        )
                        .await;
                    }
                }
                Ok(AccessDecision::Pending) => {
                    if let (Some(principal), Ok(mut coll)) =
                        (auth_principal, load_collection(doc_store, &kb_id).await)
                    {
                        let update = coll.add_pending(
                            principal,
                            auth_label.unwrap_or(principal),
                            &now_stamp(),
                        );
                        let _ = persist_and_broadcast_collection(
                            doc_store,
                            broadcaster,
                            session_id,
                            &kb_id,
                            &update,
                        )
                        .await;
                    }
                    info!(session = session_id, kb_id = %kb_id, principal = ?auth_principal, "kb/join: pending");
                    return JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "kb_id": kb_id, "status": "pending" }),
                    );
                }
                Ok(AccessDecision::Deny(msg)) | Err(msg) => {
                    warn!(session = session_id, kb_id = %kb_id, reason = %msg, "kb/join denied");
                    return JsonRpcResponse::error(id, McpError::internal_error(msg));
                }
            }

            // Read the collection doc.
            let collection_doc = format!("kbc:{kb_id}");
            let (collection_state, _sv) = match doc_store.encode_state_and_sv(&collection_doc).await
            {
                Ok(pair) => pair,
                Err(e) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!(
                            "kb not found or failed to read collection: {e}"
                        )),
                    );
                }
            };
            session_docs.insert(collection_doc.clone());
            // ADR-020 liveness: the joining member is a connected client of the docs.
            let _ = doc_store.track_client_connect(&collection_doc).await;
            broadcaster
                .lock()
                .unwrap()
                .subscribe_doc(session_id, &collection_doc);

            // Parse collection to get the list of node IDs belonging to this KB.
            let node_ids: Vec<String> = match KbCollectionDoc::from_bytes(&collection_state) {
                Ok(coll) => coll
                    .list_nodes()
                    .into_iter()
                    .map(|(id, _title)| id)
                    .collect(),
                Err(e) => {
                    warn!(session = session_id, kb_id = %kb_id, error = %e,
                        "kb/join: failed to parse collection doc, falling back to empty node list");
                    Vec::new()
                }
            };

            // ADR-022: parse the member's per-node state vectors (optional). When
            // present, reply with an incremental DIFF per node so the member can
            // reconcile (merge, no clobber) instead of adopting a full snapshot.
            // Absent (old editor, or a first-ever join) → full state, as before.
            let member_svs: std::collections::HashMap<String, Vec<u8>> = params["node_svs"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| {
                            let id = e["id"].as_str()?;
                            let sv = base64_to_update(e["sv"].as_str()?).ok()?;
                            Some((id.to_string(), sv))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let reconcile_mode = !member_svs.is_empty();

            // Fetch only the nodes listed in the collection (not all kb: docs).
            let mut nodes = Vec::new();
            let mut diff_count = 0usize;
            for node_id in &node_ids {
                let doc_name = format!("kb:{node_id}");
                // Member sent an SV for this node → send only the ops it lacks.
                let encoded = match member_svs.get(node_id) {
                    Some(member_sv) => doc_store
                        .encode_diff_and_sv(&doc_name, member_sv)
                        .await
                        .map(|(diff, sv)| (Some(diff), None, sv)),
                    None => doc_store
                        .encode_state_and_sv(&doc_name)
                        .await
                        .map(|(state, sv)| (None, Some(state), sv)),
                };
                match encoded {
                    Ok((diff, state, sv)) => {
                        session_docs.insert(doc_name.clone());
                        let _ = doc_store.track_client_connect(&doc_name).await;
                        broadcaster
                            .lock()
                            .unwrap()
                            .subscribe_doc(session_id, &doc_name);
                        // Always carry the daemon's SV so the member can compute its
                        // local-ahead diff; carry `diff` (incremental) or `state`
                        // (full) depending on whether the member sent an SV.
                        let mut obj = serde_json::Map::new();
                        obj.insert("id".into(), serde_json::json!(node_id));
                        obj.insert("sv".into(), serde_json::json!(update_to_base64(&sv)));
                        if let Some(diff) = diff {
                            diff_count += 1;
                            obj.insert("diff".into(), serde_json::json!(update_to_base64(&diff)));
                        }
                        if let Some(state) = state {
                            obj.insert("state".into(), serde_json::json!(update_to_base64(&state)));
                        }
                        nodes.push(serde_json::Value::Object(obj));
                    }
                    Err(e) => {
                        warn!(session = session_id, doc = %doc_name, error = %e, "kb/join: failed to read node doc");
                    }
                }
            }

            info!(
                session = session_id, kb_id = %kb_id, node_count = nodes.len(),
                reconcile_mode, diff_count,
                "kb/join: complete"
            );
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "kb_id": kb_id,
                    "collection_state": update_to_base64(&collection_state),
                    "nodes": nodes,
                }),
            )
        }

        // ADR-024 R1: fetch a single node's authoritative state so a fenced member
        // can ADOPT it (drop its stale-epoch divergence, ADR-023) + re-author. The
        // editor's local doc still carries the rejected op, so it cannot self-adopt.
        "kb/node_fetch" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    );
                }
            };
            let node_id = match params["node_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'node_id' field".to_string()),
                    );
                }
            };
            info!(session = session_id, kb_id = %kb_id, node_id = %node_id, "kb/node_fetch");

            // Read access suffices (members only — a non-member is denied).
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Read).await {
                Ok(AccessDecision::Allow) | Ok(AccessDecision::AllowAutoJoin) => {}
                Ok(AccessDecision::Deny(msg)) | Err(msg) => {
                    warn!(session = session_id, kb_id = %kb_id, reason = %msg, "kb/node_fetch denied");
                    return JsonRpcResponse::error(id, McpError::internal_error(msg));
                }
                Ok(_) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("cannot read KB '{kb_id}'")),
                    );
                }
            }

            let node_doc = format!("kb:{node_id}");
            match doc_store.encode_state_and_sv(&node_doc).await {
                Ok((state, sv)) => {
                    // Keep the member live-subscribed to this node going forward.
                    if session_docs.insert(node_doc.clone()) {
                        let _ = doc_store.track_client_connect(&node_doc).await;
                        broadcaster
                            .lock()
                            .unwrap()
                            .subscribe_doc(session_id, &node_doc);
                    }
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({
                            "kb_id": kb_id,
                            "node_id": node_id,
                            "state": update_to_base64(&state),
                            "sv": update_to_base64(&sv),
                        }),
                    )
                }
                Err(e) => JsonRpcResponse::error(
                    id,
                    McpError::internal_error(format!("node fetch failed for '{node_id}': {e}")),
                ),
            }
        }

        "kb/node_update" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    );
                }
            };
            let node_id = match params["node_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'node_id' field".to_string()),
                    );
                }
            };
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
            // ADR-020 traceability: log on ENTRY so a received kb/node_update is
            // greppable on the daemon (distinguishes "never arrived" from "rejected").
            info!(
                session = session_id,
                kb_id = %kb_id,
                node_id = %node_id,
                update_len = update_bytes.len(),
                "kb/node_update: received"
            );
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
            info!(session = session_id, kb_id = %kb_id, node_id = %node_id, update_len = update_bytes.len(), "kb/node_update");

            // ADR-018: editing a node requires editor/owner role. Viewers and
            // non-members are denied (least privilege).
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Edit).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::Deny(msg)) | Err(msg) => {
                    warn!(session = session_id, kb_id = %kb_id, reason = %msg, "kb/node_update denied");
                    return JsonRpcResponse::error(id, McpError::internal_error(msg));
                }
                Ok(_) => {
                    warn!(session = session_id, kb_id = %kb_id, "kb/node_update denied");
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("cannot edit KB '{kb_id}'")),
                    );
                }
            }

            let node_doc = format!("kb:{node_id}");

            // ADR-023 (B-19) EPOCH FENCE — the security core. `kb_access` above
            // confirmed the sender's *current* role permits editing, but a granted
            // member must author under their *current-epoch* client_id. Decode the
            // update's NEW ops (those beyond the daemon's authoritative node SV) and
            // reject any authored under a stale-epoch client_id. That is precisely a
            // member's pre-grant divergent lineage (e.g. viewer-era edits) trying to
            // cascade after a grant — denied here, at the daemon, independent of any
            // (possibly malicious) client behaviour. Only enforced in key-auth mode:
            // `none`/loopback has no per-identity principal to derive an epoch from.
            if let Some(principal) = auth_principal {
                let epoch_now = match load_collection(doc_store, &kb_id).await {
                    Ok(coll) => coll.epoch_of(principal),
                    Err(e) => {
                        warn!(session = session_id, kb_id = %kb_id, reason = %e, "kb/node_update: epoch lookup failed");
                        return JsonRpcResponse::error(
                            id,
                            McpError::internal_error(format!(
                                "epoch lookup failed for KB '{kb_id}': {e}"
                            )),
                        );
                    }
                };
                let c_now = derive_kb_client_id(principal, epoch_now);
                // The daemon's authoritative STATE for this node — ops it already
                // holds are grandfathered; only ops *beyond* it are fenced. We need
                // the full state (not just the SV) so the fence can detect a
                // contiguous-clock continuation of an already-canonical client (B-20),
                // which the incoming update's own SV would hide.
                let base_state = match doc_store.encode_state_and_sv(&node_doc).await {
                    Ok((state, _sv)) => state,
                    Err(e) => {
                        return JsonRpcResponse::error(
                            id,
                            McpError::internal_error(format!(
                                "node state lookup failed for '{node_id}': {e}"
                            )),
                        );
                    }
                };
                match update_new_op_authors(&update_bytes, &base_state) {
                    Ok(authors) => {
                        if let Some(stale) = authors.iter().find(|a| **a != c_now) {
                            warn!(
                                session = session_id, kb_id = %kb_id, node_id = %node_id,
                                stale_client = stale, current_client = c_now, epoch = epoch_now,
                                "kb/node_update: REBASE REQUIRED (stale-epoch op fenced — B-19)"
                            );
                            return JsonRpcResponse::error(
                                id,
                                McpError::internal_error(format!(
                                    "rebase required: node '{node_id}' carries an op from \
                                     stale-epoch client {stale} (current-epoch author is \
                                     {c_now}, epoch {epoch_now}); adopt authoritative state \
                                     and re-author the edit"
                                )),
                            );
                        }
                    }
                    Err(e) => {
                        // An update we cannot decode cannot be fenced — fail closed.
                        warn!(session = session_id, kb_id = %kb_id, node_id = %node_id, reason = %e, "kb/node_update: undecodable update rejected");
                        return JsonRpcResponse::error(
                            id,
                            McpError::parse_error(format!("could not decode update: {e}")),
                        );
                    }
                }
            }

            // ADR-020 liveness: an editing session keeps its node doc alive (count
            // it as a connected client on first touch) so the doc the member is
            // actively editing isn't idle-evicted out from under them.
            if session_docs.insert(node_doc.clone()) {
                let _ = doc_store.track_client_connect(&node_doc).await;
                broadcaster
                    .lock()
                    .unwrap()
                    .subscribe_doc(session_id, &node_doc);
            }
            match doc_store.apply_update(&node_doc, &update_bytes, None).await {
                Ok(result) => {
                    // Broadcast to other subscribers of the collection.
                    {
                        let mut bc = broadcaster.lock().unwrap();
                        bc.broadcast_except(
                            &EditorEvent::SyncUpdate {
                                buffer_name: node_doc.clone(),
                                update_base64: update_to_base64(&result.update),
                                wal_seq: result.wal_seq,
                            },
                            session_id,
                        );
                    }
                    info!(session = session_id, kb_id = %kb_id, node_id = %node_id, wal_seq = result.wal_seq, "kb/node_update: applied");
                    JsonRpcResponse::success(id, serde_json::json!({ "applied": true }))
                }
                Err(e) => JsonRpcResponse::error(
                    id,
                    McpError::internal_error(format!("failed to apply node update: {e}")),
                ),
            }
        }

        "kb/add_member" | "kb/remove_member" => {
            let add = request.method == "kb/add_member";
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    );
                }
            };
            // `member` is now a PRINCIPAL (key fingerprint), not a label.
            let member = match params["member"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'member' field".to_string()),
                    );
                }
            };
            let role = params["role"]
                .as_str()
                .and_then(SyncRole::parse)
                .unwrap_or(SyncRole::Editor);
            // ADR-018: owner-only (Manage). The verified principal is the subject.
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::Deny(m)) | Err(m) => {
                    return JsonRpcResponse::error(id, McpError::internal_error(m));
                }
                Ok(_) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
                    );
                }
            }
            let mut coll = match load_collection(doc_store, &kb_id).await {
                Ok(c) => c,
                Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
            };
            if !add && coll.owner() == member {
                return JsonRpcResponse::error(
                    id,
                    McpError::internal_error(format!("cannot remove the owner of KB '{kb_id}'")),
                );
            }
            let update = if add {
                coll.upsert_member(&member, params["label"].as_str().unwrap_or(""), role)
            } else {
                coll.remove_principal(&member)
            };
            match persist_and_broadcast_collection(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &update,
            )
            .await
            {
                Ok(_) => {
                    info!(session = session_id, kb_id = %kb_id, member = %member, add, role = role.as_str(), "kb membership change");
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "kb_id": kb_id, "member": member, "added": add }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
            }
        }

        "kb/set_policy" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    )
                }
            };
            let policy = match params["policy"].as_str().and_then(JoinPolicy::parse) {
                Some(p) => p,
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error(
                            "policy must be restrictive|invite|permissive".to_string(),
                        ),
                    )
                }
            };
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::Deny(m)) | Err(m) => {
                    return JsonRpcResponse::error(id, McpError::internal_error(m))
                }
                Ok(_) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
                    )
                }
            }
            let mut coll = match load_collection(doc_store, &kb_id).await {
                Ok(c) => c,
                Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
            };
            let update = coll.set_join_policy(policy);
            match persist_and_broadcast_collection(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &update,
            )
            .await
            {
                Ok(_) => {
                    info!(session = session_id, kb_id = %kb_id, policy = policy.as_str(), "kb/set_policy: complete");
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "kb_id": kb_id, "policy": policy.as_str() }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
            }
        }

        "kb/list_pending" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    )
                }
            };
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::Deny(m)) | Err(m) => {
                    return JsonRpcResponse::error(id, McpError::internal_error(m))
                }
                Ok(_) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
                    )
                }
            }
            let coll = match load_collection(doc_store, &kb_id).await {
                Ok(c) => c,
                Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
            };
            let pending: Vec<_> = coll
                .pending()
                .into_iter()
                .map(|p| {
                    serde_json::json!({
                        "fingerprint": p.fingerprint,
                        "label": p.label,
                        "requested_at": p.requested_at,
                    })
                })
                .collect();
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "pending": pending }),
            )
        }

        "kb/approve_member" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    )
                }
            };
            let principal = match params["principal"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'principal' field".to_string()),
                    )
                }
            };
            let role = params["role"]
                .as_str()
                .and_then(SyncRole::parse)
                .unwrap_or(SyncRole::Editor);
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::Deny(m)) | Err(m) => {
                    return JsonRpcResponse::error(id, McpError::internal_error(m))
                }
                Ok(_) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("not authorized to manage KB '{kb_id}'")),
                    )
                }
            }
            let mut coll = match load_collection(doc_store, &kb_id).await {
                Ok(c) => c,
                Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
            };
            let update = coll.approve(&principal, role);
            match persist_and_broadcast_collection(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &update,
            )
            .await
            {
                Ok(_) => {
                    info!(session = session_id, kb_id = %kb_id, principal = %principal, role = role.as_str(), "kb/approve_member: complete");
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "kb_id": kb_id, "principal": principal, "role": role.as_str() }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
            }
        }

        "kb/leave" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    );
                }
            };
            info!(session = session_id, kb_id = %kb_id, "kb/leave");

            // Read the collection doc to find which nodes belong to this KB.
            let collection_doc = format!("kbc:{kb_id}");
            let node_ids: Vec<String> = match doc_store.encode_state_and_sv(&collection_doc).await {
                Ok((state, _sv)) => match KbCollectionDoc::from_bytes(&state) {
                    Ok(coll) => coll.list_nodes().into_iter().map(|(id, _)| id).collect(),
                    Err(_) => Vec::new(),
                },
                Err(_) => Vec::new(),
            };

            // Unsubscribe from collection doc.
            session_docs.remove(&collection_doc);
            broadcaster
                .lock()
                .unwrap()
                .unsubscribe_doc(session_id, &collection_doc);

            // Unsubscribe only from this KB's node docs.
            let mut removed_count: u64 = 0;
            for node_id in &node_ids {
                let doc_name = format!("kb:{node_id}");
                if session_docs.remove(&doc_name) {
                    broadcaster
                        .lock()
                        .unwrap()
                        .unsubscribe_doc(session_id, &doc_name);
                    removed_count += 1;
                }
            }

            info!(session = session_id, kb_id = %kb_id, removed_count, "kb/leave: complete");
            JsonRpcResponse::success(id, serde_json::json!({ "kb_id": kb_id, "left": true }))
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

    /// ADR-020 B-12: an owner reconnect/re-share must PRESERVE the daemon's
    /// authoritative collection membership, not clobber it. `share_doc` was
    /// destructive (delete+replace), so re-sharing the owner-only collection
    /// silently revoked every approved member on each owner restart — unacceptable
    /// for a trusted-peer system. The fix preserves an existing collection.
    #[tokio::test]
    async fn kb_share_preserves_membership_on_owner_reshare() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut sd = HashSet::new();

        // First share: a collection that already carries an approved member.
        let mut coll = KbCollectionDoc::new("testkb", "alice");
        coll.add_node("testkb:n1", "T");
        coll.upsert_member("SHA256:bob", "bob", SyncRole::Editor);
        let share1 = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": "testkb", "name": "testkb",
                "collection_state": update_to_base64(&coll.encode_state()),
                "nodes": []
            }
        });
        handle_doc_request(
            &share1.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut sd,
        )
        .await;
        let c1 = load_collection(&store, "testkb").await.unwrap();
        assert!(
            c1.role_of("SHA256:bob").is_some(),
            "bob is a member after the first share"
        );

        // Owner RE-SHARES an owner-only collection (no members) — the clobber input.
        let owner_only = KbCollectionDoc::new("testkb", "alice");
        let share2 = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "kb/share",
            "params": {
                "kb_id": "testkb", "name": "testkb",
                "collection_state": update_to_base64(&owner_only.encode_state()),
                "nodes": []
            }
        });
        handle_doc_request(
            &share2.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            1,
            &mut HashSet::new(),
        )
        .await;

        // B-12: bob's membership must SURVIVE the re-share.
        let c2 = load_collection(&store, "testkb").await.unwrap();
        assert!(
            c2.role_of("SHA256:bob").is_some(),
            "B-12: owner re-share must preserve approved members, not silently revoke them"
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

    // --- KB protocol handler tests (Phase 0.5) ---

    /// Helper: create a KbNodeDoc with realistic org content and return encoded bytes.
    fn make_test_node(id: &str, title: &str, body: &str, tags: &[&str]) -> Vec<u8> {
        use mae_sync::kb::KbNodeDoc;
        let node = KbNodeDoc::new(
            id,
            title,
            body,
            &tags.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        );
        node.encode()
    }

    /// Realistic org content for testing (properties drawer, links, code block, Unicode).
    fn realistic_org_body() -> &'static str {
        ":PROPERTIES:\n:ID: test-node-001\n:ROAM_REFS: https://example.com\n:END:\n\
         #+TITLE: Test Node — CRDT Round-Trip\n#+FILETAGS: :research:crdt:\n\n\
         * Overview\n\
         This node tests the full round-trip: SQLite → KbNodeDoc → base64 → server → base64 → KbNodeDoc → SQLite.\n\n\
         ** Sub-heading with [[id:other-node][internal link]]\n\
         Content with Unicode: café, naïve, 日本語\n\n\
         #+begin_src rust\nfn main() { println!(\"hello\"); }\n#+end_src\n"
    }

    // --- ADR-018 access-control test harness (principals, not labels) ---

    /// A peer's principal (fake key fingerprint) from a label.
    fn fp(label: &str) -> String {
        format!("SHA256:{label}")
    }

    /// Share a KB authenticated as `auth_principal` (key fingerprint) with display
    /// `auth_label`. The daemon stamps the owner from the principal; any claimed
    /// `creator` is ignored.
    async fn kb_share_as(
        store: &Arc<DocStore>,
        bc: &SharedBroadcaster,
        auth_label: Option<&str>,
        auth_principal: Option<&str>,
        kb_id: &str,
        claimed_creator: &str,
        session_docs: &mut HashSet<String>,
    ) -> JsonRpcResponse {
        let coll = KbCollectionDoc::new_owned(kb_id, "", auth_label.unwrap_or(""));
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": kb_id,
                "name": kb_id,
                "creator": claimed_creator,
                "collection_state": update_to_base64(&coll.encode_state()),
                "nodes": [],
            }
        });
        handle_doc_request_inner(
            &msg.to_string(),
            store,
            bc,
            std::time::Instant::now(),
            0,
            auth_label,
            auth_principal,
            session_docs,
        )
        .await
    }

    /// Dispatch an arbitrary doc request as a peer (label + principal).
    async fn dispatch_as(
        store: &Arc<DocStore>,
        bc: &SharedBroadcaster,
        auth_label: Option<&str>,
        auth_principal: Option<&str>,
        msg: serde_json::Value,
        docs: &mut HashSet<String>,
    ) -> JsonRpcResponse {
        handle_doc_request_inner(
            &msg.to_string(),
            store,
            bc,
            std::time::Instant::now(),
            0,
            auth_label,
            auth_principal,
            docs,
        )
        .await
    }

    async fn load_coll(store: &Arc<DocStore>, kb_id: &str) -> KbCollectionDoc {
        let (state, _) = store
            .encode_state_and_sv(&format!("kbc:{kb_id}"))
            .await
            .expect("collection exists");
        KbCollectionDoc::from_bytes(&state).expect("valid collection")
    }

    fn kb_join_msg(kb_id: &str) -> serde_json::Value {
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/join","params":{"kb_id":kb_id}})
    }
    fn kb_node_update_msg(kb_id: &str) -> serde_json::Value {
        let mut ts = TextSync::with_client_id("", 7);
        let upd = ts.insert(0, "x");
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":"concept:n","update":update_to_base64(&upd)}})
    }

    /// ADR-023: a node edit authored under the sender's CURRENT-epoch KB client_id
    /// `derive_kb_client_id(principal, epoch)` — what the editor's `kb_client_id_for`
    /// produces and what the daemon's epoch fence accepts. `text` lets a test vary
    /// the op so a re-authored edit is distinguishable from a stale one.
    fn kb_node_update_msg_as(
        kb_id: &str,
        principal: &str,
        epoch: u64,
        text: &str,
    ) -> serde_json::Value {
        let cid = derive_kb_client_id(principal, epoch);
        let mut ts = TextSync::with_client_id("", cid);
        let upd = ts.insert(0, text);
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":kb_id,"node_id":"concept:n","update":update_to_base64(&upd)}})
    }
    /// member is a PRINCIPAL (fingerprint); optional role.
    fn kb_member_msg(
        method: &str,
        kb_id: &str,
        member: &str,
        role: Option<&str>,
    ) -> serde_json::Value {
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":method,
            "params":{"kb_id":kb_id,"member":member,"role":role,"label":member}})
    }
    fn kb_policy_msg(kb_id: &str, policy: &str) -> serde_json::Value {
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/set_policy",
            "params":{"kb_id":kb_id,"policy":policy}})
    }
    fn kb_approve_msg(kb_id: &str, principal: &str, role: Option<&str>) -> serde_json::Value {
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/approve_member",
            "params":{"kb_id":kb_id,"principal":principal,"role":role}})
    }

    #[tokio::test]
    async fn share_ignores_claimed_creator_and_binds_owner_to_principal() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        // Authenticated principal = alice's key; claims creator "mallory" → SUCCEEDS,
        // owner bound to the principal (the I-7 reject is gone; the claim is ignored).
        let resp = kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kb1",
            "mallory",
            &mut docs,
        )
        .await;
        assert!(
            resp.error.is_none(),
            "claimed creator must be ignored, not rejected: {:?}",
            resp.error
        );
        let coll = load_coll(&store, "kb1").await;
        assert_eq!(coll.owner(), fp("alice"), "owner = verified principal");
        assert_eq!(coll.role_of(&fp("alice")), Some(SyncRole::Owner));
        assert_eq!(
            coll.role_of(&fp("mallory")),
            None,
            "spoofed name is not a member"
        );
    }

    #[tokio::test]
    async fn anonymous_share_succeeds() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        let resp = kb_share_as(&store, &bc, None, None, "kb3", "whoever", &mut docs).await;
        assert!(
            resp.error.is_none(),
            "anonymous (none) share must succeed: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn restrictive_nonmember_join_denied() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbr",
            "alice",
            &mut docs,
        )
        .await;
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_policy_msg("kbr", "restrictive"),
            &mut docs,
        )
        .await;
        let denied = dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kbr"),
            &mut docs,
        )
        .await;
        assert!(
            denied.error.is_some(),
            "restrictive: non-member join denied"
        );
        assert!(denied.error.unwrap().message.contains("not a member"));
    }

    #[tokio::test]
    async fn invite_nonmember_join_pending() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        // default policy = invite
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbi",
            "alice",
            &mut docs,
        )
        .await;
        let resp = dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kbi"),
            &mut docs,
        )
        .await;
        assert!(
            resp.error.is_none(),
            "invite join returns success+pending, not error"
        );
        assert_eq!(
            resp.result.as_ref().and_then(|r| r["status"].as_str()),
            Some("pending")
        );
        let coll = load_coll(&store, "kbi").await;
        assert_eq!(coll.pending().len(), 1, "join recorded as pending");
        assert_eq!(
            coll.role_of(&fp("bob")),
            None,
            "pending peer is not yet a member"
        );
    }

    #[tokio::test]
    async fn permissive_nonmember_join_autoadds_viewer() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbp",
            "alice",
            &mut docs,
        )
        .await;
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_policy_msg("kbp", "permissive"),
            &mut docs,
        )
        .await;
        let resp = dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kbp"),
            &mut docs,
        )
        .await;
        assert!(resp.error.is_none(), "permissive join succeeds");
        let coll = load_coll(&store, "kbp").await;
        assert_eq!(
            coll.role_of(&fp("bob")),
            Some(SyncRole::Viewer),
            "auto-granted least privilege"
        );
    }

    #[tokio::test]
    async fn owner_add_member_then_join_and_edit() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbm2",
            "alice",
            &mut docs,
        )
        .await;
        // bob denied edit before being added.
        assert!(dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg("kbm2"),
            &mut docs
        )
        .await
        .error
        .is_some());
        // owner adds bob (default editor).
        assert!(dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbm2", &fp("bob"), None),
            &mut docs
        )
        .await
        .error
        .is_none());
        // bob now joins (member) + edits.
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                kb_join_msg("kbm2"),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "member joins directly"
        );
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                // bob is a freshly-added editor ⇒ epoch 0; he authors under his
                // current-epoch client_id, which the ADR-023 fence accepts.
                kb_node_update_msg_as("kbm2", &fp("bob"), 0, "x"),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "editor may edit"
        );
        // owner removes bob → next edit denied.
        assert!(dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/remove_member", "kbm2", &fp("bob"), None),
            &mut docs
        )
        .await
        .error
        .is_none());
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                kb_node_update_msg("kbm2"),
                &mut docs
            )
            .await
            .error
            .is_some(),
            "removed member denied"
        );
    }

    #[tokio::test]
    async fn viewer_cannot_node_update() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbv",
            "alice",
            &mut docs,
        )
        .await;
        // add bob as VIEWER
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbv", &fp("bob"), Some("viewer")),
            &mut docs,
        )
        .await;
        // viewer may join/read but not edit.
        assert!(dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kbv"),
            &mut docs
        )
        .await
        .error
        .is_none());
        let denied = dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_node_update_msg("kbv"),
            &mut docs,
        )
        .await;
        assert!(
            denied.error.is_some(),
            "viewer must not edit (least privilege)"
        );
    }

    /// ADR-023 (B-19) — the deferred-privilege-escalation exploit, end to end at the
    /// daemon. A viewer authors edits locally under their viewer-epoch client_id while
    /// DENIED at the daemon; once later granted editor, those pre-grant edits must NOT
    /// cascade. The epoch fence rejects the stale lineage (`"rebase required"`); only a
    /// fresh, current-epoch edit is accepted. (Red before the fence, green after.)
    #[tokio::test]
    async fn viewer_era_edits_do_not_cascade_on_grant() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbx",
            "alice",
            &mut docs,
        )
        .await;

        // bob is added as a VIEWER — a fresh grant ⇒ epoch 0.
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("viewer")),
            &mut docs,
        )
        .await;

        // bob (viewer) authors an edit under his epoch-0 client_id and pushes it —
        // DENIED at the role gate. He keeps it in his local lineage (the cascade seed).
        let viewer_era = kb_node_update_msg_as("kbx", &fp("bob"), 0, "VIEWER-ERA");
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                viewer_era.clone(),
                &mut docs
            )
            .await
            .error
            .is_some(),
            "viewer edit denied at the role gate"
        );

        // Owner PROMOTES bob viewer→editor — a role change ⇒ bob's epoch bumps to 1.
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("alice"),
                Some(&fp("alice")),
                kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("editor")),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "owner promotes bob to editor"
        );

        // THE EXPLOIT: bob re-pushes his VIEWER-ERA op (still authored under epoch 0).
        // The role gate now passes (he is an editor), but the EPOCH FENCE must reject
        // it — the op is from his stale, pre-grant client_id.
        let resp = dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            viewer_era.clone(),
            &mut docs,
        )
        .await;
        let msg = resp
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        assert!(
            resp.error.is_some() && msg.contains("rebase required"),
            "viewer-era lineage must be fenced on grant; got: {msg:?}"
        );

        // NO CASCADE: the node's canonical content never contains the viewer-era edit.
        let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
        let canonical = TextSync::from_state(&state).unwrap().content();
        assert!(
            !canonical.contains("VIEWER-ERA"),
            "pre-grant edit must not cascade; canonical = {canonical:?}"
        );

        // bob CAN make a fresh, current-epoch (epoch 1) edit — that is accepted.
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                kb_node_update_msg_as("kbx", &fp("bob"), 1, "FRESH"),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "a fresh current-epoch edit is accepted"
        );
        let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
        assert!(
            TextSync::from_state(&state)
                .unwrap()
                .content()
                .contains("FRESH"),
            "fresh current-epoch edit is applied"
        );

        // MALICIOUS-CLIENT VARIANT: re-sending the divergent op stays rejected (its
        // new ops are still from the stale-epoch client_id, never C_now).
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                viewer_era,
                &mut docs
            )
            .await
            .error
            .is_some(),
            "re-sent stale-epoch op stays fenced"
        );
    }

    /// B-20 regression (the live 9c cascade): a stale-epoch op that is a
    /// *contiguous-clock continuation* of a client already canonical in the node
    /// must still be fenced. Distinct from B-19's fresh-lineage case — here bob has
    /// a PRIOR ACCEPTED edit, so his client is in the base; the pre-fix fence (which
    /// keyed on the incoming update's own state vector) missed the continuation and
    /// let it cascade.
    #[tokio::test]
    async fn stale_epoch_continuation_of_canonical_client_is_fenced() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbx",
            "alice",
            &mut docs,
        )
        .await;

        // bob added directly as EDITOR (fresh grant ⇒ epoch 0) and makes an ACCEPTED
        // edit, so his epoch-0 client becomes part of the node's canonical lineage.
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some("editor")),
            &mut docs,
        )
        .await;
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                kb_node_update_msg_as("kbx", &fp("bob"), 0, "ACCEPTED-EDIT"),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "bob's epoch-0 edit is accepted and becomes canonical"
        );

        // Owner DEMOTES bob → viewer (epoch 1) then RE-PROMOTES → editor (epoch 2).
        // bob's editor never rotated off the epoch-0 client (no rejoin), mirroring 9c.
        for role in ["viewer", "editor"] {
            assert!(
                dispatch_as(
                    &store,
                    &bc,
                    Some("alice"),
                    Some(&fp("alice")),
                    kb_member_msg("kb/add_member", "kbx", &fp("bob"), Some(role)),
                    &mut docs
                )
                .await
                .error
                .is_none(),
                "owner role change to {role} applies"
            );
        }

        // THE EXPLOIT: bob authors a CONTINUATION under his now-stale epoch-0 client,
        // chained onto the canonical state (not a fresh lineage). Role gate passes
        // (he is an editor); the epoch fence must still reject it.
        let (canonical_state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
        let cid0 = derive_kb_client_id(&fp("bob"), 0);
        let mut ts = TextSync::from_state_with_client_id(&canonical_state, cid0).unwrap();
        let cont_update = ts.insert(0, "VIEWER-ERA-CONT ");
        let cont_msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/node_update",
            "params":{"kb_id":"kbx","node_id":"concept:n","update":update_to_base64(&cont_update)}});
        let resp = dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            cont_msg,
            &mut docs,
        )
        .await;
        let msg = resp
            .error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default();
        assert!(
            resp.error.is_some() && msg.contains("rebase required"),
            "stale-epoch continuation must be fenced (B-20); got: {msg:?}"
        );

        // NO CASCADE: the canonical content never gains the viewer-interval edit.
        let (state, _) = store.encode_state_and_sv("kb:concept:n").await.unwrap();
        let canonical = TextSync::from_state(&state).unwrap().content();
        assert!(
            !canonical.contains("VIEWER-ERA-CONT"),
            "stale continuation must not cascade; canonical = {canonical:?}"
        );

        // bob CAN still converge by re-authoring under his CURRENT epoch (2).
        assert!(
            dispatch_as(
                &store,
                &bc,
                Some("bob"),
                Some(&fp("bob")),
                kb_node_update_msg_as("kbx", &fp("bob"), 2, "REAUTHORED"),
                &mut docs
            )
            .await
            .error
            .is_none(),
            "a fresh current-epoch (2) edit is accepted"
        );
    }

    /// ADR-024 R1: `kb/node_fetch` returns a node's authoritative state+sv to a
    /// member (for adopt-and-re-author) and denies a non-member.
    #[tokio::test]
    async fn kb_node_fetch_serves_members_denies_nonmembers() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbnf",
            "alice",
            &mut docs,
        )
        .await;
        let fetch = serde_json::json!({
            "jsonrpc":"2.0","id":1,"method":"kb/node_fetch",
            "params":{"kb_id":"kbnf","node_id":"concept:n"}});

        // Owner (a member) gets state + sv.
        let resp = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            fetch.clone(),
            &mut docs,
        )
        .await;
        assert!(resp.error.is_none(), "owner fetch ok: {:?}", resp.error);
        let result = resp.result.expect("result present");
        assert!(result.get("state").and_then(|v| v.as_str()).is_some());
        assert!(result.get("sv").and_then(|v| v.as_str()).is_some());

        // A non-member is denied.
        let denied = dispatch_as(
            &store,
            &bc,
            Some("carol"),
            Some(&fp("carol")),
            fetch,
            &mut docs,
        )
        .await;
        assert!(denied.error.is_some(), "non-member fetch must be denied");
    }

    #[tokio::test]
    async fn only_owner_manages_members() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbm3",
            "alice",
            &mut docs,
        )
        .await;
        dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_member_msg("kb/add_member", "kbm3", &fp("bob"), None),
            &mut docs,
        )
        .await;
        // bob (editor, non-owner) cannot add carol.
        let denied = dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_member_msg("kb/add_member", "kbm3", &fp("carol"), None),
            &mut docs,
        )
        .await;
        assert!(denied.error.is_some(), "non-owner must not manage members");
    }

    #[tokio::test]
    async fn pending_then_approve_allows_join() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kba",
            "alice",
            &mut docs,
        )
        .await;
        // bob requests (invite default) → pending.
        dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kba"),
            &mut docs,
        )
        .await;
        // owner approves as editor.
        let ok = dispatch_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            kb_approve_msg("kba", &fp("bob"), Some("editor")),
            &mut docs,
        )
        .await;
        assert!(ok.error.is_none(), "owner approve succeeds: {:?}", ok.error);
        let coll = load_coll(&store, "kba").await;
        assert!(coll.pending().is_empty(), "approval clears pending");
        assert_eq!(coll.role_of(&fp("bob")), Some(SyncRole::Editor));
        // bob now joins as a member.
        assert!(dispatch_as(
            &store,
            &bc,
            Some("bob"),
            Some(&fp("bob")),
            kb_join_msg("kba"),
            &mut docs
        )
        .await
        .error
        .is_none());
    }

    #[tokio::test]
    async fn label_collision_two_keys_distinct_principals() {
        // Two peers share the same display label but have distinct principals — the
        // member added by one is NOT the other (no label-based impersonation).
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbc",
            "alice",
            &mut docs,
        )
        .await;
        // owner adds principal A under label "dupe".
        let a = "SHA256:keyA";
        let b = "SHA256:keyB";
        dispatch_as(&store, &bc, Some("alice"), Some(&fp("alice")),
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"kb/add_member","params":{"kb_id":"kbc","member":a,"role":"editor","label":"dupe"}}), &mut docs).await;
        let coll = load_coll(&store, "kbc").await;
        assert_eq!(coll.role_of(a), Some(SyncRole::Editor));
        assert_eq!(
            coll.role_of(b),
            None,
            "a different key with the same label is NOT a member"
        );
    }

    #[tokio::test]
    async fn raw_collection_write_smuggling_denied() {
        // A non-owner cannot escalate by sending a raw `kbc:` sync/update that
        // grants itself ownership — the membership-smuggling defense.
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(
            &store,
            &bc,
            Some("alice"),
            Some(&fp("alice")),
            "kbs",
            "alice",
            &mut docs,
        )
        .await;
        let mut coll = load_coll(&store, "kbs").await;
        let evil = coll.upsert_member(&fp("bob"), "bob", SyncRole::Owner);
        let msg = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"sync/update",
            "params":{"doc":"kbc:kbs","update":update_to_base64(&evil)}});
        let denied = dispatch_as(&store, &bc, Some("bob"), Some(&fp("bob")), msg, &mut docs).await;
        assert!(
            denied.error.is_some(),
            "non-owner raw collection write must be denied"
        );
        let after = load_coll(&store, "kbs").await;
        assert_eq!(
            after.role_of(&fp("bob")),
            None,
            "smuggled membership must not apply"
        );
    }

    #[tokio::test]
    async fn none_mode_not_gated() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut docs = HashSet::new();
        kb_share_as(&store, &bc, None, None, "kbn", "alice", &mut docs).await;
        assert!(
            dispatch_as(&store, &bc, None, None, kb_join_msg("kbn"), &mut docs)
                .await
                .error
                .is_none(),
            "none/loopback sessions are connection-trusted (dev only)"
        );
    }

    async fn share_kb_with_nodes(
        store: &Arc<DocStore>,
        bc: &SharedBroadcaster,
        kb_id: &str,
        name: &str,
        creator: &str,
        nodes: &[(&str, Vec<u8>)],
        session_docs: &mut HashSet<String>,
    ) -> JsonRpcResponse {
        use mae_sync::kb::KbCollectionDoc;

        let mut coll = KbCollectionDoc::new(name, creator);
        for (id, _) in nodes {
            coll.add_node(id, id); // title = id for simplicity
        }
        let collection_b64 = update_to_base64(&coll.encode_state());

        let nodes_json: Vec<serde_json::Value> = nodes
            .iter()
            .map(|(id, state)| serde_json::json!({ "id": id, "state": update_to_base64(state) }))
            .collect();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": kb_id,
                "name": name,
                "creator": creator,
                "collection_state": collection_b64,
                "nodes": nodes_json,
            }
        });
        handle_doc_request(
            &msg.to_string(),
            store,
            bc,
            std::time::Instant::now(),
            0,
            session_docs,
        )
        .await
    }

    #[tokio::test]
    async fn kb_share_stores_collection_and_nodes() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut session_docs = HashSet::new();

        let node1 = make_test_node(
            "concept:test",
            "Test Node",
            realistic_org_body(),
            &["research", "crdt"],
        );
        let node2 = make_test_node("concept:arch", "Architecture", "System overview", &["core"]);
        let node3 = make_test_node(
            "lesson:intro",
            "Intro Lesson",
            "Welcome to MAE",
            &["tutorial"],
        );

        let resp = share_kb_with_nodes(
            &store,
            &bc,
            "my-kb",
            "Research Notes",
            "alice",
            &[
                ("concept:test", node1),
                ("concept:arch", node2),
                ("lesson:intro", node3),
            ],
            &mut session_docs,
        )
        .await;

        assert!(resp.error.is_none(), "kb/share failed: {:?}", resp.error);
        let result = resp.result.unwrap();
        assert_eq!(result["shared"], true);
        assert_eq!(result["node_count"], 3);

        // Verify collection doc is stored.
        let (coll_state, _sv) = store.encode_state_and_sv("kbc:my-kb").await.unwrap();
        let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&coll_state)
            .expect("collection doc should decode");
        assert_eq!(coll.name(), "Research Notes");
        assert_eq!(coll.node_count(), 3, "collection should list all 3 nodes");

        // Verify each node doc is stored and decodable.
        for node_id in &["concept:test", "concept:arch", "lesson:intro"] {
            let doc_name = format!("kb:{node_id}");
            let (state, _sv) = store
                .encode_state_and_sv(&doc_name)
                .await
                .unwrap_or_else(|e| panic!("node doc '{}' should exist: {}", doc_name, e));
            let node_doc = mae_sync::kb::KbNodeDoc::from_bytes(&state)
                .unwrap_or_else(|e| panic!("node '{}' should decode: {}", node_id, e));
            assert!(
                !node_doc.title().is_empty(),
                "node '{}' title should not be empty",
                node_id
            );
        }

        // Verify session_docs tracks collection doc.
        assert!(
            session_docs.contains("kbc:my-kb"),
            "session should track collection doc"
        );
    }

    #[tokio::test]
    async fn kb_share_realistic_org_content_roundtrip() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut session_docs = HashSet::new();

        let org_body = realistic_org_body();
        let node = make_test_node("concept:org-test", "Org Round-Trip", org_body, &["test"]);

        let resp = share_kb_with_nodes(
            &store,
            &bc,
            "org-kb",
            "Org KB",
            "alice",
            &[("concept:org-test", node)],
            &mut session_docs,
        )
        .await;
        assert!(resp.error.is_none(), "kb/share failed: {:?}", resp.error);

        // Read back and verify content is byte-for-byte identical.
        let (state, _) = store
            .encode_state_and_sv("kb:concept:org-test")
            .await
            .unwrap();
        let doc = mae_sync::kb::KbNodeDoc::from_bytes(&state).unwrap();
        assert_eq!(
            doc.body(),
            org_body,
            "org body should survive server round-trip byte-for-byte"
        );
        assert_eq!(doc.title(), "Org Round-Trip");
        assert_eq!(doc.tags(), vec!["test"]);
    }

    #[tokio::test]
    async fn kb_join_returns_collection_and_all_nodes() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut sharer_docs = HashSet::new();

        // Share 3 nodes.
        let nodes = vec![
            ("n1", make_test_node("n1", "Node One", "body one", &["a"])),
            (
                "n2",
                make_test_node("n2", "Node Two", "body two — café", &["b"]),
            ),
            (
                "n3",
                make_test_node("n3", "Node Three", "body 三 日本語", &["c"]),
            ),
        ];
        share_kb_with_nodes(
            &store,
            &bc,
            "join-kb",
            "Join Test",
            "alice",
            &nodes,
            &mut sharer_docs,
        )
        .await;

        // Join from a different session.
        let mut joiner_docs = HashSet::new();
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "kb/join",
            "params": { "kb_id": "join-kb" }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            1,
            &mut joiner_docs,
        )
        .await;

        assert!(resp.error.is_none(), "kb/join failed: {:?}", resp.error);
        let result = resp.result.unwrap();

        // Verify collection state.
        let coll_b64 = result["collection_state"].as_str().unwrap();
        let coll_bytes = mae_sync::encoding::base64_to_update(coll_b64).unwrap();
        let coll = mae_sync::kb::KbCollectionDoc::from_bytes(&coll_bytes).unwrap();
        assert_eq!(coll.node_count(), 3, "collection should have 3 nodes");

        // Verify all nodes returned with correct content.
        let returned_nodes = result["nodes"].as_array().unwrap();
        assert_eq!(returned_nodes.len(), 3, "should return all 3 nodes");

        for expected in &[
            ("n1", "Node One", "body one"),
            ("n2", "Node Two", "body two — café"),
            ("n3", "Node Three", "body 三 日本語"),
        ] {
            let node_json = returned_nodes
                .iter()
                .find(|n| n["id"].as_str() == Some(expected.0))
                .unwrap_or_else(|| panic!("node '{}' should be in response", expected.0));
            let state_bytes =
                mae_sync::encoding::base64_to_update(node_json["state"].as_str().unwrap()).unwrap();
            let doc = mae_sync::kb::KbNodeDoc::from_bytes(&state_bytes).unwrap();
            assert_eq!(
                doc.title(),
                expected.1,
                "node '{}' title mismatch",
                expected.0
            );
            assert_eq!(
                doc.body(),
                expected.2,
                "node '{}' body mismatch",
                expected.0
            );
        }
    }

    #[tokio::test]
    async fn kb_join_nonexistent_returns_empty() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/join",
            "params": { "kb_id": "nonexistent-kb" }
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

        // Server creates empty doc on read (get_or_create semantics), so this
        // succeeds but returns 0 nodes — the client interprets empty collection.
        assert!(resp.error.is_none(), "kb/join creates empty doc — no error");
        let result = resp.result.unwrap();
        let nodes = result["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 0, "nonexistent KB should return 0 nodes");
    }

    #[tokio::test]
    async fn kb_node_update_applies_and_broadcasts() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut session_a = HashSet::new();

        // Share a single node.
        let node = make_test_node("n1", "Original", "original body", &[]);
        share_kb_with_nodes(
            &store,
            &bc,
            "update-kb",
            "Update Test",
            "alice",
            &[("n1", node.clone())],
            &mut session_a,
        )
        .await;

        // Subscribe session B for notifications.
        let session_b_id = 1u64;
        let mut rx = {
            let mut b = bc.lock().unwrap();
            b.subscribe(session_b_id, vec!["sync_update".to_string()]);
            b.subscribe_doc(session_b_id, "kb:n1");
            b.subscribe_doc(session_b_id, "kbc:update-kb");
            b.subscribe(session_b_id, vec!["sync_update".to_string()])
        };

        // Generate an update: change body via KbNodeDoc.
        let mut doc = mae_sync::kb::KbNodeDoc::from_bytes(&node).unwrap();
        let update = doc.set_body("updated body — café, 日本語");

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "kb/node_update",
            "params": {
                "kb_id": "update-kb",
                "node_id": "n1",
                "update": update_to_base64(&update),
            }
        });
        let resp = handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut session_a,
        )
        .await;
        assert!(
            resp.error.is_none(),
            "kb/node_update failed: {:?}",
            resp.error
        );
        assert_eq!(resp.result.unwrap()["applied"], true);

        // Verify the stored doc reflects the update.
        let (state, _) = store.encode_state_and_sv("kb:n1").await.unwrap();
        let stored = mae_sync::kb::KbNodeDoc::from_bytes(&state).unwrap();
        assert_eq!(
            stored.body(),
            "updated body — café, 日本語",
            "stored node body should reflect update"
        );

        // Verify broadcast was sent (best-effort check).
        if let Ok(EditorEvent::SyncUpdate { buffer_name, .. }) = rx.try_recv() {
            assert_eq!(
                buffer_name, "kb:n1",
                "broadcast should be for the updated node doc"
            );
        }
    }

    #[tokio::test]
    async fn kb_leave_unsubscribes_session() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut session_docs = HashSet::new();

        // Share a KB.
        let node = make_test_node("n1", "Title", "body", &[]);
        share_kb_with_nodes(
            &store,
            &bc,
            "leave-kb",
            "Leave Test",
            "alice",
            &[("n1", node)],
            &mut session_docs,
        )
        .await;

        // Verify session tracks the collection + node docs.
        assert!(session_docs.contains("kbc:leave-kb"));
        assert!(session_docs.contains("kb:n1"));

        // Leave.
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "kb/leave",
            "params": { "kb_id": "leave-kb" }
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
        assert!(resp.error.is_none(), "kb/leave failed: {:?}", resp.error);
        assert_eq!(resp.result.unwrap()["left"], true);

        // Session should no longer track collection doc.
        assert!(
            !session_docs.contains("kbc:leave-kb"),
            "session should no longer track collection doc after leave"
        );
    }

    #[tokio::test]
    async fn kb_share_with_invalid_base64_returns_error() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": {
                "kb_id": "bad-kb",
                "name": "Bad KB",
                "creator": "alice",
                "collection_state": "!!!NOT_VALID_BASE64!!!",
                "nodes": [],
            }
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
            "kb/share with invalid base64 should return error"
        );
    }

    #[tokio::test]
    async fn kb_share_missing_kb_id_returns_error() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/share",
            "params": { "name": "Test", "creator": "alice" }
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
            "kb/share without kb_id should return error"
        );
    }

    #[tokio::test]
    async fn kb_node_update_for_nonexistent_node() {
        let store = test_doc_store();
        let bc = test_broadcaster();

        // Try to update a node that was never shared.
        let mut doc = mae_sync::kb::KbNodeDoc::new("ghost", "Ghost", "body", &[]);
        let update = doc.set_body("new body");

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "kb/node_update",
            "params": {
                "kb_id": "some-kb",
                "node_id": "ghost",
                "update": update_to_base64(&update),
            }
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
        // The server creates the doc on first update (share_or_join semantics in DocStore),
        // or returns an error. Either way it shouldn't panic.
        // We just verify it doesn't crash — the exact behavior depends on DocStore.apply_update.
        // Just verify it doesn't crash — the server might create the doc on first update.
        let _ = resp;
    }

    #[tokio::test]
    async fn kb_share_then_update_then_join_sees_latest() {
        let store = test_doc_store();
        let bc = test_broadcaster();
        let mut session_a = HashSet::new();

        // Share with initial content.
        let node = make_test_node("n1", "Initial Title", "initial body", &["v1"]);
        share_kb_with_nodes(
            &store,
            &bc,
            "evolving-kb",
            "Evolving",
            "alice",
            &[("n1", node.clone())],
            &mut session_a,
        )
        .await;

        // Update the node's body.
        let mut doc = mae_sync::kb::KbNodeDoc::from_bytes(&node).unwrap();
        let update = doc.set_body("evolved body with café and 日本語");
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 2, "method": "kb/node_update",
            "params": {
                "kb_id": "evolving-kb",
                "node_id": "n1",
                "update": update_to_base64(&update),
            }
        });
        handle_doc_request(
            &msg.to_string(),
            &store,
            &bc,
            std::time::Instant::now(),
            0,
            &mut session_a,
        )
        .await;

        // Join from a new session — should see latest content.
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "kb/join",
            "params": { "kb_id": "evolving-kb" }
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
        assert!(resp.error.is_none());

        let result = resp.result.unwrap();
        let nodes = result["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 1);
        let state_bytes =
            mae_sync::encoding::base64_to_update(nodes[0]["state"].as_str().unwrap()).unwrap();
        let joined_doc = mae_sync::kb::KbNodeDoc::from_bytes(&state_bytes).unwrap();
        assert_eq!(
            joined_doc.body(),
            "evolved body with café and 日本語",
            "joined client should see the updated body, not the initial one"
        );
    }
}
