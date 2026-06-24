//! Outbound mesh dialer — the daemon as a sync CLIENT (ADR-025/026).
//!
//! Server-side, the daemon ACCEPTS mesh peers (`p2p::serve`). To JOIN a KB shared
//! by another daemon it must DIAL OUT: connect to the owner by node-id, verify the
//! connection's `remote_id()` against the ticket (anti-spoof — addresses are only
//! routing hints, the key is identity), register the owner key as the KB's external
//! trust ANCHOR (so `kb_access` derives membership from the signed op-log rather
//! than trusting the relay, ADR-026), then pull the collection + nodes over the same
//! `kb/join` protocol the editor speaks.
//!
//! This slice (2c) does the dial + verify + anchor + one-shot full-state pull, plus
//! the background drain of `pending_p2p_joins` with retry. Ongoing bidirectional
//! sync + persistent per-peer connections (live updates) layer on next.

use std::sync::Arc;
use std::time::Duration;

use iroh::Endpoint;
use tokio::io::BufReader;
use tracing::{info, warn};

use tokio::sync::Mutex;

use crate::handler::DaemonState;
use crate::p2p::MAE_ALPN;
use crate::ticket::JoinTicket;
use mae_daemon::doc_store::DocStore;
use mae_sync::encoding::base64_to_update;

const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const POLL_INTERVAL: Duration = Duration::from_secs(10);

/// Background task (spawned with the mesh): drain queued join tickets (from
/// `p2p/join_ticket`) and pull each KB. A ticket still PENDING owner approval, or
/// whose dial failed (owner offline / unreachable), is re-queued for the next poll,
/// so a join eventually completes once the owner approves + comes online. Ongoing
/// bidirectional sync + persistent per-peer connections layer on in a later slice.
pub async fn run_dialer(
    state: Arc<Mutex<DaemonState>>,
    doc_store: Arc<DocStore>,
    endpoint: Endpoint,
) {
    loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        drain_pending_once(&state, &doc_store, &endpoint).await;
    }
}

/// One drain pass: dial every queued ticket once; re-queue the ones not yet pulled
/// (still pending approval, or a transient dial failure). Returns `(pulled,
/// requeued)` for observability + tests. New tickets enqueued concurrently are
/// preserved (we re-`lock` to append, never overwrite).
async fn drain_pending_once(
    state: &Arc<Mutex<DaemonState>>,
    doc_store: &Arc<DocStore>,
    endpoint: &Endpoint,
) -> (usize, usize) {
    let pending = {
        let mut st = state.lock().await;
        std::mem::take(&mut st.pending_p2p_joins)
    };
    if pending.is_empty() {
        return (0, 0);
    }
    let mut pulled = 0usize;
    let mut requeue = Vec::new();
    for ticket in pending {
        match dial_and_join(endpoint, &ticket, doc_store).await {
            Ok(JoinOutcome::Pulled { nodes }) => {
                info!(kb_id = %ticket.kb_id, nodes, "mesh join complete");
                pulled += 1;
            }
            Ok(JoinOutcome::Pending) => {
                info!(kb_id = %ticket.kb_id, "mesh join pending owner approval — will retry");
                requeue.push(ticket);
            }
            Err(e) => {
                warn!(kb_id = %ticket.kb_id, error = %e, "mesh join failed — will retry");
                requeue.push(ticket);
            }
        }
    }
    let requeued = requeue.len();
    if requeued > 0 {
        state.lock().await.pending_p2p_joins.extend(requeue);
    }
    (pulled, requeued)
}

/// What a dial+join attempt produced.
#[derive(Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    /// The KB was pulled: the collection + `nodes` node docs are now local.
    Pulled { nodes: usize },
    /// The owner queued us for approval (invite policy); retry after approval.
    Pending,
}

