//! Collab bridge — translates between editor-side intents and the TCP connection
//! to the state server, and handles incoming collab events.
//!
//! Follows the same pattern as `lsp_bridge.rs` and `dap_bridge.rs`:
//! - `drain_collab_intents()` called every tick
//! - `handle_collab_event()` handles events from the background task
//! - `run_collab_task()` is the background tokio task owning the TCP connection

use std::collections::HashSet;
use std::sync::Mutex;

use mae_core::{CollabIntent, CollabStatus, Editor, JoinedNode};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

/// Module-level storage for the mDNS manager, preventing the memory leak
/// from `std::mem::forget`. The manager auto-unregisters its service on Drop.
static MDNS_MANAGER: Mutex<Option<crate::mdns_discovery::MdnsManager>> = Mutex::new(None);

/// Ensure the module-level mDNS manager is browsing for peers in the background,
/// so `collab-discover` reads results instantly instead of blocking the main
/// thread on a discovery sleep (CLAUDE.md #1). Idempotent: creates a browse-only
/// manager on first call; starts the browse on an existing (register-only)
/// manager. Best-effort — mDNS may be unavailable (no multicast).
fn ensure_mdns_browsing() {
    let Ok(mut guard) = MDNS_MANAGER.lock() else {
        return;
    };
    match guard.as_mut() {
        Some(mgr) => {
            if !mgr.is_browsing() {
                if let Err(e) = mgr.start_browse() {
                    debug!(error = %e, "mDNS browse start failed");
                }
            }
        }
        None => match crate::mdns_discovery::MdnsManager::new() {
            Ok(mut mgr) => {
                if let Err(e) = mgr.start_browse() {
                    debug!(error = %e, "mDNS browse start failed");
                }
                *guard = Some(mgr);
            }
            Err(e) => debug!(error = %e, "mDNS unavailable — discovery disabled"),
        },
    }
}

/// Snapshot of peers the background browse has discovered so far.
fn mdns_discovered_peers() -> Vec<crate::mdns_discovery::DiscoveredPeer> {
    MDNS_MANAGER
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|m| m.discovered_peers()))
        .unwrap_or_default()
}

/// Compute a deterministic client_id for a buffer's yrs Doc.
///
/// yrs v0.22 uses variable-length integer encoding in the v1 wire format.
/// Client IDs that exceed ~32 bits get silently corrupted during
/// encode/decode roundtrips, causing remote updates to reference unknown
/// client IDs and become no-ops (stuck in yrs pending queue).
///
/// We use FNV-1a hash of (PID, buffer_index) to produce a 32-bit value,
/// which is safe for the wire format and still deterministic per-process.
fn compute_client_id(buffer_idx: usize) -> u64 {
    let pid = std::process::id();
    // FNV-1a 32-bit
    let mut h: u32 = 0x811c_9dc5;
    for b in pid.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    for b in (buffer_idx as u32).to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    // Ensure non-zero (yrs uses 0 as sentinel in some paths).
    if h == 0 {
        1
    } else {
        h as u64
    }
}

/// Marker the daemon embeds in an epoch-fence rejection's error message
/// (daemon/src/collab_handler.rs:1780). This is the editor↔daemon contract: a
/// `kb/node_update` rejected with this text was authored under a stale,
/// pre-grant authorization epoch (ADR-023) and can never be accepted as-is.
/// Matched as a substring because the daemon appends node-specific detail.
const EPOCH_FENCE_MARKER: &str = "rebase required";

/// True if a daemon `kb/node_update` rejection is the epoch fence firing.
///
/// Centralizing the contract string here (instead of an inline `contains` at the
/// call site) means a daemon-side reword can't silently downgrade fence
/// rejections into a generic "sync failed" status line — which would drop the
/// actionable ADR-024 notification and reopen the B-19 silent-cascade UX gap.
/// The producer side is guarded by the daemon's `viewer_era_*` /
/// `stale_epoch_continuation_*` tests; this is the consumer-side guard.
fn is_epoch_fence_rejection(message: &str) -> bool {
    message.contains(EPOCH_FENCE_MARKER)
}

/// Capacity for the command channel (main thread -> collab background task).
const COLLAB_CMD_CHANNEL_CAP: usize = 256;
/// Capacity for the event channel (collab background task -> main thread).
/// Must be large enough to absorb bursts during main-thread stalls (e.g. scheme
/// init, heavy rendering). If the channel fills, events are dropped and gap
/// detection triggers a resync — no data loss, just latency.
const COLLAB_EVT_CHANNEL_CAP: usize = 512;

/// Non-blocking send on the event channel. The bg task must NEVER block on
/// `send().await` — doing so freezes the select loop, prevents reading server
/// messages and commands, and causes cascading backpressure all the way to the
/// server (which then can't write to us, blocking its handler for our session).
fn try_send_evt(tx: &mpsc::Sender<CollabEvent>, event: CollabEvent) {
    if let Err(e) = tx.try_send(event) {
        warn!("collab evt channel full/closed — event dropped: {}", e);
    }
}

/// Re-render the `*Collab Status*` buffer if it is currently open, by queuing a
/// fresh status query (ADR-019, bob's report): state-changing collab events
/// previously left it stale — e.g. it kept showing "pending owner approval"
/// even after the join succeeded. Queued (one-per-tick) so it never blocks.
fn refresh_collab_status_if_open(editor: &mut Editor) {
    if editor.find_buffer_by_name("*Collab Status*").is_some() {
        editor
            .collab
            .reconnect_intents
            .push_back(CollabIntent::ShowStatus);
    }
    // P1: the *KB Sharing* management buffer reflects the same membership state,
    // so repaint it on every collab event (share/join/leave + live kbc:
    // membership broadcasts) — a remote promote/demote/approve shows up live.
    editor.refresh_kb_sharing_buffer();
}

// --- Command / Event types ---

/// Commands sent from the main thread to the collab background task.
#[derive(Debug)]
pub enum CollabCommand {
    Connect {
        address: String,
    },
    Disconnect,
    ShareBuffer {
        doc_id: String,
        state_bytes: Vec<u8>,
    },
    ForceSync {
        doc_id: String,
    },
    ShowStatus,
    Doctor {
        /// Per-buffer sync info: (doc_id, pending_update_count).
        synced_info: Vec<(String, usize)>,
    },
    StartServer,
    /// Send a yrs update to the state server for a synced buffer.
    SendUpdate {
        doc_id: String,
        update_base64: String,
    },
    /// List documents on the server.
    ListDocs {
        for_join: bool,
    },
    /// Join (resync) a document from the server.
    JoinDoc {
        doc_id: String,
    },
    /// Send save intent to the server (docs/save_intent).
    SendSaveIntent {
        doc_id: String,
        expected_hash: String,
    },
    /// Send awareness state (cursor/selection) to the state server.
    /// Throttled at 50ms by the caller.
    SendAwareness {
        doc_id: String,
        state_json: String,
    },
    /// Confirm save completed (docs/save_committed).
    SendSaveCommitted {
        doc_id: String,
        save_epoch: u64,
        content_hash: String,
        saved_by: String,
    },
    /// Share a KB for collaborative editing (collection + node states).
    ShareKb {
        kb_id: String,
        name: String,
        creator: String,
        collection_state: Vec<u8>,
        node_states: Vec<(String, Vec<u8>)>,
    },
    /// Join a shared KB from the server. `node_svs` carries the member's per-node
    /// state vectors (ADR-022) so the daemon replies with incremental diffs and
    /// the member reconciles instead of adopting full snapshots.
    JoinKb {
        kb_id: String,
        node_svs: Vec<(String, Vec<u8>)>,
    },
    /// Leave a shared KB.
    LeaveKb {
        kb_id: String,
    },
    /// ADR-024 R1: fetch a single node's authoritative state from the daemon so the
    /// editor can adopt it (drop a fenced stale-epoch op) + re-author.
    KbAdoptNode {
        kb_id: String,
        node_id: String,
    },
    /// Send a KB node update to the server (continuous sync).
    /// `pending_rowid` is the durable SQLite queue row this update came from
    /// (`Some` when store-backed); the row is acked only after the daemon confirms
    /// it applied (queue → send → confirm → ack). `None` = transient in-memory
    /// update (no durable store) — best-effort, requeued in-memory on failure.
    KbNodeUpdate {
        kb_id: String,
        node_id: String,
        update: Vec<u8>,
        /// The KB's current authorization epoch (ADR-023) at enqueue, captured on
        /// the editor thread (which holds `kb_epochs`) so the network task — which
        /// holds the signing identity but not editor state — can stamp the ADR-036
        /// signed authorship header without a round-trip.
        epoch: u64,
        pending_rowid: Option<i64>,
    },
    /// Add or remove a peer (by principal) from a KB's members (owner-only).
    KbMember {
        kb_id: String,
        member: String,
        role: String,
        add: bool,
    },
    /// Phase D1.1 (ADR-029): add/remove a node in a KB's collection manifest so the
    /// daemon's projector materializes the create/removes the delete. The node doc
    /// itself rides `KbNodeUpdate`; this carries only the manifest membership.
    KbCollectionNode {
        kb_id: String,
        node_id: String,
        title: String,
        add: bool,
    },
    /// Approve a pending join request as `role` (owner-only, ADR-018).
    KbApprove {
        kb_id: String,
        principal: String,
        role: String,
    },
    /// List pending join requests for a KB (owner-only, ADR-018).
    KbListPending {
        kb_id: String,
    },
    /// Set a KB's join policy (owner-only, ADR-018).
    KbSetPolicy {
        kb_id: String,
        policy: String,
    },
}

/// Events sent from the collab background task back to the main thread.
#[derive(Debug)]
pub enum CollabEvent {
    Connected {
        address: String,
        peer_count: usize,
    },
    /// TOFU: an unknown daemon identity needs interactive approval. The main
    /// thread shows a confirm dialog and sends the decision back via `reply`
    /// (the connection task blocks on it). ADR-017.
    HostKeyPrompt {
        addr: String,
        fingerprint: String,
        reply: std::sync::mpsc::Sender<bool>,
    },
    Disconnected {
        reason: String,
    },
    RemoteUpdate {
        doc_id: String,
        update_bytes: Vec<u8>,
        /// WAL sequence number from server (0 if not present).
        /// Gap detection happens inside handle_incoming_message before sending;
        /// this field is carried for diagnostic/logging use by consumers.
        #[allow(dead_code)]
        wal_seq: u64,
    },
    /// Gap detected in WAL sequence — triggers resync for the doc.
    GapDetected {
        doc_id: String,
        expected: u64,
        got: u64,
    },
    /// Share failed on server — must roll back synced state.
    ShareFailed {
        doc_id: String,
        message: String,
    },
    StatusReport {
        lines: Vec<String>,
    },
    DoctorReport {
        lines: Vec<String>,
    },
    ServerStarted {
        pid: u32,
    },
    ServerFailed {
        error: String,
    },
    Error {
        message: String,
    },
    /// Buffer successfully shared with the server.
    BufferShared {
        doc_id: String,
    },
    /// Server returned the document list.
    DocList {
        documents: Vec<String>,
        for_join: bool,
    },
    /// Joined a remote document — carries the full CRDT state.
    BufferJoined {
        doc_id: String,
        state_bytes: Vec<u8>,
    },
    /// Save intent accepted — server returned save_epoch.
    SaveIntentOk {
        doc_id: String,
        save_epoch: u64,
        content_hash: String,
    },
    /// Save intent rejected — content hash mismatch (concurrent edit).
    SaveIntentConflict {
        doc_id: String,
        message: String,
    },
    /// The sharer of a document disconnected.
    SharerLeft {
        doc_id: String,
    },
    /// Peer count changed (peer joined or left).
    PeerCountChanged {
        peer_count: usize,
    },
    /// A peer saved a shared document.
    PeerSaved {
        doc: String,
        saved_by: String,
    },
    /// Remote awareness update (cursor/selection/presence from another peer).
    AwarenessUpdate {
        client_id: u64,
        doc_id: String,
        state: mae_sync::awareness::AwarenessState,
    },
    /// KB successfully shared with the server.
    KbShared {
        kb_id: String,
        node_count: usize,
        /// Authoritative collection state from the daemon (post-set_owner /
        /// preserved on re-share) — seeds the owner's local collection replica so
        /// it can introspect its own KB's membership (C1 then keeps it fresh).
        collection_state: Vec<u8>,
    },
    /// Joined a shared KB — carries collection + node states.
    KbJoined {
        kb_id: String,
        collection_state: Vec<u8>,
        /// ADR-022: per-node reconcile payloads (diff-or-state + the daemon's SV).
        nodes: Vec<JoinedNode>,
    },
    /// Left a shared KB.
    KbLeft {
        kb_id: String,
    },
    /// Remote KB node update received.
    KbNodeUpdate {
        kb_id: String,
        node_id: String,
        update_bytes: Vec<u8>,
    },
    /// ADR-020 emit durability: the background task could not put a `kb/node_update`
    /// on the wire (write failed, or no writer / disconnected). For a store-backed
    /// update the durable row still exists — just release the in-flight mark so the
    /// next drain retries it. For a transient in-memory update (`pending_rowid:None`)
    /// re-push it to the in-memory queue. Never silently dropped (B-8).
    KbUpdateRequeue {
        kb_id: String,
        node_id: String,
        update: Vec<u8>,
        pending_rowid: Option<i64>,
    },
    /// ADR-020 queue→send→confirm→**ack**: the daemon confirmed it applied a
    /// `kb/node_update` (responded `{applied:true}`). Now — and only now — the
    /// durable SQLite row is removed and the in-flight mark cleared.
    KbUpdateAcked {
        rowid: i64,
    },
    /// The daemon rejected a `kb/node_update` with an error response (e.g. access
    /// denied / malformed). Surfaced loudly; the durable row is dropped (a retry of
    /// the identical update would not succeed) and the in-flight mark cleared.
    KbUpdateFailed {
        kb_id: String,
        node_id: String,
        rowid: Option<i64>,
        message: String,
    },
    /// ADR-024 R1: the daemon returned a node's authoritative state in response to
    /// `kb/node_fetch`. The editor adopts it (dropping a fenced stale-epoch op) and,
    /// if a `pending_reauthor` entry exists, re-applies the kept edit (keep-mine).
    KbNodeAdopted {
        kb_id: String,
        node_id: String,
        state_bytes: Vec<u8>,
    },
}

// --- Intent drain (called every tick) ---

