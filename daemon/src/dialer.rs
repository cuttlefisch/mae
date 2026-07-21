//! Outbound mesh dialer — the daemon as a sync CLIENT (ADR-025/026).
//!
//! Server-side, the daemon ACCEPTS mesh peers (`p2p::serve`). To JOIN a KB shared
//! by another daemon it must DIAL OUT: connect to the owner by node-id, verify the
//! connection's `remote_id()` against the ticket (anti-spoof — addresses are only
//! routing hints, the key is identity), register the owner key as the KB's external
//! trust ANCHOR (so `kb_access` derives membership from the signed op-log rather
//! than trusting the relay, ADR-026), pull the collection + nodes, then keep the
//! session LIVE: subscribe to the owner's pushes and apply them as they arrive,
//! reconnecting with bounded backoff on any drop (mobility, ADR-025).
//!
//! Full bidirectional live sync: the owner's edits stream to us (inbound apply) and
//! our local edits stream to the owner (outbound forward, via a doc-scoped local
//! broadcaster subscription), with reconnect/backoff for mobility.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use iroh::Endpoint;
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncWrite, BufReader};
use tokio::sync::Mutex;
use tracing::{info, warn};

use crate::handler::DaemonState;
use crate::p2p::MAE_ALPN;
use crate::ticket::JoinTicket;
use mae_daemon::doc_store::DocStore;
use mae_mcp::broadcast::{EditorEvent, SharedBroadcaster};
use mae_sync::encoding::{base64_to_update, update_to_base64};
use mae_sync::kb::KbCollectionDoc;

// Daemon-side dialer timings. These live in the daemon workspace (no editor
// OptionRegistry here); they are fixed operational constants, not user options.
// Kept as named consts with rationale per the no-magic-number principle (#7
// corollary). If a deployment ever needs these tunable, promote them to
// `daemon.toml`, not to hardcoded literals at the use site.
/// Timeout for establishing an outbound peer connection (TCP/QUIC dial).
/// Distinct from the editor's host-key *prompt* wait (`collab_host_key_prompt_timeout_secs`).
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// Per-message write timeout to a peer; a slow/stuck peer is dropped, not blocked on.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// How often the dialer re-scans for newly-authorized peers / pending shares to dial.
const POLL_INTERVAL: Duration = Duration::from_secs(10);
/// Reconnect backoff floor after a peer drop (grows toward RECONNECT_MAX).
const RECONNECT_MIN: Duration = Duration::from_secs(2);
/// Reconnect backoff ceiling — cap so a long-down peer is still retried ~1/min.
const RECONNECT_MAX: Duration = Duration::from_secs(60);

/// A broadcaster session-id for a dialer's LOCAL subscription, drawn from the top of
/// the u64 range so it never collides with a real editor/collab session id (those
/// count up from a low base). Each peer session gets a distinct id so its own
/// inbound applies (`broadcast_except(dialer_sid)`) don't echo back out to the owner.
fn next_dialer_sid() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(u64::MAX);
    NEXT.fetch_sub(1, Ordering::Relaxed)
}

/// What the initial pull of a peer session produced.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    /// The KB was pulled: the collection + `nodes` node docs are now local.
    Pulled { nodes: usize },
    /// The owner queued us for approval (invite policy); retry after approval.
    Pending,
    /// The owner explicitly REJECTED the join (not a member / not shared over P2p /
    /// role denied) — an authorization decision that retrying cannot change.
    Rejected(String),
}

/// Why a peer session ended — drives the dialer's reconnect policy ([`run_peer`]).
enum SessionEnd {
    /// Recoverable (network drop, owner offline/restarting, still-pending approval) —
    /// reconnect with bounded backoff. The default for an opaque error.
    Transient(String),
    /// The owner won't accept us (explicit reject). Stop retrying + surface — a
    /// reconnect loop can't fix an authorization decision (it just wastes the mesh).
    Terminal(String),
}

impl From<String> for SessionEnd {
    /// An opaque string error is transient by default (network/protocol); terminal
    /// rejects are constructed explicitly from a [`JoinOutcome::Rejected`].
    fn from(s: String) -> Self {
        SessionEnd::Transient(s)
    }
}

/// Background task (spawned with the mesh): for each accepted join ticket, spawn a
/// PERSISTENT peer session ([`run_peer`]) that pulls the KB and keeps it live-synced,
/// reconnecting on its own. One session per KB — the `active` set prevents
/// duplicates; a ticket is consumed when its session is spawned (the session owns
/// all retry/reconnect from there, including waiting out owner approval).
pub async fn run_dialer(
    state: Arc<Mutex<DaemonState>>,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
    endpoint: Endpoint,
) {
    let active: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let pending = {
            let mut st = state.lock().await;
            std::mem::take(&mut st.pending_p2p_joins)
        };
        for ticket in pending {
            // Skip if a live session for this KB already exists.
            if !active.lock().await.insert(ticket.kb_id.clone()) {
                continue;
            }
            tokio::spawn(run_peer(
                endpoint.clone(),
                ticket,
                Arc::clone(&doc_store),
                broadcaster.clone(),
            ));
        }
    }
}