/// Dial the ticket's owner, verify identity, register the trust anchor, and pull
/// the KB. **Identity-addressed**: we connect to the node-id and REJECT if the
/// established `remote_id()` doesn't match the ticket (a spoofed/tampered address
/// can waste a dial but never impersonate the owner — ADR-025).
pub async fn dial_and_join(
    local: &Endpoint,
    ticket: &JoinTicket,
    doc_store: &Arc<DocStore>,
) -> Result<JoinOutcome, String> {
    let expected = ticket.node_id();
    let conn = tokio::time::timeout(
        DIAL_TIMEOUT,
        local.connect(ticket.endpoint.clone(), MAE_ALPN),
    )
    .await
    .map_err(|_| "dial timed out".to_string())?
    .map_err(|e| format!("dial failed: {e}"))?;

    // Anti-spoof (ADR-025): the QUIC/TLS handshake proves the peer key. Connecting
    // by EndpointAddr already targets the node-id; assert it explicitly anyway.
    let remote = conn.remote_id();
    if remote != expected {
        conn.close(1u32.into(), b"identity mismatch");
        return Err(format!(
            "remote identity {remote} != ticket node-id {expected} (spoofed address?)"
        ));
    }

    // Register the external trust anchor — the owner's node-id is the key the op-log
    // genesis must be signed by (ADR-026). This flips `kb_access` to the derived
    // (peer-verifiable) path for this joined KB.
    doc_store
        .set_kb_anchor(&ticket.kb_id, *remote.as_bytes())
        .await;

    // Pull the KB over the kb/join protocol (full state — no node_svs on a first
    // join). iroh QUIC already authenticated both ends, so there is no JSON auth
    // handshake; we open one bi stream and speak the same framing as the editor.
    let (mut send, recv) = conn.open_bi().await.map_err(|e| format!("open_bi: {e}"))?;
    let req = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "kb/join",
        "params": { "kb_id": ticket.kb_id }
    })
    .to_string();
    mae_mcp::write_framed(&mut send, req.as_bytes(), WRITE_TIMEOUT)
        .await
        .map_err(|e| format!("write kb/join: {e}"))?;
    // Do NOT finish() the send stream: that signals EOF and tears down the peer's
    // long-lived `run_session` before it flushes the reply. Content-Length framing
    // already delimits the request, so the peer responds on the still-open stream;
    // we close the whole connection after reading.
    let mut reader = BufReader::new(recv);
    let resp = mae_mcp::read_message(&mut reader)
        .await
        .map_err(|e| format!("read response: {e}"))?
        .ok_or_else(|| "owner closed without responding".to_string())?;
    drop(send);
    conn.close(0u32.into(), b"bye");

    let outcome = apply_join_response(&resp, &ticket.kb_id, doc_store).await?;
    match &outcome {
        JoinOutcome::Pulled { nodes } => {
            info!(kb_id = %ticket.kb_id, owner = %expected, nodes, "mesh join: pulled KB")
        }
        JoinOutcome::Pending => {
            info!(kb_id = %ticket.kb_id, owner = %expected, "mesh join: pending owner approval")
        }
    }
    Ok(outcome)
}