/// Drain the pending collab intent from the editor and forward to the background task.
/// Safe to call every loop iteration.
pub(crate) fn drain_collab_intents(editor: &mut Editor, collab_tx: &mpsc::Sender<CollabCommand>) {
    // Drain pending awareness update (throttled at 50ms).
    if let Some((doc_id, state_json)) = editor.collab.pending_awareness.take() {
        let cmd = CollabCommand::SendAwareness { doc_id, state_json };
        if collab_tx.try_send(cmd).is_err() {
            trace!("collab command channel full — awareness dropped");
        }
    }

    // Drain pending save_committed first (queued by SaveIntentOk handler).
    if let Some((doc_id, save_epoch, content_hash, saved_by)) =
        editor.collab.pending_save_committed.take()
    {
        let cmd = CollabCommand::SendSaveCommitted {
            doc_id,
            save_epoch,
            content_hash,
            saved_by,
        };
        if collab_tx.try_send(cmd).is_err() {
            warn!("collab command channel full — save_committed dropped");
        }
    }

    // Drain pending KB node updates (generated by kb_update_node for shared nodes).
    // Only send when connected — updates accumulate while offline.
    //
    // ADR-020 queue→send→confirm→ack: a store-backed update lives in the durable
    // SQLite queue and is **acked only after the daemon confirms it applied**
    // (CollabEvent::KbUpdateAcked → ack_pending_update). To avoid re-sending the
    // same row every tick while its response is in flight, sent rowids are held in
    // `inflight_kb_updates`; the mark is cleared on ack, on requeue, or on
    // disconnect (so an unconfirmed update retries on reconnect). When no durable
    // store exists, updates fall back to the transient in-memory queue (best-effort).
    if matches!(editor.collab.status, CollabStatus::Connected { .. }) {
        // Durable path: SQLite-persisted updates (survives crashes). Non-destructive
        // read — the row is removed only on daemon-confirmed ack.
        let pending = editor
            .kb
            .store
            .as_ref()
            .and_then(|s| s.drain_pending_updates().ok())
            .unwrap_or_default();
        for pu in pending {
            // Skip rows already on the wire awaiting the daemon's apply-confirmation.
            if !editor.collab.inflight_kb_updates.insert(pu.rowid) {
                continue;
            }
            tracing::debug!(target: "kb_sync", kb_id = %pu.kb_id, node_id = %pu.node_id, rowid = pu.rowid, bytes = pu.update_bytes.len(), "drain: send kb/node_update (durable)");
            let epoch = editor.collab.kb_epochs.get(&pu.kb_id).copied().unwrap_or(0);
            let cmd = CollabCommand::KbNodeUpdate {
                kb_id: pu.kb_id,
                node_id: pu.node_id,
                update: pu.update_bytes,
                epoch,
                pending_rowid: Some(pu.rowid),
            };
            if collab_tx.try_send(cmd).is_err() {
                // Couldn't hand off — release the mark so the next tick retries.
                editor.collab.inflight_kb_updates.remove(&pu.rowid);
                warn!("collab command channel full — persisted KB update retried next tick");
            }
        }

        // Fallback path: transient in-memory updates (only populated when there is
        // no durable store). Destructive take — requeued in-memory on send failure.
        let in_mem = editor.collab.pending_kb_updates.len();
        for (kb_id, node_id, update) in std::mem::take(&mut editor.collab.pending_kb_updates) {
            tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, bytes = update.len(), "drain: send kb/node_update (in-mem)");
            let epoch = editor.collab.kb_epochs.get(&kb_id).copied().unwrap_or(0);
            let cmd = CollabCommand::KbNodeUpdate {
                kb_id,
                node_id,
                update,
                epoch,
                pending_rowid: None,
            };
            if collab_tx.try_send(cmd).is_err() {
                warn!("collab command channel full — KB node update dropped");
            }
        }
        if in_mem > 0 {
            tracing::debug!(target: "kb_sync", count = in_mem, "drain: flushed in-memory kb updates");
        }

        // Phase D1.1: drain collection-manifest ops (created/deleted nodes) →
        // kb/collection_node_*. Best-effort: only sent when connected; creates also
        // self-heal on the reconnect re-share (which rebuilds the full manifest).
        for (kb_id, node_id, title, add) in std::mem::take(&mut editor.collab.pending_kb_manifest) {
            tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, add, "drain: send kb/collection_node");
            let cmd = CollabCommand::KbCollectionNode {
                kb_id,
                node_id,
                title,
                add,
            };
            if collab_tx.try_send(cmd).is_err() {
                warn!("collab command channel full — KB manifest op dropped");
            }
        }
    }

    // ADR-019: feed the reconstruction queue through the single intent slot,
    // one per tick, so re-join/re-share fans out across all durably-shared KBs
    // on reconnect (reusing the existing per-intent handling below).
    if editor.collab.pending_intent.is_none() {
        if let Some(next) = editor.collab.reconnect_intents.pop_front() {
            editor.collab.pending_intent = Some(next);
        }
    }

    let intent = match editor.collab.pending_intent.take() {
        Some(i) => i,
        None => return,
    };

    let cmd = match intent {
        CollabIntent::StartServer => CollabCommand::StartServer,
        CollabIntent::Connect { address } => {
            // Start discovering LAN peers in the background so `collab-discover`
            // has results ready (non-blocking — just spawns a browse thread).
            ensure_mdns_browsing();
            CollabCommand::Connect { address }
        }
        CollabIntent::Disconnect => CollabCommand::Disconnect,
        CollabIntent::ShowStatus => CollabCommand::ShowStatus,
        CollabIntent::ShareBuffer { buffer_name } => {
            // Enable sync on the buffer if not already enabled, then encode state.
            let idx = editor.find_buffer_by_name(&buffer_name);
            if let Some(idx) = idx {
                // Compute DocAddress from file_path + project root.
                let project_root = editor.active_project_root().map(|p| p.to_path_buf());
                let buf = &mut editor.buffers[idx];
                if buf.doc_address.is_none() {
                    buf.doc_address = compute_doc_address(buf, project_root.as_deref());
                }
                if buf.sync_doc.is_none() {
                    let client_id = compute_client_id(idx);
                    buf.enable_sync(client_id);
                    // Clear pending updates from enable_sync's initial insert —
                    // the full state is sent via ShareBuffer, not incremental updates.
                    buf.pending_sync_updates.clear();
                }
                let state_bytes = buf
                    .sync_doc
                    .as_ref()
                    .map(|s| s.encode_state())
                    .unwrap_or_default();
                // Use DocAddress-based doc_name for cross-session stability,
                // falling back to buffer name for unnamed/scratch buffers.
                let doc_id = buf
                    .doc_address
                    .as_ref()
                    .map(|a| a.to_doc_name())
                    .unwrap_or_else(|| buffer_name.clone());
                // Store doc_id on buffer so remote updates can find it.
                buf.collab_doc_id = Some(doc_id.clone());
                // BUG A fix: immediately track as synced so edits during the
                // server round-trip are forwarded via drain_and_broadcast().
                editor.collab.synced_buffers.insert(doc_id.clone());
                editor.collab.synced_docs = editor.collab.synced_buffers.len();
                debug!(doc = %doc_id, "share: immediately tracked as synced");
                CollabCommand::ShareBuffer {
                    doc_id,
                    state_bytes,
                }
            } else {
                return; // Buffer not found
            }
        }
        CollabIntent::ForceSync { buffer_name } => CollabCommand::ForceSync {
            doc_id: buffer_name,
        },
        CollabIntent::Doctor => {
            // Collect per-buffer sync info for the doctor report.
            let synced_info: Vec<(String, usize)> = editor
                .collab
                .synced_buffers
                .iter()
                .map(|doc_id| {
                    let pending = editor
                        .find_buffer_by_collab_doc_id(doc_id)
                        .map(|idx| editor.buffers[idx].pending_sync_updates.len())
                        .unwrap_or(0);
                    (doc_id.clone(), pending)
                })
                .collect();
            CollabCommand::Doctor { synced_info }
        }
        CollabIntent::SaveCollab {
            doc_id,
            content_hash,
        } => CollabCommand::SendSaveIntent {
            doc_id,
            expected_hash: content_hash,
        },
        CollabIntent::ListDocs => CollabCommand::ListDocs { for_join: false },
        CollabIntent::ListDocsForJoin => CollabCommand::ListDocs { for_join: true },
        CollabIntent::JoinDoc { doc_id } => CollabCommand::JoinDoc { doc_id },
        CollabIntent::ShareKb { kb_name, node_ids } => {
            // ADR-020 B-16: establish + persist a canonical CRDT lineage for every
            // shared node (incl. never-edited ones) BEFORE encoding the payload, so
            // the owner's local docs ARE the lineage peers adopt on join — otherwise
            // `to_collection` mints an ephemeral, non-persisted lineage and a peer's
            // later edit no-ops against the owner's divergent local doc (bob→alice).
            editor.kb_prepare_share_lineage(&kb_name, &node_ids);
            // Look up the KB instance: KB_DEFAULT_NAME/"primary" → editor.kb.primary,
            // otherwise resolve a registered instance. `editor.kb.instances` is keyed
            // by UUID, but callers pass a human name (e.g. ":kb-share collabtest"), so
            // map name→uuid via the registry first (find() accepts a name or a uuid).
            // Without this, every named-instance share failed with "KB not found".
            let kb = if kb_name == mae_core::KB_DEFAULT_NAME || kb_name == "primary" {
                Some(&editor.kb.primary)
            } else {
                let uuid = editor
                    .kb
                    .registry
                    .find(&kb_name)
                    .map(|inst| inst.uuid.clone());
                uuid.and_then(|u| editor.kb.instances.get(&u))
                    .or_else(|| editor.kb.instances.get(&kb_name))
            };
            let kb = match kb {
                Some(k) => k,
                None => {
                    editor.set_status(format!("KB '{}' not found", kb_name));
                    return;
                }
            };
            let creator = editor.collab.user_name.clone();
            let kb_id = kb_name.clone();

            match kb.to_collection(&kb_name, &creator, &node_ids) {
                Ok((coll, node_states)) => {
                    let collection_state = coll.encode_state();
                    let node_count = node_states.len();
                    info!(
                        kb = %kb_id,
                        node_count,
                        collection_bytes = collection_state.len(),
                        "sharing KB: encoded collection + nodes"
                    );
                    CollabCommand::ShareKb {
                        kb_id,
                        name: kb_name,
                        creator,
                        collection_state,
                        node_states,
                    }
                }
                Err(e) => {
                    error!(kb = %kb_name, error = %e, "failed to encode KB for sharing");
                    editor.set_status(format!("Failed to share KB: {}", e));
                    return;
                }
            }
        }
        CollabIntent::JoinKb { kb_id, node_svs } => CollabCommand::JoinKb { kb_id, node_svs },
        CollabIntent::LeaveKb { kb_id } => CollabCommand::LeaveKb { kb_id },
        CollabIntent::KbAddMember {
            kb_id,
            member,
            role,
        } => CollabCommand::KbMember {
            kb_id,
            member,
            role,
            add: true,
        },
        CollabIntent::KbRemoveMember { kb_id, member } => CollabCommand::KbMember {
            kb_id,
            member,
            role: String::new(),
            add: false,
        },
        CollabIntent::KbApprove {
            kb_id,
            principal,
            role,
        } => CollabCommand::KbApprove {
            kb_id,
            principal,
            role,
        },
        CollabIntent::KbListPending { kb_id } => CollabCommand::KbListPending { kb_id },
        CollabIntent::KbSetPolicy { kb_id, policy } => CollabCommand::KbSetPolicy { kb_id, policy },
        CollabIntent::KbNodeUpdate {
            kb_id,
            node_id,
            update,
        } => CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            // This intent path is not a live producer (node updates flow via the
            // drain, which stamps the real epoch); 0 is the unsigned/epoch-0 default.
            epoch: 0,
            pending_rowid: None,
        },
        CollabIntent::KbAdoptNode { kb_id, node_id } => {
            CollabCommand::KbAdoptNode { kb_id, node_id }
        }
        CollabIntent::DiscoverPeers => {
            // Read the BACKGROUND browse's current snapshot — never sleep on the
            // main thread (#1). The browse runs persistently (started on connect /
            // server-start / first discover), so results accumulate over time.
            ensure_mdns_browsing();
            let peers = mdns_discovered_peers();
            if peers.is_empty() {
                editor.set_status(
                    "Discovering MAE peers… run :collab-discover again in a moment.".to_string(),
                );
            } else {
                let mut lines = vec![
                    "Discovered MAE Peers".to_string(),
                    "====================".to_string(),
                    String::new(),
                ];
                for p in &peers {
                    lines.push(format!(
                        "  {} — {} (v{}, {} KBs)",
                        p.user_name, p.address, p.version, p.kb_count
                    ));
                }
                lines.push(String::new());
                lines.push("Use :collab-connect <address> to connect to a peer.".to_string());
                let content = lines.join("\n");
                let idx = editor.find_or_create_buffer("*Collab Discover*", || {
                    let mut buf = mae_core::buffer::Buffer::new();
                    buf.name = "*Collab Discover*".to_string();
                    buf
                });
                editor.buffers[idx].replace_contents(&content);
                editor.switch_to_buffer(idx);
                editor.set_status(format!("Found {} peer(s)", peers.len()));
            }
            return;
        }
    };

    let kind = collab_command_name(&cmd);
    if collab_tx.try_send(cmd).is_err() {
        warn!(
            kind,
            "collab command channel full or closed — intent dropped"
        );
    }
}

/// Compute a `DocAddress` from a buffer's file path and project root.
///
/// Uses `compute_project_identity()` (WU4) for stable cross-machine doc_ids:
/// git remote URL → .project name → basename → absolute path hash.
fn compute_doc_address(
    buf: &mae_core::Buffer,
    project_root: Option<&std::path::Path>,
) -> Option<mae_sync::DocAddress> {
    if let Some(fp) = buf.file_path() {
        let rel_path = if let Some(root) = project_root {
            fp.strip_prefix(root)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| fp.to_string_lossy().to_string())
        } else {
            fp.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| fp.to_string_lossy().to_string())
        };
        let project_hash = if let Some(root) = project_root {
            mae_sync::compute_project_identity(root)
        } else {
            "no-project".to_string()
        };
        Some(mae_sync::DocAddress::File {
            project_hash,
            rel_path,
        })
    } else {
        // No file path — treat as shared scratch buffer.
        Some(mae_sync::DocAddress::Shared {
            name: buf.name.clone(),
        })
    }
}

/// Awareness throttle interval (50ms = 20 Hz).
const AWARENESS_THROTTLE_MS: u64 = 50;

/// Queue an awareness update if the active buffer is synced and throttle allows.
///
/// Call this from the event loop after cursor/mode/selection changes.
pub(crate) fn queue_awareness_update(editor: &mut Editor) {
    // Only send if connected and we have synced buffers.
    if !matches!(editor.collab.status, CollabStatus::Connected { .. }) {
        return;
    }

    // Throttle: skip if < 50ms since last send.
    let now = std::time::Instant::now();
    if now
        .duration_since(editor.collab.last_awareness_sent)
        .as_millis()
        < AWARENESS_THROTTLE_MS as u128
    {
        return;
    }

    let win = editor.window_mgr.focused_window();
    let buf = &editor.buffers[win.buffer_idx];

    // Only for synced buffers.
    let doc_id = match &buf.collab_doc_id {
        Some(id) if editor.collab.synced_buffers.contains(id) => id.clone(),
        _ => return,
    };

    let selection = if matches!(editor.mode, mae_core::Mode::Visual(_)) {
        Some((
            editor.vi.visual_anchor_row,
            editor.vi.visual_anchor_col,
            win.cursor_row,
            win.cursor_col,
        ))
    } else {
        None
    };

    let state = mae_sync::awareness::AwarenessState {
        user_name: editor.collab.user_name.clone(),
        cursor_row: win.cursor_row,
        cursor_col: win.cursor_col,
        selection,
        mode: format!("{:?}", editor.mode).to_lowercase(),
        kb_node_id: None,
        kb_id: None,
    };

    match serde_json::to_string(&state) {
        Ok(json) => {
            trace!(doc = %doc_id, row = win.cursor_row, col = win.cursor_col, "queuing awareness update");
            editor.collab.pending_awareness = Some((doc_id, json));
            editor.collab.last_awareness_sent = now;
        }
        Err(e) => {
            debug!(error = %e, "failed to serialize awareness state");
        }
    }
}

/// Clean up stale remote users (call periodically, e.g. every few seconds).
pub(crate) fn cleanup_stale_awareness(editor: &mut Editor) {
    let removed = editor.collab.remote_users.cleanup_stale();
    if removed > 0 {
        debug!(removed, "cleaned up stale awareness users");
        editor.mark_full_redraw();
    }
}

fn collab_command_name(cmd: &CollabCommand) -> &'static str {
    match cmd {
        CollabCommand::Connect { .. } => "connect",
        CollabCommand::Disconnect => "disconnect",
        CollabCommand::ShareBuffer { .. } => "share-buffer",
        CollabCommand::ForceSync { .. } => "force-sync",
        CollabCommand::ShowStatus => "show-status",
        CollabCommand::Doctor { .. } => "doctor",
        CollabCommand::StartServer => "start-server",
        CollabCommand::SendUpdate { .. } => "send-update",
        CollabCommand::SendSaveIntent { .. } => "send-save-intent",
        CollabCommand::SendAwareness { .. } => "send-awareness",
        CollabCommand::SendSaveCommitted { .. } => "send-save-committed",
        CollabCommand::KbApprove { .. } => "kb-approve",
        CollabCommand::KbListPending { .. } => "kb-pending",
        CollabCommand::KbSetPolicy { .. } => "kb-policy",
        CollabCommand::ListDocs { .. } => "list-docs",
        CollabCommand::JoinDoc { .. } => "join-doc",
        CollabCommand::ShareKb { .. } => "share-kb",
        CollabCommand::JoinKb { .. } => "join-kb",
        CollabCommand::LeaveKb { .. } => "leave-kb",
        CollabCommand::KbAdoptNode { .. } => "kb-adopt-node",
        CollabCommand::KbNodeUpdate { .. } => "kb-node-update",
        CollabCommand::KbMember { .. } => "kb-member",
        CollabCommand::KbCollectionNode { .. } => "kb-collection-node",
    }
}

// --- Event handling (main thread) ---

/// C1 (ADR-023): apply a live `kbc:` collection-doc broadcast to this peer's local
/// collection replica and relearn its authorization epoch WITHOUT a reconnect.
///
/// The daemon broadcasts the KB's collection doc on every membership/role change.
/// We hold a local CRDT replica (`kb_collection_state`) seeded from the join
/// snapshot; applying the broadcast delta and re-deriving `epoch_of(fingerprint)`
/// lets the very next node edit be authored under the rotated, current-epoch
/// client_id — eliminating the manual reconnect the live two-machine test needed.
///
/// Security (CLAUDE.md #10): the daemon stays the sole authority — it re-derives
/// each member's epoch from its OWN authoritative collection when fencing
/// (B-19/B-20, untouched). This relearn is a pure client convenience: a tampered
/// or stale local replica can only mislead THIS client about its own epoch, never
/// self-elevate at the daemon. A client that ignores the relearn and keeps
/// authoring under a stale epoch is still fenced.
fn handle_kbc_membership_broadcast(editor: &mut Editor, kb_id: &str, update_bytes: &[u8]) {
    // No baseline replica ⇒ we never joined this KB (or already left); a bare
    // delta can't be applied to nothing.
    let Some(base) = editor.collab.kb_collection_state.get(kb_id).cloned() else {
        debug!(kb = %kb_id, "kbc broadcast for a KB with no local replica — ignoring");
        return;
    };
    let mut coll = match mae_sync::kb::KbCollectionDoc::from_bytes(&base) {
        Ok(c) => c,
        Err(e) => {
            warn!(kb = %kb_id, error = %e, "kbc broadcast: local replica decode failed");
            return;
        }
    };
    // Pending requests present BEFORE this broadcast (to detect new arrivals).
    let prev_pending: std::collections::HashSet<String> =
        mae_sync::kb::KbCollectionDoc::from_bytes(&base)
            .map(|c| c.pending().into_iter().map(|p| p.fingerprint).collect())
            .unwrap_or_default();

    if let Err(e) = coll.apply_update(update_bytes) {
        warn!(kb = %kb_id, error = %e, "kbc broadcast: apply_update failed");
        return;
    }
    // Persist the advanced replica so the next broadcast composes on top of it.
    editor
        .collab
        .kb_collection_state
        .insert(kb_id.to_string(), coll.encode_state());

    // Any membership change repaints the *KB Sharing* buffer (remote
    // promote/demote/approve/join shows live), even when our own epoch is
    // unchanged.
    editor.refresh_kb_sharing_buffer();

    // P4: surface NEW join requests to the owner as an actionable notification,
    // so the owner isn't blind to pending requests with the buffer closed.
    let me = editor.collab.local_fingerprint.clone();
    if !me.is_empty() && coll.role_of(&me) == Some(mae_sync::kb::Role::Owner) {
        for req in coll.pending() {
            if !prev_pending.contains(&req.fingerprint) {
                let who = mae_core::kb_sharing::format_peer(&req.label, &req.fingerprint);
                editor.notify(
                    mae_core::notifications::Notification::action_required(
                        "collab",
                        format!("KB '{kb_id}': join request from {who}"),
                    )
                    .key(format!("collab:pending:{kb_id}:{}", req.fingerprint))
                    .body(
                        "A peer asked to join this KB. Open *KB Sharing* (SPC C K m) to \
                         approve (a) or deny (d), or use :kb-approve.",
                    ),
                );
            }
        }
    }

    if editor.collab.local_fingerprint.is_empty() {
        return; // no collab identity loaded ⇒ no epoch to relearn
    }
    let new_epoch = coll.epoch_of(&editor.collab.local_fingerprint);
    let prev = editor.collab.kb_epochs.get(kb_id).copied().unwrap_or(0);
    if new_epoch == prev {
        return; // no authorization change for us
    }
    if new_epoch == 0 {
        editor.collab.kb_epochs.remove(kb_id);
    } else {
        editor.collab.kb_epochs.insert(kb_id.to_string(), new_epoch);
    }
    info!(
        kb = %kb_id, prev_epoch = prev, new_epoch,
        "relearned KB authorization epoch from live membership broadcast (C1)"
    );
    editor.notify(mae_core::notifications::Notification::info(
        "collab",
        format!("KB '{kb_id}': your access changed — edits now use updated authorization"),
    ));
    editor.fire_hook("kb-epoch-changed");
    refresh_collab_status_if_open(editor);
}

