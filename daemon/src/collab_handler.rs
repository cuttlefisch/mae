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
use mae_sync::content_ops::SignedContentOp;
use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::kb::{
    derive_kb_client_id, update_new_op_authors, JoinPolicy, KbCollectionDoc, Role as SyncRole,
    Transport,
};
use mae_sync::membership::{
    derive_governance, derive_valid_members_governed, fingerprint_of, is_recovery_rebind,
    recovery_registry, Governance, MembershipAction,
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
    transport: Transport,
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
    handle_client_authenticated(
        reader,
        writer,
        peer,
        doc_store,
        broadcaster,
        start_time,
        transport,
    )
    .await;
}

/// Anonymous (no-auth) connection — used for the loopback/`none` mode.
pub async fn handle_client<R, W>(
    reader: R,
    writer: W,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    start_time: std::time::Instant,
    transport: Transport,
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
    transport: Transport,
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
    transport: Transport,
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
                    handle_doc_request_inner(&msg, &doc_store, &broadcaster, start_time, session_id, auth_label.as_deref(), auth_principal.as_deref(), auth_pubkey.as_ref(), &mut session_docs, transport).await
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
        broadcaster,
        start_time,
        session_id,
        None,
        None,
        None,
        session_docs,
        Transport::Hub,
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
    let coll = load_collection(doc_store, kb_id).await?;
    let ops = coll.oplog_ops();
    let governance = derive_governance(&ops, anchor);
    let members = derive_valid_members_governed(
        &ops,
        anchor,
        now_unix(),
        governance,
        &doc_store.membership_view_for(kb_id).await,
    );
    signed
        .admit(&members)
        .map_err(|e| format!("signed content op rejected: {e:?}"))
}