/// Maintain a PERSISTENT, live-synced session with a KB owner: connect → verify →
/// anchor → pull (SV-reconcile) → subscribe → apply the owner's pushed updates,
/// reconnecting with **bounded exponential backoff** on any drop. Security is
/// re-established on every reconnect (re-handshake, re-verified `remote_id`,
/// re-anchored) — a network switch or an owner restart can never bypass it
/// (ADR-025 mobility). Runs for the process lifetime.
async fn run_peer(
    local: Endpoint,
    ticket: JoinTicket,
    doc_store: Arc<DocStore>,
    broadcaster: SharedBroadcaster,
) {
    let mut backoff = RECONNECT_MIN;
    loop {
        match peer_session(&local, &ticket, &doc_store, &broadcaster).await {
            Ok(()) => {
                info!(kb_id = %ticket.kb_id, "mesh peer session ended cleanly; reconnecting");
                backoff = RECONNECT_MIN;
            }
            Err(SessionEnd::Transient(e)) => {
                warn!(kb_id = %ticket.kb_id, error = %e, backoff_s = backoff.as_secs(),
                    "mesh peer session ended; backing off");
            }
            Err(SessionEnd::Terminal(e)) => {
                // Stop the loop: the owner won't accept us. Re-sharing / re-adding the
                // member re-queues a fresh ticket, which spawns a new session.
                warn!(kb_id = %ticket.kb_id, reason = %e,
                    "mesh peer: TERMINAL reject — not retrying (re-share or re-add to resume)");
                return;
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

/// One connection's lifetime: connect, verify, anchor, pull (reconcile), subscribe,
/// then apply the owner's inbound `sync_update` notifications until the link drops.
/// `Ok(())` = clean end (EOF), `Err` = failure / still-pending — both trigger a
/// reconnect in [`run_peer`].
async fn peer_session(
    local: &Endpoint,
    ticket: &JoinTicket,
    doc_store: &Arc<DocStore>,
    broadcaster: &SharedBroadcaster,
) -> Result<(), SessionEnd> {
    let dialer_sid = next_dialer_sid();
    let conn = connect_verify_anchor(local, ticket, doc_store).await?;
    let (mut send, recv) = conn.open_bi().await.map_err(|e| format!("open_bi: {e}"))?;
    let mut reader = BufReader::new(recv);

    // Initial sync: kb/join carrying our current node SVs ⇒ the owner replies with a
    // full snapshot (fresh join) or per-node diffs (reconnect), applied non-
    // destructively (apply_update merges; never clobbers local edits).
    match pull_kb(
        &mut send,
        &mut reader,
        doc_store,
        Some(broadcaster),
        dialer_sid,
        ticket,
    )
    .await?
    {
        JoinOutcome::Pulled { nodes } => {
            info!(kb_id = %ticket.kb_id, nodes, "mesh peer: synced KB")
        }
        // Pending approval is TRANSIENT — the owner may approve us later, so keep
        // reconnecting (backoff) and re-requesting.
        JoinOutcome::Pending => {
            return Err(SessionEnd::Transient("pending owner approval".to_string()))
        }
        // An explicit reject is TERMINAL — stop the reconnect loop.
        JoinOutcome::Rejected(why) => {
            return Err(SessionEnd::Terminal(format!("owner rejected join: {why}")))
        }
    }

    // No explicit notifications/subscribe is needed: the owner's kb/join handler
    // subscribed our session to `sync_update` (+ the KB's docs) AS OF the snapshot,
    // so its edits stream to us with no missed-edit window (ADR-026 2c-3c).

    // Subscribe to our LOCAL broadcaster, doc-scoped to this KB's docs, so our own
    // editor's edits (and any other local writer) are forwarded to the owner. Our
    // inbound applies use `broadcast_except(dialer_sid)`, so they are NOT delivered
    // back to this subscription — no echo loop.
    let node_docs = kb_node_docs(doc_store, &ticket.kb_id).await;
    let mut local_rx = {
        // No `.await` while holding the std Mutex guard (it isn't Send).
        let mut bc = broadcaster.lock().unwrap_or_else(|e| e.into_inner());
        let rx = bc.subscribe(dialer_sid, vec!["sync_update".to_string()]);
        bc.subscribe_doc(dialer_sid, &format!("kbc:{}", ticket.kb_id));
        for node in &node_docs {
            bc.subscribe_doc(dialer_sid, node);
        }
        rx
    };

    // Move the recv half into a reader task (read_message is not cancel-safe) so we
    // can `select!` inbound applies against outbound forwards.
    let (inbound_tx, mut inbound_rx) = tokio::sync::mpsc::channel::<Result<String, String>>(64);
    let reader_task = tokio::spawn(async move {
        loop {
            match mae_mcp::read_message(&mut reader).await {
                Ok(Some(m)) => {
                    if inbound_tx.send(Ok(m)).await.is_err() {
                        break;
                    }
                }
                Ok(None) => {
                    let _ = inbound_tx.send(Err("eof".into())).await;
                    break;
                }
                Err(e) => {
                    let _ = inbound_tx.send(Err(e.to_string())).await;
                    break;
                }
            }
        }
    });

    let result = loop {
        tokio::select! {
            // INBOUND: the owner's pushed updates → verify authorship (ADR-036), then
            // apply locally (merge). A forged/mis-attributed/unsigned op is rejected.
            inbound = inbound_rx.recv() => match inbound {
                Some(Ok(msg)) => {
                    if let Some((doc, update, header)) = parse_sync_update(&msg) {
                        // Mesh require-signed: the owner is an untrusted relay, so an
                        // unsigned/forged/mis-attributed op is rejected (shared check).
                        match mae_daemon::collab_handler::verify_relayed_content_op(
                            doc_store, &ticket.kb_id, &doc, &update, header.as_ref(), true,
                        ).await {
                            Ok(verified) => {
                                // #157 N1: the ADR-023 epoch fence runs on the mesh relay
                                // too — not just the hub `kb/node_update` — so a stale-epoch
                                // op can't slip in via the peer path (complete mediation).
                                // The op's author (from the verified signed header) is the
                                // principal whose current-epoch client_id must match.
                                let fence_reject = match (
                                    verified.as_ref().and_then(|h| h.get("author").and_then(|a| a.as_str())),
                                    doc.strip_prefix("kb:"),
                                ) {
                                    (Some(author), Some(node_id)) => {
                                        mae_daemon::collab_handler::enforce_epoch_fence(
                                            doc_store, &ticket.kb_id, node_id, &doc, &update, author,
                                        ).await.err()
                                    }
                                    _ => None, // unsigned/legacy/collection doc — no author to fence
                                };
                                match fence_reject {
                                    Some(reason) => warn!(kb_id = %ticket.kb_id, doc = %doc, %reason, "mesh: REJECTED relayed op (epoch fence — #157 N1)"),
                                    None => apply_doc(doc_store, Some(broadcaster), dialer_sid, &doc, &update, verified).await,
                                }
                            }
                            Err(e) => warn!(kb_id = %ticket.kb_id, doc = %doc, reason = %e, "mesh: REJECTED relayed content op (ADR-036)"),
                        }
                    }
                }
                Some(Err(e)) if e == "eof" => break Ok(()),
                Some(Err(e)) => break Err(format!("read: {e}")),
                None => break Ok(()),
            },
            // OUTBOUND: a local edit to one of this KB's docs → forward to the owner,
            // carrying the signed authorship header so the owner re-verifies it.
            Some(event) = local_rx.recv() => {
                if let EditorEvent::SyncUpdate { buffer_name, update_base64, content_header, .. } = event {
                    // Include kb_id (this session's KB) so the OWNER can resolve the
                    // KB's membership to re-verify our relayed op (ADR-036 §D3, B→A) —
                    // sync/update otherwise carries only the bare doc name.
                    let mut params = json!({
                        "doc": buffer_name, "update": update_base64, "kb_id": ticket.kb_id,
                    });
                    if let Some(h) = content_header {
                        params["content_header"] = h;
                    }
                    let req = json!({
                        "jsonrpc": "2.0", "id": 3, "method": "sync/update", "params": params
                    }).to_string();
                    if let Err(e) = mae_mcp::write_framed(&mut send, req.as_bytes(), WRITE_TIMEOUT).await {
                        break Err(format!("forward local edit: {e}"));
                    }
                }
            }
        }
    };

    // Clean up the local subscription + reader task before reconnecting.
    broadcaster
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .unsubscribe(dialer_sid);
    reader_task.abort();
    // Read/forward/EOF failures are all network/protocol — transient, so reconnect.
    result.map_err(SessionEnd::Transient)
}

/// The `kb:{node}` doc names currently in a KB's collection (for doc-scoped local
/// subscription). Empty if the collection isn't present yet.
async fn kb_node_docs(doc_store: &Arc<DocStore>, kb_id: &str) -> Vec<String> {
    let coll_doc = format!("kbc:{kb_id}");
    let Ok((state, _)) = doc_store.encode_state_and_sv(&coll_doc).await else {
        return Vec::new();
    };
    let Ok(coll) = KbCollectionDoc::from_bytes(&state) else {
        return Vec::new();
    };
    coll.list_nodes()
        .into_iter()
        .map(|(id, _)| format!("kb:{id}"))
        .collect()
}

/// Dial the ticket's owner, verify identity (anti-spoof), and register the trust
/// anchor. Shared by the live session and the one-shot [`dial_and_join`].
async fn connect_verify_anchor(
    local: &Endpoint,
    ticket: &JoinTicket,
    doc_store: &Arc<DocStore>,
) -> Result<iroh::endpoint::Connection, String> {
    let expected = ticket.node_id();
    let conn = tokio::time::timeout(
        DIAL_TIMEOUT,
        local.connect(ticket.endpoint.clone(), MAE_ALPN),
    )
    .await
    .map_err(|_| "dial timed out".to_string())?
    .map_err(|e| format!("dial failed: {e}"))?;

    // Identity-addressed: connecting by EndpointAddr already targets the node-id;
    // assert the handshake-proven `remote_id()` matches (a spoofed/tampered address
    // can waste a dial but never impersonate the owner — ADR-025).
    let remote = conn.remote_id();
    if remote != expected {
        conn.close(1u32.into(), b"identity mismatch");
        return Err(format!(
            "remote identity {remote} != ticket node-id {expected} (spoofed address?)"
        ));
    }
    // The owner's node-id is the external trust anchor the op-log genesis must be
    // signed by (ADR-026). This flips `kb_access` to the derived, peer-verifiable path.
    doc_store
        .set_kb_anchor(&ticket.kb_id, *remote.as_bytes())
        .await;
    Ok(conn)
}

/// One-shot: dial, verify, anchor, pull the KB, then close. The live path is
/// [`run_peer`] (persistent + reconnecting); this snapshot-only variant backs the
/// dial/verify/anchor/pull tests and is available for a future `kb-pull`-style use.
/// NOT stale despite the mae-audit finding that flagged it: its only current
/// callers are `#[cfg(test)]`, by design (see doc above) — a non-test build
/// genuinely has zero callers, so this allow is still required. Verified via
/// `cargo build` (not `cargo test`): removing it produces a real
/// `dead_code` warning, unlike the 3 other markers this same audit pass
/// found actually stale (`KbOp::Read`, `open_memory`, `wal_entries_since` —
/// all reachable from non-test code too).
#[allow(dead_code)]
pub async fn dial_and_join(
    local: &Endpoint,
    ticket: &JoinTicket,
    doc_store: &Arc<DocStore>,
) -> Result<JoinOutcome, String> {
    let conn = connect_verify_anchor(local, ticket, doc_store).await?;
    let (mut send, recv) = conn.open_bi().await.map_err(|e| format!("open_bi: {e}"))?;
    let mut reader = BufReader::new(recv);
    let outcome = pull_kb(&mut send, &mut reader, doc_store, None, 0, ticket).await?;
    drop(send);
    conn.close(0u32.into(), b"bye");
    Ok(outcome)
}

/// Send `kb/join` (carrying our current node SVs for reconcile) and apply the reply.
/// We are not subscribed yet, so the reply is the first — and only — message on the
/// stream before we return.
#[allow(clippy::too_many_arguments)]
async fn pull_kb<W, R>(
    send: &mut W,
    reader: &mut R,
    doc_store: &Arc<DocStore>,
    broadcaster: Option<&SharedBroadcaster>,
    exclude_sid: u64,
    ticket: &JoinTicket,
) -> Result<JoinOutcome, String>
where
    W: AsyncWrite + Unpin,
    R: AsyncBufRead + Unpin,
{
    let node_svs = node_svs_for(doc_store, &ticket.kb_id).await;
    // #255: forward any LOCAL pending join requests (editor members waiting behind this relay),
    // carrying each member's published wrap pubkey, so the owner can wrap the E2E content key to
    // them on approval. Without this a member joining over the mesh is admitted keyless (the owner
    // never receives their wrap key). Secure in the two-daemon model: the owner trusts this
    // authorized peer, which authenticated the member's editor session; this daemon stays key-blind
    // (it relays ciphertext + the owner-authored wrap, never the content key).
    let pending_members = pending_members_for(doc_store, &ticket.kb_id).await;
    let req = json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/join",
        "params": { "kb_id": ticket.kb_id, "node_svs": node_svs, "pending_members": pending_members }
    })
    .to_string();
    mae_mcp::write_framed(send, req.as_bytes(), WRITE_TIMEOUT)
        .await
        .map_err(|e| format!("write kb/join: {e}"))?;
    // Do NOT finish() the send stream — that EOFs the owner's long-lived session
    // before it flushes the reply. Content-Length framing delimits the request.
    let resp = mae_mcp::read_message(reader)
        .await
        .map_err(|e| format!("read kb/join: {e}"))?
        .ok_or_else(|| "owner closed without responding".to_string())?;
    apply_join_response(&resp, &ticket.kb_id, doc_store, broadcaster, exclude_sid).await
}

/// Our per-node state vectors for the KB (so the owner replies with diffs, not full
/// state, on reconnect). Empty on a fresh join (no local docs ⇒ full snapshot).
async fn node_svs_for(doc_store: &Arc<DocStore>, kb_id: &str) -> Vec<Value> {
    let coll_doc = format!("kbc:{kb_id}");
    if !doc_store.has_doc(&coll_doc).await {
        return Vec::new();
    }
    let Ok((state, _)) = doc_store.encode_state_and_sv(&coll_doc).await else {
        return Vec::new();
    };
    let Ok(coll) = KbCollectionDoc::from_bytes(&state) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (node_id, _title) in coll.list_nodes() {
        if let Ok(sv) = doc_store.state_vector(&format!("kb:{node_id}")).await {
            out.push(json!({ "id": node_id, "sv": update_to_base64(&sv) }));
        }
    }
    out
}

/// #255: local pending join requests to FORWARD to the owner over the mesh — each editor member
/// waiting behind this relay, with the wrap pubkey it published to us. Only requests carrying a
/// wrap key are forwarded (an E2e member the owner must be able to seal the content key to); a
/// keyless request has nothing to deliver. The owner records them as pending + wraps on approval.
async fn pending_members_for(doc_store: &Arc<DocStore>, kb_id: &str) -> Vec<Value> {
    let coll_doc = format!("kbc:{kb_id}");
    let Ok((state, _)) = doc_store.encode_state_and_sv(&coll_doc).await else {
        return Vec::new();
    };
    let Ok(coll) = KbCollectionDoc::from_bytes(&state) else {
        return Vec::new();
    };
    coll.pending()
        .into_iter()
        .filter_map(|p| {
            let wrap = p.wrap_pubkey?;
            Some(json!({
                "fp": p.fingerprint,
                "label": p.label,
                "pubkey": p.pubkey.map(hex::encode),
                "wrap_pubkey": hex::encode(wrap),
            }))
        })
        .collect()
}

/// Parse a `kb/join` response and apply the collection + node docs locally
/// (non-destructive merge). Full `state` (fresh) or incremental `diff` (reconcile)
/// both merge via `apply_update`.
async fn apply_join_response(
    resp: &str,
    kb_id: &str,
    doc_store: &Arc<DocStore>,
    broadcaster: Option<&SharedBroadcaster>,
    exclude_sid: u64,
) -> Result<JoinOutcome, String> {
    let v: Value = serde_json::from_str(resp).map_err(|e| format!("bad response json: {e}"))?;
    if let Some(err) = v.get("error") {
        // The owner explicitly rejected us — terminal, not a transient network error.
        return Ok(JoinOutcome::Rejected(format!("{err}")));
    }
    let result = v.get("result").ok_or("response has no result")?;
    if result.get("status").and_then(|s| s.as_str()) == Some("pending") {
        return Ok(JoinOutcome::Pending);
    }

    let coll_b64 = result
        .get("collection_state")
        .and_then(|s| s.as_str())
        .ok_or("response missing collection_state")?;
    let coll_bytes =
        base64_to_update(coll_b64).map_err(|e| format!("bad collection_state: {e}"))?;
    // The join snapshot is the owner's authoritative state (not an individual signed
    // content op), so it carries no authorship header.
    apply_doc(
        doc_store,
        broadcaster,
        exclude_sid,
        &format!("kbc:{kb_id}"),
        &coll_bytes,
        None,
    )
    .await;

    let mut nodes = 0usize;
    if let Some(arr) = result.get("nodes").and_then(|n| n.as_array()) {
        for node in arr {
            let Some(id) = node.get("id").and_then(|s| s.as_str()) else {
                continue;
            };
            // Full state on a fresh join; an incremental diff on reconnect.
            let Some(b64) = node
                .get("state")
                .or_else(|| node.get("diff"))
                .and_then(|s| s.as_str())
            else {
                continue;
            };
            let Ok(update) = base64_to_update(b64) else {
                warn!(node = %id, "mesh join: undecodable node payload");
                continue;
            };
            apply_doc(
                doc_store,
                broadcaster,
                exclude_sid,
                &format!("kb:{id}"),
                &update,
                None,
            )
            .await;
            nodes += 1;
        }
    }
    Ok(JoinOutcome::Pulled { nodes })
}

/// Apply a remote update to a local doc (non-destructive merge) and, when a
/// broadcaster is present, notify local subscribers (e.g. this peer's editor) so the
/// change surfaces live.
async fn apply_doc(
    doc_store: &Arc<DocStore>,
    broadcaster: Option<&SharedBroadcaster>,
    exclude_sid: u64,
    doc_name: &str,
    update: &[u8],
    content_header: Option<Value>,
) {
    match doc_store.apply_update(doc_name, update, None).await {
        Ok(result) => {
            if let Some(bc) = broadcaster {
                // `broadcast_except(exclude_sid)` so the dialer's own local
                // subscription (= the outbound-forward path) does NOT receive what we
                // just applied from the owner — that would echo it straight back.
                bc.lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .broadcast_except(
                        &EditorEvent::SyncUpdate {
                            buffer_name: doc_name.to_string(),
                            update_base64: update_to_base64(update),
                            wal_seq: result.wal_seq,
                            // Carry the verified header onward (local subscribers + a
                            // further mesh hop re-verify the same op, ADR-036).
                            content_header,
                        },
                        exclude_sid,
                    );
            }
        }
        Err(e) => warn!(doc = %doc_name, error = %e, "mesh: failed to apply remote update"),
    }
}

/// Extract `(doc_name, update_bytes, content_header)` from an inbound
/// `notifications/sync_update`. `EditorEvent` is `#[serde(tag = "type", content =
/// "data")]`, so the payload is `params.event.data.{buffer_name, update_base64,
/// content_header?}`. `content_header` (ADR-036) is absent for legacy/unsigned ops.
fn parse_sync_update(msg: &str) -> Option<(String, Vec<u8>, Option<Value>)> {
    let v: Value = serde_json::from_str(msg).ok()?;
    if v.get("method")?.as_str()? != "notifications/sync_update" {
        return None;
    }
    let data = v.get("params")?.get("event")?.get("data")?;
    let doc = data.get("buffer_name")?.as_str()?.to_string();
    let update = base64_to_update(data.get("update_base64")?.as_str()?).ok()?;
    let header = data.get("content_header").cloned();
    Some((doc, update, header))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::p2p::{bind_endpoint, loopback_if_unspecified, serve};
    use iroh::{EndpointAddr, RelayMode, TransportAddr};
    use mae_daemon::doc_store::DocStore;
    use mae_mcp::broadcast::{EventBroadcaster, SharedBroadcaster};
    use mae_mcp::identity::Identity;
    use mae_sync::kb::{KbCollectionDoc, KbNodeDoc, Role as SyncRole, TransportPolicy};
    use mae_sync::membership::MembershipAction;
    use mae_sync::text::TextSync;
    use std::time::Instant;

    fn mem_store() -> Arc<DocStore> {
        let backend = Arc::new(crate::storage::SqliteBackend::open_memory().unwrap());
        Arc::new(DocStore::new(backend, 500))
    }

    fn bc() -> SharedBroadcaster {
        Arc::new(std::sync::Mutex::new(EventBroadcaster::new()))
    }

    /// Spin up owner A serving `kb_id` over the mesh with one node; returns A's
    /// dialable address + the owner store/broadcaster (so a test can edit live).
    /// `member` Some ⇒ that principal is an editor (a join PULLS); None ⇒ Invite
    /// policy (a join goes PENDING).
    async fn serve_owner_kb(
        a_id: &Arc<Identity>,
        kb_id: &str,
        member: Option<&str>,
    ) -> (EndpointAddr, Arc<DocStore>, SharedBroadcaster) {
        let a_store = mem_store();
        a_store.set_signer(Arc::clone(a_id));
        let a_bc = bc();
        let owner_fp = a_id.fingerprint();

        let mut coll = KbCollectionDoc::new_owned(kb_id, &owner_fp, "A");
        coll.add_node("concept:n", "Node N");
        coll.set_transport_policy(TransportPolicy::P2p);
        if let Some(m) = member {
            coll.upsert_member(m, "member", SyncRole::Editor);
        }
        // Sign the genesis + (if any) member admit so a joiner can peer-verify.
        let secret = a_id.secret_bytes();
        let pubkey = a_id.public().to_bytes();
        let g = coll.build_membership_op(
            kb_id,
            MembershipAction::Admit,
            &owner_fp,
            Some(SyncRole::Owner),
            true,
            &owner_fp,
            0,
            None,
            0,
        );
        let gsig = g.sign(&secret);
        coll.append_signed_op(&g, &gsig, &pubkey);
        if let Some(m) = member {
            let admit = coll.build_membership_op(
                kb_id,
                MembershipAction::Admit,
                m,
                Some(SyncRole::Editor),
                false,
                &owner_fp,
                0,
                None,
                coll.epoch_of(m),
            );
            let asig = admit.sign(&secret);
            coll.append_signed_op(&admit, &asig, &pubkey);
        }
        a_store
            .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state())
            .await
            .unwrap();
        // The node doc is a plain text CRDT (the daemon stores yrs docs schema-
        // agnostically), so a test can produce a real edit update with TextSync.
        let node = KbNodeDoc::new("concept:n", "Node N", "hello", &[]);
        a_store
            .share_doc("kb:concept:n", &node.encode())
            .await
            .unwrap();

        let ep = bind_endpoint(a_id, RelayMode::Disabled).await.unwrap();
        let addr = EndpointAddr::from_parts(
            ep.id(),
            ep.bound_sockets()
                .into_iter()
                .map(|sa| TransportAddr::Ip(loopback_if_unspecified(sa))),
        );
        tokio::spawn(serve(
            ep,
            std::path::PathBuf::from("/nonexistent/authorized_keys"),
            true,
            Arc::clone(&a_store),
            a_bc.clone(),
            Instant::now(),
        ));
        (addr, a_store, a_bc)
    }

    /// Poll `cond` until true or `secs` elapse (real two-endpoint timing).
    async fn wait_until<F, Fut>(secs: u64, mut cond: F) -> bool
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = bool>,
    {
        for _ in 0..(secs * 50) {
            if cond().await {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// Sign a content op as `id` — the editor's sign-on-push (ADR-036 §D2),
    /// reproduced at the test layer so the live-mesh tests exercise real signatures.
    fn sign_content_op(
        id: &Identity,
        kb_id: &str,
        node_id: &str,
        epoch: u64,
        payload: &[u8],
    ) -> mae_sync::content_ops::SignedContentOp {
        let op = mae_sync::content_ops::ContentOp {
            kb_id: kb_id.to_string(),
            node_id: node_id.to_string(),
            base_sv: vec![],
            author: id.fingerprint(),
            epoch,
            issued_at: 0,
        };
        let sig = op.sign(&id.secret_bytes(), payload);
        mae_sync::content_ops::SignedContentOp {
            op,
            payload: payload.to_vec(),
            sig,
            author_pubkey: id.public().to_bytes(),
        }
    }

    #[tokio::test]
    async fn dial_and_join_pulls_and_peer_verifies() {
        let a_id = Arc::new(Identity::generate("owner-A"));
        let b_id = Identity::generate("joiner-B");
        let (addr, _a_store, _a_bc) = serve_owner_kb(&a_id, "kbx", Some(&b_id.fingerprint())).await;

        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let ticket = JoinTicket::new(addr, "kbx");

        let outcome = dial_and_join(&b_endpoint, &ticket, &b_store)
            .await
            .expect("dial + join");
        assert_eq!(outcome, JoinOutcome::Pulled { nodes: 1 });
        assert!(b_store.has_doc("kbc:kbx").await);
        assert!(b_store.has_doc("kb:concept:n").await);
        assert_eq!(
            b_store.kb_anchor("kbx").await,
            Some(a_id.public().to_bytes())
        );

        // B peer-verifies membership from the pulled signed op-log (no relay trust).
        let (state, _) = b_store.encode_state_and_sv("kbc:kbx").await.unwrap();
        let pulled = KbCollectionDoc::from_bytes(&state).unwrap();
        let members = mae_sync::membership::derive_valid_members(
            &pulled.oplog_ops(),
            &a_id.public().to_bytes(),
            0,
        );
        assert_eq!(
            members.get(&a_id.fingerprint()).map(|m| m.role),
            Some(SyncRole::Owner)
        );
    }

    #[tokio::test]
    async fn dial_rejects_node_id_mismatch() {
        let a_id = Arc::new(Identity::generate("owner-A"));
        let (_addr, _s, _bc) = serve_owner_kb(&a_id, "kbx", None).await;
        // A ticket pointing at A's address but claiming a DIFFERENT node-id.
        let a_sockets = {
            let ep = bind_endpoint(&Identity::generate("probe"), RelayMode::Disabled)
                .await
                .unwrap();
            ep.bound_sockets()
                .into_iter()
                .map(|sa| TransportAddr::Ip(loopback_if_unspecified(sa)))
                .collect::<Vec<_>>()
        };
        let imposter =
            iroh::SecretKey::from(Identity::generate("imposter").secret_bytes()).public();
        let ticket = JoinTicket::new(EndpointAddr::from_parts(imposter, a_sockets), "kbx");

        let b_id = Identity::generate("joiner-B");
        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let res = tokio::time::timeout(
            Duration::from_secs(8),
            dial_and_join(&b_endpoint, &ticket, &b_store),
        )
        .await;
        assert!(matches!(res, Ok(Err(_)) | Err(_)));
        assert!(b_store.kb_anchor("kbx").await.is_none());
    }

    #[tokio::test]
    async fn live_session_applies_owner_edits() {
        let a_id = Arc::new(Identity::generate("owner-A"));
        let b_id = Identity::generate("joiner-B");
        let (addr, a_store, a_bc) = serve_owner_kb(&a_id, "kbl", Some(&b_id.fingerprint())).await;

        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let b_bc = bc();
        let ticket = JoinTicket::new(addr, "kbl");

        // Spawn the persistent live session.
        let peer = tokio::spawn(run_peer(
            b_endpoint,
            ticket,
            Arc::clone(&b_store),
            b_bc.clone(),
        ));

        // B pulls the KB (initial sync) — wait for the node doc to land.
        assert!(
            wait_until(5, || {
                let s = Arc::clone(&b_store);
                async move { s.has_doc("kb:concept:n").await }
            })
            .await,
            "B should pull the node on join"
        );
        // No settle needed: kb/join subscribed B to sync_update AS OF the snapshot
        // (2c-3c), so an owner edit right after the pull is still pushed.
        let before = b_store.state_vector("kb:concept:n").await.unwrap();

        // A edits the node AFTER B subscribed, and broadcasts it the way the server's
        // node-update path does → A's session for B pushes a sync_update notification.
        let mut a_node = {
            let (st, _) = a_store.encode_state_and_sv("kb:concept:n").await.unwrap();
            // A real editor authors KB node ops under derive_kb_client_id(fp, epoch)
            // (crates/core/src/editor/kb_ops/nodes.rs:240). The mesh epoch fence (#157 N1) binds the relayed
            // payload's yrs client_id to the author's CURRENT epoch, so the test
            // must stamp the edit the way the editor would — a plain daemon-store
            // client_id would (correctly) be fenced as stale-lineage.
            TextSync::from_state_with_client_id(
                &st,
                mae_sync::kb::derive_kb_client_id(&a_id.fingerprint(), 0),
            )
            .unwrap()
        };
        let edit = a_node.insert(0, "LIVE-EDIT ");
        a_store
            .apply_update("kb:concept:n", &edit, None)
            .await
            .unwrap();
        // A (the owner = a member) SIGNS the edit (ADR-036): the broadcast carries the
        // authorship header, so B verifies it against the derived op-log membership
        // (owner ⊇ editor at epoch 0) before applying — this is the relay-can't-forge
        // guarantee end-to-end on a real 2-endpoint mesh.
        let signed = sign_content_op(&a_id, "kbl", "concept:n", 0, &edit);
        a_bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
            buffer_name: "kb:concept:n".to_string(),
            update_base64: update_to_base64(&edit),
            wal_seq: 0,
            content_header: Some(signed.header_params()),
        });

        // B receives + applies it live: its node state vector advances.
        let landed = wait_until(5, || {
            let s = Arc::clone(&b_store);
            let before = before.clone();
            async move {
                s.state_vector("kb:concept:n")
                    .await
                    .map(|sv| sv != before)
                    .unwrap_or(false)
            }
        })
        .await;
        peer.abort();
        assert!(landed, "B should apply the owner's live edit");
    }

    /// ADR-036 mesh verification, the SELECTIVE oracle: B rejects three distinct bad
    /// relayed edits (unsigned → require-signed; non-member-signed → NotAMember; real
    /// member at a stale epoch → StaleEpoch) AND still applies a valid owner edit on
    /// the same live session. The trailing positive case is the point: a bare
    /// assert-the-SV-didn't-move test would pass just as well if the session had
    /// quietly died — proving rejection is the verifier working, not a dead pipe.
    #[tokio::test]
    async fn live_session_mesh_verification_is_selective() {
        let a_id = Arc::new(Identity::generate("owner-A"));
        let b_id = Identity::generate("joiner-B");
        let (addr, a_store, a_bc) = serve_owner_kb(&a_id, "kbl", Some(&b_id.fingerprint())).await;

        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let b_bc = bc();
        let ticket = JoinTicket::new(addr, "kbl");
        let peer = tokio::spawn(run_peer(
            b_endpoint,
            ticket,
            Arc::clone(&b_store),
            b_bc.clone(),
        ));
        assert!(
            wait_until(5, || {
                let s = Arc::clone(&b_store);
                async move { s.has_doc("kb:concept:n").await }
            })
            .await,
            "B should pull the node on join"
        );
        let before = b_store.state_vector("kb:concept:n").await.unwrap();

        // A genuine edit A makes locally (so the yrs bytes are valid) — but relayed
        // two illegitimate ways.
        let mut a_node = {
            let (st, _) = a_store.encode_state_and_sv("kb:concept:n").await.unwrap();
            TextSync::from_state(&st).unwrap()
        };
        let edit = a_node.insert(0, "FORGED ");

        let push = |bc: &SharedBroadcaster, upd: &[u8], header: Option<serde_json::Value>| {
            bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
                buffer_name: "kb:concept:n".to_string(),
                update_base64: update_to_base64(upd),
                wal_seq: 0,
                content_header: header,
            });
        };
        let sv_advanced = |b_store: &Arc<DocStore>, before: &[u8]| {
            let s = Arc::clone(b_store);
            let before = before.to_vec();
            async move {
                s.state_vector("kb:concept:n")
                    .await
                    .map(|sv| sv != before)
                    .unwrap_or(false)
            }
        };

        // Three DISTINCT rejection paths, each exercising a different `admit` failure:
        // (1) UNSIGNED → mesh require-signed; (2) signed by a NON-MEMBER stranger →
        // NotAMember; (3) signed by a REAL MEMBER (B) but at a STALE epoch (its grant
        // is epoch 0) → StaleEpoch. None must apply.
        push(&a_bc, &edit, None);
        let stranger = Identity::generate("stranger");
        push(
            &a_bc,
            &edit,
            Some(sign_content_op(&stranger, "kbl", "concept:n", 0, &edit).header_params()),
        );
        push(
            &a_bc,
            &edit,
            Some(sign_content_op(&b_id, "kbl", "concept:n", 999, &edit).header_params()),
        );

        let advanced = wait_until(2, || sv_advanced(&b_store, &before)).await;
        assert!(
            !advanced,
            "B must reject the unsigned + non-member + stale-epoch relayed edits"
        );

        // SELECTIVE, not a dead session: a VALID owner-signed edit on the SAME live
        // session IS applied — proving the rejections above were the verifier doing
        // its job, not the connection having quietly died (the trap a bare
        // assert-absence test would mask).
        let valid_edit = {
            let (st, _) = a_store.encode_state_and_sv("kb:concept:n").await.unwrap();
            // Stamp the way a real editor does (derive_kb_client_id,
            // crates/core/src/editor/kb_ops/nodes.rs:240)
            // so the valid owner edit clears the #157 N1 mesh epoch fence — the
            // SELECTIVE half of the oracle (the three rejects above never reach the
            // fence; they're stopped earlier by verify_relayed_content_op).
            let mut node = TextSync::from_state_with_client_id(
                &st,
                mae_sync::kb::derive_kb_client_id(&a_id.fingerprint(), 0),
            )
            .unwrap();
            node.insert(0, "VALID ")
        };
        push(
            &a_bc,
            &valid_edit,
            Some(sign_content_op(&a_id, "kbl", "concept:n", 0, &valid_edit).header_params()),
        );
        let applied = wait_until(5, || sv_advanced(&b_store, &before)).await;
        peer.abort();
        assert!(
            applied,
            "B must APPLY a valid owner edit on the same session (rejection is selective, session alive)"
        );
    }

    #[tokio::test]
    async fn live_session_forwards_local_edits_to_owner() {
        let a_id = Arc::new(Identity::generate("owner-A"));
        let b_id = Identity::generate("joiner-B");
        let (addr, a_store, _a_bc) = serve_owner_kb(&a_id, "kbo", Some(&b_id.fingerprint())).await;

        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let b_bc = bc();
        let ticket = JoinTicket::new(addr, "kbo");

        let peer = tokio::spawn(run_peer(
            b_endpoint,
            ticket,
            Arc::clone(&b_store),
            b_bc.clone(),
        ));

        // Wait for B to pull, then for its subscribe + local subscription to settle.
        assert!(
            wait_until(5, || {
                let s = Arc::clone(&b_store);
                async move { s.has_doc("kb:concept:n").await }
            })
            .await,
            "B should pull on join"
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        let a_before = a_store.state_vector("kb:concept:n").await.unwrap();

        // B's "editor" edits the node locally and broadcasts it on B's broadcaster
        // (the dialer is subscribed, doc-scoped to this KB) → forwarded to the owner.
        let mut b_node = {
            let (st, _) = b_store.encode_state_and_sv("kb:concept:n").await.unwrap();
            // A real editor authors KB node ops under derive_kb_client_id(fp, epoch)
            // (crates/core/src/editor/kb_ops/nodes.rs:240). The B→A forward now clears the epoch fence on the owner's
            // sync/update path too (#169 M1), so B's edit must be stamped the way the editor
            // would — a plain client_id is (correctly) fenced as stale-lineage.
            TextSync::from_state_with_client_id(
                &st,
                mae_sync::kb::derive_kb_client_id(&b_id.fingerprint(), 0),
            )
            .unwrap()
        };
        let edit = b_node.insert(0, "B-EDIT ");
        b_store
            .apply_update("kb:concept:n", &edit, None)
            .await
            .unwrap();
        let push_b = |header: Option<serde_json::Value>| {
            b_bc.lock().unwrap().broadcast(&EditorEvent::SyncUpdate {
                buffer_name: "kb:concept:n".to_string(),
                update_base64: update_to_base64(&edit),
                wal_seq: 0,
                content_header: header,
            });
        };
        let a_changed = |a_store: &Arc<DocStore>, a_before: &[u8]| {
            let s = Arc::clone(a_store);
            let before = a_before.to_vec();
            async move {
                s.state_vector("kb:concept:n")
                    .await
                    .map(|sv| sv != before)
                    .unwrap_or(false)
            }
        };

        // B→A direction (ADR-036 §D3): the owner re-verifies the joiner's relayed op,
        // because B's *daemon* is an untrusted relay. (bad) an UNSIGNED edit and one
        // forged-signed by a NON-MEMBER are rejected by the OWNER ...
        push_b(None);
        let stranger = Identity::generate("stranger");
        push_b(Some(
            sign_content_op(&stranger, "kbo", "concept:n", 0, &edit).header_params(),
        ));
        assert!(
            !wait_until(2, || a_changed(&a_store, &a_before)).await,
            "owner must reject B's unsigned + non-member-forged relayed edits"
        );

        // ... (valid) B (a real member) signs its OWN edit → the owner verifies it
        // against the KB membership and applies it (selective: bad rejected, good
        // applied on the same live session).
        push_b(Some(
            sign_content_op(&b_id, "kbo", "concept:n", 0, &edit).header_params(),
        ));
        let landed = wait_until(5, || a_changed(&a_store, &a_before)).await;
        peer.abort();
        assert!(
            landed,
            "owner must apply B's VALID signed edit (B→A relay verified, selective)"
        );
    }

    /// `apply_join_response` classifies the owner's reply: an explicit error is a
    /// TERMINAL `Rejected` (carrying the reason), a pending status is `Pending`
    /// (transient). This is the seam that decides whether the dialer keeps retrying.
    #[tokio::test]
    async fn apply_join_response_classifies_reject_vs_pending() {
        let store = mem_store();
        let reject = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"not a member of KB 'k'"}}"#;
        match apply_join_response(reject, "k", &store, None, 7).await {
            Ok(JoinOutcome::Rejected(why)) => {
                assert!(
                    why.contains("not a member"),
                    "reject carries the reason: {why}"
                )
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
        let pending = r#"{"jsonrpc":"2.0","id":1,"result":{"status":"pending"}}"#;
        assert_eq!(
            apply_join_response(pending, "k", &store, None, 7)
                .await
                .unwrap(),
            JoinOutcome::Pending
        );
    }

    /// The reliability fix: on a TERMINAL reject (here, dialing a KB the owner does
    /// not host → an explicit error), `run_peer` STOPS — the task returns instead of
    /// retrying forever. A transient failure would keep the loop alive past the
    /// timeout; this asserts the loop actually ends.
    #[tokio::test]
    async fn run_peer_stops_on_terminal_reject() {
        let a_id = Arc::new(Identity::generate("owner-A"));
        let b_id = Identity::generate("joiner-B");
        let (addr, _a_store, _a_bc) = serve_owner_kb(&a_id, "kbl", Some(&b_id.fingerprint())).await;

        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let b_bc = bc();
        // Dial a KB the owner doesn't host ⇒ the owner rejects ⇒ terminal.
        let ticket = JoinTicket::new(addr, "ghost-kb");
        let peer = tokio::spawn(run_peer(b_endpoint, ticket, Arc::clone(&b_store), b_bc));

        let stopped = tokio::time::timeout(Duration::from_secs(8), peer).await;
        assert!(
            stopped.is_ok(),
            "run_peer must STOP on a terminal reject, not retry forever"
        );
    }
}