/// Handle an event from the collab background task — update editor state.
pub(crate) fn handle_collab_event(editor: &mut Editor, event: CollabEvent) {
    match event {
        CollabEvent::HostKeyPrompt {
            addr,
            fingerprint,
            reply,
        } => {
            tracing::debug!(target: "collab", addr = %addr, "host-key TOFU prompt raised (awaiting trust decision)");
            // ADR-017 TOFU, now via the ADR-024 bus: a BlockingReply notification
            // routes to a modal; the y/n answer in apply_mini_dialog sends back on
            // `reply` (unblocking the connection task). One consumer of the generic
            // mechanism — no bespoke `pending_host_key_reply` field anymore.
            editor.notify(
                mae_core::notifications::Notification::action_required(
                    "collab",
                    format!("Trust daemon at {addr}?"),
                )
                .body(format!("{fingerprint}  (first connect — accept & pin?)"))
                // B-22c: explicit bus actions so the prompt is answerable via
                // `notify_resolve` / the `*Notifications*` row (headless/agent parity,
                // and a working answer path while the GUI modal paint is fixed), in
                // addition to the modal y/n keypress — both send on the reply channel.
                .action(
                    "Accept & pin",
                    mae_core::notifications::NotifCommand::Reply(true),
                )
                .action(
                    "Reject",
                    mae_core::notifications::NotifCommand::Reply(false),
                )
                .blocking(mae_core::notifications::NotifReply::Bool(reply)),
            );
            editor.mark_full_redraw();
        }
        CollabEvent::Connected {
            address,
            peer_count,
        } => {
            info!(address = %address, peers = peer_count, "collab connected");
            editor.collab.status = CollabStatus::Connected { peer_count };
            editor.set_status(format!("Connected to {} ({} peers)", address, peer_count));
            // Proactively surface the daemon state (ADR-035 PR C-b).
            editor.notify_daemon_connected(peer_count);
            // WU3: On reconnect, re-share buffers that still have CRDT state (offline recovery).
            let offline_docs: Vec<(String, Vec<u8>)> = editor
                .buffers
                .iter()
                .filter(|b| b.collab_offline && b.sync_doc.is_some() && b.collab_doc_id.is_some())
                .filter_map(|b| {
                    let doc_id = b.collab_doc_id.as_ref()?.clone();
                    let state = b.sync_doc.as_ref()?.encode_state();
                    Some((doc_id, state))
                })
                .collect();
            for (doc_id, _state_bytes) in &offline_docs {
                info!(doc = %doc_id, "reconnect: re-sharing offline buffer");
                editor.collab.synced_buffers.insert(doc_id.clone());
            }
            if !offline_docs.is_empty() {
                editor.collab.synced_docs = editor.collab.synced_buffers.len();
                // Queue re-share for each offline doc. The first one goes via
                // pending_collab_intent; additional ones would need the command channel.
                // For now, queue the first and set a status message.
                if let Some((doc_id, _state)) = offline_docs.first() {
                    editor.collab.pending_intent = Some(CollabIntent::ForceSync {
                        buffer_name: doc_id.clone(),
                    });
                }
                editor.set_status(format!(
                    "Connected to {} — resyncing {} offline buffer(s)",
                    address,
                    offline_docs.len()
                ));
            }

            // ADR-019: KBs get the same reconnect care as buffers. Rebuild the
            // gate cache from durable markers, then re-subscribe every durably-
            // shared INSTANCE so remote edits resume flowing — guests re-join,
            // owners re-share, primary is skipped (see kb_resubscribe_intents).
            // Idempotent via subscribed_kbs; queued one-per-tick.
            editor.reconstruct_kb_sync_gate();
            let mut resubscribed = 0;
            for intent in editor.kb_resubscribe_intents() {
                let key = match &intent {
                    CollabIntent::JoinKb { kb_id, .. } => kb_id.clone(),
                    CollabIntent::ShareKb { kb_name, .. } => kb_name.clone(),
                    _ => continue,
                };
                if editor.collab.subscribed_kbs.insert(key) {
                    editor.collab.reconnect_intents.push_back(intent);
                    resubscribed += 1;
                }
            }
            if resubscribed > 0 {
                info!(count = resubscribed, "reconnect: re-subscribing shared KBs");
            }

            // Phase D (ADR-029): if opted in, host the primary KB on the daemon as a
            // "shared-with-self" collection (kbc:default). refresh_daemon_host_state()
            // flips the runtime gate now that we're Connected; if hosting and not
            // already in flight this connection, enqueue ONE host-only share — the
            // idempotent kb/share imports the primary into the daemon's CRDT, which the
            // projector materializes. host_only routing (no durable marker) is keyed by
            // daemon_host_pending, consumed on the KbShared confirm.
            editor.refresh_daemon_host_state();
            if editor.kb.daemon_hosts_primary() {
                let kb_id = mae_core::KB_DEFAULT_NAME.to_string();
                if editor.collab.daemon_host_pending.insert(kb_id.clone()) {
                    let node_ids: Vec<String> = editor.kb.primary.list_ids(None);
                    info!(
                        kb = %kb_id,
                        nodes = node_ids.len(),
                        "Phase D: hosting primary KB on daemon (host-only share)"
                    );
                    editor
                        .collab
                        .reconnect_intents
                        .push_back(CollabIntent::ShareKb {
                            kb_name: kb_id,
                            node_ids,
                        });
                }
            }

            editor.mark_full_redraw();
        }
        CollabEvent::Disconnected { reason } => {
            info!(reason = %reason, "collab disconnected");
            editor.collab.status = CollabStatus::Disconnected;
            // Proactively surface the loss BEFORE the host/share state is torn down
            // below, so `has_active_shares()` still sees that we were syncing and
            // raises the sticky "deferred, not lost" badge (ADR-035 PR C-b).
            editor.notify_daemon_disconnected(&reason);
            // Phase D3b: snapshot the mirror back to the local store BEFORE dropping
            // the host flag, so this session's edits (whose per-edit write-through was
            // retired) land in the daemon-less fallback. Cheap (only touched nodes).
            if editor.kb.daemon_hosts_primary() {
                editor.kb_snapshot_primary_to_store();
            }
            // Phase D: hosting requires a live write channel — drop the runtime flag
            // and clear in-flight host shares so the next connect re-hosts cleanly.
            editor.collab.daemon_host_pending.clear();
            editor.refresh_daemon_host_state();
            // Re-subscribe on the next connect (ADR-019).
            editor.collab.subscribed_kbs.clear();
            // ADR-020: any kb/node_update whose apply-confirmation was still in flight
            // will never be answered now — release the marks so the durable rows
            // re-drain on reconnect (the rows themselves are untouched in SQLite).
            editor.collab.inflight_kb_updates.clear();
            editor.set_status(format!("Collab disconnected: {}", reason));
            // Preserve sync_doc and collab_doc_id for offline recovery (WU3).
            // Only clear UI tracking state — CRDT state survives disconnect
            // so local edits accumulate for resync on reconnect.
            for buf in &mut editor.buffers {
                if buf.collab_doc_id.is_some() {
                    if buf.sync_doc.is_some() {
                        buf.collab_offline = true;
                    } else {
                        // Buffer with no sync_doc (e.g. ShareFailed already cleared it)
                        // has no state to preserve.
                        buf.collab_doc_id = None;
                    }
                }
            }
            editor.collab.synced_docs = 0;
            // Log pending updates that will be orphaned by clearing synced_buffers.
            let orphaned_updates: usize = editor
                .buffers
                .iter()
                .filter(|b| !b.pending_sync_updates.is_empty())
                .map(|b| b.pending_sync_updates.len())
                .sum();
            if orphaned_updates > 0 {
                warn!(
                    orphaned_updates,
                    synced_buffers_before = ?editor.collab.synced_buffers,
                    "DISCONNECT: clearing synced_buffers with pending updates — these will be LOST"
                );
            }
            editor.collab.synced_buffers.clear();
            editor.collab.confirmed_shares.clear();
            editor.mark_full_redraw();
        }
        CollabEvent::RemoteUpdate {
            doc_id,
            update_bytes,
            wal_seq,
        } => {
            // C1: a `kbc:` broadcast is a KB collection-doc (membership/epoch)
            // delta, NOT buffer text. Apply it to our local collection replica and
            // relearn our authorization epoch live — no manual reconnect. It never
            // maps to a buffer, so intercept it before the buffer lookup.
            if let Some(kb_id) = doc_id.strip_prefix("kbc:") {
                handle_kbc_membership_broadcast(editor, kb_id, &update_bytes);
            } else if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
                // Capture cursor char offsets for all windows viewing this buffer
                // so we can restore them after the rope rebuild.
                let window_cursors: Vec<(mae_core::WindowId, usize)> = editor
                    .window_mgr
                    .iter_windows()
                    .filter(|w| w.buffer_idx == idx)
                    .map(|w| {
                        (
                            w.id,
                            editor.buffers[idx].char_offset_at(w.cursor_row, w.cursor_col),
                        )
                    })
                    .collect();

                // Snapshot old rope for cursor adjustment after update.
                let old_rope = editor.buffers[idx].rope().clone();
                let old_len = old_rope.len_chars();
                match editor.buffers[idx].apply_sync_update(&update_bytes) {
                    Ok(()) => {
                        let new_rope = editor.buffers[idx].rope();
                        let new_len = new_rope.len_chars();
                        let char_delta = new_len as isize - old_len as isize;
                        let text_preview: String = new_rope.chars().take(200).collect();

                        // Find first char position where old and new ropes diverge.
                        // This is the edit point for cursor adjustment.
                        let edit_pos = if char_delta != 0 {
                            old_rope
                                .chars()
                                .zip(new_rope.chars())
                                .position(|(a, b)| a != b)
                                .unwrap_or(old_len.min(new_len))
                        } else {
                            // Same length — could be a replace or no-op.
                            // No adjustment needed if content is identical.
                            old_len
                        };

                        info!(
                            doc = %doc_id,
                            wal_seq,
                            update_len = update_bytes.len(),
                            buf_idx = idx,
                            buf_name = %editor.buffers[idx].name,
                            old_len,
                            new_len,
                            char_delta,
                            edit_pos,
                            text_preview = %text_preview,
                            "applied remote sync update"
                        );

                        // Adjust cursor positions based on where the edit occurred.
                        // Cursors before the edit stay put; cursors after shift by delta.
                        for (win_id, old_offset) in &window_cursors {
                            if let Some(win) = editor.window_mgr.window_mut(*win_id) {
                                let adjusted = if *old_offset <= edit_pos {
                                    *old_offset
                                } else {
                                    // Shift by delta, but never before edit_pos
                                    // (handles cursor inside a deleted range).
                                    let shifted =
                                        (*old_offset as isize + char_delta).max(0) as usize;
                                    shifted.max(edit_pos)
                                };
                                let clamped = adjusted.min(new_len.saturating_sub(1));
                                let (row, col) = editor.buffers[idx].row_col_from_offset(clamped);
                                win.cursor_row = row;
                                win.cursor_col = col;
                                win.clamp_cursor(&editor.buffers[idx]);
                            }
                        }
                        // Clear offline flag on successful remote update.
                        editor.buffers[idx].collab_offline = false;
                        editor.fire_hook("sync-update");
                        editor.mark_full_redraw();
                    }
                    Err(e) => {
                        warn!(doc = %doc_id, error = %e, "failed to apply remote sync update");
                    }
                }
            } else {
                warn!(doc = %doc_id, "remote update for unknown buffer — name mismatch?");
            }
        }
        CollabEvent::GapDetected {
            doc_id,
            expected,
            got,
        } => {
            warn!(doc = %doc_id, expected, got, "WAL sequence gap — requesting resync");
            editor.set_status(format!(
                "Collab: gap detected on {} (expected seq {}, got {}), resyncing",
                doc_id, expected, got
            ));
            // Queue a ForceSync to trigger resync.
            editor.collab.pending_intent = Some(CollabIntent::ForceSync {
                buffer_name: doc_id,
            });
            editor.mark_full_redraw();
        }
        CollabEvent::StatusReport { lines } => {
            debug!(line_count = lines.len(), "status report received");
            let content = lines.join("\n");
            let idx = editor.find_or_create_buffer("*Collab Status*", || {
                let mut buf = mae_core::Buffer::new();
                buf.name = "*Collab Status*".to_string();
                buf.kind = mae_core::BufferKind::Text;
                buf
            });
            editor.buffers[idx].replace_contents(&content);
            editor.switch_to_buffer(idx);
            editor.mark_full_redraw();
        }
        CollabEvent::DoctorReport { lines } => {
            debug!(line_count = lines.len(), "doctor report received");
            let content = lines.join("\n");
            let idx = editor.find_or_create_buffer("*Collab Doctor*", || {
                let mut buf = mae_core::Buffer::new();
                buf.name = "*Collab Doctor*".to_string();
                buf.kind = mae_core::BufferKind::Text;
                buf
            });
            editor.buffers[idx].replace_contents(&content);
            editor.switch_to_buffer(idx);
            editor.mark_full_redraw();
        }
        CollabEvent::ServerStarted { pid } => {
            info!(pid = pid, "state server started");
            // Register mDNS service for peer discovery.
            let port = editor
                .collab
                .server_address
                .rsplit(':')
                .next()
                .and_then(|p| p.parse::<u16>().ok())
                .unwrap_or(mae_core::DEFAULT_COLLAB_PORT);
            let user = if editor.collab.user_name.is_empty() {
                "mae-user"
            } else {
                &editor.collab.user_name
            };
            match crate::mdns_discovery::MdnsManager::new() {
                Ok(mut mgr) => {
                    let kb_count = editor.collab.shared_kbs.len() as u32;
                    if let Err(e) = mgr.register(user, port, kb_count) {
                        warn!(error = %e, "mDNS registration failed");
                    } else {
                        info!(port, user, "mDNS service registered");
                    }
                    // Also browse so this host discovers peers (and `collab-discover`
                    // reads results without blocking the main thread, #1).
                    if let Err(e) = mgr.start_browse() {
                        debug!(error = %e, "mDNS browse start failed");
                    }
                    // Store in module-level static so it lives for the editor's lifetime
                    // and auto-unregisters on drop (no memory leak).
                    if let Ok(mut guard) = MDNS_MANAGER.lock() {
                        *guard = Some(mgr);
                    }
                }
                Err(e) => {
                    debug!(error = %e, "mDNS unavailable — peer discovery disabled");
                }
            }
            editor.set_status(format!("State server started (PID {})", pid));
            editor.mark_full_redraw();
        }
        CollabEvent::ServerFailed { error } => {
            error!(error = %error, "state server failed to start");
            editor.set_status(format!("State server failed: {}", error));
            editor.mark_full_redraw();
        }
        CollabEvent::Error { message } => {
            warn!(error = %message, "collab error");
            editor.set_status(format!("Collab: {}", message));
            editor.mark_full_redraw();
        }
        CollabEvent::BufferShared { doc_id } => {
            info!(doc = %doc_id, "buffer shared (server confirmed)");
            // Doc was already added optimistically in drain_collab_intents (BUG A fix).
            // This insert is idempotent — ensures consistency if event ordering varies.
            editor.collab.synced_buffers.insert(doc_id.clone());
            editor.collab.synced_docs = editor.collab.synced_buffers.len();
            // Mark as server-confirmed (not just optimistically requested).
            editor.collab.confirmed_shares.insert(doc_id.clone());
            // Mark this buffer as the sharer (authoritative saver).
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
                editor.buffers[idx].collab_is_sharer = true;
            }
            editor.set_status(format!("Shared: {}", doc_id));
            editor.mark_full_redraw();
        }
        CollabEvent::DocList {
            documents,
            for_join,
        } => {
            debug!(count = documents.len(), for_join, "doc list received");
            if for_join {
                // Open a palette picker with the document names.
                if documents.is_empty() {
                    editor.set_status("No documents on server");
                } else {
                    let names: Vec<&str> = documents.iter().map(|s| s.as_str()).collect();
                    let palette =
                        mae_core::command_palette::CommandPalette::for_collab_join(&names);
                    editor.command_palette = Some(palette);
                    editor.set_mode(mae_core::Mode::CommandPalette);
                    editor.mark_full_redraw();
                }
            } else {
                // Create a *Collab Docs* buffer with the listing.
                let content = if documents.is_empty() {
                    "No documents shared on server.".to_string()
                } else {
                    let mut lines = vec![format!(
                        "Shared Documents ({})\n{}",
                        documents.len(),
                        "=".repeat(40)
                    )];
                    for doc in &documents {
                        lines.push(format!("  {}", doc));
                    }
                    lines.push(String::new());
                    lines
                        .push("Use :collab-join <name> or SPC C j to join a document.".to_string());
                    lines.join("\n")
                };
                let idx = editor.find_or_create_buffer("*Collab Docs*", || {
                    let mut buf = mae_core::Buffer::new();
                    buf.name = "*Collab Docs*".to_string();
                    buf.kind = mae_core::BufferKind::Text;
                    buf
                });
                editor.buffers[idx].replace_contents(&content);
                editor.switch_to_buffer(idx);
                editor.mark_full_redraw();
            }
        }
        CollabEvent::BufferJoined {
            doc_id,
            state_bytes,
        } => {
            info!(doc = %doc_id, state_bytes = state_bytes.len(), "buffer joined event received");
            // Parse DocAddress from doc_id for structured addressing.
            let doc_addr = mae_sync::DocAddress::parse(&doc_id);
            // Use a display-friendly name for the buffer.
            let buf_name = match &doc_addr {
                Some(mae_sync::DocAddress::File { rel_path, .. }) => rel_path.clone(),
                Some(mae_sync::DocAddress::Shared { name }) => name.clone(),
                Some(mae_sync::DocAddress::KbNode { node_id }) => node_id.clone(),
                Some(mae_sync::DocAddress::KbCollection { kb_id }) => {
                    format!("[kbc:{kb_id}]")
                }
                None => doc_id.clone(),
            };
            // Check if a buffer with this collab_doc_id already exists (e.g.,
            // the user shared a buffer and then also joined it, or a ForceSync
            // response arrived). Using the doc_id match prevents creating a
            // duplicate buffer with a different name but the same CRDT doc,
            // which causes remote updates to be applied to the wrong sync_doc.
            let existing_by_doc_id = editor.find_buffer_by_collab_doc_id(&doc_id);
            let already_existed =
                existing_by_doc_id.is_some() || editor.find_buffer_by_name(&buf_name).is_some();
            let idx = if let Some(i) = existing_by_doc_id {
                info!(doc = %doc_id, buf_idx = i, buf_name = %editor.buffers[i].name,
                    "join: reusing existing buffer with same collab_doc_id");
                i
            } else {
                editor.find_or_create_buffer(&buf_name, || {
                    let mut buf = mae_core::Buffer::new();
                    buf.name = buf_name.clone();
                    buf.kind = mae_core::BufferKind::Text;
                    buf
                })
            };
            // Snapshot project root before mutable borrow of buffer.
            let project_root = editor.active_project_root().map(|p| p.to_path_buf());
            let client_id = compute_client_id(idx);
            let load_ok = {
                let buf = &mut editor.buffers[idx];
                if already_existed && buf.sync_doc.is_some() {
                    // Existing synced buffer (ForceSync resync): merge state via
                    // apply_update to preserve undo/redo history. yrs handles
                    // already-applied operations idempotently via vector clocks.
                    info!(doc = %doc_id, "resync: merging state into existing buffer (preserving undo history)");
                    match buf.apply_sync_update(&state_bytes) {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            warn!(doc = %doc_id, error = %e, "resync merge failed, falling back to full load");
                            buf.load_sync_state(&state_bytes, client_id).map(|()| {
                                buf.doc_address = doc_addr.clone();
                            })
                        }
                    }
                } else {
                    // New buffer (explicit join): full state load.
                    match buf.load_sync_state(&state_bytes, client_id) {
                        Ok(()) => {
                            // Set doc_address for save policy resolution.
                            buf.doc_address = doc_addr.clone();
                            // Joined buffers have NO auto file_path. Users must :saveas
                            // to create a local copy. This matches industry standard
                            // (VS Code Live Share, Zed — guests get no local files).
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
            };
            match load_ok {
                Ok(()) => {
                    let text_preview: String =
                        editor.buffers[idx].text().chars().take(200).collect();
                    info!(doc = %doc_id, buf_idx = idx, text_len = editor.buffers[idx].text().len(),
                        text_preview = %text_preview, "buffer joined: sync state loaded");
                    // Store doc_id on buffer only after successful load — prevents
                    // RemoteUpdate from targeting a buffer with no valid sync_doc.
                    editor.buffers[idx].collab_doc_id = Some(doc_id.clone());
                    // Detect language from doc_id for syntax highlighting.
                    {
                        let content = editor.buffers[idx].text();
                        let path_hint = std::path::Path::new(&doc_id);
                        if let Some(lang) =
                            mae_core::syntax::language_for_buffer(path_hint, &content)
                        {
                            editor.syntax.set_language(idx, lang);
                            editor.buffers[idx]
                                .local_options
                                .apply_defaults(&lang.default_local_options());
                            // Force tree-sitter reparse so the full structural
                            // parser (compute_org_spans) runs on the joined buffer.
                            editor.syntax.invalidate(idx);
                        }
                    }
                    editor.collab.synced_buffers.insert(doc_id.clone());
                    editor.collab.synced_docs = editor.collab.synced_buffers.len();
                    // Mark as server-confirmed.
                    editor.collab.confirmed_shares.insert(doc_id.clone());
                    // Only switch active buffer for newly created buffers (explicit join).
                    // For existing buffers (ForceSync resync), don't steal focus.
                    if !already_existed {
                        editor.switch_to_buffer(idx);
                        editor.set_status(format!("Joined: {}", doc_id));
                    } else {
                        info!(doc = %doc_id, buf_idx = idx, "buffer resync complete (no focus switch)");
                    }
                    editor.mark_full_redraw();

                    // Opt-in: if collab_auto_resolve_paths is enabled and the
                    // doc has a file address with a matching local file, prompt
                    // the user to map the buffer to their local project path.
                    if editor
                        .get_option("collab_auto_resolve_paths")
                        .map(|(v, _)| v == "true")
                        .unwrap_or(false)
                    {
                        if let Some(mae_sync::DocAddress::File { rel_path, .. }) = &doc_addr {
                            let resolved = if let Some(root) = &project_root {
                                let rooted = root.join(rel_path);
                                if rooted.exists() && rooted.parent().is_some_and(|p| p.is_dir()) {
                                    Some(rooted.canonicalize().unwrap_or(rooted))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if let Some(resolved_path) = resolved {
                                let display = rel_path.clone();
                                editor.mini_dialog = Some(
                                    mae_core::command_palette::MiniDialogState::confirm(
                                        format!("Map to local project file {}? (y/n)", display),
                                        mae_core::command_palette::MiniDialogContext::CollabResolvePath {
                                            buf_idx: idx,
                                            resolved_path,
                                        },
                                    ),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    editor.set_status(format!("Failed to join {}: {}", doc_id, e));
                }
            }
        }
        CollabEvent::ShareFailed { doc_id, message } => {
            warn!(doc = %doc_id, error = %message, "share failed — rolling back synced state");
            // Remove from synced set (was optimistically added in drain_collab_intents).
            editor.collab.synced_buffers.remove(&doc_id);
            editor.collab.synced_docs = editor.collab.synced_buffers.len();
            // Clear all collab state on the buffer so re-share starts fresh.
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
                editor.buffers[idx].collab_doc_id = None;
                editor.buffers[idx].sync_doc = None;
                editor.buffers[idx].pending_sync_updates.clear();
            }
            editor.set_status(format!("Share failed: {}", message));
            editor.mark_full_redraw();
        }
        CollabEvent::SaveIntentOk {
            doc_id,
            save_epoch,
            content_hash,
        } => {
            info!(doc = %doc_id, save_epoch, "save intent accepted — sending save_committed");
            let saved_by = if editor.collab.user_name.is_empty() {
                "unknown".to_string()
            } else {
                editor.collab.user_name.clone()
            };
            // Queue the save_committed command for the next drain tick.
            editor.collab.pending_save_committed =
                Some((doc_id.clone(), save_epoch, content_hash, saved_by));
            editor.set_status(format!("Saved (collab epoch {})", save_epoch));
            editor.mark_full_redraw();
        }
        CollabEvent::SaveIntentConflict { doc_id, message } => {
            warn!(doc = %doc_id, "save intent conflict: {}", message);
            editor.set_status(format!(
                "Save conflict on {} — sync first (:collab-sync)",
                doc_id
            ));
            editor.mark_full_redraw();
        }
        CollabEvent::SharerLeft { doc_id } => {
            warn!(doc = %doc_id, "sharer disconnected");
            editor.set_status(format!("Sharer disconnected for {}", doc_id));
            editor.mark_full_redraw();
        }
        CollabEvent::PeerCountChanged { peer_count } => {
            debug!(peer_count, "peer count changed");
            if let CollabStatus::Connected { .. } = editor.collab.status {
                editor.collab.status = CollabStatus::Connected { peer_count };
                if peer_count == 0 {
                    editor.set_status("All other collaborators disconnected");
                } else {
                    editor.set_status(format!(
                        "Peer count: {} collaborator{}",
                        peer_count,
                        if peer_count == 1 { "" } else { "s" }
                    ));
                }
                editor.mark_full_redraw();
            }
        }
        CollabEvent::PeerSaved { doc, saved_by } => {
            debug!(doc = %doc, saved_by = %saved_by, "peer saved");
            editor.set_status(format!("[{}] saved by {}", doc, saved_by));
            // Mark the local buffer clean if we have it (content matches what was saved).
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc) {
                editor.buffers[idx].modified = false;
            }
            editor.mark_full_redraw();
        }
        CollabEvent::AwarenessUpdate {
            client_id,
            doc_id,
            state,
        } => {
            let color_index = mae_core::render_common::collab_colors::collab_color_index(client_id);
            debug!(
                client_id,
                doc = %doc_id,
                user = %state.user_name,
                row = state.cursor_row,
                col = state.cursor_col,
                "awareness update received"
            );
            editor
                .collab
                .remote_users
                .update(client_id, doc_id, state, color_index);
            editor.mark_full_redraw();
        }
        CollabEvent::KbShared {
            kb_id,
            node_count,
            collection_state,
        } => {
            info!(kb = %kb_id, node_count, "KB shared successfully");
            // OQ1 / introspection: seed the owner's local collection replica from
            // the daemon's authoritative collection state so the owner can see its
            // own KB's members/roles/policy in `kb_sharing_snapshot`. C1's
            // `handle_kbc_membership_broadcast` then advances this replica on every
            // membership change. Re-derive the owner's epoch from the same doc.
            if !collection_state.is_empty() {
                if let Ok(coll) = mae_sync::kb::KbCollectionDoc::from_bytes(&collection_state) {
                    if !editor.collab.local_fingerprint.is_empty() {
                        let epoch = coll.epoch_of(&editor.collab.local_fingerprint);
                        if epoch == 0 {
                            editor.collab.kb_epochs.remove(&kb_id);
                        } else {
                            editor.collab.kb_epochs.insert(kb_id.clone(), epoch);
                        }
                    }
                }
                editor
                    .collab
                    .kb_collection_state
                    .insert(kb_id.clone(), collection_state);
            }
            // Track which nodes are shared for continuous sync.
            // Get node IDs from the primary KB (or the named instance).
            let node_ids: HashSet<String> =
                if kb_id == mae_core::KB_DEFAULT_NAME || kb_id == "primary" {
                    if let Some(q) = editor.kb.query_layer() {
                        q.list_ids(None).into_iter().collect()
                    } else {
                        editor.kb.primary.list_ids(None).into_iter().collect()
                    }
                } else {
                    // `instances` is keyed by UUID, but `kb_id` is the human name
                    // (e.g. "collabtest"). Resolve name→uuid via the registry first,
                    // else `shared_kbs` gets an EMPTY set and later edits to the
                    // shared KB never match → no kb/node_update is broadcast (I-9).
                    let uuid = editor
                        .kb
                        .registry
                        .find(&kb_id)
                        .map(|inst| inst.uuid.clone());
                    let kb = uuid
                        .and_then(|u| editor.kb.instances.get(&u))
                        .or_else(|| editor.kb.instances.get(&kb_id));
                    match kb {
                        Some(kb) => kb.list_ids(None).into_iter().collect(),
                        None => HashSet::new(),
                    }
                };
            editor.collab.shared_kbs.insert(kb_id.clone(), node_ids);

            // Phase D (ADR-029): was this a daemon-host share (auto-hosting the
            // primary) rather than a user/peer share? Consume the in-flight marker.
            // Host shares are runtime-only — they must NOT stamp the durable
            // `primary_shared` marker (which would imply peer-share and persist into a
            // later daemon-less launch). The collection + node docs are already on the
            // daemon (the point of hosting); we only refresh the runtime gate.
            if editor.collab.daemon_host_pending.remove(&kb_id) {
                editor.refresh_daemon_host_state();
                tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_count, "host: primary hosted on daemon CRDT (runtime, no durable marker)");
                editor.set_status(format!(
                    "KB '{}' hosted on daemon ({} nodes)",
                    kb_id, node_count
                ));
            } else {
                // ADR-019: stamp the DURABLE share marker so "this KB syncs" survives
                // editor restart / reconnect (the transient `shared_kbs` set above is
                // only a cache; the emit gate now reads these markers). Persisted to
                // the XDG-first registry. Only on a confirmed share.
                let now = mae_kb::data_dir::chrono_now_iso();
                if kb_id == mae_core::KB_DEFAULT_NAME || kb_id == "primary" {
                    editor.kb.registry.primary_shared = true;
                    editor.kb.registry.primary_collab_id = Some(kb_id.clone());
                } else if let Some(inst) = editor.kb.registry.find_mut(&kb_id) {
                    inst.shared = true;
                    inst.collab_id = Some(kb_id.clone());
                    inst.last_sync = Some(now);
                }
                if let Some(dir) = editor.mae_data_dir() {
                    if let Err(e) = editor.kb.registry.save(&dir) {
                        warn!(kb = %kb_id, error = %e, "failed to persist shared-KB registry marker");
                    }
                }
                tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_count, "share: durable marker stamped");
                editor.set_status(format!("KB '{}' shared ({} nodes)", kb_id, node_count));
            }
            refresh_collab_status_if_open(editor);
        }
        CollabEvent::KbJoined {
            kb_id,
            collection_state,
            nodes,
        } => {
            let node_count = nodes.len();
            info!(kb = %kb_id, node_count, collection_bytes = collection_state.len(), "KB joined — reconciling into local store");

            // ADR-019 + ADR-020 + ADR-022: register the joined KB as a FIRST-CLASS
            // federated instance (durable markers, addressable, in kb_instances) and
            // RECONCILE each node's CRDT state (state-vector diff + local-ahead push)
            // rather than overwrite — so a member's offline/local edits survive a
            // (re)join even if their pending-queue row was lost in a crash. Per-node
            // decode/reconcile errors are tolerated (skipped + warned) inside the helper.
            let joined_node_ids: HashSet<String> = nodes.iter().map(|n| n.id.clone()).collect();
            editor.kb_register_joined_instance(&kb_id, nodes);
            // Keep the transient index in sync as a cache.
            editor
                .collab
                .shared_kbs
                .insert(kb_id.clone(), joined_node_ids);

            // ADR-023: learn THIS peer's current authorization epoch for the KB from
            // the (full) collection snapshot, so subsequent node edits are authored
            // under the current-epoch client_id the daemon's fence expects. This is
            // also the post-rebase relearn point: a "rebase required" rejection
            // triggers a re-join, which re-delivers collection_state with the bumped
            // epoch. Absent/0 ⇒ fresh grant ⇒ the legacy base client_id (no change).
            if !editor.collab.local_fingerprint.is_empty() {
                if let Ok(coll) = mae_sync::kb::KbCollectionDoc::from_bytes(&collection_state) {
                    let epoch = coll.epoch_of(&editor.collab.local_fingerprint);
                    if epoch == 0 {
                        editor.collab.kb_epochs.remove(&kb_id);
                    } else {
                        editor.collab.kb_epochs.insert(kb_id.clone(), epoch);
                    }
                    debug!(kb = %kb_id, epoch, "learned KB authorization epoch (ADR-023)");
                }
            }

            // C1: seed the local collection replica from the full join snapshot so
            // subsequent live `kbc:` membership broadcasts can be applied as deltas
            // and the epoch relearned without a reconnect.
            editor
                .collab
                .kb_collection_state
                .insert(kb_id.clone(), collection_state);

            info!(kb = %kb_id, node_count, "KB join complete (merged)");
            editor.set_status(format!("Joined KB '{}' ({} nodes)", kb_id, node_count));
            refresh_collab_status_if_open(editor);
            editor.mark_full_redraw();
        }
        CollabEvent::KbLeft { kb_id } => {
            info!(kb = %kb_id, "left shared KB — local copy preserved");
            // Local KB nodes persist after leaving (local-first principle).
            // Only stop receiving further updates.
            editor.collab.shared_kbs.remove(&kb_id);
            // C1: drop the collection replica + learned epoch — we no longer track
            // this KB's membership (a later rejoin re-seeds from a fresh snapshot).
            editor.collab.kb_collection_state.remove(&kb_id);
            editor.collab.kb_epochs.remove(&kb_id);
            editor.set_status(format!("Left KB '{}' (local copy preserved)", kb_id));
            refresh_collab_status_if_open(editor);
            editor.mark_full_redraw();
        }
        CollabEvent::KbNodeUpdate {
            kb_id,
            node_id,
            update_bytes,
        } => {
            debug!(
                kb = %kb_id,
                node = %node_id,
                update_len = update_bytes.len(),
                "remote KB node update — applying"
            );
            // ADR-019: route to the OWNING KB (instance or primary), not always
            // primary. The node-id namespace prefix hints the target instance
            // for a node not yet present locally.
            let hint = node_id.split(':').next().filter(|p| !p.is_empty());
            match editor.kb_apply_remote_update(&node_id, &update_bytes, hint) {
                Ok(changed) => {
                    if changed {
                        debug!(kb = %kb_id, node = %node_id, "KB node content changed by remote update");
                        editor.mark_full_redraw();
                    }
                }
                Err(e) => {
                    warn!(kb = %kb_id, node = %node_id, error = %e, "failed to apply remote KB node update");
                }
            }
        }
        CollabEvent::KbUpdateRequeue {
            kb_id,
            node_id,
            update,
            pending_rowid,
        } => {
            // ADR-020 durability: the bg task couldn't put this kb/node_update on the
            // wire — never silently lost (B-8). A store-backed row still EXISTS (the
            // drain is non-destructive and acks only on daemon-confirm), so we must
            // NOT re-persist it (that would duplicate the row) — just release the
            // in-flight mark so the next drain retries the same row. A transient
            // in-memory update has no durable row, so re-push it to the in-mem queue.
            match pending_rowid {
                Some(rowid) => {
                    editor.collab.inflight_kb_updates.remove(&rowid);
                    tracing::debug!(target: "kb_sync", kb = %kb_id, node = %node_id, rowid, "requeue: released in-flight durable row for retry");
                }
                None => {
                    editor
                        .collab
                        .pending_kb_updates
                        .push((kb_id, node_id, update));
                }
            }
        }
        CollabEvent::KbUpdateAcked { rowid } => {
            // ADR-020 queue→send→confirm→**ack**: the daemon confirmed apply, so now
            // remove the durable row and clear the in-flight mark.
            editor.collab.inflight_kb_updates.remove(&rowid);
            if let Some(ref store) = editor.kb.store {
                if let Err(e) = store.ack_pending_update(rowid) {
                    warn!(target: "kb_sync", rowid, error = %e, "ack: failed to remove confirmed pending kb update");
                } else {
                    tracing::debug!(target: "kb_sync", rowid, "ack: durable pending kb update confirmed + removed");
                }
            }
        }
        CollabEvent::KbUpdateFailed {
            kb_id,
            node_id,
            rowid,
            message,
        } => {
            // The daemon rejected the update (e.g. access denied / malformed). Surface
            // loudly and drop the durable row — retrying the identical update would not
            // succeed. (A future ReplicationFailed status surfaces this in the UI.)
            //
            // ADR-023: a "rebase required" rejection is the epoch fence firing — this
            // op was authored under a stale (pre-grant) authorization epoch and can
            // NEVER be accepted as-is (that is the whole point: a pre-grant divergent
            // lineage must not cascade). The security guarantee holds regardless of
            // what we do here (the daemon enforced it); we drop the row so it isn't
            // retried forever, relearn our epoch on the next (re)join, and tell the
            // user their pre-grant edit was not synced and must be re-applied. (The
            // graceful auto-adopt + re-author under the current epoch is tracked as a
            // follow-up; it needs a targeted node-adopt primitive.)
            let is_rebase = is_epoch_fence_rejection(&message);
            if is_rebase {
                warn!(target: "kb_sync", kb = %kb_id, node = %node_id, error = %message, "kb/node_update fenced (stale-epoch) — pre-grant edit not synced (B-19)");
                use mae_core::notifications::{NotifCommand, Notification};
                // P4: `collab_fence_resolution = auto` resolves the fence in the
                // background — adopt the authoritative state + re-author the local
                // edit under the current epoch (the keep-mine path), no prompt.
                // Default `prompt` keeps the user in the loop (#10).
                if editor.collab.fence_resolution == "auto" {
                    if editor.notify_collab_keep_mine(&kb_id, &node_id) {
                        editor.notify(Notification::info(
                            "collab",
                            format!(
                                "KB '{kb_id}': access changed — re-applying your edit to {node_id} under updated authorization"
                            ),
                        ));
                    }
                    if let Some(rowid) = rowid {
                        editor.collab.inflight_kb_updates.remove(&rowid);
                        if let Some(ref store) = editor.kb.store {
                            let _ = store.ack_pending_update(rowid);
                        }
                    }
                    editor.mark_full_redraw();
                    return;
                }
                // ADR-024 R2: raise an actionable, non-clobberable notification (badge
                // + *Notifications* row) instead of a status line that gets drowned out.
                // The actions invoke the R1 adopt-and-re-author round-trip.
                editor.notify(
                    Notification::action_required(
                        "collab",
                        format!("KB '{kb_id}': your edit to {node_id} wasn't synced"),
                    )
                    .key(format!("collab:fence:{kb_id}:{node_id}"))
                    .body(
                        "You edited this node before your access to the KB changed, so the \
                         server rejected the edit. Choose what to do: Accept-remote replaces \
                         your copy with the current shared version (your edit is lost); \
                         Keep-mine re-applies your edit on top of the current version; Stash \
                         externally saves your version to a separate file first.",
                    )
                    .action(
                        "Accept-remote (clobber local)",
                        NotifCommand::AdoptRemote {
                            kb_id: kb_id.clone(),
                            node_id: node_id.clone(),
                        },
                    )
                    .action(
                        "Keep-mine (re-author)",
                        NotifCommand::KeepMine {
                            kb_id: kb_id.clone(),
                            node_id: node_id.clone(),
                        },
                    )
                    .action(
                        "Stash externally",
                        NotifCommand::StashExternally {
                            kb_id: kb_id.clone(),
                            node_id: node_id.clone(),
                        },
                    ),
                );
            } else {
                warn!(target: "kb_sync", kb = %kb_id, node = %node_id, error = %message, "kb/node_update failed — dropping");
                editor.set_status(format!("KB sync rejected for {node_id}: {message}"));
            }
            if let Some(rowid) = rowid {
                editor.collab.inflight_kb_updates.remove(&rowid);
                if let Some(ref store) = editor.kb.store {
                    let _ = store.ack_pending_update(rowid);
                }
            }
        }
        CollabEvent::KbNodeAdopted {
            kb_id,
            node_id,
            state_bytes,
        } => {
            // ADR-024 R1: replace the local node with the daemon's authoritative
            // state (dropping the fenced stale-epoch op), then — if keep-mine
            // captured the user's edit — re-apply it under the current epoch so it
            // converges as a fresh, authorized op.
            match editor.kb_adopt_node(&node_id, &state_bytes) {
                Ok(_) => {
                    let kept = editor
                        .collab
                        .pending_reauthor
                        .remove(&(kb_id.clone(), node_id.clone()));
                    if let Some(f) = kept {
                        match editor.kb_update_node(
                            &node_id,
                            Some(&f.title),
                            Some(&f.body),
                            Some(f.tags),
                        ) {
                            Ok(()) => {
                                editor.notify(mae_core::notifications::Notification::success(
                                    "collab",
                                    format!(
                                        "Re-applied your edit to {node_id} under current access"
                                    ),
                                ));
                            }
                            Err(e) => {
                                editor.set_status(format!("Re-author failed for {node_id}: {e}"))
                            }
                        }
                    } else {
                        editor.notify(mae_core::notifications::Notification::success(
                            "collab",
                            format!("Adopted authoritative {node_id} (local changes discarded)"),
                        ));
                    }
                    editor.mark_full_redraw();
                }
                Err(e) => {
                    editor
                        .collab
                        .pending_reauthor
                        .remove(&(kb_id, node_id.clone()));
                    editor.set_status(format!("Adopt failed for {node_id}: {e}"));
                }
            }
        }
    }
}

// --- Background task ---

/// Deferred spawn state — holds the background task's channel ends and config.
/// Created by `setup_collab_channels`, consumed by `spawn_collab_task`.
pub(crate) struct CollabSpawn {
    cmd_rx: mpsc::Receiver<CollabCommand>,
    evt_tx: mpsc::Sender<CollabEvent>,
    reconnect_secs: u64,
    write_timeout_ms: u64,
    auto_connect_addr: Option<String>,
    cmd_tx_clone: mpsc::Sender<CollabCommand>,
    backoff_factor: u64,
    max_reconnect_attempts: u64,
    heartbeat_secs: u64,
    /// How this editor authenticates to the daemon (psk / key+mTLS / key+JSON).
    transport: ClientTransport,
}

/// Resolve the client collab credential: `(psk_or_cmd_sentinel, key_id)`.
///
/// Precedence:
///   1. `psk_command` (legacy) — returned as a `cmd:` sentinel, resolved async
///      in `run_collab_task`. No `key_id`.
///   2. `psk` (legacy plaintext). No `key_id`.
///   3. the trusted-keys keystore at `keystore_path` — present its primary key,
///      advertising the key's name as the wire `key_id` so a multi-key daemon
///      can select it.
///   4. nothing configured → empty secret (no-auth).
///
/// Pure (the only I/O is reading `keystore_path`) so it is unit-testable with a
/// temp path or `None`.
fn resolve_client_credential(
    psk_command: &str,
    psk: &str,
    keystore_path: Option<&std::path::Path>,
) -> (String, Option<String>) {
    if !psk_command.is_empty() {
        (format!("cmd:{psk_command}"), None)
    } else if !psk.is_empty() {
        (psk.to_string(), None)
    } else if let Some(entry) = keystore_path
        .and_then(|p| mae_mcp::keystore::load_optional(p).ok().flatten())
        .and_then(|ks| ks.primary().cloned())
    {
        (entry.secret, entry.name)
    } else {
        (String::new(), None)
    }
}

/// Create collab channels and read config. Does NOT require a tokio runtime.
/// Returns `(event_rx, command_tx, spawn)` — caller must pass `spawn` to
/// `spawn_collab_task()` from within a tokio runtime context.
pub(crate) fn setup_collab_channels(
    editor: &Editor,
) -> (
    mpsc::Receiver<CollabEvent>,
    mpsc::Sender<CollabCommand>,
    CollabSpawn,
) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<CollabCommand>(COLLAB_CMD_CHANNEL_CAP);
    let (evt_tx, evt_rx) = mpsc::channel::<CollabEvent>(COLLAB_EVT_CHANNEL_CAP);

    let reconnect_secs = editor.collab.reconnect_interval;
    let write_timeout_ms = editor.collab.write_timeout_ms;

    let auto_connect_addr =
        if editor.collab.auto_connect && !editor.collab.server_address.is_empty() {
            Some(editor.collab.server_address.clone())
        } else {
            None
        };

    let backoff_factor = editor.collab.reconnect_backoff_factor;
    let max_reconnect_attempts = editor.collab.max_reconnect_attempts;
    let heartbeat_secs = editor.collab.heartbeat_interval;

    let transport = resolve_client_transport(editor, &evt_tx);

    let spawn = CollabSpawn {
        cmd_rx,
        evt_tx,
        reconnect_secs,
        write_timeout_ms,
        auto_connect_addr,
        cmd_tx_clone: cmd_tx.clone(),
        backoff_factor,
        max_reconnect_attempts,
        heartbeat_secs,
        transport,
    };

    (evt_rx, cmd_tx, spawn)
}

/// Resolve how the editor authenticates to the daemon from `collab_*` options.
///
/// - `auth_mode = "key"` → load this editor's Ed25519 identity + a
///   `known_hosts` verifier (TOFU policy from `collab_host_key_policy`); use
///   mTLS unless `collab_tls = false` (then the JSON KeyAuth fallback).
/// - otherwise → PSK / none (see `resolve_client_credential`).
fn resolve_client_transport(
    editor: &Editor,
    evt_tx: &mpsc::Sender<CollabEvent>,
) -> ClientTransport {
    if editor.collab.auth_mode == "key" {
        if let Some(dir) = mae_mcp::identity::default_collab_dir() {
            let label = hostname::get()
                .ok()
                .and_then(|h| h.into_string().ok())
                .unwrap_or_else(|| "mae-editor".to_string());
            match mae_mcp::identity::Identity::load_or_generate(&dir, &label) {
                Ok(id) => {
                    let known_hosts = dir.join("known_hosts");
                    // B-21: the verifier reads the LIVE policy at verify-time (via the
                    // shared cell), so a runtime `:set collab-host-key-policy` is honored
                    // on the next connect without a relaunch — the transport is otherwise
                    // built once at collab-task setup and cached. Seed the cell from the
                    // current option value (covers config-load paths that set the field
                    // directly), then hand the verifier a clone of the Arc. We always use
                    // the prompting verifier because it is the only one that CAN prompt;
                    // it dispatches on the live policy (accept-new → pin, strict → reject,
                    // prompt → ask the user).
                    if let Ok(mut p) = editor.collab.host_key_policy_live.lock() {
                        *p = editor.collab.host_key_policy.clone();
                    }
                    let verifier: std::sync::Arc<dyn mae_mcp::identity::HostKeyVerifier> =
                        std::sync::Arc::new(PromptingHostKeyVerifier {
                            known_hosts,
                            evt_tx: evt_tx.clone(),
                            timeout: std::time::Duration::from_secs(120),
                            policy: editor.collab.host_key_policy_live.clone(),
                        });
                    let identity = std::sync::Arc::new(id);
                    return if editor.collab.tls {
                        ClientTransport::KeyTls { identity, verifier }
                    } else {
                        ClientTransport::KeyJson { identity, verifier }
                    };
                }
                Err(e) => {
                    warn!(error = %e, "failed to load collab identity; falling back to no-auth");
                }
            }
        }
    }
    let (psk, key_id) = resolve_client_credential(
        &editor.collab.psk_command,
        &editor.collab.psk,
        mae_mcp::keystore::default_keystore_path().as_deref(),
    );
    ClientTransport::Plain { psk, key_id }
}

/// Spawn the collab background task. MUST be called from within a tokio runtime.
pub(crate) fn spawn_collab_task(spawn: CollabSpawn) {
    let write_timeout = std::time::Duration::from_millis(spawn.write_timeout_ms);
    tokio::spawn(run_collab_task(
        spawn.cmd_rx,
        spawn.evt_tx,
        spawn.reconnect_secs,
        write_timeout,
        spawn.backoff_factor,
        spawn.max_reconnect_attempts,
        spawn.heartbeat_secs,
        spawn.transport,
    ));

    // Auto-connect if configured
    if let Some(addr) = spawn.auto_connect_addr {
        let _ = spawn
            .cmd_tx_clone
            .try_send(CollabCommand::Connect { address: addr });
    }
}

/// Kinds of pending request-response correlations.
#[derive(Debug)]
pub(crate) enum PendingResponseKind {
    ListDocs {
        for_join: bool,
    },
    JoinDoc {
        doc_id: String,
    },
    ShareBuffer {
        doc_id: String,
    },
    ForceSync {
        doc_id: String,
    },
    SyncUpdate {
        doc_id: String,
    },
    SaveIntent {
        doc_id: String,
        expected_hash: String,
    },
    Subscribe,
    Ping {
        sent_at: std::time::Instant,
    },
    /// Doctor ping — response forwarded to a oneshot channel.
    DoctorPing {
        sent_at: std::time::Instant,
        reply: tokio::sync::oneshot::Sender<Option<u64>>,
    },
    /// Doctor debug — response forwarded to a oneshot channel.
    DoctorDebug {
        reply: tokio::sync::oneshot::Sender<Option<serde_json::Value>>,
    },
    KbShare {
        kb_id: String,
    },
    KbJoin {
        kb_id: String,
    },
    /// ADR-024 R1: response to `kb/node_fetch` — carries `{state, sv}` to adopt.
    KbAdoptNode {
        kb_id: String,
        node_id: String,
    },
    KbLeave {
        kb_id: String,
    },
    KbMember {
        kb_id: String,
        member: String,
        add: bool,
    },
    /// Awaiting the daemon's apply-confirmation for a `kb/node_update`. On success
    /// (`{applied:true}`) the durable SQLite row `pending_rowid` is acked; on an
    /// error response it is surfaced loudly and dropped (queue→send→confirm→ack).
    KbNodeUpdate {
        kb_id: String,
        node_id: String,
        pending_rowid: Option<i64>,
    },
}

/// Spawn a dedicated reader task that feeds complete messages into an mpsc channel.
///
/// CANCEL-SAFETY: `read_message()` uses multi-step I/O (peek → read headers → read body).
/// If `tokio::select!` cancels it mid-parse, the BufReader's internal cursor is left past
/// partially-consumed data, corrupting all subsequent reads. By running `read_message` in
/// a dedicated task that is never cancelled, we ensure it always runs to completion.
/// The `select!` loop receives from the channel, which is always cancel-safe.
/// A type-erased read half — TCP or TLS — so the connection loop is uniform.
type BoxReader = Box<dyn tokio::io::AsyncBufRead + Unpin + Send>;
/// A type-erased write half — TCP or TLS.
type BoxWriter = Box<dyn tokio::io::AsyncWrite + Unpin + Send>;

/// How the editor authenticates to the daemon. Resolved once from options.
enum ClientTransport {
    /// Plaintext TCP. `psk` empty = no auth; otherwise PSK handshake (may be a
    /// `cmd:` sentinel resolved at task start). `key_id` names a keystore key.
    Plain { psk: String, key_id: Option<String> },
    /// Plaintext TCP + the JSON KeyAuth signed-challenge handshake (key mode,
    /// `collab_tls = false` fallback).
    KeyJson {
        identity: std::sync::Arc<mae_mcp::identity::Identity>,
        verifier: std::sync::Arc<dyn mae_mcp::identity::HostKeyVerifier>,
    },
    /// Native mTLS (key mode, default) — identity + pinning are the TLS layer.
    KeyTls {
        identity: std::sync::Arc<mae_mcp::identity::Identity>,
        verifier: std::sync::Arc<dyn mae_mcp::identity::HostKeyVerifier>,
    },
}

impl ClientTransport {
    /// The PSK (or `cmd:` sentinel) for a Plain transport — test inspection.
    #[cfg(test)]
    fn plain_psk(&self) -> Option<&str> {
        match self {
            ClientTransport::Plain { psk, .. } => Some(psk.as_str()),
            _ => None,
        }
    }
}

/// A `HostKeyVerifier` that prompts the user interactively (TOFU) for an unknown
/// daemon identity, via a round-trip to the main thread. A previously pinned key
/// that matches is accepted silently; a CHANGED key is rejected (MITM defense).
/// The editor's host-key verifier. Unlike the non-interactive
/// `FileHostKeyVerifier`, this one can prompt — and it reads the trust policy
/// from a **live** shared cell at verify-time (B-21), so a runtime
/// `:set collab-host-key-policy` is honored on the next connect (the transport
/// is built once at collab-task setup and cached). Dispatches on the live policy:
/// `accept-new` → pin on first use, `strict` → reject unknown, `prompt` → ask.
struct PromptingHostKeyVerifier {
    known_hosts: std::path::PathBuf,
    evt_tx: mpsc::Sender<CollabEvent>,
    timeout: std::time::Duration,
    /// Live mirror of `collab_host_key_policy` (shared with the editor).
    policy: std::sync::Arc<std::sync::Mutex<String>>,
}

impl std::fmt::Debug for PromptingHostKeyVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptingHostKeyVerifier")
            .field("known_hosts", &self.known_hosts)
            .finish_non_exhaustive()
    }
}