/// Parse a `kb/join` response and apply the collection + node docs locally. A fresh
/// join receives full `state` per doc (created via `share_doc`); the incremental
/// `diff` reconcile path arrives with ongoing sync.
async fn apply_join_response(
    resp: &str,
    kb_id: &str,
    doc_store: &Arc<DocStore>,
) -> Result<JoinOutcome, String> {
    let v: serde_json::Value =
        serde_json::from_str(resp).map_err(|e| format!("bad response json: {e}"))?;
    if let Some(err) = v.get("error") {
        return Err(format!("owner rejected join: {err}"));
    }
    let result = v.get("result").ok_or("response has no result")?;
    if result.get("status").and_then(|s| s.as_str()) == Some("pending") {
        return Ok(JoinOutcome::Pending);
    }

    // The collection doc (membership + node manifest + signed op-log).
    let coll_b64 = result
        .get("collection_state")
        .and_then(|s| s.as_str())
        .ok_or("response missing collection_state")?;
    let coll_bytes =
        base64_to_update(coll_b64).map_err(|e| format!("bad collection_state: {e}"))?;
    doc_store
        .share_doc(&format!("kbc:{kb_id}"), &coll_bytes)
        .await
        .map_err(|e| format!("store collection: {e}"))?;

    // The node docs (full state on a fresh join).
    let mut nodes = 0usize;
    if let Some(arr) = result.get("nodes").and_then(|n| n.as_array()) {
        for node in arr {
            let Some(id) = node.get("id").and_then(|s| s.as_str()) else {
                continue;
            };
            let Some(state_b64) = node.get("state").and_then(|s| s.as_str()) else {
                continue; // a diff-only entry (reconcile) — not expected on a fresh join
            };
            let Ok(state) = base64_to_update(state_b64) else {
                warn!(node = %id, "mesh join: undecodable node state");
                continue;
            };
            if let Err(e) = doc_store.share_doc(&format!("kb:{id}"), &state).await {
                warn!(node = %id, error = %e, "mesh join: failed to store node");
                continue;
            }
            nodes += 1;
        }
    }
    Ok(JoinOutcome::Pulled { nodes })
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
    use std::time::Instant;

    fn mem_store() -> Arc<DocStore> {
        let backend = Arc::new(crate::storage::SqliteBackend::open_memory().unwrap());
        Arc::new(DocStore::new(backend, 500))
    }

    /// A real two-endpoint mesh: owner A serves a p2p-shared KB; joiner B dials by
    /// node-id, verifies identity, registers the anchor, and pulls the KB.
    #[tokio::test]
    async fn dialer_pulls_a_shared_kb_over_the_mesh() {
        // --- Owner A: identity, endpoint, a KB (1 node) with B as an editor. ---
        let a_id = Arc::new(Identity::generate("owner-A"));
        let b_id = Identity::generate("joiner-B");
        let b_fp = b_id.fingerprint();

        let a_store = mem_store();
        a_store.set_signer(Arc::clone(&a_id));
        let a_bc: mae_mcp::broadcast::SharedBroadcaster =
            Arc::new(std::sync::Mutex::new(EventBroadcaster::new()));

        let owner_fp = a_id.fingerprint();
        let mut coll = KbCollectionDoc::new_owned("kbx", &owner_fp, "A");
        coll.add_node("concept:n", "Node N");
        coll.upsert_member(&b_fp, "B", SyncRole::Editor);
        coll.set_transport_policy(TransportPolicy::P2p); // reachable over the mesh
                                                         // Populate the signed op-log A would build via the handler (genesis
                                                         // owner self-admit + the admit of B), so B can peer-verify it.
        let secret = a_id.secret_bytes();
        let pubkey = a_id.public().to_bytes();
        let g = coll.build_membership_op(
            "kbx",
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
        let admit = coll.build_membership_op(
            "kbx",
            MembershipAction::Admit,
            &b_fp,
            Some(SyncRole::Editor),
            false,
            &owner_fp,
            0,
            None,
            coll.epoch_of(&b_fp),
        );
        let asig = admit.sign(&secret);
        coll.append_signed_op(&admit, &asig, &pubkey);
        a_store
            .share_doc("kbc:kbx", &coll.encode_state())
            .await
            .unwrap();
        let node = KbNodeDoc::new("concept:n", "Node N", "hello mesh", &[]);
        a_store
            .share_doc("kb:concept:n", &node.encode())
            .await
            .unwrap();

        let a_endpoint = bind_endpoint(&a_id, RelayMode::Disabled).await.unwrap();
        // Compute the dialable loopback address BEFORE serve() moves the endpoint.
        let a_addr = EndpointAddr::from_parts(
            a_endpoint.id(),
            a_endpoint
                .bound_sockets()
                .into_iter()
                .map(|sa| TransportAddr::Ip(loopback_if_unspecified(sa))),
        );
        // gate_open = true ⇒ admit B by bare fingerprint without an authorized_keys
        // file; per-KB membership still gates (B is an editor of kbx).
        tokio::spawn(serve(
            a_endpoint,
            std::path::PathBuf::from("/nonexistent/authorized_keys"),
            true,
            Arc::clone(&a_store),
            a_bc,
            Instant::now(),
        ));

        // --- Joiner B: dial the ticket, pull the KB. ---
        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let ticket = JoinTicket::new(a_addr, "kbx");

        let outcome = dial_and_join(&b_endpoint, &ticket, &b_store)
            .await
            .expect("dial + join succeeds");
        assert_eq!(outcome, JoinOutcome::Pulled { nodes: 1 });

        // B now holds the collection + the node, and anchored the owner key.
        assert!(b_store.has_doc("kbc:kbx").await, "collection pulled");
        assert!(b_store.has_doc("kb:concept:n").await, "node pulled");
        assert_eq!(
            b_store.kb_anchor("kbx").await,
            Some(a_id.public().to_bytes()),
            "the owner node-id is registered as the KB's trust anchor"
        );

        // And B can VERIFY membership from the pulled signed op-log (ADR-026): the
        // anchor + the op-log A signed let B derive A as owner without trusting any
        // relay.
        let (state, _) = b_store.encode_state_and_sv("kbc:kbx").await.unwrap();
        let pulled = KbCollectionDoc::from_bytes(&state).unwrap();
        let members = mae_sync::membership::derive_valid_members(
            &pulled.oplog_ops(),
            &a_id.public().to_bytes(),
            0,
        );
        assert_eq!(
            members.get(&a_id.fingerprint()).map(|m| m.role),
            Some(SyncRole::Owner),
            "B derives A as owner from the pulled op-log"
        );
    }

    /// A ticket whose node-id doesn't match the dialed endpoint is rejected — the
    /// anti-spoof invariant (identity is the key, not the address).
    #[tokio::test]
    async fn dialer_rejects_node_id_mismatch() {
        let a_id = Arc::new(Identity::generate("owner-A"));
        let a_store = mem_store();
        let a_bc: mae_mcp::broadcast::SharedBroadcaster =
            Arc::new(std::sync::Mutex::new(EventBroadcaster::new()));
        let a_endpoint = bind_endpoint(&a_id, RelayMode::Disabled).await.unwrap();
        let a_sockets: Vec<_> = a_endpoint
            .bound_sockets()
            .into_iter()
            .map(|sa| TransportAddr::Ip(loopback_if_unspecified(sa)))
            .collect();
        tokio::spawn(serve(
            a_endpoint,
            std::path::PathBuf::from("/nonexistent/authorized_keys"),
            true,
            Arc::clone(&a_store),
            a_bc,
            Instant::now(),
        ));

        // Build a ticket pointing at A's ADDRESS but claiming a DIFFERENT node-id.
        let imposter =
            iroh::SecretKey::from(Identity::generate("imposter").secret_bytes()).public();
        let bad_addr = EndpointAddr::from_parts(imposter, a_sockets);
        let ticket = JoinTicket::new(bad_addr, "kbx");

        let b_id = Identity::generate("joiner-B");
        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();

        // iroh dials by the (imposter) node-id and won't reach A under that key, so
        // the dial fails — either way the join never succeeds and no anchor is set.
        let res = tokio::time::timeout(
            Duration::from_secs(8),
            dial_and_join(&b_endpoint, &ticket, &b_store),
        )
        .await;
        assert!(
            matches!(res, Ok(Err(_)) | Err(_)),
            "a node-id mismatch never yields a successful join"
        );
        assert!(
            b_store.kb_anchor("kbx").await.is_none(),
            "no anchor on failure"
        );
    }

    /// Spin up owner A serving `kb_id` over the mesh; returns A's dialable address.
    /// `member` Some ⇒ that principal is an editor (a join PULLS); None ⇒ the
    /// default Invite policy (a join goes PENDING).
    async fn serve_owner_kb(
        a_id: &Arc<Identity>,
        kb_id: &str,
        member: Option<&str>,
    ) -> EndpointAddr {
        let a_store = mem_store();
        let a_bc: SharedBroadcaster = Arc::new(std::sync::Mutex::new(EventBroadcaster::new()));
        let mut coll = KbCollectionDoc::new_owned(kb_id, &a_id.fingerprint(), "A");
        coll.set_transport_policy(TransportPolicy::P2p);
        if let Some(m) = member {
            coll.upsert_member(m, "member", SyncRole::Editor);
        }
        a_store
            .share_doc(&format!("kbc:{kb_id}"), &coll.encode_state())
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
            a_store,
            a_bc,
            Instant::now(),
        ));
        addr
    }

    #[tokio::test]
    async fn drain_pulls_a_member_kb_and_clears_the_queue() {
        let a_id = Arc::new(Identity::generate("owner-D"));
        let b_id = Identity::generate("joiner-D");
        let addr = serve_owner_kb(&a_id, "kbm", Some(&b_id.fingerprint())).await;

        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let state = Arc::new(Mutex::new(DaemonState::new()));
        state
            .lock()
            .await
            .pending_p2p_joins
            .push(JoinTicket::new(addr, "kbm"));

        let (pulled, requeued) = drain_pending_once(&state, &b_store, &b_endpoint).await;
        assert_eq!((pulled, requeued), (1, 0));
        assert!(b_store.has_doc("kbc:kbm").await, "KB pulled");
        assert!(
            state.lock().await.pending_p2p_joins.is_empty(),
            "a pulled ticket is not re-queued"
        );
    }

    #[tokio::test]
    async fn drain_requeues_a_pending_join_for_retry() {
        let a_id = Arc::new(Identity::generate("owner-P"));
        let b_id = Identity::generate("joiner-P");
        // B is NOT a member ⇒ invite policy ⇒ kb/join returns pending.
        let addr = serve_owner_kb(&a_id, "kbp", None).await;

        let b_endpoint = bind_endpoint(&b_id, RelayMode::Disabled).await.unwrap();
        let b_store = mem_store();
        let state = Arc::new(Mutex::new(DaemonState::new()));
        state
            .lock()
            .await
            .pending_p2p_joins
            .push(JoinTicket::new(addr, "kbp"));

        let (pulled, requeued) = drain_pending_once(&state, &b_store, &b_endpoint).await;
        assert_eq!(
            (pulled, requeued),
            (0, 1),
            "a pending join is retried, not dropped"
        );
        assert_eq!(
            state.lock().await.pending_p2p_joins.len(),
            1,
            "the ticket is re-queued for the next poll"
        );
        assert!(
            !b_store.has_doc("kbc:kbp").await,
            "no KB pulled while pending"
        );
        // The anchor is still registered — identity was verified on connect (ADR-026).
        assert_eq!(
            b_store.kb_anchor("kbp").await,
            Some(a_id.public().to_bytes()),
            "anchor registered even while the join is pending"
        );
    }
}