/// Resolve a KB's content trust anchor (the genesis owner pubkey): the registered
/// external anchor for a JOINED KB, else this daemon's own signer key when it is the
/// collection's owner (an OWNED KB — A is its own authority). `None` when neither
/// holds (un-anchored + not ours), in which case the caller applies the legacy gate.
pub async fn resolve_content_anchor(doc_store: &DocStore, kb_id: &str) -> Option<[u8; 32]> {
    if let Some(a) = doc_store.kb_anchor(kb_id).await {
        return Some(a);
    }
    let signer = doc_store.signer()?;
    let coll = load_collection(doc_store, kb_id).await.ok()?;
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
    let Some(node_id) = doc.strip_prefix("kb:") else {
        return Ok(None); // collection/non-KB doc — not a content op
    };
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
/// Best-effort: a signing/persist failure is logged, never fatal — the legacy
/// `member_roles` map remains authoritative until `kb_access` switches to derived
/// membership (slice 2b-6c).
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
) {
    let Some(signer) = doc_store.signer() else {
        return;
    };
    let owner = coll.owner();
    if owner.is_empty() || signer.fingerprint() != owner {
        return; // not an owned KB — the relay/hub path stays unsigned
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
            return;
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
    let ops = coll.oplog_ops();
    let governance = derive_governance(&ops, &anchor);
    let members = derive_valid_members_governed(
        &ops,
        &anchor,
        now,
        governance,
        &doc_store.membership_view_for(kb_id).await,
    );
    let signer_fp = signer.fingerprint();
    if members.get(&signer_fp).map(|m| m.role) != Some(SyncRole::Owner) {
        return Err(format!(
            "signer is not a current owner of KB '{kb_id}'; cannot revoke"
        ));
    }
    if !members.contains_key(subject) {
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
        Some(anchor) if coll.oplog_head().is_some() => {
            let ops = coll.oplog_ops();
            let governance = derive_governance(&ops, &anchor);
            derive_valid_members_governed(
                &ops,
                &anchor,
                now_unix(),
                governance,
                &doc_store.membership_view_for(kb_id).await,
            )
            .get(principal)
            .map(|m| m.epoch)
            .unwrap_or(0)
        }
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
    let coll = load_collection(doc_store, kb_id)
        .await
        .map_err(|e| format!("epoch lookup failed for KB '{kb_id}': {e}"))?;
    let epoch_now = kb_member_epoch(doc_store, kb_id, &coll, principal).await;
    let c_now = derive_kb_client_id(principal, epoch_now);
    // Full authoritative state (not just the SV) so the fence detects a contiguous-clock
    // continuation of an already-canonical client (B-20) that the update's own SV hides.
    let (base_state, _sv) = doc_store
        .encode_state_and_sv(node_doc)
        .await
        .map_err(|e| format!("node state lookup failed for '{node_id}': {e}"))?;
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
    transport: Transport,
) -> Result<AccessDecision, String> {
    let principal = match principal {
        Some(p) => p,
        None => return Ok(AccessDecision::Allow),
    };
    let coll = load_collection(doc_store, kb_id).await?;
    // ADR-026: for a KB JOINED from a relay we don't trust, an external anchor (the
    // join-ticket node-id) is registered — derive membership from the SIGNED op-log
    // rather than the relay-supplied `member_roles`. Owned / un-anchored KBs keep
    // the locally-authoritative legacy `member_roles` (the daemon owns that state).
    let role = match doc_store.kb_anchor(kb_id).await {
        Some(anchor) if coll.oplog_head().is_some() => {
            // ADR-026 §A4: read the owner-declared governance from the op-log, then
            // derive membership under it — so a `Quorum{m}` KB enforces m-of-n
            // co-signed removals (and an Owner removed by quorum loses access here)
            // exactly as every honest peer derives it. `SingleOwner` (the default)
            // reduces to the prior single-author rule.
            let ops = coll.oplog_ops();
            let governance = derive_governance(&ops, &anchor);
            derive_valid_members_governed(
                &ops,
                &anchor,
                now_unix(),
                governance,
                &doc_store.membership_view_for(kb_id).await,
            )
            .get(principal)
            .map(|m| m.role)
        }
        _ => coll.role_of(principal),
    };
    // Per-KB transport policy (ADR-018/025): a KB is reachable over a transport
    // only if its policy exposes it there — EXCEPT the owner, who always reaches
    // their own KB (e.g. their local editor over the hub socket) and is the one who
    // manages exposure. Non-owner members + would-be joiners are transport-gated.
    if role != Some(SyncRole::Owner) && !coll.transport_policy().allows(transport) {
        let t = match transport {
            Transport::Hub => "the hub",
            Transport::P2p => "the P2P mesh",
        };
        return Ok(AccessDecision::Deny(format!(
            "KB '{kb_id}' is not shared over {t}"
        )));
    }
    match role {
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

/// The current member principals as the daemon derives them for `kb_id` — the same
/// anchored-vs-legacy split [`kb_access`] uses: an anchored, op-logged KB derives the
/// set from the SIGNED op-log under its declared governance; an owned / un-anchored KB
/// reads the locally-authoritative `member_roles`. Used by the member-`Rebind` gate to
/// confirm the rotating author is a current member and the successor is fresh.
async fn current_member_set(
    doc_store: &DocStore,
    kb_id: &str,
    coll: &KbCollectionDoc,
) -> std::collections::BTreeSet<String> {
    match doc_store.kb_anchor(kb_id).await {
        Some(anchor) if coll.oplog_head().is_some() => {
            let ops = coll.oplog_ops();
            let governance = derive_governance(&ops, &anchor);
            derive_valid_members_governed(
                &ops,
                &anchor,
                now_unix(),
                governance,
                &doc_store.membership_view_for(kb_id).await,
            )
            .into_keys()
            .collect()
        }
        _ => coll
            .member_roles()
            .into_iter()
            .map(|m| m.fingerprint)
            .collect(),
    }
}

/// ADR-040 PR2c/PR3 — the member-authored **self-service** write gate. `kb/collection_op`
/// is otherwise owner-only (`KbOp::Manage`, ADR-018); this is the *single, narrow*
/// exception that lets a NON-owner member manage their **own** identity (rotation +
/// recovery-key registration + recovery rotation) without owner mediation. It accepts the
/// update **iff** every op it introduces is one of those three self-service shapes and the
/// update mutates **nothing else** in the collection — so a member cannot smuggle a
/// privilege change (an `Admit`, a `SetRole`, an owner flip) alongside it. Concretely,
/// applying `update` to the stored collection must (1) grow the op-log by ≥1 record and
/// change **only** the op-log (owner / member roster / policies / encryption byte-identical
/// before and after); and (2) introduce only NEW ops that are each exactly one of these three
/// self-service shapes:
///
/// - **member self-`Rebind`** — crypto-valid (`verify_signed`), `author` == the connection's
///   authenticated principal (you rotate yourself) AND a current member, with a well-formed
///   non-elevating successor.
/// - **member `RegisterRecoveryKey`** — crypto-valid (primary-signed), `author` == `subject`
///   == the principal AND a current member, carrying a `recovery_pubkey` (grants no roster
///   access — it just publishes the offline recovery key for a future recovery).
/// - **recovery-signed `Rebind`** (ADR-040 §Recovery-key) — signed NOT by the lost primary but
///   by the predecessor's *registered* recovery key (validated against the recovery registry
///   built from the **pre-existing** op-log), and submitted by the SUCCESSOR key's
///   authenticated connection (`subject` == principal). The lost-primary path: the holder of
///   the offline recovery key rotates a member that can no longer self-sign. The predecessor
///   must be a current member and the successor well-formed/fresh.
///
/// The self-rotation + recovery arms mirror [`membership::authorized`]'s `Rebind` arm + [`membership::crypto_valid`]'s
/// recovery filter; we re-check here because the daemon — not just the deriving peers — is
/// now an authorization point for these ops. `auth_principal` is the verified session
/// principal; `None` (un-authed local socket) never reaches here because the owner-`Manage`
/// check already allowed it. On success returns the accepted `(successor_fp, predecessor_fp)`
/// rebind pairs (rotation + recovery; registration contributes none) so the caller can mirror
/// each successor into the owned-KB roster (inheriting the predecessor's role), giving it
/// access on a roster-model daemon — the derive-based peers already alias it via the PR2a/PR3
/// post-pass.
async fn verify_member_self_service_update(
    doc_store: &DocStore,
    kb_id: &str,
    auth_principal: Option<&str>,
    update: &[u8],
) -> Result<Vec<(String, String)>, String> {
    let principal = auth_principal
        .ok_or_else(|| "member self-rotation requires an authenticated principal".to_string())?;
    let collection_doc = format!("kbc:{kb_id}");
    let (state, _sv) = doc_store
        .encode_state_and_sv(&collection_doc)
        .await
        .map_err(|e| format!("KB '{kb_id}' not found: {e}"))?;
    let before = KbCollectionDoc::from_bytes(&state).map_err(|e| format!("bad collection: {e}"))?;
    let mut after =
        KbCollectionDoc::from_bytes(&state).map_err(|e| format!("bad collection: {e}"))?;
    after
        .apply_update(update)
        .map_err(|e| format!("collection update did not apply: {e}"))?;

    // (1) The update must touch ONLY the op-log. Authority on the daemon derives from
    // the op-log (anchored) or the `member_roles` roster (owned/legacy) plus the owner
    // and the policy/encryption fields — pin every one of them so a rebind cannot ride
    // a roster or policy mutation. The roster is compared as a SET (keyed by
    // fingerprint): `member_roles()` is a yrs-map projection whose Vec order is not
    // stable across decodes, so an order-sensitive `!=` would false-positive.
    let roster_of =
        |c: &KbCollectionDoc| -> std::collections::BTreeMap<String, (SyncRole, String)> {
            c.member_roles()
                .into_iter()
                .map(|m| (m.fingerprint, (m.role, m.label)))
                .collect()
        };
    if after.owner() != before.owner()
        || roster_of(&after) != roster_of(&before)
        || after.join_policy() != before.join_policy()
        || after.transport_policy_raw() != before.transport_policy_raw()
        || after.encryption() != before.encryption()
        || after.creator() != before.creator()
    {
        return Err(
            "a member self-rotation may not modify the owner, member roster, policy, \
             or encryption state of the collection"
                .to_string(),
        );
    }

    // (2) Compute the op-log delta and require every NEW op be one of the three
    // self-service shapes. The recovery registry is built from the *pre-existing*
    // op-log (`before`) so a recovery key must already be registered to authorize a
    // recovery rotation — a registration cannot be smuggled into the same update to
    // self-authorize (the registration itself requires a primary signature, which the
    // recovering principal lacks).
    let before_ops = before.oplog_ops();
    let before_hashes: HashSet<String> = before_ops.iter().map(|o| o.chain_hash()).collect();
    let registry = recovery_registry(&before_ops);
    let new_ops: Vec<_> = after
        .oplog_ops()
        .into_iter()
        .filter(|o| !before_hashes.contains(&o.chain_hash()))
        .collect();
    if new_ops.is_empty() {
        return Err("update introduces no new membership op (not a member rotation)".to_string());
    }
    let members = current_member_set(doc_store, kb_id, &before).await;
    let mut pairs = Vec::with_capacity(new_ops.len());
    for o in &new_ops {
        match o.op.action {
            MembershipAction::Rebind => {
                // Common successor validity (shapes a + c): well-formed, fingerprint-bound,
                // non-self, fresh, with the predecessor a current member.
                let npk = match o.op.new_pubkey {
                    Some(k) => k,
                    None => {
                        return Err("rotation op is missing the successor public key".to_string())
                    }
                };
                if o.op.new_wrap_pubkey.is_none() {
                    return Err("rotation op is missing the successor wrap key".to_string());
                }
                if fingerprint_of(&npk) != o.op.subject {
                    return Err("rotation successor is not bound to its public key".to_string());
                }
                if o.op.subject == o.op.author {
                    return Err("rotation successor equals the author (no-op)".to_string());
                }
                if !members.contains(&o.op.author) {
                    return Err("rotation predecessor is not a current member".to_string());
                }
                if members.contains(&o.op.subject) {
                    return Err(
                        "rotation successor is already a member (must rotate into a fresh key)"
                            .to_string(),
                    );
                }
                if o.verify_signed() {
                    // (a) self-rotation: signed by the rotating principal's own primary, so
                    // the op author must be the authenticated connection principal.
                    if o.op.author != principal {
                        return Err("a member may only rotate their own identity".to_string());
                    }
                } else if is_recovery_rebind(o, &registry) {
                    // (c) recovery rotation: signed by the predecessor's *registered* recovery
                    // key (the lost primary cannot self-sign), submitted by the SUCCESSOR key's
                    // authenticated connection — so the recovering user proves control of the new
                    // key it is rotating into, and the recovery key proves the authority to do so.
                    if o.op.subject != principal {
                        return Err(
                            "a recovery rotation must be submitted by the successor key it rotates into"
                                .to_string(),
                        );
                    }
                } else {
                    return Err(
                        "rotation op is neither self-signed nor signed by a registered recovery key"
                            .to_string(),
                    );
                }
                pairs.push((o.op.subject.clone(), o.op.author.clone()));
            }
            MembershipAction::RegisterRecoveryKey => {
                // (b) recovery-key registration: primary-signed self-registration. Grants no
                // roster access — it only publishes the offline recovery key for a future (c).
                if !o.verify_signed() {
                    return Err("recovery-key registration signature is invalid".to_string());
                }
                if o.op.author != principal || o.op.subject != principal {
                    return Err("a member may only register their OWN recovery key".to_string());
                }
                if o.op.recovery_pubkey.is_none() {
                    return Err(
                        "recovery-key registration is missing the recovery public key".to_string(),
                    );
                }
                if !members.contains(&o.op.author) {
                    return Err("recovery-key registrant is not a current member".to_string());
                }
            }
            other => {
                return Err(format!(
                    "a member may only author a Rebind or RegisterRecoveryKey on this path, \
                     not a {} op",
                    other.as_str()
                ));
            }
        }
    }
    Ok(pairs)
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
    if let Some(kb_id) = doc_name.strip_prefix("kbc:") {
        match kb_access(doc_store, kb_id, principal, KbOp::Manage, transport).await? {
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
    auth_pubkey: Option<&[u8; 32]>,
    session_docs: &mut HashSet<String>,
    transport: Transport,
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
            if let Err(e) =
                deny_collection_smuggling(doc_store, &doc_name, auth_principal, transport).await
            {
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

            // ADR-036 §D3 (B→A): a content op arriving over the mesh — a peer daemon's
            // forwarded edit — is re-verified against THIS KB's membership before
            // apply, because the relaying peer is untrusted (even a member's *daemon*
            // could lie). The joiner's dialer sends `kb_id`; require-signed on P2p, the
            // hub accepts legacy unsigned during migration. Same shared check as the
            // dialer's inbound path. On success the verified header is carried onward.
            let mut sync_content_header: Option<serde_json::Value> = None;
            if let Some(kb_id) = params.get("kb_id").and_then(|v| v.as_str()) {
                match verify_relayed_content_op(
                    doc_store,
                    kb_id,
                    &doc_name,
                    &update_bytes,
                    params.get("content_header"),
                    matches!(transport, Transport::P2p),
                )
                .await
                {
                    Ok(verified) => sync_content_header = verified,
                    Err(e) => {
                        warn!(session = session_id, doc = %doc_name, reason = %e, "sync/update: SIGNED CONTENT OP REJECTED (ADR-036)");
                        return JsonRpcResponse::error(id, McpError::internal_error(e));
                    }
                }
            }

            // #169 M1: a `kb:{node}` write arriving via `sync/update` must carry its `kb_id`
            // and clear the ADR-023 epoch fence — the SAME mediation as `kb/node_update` and
            // the mesh dialer. Before this, a bare `kb:` `sync/update` (no `kb_id`) skipped
            // `verify_relayed_content_op` (it's gated on `kb_id`), `kb_access`, AND the fence,
            // writing the node CRDT with no role/signature/epoch check, then broadcasting.
            // Text buffers (non-`kb:` docs) are unaffected.
            if let Some(node_id) = doc_name.strip_prefix("kb:") {
                match params.get("kb_id").and_then(|v| v.as_str()) {
                    Some(kb_id) => {
                        // Fence on the VERIFIED header author (a relayed op's true author —
                        // never the connection principal), mirroring the dialer's #157 N1 path.
                        if let Some(author) = sync_content_header
                            .as_ref()
                            .and_then(|h| h.get("author").and_then(|a| a.as_str()))
                        {
                            if let Err(reason) = enforce_epoch_fence(
                                doc_store,
                                kb_id,
                                node_id,
                                &doc_name,
                                &update_bytes,
                                author,
                            )
                            .await
                            {
                                warn!(session = session_id, doc = %doc_name, %reason, "sync/update: kb node REJECTED (epoch fence — #169 M1)");
                                return JsonRpcResponse::error(
                                    id,
                                    McpError::internal_error(reason),
                                );
                            }
                        }
                    }
                    None => {
                        warn!(session = session_id, doc = %doc_name, "sync/update: kb node write WITHOUT kb_id — REJECTED (would bypass membership + epoch fence, #169 M1)");
                        return JsonRpcResponse::error(
                            id,
                            McpError::internal_error(
                                "kb node writes via sync/update must carry kb_id".to_string(),
                            ),
                        );
                    }
                }
            }

            match doc_store
                .apply_update(&doc_name, &update_bytes, client_id)
                .await
            {
                Ok(result) => {
                    // Broadcast to other subscribers (skip sender to avoid echo).
                    {
                        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                        bc.broadcast_except(
                            &EditorEvent::SyncUpdate {
                                buffer_name: doc_name.clone(),
                                update_base64: update_to_base64(&result.update),
                                wal_seq: result.wal_seq,
                                content_header: sync_content_header,
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
                let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
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

        "sync/share" => {
            // BUG D fix: use atomic share_doc (delete + create + connected_clients=1).
            let doc_name = params["doc"].as_str().unwrap_or("default").to_string();
            info!(session = session_id, doc = %doc_name, "sync/share: processing");
            // Track this doc for disconnect cleanup and doc-scoped broadcast filtering.
            session_docs.insert(doc_name.clone());
            broadcaster
                .lock()
                .unwrap_or_else(|e| e.into_inner())
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
                        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                        bc.broadcast_except(
                            &EditorEvent::SyncUpdate {
                                buffer_name: doc_name.clone(),
                                update_base64: update_to_base64(&result.update),
                                wal_seq: result.wal_seq,
                                content_header: None,
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
                .unwrap_or_else(|e| e.into_inner())
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
                    Err(e) => {
                        warn!(
                            kb_id = %kb_id,
                            error = %e,
                            "kb/share: owner-binding decode failed; using client collection unbound"
                        );
                        collection_bytes
                    }
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
                .unwrap_or_else(|e| e.into_inner())
                .subscribe_doc(session_id, &collection_doc);

            // Transport exposure (ADR-018/025): the `transport` param (hub|p2p|both,
            // default hub) WIDENS the KB's policy from its stored value, so a first
            // p2p-share is P2p-only while `kb-share` + `kb-share-p2p` becomes Both
            // (unlike membership, transport is owner-set metadata — safe on
            // re-share). This is how `kb-share-p2p` *establishes* a mesh-exposed KB.
            let requested_transport = params["transport"]
                .as_str()
                .and_then(mae_sync::kb::TransportPolicy::parse)
                .unwrap_or(mae_sync::kb::TransportPolicy::Hub);
            if let Ok(mut coll) = load_collection(doc_store, &kb_id).await {
                let raw = coll.transport_policy_raw();
                let new = raw.map_or(requested_transport, |c| c.union(requested_transport));
                if Some(new) != raw {
                    let update = coll.set_transport_policy(new);
                    if let Err(e) = persist_and_broadcast_collection(
                        doc_store,
                        broadcaster,
                        session_id,
                        &kb_id,
                        &update,
                    )
                    .await
                    {
                        warn!(kb_id = %kb_id, error = %e, "kb/share: failed to set transport policy");
                    } else {
                        info!(kb_id = %kb_id, transport = %new.as_str(), "kb/share: transport exposure set");
                    }
                }
            }

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
            // Return the AUTHORITATIVE collection state (post-set_owner, or the
            // preserved one on re-share — B-12) so the owner can seed a local
            // collection replica and introspect its own KB's members/roles/policy
            // on the same CRDT lineage future `kbc:` broadcasts advance (C1).
            let owner_collection = doc_store
                .encode_state_and_sv(&collection_doc)
                .await
                .map(|(state, _sv)| update_to_base64(&state))
                .unwrap_or_default();
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "kb_id": kb_id,
                    "shared": true,
                    "node_count": node_count,
                    "collection_state": owner_collection,
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

            // ADR-041 (#158 I1): the joiner's PUBLISHED X25519 wrap key (hex), sent in the
            // request — the daemon can't derive it (it's not the ed25519 session key), so
            // the joiner publishes it for the owner to wrap the content key to on approval.
            let join_wrap_pubkey: Option<[u8; 32]> = params["wrap_pubkey"]
                .as_str()
                .and_then(|h| hex::decode(h).ok())
                .and_then(|b| <[u8; 32]>::try_from(b.as_slice()).ok());

            // ADR-018 access gate (complete mediation): member → join; non-member
            // resolved by the KB's join policy (restrictive/invite/permissive).
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Join, transport).await {
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
                            auth_pubkey,
                            join_wrap_pubkey.as_ref(),
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
            {
                let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                bc.subscribe_doc(session_id, &collection_doc);
                // Subscribe to sync_update AS OF the join snapshot, so the owner's
                // edits made between this snapshot and the member's separate
                // notifications/subscribe are still pushed (no missed-edit window).
                bc.add_event_sub(session_id, "sync_update");
            }

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
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Read, transport).await {
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
            // ADR-037 (#171) RESEAL-AS-REPLACE: when E2E is enabled on a
            // previously-plaintext KB, the owner reseals each node as a FRESH op-set and
            // sets `reseal:true` so we PURGE the pre-enable plaintext lineage — `share_doc`
            // atomically deletes the `kb:{node}` snapshot+WAL and recreates it from this
            // ciphertext-only op (vs `apply_update`, which would stack the op-set on top of
            // the readable plaintext). It is OWNER-gated (Manage) and bypasses the epoch
            // fence (a full-doc replace, not a merge against the daemon's node SV).
            let reseal = params["reseal"].as_bool().unwrap_or(false);
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
            // non-members are denied (least privilege). A RESEAL replaces the whole node
            // doc, so it requires Manage (owner-only) — a mere Editor cannot purge/replace
            // another's node lineage.
            let required_op = if reseal { KbOp::Manage } else { KbOp::Edit };
            match kb_access(doc_store, &kb_id, auth_principal, required_op, transport).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::Deny(msg)) | Err(msg) => {
                    warn!(session = session_id, kb_id = %kb_id, reseal, reason = %msg, "kb/node_update denied");
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

            // ADR-036 §D3 SIGNED CONTENT OP VERIFICATION. If the op carries a signed
            // authorship header, the content AUTHOR (from the *signed header*, not the
            // connection principal) must be a current Editor+ member at the op's
            // epoch, and the signature must bind that author to this exact
            // (kb_id, node_id, payload). This is what makes a *relayed* edit
            // peer-verifiable: a hostile relay can neither forge an edit nor
            // mis-attribute one (the node_id is signed, so a header claiming a
            // different node than it carries fails `verify_signed`). Enforced for an
            // ANCHORED (joined) KB, whose authority IS the signed op-log; an owned KB
            // over the trusted local socket keeps the legacy gate below. An unsigned
            // op (no header) falls through to that legacy epoch fence — the migration
            // path (mesh-side require-signed lands with the dialer-relay slice).
            // When verified, the authorship header is carried into the broadcast (and
            // thus the dialer's relay to peers) so a downstream peer re-verifies the
            // same op — `verify_content_op` is the single check shared with the mesh
            // relay path (principle #8).
            let mut content_header: Option<serde_json::Value> = None;
            if let Some(anchor) = doc_store.kb_anchor(&kb_id).await {
                if let Some(signed) = SignedContentOp::from_params(&params, update_bytes.clone()) {
                    if let Err(e) = verify_content_op(doc_store, &kb_id, &anchor, &signed).await {
                        warn!(
                            session = session_id, kb_id = %kb_id, node_id = %node_id,
                            author = %signed.op.author, reason = %e,
                            "kb/node_update: SIGNED CONTENT OP REJECTED (ADR-036)"
                        );
                        return JsonRpcResponse::error(id, McpError::internal_error(e));
                    }
                    info!(
                        session = session_id, kb_id = %kb_id, node_id = %node_id,
                        author = %signed.op.author,
                        "kb/node_update: signed content op verified (ADR-036)"
                    );
                    content_header = Some(signed.header_params());
                }
            }

            // ADR-023 (B-19) EPOCH FENCE — the security core. `kb_access` above
            // confirmed the sender's *current* role permits editing, but a granted
            // member must author under their *current-epoch* client_id. Decode the
            // update's NEW ops (those beyond the daemon's authoritative node SV) and
            // reject any authored under a stale-epoch client_id. That is precisely a
            // member's pre-grant divergent lineage (e.g. viewer-era edits) trying to
            // cascade after a grant — denied here, at the daemon, independent of any
            // (possibly malicious) client behaviour. Only enforced in key-auth mode:
            // `none`/loopback has no per-identity principal to derive an epoch from.
            // A reseal REPLACES the doc (fresh op-set at clock 0) — there is no prior-SV
            // merge to fence, and the owner's op is authored under its current epoch
            // anyway, so the fence is skipped for it (owner-gated above).
            if !reseal {
                if let Some(principal) = auth_principal {
                    if let Err(reason) = enforce_epoch_fence(
                        doc_store,
                        &kb_id,
                        &node_id,
                        &node_doc,
                        &update_bytes,
                        principal,
                    )
                    .await
                    {
                        warn!(
                            session = session_id, kb_id = %kb_id, node_id = %node_id, %reason,
                            "kb/node_update: epoch fence rejected (B-19)"
                        );
                        return JsonRpcResponse::error(id, McpError::internal_error(reason));
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
            // #171: a reseal PURGES + replaces (delete plaintext snapshot+WAL, recreate
            // from this ciphertext-only op); an ordinary edit MERGES into the CRDT.
            let applied = if reseal {
                doc_store.share_doc(&node_doc, &update_bytes).await
            } else {
                doc_store.apply_update(&node_doc, &update_bytes, None).await
            };
            match applied {
                Ok(result) => {
                    // Broadcast to other subscribers of the collection.
                    {
                        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
                        bc.broadcast_except(
                            &EditorEvent::SyncUpdate {
                                buffer_name: node_doc.clone(),
                                update_base64: update_to_base64(&result.update),
                                wal_seq: result.wal_seq,
                                // Relay the verified authorship header so a peer daemon
                                // re-verifies this op (ADR-036 mesh propagation).
                                content_header,
                            },
                            session_id,
                        );
                    }
                    info!(session = session_id, kb_id = %kb_id, node_id = %node_id, wal_seq = result.wal_seq, reseal, "kb/node_update: applied");
                    JsonRpcResponse::success(id, serde_json::json!({ "applied": true }))
                }
                Err(e) => JsonRpcResponse::error(
                    id,
                    McpError::internal_error(format!("failed to apply node update: {e}")),
                ),
            }
        }

        "kb/collection_op" => {
            // ADR-037 Phase 3a: the editor's outbound collection-write path. The owner
            // authors a signed collection delta LOCALLY (a signed membership op, an
            // encryption-flag flip, …) and ships the opaque bytes here. The daemon stays
            // KEY-BLIND: it never inspects op semantics nor holds a content key — it only
            // confirms owner authority (Manage) and stores + rebroadcasts the
            // owner-signed bytes. `append_signed_op` (which produced them) does not
            // validate; every peer DERIVES validity from the signed log. This is the
            // only way to author signed membership for an EDITOR-owned KB while keeping
            // the daemon unable to read content (the owner, not the daemon, holds the key).
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    );
                }
            };
            let update = match params["update"].as_str() {
                Some(s) => match base64_to_update(s) {
                    Ok(b) => b,
                    Err(e) => {
                        return JsonRpcResponse::error(
                            id,
                            McpError::parse_error(format!("invalid 'update' base64: {e}")),
                        );
                    }
                },
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'update' field".to_string()),
                    );
                }
            };
            if update.len() > MAX_UPDATE_SIZE {
                return JsonRpcResponse::error(
                    id,
                    McpError::internal_error(format!(
                        "collection update exceeds {MAX_UPDATE_SIZE} bytes"
                    )),
                );
            }
            // ADR-018: collection ops are owner-only (Manage) — with ONE narrow
            // exception (ADR-040 PR2c/PR3): a non-owner member managing their OWN identity
            // (self-rotation, recovery-key registration, or recovery rotation). Probe Manage
            // first; if it denies, accept the update IFF it is exactly such a self-service op
            // (`verify_member_self_service_update`) and nothing else.
            let manage =
                kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await;
            // The `(successor, predecessor)` pairs of an accepted member rotation/recovery,
            // empty for the owner path and for a bare recovery-key registration — used to
            // mirror the successor into the roster below.
            let mut rebind_pairs: Vec<(String, String)> = Vec::new();
            match manage {
                Ok(AccessDecision::Allow) => {}
                other => {
                    match verify_member_self_service_update(
                        doc_store,
                        &kb_id,
                        auth_principal,
                        &update,
                    )
                    .await
                    {
                        Ok(pairs) => {
                            info!(
                                session = session_id,
                                kb_id = %kb_id,
                                principal = auth_principal.unwrap_or("?"),
                                rebinds = pairs.len(),
                                "kb/collection_op: accepted a member self-service identity op (ADR-040 PR2c/PR3)"
                            );
                            rebind_pairs = pairs;
                        }
                        Err(rebind_reason) => {
                            let base = match other {
                                Ok(AccessDecision::Deny(m)) | Err(m) => m,
                                _ => format!("not authorized to manage KB '{kb_id}'"),
                            };
                            return JsonRpcResponse::error(
                                id,
                                McpError::internal_error(format!(
                                    "{base} (and not a member self-service identity op: {rebind_reason})"
                                )),
                            );
                        }
                    }
                }
            }
            // #156 F5: the owner sets `scrub` on the enable-time manifest-title-blank op so
            // the daemon force-COMPACTS the `kbc:` doc after applying — re-snapshotting the
            // title-blanked state and trimming + TRUNCATE-checkpointing the WAL (secure_delete
            // zeroes the freed pages), so the pre-enable cleartext title cannot linger in the
            // `kbc:` WAL at rest. (The manifest doc is mutated in place, not replaced like a
            // node doc under #171.) Additive flag — an old daemon ignores it.
            let scrub = params["scrub"].as_bool().unwrap_or(false);
            match persist_and_broadcast_collection(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &update,
            )
            .await
            {
                Ok(wal_seq) => {
                    if scrub {
                        let coll_doc = format!("kbc:{kb_id}");
                        if let Err(e) = doc_store.compact_doc(&coll_doc).await {
                            warn!(session = session_id, kb_id = %kb_id, error = %e, "kb/collection_op: scrub compaction failed (title may linger in the WAL until next compaction)");
                        }
                    }
                    // ADR-040 PR2c/PR3: a member self-rotation or recovery rotation only appends
                    // the `Rebind` to the op-log (a bare recovery-key registration appends none).
                    // On a roster-model (owned/un-anchored) daemon, mirror each successor into the
                    // `member_roles` roster with the PREDECESSOR's role so
                    // it gains access here too — parallel to how `kb/add_member` mirrors
                    // roster↔op-log. Additive: the predecessor is left in place (no lockout of
                    // the rotating session); the user removes the old key explicitly. Derive
                    // peers already alias + retire via the PR2a post-pass. Key-blind (the
                    // roster holds no secrets).
                    if !rebind_pairs.is_empty() {
                        if let Ok(mut coll) = load_collection(doc_store, &kb_id).await {
                            for (successor, predecessor) in &rebind_pairs {
                                let role = coll.role_of(predecessor).unwrap_or(SyncRole::Editor);
                                let u = coll.upsert_member(successor, successor, role);
                                if let Err(e) = persist_and_broadcast_collection(
                                    doc_store,
                                    broadcaster,
                                    session_id,
                                    &kb_id,
                                    &u,
                                )
                                .await
                                {
                                    warn!(session = session_id, kb_id = %kb_id, error = %e, "kb/collection_op: failed to mirror rotated successor into the roster");
                                }
                            }
                        }
                    }
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "applied": true, "wal_seq": wal_seq, "scrubbed": scrub }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
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
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
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
                    // ADR-026: mirror the change into the signed op-log (owned KBs).
                    let (action, signed_role) = if add {
                        (MembershipAction::Admit, Some(role))
                    } else {
                        (MembershipAction::Remove, None)
                    };
                    append_signed_membership(
                        doc_store,
                        broadcaster,
                        session_id,
                        &kb_id,
                        &mut coll,
                        action,
                        &member,
                        signed_role,
                        false,
                        None,
                    )
                    .await;
                    info!(session = session_id, kb_id = %kb_id, member = %member, add, role = role.as_str(), "kb membership change");
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "kb_id": kb_id, "member": member, "added": add }),
                    )
                }
                Err(e) => JsonRpcResponse::error(id, McpError::internal_error(e)),
            }
        }

        // Phase D1.1 (ADR-029): add/remove a node in a KB's collection manifest
        // (`kbc:{kb_id}`). The projector materializes manifest membership into cozo, so
        // this is how a *created* node joins the daemon's projection and a *deleted* one
        // leaves it (the node doc itself rides `kb/node_update`). Daemon computes the
        // collection update server-side (mirrors `kb/add_member`); authorized at Editor
        // level (KbOp::Edit) like a node edit, and broadcast to other subscribers.
        "kb/collection_node_add" | "kb/collection_node_remove" => {
            let add = request.method == "kb/collection_node_add";
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
            let title = params["title"].as_str().unwrap_or("");
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Edit, transport).await {
                Ok(AccessDecision::Allow) => {}
                Ok(AccessDecision::Deny(m)) | Err(m) => {
                    return JsonRpcResponse::error(id, McpError::internal_error(m));
                }
                Ok(_) => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!("not authorized to edit KB '{kb_id}'")),
                    );
                }
            }
            let mut coll = match load_collection(doc_store, &kb_id).await {
                Ok(c) => c,
                Err(e) => return JsonRpcResponse::error(id, McpError::internal_error(e)),
            };
            let update = if add {
                coll.add_node(&node_id, title)
            } else {
                coll.remove_node(&node_id)
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
                    info!(session = session_id, kb_id = %kb_id, node_id = %node_id, add, "kb collection-manifest change");
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "kb_id": kb_id, "node_id": node_id, "added": add }),
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
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
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

        // ADR-039 A2 (#162): manage this daemon's LOCAL self-protection blocklist for a KB.
        // Deliberately NOT owner-gated and NOT routed through `kb_access`:
        //  - blocking even the owner is the explicit capability (you reach for a local
        //    block precisely when you *cannot* get a principal globally removed — e.g. you
        //    lack quorum — but want THIS daemon to stop trusting them);
        //  - it mutates only local trust, never the synced `kbc:` collection, and is never
        //    broadcast — so there is nothing for a peer to observe and no membership to gate on.
        // The block is honored at every membership-derivation site (`membership_view_for`),
        // so once set it fences the principal at the access gate AND the content paths AND
        // the removal derivation (complete mediation). Authz floor: it changes only this
        // daemon's local state, so any trusted operator of this daemon may set it (a
        // loopback `none` session — already fully trusted by `kb_access` — or an
        // authenticated client). Trade-off: an authenticated remote client of a *shared*
        // daemon could set a local block (a bounded, reversible local nuisance — never a
        // privilege-escalation or data-exposure). Tightening this to the Unix operator
        // socket only is a documented hardening follow-up (the handler isn't told the
        // socket kind today; `Transport` is Hub/P2p, not Unix/Tcp).
        "kb/block_principal" | "kb/unblock_principal" => {
            let block = request.method == "kb/block_principal";
            let kb_id = match params["kb_id"].as_str() {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    )
                }
            };
            // Accept `fingerprint` (the canonical name) or `principal` as an alias.
            let principal = match params["fingerprint"]
                .as_str()
                .or_else(|| params["principal"].as_str())
            {
                Some(s) if !s.is_empty() => s.to_string(),
                _ => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error(
                            "missing 'fingerprint' field (the principal to block)".to_string(),
                        ),
                    )
                }
            };
            let result = if block {
                doc_store.add_kb_block(&kb_id, &principal).await
            } else {
                doc_store.remove_kb_block(&kb_id, &principal).await
            };
            match result {
                Ok(()) => {
                    info!(
                        session = session_id,
                        operator = ?auth_principal,
                        kb_id = %kb_id,
                        principal = %principal,
                        block,
                        "kb local blocklist updated (ADR-039 A2; local-only, not propagated)"
                    );
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({
                            "kb_id": kb_id,
                            "fingerprint": principal,
                            "blocked": block,
                        }),
                    )
                }
                Err(e) => JsonRpcResponse::error(
                    id,
                    McpError::internal_error(format!("blocklist update failed: {e}")),
                ),
            }
        }

        // ADR-039 A2 (#162): read this daemon's LOCAL blocklist. Read-only introspection
        // for the operator's `*KB Sharing*` Blocked view — the ONLY way a client learns
        // the local blocklist, since it is never in the synced `kbc:` collection. Returns
        // `{ blocklist: { kb_id: [fingerprint, ...] } }`; with `kb_id` it scopes to one.
        "kb/blocklist" => {
            let all = doc_store.all_kb_blocklists().await;
            let payload = match params["kb_id"].as_str() {
                Some(kb_id) => {
                    let one = all.get(kb_id).cloned().unwrap_or_default();
                    serde_json::json!({ "blocklist": { kb_id: one } })
                }
                None => serde_json::json!({ "blocklist": all }),
            };
            JsonRpcResponse::success(id, payload)
        }

        // ADR-026 §A4: set the KB's governance rule (`single-owner` | `quorum:N`),
        // recorded as an owner-signed `SetGovernance` op in the membership log so
        // every peer derives the same rule. Genesis-owner-only: changing the rule is
        // anchored to the trust root (`derive_governance` counts only the anchored
        // owner's op), so a non-owner daemon — which can only join, never sign as the
        // owner — is rejected with a clear error rather than a silent no-op.
        "kb/set_governance" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    )
                }
            };
            let governance = match params["governance"]
                .as_str()
                .and_then(Governance::parse_spec)
            {
                Some(g) => g,
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error(
                            "governance must be single-owner|quorum:N (N>=1)".to_string(),
                        ),
                    )
                }
            };
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
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
            // Only the genesis owner's signature counts toward `derive_governance`;
            // reject up front so the caller never sees a false success.
            match doc_store.signer() {
                Some(s) if s.fingerprint() == coll.owner() && !coll.owner().is_empty() => {}
                _ => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::internal_error(format!(
                            "only the owner of KB '{kb_id}' may set its governance"
                        )),
                    )
                }
            }
            append_signed_membership(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &mut coll,
                MembershipAction::SetGovernance,
                &governance.to_spec(),
                None,
                false,
                None,
            )
            .await;
            info!(session = session_id, kb_id = %kb_id, governance = %governance.to_spec(), "kb/set_governance");
            JsonRpcResponse::success(
                id,
                serde_json::json!({ "kb_id": kb_id, "governance": governance.to_spec() }),
            )
        }

        // ADR-026 §A4: strong-removal of a member, the m-of-n quorum co-sign
        // primitive. Each current owner's daemon contributes one distinct-author
        // `Revoke`; under `Quorum{m}` the member is removed once m distinct owners
        // have co-signed (under `SingleOwner` a single owner's revoke suffices).
        // Authorized at Manage (any current owner) — distinct from `kb/remove_member`
        // (genesis-owner-only owned-KB housekeeping).
        "kb/revoke" => {
            let kb_id = match params["kb_id"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'kb_id' field".to_string()),
                    )
                }
            };
            let member = match params["member"].as_str() {
                Some(s) => s.to_string(),
                None => {
                    return JsonRpcResponse::error(
                        id,
                        McpError::parse_error("missing 'member' field".to_string()),
                    )
                }
            };
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
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
            match append_signed_revoke(
                doc_store,
                broadcaster,
                session_id,
                &kb_id,
                &mut coll,
                &member,
            )
            .await
            {
                Ok(()) => {
                    info!(session = session_id, kb_id = %kb_id, member = %member, "kb/revoke: signed co-removal appended");
                    JsonRpcResponse::success(
                        id,
                        serde_json::json!({ "kb_id": kb_id, "member": member, "revoked": true }),
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
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
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
            match kb_access(doc_store, &kb_id, auth_principal, KbOp::Manage, transport).await {
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
                    // ADR-026: an approval is a signed Admit in the op-log.
                    append_signed_membership(
                        doc_store,
                        broadcaster,
                        session_id,
                        &kb_id,
                        &mut coll,
                        MembershipAction::Admit,
                        &principal,
                        Some(role),
                        false,
                        None,
                    )
                    .await;
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
                    Err(e) => {
                        warn!(
                            kb_id = %kb_id,
                            error = %e,
                            "kb/leave: collection decode failed; no nodes to unsubscribe"
                        );
                        Vec::new()
                    }
                },
                Err(e) => {
                    debug!(
                        kb_id = %kb_id,
                        error = %e,
                        "kb/leave: collection state unavailable; no nodes to unsubscribe"
                    );
                    Vec::new()
                }
            };

            // Unsubscribe from collection doc.
            session_docs.remove(&collection_doc);
            broadcaster
                .lock()
                .unwrap_or_else(|e| e.into_inner())
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
#[path = "collab_handler_tests.rs"]
mod tests;