impl PromptingHostKeyVerifier {
    /// The current trust policy (read live each verify so runtime changes apply).
    fn current_policy(&self) -> mae_mcp::identity::HostKeyPolicy {
        let s = self
            .policy
            .lock()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "prompt".to_string());
        mae_mcp::identity::HostKeyPolicy::from_str_opt(&s)
    }
}

impl mae_mcp::identity::HostKeyVerifier for PromptingHostKeyVerifier {
    fn verify(&self, addr: &str, server_pub: &mae_mcp::identity::PublicKey) -> bool {
        let mut kh = mae_mcp::identity::KnownHosts::load(&self.known_hosts);
        if let Some(pinned) = kh.get(addr) {
            // Known host: pin-match (MITM defense) regardless of policy.
            if pinned.to_bytes() == server_pub.to_bytes() {
                return true;
            }
            tracing::error!(
                addr,
                expected = %pinned.fingerprint(),
                got = %server_pub.fingerprint(),
                "daemon host key CHANGED — aborting (possible MITM)"
            );
            return false;
        }
        // Unknown host — dispatch on the LIVE policy.
        match self.current_policy() {
            mae_mcp::identity::HostKeyPolicy::AcceptNew => match kh.pin(addr, server_pub) {
                Ok(_) => {
                    info!(addr, fp = %server_pub.fingerprint(), "pinned new daemon host key (accept-new)");
                    true
                }
                Err(e) => {
                    warn!(addr, error = %e, "failed to pin host key");
                    false
                }
            },
            mae_mcp::identity::HostKeyPolicy::Strict => {
                warn!(addr, fp = %server_pub.fingerprint(), "unknown host key rejected (strict)");
                false
            }
            mae_mcp::identity::HostKeyPolicy::Prompt => {
                // Ask the user (the connection task blocks here; the main UI thread
                // is separate, so no deadlock).
                let (reply_tx, reply_rx) = std::sync::mpsc::channel::<bool>();
                if self
                    .evt_tx
                    .try_send(CollabEvent::HostKeyPrompt {
                        addr: addr.to_string(),
                        fingerprint: server_pub.fingerprint(),
                        reply: reply_tx,
                    })
                    .is_err()
                {
                    warn!("host-key prompt could not be delivered; rejecting");
                    return false;
                }
                match reply_rx.recv_timeout(self.timeout) {
                    Ok(true) => {
                        tracing::debug!(target: "collab", addr, fp = %server_pub.fingerprint(), "host-key trusted by user — pinning");
                        kh.pin(addr, server_pub).is_ok()
                    }
                    Ok(false) => {
                        tracing::debug!(target: "collab", addr, "host-key rejected by user — aborting");
                        false
                    }
                    Err(_) => {
                        tracing::debug!(target: "collab", addr, "host-key prompt timed out — aborting");
                        false
                    }
                }
            }
        }
    }
}

