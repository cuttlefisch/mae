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
//! .claude/commands/mae-audit.md's "Known exceptions" and ROADMAP.md's
//! "Architecture Debt" section.

mod docs_methods;
mod kb_content;
mod kb_governance;
mod kb_membership;
mod sync_methods;

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
    fingerprint_of, is_recovery_rebind, recovery_registry, Governance, MembershipAction,
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
    kb_access_with_coll(doc_store, kb_id, principal, op, transport, None).await
}

/// As [`kb_access`], but accepts a pre-loaded collection snapshot so a handler
/// running several gates on one request loads `kbc:{kb_id}` once. `None` loads
/// itself — identical to [`kb_access`]. The `None`-principal (loopback) case
/// returns before any load either way.
async fn kb_access_with_coll(
    doc_store: &DocStore,
    kb_id: &str,
    principal: Option<&str>,
    op: KbOp,
    transport: Transport,
    coll: Option<&KbCollectionDoc>,
) -> Result<AccessDecision, String> {
    let principal = match principal {
        Some(p) => p,
        None => return Ok(AccessDecision::Allow),
    };
    let loaded;
    let coll = match coll {
        Some(c) => c,
        None => {
            loaded = load_collection(doc_store, kb_id).await?;
            &loaded
        }
    };
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
            // ADR-042 (#247): membership derivation is memoized in `derived_membership` — an
            // unchanged op-log (state-vector) + anchor + timebox horizon returns the cached set
            // without re-decoding the whole op-log. This gate runs on every anchored/E2E access;
            // the cache is what keeps it O(1) at membership-churn scale.
            doc_store
                .derived_membership(kb_id, coll, &anchor, now_unix())
                .await
                .members
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
        Some(anchor) if coll.oplog_head().is_some() => doc_store
            .derived_membership(kb_id, coll, &anchor, now_unix())
            .await
            .members
            .keys()
            .cloned()
            .collect(),
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
/// Extract the `(successor, predecessor)` pairs of the authenticated principal's OWN
/// self-`Rebind`s from a collection `update`, for mirroring an **owner's** rotation into
/// the roster (#265).
///
/// Unlike [`verify_member_self_service_update`] — which requires the WHOLE update be a bare
/// self-service op and is only reached when `Manage` DENIES — an owner reaches the caller
/// via `Manage = Allow` and is authorized to make OTHER changes in the same update (e.g.
/// re-wrapping the E2E content key to the new key). So we cannot run the strict
/// append-only/every-op-is-self-service gate; instead we scan for JUST the caller's own
/// valid self-`Rebind`s and ignore the rest. A pair is emitted ONLY when the new `Rebind`
/// is authored by `principal`, self-signed by `principal`'s primary, and binds a fresh
/// successor key to its fingerprint — so this can never inject an arbitrary member (the
/// predecessor is always the authenticated caller, and the successor inherits the caller's
/// own role). Best-effort: any decode/apply failure yields no pairs (the op-log write still
/// happens; only the roster mirror is skipped).
async fn owner_self_rebind_pairs(
    doc_store: &DocStore,
    kb_id: &str,
    principal: &str,
    update: &[u8],
) -> Vec<(String, String)> {
    if principal.is_empty() {
        return Vec::new();
    }
    let collection_doc = format!("kbc:{kb_id}");
    let Ok((state, _sv)) = doc_store.encode_state_and_sv(&collection_doc).await else {
        return Vec::new();
    };
    let Ok(before) = KbCollectionDoc::from_bytes(&state) else {
        return Vec::new();
    };
    let Ok(mut after) = KbCollectionDoc::from_bytes(&state) else {
        return Vec::new();
    };
    if after.apply_update(update).is_err() {
        return Vec::new();
    }
    let before_hashes: HashSet<String> =
        before.oplog_ops().iter().map(|o| o.chain_hash()).collect();
    let mut pairs = Vec::new();
    for o in after.oplog_ops() {
        if before_hashes.contains(&o.chain_hash()) {
            continue; // only NEW ops
        }
        if o.op.action != MembershipAction::Rebind {
            continue;
        }
        let Some(npk) = o.op.new_pubkey else { continue };
        // The caller's OWN self-rotation: predecessor (author) is the authenticated
        // principal, self-signed by its primary, successor bound to a fresh key.
        if o.op.author != principal || o.op.subject == o.op.author {
            continue;
        }
        if fingerprint_of(&npk) != o.op.subject || !o.verify_signed() {
            continue;
        }
        pairs.push((o.op.subject.clone(), o.op.author.clone()));
    }
    pairs
}

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
    let after_ops = after.oplog_ops();
    let after_hashes: HashSet<String> = after_ops.iter().map(|o| o.chain_hash()).collect();
    // The membership op-log is APPEND-ONLY: for an anchored/E2e KB the authoritative
    // membership + governance + encryption are DERIVED from it (not the manifest roster this
    // gate pins), so a self-service update must never DELETE a pre-existing op. Without this,
    // a member could ride a valid self-`Rebind` while dropping a co-member's `Admit`, the
    // owner's `SetEncryption("e2e")` (an ADR-039 anti-downgrade attack), or the genesis (DoS)
    // — none of which touch the pinned manifest fields. Reject any update that loses a prior op.
    if !before_hashes.is_subset(&after_hashes) {
        return Err(
            "a member self-service update may not remove or rewrite any existing membership \
             op — the op-log is append-only"
                .to_string(),
        );
    }
    let new_ops: Vec<_> = after_ops
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
    if let Some(kb_id) = doc_name.strip_prefix("kbc:") {
        match kb_access(doc_store, kb_id, principal, KbOp::Read, transport).await? {
            AccessDecision::Allow => Ok(()),
            _ => Err(format!(
                "not authorized to read the collection doc for KB '{kb_id}' (members only)"
            )),
        }
    } else if doc_name.starts_with("kb:") {
        Err(
            "KB node content must be fetched via the access-gated `kb/node_fetch`, \
             not the raw `sync/full_state` / `sync/state_vector` path"
                .to_string(),
        )
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

        "sync/update" => {
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

        "sync/awareness" => {
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

        "sync/full_state" => {
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

        "sync/diff" => sync_methods::handle_sync_diff(doc_store, id, &params).await,

        "docs/list" => docs_methods::handle_docs_list(doc_store, id).await,

        "docs/content" => docs_methods::handle_docs_content(doc_store, id, &params).await,

        "sync/resync" => {
            sync_methods::handle_sync_resync(
                doc_store,
                broadcaster,
                session_id,
                session_docs,
                id,
                &params,
            )
            .await
        }

        "docs/stats" => docs_methods::handle_docs_stats(doc_store, id, &params).await,

        "docs/metadata" => {
            docs_methods::handle_docs_metadata(doc_store, broadcaster, id, &params).await
        }

        "docs/save_intent" => {
            docs_methods::handle_docs_save_intent(doc_store, session_id, id, &params).await
        }

        "docs/save_committed" => {
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

        "sync/share" => {
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

        "docs/delete" => docs_methods::handle_docs_delete(doc_store, session_id, id, &params).await,

        "$/debug" => docs_methods::handle_debug_stats(doc_store, broadcaster, start_time, id).await,

        "kb/register" => {
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

        "kb/list" => kb_membership::handle_kb_list(doc_store, id).await,

        "kb/unregister" => {
            kb_membership::handle_kb_unregister(doc_store, session_id, session_docs, id, &params)
                .await
        }

        "kb/share" => {
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

        "kb/join" => {
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

        "kb/node_fetch" => {
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

        "kb/node_update" => {
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

        "kb/collection_op" => {
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
        "kb/add_member" | "kb/remove_member" => {
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

        "kb/collection_node_add" | "kb/collection_node_remove" => {
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

        "kb/set_policy" => {
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

        "kb/block_principal" | "kb/unblock_principal" => {
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

        "kb/blocklist" => kb_governance::handle_kb_blocklist(doc_store, id, &params).await,

        "kb/set_governance" => {
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

        "kb/revoke" => {
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

        "kb/list_pending" => {
            kb_membership::handle_kb_list_pending(doc_store, auth_principal, transport, id, &params)
                .await
        }

        "kb/approve_member" => {
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

        "kb/leave" => {
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
mod tests;
