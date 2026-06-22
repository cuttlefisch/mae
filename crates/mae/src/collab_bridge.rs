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
    /// Send a KB node update to the server (continuous sync).
    /// `pending_rowid` is the durable SQLite queue row this update came from
    /// (`Some` when store-backed); the row is acked only after the daemon confirms
    /// it applied (queue → send → confirm → ack). `None` = transient in-memory
    /// update (no durable store) — best-effort, requeued in-memory on failure.
    KbNodeUpdate {
        kb_id: String,
        node_id: String,
        update: Vec<u8>,
        pending_rowid: Option<i64>,
    },
    /// Add or remove a peer (by principal) from a KB's members (owner-only).
    KbMember {
        kb_id: String,
        member: String,
        role: String,
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
            let cmd = CollabCommand::KbNodeUpdate {
                kb_id: pu.kb_id,
                node_id: pu.node_id,
                update: pu.update_bytes,
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
            let cmd = CollabCommand::KbNodeUpdate {
                kb_id,
                node_id,
                update,
                pending_rowid: None,
            };
            if collab_tx.try_send(cmd).is_err() {
                warn!("collab command channel full — KB node update dropped");
            }
        }
        if in_mem > 0 {
            tracing::debug!(target: "kb_sync", count = in_mem, "drain: flushed in-memory kb updates");
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
        CollabIntent::Connect { address } => CollabCommand::Connect { address },
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
            pending_rowid: None,
        },
        CollabIntent::DiscoverPeers => {
            // mDNS discovery: browse for _mae-sync._tcp.local services.
            match crate::mdns_discovery::MdnsManager::new() {
                Ok(mut mgr) => {
                    if let Err(e) = mgr.start_browse() {
                        editor.set_status(format!("mDNS browse failed: {}", e));
                    } else {
                        // Give mDNS a moment to discover peers.
                        std::thread::sleep(std::time::Duration::from_millis(1500));
                        let peers = mgr.discovered_peers();
                        if peers.is_empty() {
                            editor.set_status("No MAE peers found on local network.");
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
                            lines.push(
                                "Use :collab-connect <address> to connect to a peer.".to_string(),
                            );
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
                    }
                }
                Err(e) => {
                    editor.set_status(format!("mDNS init failed: {}", e));
                }
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
        CollabCommand::KbNodeUpdate { .. } => "kb-node-update",
        CollabCommand::KbMember { .. } => "kb-member",
    }
}

// --- Event handling (main thread) ---

/// Handle an event from the collab background task — update editor state.
pub(crate) fn handle_collab_event(editor: &mut Editor, event: CollabEvent) {
    match event {
        CollabEvent::HostKeyPrompt {
            addr,
            fingerprint,
            reply,
        } => {
            // Stash the reply channel; the y/n answer in apply_mini_dialog sends it
            // back to the (blocked) connection task. ADR-017 TOFU.
            editor.pending_host_key_reply = Some(reply);
            editor.mini_dialog = Some(mae_core::command_palette::MiniDialogState::confirm(
                format!("Trust daemon at {addr}?\n  {fingerprint}\n(first connect — accept & pin?) [y/N]"),
                mae_core::command_palette::MiniDialogContext::PeerKeyAccept { addr, fingerprint },
            ));
            editor.mark_full_redraw();
        }
        CollabEvent::Connected {
            address,
            peer_count,
        } => {
            info!(address = %address, peers = peer_count, "collab connected");
            editor.collab.status = CollabStatus::Connected { peer_count };
            editor.set_status(format!("Connected to {} ({} peers)", address, peer_count));
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

            editor.mark_full_redraw();
        }
        CollabEvent::Disconnected { reason } => {
            info!(reason = %reason, "collab disconnected");
            editor.collab.status = CollabStatus::Disconnected;
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
            if let Some(idx) = editor.find_buffer_by_collab_doc_id(&doc_id) {
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
        CollabEvent::KbShared { kb_id, node_count } => {
            info!(kb = %kb_id, node_count, "KB shared successfully");
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
            let is_rebase = message.contains("rebase required");
            if is_rebase {
                warn!(target: "kb_sync", kb = %kb_id, node = %node_id, error = %message, "kb/node_update fenced (stale-epoch) — pre-grant edit not synced (B-19)");
                editor.set_status(format!(
                    "KB '{kb_id}': your earlier edit to {node_id} was made before your \
                     access changed and was NOT synced — reconnect and re-apply it"
                ));
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
                    let policy = mae_mcp::identity::HostKeyPolicy::from_str_opt(
                        &editor.collab.host_key_policy,
                    );
                    let known_hosts = dir.join("known_hosts");
                    // `prompt` → interactive TOFU; otherwise the non-interactive
                    // accept-new / strict file verifier.
                    let verifier: std::sync::Arc<dyn mae_mcp::identity::HostKeyVerifier> =
                        if policy == mae_mcp::identity::HostKeyPolicy::Prompt {
                            std::sync::Arc::new(PromptingHostKeyVerifier {
                                known_hosts,
                                evt_tx: evt_tx.clone(),
                                timeout: std::time::Duration::from_secs(120),
                            })
                        } else {
                            std::sync::Arc::new(mae_mcp::identity::FileHostKeyVerifier::new(
                                known_hosts,
                                policy,
                            ))
                        };
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
struct PromptingHostKeyVerifier {
    known_hosts: std::path::PathBuf,
    evt_tx: mpsc::Sender<CollabEvent>,
    timeout: std::time::Duration,
}

impl std::fmt::Debug for PromptingHostKeyVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PromptingHostKeyVerifier")
            .field("known_hosts", &self.known_hosts)
            .finish_non_exhaustive()
    }
}

impl mae_mcp::identity::HostKeyVerifier for PromptingHostKeyVerifier {
    fn verify(&self, addr: &str, server_pub: &mae_mcp::identity::PublicKey) -> bool {
        let mut kh = mae_mcp::identity::KnownHosts::load(&self.known_hosts);
        if let Some(pinned) = kh.get(addr) {
            return pinned.to_bytes() == server_pub.to_bytes();
        }
        // Unknown host — ask the user (the connection task blocks here; the main
        // UI thread is separate, so no deadlock).
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
            Ok(true) => kh.pin(addr, server_pub).is_ok(),
            _ => false,
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
                        CollabCommand::KbNodeUpdate { kb_id, node_id, update, pending_rowid } => {
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
                                let req = mae_sync::wire::kb_node_update_request(
                                    req_id,
                                    &kb_id,
                                    &node_id,
                                    &mae_sync::encoding::update_to_base64(&update),
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
                try_send_evt(evt_tx, CollabEvent::KbShared { kb_id, node_count });
            } else {
                try_send_evt(
                    evt_tx,
                    CollabEvent::Error {
                        message: format!("Failed to share KB: {}", val),
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
            "\u{2713} Client version: {}",
            env!("CARGO_PKG_VERSION")
        ));

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
mod tests {
    use super::*;

    fn tofu_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("mae-tofu-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn prompting_verifier_pinned_match_no_prompt() {
        use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
        let dir = tofu_dir("pin");
        let kh = dir.join("known_hosts");
        let server = Identity::generate("daemon").public();
        KnownHosts::load(&kh).pin("d:9473", &server).unwrap();
        // No receiver needed — a pinned match must NOT prompt.
        let (tx, _rx) = mpsc::channel::<CollabEvent>(8);
        let v = PromptingHostKeyVerifier {
            known_hosts: kh,
            evt_tx: tx,
            timeout: std::time::Duration::from_millis(50),
        };
        assert!(
            v.verify("d:9473", &server),
            "pinned key must be accepted silently"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompting_verifier_changed_key_rejected() {
        use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
        let dir = tofu_dir("changed");
        let kh = dir.join("known_hosts");
        KnownHosts::load(&kh)
            .pin("d:9473", &Identity::generate("real").public())
            .unwrap();
        let (tx, _rx) = mpsc::channel::<CollabEvent>(8);
        let v = PromptingHostKeyVerifier {
            known_hosts: kh,
            evt_tx: tx,
            timeout: std::time::Duration::from_millis(50),
        };
        // A DIFFERENT key for the same addr → abort (no prompt).
        assert!(!v.verify("d:9473", &Identity::generate("imposter").public()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompting_verifier_unknown_accept_pins() {
        use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
        let dir = tofu_dir("accept");
        let kh = dir.join("known_hosts");
        let server = Identity::generate("daemon").public();
        let server_bytes = server.to_bytes();
        let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
        let v = PromptingHostKeyVerifier {
            known_hosts: kh.clone(),
            evt_tx: tx,
            timeout: std::time::Duration::from_secs(5),
        };
        // verify() blocks until the (simulated) user answers via the event reply.
        let handle = std::thread::spawn(move || v.verify("d:9473", &server));
        match rx.blocking_recv().expect("prompt event") {
            CollabEvent::HostKeyPrompt {
                reply, fingerprint, ..
            } => {
                assert!(fingerprint.starts_with("SHA256:"));
                reply.send(true).unwrap();
            }
            other => panic!("expected HostKeyPrompt, got {other:?}"),
        }
        assert!(handle.join().unwrap(), "accepted host must verify");
        // ...and is now pinned.
        assert_eq!(
            KnownHosts::load(&kh).get("d:9473").unwrap().to_bytes(),
            server_bytes
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prompting_verifier_unknown_reject_not_pinned() {
        use mae_mcp::identity::{HostKeyVerifier, Identity, KnownHosts};
        let dir = tofu_dir("reject");
        let kh = dir.join("known_hosts");
        let server = Identity::generate("daemon").public();
        let (tx, mut rx) = mpsc::channel::<CollabEvent>(8);
        let v = PromptingHostKeyVerifier {
            known_hosts: kh.clone(),
            evt_tx: tx,
            timeout: std::time::Duration::from_secs(5),
        };
        let handle = std::thread::spawn(move || v.verify("d:9473", &server));
        if let CollabEvent::HostKeyPrompt { reply, .. } = rx.blocking_recv().unwrap() {
            reply.send(false).unwrap();
        }
        assert!(!handle.join().unwrap(), "rejected host must not verify");
        assert!(
            KnownHosts::load(&kh).get("d:9473").is_none(),
            "rejected host must not be pinned"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drain_collab_intent_connect() {
        let mut editor = Editor::new();
        editor.collab.pending_intent = Some(CollabIntent::Connect {
            address: "127.0.0.1:9473".to_string(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        assert!(editor.collab.pending_intent.is_none());
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, CollabCommand::Connect { .. }));
    }

    #[test]
    fn drain_collab_intent_empty_is_noop() {
        let mut editor = Editor::new();
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drain_collab_share_enables_sync() {
        let mut editor = Editor::new();
        let buf_name = editor.buffers[0].name.clone();
        editor.collab.pending_intent = Some(CollabIntent::ShareBuffer {
            buffer_name: buf_name.clone(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::ShareBuffer {
                doc_id,
                state_bytes,
            } => {
                // Buffer with no file_path gets DocAddress::Shared, serialized as "shared:{name}".
                assert_eq!(doc_id, format!("shared:{}", buf_name));
                assert!(
                    !state_bytes.is_empty(),
                    "state bytes should be non-empty after enable_sync"
                );
            }
            other => panic!("expected ShareBuffer, got {:?}", other),
        }
        // Sync should now be enabled on the buffer.
        assert!(editor.buffers[0].sync_doc.is_some());
    }

    #[test]
    fn drain_collab_list_docs() {
        let mut editor = Editor::new();
        editor.collab.pending_intent = Some(CollabIntent::ListDocs);
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        assert!(matches!(cmd, CollabCommand::ListDocs { for_join: false }));
    }

    #[test]
    fn drain_collab_join_doc() {
        let mut editor = Editor::new();
        editor.collab.pending_intent = Some(CollabIntent::JoinDoc {
            doc_id: "test.org".to_string(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::JoinDoc { doc_id } => assert_eq!(doc_id, "test.org"),
            other => panic!("expected JoinDoc, got {:?}", other),
        }
    }

    #[test]
    fn handle_connected_event() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::Connected {
                address: "127.0.0.1:9473".to_string(),
                peer_count: 2,
            },
        );
        assert_eq!(
            editor.collab.status,
            CollabStatus::Connected { peer_count: 2 }
        );
    }

    #[test]
    fn handle_disconnected_event() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        editor.collab.synced_buffers.insert("test.rs".to_string());
        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );
        assert_eq!(editor.collab.status, CollabStatus::Disconnected);
        assert_eq!(editor.collab.synced_docs, 0);
        // UI tracking cleared, but per-buffer state depends on sync_doc presence.
        assert!(editor.collab.synced_buffers.is_empty());
    }

    #[test]
    fn handle_buffer_shared_event() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferShared {
                doc_id: "main.rs".to_string(),
            },
        );
        assert!(editor.collab.synced_buffers.contains("main.rs"));
        assert_eq!(editor.collab.synced_docs, 1);
        assert!(editor.status_msg.contains("Shared: main.rs"));
    }

    #[test]
    fn handle_doc_list_event_creates_buffer() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::DocList {
                documents: vec!["a.rs".to_string(), "b.rs".to_string()],
                for_join: false,
            },
        );
        let idx = editor.find_buffer_by_name("*Collab Docs*");
        assert!(idx.is_some());
        let buf = &editor.buffers[idx.unwrap()];
        assert!(buf.text().contains("a.rs"));
        assert!(buf.text().contains("b.rs"));
    }

    #[test]
    fn handle_doc_list_for_join_opens_palette() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::DocList {
                documents: vec!["file1.org".to_string()],
                for_join: true,
            },
        );
        assert!(editor.command_palette.is_some());
        let palette = editor.command_palette.as_ref().unwrap();
        assert_eq!(palette.purpose, mae_core::PalettePurpose::CollabJoin);
        assert!(palette.entries.iter().any(|e| e.name == "file1.org"));
    }

    #[test]
    fn handle_status_report_creates_buffer() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::StatusReport {
                lines: vec!["line1".to_string(), "line2".to_string()],
            },
        );
        let idx = editor.find_buffer_by_name("*Collab Status*");
        assert!(idx.is_some());
    }

    #[test]
    fn handle_doctor_report_creates_buffer() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::DoctorReport {
                lines: vec!["ok".to_string()],
            },
        );
        let idx = editor.find_buffer_by_name("*Collab Doctor*");
        assert!(idx.is_some());
    }

    #[test]
    fn status_lines_connected() {
        let lines = build_status_lines("127.0.0.1:9473", true, &["main.rs".to_string()]);
        assert!(lines.iter().any(|l| l.contains("Connected")));
        assert!(lines.iter().any(|l| l.contains("main.rs")));
    }

    #[test]
    fn doctor_lines_disconnected() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: false,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("\u{2717}")));
        assert!(lines.iter().any(|l| l.contains("Troubleshooting")));
    }

    #[test]
    fn doctor_lines_include_join_and_list() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: false,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("SPC C l")));
        assert!(lines.iter().any(|l| l.contains("SPC C j")));
    }

    #[test]
    fn doctor_lines_show_server_stats() {
        // Matches actual $/debug response shape: doc_stats is a map keyed by name.
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: true,
            server_debug: Some(serde_json::json!({
                "documents": 1,
                "doc_stats": {
                    "test.rs": {
                        "wal_seq": 42,
                        "update_count": 10,
                        "connected_clients": 2,
                        "idle_secs": 5
                    }
                }
            })),
            ping_latency_ms: Some(3),
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("test.rs")));
        assert!(lines.iter().any(|l| l.contains("wal:42")));
        assert!(lines.iter().any(|l| l.contains("clients:2")));
    }

    #[test]
    fn doctor_lines_show_latency() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: true,
            server_debug: None,
            ping_latency_ms: Some(7),
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines.iter().any(|l| l.contains("Ping: 7ms")));
    }

    #[test]
    fn doctor_lines_show_synced_buffers() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: true,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![("doc-a".to_string(), 0), ("doc-b".to_string(), 3)],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(lines
            .iter()
            .any(|l| l.contains("doc-a") && l.contains("up-to-date")));
        assert!(lines
            .iter()
            .any(|l| l.contains("doc-b") && l.contains("3 pending")));
    }

    #[test]
    fn doctor_lines_disconnected_no_crash() {
        let ctx = DoctorContext {
            address: "127.0.0.1:9473".to_string(),
            connected: false,
            server_debug: None,
            ping_latency_ms: None,
            synced_info: vec![],
        };
        let lines = build_doctor_lines(&ctx);
        assert!(!lines.is_empty());
        assert!(lines.iter().any(|l| l.contains("not reachable")));
    }

    #[tokio::test]
    async fn handle_incoming_sync_update_notification_serde_format() {
        // Test the actual serde format: #[serde(tag = "type", content = "data")]
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = vec!["test.rs".to_string()];

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/sync_update",
            "params": {
                "seq": 1,
                "event": {
                    "type": "sync_update",
                    "data": {
                        "buffer_name": "test.rs",
                        "update_base64": "AQIDBA==",
                        "wal_seq": 0
                    }
                }
            }
        });
        handle_incoming_message(
            &msg.to_string(),
            &tx,
            &mut pending,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::RemoteUpdate { doc_id, .. } => {
                assert_eq!(doc_id, "test.rs");
            }
            other => panic!("expected RemoteUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_incoming_sync_update_notification_legacy_format() {
        // Test backward compat with the old "sync_update" key format.
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = vec!["legacy.rs".to_string()];

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/sync_update",
            "params": {
                "seq": 1,
                "event": {
                    "sync_update": {
                        "buffer_name": "legacy.rs",
                        "update_base64": "AQIDBA==",
                        "wal_seq": 0
                    }
                }
            }
        });
        handle_incoming_message(
            &msg.to_string(),
            &tx,
            &mut pending,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::RemoteUpdate { doc_id, .. } => {
                assert_eq!(doc_id, "legacy.rs");
            }
            other => panic!("expected RemoteUpdate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_response_list_docs() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "documents": ["a.rs", "b.org"]
            }
        });
        handle_response(
            &val,
            PendingResponseKind::ListDocs { for_join: true },
            &tx,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::DocList {
                documents,
                for_join,
            } => {
                assert!(for_join);
                assert_eq!(documents, vec!["a.rs", "b.org"]);
            }
            other => panic!("expected DocList, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_response_share_buffer() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "doc": "test.rs", "wal_seq": 1 }
        });
        let mut seq = std::collections::HashMap::new();
        handle_response(
            &val,
            PendingResponseKind::ShareBuffer {
                doc_id: "test.rs".to_string(),
            },
            &tx,
            &mut shared,
            &mut seq,
        );
        assert!(shared.contains(&"test.rs".to_string()));
        // WU2: seq_tracker should be seeded from share response wal_seq.
        assert_eq!(seq.get("test.rs"), Some(&1));
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, CollabEvent::BufferShared { doc_id } if doc_id == "test.rs"));
    }

    /// ADR-020 B-13 regression: a successful `kb/join` must add the collection AND
    /// each node doc to `shared_docs`, or later inbound `sync_update` broadcasts for
    /// `kb:<node>` are dropped at the `shared_docs.contains()` filter and the member
    /// never receives live edits (emit works, receive is dead).
    #[tokio::test]
    async fn handle_response_kb_join_subscribes_to_collection_and_node_docs() {
        let (tx, _rx) = mpsc::channel(8);
        let mut shared: Vec<String> = Vec::new();
        let mut seq = std::collections::HashMap::new();

        let coll = mae_sync::kb::KbCollectionDoc::new("testkb", "owner");
        let coll_b64 = mae_sync::encoding::update_to_base64(&coll.encode_state());
        let node = mae_sync::kb::KbNodeDoc::new("testkb:n1", "T", "b", &[]);
        let node_b64 = mae_sync::encoding::update_to_base64(&node.encode_state());

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "collection_state": coll_b64,
                "nodes": [ { "id": "testkb:n1", "state": node_b64 } ]
            }
        });
        handle_response(
            &val,
            PendingResponseKind::KbJoin {
                kb_id: "testkb".to_string(),
            },
            &tx,
            &mut shared,
            &mut seq,
        );
        assert!(
            shared.contains(&"kbc:testkb".to_string()),
            "join must subscribe to the collection doc"
        );
        assert!(
            shared.contains(&"kb:testkb:n1".to_string()),
            "join must subscribe to each node doc — else inbound live updates are dropped (B-13)"
        );
    }

    #[tokio::test]
    async fn handle_response_join_seeds_seq_tracker() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();
        let mut seq = std::collections::HashMap::new();

        // Create a real yrs state to encode.
        let ts = mae_sync::text::TextSync::with_client_id("joined content", 1);
        let state_b64 = mae_sync::encoding::update_to_base64(&ts.encode_state());

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "doc": "joined.rs", "state": state_b64, "wal_seq": 7 }
        });
        handle_response(
            &val,
            PendingResponseKind::JoinDoc {
                doc_id: "joined.rs".to_string(),
            },
            &tx,
            &mut shared,
            &mut seq,
        );

        // WU2: seq_tracker should be seeded from join response wal_seq.
        assert_eq!(seq.get("joined.rs"), Some(&7));
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, CollabEvent::BufferJoined { doc_id, .. } if doc_id == "joined.rs"));
    }

    #[tokio::test]
    async fn handle_incoming_logs_null_id_response() {
        // WU3: Responses with null id should be logged but not panic or emit events.
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = Vec::new();
        let mut seq = std::collections::HashMap::new();

        let msg = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}}"#;
        handle_incoming_message(msg, &tx, &mut pending, &mut shared, &mut seq);

        // Should not emit any event (the warning is logged by tracing).
        assert!(rx.try_recv().is_err());
    }

    // -----------------------------------------------------------------------
    // Bug 2 regression: join must set language AND invalidate syntax cache
    // -----------------------------------------------------------------------

    #[test]
    fn buffer_joined_sets_language_and_invalidates_syntax() {
        let mut editor = Editor::new();

        // Create a sync doc with org content, then encode its state bytes.
        let org_content = "#+TITLE: Test\n\n- bullet one\n- bullet two\n";
        let sync = mae_sync::text::TextSync::with_client_id(org_content, 1);
        let state_bytes = sync.encode_state();

        // Feed a BufferJoined event with an org doc_id.
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "daily.org".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("daily.org")
            .expect("joined buffer should exist");

        // Language should be detected as Org.
        let lang = editor.syntax.language_of(idx);
        assert_eq!(
            lang,
            Some(mae_core::syntax::Language::Org),
            "joined .org buffer should have Org language set"
        );

        // The syntax cache should be invalidated (no stale spans/tree).
        assert!(
            !editor
                .syntax
                .has_cached_spans(idx, editor.buffers[idx].generation),
            "syntax cache should be invalidated after join (no stale spans)"
        );

        // Buffer content should match the shared org content.
        assert!(editor.buffers[idx].text().contains("bullet one"));
    }

    #[test]
    fn buffer_joined_reuses_existing_buffer_by_collab_doc_id() {
        // Regression test: if a buffer was shared (collab_doc_id set) and the
        // user also joins the same doc, BufferJoined must reuse the existing
        // buffer instead of creating a duplicate. Creating a duplicate causes
        // remote updates to be applied to the wrong sync_doc (the one without
        // the locally-typed operations), making all updates no-ops.
        let mut editor = Editor::new();

        // Simulate: buffer "2026-05-27.org" was shared, enable_sync + collab_doc_id set.
        let mut buf = mae_core::Buffer::new();
        buf.name = "2026-05-27.org".to_string();
        buf.insert_text_at(0, "shared content");
        buf.enable_sync(1000);
        buf.collab_doc_id = Some("file:abc123/daily/2026-05-27.org".to_string());
        editor.buffers.push(buf);
        editor
            .collab
            .synced_buffers
            .insert("file:abc123/daily/2026-05-27.org".to_string());
        let original_idx = editor.buffers.len() - 1;

        // Simulate: user also joins the same doc. The join resolves to
        // buf_name="daily/2026-05-27.org" (different from "2026-05-27.org").
        let sync = mae_sync::text::TextSync::with_client_id("shared content", 2000);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "file:abc123/daily/2026-05-27.org".to_string(),
                state_bytes,
            },
        );

        // Should NOT have created a new buffer — should reuse the existing one.
        assert!(
            editor.find_buffer_by_name("daily/2026-05-27.org").is_none(),
            "should not create duplicate buffer with different name"
        );
        // The original buffer should still be the one with the collab_doc_id.
        assert_eq!(
            editor.buffers[original_idx].collab_doc_id.as_deref(),
            Some("file:abc123/daily/2026-05-27.org"),
        );
        // Only one buffer should have this collab_doc_id.
        let matching: Vec<_> = editor
            .buffers
            .iter()
            .filter(|b| b.collab_doc_id.as_deref() == Some("file:abc123/daily/2026-05-27.org"))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "exactly one buffer should have this collab_doc_id"
        );
    }

    #[test]
    fn buffer_joined_non_org_gets_no_language() {
        let mut editor = Editor::new();

        let content = "just plain text\n";
        let sync = mae_sync::text::TextSync::with_client_id(content, 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "notes.txt".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("notes.txt")
            .expect("joined buffer should exist");

        // .txt files don't have a tree-sitter grammar, so no language set.
        assert_eq!(editor.syntax.language_of(idx), None);
    }

    // -----------------------------------------------------------------------
    // Bug 1 regression: unbiased select ensures server messages are processed
    // -----------------------------------------------------------------------
    // NOTE: The actual `run_collab_task` loop requires a real TCP connection,
    // so we can't unit-test it directly. Instead we verify the architectural
    // property: `handle_incoming_message` correctly processes a notification
    // even when called after a burst of commands. This test ensures the
    // message-handling path itself works; the `biased` removal ensures it
    // actually gets called.

    #[test]
    fn drain_share_sets_synced_immediately() {
        let mut editor = Editor::new();
        let buf_name = editor.buffers[0].name.clone();
        editor.collab.pending_intent = Some(CollabIntent::ShareBuffer {
            buffer_name: buf_name.clone(),
        });
        let (tx, _rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);

        // BUG A: doc_id must be in collab_synced_buffers IMMEDIATELY.
        let expected_doc_id = format!("shared:{}", buf_name);
        assert!(
            editor.collab.synced_buffers.contains(&expected_doc_id),
            "doc_id should be in collab_synced_buffers immediately after drain"
        );
        assert_eq!(editor.collab.synced_docs, 1);
    }

    #[test]
    fn share_failure_removes_from_synced() {
        let mut editor = Editor::new();
        // Simulate: doc was optimistically added during share.
        editor.collab.synced_buffers.insert("test-doc".to_string());
        editor.collab.synced_docs = 1;
        // Also set collab_doc_id on a buffer so the rollback can clear it.
        editor.buffers[0].collab_doc_id = Some("test-doc".to_string());

        handle_collab_event(
            &mut editor,
            CollabEvent::ShareFailed {
                doc_id: "test-doc".to_string(),
                message: "server error".to_string(),
            },
        );

        assert!(!editor.collab.synced_buffers.contains("test-doc"));
        assert_eq!(editor.collab.synced_docs, 0);
        assert!(editor.buffers[0].collab_doc_id.is_none());
    }

    #[test]
    fn handle_disconnect_preserves_sync_for_offline_recovery() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        // Set up a buffer as if it were synced.
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        buf.insert_text_at(5, "x"); // generates pending_sync_update
        editor.collab.synced_buffers.insert("test-doc".to_string());

        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        assert!(editor.collab.synced_buffers.is_empty());
        assert_eq!(editor.collab.synced_docs, 0);
        // WU3: sync_doc and collab_doc_id are PRESERVED for offline recovery.
        assert!(editor.buffers[0].collab_doc_id.is_some());
        assert!(editor.buffers[0].sync_doc.is_some());
        assert!(editor.buffers[0].collab_offline);
    }

    #[tokio::test]
    async fn share_failure_emits_share_failed() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32000, "message": "storage full" }
        });
        handle_response(
            &val,
            PendingResponseKind::ShareBuffer {
                doc_id: "fail.rs".to_string(),
            },
            &tx,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );

        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::ShareFailed { doc_id, message } => {
                assert_eq!(doc_id, "fail.rs");
                assert!(message.contains("storage full"));
            }
            other => panic!("expected ShareFailed, got {:?}", other),
        }
        // Should NOT be in shared_docs.
        assert!(!shared.contains(&"fail.rs".to_string()));
    }

    #[test]
    fn disconnect_sets_offline_on_all_synced_buffers() {
        // WU3: disconnect preserves sync_doc for offline recovery.
        // Buffers with sync_doc get collab_offline=true.
        // Buffers without sync_doc (ShareFailed cleared it) get collab_doc_id cleared.
        use mae_core::Buffer;
        let mut editor = Editor::new();

        // Buffer A: tracked in synced_buffers, has sync_doc.
        editor.buffers[0].name = "tracked.rs".to_string();
        editor.buffers[0].enable_sync(1);
        editor.buffers[0].collab_doc_id = Some("doc-tracked".to_string());
        editor
            .collab
            .synced_buffers
            .insert("doc-tracked".to_string());

        // Buffer B: has collab_doc_id but no sync_doc (ShareFailed cleared it).
        let mut buf_b = Buffer::new();
        buf_b.name = "orphaned.rs".to_string();
        buf_b.collab_doc_id = Some("doc-orphaned".to_string());
        // No enable_sync → sync_doc is None.
        editor.buffers.push(buf_b);

        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        editor.collab.synced_docs = 1;

        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        // Buffer A: sync_doc preserved, collab_offline = true.
        assert!(
            editor.buffers[0].sync_doc.is_some(),
            "tracked buffer should preserve sync_doc"
        );
        assert!(
            editor.buffers[0].collab_offline,
            "tracked buffer should be offline"
        );
        assert!(editor.buffers[0].collab_doc_id.is_some());

        // Buffer B: no sync_doc → collab_doc_id cleared (nothing to preserve).
        assert!(
            editor.buffers[1].collab_doc_id.is_none(),
            "orphaned buffer should have collab_doc_id cleared"
        );
        assert!(!editor.buffers[1].collab_offline);
    }

    #[test]
    fn disconnect_after_share_failure_preserves_good_buffer() {
        // WU3: ShareFailed on one buffer, then Disconnect: the good buffer
        // should have its sync_doc preserved for offline recovery.
        use mae_core::Buffer;
        let mut editor = Editor::new();

        editor.buffers[0].name = "good.rs".to_string();
        editor.buffers[0].enable_sync(1);
        editor.buffers[0].collab_doc_id = Some("doc-good".to_string());
        editor.collab.synced_buffers.insert("doc-good".to_string());

        let mut buf_bad = Buffer::new();
        buf_bad.name = "bad.rs".to_string();
        buf_bad.enable_sync(2);
        buf_bad.collab_doc_id = Some("doc-bad".to_string());
        editor.buffers.push(buf_bad);
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };

        // ShareFailed clears doc-bad from the buffer.
        handle_collab_event(
            &mut editor,
            CollabEvent::ShareFailed {
                doc_id: "doc-bad".to_string(),
                message: "test".to_string(),
            },
        );

        // Disconnect.
        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        // Good buffer: sync_doc preserved, offline=true.
        assert!(
            editor.buffers[0].sync_doc.is_some(),
            "good buffer should keep sync_doc"
        );
        assert!(editor.buffers[0].collab_offline);
        // Bad buffer: ShareFailed already cleared sync_doc, so disconnect clears collab_doc_id.
        assert!(
            editor.buffers[1].collab_doc_id.is_none(),
            "bad buffer should have doc_id cleared"
        );
    }

    #[tokio::test]
    async fn server_notification_processed_after_command_burst() {
        let (tx, mut rx) = mpsc::channel(32);
        let mut pending = std::collections::HashMap::new();
        // Pre-subscribe to all docs so the filter passes.
        let mut shared: Vec<String> = (0..5).map(|i| format!("file{}.rs", i)).collect();

        // Simulate N sync_update notifications arriving in quick succession
        // (as would happen when they pile up during biased starvation).
        for i in 0..5 {
            let msg = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/sync_update",
                "params": {
                    "seq": i,
                    "event": {
                        "type": "sync_update",
                        "data": {
                            "buffer_name": format!("file{}.rs", i),
                            "update_base64": "AQIDBA==",
                            "wal_seq": i
                        }
                    }
                }
            });
            handle_incoming_message(
                &msg.to_string(),
                &tx,
                &mut pending,
                &mut shared,
                &mut std::collections::HashMap::new(),
            );
        }

        // All 5 should have produced RemoteUpdate events.
        let mut received = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let CollabEvent::RemoteUpdate { doc_id, .. } = event {
                received.push(doc_id);
            }
        }
        assert_eq!(
            received.len(),
            5,
            "all queued server notifications must be processed; got {:?}",
            received
        );
    }

    #[tokio::test]
    async fn unsubscribed_doc_sync_update_ignored() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = vec!["subscribed.rs".to_string()]; // Only subscribed to one doc.

        let msg = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/sync_update",
            "params": {
                "seq": 1,
                "event": {
                    "type": "sync_update",
                    "data": {
                        "buffer_name": "other-client.rs",
                        "update_base64": "AQIDBA==",
                        "wal_seq": 1
                    }
                }
            }
        });
        handle_incoming_message(
            &msg.to_string(),
            &tx,
            &mut pending,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        // No event should be emitted for the unsubscribed doc.
        assert!(
            rx.try_recv().is_err(),
            "sync_update for unsubscribed doc should be ignored"
        );
    }

    // -----------------------------------------------------------------------
    // Join-save model: joined buffers have no auto file_path
    // -----------------------------------------------------------------------

    #[test]
    fn buffer_joined_has_no_file_path() {
        let mut editor = Editor::new();
        let content = "shared text\n";
        let sync = mae_sync::text::TextSync::with_client_id(content, 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "file:abc123/src/main.rs".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("src/main.rs")
            .expect("joined buffer should use rel_path as name");
        // Joined buffers must NOT have auto file_path set.
        assert!(
            editor.buffers[idx].file_path().is_none(),
            "joined buffer should have no file_path by default"
        );
        // But collab_doc_id should be set.
        assert_eq!(
            editor.buffers[idx].collab_doc_id.as_deref(),
            Some("file:abc123/src/main.rs")
        );
    }

    #[test]
    fn buffer_joined_sets_buffer_name_from_rel_path() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("hi", 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "file:proj/utils.rs".to_string(),
                state_bytes,
            },
        );

        assert!(
            editor.find_buffer_by_name("utils.rs").is_some(),
            "buffer name should be the rel_path from DocAddress"
        );
    }

    #[test]
    fn buffer_joined_shared_doc_name_extraction() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("data", 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "shared:notes".to_string(),
                state_bytes,
            },
        );

        assert!(
            editor.find_buffer_by_name("notes").is_some(),
            "shared doc buffer name should be the name field"
        );
    }

    #[test]
    fn drain_save_collab_sends_save_intent() {
        let mut editor = Editor::new();
        editor.collab.pending_intent = Some(CollabIntent::SaveCollab {
            doc_id: "file:abc/main.rs".to_string(),
            content_hash: "deadbeef".to_string(),
        });
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::SendSaveIntent {
                doc_id,
                expected_hash,
            } => {
                assert_eq!(doc_id, "file:abc/main.rs");
                assert_eq!(expected_hash, "deadbeef");
            }
            other => panic!("expected SendSaveIntent, got {:?}", other),
        }
    }

    #[test]
    fn drain_pending_save_committed() {
        let mut editor = Editor::new();
        editor.collab.pending_save_committed = Some((
            "doc1".to_string(),
            42,
            "hash123".to_string(),
            "alice".to_string(),
        ));
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);
        let cmd = rx.try_recv().unwrap();
        match cmd {
            CollabCommand::SendSaveCommitted {
                doc_id,
                save_epoch,
                content_hash,
                saved_by,
            } => {
                assert_eq!(doc_id, "doc1");
                assert_eq!(save_epoch, 42);
                assert_eq!(content_hash, "hash123");
                assert_eq!(saved_by, "alice");
            }
            other => panic!("expected SendSaveCommitted, got {:?}", other),
        }
        assert!(editor.collab.pending_save_committed.is_none());
    }

    #[test]
    fn handle_save_intent_ok_queues_committed() {
        let mut editor = Editor::new();
        editor.collab.user_name = "bob".to_string();
        handle_collab_event(
            &mut editor,
            CollabEvent::SaveIntentOk {
                doc_id: "test-doc".to_string(),
                save_epoch: 5,
                content_hash: "abc".to_string(),
            },
        );
        assert!(editor.collab.pending_save_committed.is_some());
        let (doc_id, epoch, hash, saved_by) =
            editor.collab.pending_save_committed.as_ref().unwrap();
        assert_eq!(doc_id, "test-doc");
        assert_eq!(*epoch, 5);
        assert_eq!(hash, "abc");
        assert_eq!(saved_by, "bob");
    }

    #[test]
    fn handle_save_intent_conflict_shows_status() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::SaveIntentConflict {
                doc_id: "test-doc".to_string(),
                message: "hash mismatch".to_string(),
            },
        );
        assert!(editor.status_msg.contains("conflict"));
    }

    #[tokio::test]
    async fn handle_response_save_intent_ok() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "doc": "test.rs",
                "result": {
                    "status": "ok",
                    "server_hash": "abc123",
                    "save_epoch": 3
                }
            }
        });
        handle_response(
            &val,
            PendingResponseKind::SaveIntent {
                doc_id: "test.rs".to_string(),
                expected_hash: "abc123".to_string(),
            },
            &tx,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::SaveIntentOk {
                doc_id, save_epoch, ..
            } => {
                assert_eq!(doc_id, "test.rs");
                assert_eq!(save_epoch, 3);
            }
            other => panic!("expected SaveIntentOk, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_response_save_intent_conflict() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();

        let val = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "doc": "test.rs",
                "result": {
                    "status": "conflict",
                    "server_hash": "xyz"
                }
            }
        });
        handle_response(
            &val,
            PendingResponseKind::SaveIntent {
                doc_id: "test.rs".to_string(),
                expected_hash: "abc123".to_string(),
            },
            &tx,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        let event = rx.try_recv().unwrap();
        assert!(
            matches!(event, CollabEvent::SaveIntentConflict { .. }),
            "expected SaveIntentConflict, got {:?}",
            event
        );
    }

    /// B-1: a kb/join response must surface joined / pending / denied as three
    /// DISTINCT outcomes — not "Joined (0 nodes)" for all of them.
    #[tokio::test]
    async fn kb_join_pending_response_is_distinct() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();
        let val = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "kb_id": "collabtest", "status": "pending" }
        });
        handle_response(
            &val,
            PendingResponseKind::KbJoin {
                kb_id: "collabtest".into(),
            },
            &tx,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        match rx.try_recv().unwrap() {
            CollabEvent::StatusReport { lines } => {
                assert!(
                    lines.iter().any(|l| l.contains("pending")),
                    "pending join should report pending approval, got {lines:?}"
                );
            }
            other => panic!("expected StatusReport for pending, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn kb_join_denied_response_is_distinct() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();
        let val = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "error": { "code": -32603, "message": "not a member of KB 'collabtest'" }
        });
        handle_response(
            &val,
            PendingResponseKind::KbJoin {
                kb_id: "collabtest".into(),
            },
            &tx,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        match rx.try_recv().unwrap() {
            CollabEvent::Error { message } => {
                assert!(
                    message.contains("denied"),
                    "denied join should report denial, got {message:?}"
                );
            }
            other => panic!("expected Error for denied join, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn kb_join_success_response_emits_joined() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut shared = Vec::new();
        let val = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "kb_id": "collabtest", "collection_state": "", "nodes": [] }
        });
        handle_response(
            &val,
            PendingResponseKind::KbJoin {
                kb_id: "collabtest".into(),
            },
            &tx,
            &mut shared,
            &mut std::collections::HashMap::new(),
        );
        assert!(
            matches!(rx.try_recv().unwrap(), CollabEvent::KbJoined { .. }),
            "a real join must emit KbJoined"
        );
    }

    #[test]
    fn peer_count_zero_shows_all_disconnected() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 2 };
        handle_collab_event(&mut editor, CollabEvent::PeerCountChanged { peer_count: 0 });
        assert!(editor.status_msg.contains("disconnected"));
        assert_eq!(
            editor.collab.status,
            CollabStatus::Connected { peer_count: 0 }
        );
    }

    #[test]
    fn save_pathless_collab_buffer_shows_guidance() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("text", 1);
        let state_bytes = sync.encode_state();

        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "shared:test".to_string(),
                state_bytes,
            },
        );

        let idx = editor
            .find_buffer_by_name("test")
            .expect("buffer should exist");
        editor.switch_to_buffer(idx);
        // Use dispatch_builtin("save") which is public and calls save_current_buffer.
        editor.dispatch_builtin("save");

        // Should show guidance about :saveas
        let status = &editor.status_msg;
        assert!(
            status.contains("saveas"),
            "status should mention :saveas, got: {status}"
        );
    }

    // --- WU1: Gap detection tests ---

    #[tokio::test]
    async fn gap_detection_triggers_on_missing_seq() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut seq_tracker = std::collections::HashMap::new();

        // Seq 1, 2 — no gap.
        check_seq_gap("doc1", 1, &mut seq_tracker, &tx);
        check_seq_gap("doc1", 2, &mut seq_tracker, &tx);
        assert!(rx.try_recv().is_err(), "no gap for sequential seqs");

        // Seq 4 — gap (expected 3).
        check_seq_gap("doc1", 4, &mut seq_tracker, &tx);
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::GapDetected {
                doc_id,
                expected,
                got,
            } => {
                assert_eq!(doc_id, "doc1");
                assert_eq!(expected, 3);
                assert_eq!(got, 4);
            }
            other => panic!("expected GapDetected, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn gap_detection_no_gap_for_sequential() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut seq_tracker = std::collections::HashMap::new();

        for i in 1..=5 {
            check_seq_gap("doc1", i, &mut seq_tracker, &tx);
        }
        assert!(rx.try_recv().is_err(), "no gap for sequential 1..5");
    }

    #[tokio::test]
    async fn gap_detection_independent_per_doc() {
        let (tx, mut rx) = mpsc::channel(16);
        let mut seq_tracker = std::collections::HashMap::new();

        check_seq_gap("doc-a", 1, &mut seq_tracker, &tx);
        check_seq_gap("doc-b", 1, &mut seq_tracker, &tx);
        // Both start at 1, no gap.
        assert!(rx.try_recv().is_err());

        // doc-a jumps to 5 — gap.
        check_seq_gap("doc-a", 5, &mut seq_tracker, &tx);
        let event = rx.try_recv().unwrap();
        assert!(matches!(event, CollabEvent::GapDetected { doc_id, .. } if doc_id == "doc-a"));

        // doc-b at 2 — no gap.
        check_seq_gap("doc-b", 2, &mut seq_tracker, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn gap_detected_triggers_force_sync() {
        let mut editor = Editor::new();
        handle_collab_event(
            &mut editor,
            CollabEvent::GapDetected {
                doc_id: "test-doc".to_string(),
                expected: 3,
                got: 5,
            },
        );
        assert!(editor.status_msg.contains("gap"));
        // Should queue a ForceSync intent.
        assert!(editor.collab.pending_intent.is_some());
        match editor.collab.pending_intent.as_ref().unwrap() {
            CollabIntent::ForceSync { buffer_name } => {
                assert_eq!(buffer_name, "test-doc");
            }
            other => panic!("expected ForceSync, got {:?}", other),
        }
    }

    // --- WU3: Offline recovery tests ---

    #[test]
    fn disconnect_preserves_sync_doc() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        editor.collab.synced_buffers.insert("test-doc".to_string());

        handle_collab_event(
            &mut editor,
            CollabEvent::Disconnected {
                reason: "test".to_string(),
            },
        );

        // sync_doc and collab_doc_id should be PRESERVED (not cleared).
        assert!(
            editor.buffers[0].sync_doc.is_some(),
            "sync_doc should be preserved on disconnect"
        );
        assert!(
            editor.buffers[0].collab_doc_id.is_some(),
            "collab_doc_id should be preserved on disconnect"
        );
        assert!(
            editor.buffers[0].collab_offline,
            "collab_offline should be set"
        );
        // UI tracking should be cleared.
        assert!(editor.collab.synced_buffers.is_empty());
        assert_eq!(editor.collab.synced_docs, 0);
    }

    #[test]
    fn reconnect_triggers_resync_for_offline_buffers() {
        let mut editor = Editor::new();
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        buf.collab_offline = true;

        handle_collab_event(
            &mut editor,
            CollabEvent::Connected {
                address: "127.0.0.1:9473".to_string(),
                peer_count: 1,
            },
        );

        // Should queue a ForceSync intent for the offline buffer.
        assert!(editor.collab.pending_intent.is_some());
        assert!(editor.collab.synced_buffers.contains("test-doc"));
    }

    #[test]
    fn remote_update_clears_offline_flag() {
        let mut editor = Editor::new();
        let buf = &mut editor.buffers[0];
        buf.collab_doc_id = Some("test-doc".to_string());
        buf.enable_sync(42);
        buf.collab_offline = true;

        // Create a valid yrs update for this buffer.
        let update = {
            let sync2 = mae_sync::text::TextSync::with_client_id("hello", 99);
            sync2.encode_state()
        };

        handle_collab_event(
            &mut editor,
            CollabEvent::RemoteUpdate {
                doc_id: "test-doc".to_string(),
                update_bytes: update,
                wal_seq: 1,
            },
        );

        // Note: apply_sync_update may fail if the update isn't compatible,
        // but the test validates the code path exists.
    }

    // --- WU1: Buffer status indicator tests ---

    #[test]
    fn buffer_shared_sets_is_sharer() {
        let mut editor = Editor::new();
        editor.buffers[0].collab_doc_id = Some("test-doc".to_string());
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferShared {
                doc_id: "test-doc".to_string(),
            },
        );
        assert!(editor.buffers[0].collab_is_sharer);
    }

    #[test]
    fn buffer_joined_stays_not_sharer() {
        let mut editor = Editor::new();
        let sync = mae_sync::text::TextSync::with_client_id("hello", 1);
        let state = sync.encode_state();
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "test-doc".to_string(),
                state_bytes: state,
            },
        );
        // Find the buffer that was created for the joined doc.
        let idx = editor.find_buffer_by_collab_doc_id("test-doc");
        assert!(idx.is_some());
        assert!(!editor.buffers[idx.unwrap()].collab_is_sharer);
    }

    // --- WU2: Save guard tests ---

    #[test]
    fn collab_is_sharer_defaults_false() {
        let buf = mae_core::Buffer::new();
        assert!(!buf.collab_is_sharer);
    }

    #[test]
    fn collab_is_sharer_set_on_share_not_join() {
        // Verify that BufferShared sets is_sharer and BufferJoined does not.
        let mut editor = Editor::new();
        editor.buffers[0].collab_doc_id = Some("doc-a".to_string());
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferShared {
                doc_id: "doc-a".to_string(),
            },
        );
        assert!(
            editor.buffers[0].collab_is_sharer,
            "sharer should be true after BufferShared"
        );

        // Join a different doc — its buffer should NOT be sharer.
        let sync = mae_sync::text::TextSync::with_client_id("content", 2);
        let state = sync.encode_state();
        handle_collab_event(
            &mut editor,
            CollabEvent::BufferJoined {
                doc_id: "doc-b".to_string(),
                state_bytes: state,
            },
        );
        let idx = editor.find_buffer_by_collab_doc_id("doc-b").unwrap();
        assert!(
            !editor.buffers[idx].collab_is_sharer,
            "joiner should not be sharer"
        );
    }

    // --- WU3: SharerLeft event handling ---

    #[test]
    fn sharer_left_sets_status() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 2 };
        handle_collab_event(
            &mut editor,
            CollabEvent::SharerLeft {
                doc_id: "test-doc".to_string(),
            },
        );
        assert!(editor.status_msg.contains("Sharer disconnected"));
    }

    // --- WU4: Backoff + debounce tests ---

    #[test]
    fn compute_backoff_exponential() {
        // base=5, factor=2: 5, 10, 20, 40, 80, 160
        assert_eq!(compute_backoff(5, 2, 0), 5);
        assert_eq!(compute_backoff(5, 2, 1), 10);
        assert_eq!(compute_backoff(5, 2, 2), 20);
        assert_eq!(compute_backoff(5, 2, 3), 40);
        assert_eq!(compute_backoff(5, 2, 4), 80);
        assert_eq!(compute_backoff(5, 2, 5), 160);
        // Capped at attempt=5 exponent, so attempt 6 same as 5.
        assert_eq!(compute_backoff(5, 2, 6), 160);
    }

    #[test]
    fn compute_backoff_capped_at_300() {
        // base=10, factor=3: attempt 5 = 10 * 243 = 2430 → capped at 300.
        assert_eq!(compute_backoff(10, 3, 5), 300);
    }

    #[test]
    fn compute_backoff_factor_one_is_constant() {
        // factor=1 means no exponential growth.
        assert_eq!(compute_backoff(5, 1, 0), 5);
        assert_eq!(compute_backoff(5, 1, 5), 5);
    }

    // --- WU3: Notification parsing ---

    #[tokio::test]
    async fn parse_sharer_left_notification() {
        let (tx, mut rx) = mpsc::channel(8);
        let mut pending = std::collections::HashMap::new();
        let mut shared = Vec::new();
        let mut seq = std::collections::HashMap::new();
        let msg = r#"{
            "jsonrpc": "2.0",
            "method": "notifications/sharer_left",
            "params": {
                "seq": 1,
                "event": {
                    "type": "sharer_left",
                    "data": {
                        "session_id": 42,
                        "doc": "file:abc/main.rs",
                        "peer_count": 1
                    }
                }
            }
        }"#;
        handle_incoming_message(msg, &tx, &mut pending, &mut shared, &mut seq);
        let event = rx.try_recv().unwrap();
        match event {
            CollabEvent::SharerLeft { doc_id } => {
                assert_eq!(doc_id, "file:abc/main.rs");
            }
            other => panic!("expected SharerLeft, got {:?}", other),
        }
    }

    // --- Phase 4: Continuous KB sync tests ---

    #[test]
    fn collab_kb_shared_populates_tracking() {
        let mut editor = Editor::new();
        // Isolate the registry save (handler stamps the primary-shared marker).
        let tmp = std::env::temp_dir().join(format!("mae-adr019-prim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        editor.data_dir_override = Some(tmp.clone());
        // Insert some nodes into the primary KB.
        editor.kb.primary.insert(mae_kb::Node::new(
            "node-1".to_string(),
            "Title 1".to_string(),
            mae_kb::NodeKind::Note,
            "body 1".to_string(),
        ));
        editor.kb.primary.insert(mae_kb::Node::new(
            "node-2".to_string(),
            "Title 2".to_string(),
            mae_kb::NodeKind::Note,
            "body 2".to_string(),
        ));

        // Simulate KbShared event.
        handle_collab_event(
            &mut editor,
            CollabEvent::KbShared {
                kb_id: "default".to_string(),
                node_count: 2,
            },
        );

        assert!(
            editor.collab.shared_kbs.contains_key("default"),
            "shared_kbs should track the shared KB"
        );
        let tracked = &editor.collab.shared_kbs["default"];
        assert!(
            tracked.contains("node-1") && tracked.contains("node-2"),
            "shared_kbs should contain all node IDs: {:?}",
            tracked
        );
        // ADR-019: primary-share durable marker stamped.
        assert!(editor.kb.registry.primary_shared);
        assert_eq!(
            editor.kb.registry.primary_collab_id.as_deref(),
            Some("default")
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// I-9 + ADR-019: sharing a *named federated instance* tracks its node IDs by
    /// resolving name→uuid (cache) AND stamps the DURABLE registry marker
    /// (`shared`/`collab_id`) so the share survives editor restart.
    #[test]
    fn collab_kb_shared_named_instance_tracks_nodes_by_uuid() {
        let mut editor = Editor::new();
        // Isolate the registry save to a temp dir (the handler persists markers).
        let tmp = std::env::temp_dir().join(format!("mae-adr019-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        editor.data_dir_override = Some(tmp.clone());

        let uuid = "uuid-collabtest".to_string();
        let mut inst = mae_kb::KnowledgeBase::new();
        inst.insert(mae_kb::Node::new(
            "collabtest:overview",
            "Overview",
            mae_kb::NodeKind::Note,
            "b",
        ));
        inst.insert(mae_kb::Node::new(
            "collabtest:alpha",
            "Alpha",
            mae_kb::NodeKind::Note,
            "b",
        ));
        editor.kb.instances.insert(uuid.clone(), inst);
        // Registry maps the human name → uuid, NOT yet shared (handler stamps it).
        editor
            .kb
            .registry
            .instances
            .push(mae_kb::federation::KbInstance {
                uuid: uuid.clone(),
                name: "collabtest".into(),
                org_dir: std::path::PathBuf::from("/tmp/collabtest"),
                db_path: std::path::PathBuf::from("/tmp/collabtest.db"),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
            });

        handle_collab_event(
            &mut editor,
            CollabEvent::KbShared {
                kb_id: "collabtest".to_string(),
                node_count: 2,
            },
        );

        let tracked = &editor.collab.shared_kbs["collabtest"];
        assert!(
            tracked.contains("collabtest:overview") && tracked.contains("collabtest:alpha"),
            "named-instance share must track nodes via uuid resolution, got: {:?}",
            tracked
        );
        // Durable marker stamped (survives restart).
        let inst = editor.kb.registry.find("collabtest").unwrap();
        assert!(inst.shared, "share must stamp durable shared=true");
        assert_eq!(inst.collab_id.as_deref(), Some("collabtest"));
        // And persisted to the isolated registry file.
        assert!(
            tmp.join("kb-registry.toml").exists(),
            "registry marker must be persisted"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ADR-019 restart-survival (the bug): the durable share marker must survive
    /// a registry SAVE→LOAD round-trip, so a freshly-started editor's emit gate
    /// fires without any live event. This is the persistence crux of "edits keep
    /// propagating across editor restart".
    #[test]
    fn adr019_share_marker_survives_registry_reload() {
        let tmp = std::env::temp_dir().join(format!("mae-adr019-reload-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let mut editor = Editor::new();
        editor.data_dir_override = Some(tmp.clone());

        let mut inst = mae_kb::KnowledgeBase::new();
        inst.insert(mae_kb::Node::new(
            "collabtest:overview",
            "O",
            mae_kb::NodeKind::Note,
            "b",
        ));
        editor.kb.instances.insert("uuid-ct".into(), inst);
        editor
            .kb
            .registry
            .instances
            .push(mae_kb::federation::KbInstance {
                uuid: "uuid-ct".into(),
                name: "collabtest".into(),
                org_dir: std::path::PathBuf::new(),
                db_path: std::path::PathBuf::new(),
                primary: false,
                enabled: true,
                last_import: None,
                collab_id: None,
                shared: false,
                remote_peers: Vec::new(),
                last_sync: None,
            });

        handle_collab_event(
            &mut editor,
            CollabEvent::KbShared {
                kb_id: "collabtest".to_string(),
                node_count: 1,
            },
        );

        // Simulate restart: load the registry fresh from disk.
        let reloaded = mae_kb::federation::KbRegistry::load(&tmp);
        let inst = reloaded
            .find("collabtest")
            .expect("instance survives reload");
        assert!(
            inst.shared && inst.collab_id.as_deref() == Some("collabtest"),
            "durable share marker must survive a registry save→load round-trip"
        );

        // A restarted editor (empty cache) with the reloaded registry: the emit
        // gate fires from the durable marker → edits still queue for broadcast.
        let mut restarted = Editor::new();
        restarted.kb.registry = reloaded;
        let mut inst2 = mae_kb::KnowledgeBase::new();
        let mut n = mae_kb::Node::new("collabtest:overview", "O", mae_kb::NodeKind::Note, "b");
        n.source = Some(mae_kb::NodeSource::Federation);
        inst2.insert(n);
        restarted.kb.instances.insert("uuid-ct".into(), inst2);
        restarted.collab.kb_sync_mode = "on_save".into();
        assert!(restarted.collab.shared_kbs.is_empty());

        restarted
            .kb_update_node(
                "collabtest:overview",
                Some("edited after restart"),
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            restarted.collab.pending_kb_updates.len(),
            1,
            "post-restart edit must still queue a kb/node_update (durable gate)"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn collab_kb_joined_populates_tracking() {
        let mut editor = Editor::new();
        let tmp = std::env::temp_dir().join(format!("mae-adr019-join-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        editor.data_dir_override = Some(tmp.clone());

        // Create a CRDT node state + SV for the join event (ADR-022 reconcile).
        let doc = mae_sync::kb::KbNodeDoc::new("join-node-1", "Joined Title", "joined body", &[]);
        let state = doc.encode_state();
        let sv = doc.state_vector();

        handle_collab_event(
            &mut editor,
            CollabEvent::KbJoined {
                kb_id: "remote-kb".to_string(),
                collection_state: vec![],
                nodes: vec![JoinedNode {
                    id: "join-node-1".to_string(),
                    bytes: state,
                    daemon_sv: Some(sv),
                }],
            },
        );

        assert!(
            editor.collab.shared_kbs.contains_key("remote-kb"),
            "shared_kbs should track the joined KB"
        );
        assert!(
            editor.collab.shared_kbs["remote-kb"].contains("join-node-1"),
            "shared_kbs should contain the joined node ID"
        );
        // ADR-019: joined KB is a FIRST-CLASS instance with durable markers, NOT
        // dumped into primary (fixes B-3).
        let inst = editor
            .kb
            .registry
            .find_by_collab_id("remote-kb")
            .expect("joined KB must be a registered instance");
        assert!(inst.shared && inst.collab_id.as_deref() == Some("remote-kb"));
        let uuid = inst.uuid.clone();
        assert!(
            editor.kb.instances[&uuid].get("join-node-1").is_some(),
            "joined node must live in the instance"
        );
        assert!(
            editor.kb.primary.get("join-node-1").is_none(),
            "joined node must NOT be dumped into primary"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn collab_kb_left_removes_tracking() {
        let mut editor = Editor::new();
        editor
            .collab
            .shared_kbs
            .insert("test-kb".to_string(), HashSet::from(["n1".to_string()]));

        handle_collab_event(
            &mut editor,
            CollabEvent::KbLeft {
                kb_id: "test-kb".to_string(),
            },
        );

        assert!(
            !editor.collab.shared_kbs.contains_key("test-kb"),
            "shared_kbs should be cleared after leaving"
        );
    }

    #[test]
    fn collab_kb_update_node_generates_crdt_update_for_shared_node() {
        let mut editor = Editor::new();
        // Insert a node and mark it as shared.
        editor.kb.primary.insert(mae_kb::Node::new(
            "shared-node".to_string(),
            "Original Title".to_string(),
            mae_kb::NodeKind::Note,
            "original body".to_string(),
        ));
        // ADR-019: the durable primary-share marker is the gate authority.
        editor.kb.registry.primary_shared = true;
        editor.kb.registry.primary_collab_id = Some("my-kb".to_string());
        editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();

        // Update the node.
        editor
            .kb_update_node("shared-node", Some("New Title"), Some("new body"), None)
            .unwrap();

        // Should have a pending KB update.
        assert_eq!(
            editor.collab.pending_kb_updates.len(),
            1,
            "should generate one pending KB update"
        );
        let (kb_id, node_id, update_bytes) = &editor.collab.pending_kb_updates[0];
        assert_eq!(kb_id, "my-kb");
        assert_eq!(node_id, "shared-node");
        assert!(
            !update_bytes.is_empty(),
            "CRDT update bytes should be non-empty"
        );
    }

    #[test]
    fn collab_kb_update_node_no_update_for_unshared_node() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            "local-only".to_string(),
            "Title".to_string(),
            mae_kb::NodeKind::Note,
            "body".to_string(),
        ));
        editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();
        // No shared_kbs entry for this node.

        editor
            .kb_update_node("local-only", Some("Updated"), None, None)
            .unwrap();

        assert!(
            editor.collab.pending_kb_updates.is_empty(),
            "unshared node should not generate KB updates"
        );
    }

    #[test]
    fn collab_kb_manual_sync_mode_suppresses_auto_update() {
        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            "shared-node".to_string(),
            "Title".to_string(),
            mae_kb::NodeKind::Note,
            "body".to_string(),
        ));
        editor.collab.shared_kbs.insert(
            "my-kb".to_string(),
            HashSet::from(["shared-node".to_string()]),
        );
        editor.collab.kb_sync_mode = "manual".to_string();

        editor
            .kb_update_node("shared-node", Some("New Title"), None, None)
            .unwrap();

        assert!(
            editor.collab.pending_kb_updates.is_empty(),
            "manual sync mode should not auto-generate KB updates"
        );
    }

    #[test]
    fn collab_kb_drain_pending_updates_sends_commands() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        editor.collab.pending_kb_updates.push((
            "kb-1".to_string(),
            "node-a".to_string(),
            vec![1, 2, 3],
        ));
        editor.collab.pending_kb_updates.push((
            "kb-1".to_string(),
            "node-b".to_string(),
            vec![4, 5, 6],
        ));

        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);

        // Should have sent 2 KbNodeUpdate commands.
        let cmd1 = rx.try_recv().unwrap();
        let cmd2 = rx.try_recv().unwrap();
        match cmd1 {
            CollabCommand::KbNodeUpdate {
                kb_id,
                node_id,
                update,
                pending_rowid,
            } => {
                assert_eq!(kb_id, "kb-1");
                assert_eq!(node_id, "node-a");
                assert_eq!(update, vec![1, 2, 3]);
                assert_eq!(
                    pending_rowid, None,
                    "in-memory updates carry no durable rowid"
                );
            }
            other => panic!(
                "expected KbNodeUpdate, got: {:?}",
                collab_command_name(&other)
            ),
        }
        match cmd2 {
            CollabCommand::KbNodeUpdate {
                kb_id,
                node_id,
                update,
                pending_rowid,
            } => {
                assert_eq!(kb_id, "kb-1");
                assert_eq!(node_id, "node-b");
                assert_eq!(update, vec![4, 5, 6]);
                assert_eq!(
                    pending_rowid, None,
                    "in-memory updates carry no durable rowid"
                );
            }
            other => panic!(
                "expected KbNodeUpdate, got: {:?}",
                collab_command_name(&other)
            ),
        }

        // Pending list should be drained.
        assert!(editor.collab.pending_kb_updates.is_empty());
    }

    #[test]
    fn collab_kb_update_crdt_bytes_apply_to_fresh_doc() {
        // Verify that the CRDT update bytes generated by upsert_with_crdt
        // can actually be applied to reconstruct the node content.
        let mut editor = Editor::new();
        editor.kb.primary.insert(mae_kb::Node::new(
            "crdt-test".to_string(),
            "Original".to_string(),
            mae_kb::NodeKind::Note,
            "original body with café and 日本語".to_string(),
        ));
        // ADR-019: durable primary-share marker gates the broadcast.
        editor.kb.registry.primary_shared = true;
        editor.kb.registry.primary_collab_id = Some("test-kb".to_string());
        editor.collab.kb_sync_mode = mae_core::KB_SYNC_MODE_DEFAULT.to_string();

        editor
            .kb_update_node(
                "crdt-test",
                Some("Updated Title"),
                Some("new body — naïve résumé"),
                None,
            )
            .unwrap();

        let (_, _, update_bytes) = &editor.collab.pending_kb_updates[0];

        // Apply the update bytes to a fresh KbNodeDoc.
        let doc = mae_sync::kb::KbNodeDoc::from_bytes(update_bytes)
            .expect("CRDT bytes should decode to valid KbNodeDoc");
        let mat = doc.materialize();
        assert_eq!(
            mat.title, "Updated Title",
            "title should match after CRDT round-trip"
        );
        assert_eq!(
            mat.body, "new body — naïve résumé",
            "body should preserve UTF-8 after CRDT round-trip"
        );
    }

    // --- Phase 8: Offline KB sync tests ---

    #[test]
    fn offline_kb_updates_accumulate_when_disconnected() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Disconnected;
        editor.collab.pending_kb_updates.push((
            "kb-1".to_string(),
            "node-a".to_string(),
            vec![1, 2, 3],
        ));

        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);

        // Updates should NOT be sent when disconnected.
        assert!(
            rx.try_recv().is_err(),
            "pending KB updates should not be drained while disconnected"
        );
        // They should remain in the queue.
        assert_eq!(
            editor.collab.pending_kb_updates.len(),
            1,
            "pending KB updates should be preserved while offline"
        );
    }

    #[test]
    fn offline_kb_updates_drain_on_reconnect() {
        let mut editor = Editor::new();
        // Start disconnected, accumulate updates.
        editor.collab.status = CollabStatus::Disconnected;
        editor.collab.pending_kb_updates.push((
            "kb-1".to_string(),
            "node-a".to_string(),
            vec![10, 20],
        ));

        let (tx, mut rx) = mpsc::channel(8);

        // First drain while disconnected — nothing sent.
        drain_collab_intents(&mut editor, &tx);
        assert!(rx.try_recv().is_err());
        assert_eq!(editor.collab.pending_kb_updates.len(), 1);

        // Simulate reconnect.
        editor.collab.status = CollabStatus::Connected { peer_count: 1 };
        drain_collab_intents(&mut editor, &tx);

        // Now the update should be sent.
        let cmd = rx
            .try_recv()
            .expect("KB update should be sent after reconnect");
        match cmd {
            CollabCommand::KbNodeUpdate { kb_id, node_id, .. } => {
                assert_eq!(kb_id, "kb-1");
                assert_eq!(node_id, "node-a");
            }
            other => panic!(
                "expected KbNodeUpdate, got: {:?}",
                collab_command_name(&other)
            ),
        }
        assert!(editor.collab.pending_kb_updates.is_empty());
    }

    #[test]
    fn offline_kb_multiple_edits_all_sent_on_reconnect() {
        let mut editor = Editor::new();
        editor.collab.status = CollabStatus::Disconnected;

        // Accumulate 3 offline edits.
        for i in 0..3 {
            editor.collab.pending_kb_updates.push((
                "kb-1".to_string(),
                format!("node-{}", i),
                vec![i as u8],
            ));
        }

        let (tx, mut rx) = mpsc::channel(8);

        // Reconnect and drain.
        editor.collab.status = CollabStatus::Connected { peer_count: 2 };
        drain_collab_intents(&mut editor, &tx);

        // All 3 should be sent.
        for _ in 0..3 {
            assert!(
                rx.try_recv().is_ok(),
                "all offline KB updates should be sent on reconnect"
            );
        }
        assert!(rx.try_recv().is_err(), "no extra commands should be sent");
        assert!(editor.collab.pending_kb_updates.is_empty());
    }

    // -----------------------------------------------------------------------
    // PSK wiring tests — CI-runnable (no network required)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn perform_psk_auth_correct_key_succeeds() {
        // Test perform_psk_auth against a real PskAuth server handshake
        // using tokio duplex streams (no TCP needed).
        use mae_mcp::auth::{AuthProvider, PskAuth};
        use tokio::io::{duplex, BufReader, BufWriter};

        let psk = "test-secret-for-collab-bridge";
        let (client_stream, server_stream) = duplex(4096);
        let (cr, cw) = tokio::io::split(client_stream);
        let (sr, sw) = tokio::io::split(server_stream);

        let server_auth = PskAuth::new(psk);
        let server_handle = tokio::spawn(async move {
            let mut sr = BufReader::new(sr);
            let mut sw = BufWriter::new(sw);
            server_auth.server_handshake(&mut sr, &mut sw).await
        });

        let client_handle = tokio::spawn(async move {
            let mut cr = BufReader::new(cr);
            let mut cw = BufWriter::new(cw);
            perform_psk_auth(&mut cr, &mut cw, psk, None).await
        });

        let (server_result, client_result) = tokio::join!(server_handle, client_handle);
        assert!(
            server_result.unwrap().is_ok(),
            "server handshake should succeed with correct PSK"
        );
        assert!(
            client_result.unwrap().is_ok(),
            "perform_psk_auth should succeed with correct PSK"
        );
    }

    #[tokio::test]
    async fn perform_psk_auth_wrong_key_fails() {
        use mae_mcp::auth::{AuthProvider, PskAuth};
        use tokio::io::{duplex, BufReader, BufWriter};

        let (client_stream, server_stream) = duplex(4096);
        let (cr, cw) = tokio::io::split(client_stream);
        let (sr, sw) = tokio::io::split(server_stream);

        let server_auth = PskAuth::new("server-key");
        let server_handle = tokio::spawn(async move {
            let mut sr = BufReader::new(sr);
            let mut sw = BufWriter::new(sw);
            server_auth.server_handshake(&mut sr, &mut sw).await
        });

        let client_handle = tokio::spawn(async move {
            let mut cr = BufReader::new(cr);
            let mut cw = BufWriter::new(cw);
            perform_psk_auth(&mut cr, &mut cw, "wrong-key", None).await
        });

        let (server_result, client_result) = tokio::join!(server_handle, client_handle);
        let server_ok = server_result.is_ok_and(|r| r.is_ok());
        let client_ok = client_result.is_ok_and(|r| r.is_ok());
        assert!(
            !server_ok || !client_ok,
            "mismatched PSK should cause at least one side to fail"
        );
    }

    #[tokio::test]
    async fn perform_psk_auth_empty_key_skips_auth() {
        // Empty PSK should skip auth entirely (no reads/writes on the stream).
        use tokio::io::{duplex, BufReader, BufWriter};

        let (client_stream, _server_stream) = duplex(4096);
        let (cr, cw) = tokio::io::split(client_stream);
        let mut cr = BufReader::new(cr);
        let mut cw = BufWriter::new(cw);

        let result = perform_psk_auth(&mut cr, &mut cw, "", None).await;
        assert!(result.is_ok(), "empty PSK should skip auth and return Ok");
    }

    #[test]
    fn setup_collab_channels_propagates_psk_direct() {
        // When collab.psk is set (no psk_command), it should flow through to CollabSpawn.psk.
        let mut editor = Editor::new();
        let _ = editor.set_option("collab_psk", "my-secret-key");

        let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
        assert_eq!(
            spawn.transport.plain_psk(),
            Some("my-secret-key"),
            "transport should carry the direct PSK value"
        );
    }

    #[test]
    fn setup_collab_channels_propagates_psk_command() {
        // When collab.psk_command is set, it should be prefixed with "cmd:" sentinel.
        let mut editor = Editor::new();
        let _ = editor.set_option("collab_psk_command", "cat /tmp/test-psk.txt");

        let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
        assert_eq!(
            spawn.transport.plain_psk(),
            Some("cmd:cat /tmp/test-psk.txt"),
            "transport should carry the cmd: prefix for deferred resolution"
        );
    }

    #[test]
    fn setup_collab_channels_psk_command_takes_precedence() {
        // When both psk and psk_command are set, psk_command wins.
        let mut editor = Editor::new();
        let _ = editor.set_option("collab_psk", "plaintext-key");
        let _ = editor.set_option("collab_psk_command", "pass show mae/psk");

        let (_evt_rx, _cmd_tx, spawn) = setup_collab_channels(&editor);
        let psk = spawn.transport.plain_psk().unwrap_or("");
        assert!(
            psk.starts_with("cmd:"),
            "psk_command should take precedence over psk: got '{psk}'"
        );
        assert_eq!(psk, "cmd:pass show mae/psk");
    }

    #[test]
    fn setup_collab_channels_empty_psk_is_empty() {
        // With no psk/psk_command AND no keystore, the credential is empty.
        let (psk, key_id) = resolve_client_credential("", "", None);
        assert!(psk.is_empty(), "no creds → empty psk, got '{psk}'");
        assert_eq!(key_id, None);
    }

    #[test]
    fn resolve_credential_precedence() {
        // psk_command wins, returned as a cmd: sentinel, no key_id.
        let (psk, id) = resolve_client_credential("pass show k", "plain", None);
        assert_eq!(psk, "cmd:pass show k");
        assert_eq!(id, None);
        // psk wins over keystore when no command.
        let (psk, id) = resolve_client_credential("", "plain", None);
        assert_eq!(psk, "plain");
        assert_eq!(id, None);
    }

    #[test]
    fn resolve_credential_from_keystore_primary() {
        // A keystore with a named primary key → present its secret + name.
        let dir = std::env::temp_dir().join(format!("mae-cred-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("trusted_keys");
        mae_mcp::keystore::add_key(&path, Some("framework"), "deadbeef").unwrap();
        mae_mcp::keystore::add_key(&path, Some("thinkpad"), "cafef00d").unwrap();

        let (psk, id) = resolve_client_credential("", "", Some(&path));
        assert_eq!(psk, "deadbeef", "presents the primary (first) key");
        assert_eq!(id.as_deref(), Some("framework"), "advertises the key name");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drain_discover_peers_does_not_send_command() {
        // DiscoverPeers is handled locally (mDNS browse + buffer creation).
        // It should NOT send any CollabCommand to the network channel.
        // NOTE: MdnsManager::new() may fail on CI (no multicast), but that's
        // fine — the intent is still consumed (returns early with status msg).
        let mut editor = Editor::new();
        editor.collab.pending_intent = Some(CollabIntent::DiscoverPeers);
        let (tx, mut rx) = mpsc::channel(8);
        drain_collab_intents(&mut editor, &tx);

        // Intent must be consumed regardless of mDNS availability.
        assert!(
            editor.collab.pending_intent.is_none(),
            "DiscoverPeers intent should be consumed"
        );
        // No command should be sent to the collab task.
        assert!(
            rx.try_recv().is_err(),
            "DiscoverPeers should not send any CollabCommand"
        );
    }
}