/// Connect to `addr` and run the auth handshake, returning boxed read/write
/// halves ready for `send_initialize`. The trust decision (TOFU / authorized
/// peer) happens inside the handshake.
async fn establish_connection(
    addr: &str,
    transport: &ClientTransport,
) -> Result<(BoxReader, BoxWriter), String> {
    use tokio::io::BufReader;
    use tokio::net::TcpStream;
    match transport {
        ClientTransport::KeyTls { identity, verifier } => {
            let cfg = mae_mcp::tls::client_config(identity, addr.to_string(), verifier.clone())?;
            let stream = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
            let connector = mae_mcp::tls::TlsConnector::from(cfg);
            let server_name =
                mae_mcp::tls::ServerName::try_from(mae_mcp::tls::SNI).map_err(|e| e.to_string())?;
            let tls = connector
                .connect(server_name, stream)
                .await
                .map_err(|e| format!("TLS handshake failed: {e}"))?;
            let (r, w) = tokio::io::split(tls);
            Ok((
                Box::new(BufReader::new(r)) as BoxReader,
                Box::new(w) as BoxWriter,
            ))
        }
        ClientTransport::KeyJson { identity, verifier } => {
            use mae_mcp::auth::{AuthProvider, KeyAuth};
            let stream = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
            let (r, mut w) = stream.into_split();
            let mut br = BufReader::new(r);
            let auth = KeyAuth::client(identity.clone(), addr.to_string(), verifier.clone());
            auth.client_handshake(&mut br, &mut w)
                .await
                .map_err(|e| format!("key auth failed: {e}"))?;
            Ok((Box::new(br) as BoxReader, Box::new(w) as BoxWriter))
        }
        ClientTransport::Plain { psk, key_id } => {
            let stream = TcpStream::connect(addr).await.map_err(|e| e.to_string())?;
            let (r, mut w) = stream.into_split();
            let mut br = BufReader::new(r);
            perform_psk_auth(&mut br, &mut w, psk, key_id.as_deref()).await?;
            Ok((Box::new(br) as BoxReader, Box::new(w) as BoxWriter))
        }
    }
}

fn spawn_reader_task<R: tokio::io::AsyncBufRead + Unpin + Send + 'static>(
    reader: R,
) -> mpsc::Receiver<Result<String, String>> {
    let (msg_tx, msg_rx) = mpsc::channel::<Result<String, String>>(32);
    tokio::spawn(async move {
        let mut reader = reader;
        loop {
            match mae_mcp::read_message(&mut reader).await {
                Ok(Some(msg)) => {
                    if msg_tx.send(Ok(msg)).await.is_err() {
                        break; // main task dropped the receiver
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
    msg_rx
}

/// Build a `kb/node_update` JSON-RPC request, signing the ADR-036 authorship header
/// when a key-mode `signing_identity` is present (else the legacy unsigned form).
///
/// This is where the editor's **sign-on-push** (ADR-036 §D2) lives — kept a pure
/// function out of the network loop so it is directly unit-testable. The author is
/// this editor's own fingerprint; `epoch` is the KB's authorization epoch captured
/// on the editor thread (carried in `CollabCommand::KbNodeUpdate`). `base_sv` is left
/// empty: per ADR-036 §D4 the yrs SV + the daemon's epoch fence carry replay safety,
/// so the signature binds author + epoch + payload, and a node-SV binding is a
/// documented future refinement (not a security boundary here).
#[allow(clippy::too_many_arguments)]
fn build_kb_node_update_request(
    req_id: u64,
    kb_id: &str,
    node_id: &str,
    update: &[u8],
    epoch: u64,
    signing_identity: Option<&std::sync::Arc<mae_mcp::identity::Identity>>,
    content_key: Option<&mae_sync::content_crypto::ContentKey>,
    op_set_state: &[u8],
) -> (serde_json::Value, Vec<u8>) {
    // ADR-037 E2E (#146): on an encrypted KB, the PLAINTEXT node update is sealed into
    // the op-set — encrypt under the content key, build the outer YMap-insert op
    // (stamped with the epoch client_id so the daemon's ADR-023 fence still authorizes
    // it) — and THAT outer op becomes the wire payload + what's signed (encrypt-then-
    // sign: the daemon verifies authorship over the ciphertext-bearing op, key-blind).
    // The returned op-set state advances by the sealed op. Without a content key the
    // plaintext update is the payload (today's behaviour) and the state is unchanged.
    let (payload, new_op_set_state): (Vec<u8>, Vec<u8>) = match (content_key, signing_identity) {
        (Some(key), Some(id)) => {
            let client_id = mae_sync::kb::derive_kb_client_id(&id.fingerprint(), epoch);
            match mae_sync::op_set::seal_op(op_set_state, key, update, client_id) {
                Ok((_op_id, outer)) => {
                    let merged = mae_sync::op_set::merge(op_set_state, &outer)
                        .unwrap_or_else(|_| op_set_state.to_vec());
                    (outer, merged)
                }
                // Sealing can't fail for a valid op-set, but fail safe: keep the
                // plaintext payload rather than dropping the edit.
                Err(_) => (update.to_vec(), op_set_state.to_vec()),
            }
        }
        _ => (update.to_vec(), op_set_state.to_vec()),
    };

    let payload_b64 = mae_sync::encoding::update_to_base64(&payload);
    let request = match signing_identity {
        Some(id) => {
            let op = mae_sync::content_ops::ContentOp {
                kb_id: kb_id.to_string(),
                node_id: node_id.to_string(),
                base_sv: Vec::new(),
                author: id.fingerprint(),
                epoch,
                issued_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            };
            let sig = op.sign(&id.secret_bytes(), &payload);
            let signed = mae_sync::content_ops::SignedContentOp {
                op,
                payload: payload.clone(),
                sig,
                author_pubkey: id.public().to_bytes(),
            };
            mae_sync::wire::kb_node_update_request_signed(
                req_id,
                kb_id,
                node_id,
                &payload_b64,
                signed.header_params(),
            )
        }
        None => mae_sync::wire::kb_node_update_request(req_id, kb_id, node_id, &payload_b64),
    };
    (request, new_op_set_state)
}

/// Background task that owns the TCP connection to the state server.
///
/// Receives commands from the main thread, manages the connection lifecycle,
/// and forwards events back.
#[allow(clippy::too_many_arguments)]
async fn run_collab_task(
    mut cmd_rx: mpsc::Receiver<CollabCommand>,
    evt_tx: mpsc::Sender<CollabEvent>,
    reconnect_secs: u64,
    write_timeout: std::time::Duration,
    backoff_factor: u64,
    max_reconnect_attempts: u64,
    heartbeat_secs: u64,
    transport: ClientTransport,
) {
    use mae_mcp::write_framed;
    use std::collections::HashMap;

    // Resolve a `cmd:` PSK sentinel once (run the command to get the key).
    let transport = match transport {
        ClientTransport::Plain { psk, key_id } => {
            let psk = if let Some(cmd) = psk.strip_prefix("cmd:") {
                mae_mcp::auth::load_psk(Some(cmd), None)
                    .await
                    .unwrap_or_default()
            } else {
                psk
            };
            ClientTransport::Plain { psk, key_id }
        }
        other => other,
    };
    // ADR-036 D2: the signing identity for content ops — present only in key mode
    // (the editor holds its Ed25519 key for mTLS/JSON-key auth). psk/none mode has
    // no per-identity key, so its ops stay unsigned (the legacy/hub path).
    let signing_identity: Option<std::sync::Arc<mae_mcp::identity::Identity>> = match &transport {
        ClientTransport::KeyTls { identity, .. } | ClientTransport::KeyJson { identity, .. } => {
            Some(identity.clone())
        }
        ClientTransport::Plain { .. } => None,
    };
    match &transport {
        ClientTransport::KeyTls { .. } => {
            info!(
                auth = "key",
                tls = true,
                "mTLS authentication enabled for collab"
            )
        }
        ClientTransport::KeyJson { .. } => {
            info!(
                auth = "key",
                tls = false,
                "KeyAuth authentication enabled for collab"
            )
        }
        ClientTransport::Plain { psk, .. } if !psk.is_empty() => {
            info!(auth = "psk", "PSK authentication enabled for collab")
        }
        _ => {}
    }

    let mut msg_rx: Option<mpsc::Receiver<Result<String, String>>> = None;
    let mut writer: Option<BoxWriter> = None;
    let mut target_address: Option<String> = None;
    let mut shared_docs: Vec<String> = Vec::new();
    let mut reconnect_enabled = false;
    let mut reconnect_attempt: u32 = 0;
    // ForceSync debounce: track last force-sync time per doc.
    let mut last_force_sync: std::collections::HashMap<String, std::time::Instant> =
        std::collections::HashMap::new();
    let mut next_request_id: u64 = 10; // Start after handshake IDs
    let mut pending_responses: HashMap<u64, PendingResponseKind> = HashMap::new();
    // WU1: Track wal_seq per doc for gap detection.
    let mut seq_tracker: HashMap<String, u64> = HashMap::new();
    // WU6: Transport health counter for periodic diagnostics.
    let mut messages_received: u64 = 0;
    // WU2: Heartbeat interval (from collab_heartbeat_interval option, disabled if 0).
    let mut heartbeat_interval =
        tokio::time::interval(std::time::Duration::from_secs(if heartbeat_secs > 0 {
            heartbeat_secs
        } else {
            3600
        }));
    heartbeat_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the first immediate tick.
    heartbeat_interval.tick().await;
    let mut ping_pending = false;

    /// Helper: tear down connection.
    /// Dropping msg_rx causes the reader task to terminate on its next send.
    fn tear_down(
        rx: &mut Option<mpsc::Receiver<Result<String, String>>>,
        wr: &mut Option<BoxWriter>,
    ) {
        *rx = None;
        *wr = None;
    }

    loop {
        let connected = msg_rx.is_some();

        if connected {
            let rx = msg_rx.as_mut().unwrap();

            tokio::select! {
                Some(cmd) = cmd_rx.recv() => {
                    debug!(cmd = ?std::mem::discriminant(&cmd),
                        "bridge: received command");
                    match cmd {
                        CollabCommand::Disconnect => {
                            tear_down(&mut msg_rx, &mut writer);
                            reconnect_enabled = false;
                            shared_docs.clear();
                            pending_responses.clear();
                            try_send_evt(&evt_tx, CollabEvent::Disconnected {
                                reason: "user requested".to_string(),
                            });
                            continue;
                        }
                        CollabCommand::ShowStatus => {
                            let lines = build_status_lines(
                                target_address.as_deref().unwrap_or("?"),
                                true,
                                &shared_docs,
                            );
                            try_send_evt(&evt_tx, CollabEvent::StatusReport { lines });
                        }
                        CollabCommand::Doctor { synced_info } => {
                            let addr = target_address.as_deref().unwrap_or("?").to_string();
                            // Route doctor queries through the normal request/response
                            // mechanism using oneshot channels, so we don't need direct
                            // reader access (which is now in the reader task).
                            let (ping_tx, ping_rx) = tokio::sync::oneshot::channel();
                            let (debug_tx, debug_rx) = tokio::sync::oneshot::channel();
                            if let Some(ref mut w) = writer {
                                // Send $/ping
                                let ping_id = next_request_id;
                                next_request_id += 1;
                                let ping_req = serde_json::json!({
                                    "jsonrpc": "2.0", "id": ping_id, "method": "$/ping",
                                });
                                let body = serde_json::to_vec(&ping_req).unwrap_or_default();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(ping_id, PendingResponseKind::DoctorPing {
                                        sent_at: std::time::Instant::now(),
                                        reply: ping_tx,
                                    });
                                } else {
                                    let _ = ping_tx.send(None);
                                }
                                // Send $/debug
                                let debug_id = next_request_id;
                                next_request_id += 1;
                                let debug_req = serde_json::json!({
                                    "jsonrpc": "2.0", "id": debug_id, "method": "$/debug",
                                });
                                let body = serde_json::to_vec(&debug_req).unwrap_or_default();
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(debug_id, PendingResponseKind::DoctorDebug {
                                        reply: debug_tx,
                                    });
                                } else {
                                    let _ = debug_tx.send(None);
                                }
                            } else {
                                let _ = ping_tx.send(None);
                                let _ = debug_tx.send(None);
                            }
                            // Spawn a task to await responses and build the doctor report.
                            let evt_tx_clone = evt_tx.clone();
                            tokio::spawn(async move {
                                let gather_timeout = std::time::Duration::from_secs(2);
                                let ping_latency_ms = tokio::time::timeout(gather_timeout, ping_rx)
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .flatten();
                                let server_debug = tokio::time::timeout(gather_timeout, debug_rx)
                                    .await
                                    .ok()
                                    .and_then(|r| r.ok())
                                    .flatten();
                                let ctx = DoctorContext {
                                    address: addr,
                                    connected: true,
                                    server_debug,
                                    ping_latency_ms,
                                    synced_info,
                                };
                                let lines = build_doctor_lines(&ctx);
                                try_send_evt(&evt_tx_clone, CollabEvent::DoctorReport { lines });
                            });
                        }
                        CollabCommand::ShareBuffer { doc_id, state_bytes } => {
                            if let Some(ref mut w) = writer {
                                // Atomic share: server deletes old doc + applies update in one step.
                                let update_b64 = mae_sync::encoding::update_to_base64(&state_bytes);
                                info!(doc = %doc_id, state_len = state_bytes.len(), b64_len = update_b64.len(), "share: sending sync/share");
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/share",
                                    "params": {
                                        "doc": doc_id,
                                        "update": update_b64,
                                    }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                match write_framed(w, &body, write_timeout).await {
                                    Ok(()) => {
                                        info!(doc = %doc_id, req_id, body_len = body.len(),
                                            "share: write_framed completed successfully");
                                        pending_responses.insert(req_id, PendingResponseKind::ShareBuffer { doc_id });
                                    }
                                    Err(e) => {
                                        error!(doc = %doc_id, error = %e,
                                            "share: write_framed failed");
                                        try_send_evt(&evt_tx, CollabEvent::Error {
                                            message: format!("Failed to share {}", doc_id),
                                        });
                                    }
                                }
                            }
                        }
                        CollabCommand::ForceSync { doc_id } => {
                            // Debounce: skip if we sent ForceSync for this doc within 2s.
                            let now = std::time::Instant::now();
                            if let Some(last) = last_force_sync.get(&doc_id) {
                                if now.duration_since(*last).as_secs() < 2 {
                                    debug!(doc = %doc_id, "ForceSync debounced (within 2s)");
                                    continue;
                                }
                            }
                            last_force_sync.insert(doc_id.clone(), now);
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/full_state",
                                    "params": { "doc": doc_id }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::ForceSync { doc_id });
                                } else {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: format!("Failed to sync {}", doc_id),
                                    });
                                }
                            }
                        }
                        CollabCommand::SendUpdate { doc_id, update_base64 } => {
                            info!(
                                doc = %doc_id,
                                update_b64_len = update_base64.len(),
                                "collab_bridge: SendUpdate → server"
                            );
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/update",
                                    "params": {
                                        "doc": doc_id,
                                        "update": update_base64,
                                    }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::SyncUpdate { doc_id });
                                }
                            }
                        }
                        CollabCommand::SendAwareness { doc_id, state_json } => {
                            // Fire-and-forget: awareness is ephemeral, no response needed.
                            if let Some(ref mut w) = writer {
                                let state_val: serde_json::Value = match serde_json::from_str(&state_json) {
                                    Ok(v) => v,
                                    Err(e) => { error!("awareness parse error: {e}"); continue; }
                                };
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "method": "sync/awareness",
                                    "params": {
                                        "doc": doc_id,
                                        "state": state_val,
                                    }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("awareness serialize error: {e}"); continue; }
                                };
                                if let Err(e) = write_framed(w, &body, write_timeout).await {
                                    debug!(error = %e, "awareness send failed (non-fatal)");
                                }
                            }
                        }
                        CollabCommand::ListDocs { for_join } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "docs/list",
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::ListDocs { for_join });
                                } else {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "Failed to list documents".to_string(),
                                    });
                                }
                            }
                        }
                        CollabCommand::JoinDoc { doc_id } => {
                            info!(doc = %doc_id, "join: sending sync/resync");
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "sync/resync",
                                    "params": { "doc": doc_id },
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::JoinDoc { doc_id: doc_id.clone() });
                                    if !shared_docs.contains(&doc_id) {
                                        shared_docs.push(doc_id);
                                    }
                                } else {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: format!("Failed to join {}", doc_id),
                                    });
                                }
                            }
                        }
                        CollabCommand::SendSaveIntent { doc_id, expected_hash } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "docs/save_intent",
                                    "params": {
                                        "doc": doc_id,
                                        "expected_hash": expected_hash,
                                    }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::SaveIntent {
                                        doc_id,
                                        expected_hash,
                                    });
                                } else {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "Failed to send save intent".to_string(),
                                    });
                                }
                            }
                        }
                        CollabCommand::SendSaveCommitted { doc_id, save_epoch, content_hash, saved_by } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "docs/save_committed",
                                    "params": {
                                        "doc": doc_id,
                                        "save_epoch": save_epoch,
                                        "content_hash": content_hash,
                                        "saved_by": saved_by,
                                    }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("collab serialize error: {e}"); continue; }
                                };
                                // Fire-and-forget — no pending response tracking needed.
                                if write_framed(w, &body, write_timeout).await.is_err() {
                                    warn!(doc = %doc_id, "failed to send save_committed");
                                }
                            }
                        }
                        CollabCommand::ShareKb { kb_id, name, creator, collection_state, node_states } => {
                            info!(kb = %kb_id, nodes = node_states.len(), "sharing KB");
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let nodes: Vec<(String, String)> = node_states.iter().map(|(id, state)| {
                                    (id.clone(), mae_sync::encoding::update_to_base64(state))
                                }).collect();
                                let req = mae_sync::wire::kb_share_request(
                                    req_id,
                                    &kb_id,
                                    &name,
                                    &creator,
                                    &mae_sync::encoding::update_to_base64(&collection_state),
                                    &nodes,
                                );
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("kb share serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    // ADR-020 B-13: subscribe the OWNER to its own
                                    // collection + node docs so peer edits broadcast
                                    // back are applied, not dropped at the
                                    // shared_docs filter. (Mirror of the text-buffer
                                    // share path which adds the doc to shared_docs.)
                                    let coll_doc = format!("kbc:{kb_id}");
                                    if !shared_docs.contains(&coll_doc) {
                                        shared_docs.push(coll_doc);
                                    }
                                    for (id, _) in &node_states {
                                        let node_doc = format!("kb:{id}");
                                        if !shared_docs.contains(&node_doc) {
                                            shared_docs.push(node_doc);
                                        }
                                    }
                                    pending_responses.insert(req_id, PendingResponseKind::KbShare { kb_id });
                                }
                            }
                        }
                        CollabCommand::JoinKb { kb_id, node_svs } => {
                            info!(kb = %kb_id, node_sv_count = node_svs.len(), "joining KB (ADR-022 reconcile)");
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let svs_b64: Vec<(String, String)> = node_svs
                                    .iter()
                                    .map(|(id, sv)| {
                                        (id.clone(), mae_sync::encoding::update_to_base64(sv))
                                    })
                                    .collect();
                                let req = mae_sync::wire::kb_join_request(req_id, &kb_id, &svs_b64);
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("kb join serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::KbJoin { kb_id });
                                }
                            }
                        }
                        CollabCommand::KbAdoptNode { kb_id, node_id } => {
                            info!(kb = %kb_id, node = %node_id, "kb/node_fetch (adopt authoritative — ADR-024 R1)");
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req =
                                    mae_sync::wire::kb_node_fetch_request(req_id, &kb_id, &node_id);
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => {
                                        error!("kb node_fetch serialize error: {e}");
                                        continue;
                                    }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(
                                        req_id,
                                        PendingResponseKind::KbAdoptNode { kb_id, node_id },
                                    );
                                }
                            }
                        }
                        CollabCommand::LeaveKb { kb_id } => {
                            info!(kb = %kb_id, "leaving KB");
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": "kb/leave",
                                    "params": { "kb_id": kb_id }
                                });
                                let body = match serde_json::to_vec(&req) {
                                    Ok(b) => b,
                                    Err(e) => { error!("kb leave serialize error: {e}"); continue; }
                                };
                                if write_framed(w, &body, write_timeout).await.is_ok() {
                                    pending_responses.insert(req_id, PendingResponseKind::KbLeave { kb_id });
                                }
                            }
                        }
                        CollabCommand::KbNodeUpdate { kb_id, node_id, update, epoch, pending_rowid } => {
                            // ADR-020: a kb/node_update is a REQUEST (carries an `id`),
                            // built via the shared `mae_sync::wire` constructor so the
                            // editor and the daemon (+ tests) serialise identically. The
                            // daemon dispatches it to the apply+broadcast handler and
                            // replies `{applied:true}`; the durable row is acked only on
                            // that response (PendingResponseKind::KbNodeUpdate → ack).
                            // On write failure / no writer it is NEVER silently lost:
                            // re-queue (durable row released, or in-mem re-pushed).
                            let mut delivered = false;
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                // ADR-036 D2: sign the authorship header when we hold
                                // a key-mode identity (else legacy unsigned).
                                // ADR-037 §2b (TODO): pass the KB's content key + the
                                // node's op-set state here to encrypt on push; until
                                // wired, `None`/`&[]` keep the plaintext path.
                                let (req, _new_op_set) = build_kb_node_update_request(
                                    req_id,
                                    &kb_id,
                                    &node_id,
                                    &update,
                                    epoch,
                                    signing_identity.as_ref(),
                                    None,
                                    &[],
                                );
                                match serde_json::to_vec(&req) {
                                    Ok(body) => match write_framed(w, &body, write_timeout).await {
                                        Ok(()) => {
                                            delivered = true;
                                            pending_responses.insert(req_id, PendingResponseKind::KbNodeUpdate {
                                                kb_id: kb_id.clone(),
                                                node_id: node_id.clone(),
                                                pending_rowid,
                                            });
                                            tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, req_id, "bg: kb/node_update written to wire (awaiting apply-ack)");
                                        }
                                        Err(e) => warn!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, error = %e, "bg: kb/node_update wire write FAILED — requeueing"),
                                    },
                                    Err(e) => {
                                        // Serialize errors are not transient; drop with a loud log.
                                        error!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, error = %e, "bg: kb/node_update serialize error — dropping");
                                        delivered = true;
                                    }
                                }
                            } else {
                                warn!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, "bg: kb/node_update — writer absent while connected, requeueing");
                            }
                            if !delivered {
                                try_send_evt(&evt_tx, CollabEvent::KbUpdateRequeue { kb_id, node_id, update, pending_rowid });
                            }
                        }
                        CollabCommand::KbMember { kb_id, member, role, add } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let method = if add { "kb/add_member" } else { "kb/remove_member" };
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": method,
                                    "params": { "kb_id": kb_id, "member": member, "role": role, "label": member }
                                });
                                if let Ok(body) = serde_json::to_vec(&req) {
                                    if write_framed(w, &body, write_timeout).await.is_ok() {
                                        pending_responses.insert(
                                            req_id,
                                            PendingResponseKind::KbMember { kb_id, member, add },
                                        );
                                    }
                                }
                            }
                        }
                        // Phase D1.1: collection-manifest add/remove (best-effort,
                        // fire-and-forget — the daemon broadcasts the kbc: change and the
                        // projector reconciles; no response tracking needed).
                        CollabCommand::KbCollectionNode { kb_id, node_id, title, add } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let method = if add {
                                    "kb/collection_node_add"
                                } else {
                                    "kb/collection_node_remove"
                                };
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": req_id,
                                    "method": method,
                                    "params": { "kb_id": kb_id, "node_id": node_id, "title": title }
                                });
                                if let Ok(body) = serde_json::to_vec(&req) {
                                    let _ = write_framed(w, &body, write_timeout).await;
                                }
                            }
                        }
                        CollabCommand::KbApprove { kb_id, principal, role } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0", "id": req_id, "method": "kb/approve_member",
                                    "params": { "kb_id": kb_id, "principal": principal, "role": role }
                                });
                                if let Ok(body) = serde_json::to_vec(&req) {
                                    let _ = write_framed(w, &body, write_timeout).await;
                                }
                            }
                        }
                        CollabCommand::KbListPending { kb_id } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0", "id": req_id, "method": "kb/list_pending",
                                    "params": { "kb_id": kb_id }
                                });
                                if let Ok(body) = serde_json::to_vec(&req) {
                                    let _ = write_framed(w, &body, write_timeout).await;
                                }
                            }
                        }
                        CollabCommand::KbSetPolicy { kb_id, policy } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0", "id": req_id, "method": "kb/set_policy",
                                    "params": { "kb_id": kb_id, "policy": policy }
                                });
                                if let Ok(body) = serde_json::to_vec(&req) {
                                    let _ = write_framed(w, &body, write_timeout).await;
                                }
                            }
                        }
                        CollabCommand::Connect { address } => {
                            tear_down(&mut msg_rx, &mut writer);
                            pending_responses.clear();
                            target_address = Some(address);
                            continue;
                        }
                        CollabCommand::StartServer => {
                            try_send_evt(&evt_tx, CollabEvent::Error {
                                message: "Already connected to a state server".to_string(),
                            });
                        }
                    }
                }
                msg = rx.recv() => {
                    match msg {
                        Some(Ok(text)) => {
                            messages_received += 1;
                            debug!(msg_len = text.len(),
                                preview = &text[..text.len().min(120)],
                                "bridge: incoming server message");
                            handle_incoming_message(
                                &text,
                                &evt_tx,
                                &mut pending_responses,
                                &mut shared_docs,
                                &mut seq_tracker,
                            );
                            // Any valid message resets the ping_pending flag.
                            ping_pending = false;
                        }
                        Some(Err(e)) => {
                            debug!(error = %e, "bridge: reader task reported error");
                            tear_down(&mut msg_rx, &mut writer);
                            shared_docs.clear();
                            pending_responses.clear();
                            seq_tracker.clear();
                            ping_pending = false;
                            try_send_evt(&evt_tx, CollabEvent::Disconnected {
                                reason: format!("connection lost: {}", e),
                            });
                            if reconnect_enabled {
                                continue;
                            }
                        }
                        None => {
                            // Reader task exited (channel closed).
                            tear_down(&mut msg_rx, &mut writer);
                            shared_docs.clear();
                            pending_responses.clear();
                            seq_tracker.clear();
                            ping_pending = false;
                            try_send_evt(&evt_tx, CollabEvent::Disconnected {
                                reason: "connection lost".to_string(),
                            });
                            if reconnect_enabled {
                                continue;
                            }
                        }
                    }
                }
                // WU2: Periodic heartbeat.
                _ = heartbeat_interval.tick() => {
                    if heartbeat_secs == 0 {
                        continue;
                    }
                    if ping_pending {
                        // Previous ping got no response — connection dead.
                        warn!("heartbeat: no response to previous ping — disconnecting");
                        tear_down(&mut msg_rx, &mut writer);
                        shared_docs.clear();
                        pending_responses.clear();
                        seq_tracker.clear();
                        ping_pending = false;
                        try_send_evt(&evt_tx, CollabEvent::Disconnected {
                            reason: "heartbeat timeout".to_string(),
                        });
                        if reconnect_enabled {
                            continue;
                        }
                    } else if let Some(ref mut w) = writer {
                        // WU6: Transport health summary on each heartbeat tick.
                        debug!(
                            messages_received,
                            shared_doc_count = shared_docs.len(),
                            pending_response_count = pending_responses.len(),
                            "transport: health summary"
                        );
                        let req_id = next_request_id;
                        next_request_id += 1;
                        let req = serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": req_id,
                            "method": "$/ping",
                        });
                        let body = serde_json::to_vec(&req).unwrap_or_default();
                        if write_framed(w, &body, write_timeout).await.is_ok() {
                            pending_responses.insert(req_id, PendingResponseKind::Ping {
                                sent_at: std::time::Instant::now(),
                            });
                            ping_pending = true;
                        } else {
                            // Write failed — connection is broken.
                            tear_down(&mut msg_rx, &mut writer);
                            shared_docs.clear();
                            pending_responses.clear();
                            seq_tracker.clear();
                            ping_pending = false;
                            try_send_evt(&evt_tx, CollabEvent::Disconnected {
                                reason: "heartbeat write failed".to_string(),
                            });
                        }
                    }
                }
            }
        } else {
            // No connection — wait for commands or handle reconnection
            if reconnect_enabled {
                if let Some(ref addr) = target_address {
                    let addr_clone = addr.clone();
                    tokio::select! {
                        Some(cmd) = cmd_rx.recv() => {
                            handle_disconnected_cmd(
                                cmd, &evt_tx, &mut msg_rx, &mut writer,
                                &mut target_address, &mut reconnect_enabled,
                                &mut shared_docs, &mut next_request_id,
                                &mut pending_responses, write_timeout,
                                &transport,
                            ).await;
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_secs(
                            compute_backoff(reconnect_secs, backoff_factor, reconnect_attempt)
                        )) => {
                            // Check max attempts (0 = infinite).
                            if max_reconnect_attempts > 0
                                && reconnect_attempt as u64 >= max_reconnect_attempts
                            {
                                warn!(attempts = reconnect_attempt, max = max_reconnect_attempts,
                                    "max reconnect attempts exhausted");
                                reconnect_enabled = false;
                                try_send_evt(&evt_tx, CollabEvent::Disconnected {
                                    reason: format!("max reconnect attempts ({}) exhausted", max_reconnect_attempts),
                                });
                                continue;
                            }
                            reconnect_attempt += 1;
                            match establish_connection(&addr_clone, &transport).await {
                                Ok((mut reader, mut w)) => {
                                    if let Some(peer_count) = send_initialize(&mut w, &mut reader, write_timeout).await {
                                        // Spawn dedicated reader task (cancel-safety fix).
                                        // ADR-020: set writer before msg_rx so a
                                        // command never observes connected(msg_rx)=Some
                                        // while writer is None (avoids a spurious requeue).
                                        writer = Some(w);
                                        msg_rx = Some(spawn_reader_task(reader));
                                        reconnect_attempt = 0; // Reset on success.
                                        // Subscribe to sync_update events (B4 fix).
                                        if let Some(ref mut w) = writer {
                                            send_subscribe(w, &mut next_request_id, &mut pending_responses, write_timeout).await;
                                        }
                                        try_send_evt(&evt_tx, CollabEvent::Connected {
                                            address: addr_clone,
                                            peer_count,
                                        });
                                    }
                                }
                                Err(e) => {
                                    debug!(addr = %addr_clone, attempt = reconnect_attempt, error = %e,
                                        "reconnect failed, will retry");
                                }
                            }
                        }
                    }
                } else {
                    reconnect_enabled = false;
                }
            } else {
                let Some(cmd) = cmd_rx.recv().await else {
                    break;
                };
                handle_disconnected_cmd(
                    cmd,
                    &evt_tx,
                    &mut msg_rx,
                    &mut writer,
                    &mut target_address,
                    &mut reconnect_enabled,
                    &mut shared_docs,
                    &mut next_request_id,
                    &mut pending_responses,
                    write_timeout,
                    &transport,
                )
                .await;
            }
        }
    }
}

/// Check WAL sequence continuity for a doc. If a gap is detected, emit GapDetected.
fn check_seq_gap(
    doc_id: &str,
    wal_seq: u64,
    seq_tracker: &mut std::collections::HashMap<String, u64>,
    evt_tx: &mpsc::Sender<CollabEvent>,
) {
    let expected = seq_tracker
        .get(doc_id)
        .map(|last| last + 1)
        .unwrap_or(wal_seq); // first time: no gap
    if wal_seq > expected {
        warn!(doc = %doc_id, expected, got = wal_seq, "WAL sequence gap detected");
        try_send_evt(
            evt_tx,
            CollabEvent::GapDetected {
                doc_id: doc_id.to_string(),
                expected,
                got: wal_seq,
            },
        );
    }
    // Always update tracker to the latest seen seq.
    seq_tracker.insert(doc_id.to_string(), wal_seq);
}

/// Compute exponential backoff delay: `base * factor^min(attempt, 5)`, capped at 300s.
fn compute_backoff(base_secs: u64, factor: u64, attempt: u32) -> u64 {
    let exp = attempt.min(5);
    let delay = base_secs.saturating_mul(factor.saturating_pow(exp));
    delay.min(300)
}

/// Handle an incoming JSON-RPC message from the server.
/// Dispatches to response handler or notification handler based on content.
/// Non-blocking: uses try_send to avoid backpressure deadlock.
pub(crate) fn handle_incoming_message(
    text: &str,
    evt_tx: &mpsc::Sender<CollabEvent>,
    pending_responses: &mut std::collections::HashMap<u64, PendingResponseKind>,
    shared_docs: &mut Vec<String>,
    seq_tracker: &mut std::collections::HashMap<String, u64>,
) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };

    // Case 1: JSON-RPC response (has `id` + (`result` or `error`), no `method`)
    if let Some(id) = val.get("id").and_then(|v| v.as_u64()) {
        if val.get("method").is_none() {
            if let Some(kind) = pending_responses.remove(&id) {
                let has_error = val.get("error").is_some();
                debug!(id, has_error, kind = ?std::mem::discriminant(&kind),
                    "bridge: matched response to pending request");
                handle_response(&val, kind, evt_tx, shared_docs, seq_tracker);
            } else {
                debug!(id, "bridge: response for unknown/expired request id");
            }
            return;
        }
    }

    // WU3: Log responses with null/non-integer id (likely server notification parse error).
    if val.get("method").is_none() && (val.get("error").is_some() || val.get("result").is_some()) {
        let error_msg = val
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown");
        warn!(
            error = %error_msg,
            "bridge: received response with non-integer id \
             (likely server notification parse error)"
        );
        return;
    }

    // Case 2: Server notification (has `method`, no `id` or id is null)
    if let Some(method) = val.get("method").and_then(|m| m.as_str()) {
        match method {
            // B3 fix: server sends "notifications/sync_update" with nested event data.
            "notifications/sync_update" => {
                if let Some(params) = val.get("params") {
                    // Server format: {"params": {"seq": N, "event": {"type": "sync_update", "data": {"buffer_name": "...", "update_base64": "..."}}}}
                    // The "data" key comes from serde's #[serde(tag = "type", content = "data")] on EditorEvent.
                    let wal_seq = params.get("seq").and_then(|v| v.as_u64()).unwrap_or(0);
                    let event_data = params
                        .get("event")
                        .and_then(|e| e.get("data").or_else(|| e.get("sync_update")));
                    if let Some(sync_data) = event_data {
                        let buffer_name = sync_data
                            .get("buffer_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let update_b64 = sync_data
                            .get("update_base64")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        // Only process updates for docs this client has shared/joined.
                        // The server broadcasts to ALL clients; we filter client-side.
                        if !shared_docs.contains(&buffer_name) {
                            info!(doc = %buffer_name, "ignoring sync_update for unsubscribed doc");
                        } else {
                            info!(doc = %buffer_name, wal_seq, update_b64_len = update_b64.len(), "received sync_update notification");
                            // Gap detection: check wal_seq continuity per doc.
                            if wal_seq > 0 {
                                check_seq_gap(&buffer_name, wal_seq, seq_tracker, evt_tx);
                            }
                            if let Ok(bytes) = mae_sync::encoding::base64_to_update(update_b64) {
                                // Route KB node updates to KbNodeUpdate event.
                                if let Some(node_id) = buffer_name.strip_prefix("kb:") {
                                    debug!(node = %node_id, wal_seq, "routing sync_update as KB node update");
                                    try_send_evt(
                                        evt_tx,
                                        CollabEvent::KbNodeUpdate {
                                            kb_id: String::new(), // not available in notification
                                            node_id: node_id.to_string(),
                                            update_bytes: bytes,
                                        },
                                    );
                                } else {
                                    try_send_evt(
                                        evt_tx,
                                        CollabEvent::RemoteUpdate {
                                            doc_id: buffer_name,
                                            update_bytes: bytes,
                                            wal_seq,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }
            // Also handle direct sync/update format (legacy / future compat).
            "sync/update" => {
                if let Some(params) = val.get("params") {
                    let doc_id = params
                        .get("doc")
                        .or_else(|| params.get("buffer_name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // Only process updates for docs this client has shared/joined.
                    if !shared_docs.contains(&doc_id) {
                        debug!(doc = %doc_id, "ignoring sync/update for unsubscribed doc");
                    } else {
                        let wal_seq = params.get("wal_seq").and_then(|v| v.as_u64()).unwrap_or(0);
                        let update_b64 = params
                            .get("update")
                            .or_else(|| params.get("update_base64"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if wal_seq > 0 {
                            check_seq_gap(&doc_id, wal_seq, seq_tracker, evt_tx);
                        }
                        if let Ok(bytes) = mae_sync::encoding::base64_to_update(update_b64) {
                            // Route KB node updates to KbNodeUpdate event.
                            if let Some(node_id) = doc_id.strip_prefix("kb:") {
                                try_send_evt(
                                    evt_tx,
                                    CollabEvent::KbNodeUpdate {
                                        kb_id: String::new(),
                                        node_id: node_id.to_string(),
                                        update_bytes: bytes,
                                    },
                                );
                            } else {
                                try_send_evt(
                                    evt_tx,
                                    CollabEvent::RemoteUpdate {
                                        doc_id,
                                        update_bytes: bytes,
                                        wal_seq,
                                    },
                                );
                            }
                        }
                    }
                }
            }
            "notifications/peer_joined" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let peer_count =
                        data.get("peer_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    debug!(peer_count, "received peer_joined notification");
                    try_send_evt(evt_tx, CollabEvent::PeerCountChanged { peer_count });
                }
            }
            "notifications/peer_left" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let peer_count =
                        data.get("peer_count").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                    debug!(peer_count, "received peer_left notification");
                    try_send_evt(evt_tx, CollabEvent::PeerCountChanged { peer_count });
                }
            }
            "notifications/sharer_left" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let doc_id = data
                        .get("doc")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    debug!(doc = %doc_id, "received sharer_left notification");
                    try_send_evt(evt_tx, CollabEvent::SharerLeft { doc_id });
                }
            }
            "notifications/awareness_update" | "sync/awareness" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let client_id = data.get("client_id").and_then(|v| v.as_u64()).unwrap_or(0);
                    let doc_id = data
                        .get("doc")
                        .or_else(|| data.get("doc_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    // The server broadcasts EditorEvent::AwarenessUpdate which
                    // flattens state fields into `data`. Try nested "state" first
                    // (direct sync/awareness format), then fall back to `data` itself
                    // (broadcast notification format).
                    let state_source = data.get("state").unwrap_or(data);
                    if let Ok(state) = serde_json::from_value::<mae_sync::awareness::AwarenessState>(
                        state_source.clone(),
                    ) {
                        debug!(
                            client_id,
                            doc = %doc_id,
                            user = %state.user_name,
                            row = state.cursor_row,
                            col = state.cursor_col,
                            "received awareness update"
                        );
                        try_send_evt(
                            evt_tx,
                            CollabEvent::AwarenessUpdate {
                                client_id,
                                doc_id,
                                state,
                            },
                        );
                    } else {
                        debug!(
                            client_id,
                            doc = %doc_id,
                            "awareness parse failed — data keys: {:?}",
                            data.as_object().map(|o| o.keys().collect::<Vec<_>>())
                        );
                    }
                }
            }
            "notifications/save_committed" => {
                if let Some(params) = val.get("params") {
                    let event = params.get("event").unwrap_or(params);
                    let data = event.get("data").unwrap_or(event);
                    let doc = data
                        .get("doc")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let saved_by = data
                        .get("saved_by")
                        .and_then(|v| v.as_str())
                        .unwrap_or("peer")
                        .to_string();
                    debug!(doc = %doc, saved_by = %saved_by, "received save_committed notification");
                    try_send_evt(evt_tx, CollabEvent::PeerSaved { doc, saved_by });
                }
            }
            _ => {
                debug!(method = method, "unhandled server notification");
            }
        }
    }
}

/// Handle a correlated JSON-RPC response based on the pending request kind.
/// Non-blocking: uses try_send to avoid backpressure deadlock.
fn handle_response(
    val: &serde_json::Value,
    kind: PendingResponseKind,
    evt_tx: &mpsc::Sender<CollabEvent>,
    shared_docs: &mut Vec<String>,
    seq_tracker: &mut std::collections::HashMap<String, u64>,
) {
    let result = val.get("result");

    match kind {
        PendingResponseKind::ShareBuffer { doc_id } => {
            if val.get("error").is_some() {
                let err_msg = val
                    .get("error")
                    .and_then(|e| e.get("message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                error!(doc = %doc_id, error = %err_msg, "share: server rejected");
                try_send_evt(
                    evt_tx,
                    CollabEvent::ShareFailed {
                        doc_id,
                        message: err_msg,
                    },
                );
            } else {
                info!(doc = %doc_id, "share: server accepted sync/share");
                // WU2: Seed seq_tracker from share response wal_seq.
                let wal_seq = result
                    .and_then(|r| r.get("wal_seq"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                if wal_seq > 0 {
                    debug!(doc = %doc_id, wal_seq, "share: seeding seq_tracker");
                    seq_tracker.insert(doc_id.clone(), wal_seq);
                }
                if !shared_docs.contains(&doc_id) {
                    shared_docs.push(doc_id.clone());
                }
                try_send_evt(evt_tx, CollabEvent::BufferShared { doc_id });
            }
        }
        PendingResponseKind::ListDocs { for_join } => {
            let documents = result
                .and_then(|r| r.get("documents"))
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            info!(count = documents.len(), for_join, docs = ?documents, "docs/list response");
            try_send_evt(
                evt_tx,
                CollabEvent::DocList {
                    documents,
                    for_join,
                },
            );
        }
        PendingResponseKind::JoinDoc { doc_id } => {
            // sync/resync response: {"result": {"doc": "...", "state": "<base64>", "sv": "<base64>"}}
            // Use server-resolved doc_id (suffix matching may have expanded bare
            // filenames like "test.txt" → "file:no-project/test.txt").
            let resolved_doc_id = result
                .and_then(|r| r.get("doc"))
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| doc_id.clone());
            let state_b64 = result
                .and_then(|r| r.get("state"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            info!(doc = %resolved_doc_id, b64_len = state_b64.len(), "join: received sync/resync response");
            // WU2: Seed seq_tracker from join response wal_seq.
            let wal_seq = result
                .and_then(|r| r.get("wal_seq"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            if wal_seq > 0 {
                debug!(doc = %resolved_doc_id, wal_seq, "join: seeding seq_tracker");
                seq_tracker.insert(resolved_doc_id.clone(), wal_seq);
            }
            // Update shared_docs to use the resolved name (replace unresolved if present).
            if resolved_doc_id != doc_id {
                if let Some(pos) = shared_docs.iter().position(|d| d == &doc_id) {
                    shared_docs[pos] = resolved_doc_id.clone();
                } else if !shared_docs.contains(&resolved_doc_id) {
                    shared_docs.push(resolved_doc_id.clone());
                }
            }
            match mae_sync::encoding::base64_to_update(state_b64) {
                Ok(state_bytes) => {
                    info!(doc = %resolved_doc_id, state_len = state_bytes.len(), "join: decoded state, sending BufferJoined");
                    try_send_evt(
                        evt_tx,
                        CollabEvent::BufferJoined {
                            doc_id: resolved_doc_id,
                            state_bytes,
                        },
                    );
                }
                Err(e) => {
                    error!(doc = %doc_id, error = %e, b64_preview = &state_b64[..state_b64.len().min(100)], "join: failed to decode state");
                    try_send_evt(
                        evt_tx,
                        CollabEvent::Error {
                            message: format!("Failed to decode state for {}: {}", doc_id, e),
                        },
                    );
                }
            }
        }
        PendingResponseKind::ForceSync { doc_id } => {
            // sync/full_state response: {"result": {"doc": "...", "state": "<base64>"}}
            // Use BufferJoined (load_sync_state path) to avoid content duplication
            // that occurs when applying full state as an incremental update.
            let state_b64 = result
                .and_then(|r| r.get("state"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            if !state_b64.is_empty() {
                match mae_sync::encoding::base64_to_update(state_b64) {
                    Ok(state_bytes) => {
                        try_send_evt(
                            evt_tx,
                            CollabEvent::BufferJoined {
                                doc_id,
                                state_bytes,
                            },
                        );
                    }
                    Err(e) => {
                        try_send_evt(
                            evt_tx,
                            CollabEvent::Error {
                                message: format!("Failed to decode resync for {}: {}", doc_id, e),
                            },
                        );
                    }
                }
            }
        }
        PendingResponseKind::SyncUpdate { doc_id } => {
            if let Some(err) = val.get("error") {
                warn!(doc = %doc_id, error = ?err, "server rejected sync update");
            }
        }
        PendingResponseKind::KbNodeUpdate {
            kb_id,
            node_id,
            pending_rowid,
        } => {
            // ADR-020 queue→send→confirm→ack: the daemon has now answered.
            if let Some(err) = val.get("error") {
                let message = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("kb/node_update rejected")
                    .to_string();
                warn!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, error = %message, "kb/node_update REJECTED by daemon");
                try_send_evt(
                    evt_tx,
                    CollabEvent::KbUpdateFailed {
                        kb_id,
                        node_id,
                        rowid: pending_rowid,
                        message,
                    },
                );
            } else {
                tracing::debug!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, rowid = ?pending_rowid, "kb/node_update: daemon confirmed applied");
                if let Some(rowid) = pending_rowid {
                    try_send_evt(evt_tx, CollabEvent::KbUpdateAcked { rowid });
                }
            }
        }
        PendingResponseKind::SaveIntent {
            doc_id,
            expected_hash,
        } => {
            if let Some(err) = val.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("save intent failed")
                    .to_string();
                try_send_evt(
                    evt_tx,
                    CollabEvent::SaveIntentConflict {
                        doc_id,
                        message: msg,
                    },
                );
            } else if let Some(r) = result {
                let save_result = r.get("result").unwrap_or(r);
                let status = save_result
                    .get("status")
                    .and_then(|s| s.as_str())
                    .unwrap_or("");
                if status == "conflict" {
                    try_send_evt(
                        evt_tx,
                        CollabEvent::SaveIntentConflict {
                            doc_id,
                            message: "Content hash mismatch — sync first".to_string(),
                        },
                    );
                } else {
                    let save_epoch = save_result
                        .get("save_epoch")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    try_send_evt(
                        evt_tx,
                        CollabEvent::SaveIntentOk {
                            doc_id,
                            save_epoch,
                            content_hash: expected_hash,
                        },
                    );
                }
            }
        }
        PendingResponseKind::Subscribe => {
            // Acknowledgement — no action needed.
        }
        PendingResponseKind::Ping { sent_at } => {
            let latency_ms = sent_at.elapsed().as_millis() as u64;
            debug!(latency_ms, "heartbeat pong received");
            // Latency is logged — could be exposed to doctor in the future.
            let _ = latency_ms; // suppress unused warning
        }
        PendingResponseKind::DoctorPing { sent_at, reply } => {
            let latency_ms = sent_at.elapsed().as_millis() as u64;
            let _ = reply.send(Some(latency_ms));
        }
        PendingResponseKind::DoctorDebug { reply } => {
            let debug_data = val.get("result").cloned();
            let _ = reply.send(debug_data);
        }
        PendingResponseKind::KbShare { kb_id } => {
            if result
                .and_then(|r| r.get("shared"))
                .and_then(|v| v.as_bool())
                == Some(true)
            {
                let node_count = result
                    .and_then(|r| r.get("node_count"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                let collection_state = result
                    .and_then(|r| r.get("collection_state"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| mae_sync::encoding::base64_to_update(s).ok())
                    .unwrap_or_default();
                try_send_evt(
                    evt_tx,
                    CollabEvent::KbShared {
                        kb_id,
                        node_count,
                        collection_state,
                    },
                );
            } else {
                try_send_evt(
                    evt_tx,
                    CollabEvent::Error {
                        message: format!("Failed to share KB: {}", val),
                    },
                );
            }
        }
        PendingResponseKind::KbAdoptNode { kb_id, node_id } => {
            // ADR-024 R1: response to kb/node_fetch → adopt the authoritative state.
            if let Some(err) = val.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("fetch failed");
                try_send_evt(
                    evt_tx,
                    CollabEvent::Error {
                        message: format!("KB '{kb_id}': could not fetch {node_id} to adopt: {msg}"),
                    },
                );
            } else if let Some(state) = result
                .and_then(|r| r.get("state"))
                .and_then(|v| v.as_str())
                .and_then(|s| mae_sync::encoding::base64_to_update(s).ok())
            {
                try_send_evt(
                    evt_tx,
                    CollabEvent::KbNodeAdopted {
                        kb_id,
                        node_id,
                        state_bytes: state,
                    },
                );
            } else {
                try_send_evt(
                    evt_tx,
                    CollabEvent::Error {
                        message: format!("KB '{kb_id}': empty state for {node_id} (cannot adopt)"),
                    },
                );
            }
        }
        PendingResponseKind::KbJoin { kb_id } => {
            // Distinguish denied / pending / joined so the editor stops showing
            // "Joined (0 nodes)" for all three outcomes (B-1). Daemon shapes:
            //   denied  → JSON-RPC error
            //   pending → success { status: "pending" }   (invite policy)
            //   joined  → success { collection_state, nodes: [...] }
            if let Some(err) = val.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("access denied");
                try_send_evt(
                    evt_tx,
                    CollabEvent::Error {
                        message: format!("KB '{kb_id}' join denied: {msg}"),
                    },
                );
            } else if result
                .and_then(|r| r.get("status"))
                .and_then(|s| s.as_str())
                == Some("pending")
            {
                try_send_evt(
                    evt_tx,
                    CollabEvent::StatusReport {
                        lines: vec![format!(
                            "KB '{kb_id}': join request sent — pending owner approval"
                        )],
                    },
                );
            } else {
                let collection_state = result
                    .and_then(|r| r.get("collection_state"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| mae_sync::encoding::base64_to_update(s).ok())
                    .unwrap_or_default();
                // ADR-022: each node carries the daemon's SV plus either an
                // incremental `diff` (we sent an SV → reconcile) or a full `state`
                // (first join / pre-ADR-022 daemon). `daemon_sv` is None only for an
                // old daemon that omits `sv` → the member falls back to adopt.
                let nodes: Vec<JoinedNode> = result
                    .and_then(|r| r.get("nodes"))
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|n| {
                                let id = n.get("id")?.as_str()?.to_string();
                                // Prefer the incremental diff; fall back to full state.
                                let bytes = n
                                    .get("diff")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| n.get("state").and_then(|v| v.as_str()))
                                    .and_then(|s| mae_sync::encoding::base64_to_update(s).ok())?;
                                let daemon_sv = n
                                    .get("sv")
                                    .and_then(|v| v.as_str())
                                    .and_then(|s| mae_sync::encoding::base64_to_update(s).ok());
                                Some(JoinedNode {
                                    id,
                                    bytes,
                                    daemon_sv,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                // ADR-020 B-13: establish the member's LIVE subscription to the
                // collection + each joined node doc, so subsequent inbound
                // sync_update broadcasts are applied instead of being dropped at the
                // shared_docs filter. The one-time join snapshot (KbJoined) catches
                // up; these subscriptions keep it live. (Mirror of the text-buffer
                // join path.)
                let coll_doc = format!("kbc:{kb_id}");
                if !shared_docs.contains(&coll_doc) {
                    shared_docs.push(coll_doc);
                }
                for n in &nodes {
                    let node_doc = format!("kb:{}", n.id);
                    if !shared_docs.contains(&node_doc) {
                        shared_docs.push(node_doc);
                    }
                }
                try_send_evt(
                    evt_tx,
                    CollabEvent::KbJoined {
                        kb_id,
                        collection_state,
                        nodes,
                    },
                );
            }
        }
        PendingResponseKind::KbLeave { kb_id } => {
            try_send_evt(evt_tx, CollabEvent::KbLeft { kb_id });
        }
        PendingResponseKind::KbMember { kb_id, member, add } => {
            // The membership change response carries error (e.g. not owner) or ok.
            if let Some(err) = val.get("error") {
                let msg = err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("membership change failed");
                try_send_evt(
                    evt_tx,
                    CollabEvent::Error {
                        message: msg.to_string(),
                    },
                );
            } else {
                try_send_evt(
                    evt_tx,
                    CollabEvent::StatusReport {
                        lines: vec![format!(
                            "{} '{member}' {} KB '{kb_id}'",
                            if add { "Added" } else { "Removed" },
                            if add { "to" } else { "from" }
                        )],
                    },
                );
            }
        }
    }
}

/// Send `notifications/subscribe` to opt into sync_update events (B4 fix).
async fn send_subscribe<W: tokio::io::AsyncWrite + Unpin>(
    writer: &mut W,
    next_id: &mut u64,
    pending: &mut std::collections::HashMap<u64, PendingResponseKind>,
    timeout: std::time::Duration,
) {
    use mae_mcp::write_framed;

    let req_id = *next_id;
    *next_id += 1;
    let req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "notifications/subscribe",
        "params": {
            "types": ["sync_update", "peer_joined", "peer_left", "save_committed", "awareness_update", "sharer_left"]
        }
    });
    let body = serde_json::to_vec(&req).unwrap();
    if write_framed(writer, &body, timeout).await.is_ok() {
        pending.insert(req_id, PendingResponseKind::Subscribe);
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_disconnected_cmd(
    cmd: CollabCommand,
    evt_tx: &mpsc::Sender<CollabEvent>,
    msg_rx: &mut Option<mpsc::Receiver<Result<String, String>>>,
    writer: &mut Option<BoxWriter>,
    target_address: &mut Option<String>,
    reconnect_enabled: &mut bool,
    shared_docs: &mut Vec<String>,
    next_request_id: &mut u64,
    pending_responses: &mut std::collections::HashMap<u64, PendingResponseKind>,
    write_timeout: std::time::Duration,
    transport: &ClientTransport,
) {
    match cmd {
        CollabCommand::Connect { address } => {
            *target_address = Some(address.clone());
            match establish_connection(&address, transport).await {
                Ok((mut reader, mut w)) => {
                    if let Some(peer_count) =
                        send_initialize(&mut w, &mut reader, write_timeout).await
                    {
                        // Spawn dedicated reader task (cancel-safety fix).
                        // ADR-020: writer before msg_rx (see reconnect note).
                        *writer = Some(w);
                        *msg_rx = Some(spawn_reader_task(reader));
                        *reconnect_enabled = true;
                        // Subscribe to sync_update events (B4 fix).
                        if let Some(ref mut w) = writer {
                            send_subscribe(w, next_request_id, pending_responses, write_timeout)
                                .await;
                        }
                        try_send_evt(
                            evt_tx,
                            CollabEvent::Connected {
                                address,
                                peer_count,
                            },
                        );
                    } else {
                        *reconnect_enabled = true;
                        try_send_evt(
                            evt_tx,
                            CollabEvent::Error {
                                message: format!("Handshake failed with {}", address),
                            },
                        );
                    }
                }
                Err(e) => {
                    *reconnect_enabled = true;
                    try_send_evt(
                        evt_tx,
                        CollabEvent::Error {
                            message: format!("Cannot connect to {}: {}", address, e),
                        },
                    );
                }
            }
        }
        CollabCommand::StartServer => {
            match tokio::process::Command::new("mae-daemon")
                .arg("start")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()
            {
                Ok(child) => {
                    let pid = child.id().unwrap_or(0);
                    if let Err(e) = evt_tx.try_send(CollabEvent::ServerStarted { pid }) {
                        warn!("failed to send ServerStarted event: {}", e);
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let default_addr = mae_core::DEFAULT_COLLAB_ADDRESS.to_string();
                    let addr = target_address
                        .clone()
                        .unwrap_or_else(|| default_addr.clone());
                    *target_address = Some(addr.clone());
                    match establish_connection(&addr, transport).await {
                        Ok((mut reader, mut w)) => {
                            if let Some(peer_count) =
                                send_initialize(&mut w, &mut reader, write_timeout).await
                            {
                                // Spawn dedicated reader task (cancel-safety fix).
                                *msg_rx = Some(spawn_reader_task(reader));
                                *writer = Some(w);
                                *reconnect_enabled = true;
                                // Subscribe after server start too.
                                if let Some(ref mut w) = writer {
                                    send_subscribe(
                                        w,
                                        next_request_id,
                                        pending_responses,
                                        write_timeout,
                                    )
                                    .await;
                                }
                                if let Err(e) = evt_tx
                                    .send(CollabEvent::Connected {
                                        address: addr,
                                        peer_count,
                                    })
                                    .await
                                {
                                    warn!("failed to send Connected event: {}", e);
                                }
                            }
                        }
                        Err(e) => {
                            try_send_evt(
                                evt_tx,
                                CollabEvent::Error {
                                    message: format!("Server started but connect failed: {}", e),
                                },
                            );
                        }
                    }
                }
                Err(e) => {
                    try_send_evt(
                        evt_tx,
                        CollabEvent::ServerFailed {
                            error: format!("Failed to spawn mae-daemon: {}", e),
                        },
                    );
                }
            }
        }
        CollabCommand::ShowStatus => {
            let lines = build_status_lines(
                target_address.as_deref().unwrap_or("not configured"),
                false,
                shared_docs,
            );
            try_send_evt(evt_tx, CollabEvent::StatusReport { lines });
        }
        CollabCommand::Doctor { synced_info } => {
            let ctx = DoctorContext {
                address: target_address
                    .as_deref()
                    .unwrap_or("not configured")
                    .to_string(),
                connected: false,
                server_debug: None,
                ping_latency_ms: None,
                synced_info,
            };
            let lines = build_doctor_lines(&ctx);
            try_send_evt(evt_tx, CollabEvent::DoctorReport { lines });
        }
        CollabCommand::Disconnect => {
            *reconnect_enabled = false;
            shared_docs.clear();
        }
        CollabCommand::ShareBuffer { doc_id, .. } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot share '{}'", doc_id),
                },
            );
        }
        CollabCommand::ForceSync { doc_id } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot sync '{}'", doc_id),
                },
            );
        }
        CollabCommand::SendUpdate { .. } => {
            // Silently drop — not connected.
        }
        CollabCommand::ListDocs { .. } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: "Not connected \u{2014} cannot list documents".to_string(),
                },
            );
        }
        CollabCommand::JoinDoc { doc_id } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot join '{}'", doc_id),
                },
            );
        }
        CollabCommand::SendSaveIntent { doc_id, .. } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot save '{}'", doc_id),
                },
            );
        }
        CollabCommand::SendAwareness { .. } => {
            // Silently drop — not connected.
        }
        CollabCommand::SendSaveCommitted { .. } => {
            // Silently drop — not connected.
        }
        CollabCommand::ShareKb { kb_id, .. } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot share KB '{}'", kb_id),
                },
            );
        }
        CollabCommand::JoinKb { kb_id, .. } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot join KB '{}'", kb_id),
                },
            );
        }
        CollabCommand::KbAdoptNode { kb_id, node_id } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!(
                        "Not connected \u{2014} cannot adopt {node_id} in KB '{kb_id}'"
                    ),
                },
            );
        }
        CollabCommand::LeaveKb { kb_id } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected \u{2014} cannot leave KB '{}'", kb_id),
                },
            );
        }
        CollabCommand::KbNodeUpdate {
            kb_id,
            node_id,
            update,
            // Re-derived from `kb_epochs` at the next drain, so the stale epoch isn't
            // carried across the requeue.
            epoch: _,
            pending_rowid,
        } => {
            // ADR-020 durability: not connected — re-queue (don't drop) so the
            // edit is retried on reconnect.
            try_send_evt(
                evt_tx,
                CollabEvent::KbUpdateRequeue {
                    kb_id,
                    node_id,
                    update,
                    pending_rowid,
                },
            );
        }
        CollabCommand::KbMember { kb_id, .. }
        | CollabCommand::KbApprove { kb_id, .. }
        | CollabCommand::KbListPending { kb_id }
        | CollabCommand::KbSetPolicy { kb_id, .. } => {
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: format!("Not connected — cannot manage KB '{kb_id}'"),
                },
            );
        }
        // Phase D1.1: a manifest op that arrives while disconnected is dropped
        // silently (background op, no user-facing error) — the reconnect re-share
        // rebuilds the full manifest, healing missed creates.
        CollabCommand::KbCollectionNode {
            kb_id,
            node_id,
            add,
            ..
        } => {
            debug!(kb = %kb_id, node = %node_id, add, "not connected — KB manifest op dropped (heals on reconnect re-share)");
        }
    }
}

/// Perform PSK mutual authentication handshake before JSON-RPC initialize.
/// Returns `Ok(())` on success or if no PSK is configured (no-auth mode).
/// Returns `Err(message)` if auth fails.
async fn perform_psk_auth<R, W>(
    reader: &mut R,
    writer: &mut W,
    psk: &str,
    key_id: Option<&str>,
) -> Result<(), String>
where
    R: tokio::io::AsyncBufRead + Unpin + Send,
    W: tokio::io::AsyncWrite + Unpin + Send,
{
    use mae_mcp::auth::{AuthProvider, PskAuth};

    if psk.is_empty() {
        return Ok(());
    }

    let auth = PskAuth::new(psk).offering(key_id.map(String::from));
    auth.client_handshake(reader, writer)
        .await
        .map_err(|e| format!("PSK auth failed: {e}"))
}

/// Send JSON-RPC `initialize` handshake to the state server.
/// Returns `Some(peer_count)` on success, `None` on failure.
/// Reads the response to extract `serverInfo.connections`.
///
/// IMPORTANT: Takes already-split writer + BufReader to avoid creating a
/// temporary BufReader that could over-read and drop bytes from the TCP
/// stream, breaking Content-Length framing for subsequent messages.
async fn send_initialize<W, R>(
    writer: &mut W,
    reader: &mut R,
    timeout: std::time::Duration,
) -> Option<usize>
where
    W: tokio::io::AsyncWrite + Unpin,
    R: tokio::io::AsyncBufRead + Unpin,
{
    use mae_mcp::write_framed;

    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 0,
        "method": "initialize",
        "params": {
            "client_name": "mae-editor",
            "version": env!("CARGO_PKG_VERSION"),
        }
    });
    let body = serde_json::to_vec(&init_req).unwrap();
    if write_framed(writer, &body, timeout).await.is_err() {
        return None;
    }

    match mae_mcp::read_message(reader).await {
        Ok(Some(text)) => {
            let peer_count = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("result")?
                        .get("serverInfo")?
                        .get("connections")?
                        .as_u64()
                })
                .map(|c| c as usize)
                .unwrap_or(0);
            Some(peer_count)
        }
        _ => None,
    }
}

fn build_status_lines(address: &str, connected: bool, shared_docs: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Collaborative Editing Status".to_string());
    lines.push(String::from_utf8(vec![b'='; 40]).unwrap());
    lines.push(format!(
        "Connection:  {}",
        if connected {
            format!("Connected ({})", address)
        } else {
            "Disconnected".to_string()
        }
    ));
    lines.push(String::new());
    if shared_docs.is_empty() {
        lines.push("No documents shared.".to_string());
    } else {
        lines.push(format!("Synced Documents ({}):", shared_docs.len()));
        for doc in shared_docs {
            lines.push(format!("  {}", doc));
        }
    }
    lines.push(String::new());
    lines.push(format!("Server: {}", address));
    lines
}

/// Context gathered for the doctor report — pre-fetched data from server queries.
pub(crate) struct DoctorContext {
    pub(crate) address: String,
    pub(crate) connected: bool,
    /// Per-doc stats from $/debug response, if available.
    pub(crate) server_debug: Option<serde_json::Value>,
    /// Round-trip latency in ms from $/ping.
    pub(crate) ping_latency_ms: Option<u64>,
    /// Per-buffer sync info: (doc_id, pending_update_count).
    pub(crate) synced_info: Vec<(String, usize)>,
}

pub(crate) fn build_doctor_lines(ctx: &DoctorContext) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push("Collab Doctor".to_string());
    lines.push(String::from_utf8(vec![b'='; 20]).unwrap());
    if ctx.connected {
        lines.push(format!("\u{2713} State server reachable ({})", ctx.address));
        lines.push("\u{2713} Protocol: JSON-RPC 2.0 (Content-Length framing)".to_string());
        lines.push(format!(
            "\u{2713} Client version: {} ({})",
            env!("CARGO_PKG_VERSION"),
            crate::BUILD_SHA
        ));

        // Cross-machine build check: the daemon reports its own version + build
        // in `$/debug`. Surface both and warn on a mismatch — the "are we on the
        // same commit?" question the live two-machine test kept asking by hand.
        if let Some(ref debug_val) = ctx.server_debug {
            let server_ver = debug_val.get("version").and_then(|v| v.as_str());
            let server_build = debug_val.get("build").and_then(|v| v.as_str());
            if let Some(ver) = server_ver {
                match server_build {
                    Some(build) => lines.push(format!("\u{2713} Server version: {ver} ({build})")),
                    None => lines.push(format!("\u{2713} Server version: {ver}")),
                }
            }
            if let Some(build) = server_build {
                if build != "unknown" && crate::BUILD_SHA != "unknown" && build != crate::BUILD_SHA
                {
                    lines.push(format!(
                        "\u{26a0} Build mismatch: editor {} vs daemon {} — rebuild/redeploy both to the same commit",
                        crate::BUILD_SHA, build
                    ));
                }
            }
        }

        // Latency
        match ctx.ping_latency_ms {
            Some(ms) => lines.push(format!("\u{2713} Ping: {}ms", ms)),
            None => lines.push("\u{26a0} Ping: timeout".to_string()),
        }

        // Per-doc server stats from $/debug
        // Server returns: {"documents": N, "doc_stats": {"name": {stats...}}}
        if let Some(ref debug_val) = ctx.server_debug {
            if let Some(stats_map) = debug_val.get("doc_stats").and_then(|d| d.as_object()) {
                lines.push(String::new());
                lines.push(format!("Server Documents ({}):", stats_map.len()));
                for (name, stats) in stats_map {
                    let wal_seq = stats.get("wal_seq").and_then(|v| v.as_u64()).unwrap_or(0);
                    let updates = stats
                        .get("update_count")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let clients = stats
                        .get("connected_clients")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let idle = stats.get("idle_secs").and_then(|v| v.as_u64());
                    let mut info = format!(
                        "  {} — wal:{} updates:{} clients:{}",
                        name, wal_seq, updates, clients
                    );
                    if let Some(s) = idle {
                        info.push_str(&format!(" idle:{}s", s));
                    }
                    lines.push(info);
                }
            }
        }

        // Synced buffers
        if !ctx.synced_info.is_empty() {
            lines.push(String::new());
            lines.push(format!("Synced Buffers ({}):", ctx.synced_info.len()));
            for (doc_id, pending) in &ctx.synced_info {
                let status = if *pending > 0 {
                    format!("{} pending", pending)
                } else {
                    "up-to-date".to_string()
                };
                lines.push(format!("  {} — {}", doc_id, status));
            }
        }
    } else {
        lines.push(format!(
            "\u{2717} State server not reachable ({})",
            ctx.address
        ));
        lines.push(String::new());
        lines.push("Troubleshooting:".to_string());
        lines.push("  1. Is mae-daemon running?".to_string());
        lines.push("     Start: systemctl --user start mae-daemon".to_string());
        lines.push(format!("     Or:    mae-daemon --bind {}", ctx.address));
        lines.push("  2. Check if the port is listening:".to_string());
        lines.push("     ss -tlnp | grep 9473".to_string());
        lines.push("  3. Check firewall:".to_string());
        lines.push(
            "     Fedora:  sudo firewall-cmd --add-port=9473/tcp --permanent && sudo firewall-cmd --reload"
                .to_string(),
        );
        lines.push("     Ubuntu:  sudo ufw allow 9473/tcp".to_string());
        lines.push(format!(
            "  4. Test connectivity: nc -zv {} {}",
            ctx.address.split(':').next().unwrap_or("127.0.0.1"),
            ctx.address.split(':').next_back().unwrap_or("9473")
        ));
        lines.push("  5. Use SPC C s to start a local server".to_string());
    }
    lines.push(String::new());
    lines.push("Commands:".to_string());
    lines.push("  SPC C l  — list shared documents on server".to_string());
    lines.push("  SPC C j  — join a shared document".to_string());
    lines.push(String::new());
    lines.push("! No authentication configured (trusted LAN mode)".to_string());
    lines
}

#[cfg(test)]
#[path = "collab_bridge_tests.rs"]
mod tests;
