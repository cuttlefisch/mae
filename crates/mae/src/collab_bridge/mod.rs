//! Collab bridge — translates between editor-side intents and the TCP connection
//! to the state server, and handles incoming collab events.
//!
//! Follows the same pattern as `lsp_bridge.rs` and `dap_bridge.rs`:
//! - `drain_collab_intents()` called every tick
//! - `handle_collab_event()` handles events from the background task
//! - `run_collab_task()` is the background tokio task owning the TCP connection
//!
//! @ai-caution: [architecture-debt] Collab event/intent bridge. Went from
//! 6,546 to ~5,300 lines (2026-07) via the split described below; the
//! remaining debt is narrower now — see `run_collab_task`'s own
//! `@ai-caution` marker for why it's deliberately not split further. Its
//! test module was also split into `tests/` (per-feature files, all under
//! the 500-line ceiling). Tracked in `.claude/commands/mae-audit.md`'s
//! "Known exceptions" and `ROADMAP.md`'s "Architecture Debt" section.
//!
//! `handle_collab_event`'s per-variant bodies and `drain_collab_intents`'s
//! per-intent translation are split by topic into sibling files (pure code
//! motion — same pattern as `crates/core/src/editor/kb_ops/` and
//! `daemon/src/collab_handler/`): `events_connection.rs` (connection
//! lifecycle), `events_doc.rs` (buffer/doc sync), `events_kb.rs` (KB
//! sharing). This `mod.rs` keeps the top-level dispatchers plus
//! `run_collab_task`, `handle_response`, `handle_disconnected_cmd`, and
//! everything else not explicitly split out.

mod events_connection;
mod events_doc;
mod events_kb;

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

/// Capacity for the command channel (main thread -> collab background task) is
/// user-tunable via the `collab_command_queue_size` option (default 256); see
/// `spawn_collab_task`. Raise it for very high local edit throughput.
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
        /// ADR-037 fail-CLOSED gate (#168): whether this KB is E2e per its AUTHORITATIVE
        /// signed encryption mode (`derive_encryption`), stamped on the editor thread
        /// (the authority that holds the collection replica). When `true`, the network
        /// task MUST NOT emit plaintext — if it holds no content key (and can't reload
        /// one) or sealing fails, it refuses + requeues rather than leaking plaintext to
        /// the key-blind daemon.
        e2e: bool,
        pending_rowid: Option<i64>,
    },
    /// Add or remove a peer (by principal) from a KB's members (owner-only). For a
    /// REMOVE from an E2e KB the network task authors the §D3 rotation (signed Remove +
    /// re-key the remaining members) against its OWN `kb_collections` replica — the
    /// daemon's lineage — never a main-thread snapshot (which is stale for an owner who
    /// enabled on the network task, and whose independent reconstruction would tombstone
    /// the op-log; see the kb/approve #179 fail-closed rule). Shipped key-blind via
    /// `kb/collection_op`. Adds + legacy removes go through the daemon-authored RPC.
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
    /// Approve a pending join request as `role` (owner-only, ADR-018). On an E2e KB the
    /// network task wraps the content key to the approved member (ADR-037/038), authoring
    /// against its OWN `kb_collections` replica (the daemon's lineage) — NOT a main-thread
    /// snapshot, which can be a divergent lineage that corrupts the op-log on merge.
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
    /// Add/remove a principal on a KB's LOCAL self-protection blocklist (ADR-039 A2,
    /// #162). `block` = true → `kb/block_principal`, false → `kb/unblock_principal`.
    /// Local-only to the daemon; never propagated; not owner-gated.
    KbBlockPrincipal {
        kb_id: String,
        principal: String,
        block: bool,
    },
    /// Enable E2E encryption on an owned KB (ADR-037/038/039, owner-only). The network
    /// task generates + persists the content key, self-wraps it, authors the signed
    /// genesis + `SetEncryption` op against `collection_state` (carried from the main
    /// thread's replica), and ships the combined delta via `kb/collection_op`.
    KbSetEncryption {
        kb_id: String,
        mode: String,
        collection_state: Vec<u8>,
        /// #171: plaintext `(node_id, encode_state)` for every shared node, so the network
        /// task can RE-SEAL them under the new content key — sealed edits then graft onto
        /// the op-set + joiners can read sealed content (a node shared plaintext THEN
        /// encrypted otherwise keeps a plaintext daemon lineage that swallows sealed ops).
        node_states: Vec<(String, Vec<u8>)>,
    },
    /// Ship an owner-signed, opaque collection delta to the daemon's key-blind
    /// `kb/collection_op` RPC (ADR-038). The send + disconnect handling is ready here;
    /// the producer (wrap-on-admit) lands in PR B (#151 follow-up).
    #[allow(dead_code)]
    KbCollectionOp {
        kb_id: String,
        update: Vec<u8>,
    },
    /// ADR-040 PR2b: rotate this peer's identity key. The network task (which holds the old
    /// Ed25519 secret + the per-KB collection replicas + content keys) generates a new keypair,
    /// authors a `Rebind` (+ E2e content-key re-wrap) into every KB it OWNS, ships them via the
    /// owner-gated `kb/collection_op`, persists the new key, and swaps to it. Owner-only in v1
    /// (a non-owner member's self-rebind has no daemon path yet — PR2c/#213). The transport
    /// re-anchor is out-of-band (ADR-040 §4): the daemon must `authorize` the new pubkey, then
    /// the user reconnects with the new key.
    RotateIdentity,
    /// ADR-040 §Recovery-key — register an offline **recovery key**: the network task generates
    /// a fresh Ed25519 keypair, authors a `RegisterRecoveryKey` (signed by the current primary)
    /// into every KB it is a member of, ships them, and saves the recovery SECRET to a distinct
    /// on-disk path so the user can back it up OFFLINE. The recovery key can later authorize a
    /// `Rebind` if the primary is lost ([`CollabCommand::RecoverIdentity`]).
    RegisterRecoveryKey,
    /// ADR-040 §Recovery-key — recover a lost/compromised primary using the pre-registered
    /// offline recovery key at `recovery_path`. The recovery is run AS the new key (already
    /// authorized + connected, ADR-040 §4): the task loads the recovery secret and authors a
    /// recovery-signed `Rebind` (`old_fp` → the current connected identity) into every KB
    /// `old_fp` is a member of, so the new key inherits the lost key's seats.
    RecoverIdentity {
        recovery_path: String,
        old_fp: String,
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
    /// Daemon returned its LOCAL self-protection blocklist (ADR-039 A2, #162):
    /// `kb_id → blocked principals`. Cached for the `*KB Sharing*` Blocked view.
    BlocklistUpdated {
        blocklist: std::collections::HashMap<String, Vec<String>>,
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
    /// #206: `open_new_ops` found one or more op-set entries for this node it could
    /// NOT decrypt (wrong/rotated key, or a tampered/corrupt blob) — a non-fatal but
    /// user-visible signal, since this is indistinguishable from "not yet received"
    /// without it. Fail-closed on confidentiality is unaffected; this only surfaces
    /// the count.
    KbOpsUndecryptable {
        kb_id: String,
        node_id: String,
        count: usize,
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

    // Drain pending KB node updates (generated by kb_update_node for shared nodes)
    // and the collection-manifest ops queue. See events_kb::drain_kb_node_updates
    // for the ADR-020 queue→send→confirm→ack durability contract.
    events_kb::drain_kb_node_updates(editor, collab_tx);

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
        CollabIntent::StartServer
        | CollabIntent::Connect { .. }
        | CollabIntent::Disconnect
        | CollabIntent::RotateIdentity
        | CollabIntent::RegisterRecoveryKey
        | CollabIntent::RecoverIdentity { .. }
        | CollabIntent::ShowStatus => {
            events_connection::connection_intent_to_command(editor, intent)
        }
        CollabIntent::ShareBuffer { .. }
        | CollabIntent::ForceSync { .. }
        | CollabIntent::Doctor
        | CollabIntent::SaveCollab { .. }
        | CollabIntent::ListDocs
        | CollabIntent::ListDocsForJoin
        | CollabIntent::JoinDoc { .. }
        | CollabIntent::DiscoverPeers => match events_doc::doc_intent_to_command(editor, intent) {
            Some(c) => c,
            None => return,
        },
        CollabIntent::ShareKb { .. }
        | CollabIntent::JoinKb { .. }
        | CollabIntent::LeaveKb { .. }
        | CollabIntent::KbAddMember { .. }
        | CollabIntent::KbRemoveMember { .. }
        | CollabIntent::KbApprove { .. }
        | CollabIntent::KbListPending { .. }
        | CollabIntent::KbSetPolicy { .. }
        | CollabIntent::KbSetBlock { .. }
        | CollabIntent::KbSetEncryption { .. }
        | CollabIntent::KbNodeUpdate { .. }
        | CollabIntent::KbAdoptNode { .. } => {
            match events_kb::kb_intent_to_command(editor, intent) {
                Some(c) => c,
                None => return,
            }
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
        CollabCommand::RotateIdentity => "rotate-identity",
        CollabCommand::RegisterRecoveryKey => "register-recovery-key",
        CollabCommand::RecoverIdentity { .. } => "recover-identity",
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
        CollabCommand::KbBlockPrincipal { .. } => "kb-block-principal",
        CollabCommand::KbSetEncryption { .. } => "kb-set-encryption",
        CollabCommand::KbCollectionOp { .. } => "kb-collection-op",
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
        } => events_connection::handle_host_key_prompt_event(editor, addr, fingerprint, reply),
        CollabEvent::Connected {
            address,
            peer_count,
        } => events_connection::handle_connected_event(editor, address, peer_count),
        CollabEvent::Disconnected { reason } => {
            events_connection::handle_disconnected_event(editor, reason)
        }
        CollabEvent::RemoteUpdate {
            doc_id,
            update_bytes,
            wal_seq,
        } => events_doc::handle_remote_update_event(editor, doc_id, update_bytes, wal_seq),
        CollabEvent::GapDetected {
            doc_id,
            expected,
            got,
        } => events_doc::handle_gap_detected_event(editor, doc_id, expected, got),
        CollabEvent::StatusReport { lines } => {
            events_connection::handle_status_report_event(editor, lines)
        }
        CollabEvent::DoctorReport { lines } => {
            events_connection::handle_doctor_report_event(editor, lines)
        }
        CollabEvent::ServerStarted { pid } => {
            events_connection::handle_server_started_event(editor, pid)
        }
        CollabEvent::ServerFailed { error } => {
            events_connection::handle_server_failed_event(editor, error)
        }
        CollabEvent::Error { message } => events_connection::handle_error_event(editor, message),
        CollabEvent::BufferShared { doc_id } => {
            events_doc::handle_buffer_shared_event(editor, doc_id)
        }
        CollabEvent::BlocklistUpdated { blocklist } => {
            events_doc::handle_blocklist_updated_event(editor, blocklist)
        }
        CollabEvent::DocList {
            documents,
            for_join,
        } => events_doc::handle_doc_list_event(editor, documents, for_join),
        CollabEvent::BufferJoined {
            doc_id,
            state_bytes,
        } => events_doc::handle_buffer_joined_event(editor, doc_id, state_bytes),
        CollabEvent::ShareFailed { doc_id, message } => {
            events_doc::handle_share_failed_event(editor, doc_id, message)
        }
        CollabEvent::SaveIntentOk {
            doc_id,
            save_epoch,
            content_hash,
        } => events_doc::handle_save_intent_ok_event(editor, doc_id, save_epoch, content_hash),
        CollabEvent::SaveIntentConflict { doc_id, message } => {
            events_doc::handle_save_intent_conflict_event(editor, doc_id, message)
        }
        CollabEvent::SharerLeft { doc_id } => events_doc::handle_sharer_left_event(editor, doc_id),
        CollabEvent::PeerCountChanged { peer_count } => {
            events_connection::handle_peer_count_changed_event(editor, peer_count)
        }
        CollabEvent::PeerSaved { doc, saved_by } => {
            events_doc::handle_peer_saved_event(editor, doc, saved_by)
        }
        CollabEvent::AwarenessUpdate {
            client_id,
            doc_id,
            state,
        } => events_doc::handle_awareness_update_event(editor, client_id, doc_id, state),
        CollabEvent::KbShared {
            kb_id,
            node_count,
            collection_state,
        } => events_kb::handle_kb_shared_event(editor, kb_id, node_count, collection_state),
        CollabEvent::KbJoined {
            kb_id,
            collection_state,
            nodes,
        } => events_kb::handle_kb_joined_event(editor, kb_id, collection_state, nodes),
        CollabEvent::KbLeft { kb_id } => events_kb::handle_kb_left_event(editor, kb_id),
        CollabEvent::KbNodeUpdate {
            kb_id,
            node_id,
            update_bytes,
        } => events_kb::handle_kb_node_update_event(editor, kb_id, node_id, update_bytes),
        CollabEvent::KbOpsUndecryptable {
            kb_id,
            node_id,
            count,
        } => events_kb::handle_kb_ops_undecryptable_event(editor, kb_id, node_id, count),
        CollabEvent::KbUpdateRequeue {
            kb_id,
            node_id,
            update,
            pending_rowid,
        } => {
            events_kb::handle_kb_update_requeue_event(editor, kb_id, node_id, update, pending_rowid)
        }
        CollabEvent::KbUpdateAcked { rowid } => {
            events_kb::handle_kb_update_acked_event(editor, rowid)
        }
        CollabEvent::KbUpdateFailed {
            kb_id,
            node_id,
            rowid,
            message,
        } => events_kb::handle_kb_update_failed_event(editor, kb_id, node_id, rowid, message),
        CollabEvent::KbNodeAdopted {
            kb_id,
            node_id,
            state_bytes,
        } => events_kb::handle_kb_node_adopted_event(editor, kb_id, node_id, state_bytes),
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
    /// Minimum seconds between force-sync gathers for the same doc (debounce).
    force_sync_debounce_secs: u64,
    /// Milliseconds to wait after spawning a local daemon before connecting.
    daemon_start_grace_ms: u64,
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
    let cmd_channel_cap = (editor.collab.command_queue_size as usize).max(1);
    let (cmd_tx, cmd_rx) = mpsc::channel::<CollabCommand>(cmd_channel_cap);
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
    let force_sync_debounce_secs = editor.collab.force_sync_debounce_secs;
    let daemon_start_grace_ms = editor.collab.daemon_start_grace_ms;

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
        force_sync_debounce_secs,
        daemon_start_grace_ms,
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
                            timeout: std::time::Duration::from_secs(
                                editor.collab.host_key_prompt_timeout_secs,
                            ),
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
        spawn.force_sync_debounce_secs,
        spawn.daemon_start_grace_ms,
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
    /// Reply to `kb/blocklist` (ADR-039 A2, #162): the daemon's local blocklist.
    Blocklist,
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
/// Returns `None` to mean **refuse / fail-closed** (#168): an E2e KB for which we hold no
/// content key (and couldn't reload one) or for which sealing failed — the caller MUST NOT
/// fall back to plaintext; it requeues until a key arrives. `Some(..)` carries the request,
/// the advanced op-set state, and the sealed op-id (when sealed).
#[allow(clippy::too_many_arguments)]
fn build_kb_node_update_request(
    req_id: u64,
    kb_id: &str,
    node_id: &str,
    update: &[u8],
    epoch: u64,
    signing_identity: Option<&std::sync::Arc<mae_mcp::identity::Identity>>,
    content_key: Option<&mae_sync::content_crypto::ContentKey>,
    e2e: bool,
    op_set_state: &[u8],
) -> Option<(serde_json::Value, Vec<u8>, Option<String>)> {
    // ADR-037 E2E (#146): on an encrypted KB, the PLAINTEXT node update is sealed into
    // the op-set — encrypt under the content key, build the outer YMap-insert op
    // (stamped with the epoch client_id so the daemon's ADR-023 fence still authorizes
    // it) — and THAT outer op becomes the wire payload + what's signed (encrypt-then-
    // sign: the daemon verifies authorship over the ciphertext-bearing op, key-blind).
    // The returned op-set state advances by the sealed op. On an UNENCRYPTED KB the
    // plaintext update is the payload (legacy behaviour) and the state is unchanged.
    // `sealed_op_id` is `Some` only when we sealed — the caller records it in
    // `seen_ops` so the daemon's echo of our own op isn't re-materialized.
    //
    // #168 fail-CLOSED: when `e2e` is true we NEVER emit plaintext to the key-blind
    // daemon — a missing key or a seal failure returns `None` (refuse), not the cleartext.
    let (payload, new_op_set_state, sealed_op_id): (Vec<u8>, Vec<u8>, Option<String>) =
        match (content_key, signing_identity) {
            (Some(key), Some(id)) => {
                let client_id = mae_sync::kb::derive_kb_client_id(&id.fingerprint(), epoch);
                match mae_sync::op_set::seal_op(op_set_state, key, update, client_id) {
                    Ok((op_id, outer)) => {
                        let merged = mae_sync::op_set::merge(op_set_state, &outer)
                            .unwrap_or_else(|_| op_set_state.to_vec());
                        (outer, merged, Some(op_id))
                    }
                    // Sealing can't fail for a valid op-set; on an E2e KB refuse rather
                    // than leak plaintext, else (unencrypted, impossible here) keep it.
                    Err(_) if e2e => return None,
                    Err(_) => (update.to_vec(), op_set_state.to_vec(), None),
                }
            }
            // No content key: refuse on an E2e KB (fail closed); plaintext only when
            // the KB is genuinely unencrypted.
            _ if e2e => return None,
            _ => (update.to_vec(), op_set_state.to_vec(), None),
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
    Some((request, new_op_set_state, sealed_op_id))
}

/// ADR-037 fail-CLOSED gate (#168): is `collection_state` an E2e KB per its AUTHORITATIVE
/// signed op-log? Mirrors the F1 anchor pin + F2 signed-mode read of `derive_kb_content_key`
/// but needs no identity — the editor thread calls it to stamp
/// `CollabCommand::KbNodeUpdate.e2e`, so the seal path can refuse to emit plaintext on an
/// encrypted KB. Reads `derive_encryption` (the SIGNED, monotonic mode), never the
/// relay-flippable unsigned `coll.encryption()` flag, so a downgrade can't trick us into
/// shipping plaintext. `false` on a malformed/un-anchored/unencrypted collection.
fn kb_collection_is_e2e(collection_state: &[u8]) -> bool {
    let Ok(coll) = mae_sync::kb::KbCollectionDoc::from_bytes(collection_state) else {
        return false;
    };
    let ops = coll.oplog_ops();
    let Some(anchor) = ops
        .iter()
        .find(|o| {
            o.op.prev_hash.is_empty()
                && o.op.action == mae_sync::membership::MembershipAction::Admit
                && o.op.subject == o.op.author
        })
        .map(|o| o.author_pubkey)
    else {
        return false;
    };
    // F1: the genesis anchor must be the authenticated owner (refuse a forged genesis).
    if mae_sync::membership::fingerprint_of(&anchor) != coll.owner() {
        return false;
    }
    mae_sync::membership::derive_encryption(&ops, &anchor) == mae_sync::kb::Encryption::E2e
}

/// #170 fail-CLOSED: choose the node states to put on the wire for a `kb/share`. On an
/// UNENCRYPTED KB this is byte-identical to the legacy path (base64 the plaintext state).
/// On an **E2e** KB it NEVER ships the plaintext snapshot to the key-blind daemon: it sends
/// the already-sealed op-set state we hold for the node (idempotent — the daemon stores the
/// same op-set that `kb/node_update` produces), and SKIPS a node we have no op-set for (its
/// content is either pre-encryption — already on the daemon — or arrives via a sealed
/// node_update). Pure + unit-testable (principle #8).
fn select_share_node_states(
    kb_id: &str,
    e2e: bool,
    node_states: &[(String, Vec<u8>)],
    op_sets: &std::collections::HashMap<String, Vec<u8>>,
) -> Vec<(String, String)> {
    node_states
        .iter()
        .filter_map(|(id, state)| {
            if e2e {
                match op_sets.get(id) {
                    Some(sealed) if !sealed.is_empty() => {
                        Some((id.clone(), mae_sync::encoding::update_to_base64(sealed)))
                    }
                    _ => {
                        warn!(target: "kb_sync", kb = %kb_id, node = %id, "E2e share: no sealed op-set for node — SKIPPING (refusing to ship plaintext snapshot, #170)");
                        None
                    }
                }
            } else {
                Some((id.clone(), mae_sync::encoding::update_to_base64(state)))
            }
        })
        .collect()
}

/// ADR-037 §D2 (#146 Phase 2b): recover **this peer's** per-KB content key from a
/// collection doc, in the network task (the only place that holds both the collection
/// bytes — at the join/share response — and the identity secret). Returns `None` for
/// an unencrypted KB, a collection without a trusted genesis, or a wrap that doesn't
/// open for me (non-member / not yet wrapped to me). Pure derivation, no key server:
/// the genesis owner self-admit is the trust anchor, and the latest owner-authored
/// `wrapped_key` targeting me (causal order) is unwrapped with my Ed25519 secret.
fn derive_kb_content_key(
    collection_state: &[u8],
    identity: &mae_mcp::identity::Identity,
) -> Option<mae_sync::content_crypto::ContentKey> {
    let me = identity.fingerprint();
    let coll = mae_sync::kb::KbCollectionDoc::from_bytes(collection_state).ok()?;
    let ops = coll.oplog_ops();
    // Anchor: the genesis owner self-admit (mirror of `derive_governance` /
    // `derive_content_key`). Without it there is no trust root and we derive nothing.
    let anchor_owner_pubkey = ops
        .iter()
        .find(|o| {
            o.op.prev_hash.is_empty()
                && o.op.action == mae_sync::membership::MembershipAction::Admit
                && o.op.subject == o.op.author
        })
        .map(|o| o.author_pubkey)?;
    // F1/A3 (ADR-039): the genesis anchor MUST be the AUTHENTICATED owner — `COLL_OWNER_KEY`,
    // which the daemon binds to the verified mTLS principal (ADR-018). This refuses a forged
    // genesis a relay substituted (its key wouldn't match the daemon-attested owner) instead
    // of TOFU-trusting whatever genesis the collection carries. (Mesh node-id pinning = #158.)
    if mae_sync::membership::fingerprint_of(&anchor_owner_pubkey) != coll.owner() {
        return None;
    }
    // F2 (ADR-039): the AUTHORITATIVE encryption mode is the SIGNED, monotonic op-log — NOT
    // the unsigned collection flag a relay could flip to `none`. Once it asserts E2e the key
    // is derived; the seal path (gated on the resulting `content_keys` entry) stays
    // fail-closed — it never reverts to plaintext on a flag downgrade.
    if mae_sync::membership::derive_encryption(&ops, &anchor_owner_pubkey)
        != mae_sync::kb::Encryption::E2e
    {
        return None;
    }
    mae_sync::membership::derive_content_key(
        &ops,
        &anchor_owner_pubkey,
        &me,
        &identity.secret_bytes(),
    )
}

/// ADR-037 §2b (#146): route one inbound `kb:{node}` sync_update to the main thread as
/// plaintext. The single seam shared by both inbound formats (`notifications/sync_update`
/// + legacy `sync/update`) so the encrypted + plaintext paths can't drift (principle #8).
///
/// - **No content key** for this node's KB (unencrypted, or a node not yet registered in
///   `node_to_kb`) ⇒ **plaintext passthrough**: emit the bytes verbatim, exactly as the
///   pre-encryption code did (byte-identical behaviour).
/// - **Encrypted:** merge the inbound op-set update (the daemon relayed opaque ciphertext
///   blobs) into our op-set mirror, `open_new_ops` the ops we haven't materialized yet
///   (causal order; blobs that don't open are skipped), and emit each inner **plaintext**
///   update. `seen_ops` makes this idempotent and suppresses the echo of our own ops.
fn route_kb_node_update(
    node_id: &str,
    bytes: Vec<u8>,
    content_keys: &std::collections::HashMap<String, mae_sync::content_crypto::ContentKey>,
    node_to_kb: &std::collections::HashMap<String, String>,
    op_sets: &mut std::collections::HashMap<String, Vec<u8>>,
    seen_ops: &mut std::collections::HashMap<String, std::collections::BTreeSet<String>>,
    evt_tx: &mpsc::Sender<CollabEvent>,
) {
    let kb_id = node_to_kb.get(node_id).cloned().unwrap_or_default();
    let key = node_to_kb.get(node_id).and_then(|kb| content_keys.get(kb));
    let Some(key) = key else {
        // Plaintext passthrough — unencrypted KB or not-yet-keyed node.
        try_send_evt(
            evt_tx,
            CollabEvent::KbNodeUpdate {
                kb_id,
                node_id: node_id.to_string(),
                update_bytes: bytes,
            },
        );
        return;
    };
    let merged = mae_sync::op_set::merge(
        op_sets.get(node_id).map(|v| v.as_slice()).unwrap_or(&[]),
        &bytes,
    )
    .unwrap_or_else(|_| bytes.clone());
    let seen = seen_ops.entry(node_id.to_string()).or_default();
    let opened = mae_sync::op_set::open_new_ops(&merged, key, seen);
    for (op_id, plaintext) in opened.ops {
        seen.insert(op_id);
        try_send_evt(
            evt_tx,
            CollabEvent::KbNodeUpdate {
                kb_id: kb_id.clone(),
                node_id: node_id.to_string(),
                update_bytes: plaintext,
            },
        );
    }
    // #206: a wrong/rotated key (or tamper) must be distinguishable from "not yet
    // received" — surface the count instead of letting it vanish silently.
    if opened.undecryptable > 0 {
        try_send_evt(
            evt_tx,
            CollabEvent::KbOpsUndecryptable {
                kb_id,
                node_id: node_id.to_string(),
                count: opened.undecryptable,
            },
        );
    }
    op_sets.insert(node_id.to_string(), merged);
}

/// Background task that owns the TCP connection to the state server.
///
/// Receives commands from the main thread, manages the connection lifecycle,
/// and forwards events back.
// @ai-caution: [architecture-debt] run_collab_task's ~19 locals (writer,
// pending_responses, content_keys, kb_collections, signing_identity, seq_tracker,
// op_sets, node_to_kb, seen_ops, pending_collection_ops, shared_docs,
// target_address, reconnect_enabled/reconnect_attempt, last_force_sync,
// messages_received, heartbeat_interval, ping_pending, msg_rx, ...) are read/written
// across nearly every one of its 29 CollabCommand match arms with no existing state
// struct — a mechanical split-by-arm would relocate the entanglement, not resolve
// it. A future dedicated pass could bundle these locals into a CollabTaskState
// struct first, converting arms to methods, then split by command prefix. See
// ROADMAP.md's "Architecture Debt" section and .claude/commands/mae-audit.md's
// "Known exceptions" list.
#[allow(clippy::too_many_arguments)]
async fn run_collab_task(
    mut cmd_rx: mpsc::Receiver<CollabCommand>,
    evt_tx: mpsc::Sender<CollabEvent>,
    reconnect_secs: u64,
    write_timeout: std::time::Duration,
    backoff_factor: u64,
    max_reconnect_attempts: u64,
    heartbeat_secs: u64,
    force_sync_debounce_secs: u64,
    daemon_start_grace_ms: u64,
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
    // `mut`: ADR-040 PR2b swaps this to the successor key on `(rotate-identity)`.
    let mut signing_identity: Option<std::sync::Arc<mae_mcp::identity::Identity>> =
        match &transport {
            ClientTransport::KeyTls { identity, .. }
            | ClientTransport::KeyJson { identity, .. } => Some(identity.clone()),
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
    // ADR-037 E2E (#146 Phase 2b): all content-key crypto lives here in the network
    // task — the only place that holds both the collection bytes (join/share response)
    // and the identity secret. The main thread only ever sees plaintext.
    //   content_keys: kb_id  → per-KB content key (derived on join/share)
    //   op_sets:      node_id → full op-set yrs state (the ciphertext YMap mirror)
    //   node_to_kb:   node_id → kb_id (selects the content key on receive)
    //   seen_ops:     node_id → op_ids already materialized (avoids re-applying echoes)
    // All transient: rebuilt from the collection + daemon op-set on (re)join.
    let mut content_keys: HashMap<String, mae_sync::content_crypto::ContentKey> = HashMap::new();
    let mut op_sets: HashMap<String, Vec<u8>> = HashMap::new();
    let mut node_to_kb: HashMap<String, String> = HashMap::new();
    let mut seen_ops: HashMap<String, std::collections::BTreeSet<String>> = HashMap::new();
    // #173: kb_id → full collection replica, seeded at join/share/enable and advanced by
    // inbound `kbc:` deltas, so the content key is RE-DERIVED on any membership change
    // (rotation / post-join wrap) instead of being frozen at first derive.
    // ADR-040 §Recovery (B2): re-seed from the durable on-disk store so a restarted (or
    // restored-from-backup) member keeps its key-blind op-logs without a re-fetch — the
    // precondition for self-service recovery, since the daemon serves the op-log only to
    // members and a recovering peer is (yet) a non-member.
    let mut kb_collections: HashMap<String, Vec<u8>> = HashMap::new();
    if let Some(d) = mae_mcp::collection_store::collections_dir() {
        for (kb_id, bytes) in mae_mcp::collection_store::load_all(&d) {
            kb_collections.insert(kb_id, bytes);
        }
    }
    // ADR-040 PR2c: owner-side reactive re-wrap outbox — filled by the receive path when a
    // member rotation lands on an E2e KB this peer owns, drained right after each inbound
    // message is handled (ships `kb/collection_op` + advances the replica).
    let mut pending_collection_ops: Vec<(String, Vec<u8>)> = Vec::new();
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
                        CollabCommand::RotateIdentity => {
                            // ADR-040 PR2b: owner identity rotation. Generate a successor keypair,
                            // author a Rebind (+ E2e re-wrap) into every KB this peer OWNS, ship
                            // them over the CURRENT connection (the old owner key still passes the
                            // daemon's owner-Manage gate at this instant), persist + swap to the new
                            // key. The transport stays authenticated as the now-retired old key, so
                            // the user must authorize the new key on the daemon and reconnect
                            // (out-of-band, ADR-040 §4). Owner-only in v1 — non-owned KBs are left
                            // for the PR2c member path (#213).
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            match (signing_identity.clone(), writer.as_mut()) {
                                (Some(old_id), Some(w)) => {
                                    let new_id = std::sync::Arc::new(
                                        mae_mcp::identity::Identity::generate(old_id.label()),
                                    );
                                    // Owner-owned KBs (Rebind + self re-wrap) AND non-owned KBs
                                    // where I am a member (Rebind only — the owner re-wraps the
                                    // content key reactively, ADR-040 PR2c).
                                    let owner_plans = plan_owner_rotation(
                                        &kb_collections, &content_keys, &old_id, &new_id, now,
                                    );
                                    let member_plans =
                                        plan_member_rotation(&kb_collections, &old_id, &new_id, now);
                                    let owned_count = owner_plans.len();
                                    let member_count = member_plans.len();
                                    let plans: Vec<RotationPlan> =
                                        owner_plans.into_iter().chain(member_plans).collect();
                                    let total_plans = plans.len();
                                    let mut rotated = 0usize;
                                    // #round-4: a wire-write failure used to be silently discarded
                                    // (`let _ = write_framed(...)`) while local state advanced
                                    // unconditionally — a dropped delta left the daemon's roster on
                                    // the OLD key while `signing_identity` swapped to the new one
                                    // below, so the very next signed op would be rejected by the
                                    // daemon: a self-inflicted lockout reported as "rotation
                                    // shipped". Only advance local state for a KB whose delta(s)
                                    // actually reached the daemon; track the rest to report honestly.
                                    let mut failed_kb_ids: Vec<String> = Vec::new();
                                    for plan in &plans {
                                        let mut plan_shipped = true;
                                        for delta in &plan.deltas {
                                            let req_id = next_request_id;
                                            next_request_id += 1;
                                            let req = mae_sync::wire::kb_collection_op_request(
                                                req_id,
                                                &plan.kb_id,
                                                &mae_sync::encoding::update_to_base64(delta),
                                            );
                                            let sent = match serde_json::to_vec(&req) {
                                                Ok(body) => {
                                                    write_framed(w, &body, write_timeout).await.is_ok()
                                                }
                                                Err(_) => false,
                                            };
                                            if !sent {
                                                plan_shipped = false;
                                                break;
                                            }
                                        }
                                        if plan_shipped {
                                            // Advance the local replica so later ops chain on the
                                            // rotated state (and the new key derives its own content key).
                                            kb_collections
                                                .insert(plan.kb_id.clone(), plan.new_replica.clone());
                                            // B2: persist so the rotated/recovered op-log survives a restart.
                                            persist_collection(&plan.kb_id, &plan.new_replica);
                                            rotated += 1;
                                        } else {
                                            failed_kb_ids.push(plan.kb_id.clone());
                                        }
                                    }
                                    if rotated == 0 && total_plans > 0 {
                                        warn!(owned_count, member_count, "rotate-identity: failed to ship ANY rotation delta — identity NOT rotated");
                                        try_send_evt(&evt_tx, CollabEvent::Error {
                                            message: format!(
                                                "Identity rotation failed — could not reach the daemon for any of {total_plans} KB(s). Your identity was NOT changed; try again once reconnected."
                                            ),
                                        });
                                    } else {
                                        // Persist the successor key (the old key already signed the
                                        // Rebinds, so it is safe to replace on disk now).
                                        if let Some(dir) = mae_mcp::identity::default_collab_dir() {
                                            if let Err(e) = new_id.save(&dir) {
                                                warn!(error = %e, "rotate-identity: failed to persist the new key");
                                            }
                                        }
                                        let new_fp = new_id.fingerprint();
                                        // Swap the live signing identity — future content ops sign with
                                        // the new key (which the rotated membership recognises as owner).
                                        signing_identity = Some(new_id);
                                        info!(rotated, owned_count, member_count, failed = failed_kb_ids.len(), new_fp = %new_fp, "rotate-identity: rotation shipped");
                                        let mut lines = vec![
                                            format!("Identity rotated across {rotated} KB(s) ({owned_count} owned, {member_count} as member)."),
                                            format!("New fingerprint: {new_fp}"),
                                            "Next: authorize the new key on the daemon (`mae-daemon authorize`), then reconnect — this connection still uses the old key.".to_string(),
                                            // The new key has a new ADR-023 client-id, so the FIRST edit to an
                                            // existing node after rotation trips the epoch fence once and must
                                            // rebase; `collab-fence-resolution = auto` re-authors it silently.
                                            "Your first edit to an existing node may rebase once (the rotated key has a new write lineage) — set `collab-fence-resolution` to `auto` to handle it silently.".to_string(),
                                        ];
                                        if !failed_kb_ids.is_empty() {
                                            lines.push(format!(
                                                "WARNING: rotation FAILED to ship for {} KB(s) — still on the OLD key there, re-run rotate-identity once reconnected: {}",
                                                failed_kb_ids.len(),
                                                failed_kb_ids.join(", ")
                                            ));
                                        }
                                        try_send_evt(&evt_tx, CollabEvent::StatusReport { lines });
                                    }
                                }
                                (None, _) => {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "rotate-identity requires `key` auth mode (no signing identity in psk/none mode)".to_string(),
                                    });
                                }
                                (_, None) => {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "rotate-identity: no active connection writer".to_string(),
                                    });
                                }
                            }
                        }
                        CollabCommand::RegisterRecoveryKey => {
                            // ADR-040 §Recovery-key: generate a fresh offline recovery keypair,
                            // author a `RegisterRecoveryKey` (signed by the current primary) into
                            // every KB I am a member of, ship them (owned KBs ride the owner gate,
                            // member KBs the PR3 self-service gate), and save the recovery SECRET
                            // to a DISTINCT path for the user to back up offline. The recovery key
                            // never reads on its own — it only authorizes a future `Rebind`.
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            match (signing_identity.clone(), writer.as_mut()) {
                                (Some(id), Some(w)) => {
                                    let recovery = mae_mcp::identity::Identity::generate("recovery");
                                    let recovery_pubkey = recovery.public().to_bytes();
                                    let plans = plan_register_recovery_key(
                                        &kb_collections, &id, &recovery_pubkey, now,
                                    );
                                    let mut registered = 0usize;
                                    // #round-4: same fix shape as RotateIdentity above — a wire
                                    // write can silently fail; only advance local state for a KB
                                    // whose delta actually reached the daemon.
                                    let mut failed_kb_ids: Vec<String> = Vec::new();
                                    for plan in &plans {
                                        let mut plan_shipped = true;
                                        for delta in &plan.deltas {
                                            let req_id = next_request_id;
                                            next_request_id += 1;
                                            let req = mae_sync::wire::kb_collection_op_request(
                                                req_id,
                                                &plan.kb_id,
                                                &mae_sync::encoding::update_to_base64(delta),
                                            );
                                            let sent = match serde_json::to_vec(&req) {
                                                Ok(body) => {
                                                    write_framed(w, &body, write_timeout).await.is_ok()
                                                }
                                                Err(_) => false,
                                            };
                                            if !sent {
                                                plan_shipped = false;
                                                break;
                                            }
                                        }
                                        if plan_shipped {
                                            kb_collections
                                                .insert(plan.kb_id.clone(), plan.new_replica.clone());
                                            // B2: persist so the rotated/recovered op-log survives a restart.
                                            persist_collection(&plan.kb_id, &plan.new_replica);
                                            registered += 1;
                                        } else {
                                            failed_kb_ids.push(plan.kb_id.clone());
                                        }
                                    }
                                    // Save the recovery secret to `<collab_dir>/recovery` (a
                                    // SEPARATE path from the primary `id_ed25519`, so it is not
                                    // clobbered) for the user to move offline.
                                    let rec_fp = recovery.fingerprint();
                                    let saved_path = match mae_mcp::identity::default_collab_dir() {
                                        Some(dir) => {
                                            let rec_dir = dir.join("recovery");
                                            match recovery.save(&rec_dir) {
                                                Ok(()) => Some(rec_dir.join("id_ed25519")),
                                                Err(e) => {
                                                    warn!(error = %e, "register-recovery-key: failed to save the recovery key");
                                                    None
                                                }
                                            }
                                        }
                                        None => None,
                                    };
                                    info!(registered, failed = failed_kb_ids.len(), rec_fp = %rec_fp, "register-recovery-key: recovery key registered");
                                    let mut lines = vec![
                                        format!("Recovery key registered across {registered} KB(s)."),
                                        format!("Recovery fingerprint: {rec_fp}"),
                                    ];
                                    match &saved_path {
                                        Some(p) => {
                                            lines.push(format!("Saved to: {}", p.display()));
                                            lines.push("BACK THIS UP OFFLINE and remove it from this machine — anyone holding it can rotate your identity. To recover later: `:collab-recover-identity <path> <old-fingerprint>`.".to_string());
                                        }
                                        None => lines.push("WARNING: the recovery key was registered but could NOT be saved to disk — it is lost. Re-run after fixing the collab directory.".to_string()),
                                    }
                                    if !failed_kb_ids.is_empty() {
                                        lines.push(format!(
                                            "WARNING: registration FAILED to ship for {} KB(s) — re-run register-recovery-key once reconnected: {}",
                                            failed_kb_ids.len(),
                                            failed_kb_ids.join(", ")
                                        ));
                                    }
                                    try_send_evt(&evt_tx, CollabEvent::StatusReport { lines });
                                }
                                (None, _) => {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "register-recovery-key requires `key` auth mode (no signing identity in psk/none mode)".to_string(),
                                    });
                                }
                                (_, None) => {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "register-recovery-key: no active connection writer".to_string(),
                                    });
                                }
                            }
                        }
                        CollabCommand::RecoverIdentity { recovery_path, old_fp } => {
                            // ADR-040 §Recovery-key: recover a lost primary. Run AS the new key —
                            // the user generated a fresh primary, authorized it on the daemon
                            // out-of-band (§4), and connected with it, so `signing_identity` is the
                            // successor. Load the offline recovery secret and author a
                            // recovery-signed `Rebind` (old_fp → me) into every KB old_fp belonged
                            // to; the daemon's PR3 gate accepts it (subject == this connection's
                            // principal), and the new key inherits the lost key's seats.
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0);
                            let recovery = mae_mcp::identity::Identity::load_secret(
                                std::path::Path::new(&recovery_path),
                                "recovery",
                            );
                            // B2: the recovering user just restored their data dir on a new
                            // machine — re-seed kb_collections from the persisted op-logs so a KB
                            // restored AFTER this session started is recoverable. Never clobber a
                            // live (more-current) replica.
                            if let Some(d) = mae_mcp::collection_store::collections_dir() {
                                for (kb_id, bytes) in mae_mcp::collection_store::load_all(&d) {
                                    kb_collections.entry(kb_id).or_insert(bytes);
                                }
                            }
                            match (signing_identity.clone(), writer.as_mut(), recovery) {
                                (Some(new_id), Some(w), Some(recovery)) => {
                                    let plans = plan_recovery_rotation(
                                        &kb_collections, &old_fp, &new_id, &recovery, now,
                                    );
                                    let mut recovered = 0usize;
                                    for plan in &plans {
                                        for delta in &plan.deltas {
                                            let req_id = next_request_id;
                                            next_request_id += 1;
                                            let req = mae_sync::wire::kb_collection_op_request(
                                                req_id,
                                                &plan.kb_id,
                                                &mae_sync::encoding::update_to_base64(delta),
                                            );
                                            if let Ok(body) = serde_json::to_vec(&req) {
                                                let _ = write_framed(w, &body, write_timeout).await;
                                            }
                                        }
                                        kb_collections
                                            .insert(plan.kb_id.clone(), plan.new_replica.clone());
                                        // B2: persist so the rotated/recovered op-log survives a restart.
                                        persist_collection(&plan.kb_id, &plan.new_replica);
                                        recovered += 1;
                                    }
                                    let new_fp = new_id.fingerprint();
                                    info!(recovered, old_fp = %old_fp, new_fp = %new_fp, "recover-identity: recovery shipped");
                                    if recovered == 0 {
                                        try_send_evt(&evt_tx, CollabEvent::StatusReport { lines: vec![
                                            format!("No KBs found where {old_fp} is a member — nothing to recover."),
                                            "Ensure this peer holds the shared KB's collection state (it must know the KB locally to author the recovery).".to_string(),
                                        ]});
                                    } else {
                                        try_send_evt(&evt_tx, CollabEvent::StatusReport { lines: vec![
                                            format!("Recovered {recovered} KB(s): {old_fp} → {new_fp}."),
                                            "Your new key now holds the lost key's seats. The owner re-wraps any E2e content keys to you reactively; your first edit may rebase once (set `collab-fence-resolution` to `auto`).".to_string(),
                                        ]});
                                    }
                                }
                                (None, _, _) => {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "recover-identity requires `key` auth mode (connect with your NEW key first)".to_string(),
                                    });
                                }
                                (_, None, _) => {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: "recover-identity: no active connection writer".to_string(),
                                    });
                                }
                                (_, _, None) => {
                                    try_send_evt(&evt_tx, CollabEvent::Error {
                                        message: format!("recover-identity: could not load a recovery key from '{recovery_path}' (expected an id_ed25519 file in that directory)"),
                                    });
                                }
                            }
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
                            // Debounce: skip if we sent ForceSync for this doc within
                            // the configured window (collab_force_sync_debounce_secs).
                            let now = std::time::Instant::now();
                            if let Some(last) = last_force_sync.get(&doc_id) {
                                if now.duration_since(*last).as_secs() < force_sync_debounce_secs {
                                    debug!(doc = %doc_id, window_secs = force_sync_debounce_secs,
                                        "ForceSync debounced");
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
                            // #170 fail-CLOSED: on an E2e KB, NEVER ship plaintext node
                            // snapshots to the key-blind daemon (the share/re-share path).
                            // Send the already-sealed op-set state we hold (idempotent — the
                            // daemon stores the same op-set node_updates produce); SKIP a node
                            // we have no op-set for rather than leak its plaintext (its content
                            // is either pre-encryption — already on the daemon — or arrives via
                            // a sealed node_update). The unencrypted path stays byte-identical.
                            let e2e = kb_collection_is_e2e(&collection_state);
                            info!(kb = %kb_id, nodes = node_states.len(), e2e, "sharing KB");
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let nodes =
                                    select_share_node_states(&kb_id, e2e, &node_states, &op_sets);
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
                                let mut req = mae_sync::wire::kb_join_request(req_id, &kb_id, &svs_b64);
                                // ADR-041 (#158 I1): publish our X25519 wrap key so the owner can
                                // wrap the content key to it on approval (the daemon can't derive
                                // it). Only in key mode (no identity ⇒ no E2e anyway).
                                if let Some(idy) = signing_identity.as_ref() {
                                    let wrap_pub = mae_sync::content_crypto::wrap_public_for(&idy.secret_bytes());
                                    if let Some(p) = req.get_mut("params").and_then(|p| p.as_object_mut()) {
                                        p.insert("wrap_pubkey".to_string(), serde_json::Value::String(hex::encode(wrap_pub)));
                                    }
                                }
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
                            // ADR-037 §2b: drop the content key + per-node op-set / seen /
                            // routing state for this KB (leaving discards the synced replica;
                            // a later re-join rebuilds from the collection + daemon op-set).
                            content_keys.remove(&kb_id);
                            let gone: Vec<String> = node_to_kb
                                .iter()
                                .filter(|(_, k)| *k == &kb_id)
                                .map(|(n, _)| n.clone())
                                .collect();
                            for n in &gone {
                                node_to_kb.remove(n);
                                op_sets.remove(n);
                                seen_ops.remove(n);
                            }
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
                        CollabCommand::KbNodeUpdate { kb_id, node_id, update, epoch, e2e, pending_rowid } => {
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
                                // ADR-037 §2b: on an encrypted KB the plaintext update is
                                // sealed into the op-set under the content key (None ⇒
                                // plaintext path). Register node→kb so the receive side
                                // can pick this key (covers owner + member, every node).
                                node_to_kb
                                    .entry(node_id.clone())
                                    .or_insert_with(|| kb_id.clone());
                                // #168 fail-CLOSED + restart liveness: on an E2e KB with no
                                // in-memory key, lazily reload the persisted one from the
                                // key store (the owner's key survives a restart this way)
                                // BEFORE the seal decision — so a post-restart edit seals
                                // instead of being refused.
                                if e2e && !content_keys.contains_key(&kb_id) {
                                    if let Some(b) = mae_mcp::content_key_store::content_keys_dir()
                                        .as_ref()
                                        .and_then(|d| mae_mcp::content_key_store::load(d, &kb_id))
                                    {
                                        content_keys.insert(
                                            kb_id.clone(),
                                            mae_sync::content_crypto::ContentKey::from_bytes(b),
                                        );
                                        info!(target: "kb_sync", kb_id = %kb_id, "bg: reloaded persisted content key for E2e KB (post-restart)");
                                    }
                                }
                                let op_set_state = op_sets.get(&node_id).cloned().unwrap_or_default();
                                let built = build_kb_node_update_request(
                                    req_id,
                                    &kb_id,
                                    &node_id,
                                    &update,
                                    epoch,
                                    signing_identity.as_ref(),
                                    content_keys.get(&kb_id),
                                    e2e,
                                    &op_set_state,
                                );
                                let Some((req, new_op_set, sealed_op_id)) = built else {
                                    // #168 FAIL-CLOSED: E2e KB, no content key (and none on
                                    // disk) or seal failed — REFUSE to emit plaintext to the
                                    // key-blind daemon. `delivered` stays false → the update
                                    // is requeued and retried once the key arrives (owner: on
                                    // reload; member: on approve). Loud + observable.
                                    warn!(target: "kb_sync", kb_id = %kb_id, node_id = %node_id, "bg: E2e KB has no content key — REFUSING to send plaintext (fail-closed, #168); requeued");
                                    try_send_evt(&evt_tx, CollabEvent::KbUpdateRequeue { kb_id, node_id, update, pending_rowid });
                                    continue;
                                };
                                match serde_json::to_vec(&req) {
                                    Ok(body) => match write_framed(w, &body, write_timeout).await {
                                        Ok(()) => {
                                            delivered = true;
                                            // Commit the advanced op-set state ONLY now
                                            // that the op is on the wire — a failed write
                                            // requeues + rebuilds from the unchanged state
                                            // (no double-seal). `seen_ops` suppresses the
                                            // daemon's echo of our own op.
                                            if let Some(op_id) = sealed_op_id {
                                                op_sets.insert(node_id.clone(), new_op_set);
                                                seen_ops
                                                    .entry(node_id.clone())
                                                    .or_default()
                                                    .insert(op_id);
                                            }
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
                            // ADR-037 §D3: removing a member from an E2e KB ROTATES the content
                            // key — the owner authors a signed Remove + a fresh-key re-wrap to
                            // each remaining member, shipped key-blind via kb/collection_op. The
                            // removed member receives no new wrap → it keeps only the old key and
                            // can't read post-rotation content. Adds + legacy removes fall through.
                            let mut shipped_e2e = false;
                            // #round-4: distinct from `shipped_e2e == false` (E2E not applicable —
                            // falls through to the generic add/remove below). If we DID enter the
                            // E2E revoke+rekey path but the wire write failed, falling through to
                            // the generic (non-rekeying) remove would silently DOWNGRADE the
                            // security semantics — the removed member would be dropped from the
                            // roster without their key access ever being revoked. Track it
                            // separately so that case gets a loud, honest error instead.
                            let mut e2e_write_failed = false;
                            if !add {
                                if let (Some(_old_key), Some(id)) =
                                    (content_keys.get(&kb_id).cloned(), signing_identity.as_ref())
                                {
                                    // Fail-CLOSED on the network task's OWN replica
                                    // (`kb_collections` — the daemon's lineage, CRDT-merged from
                                    // the ops WE authored + every broadcast). NEVER author against
                                    // a main-thread snapshot: it's stale for an owner who enabled
                                    // here, and an independent `from_bytes` reconstruction mints a
                                    // divergent yrs client_id whose merge tombstones the op-log
                                    // (the kb/approve #179 rule). Absent replica (post-restart,
                                    // before a kbc: broadcast re-seeds it) → empty base → decode
                                    // fails → legacy daemon remove (no rotation; the removed member
                                    // is re-stranded on a later re-share).
                                    let base_state = match kb_collections.get(&kb_id) {
                                        Some(s) => s.clone(),
                                        None => {
                                            warn!(kb = %kb_id, member = %member, "kb/remove: E2e KB but no local collection replica yet (post-restart?) — deferring to the legacy daemon remove (no key rotation)");
                                            Vec::new()
                                        }
                                    };
                                    if let Ok(mut coll) =
                                        mae_sync::kb::KbCollectionDoc::from_bytes(&base_state)
                                    {
                                        let owner_fp = id.fingerprint();
                                        if coll.owner() == owner_fp {
                                            let owner_pubkey = id.public().to_bytes();
                                            let owner_secret = id.secret_bytes();
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_secs())
                                                .unwrap_or(0);
                                            let ops = coll.oplog_ops();
                                            let gov = mae_sync::membership::derive_governance(
                                                &ops,
                                                &owner_pubkey,
                                            );
                                            let members =
                                                mae_sync::membership::derive_valid_members_governed(
                                                    &ops,
                                                    &owner_pubkey,
                                                    now,
                                                    gov,
                                                    &mae_sync::membership::MembershipView::default(),
                                                );
                                            // Fresh key, wrapped once per REMAINING member (owner
                                            // re-keys itself via its own pubkey).
                                            let k2 = mae_sync::content_crypto::ContentKey::generate();
                                            let mut rewraps: Vec<(String, Vec<u8>)> = Vec::new();
                                            let mut skipped: Vec<String> = Vec::new();
                                            for fp in members.keys() {
                                                if *fp == member {
                                                    continue;
                                                }
                                                // ADR-041 (#158 I1): re-wrap to each member's
                                                // PUBLISHED X25519 wrap key — the owner's own
                                                // (derived from its seed) or the stored member key.
                                                let wrap_pk = if *fp == owner_fp {
                                                    Some(mae_sync::content_crypto::wrap_public_for(
                                                        &owner_secret,
                                                    ))
                                                } else {
                                                    coll.member_wrap_pubkey(fp)
                                                };
                                                match wrap_pk.and_then(|wpk| {
                                                    mae_sync::content_crypto::wrap_to_member(&k2, &wpk)
                                                        .ok()
                                                }) {
                                                    Some(w) => rewraps.push((fp.clone(), w)),
                                                    None => skipped.push(fp.clone()),
                                                }
                                            }
                                            if !skipped.is_empty() {
                                                warn!(kb = %kb_id, ?skipped, "kb/remove (E2e): re-key skipped members with no stored pubkey — they keep the OLD key until a re-share re-wraps them");
                                            }
                                            let delta = coll.author_rotate_on_remove(
                                                &kb_id,
                                                &member,
                                                &rewraps,
                                                &owner_fp,
                                                &owner_secret,
                                                &owner_pubkey,
                                                now,
                                            );
                                            let sent = if let Some(ref mut w) = writer {
                                                let req_id = next_request_id;
                                                next_request_id += 1;
                                                let req = mae_sync::wire::kb_collection_op_request(
                                                    req_id,
                                                    &kb_id,
                                                    &mae_sync::encoding::update_to_base64(&delta),
                                                );
                                                match serde_json::to_vec(&req) {
                                                    Ok(body) => write_framed(w, &body, write_timeout)
                                                        .await
                                                        .is_ok(),
                                                    Err(_) => false,
                                                }
                                            } else {
                                                false
                                            };
                                            if sent {
                                                // #169 M2 + #173: register the key
                                                // `find_wrapped_content_key` resolves from the
                                                // post-rotation collection (the causal-order
                                                // winner), NOT the locally-generated k2 — so the
                                                // owner's seal key matches what every receiver
                                                // derives. Seed the collection replica with the
                                                // post-rotation state so subsequent membership
                                                // deltas re-derive correctly (#173).
                                                kb_collections
                                                    .insert(kb_id.clone(), coll.encode_state());
                                                let winner = derive_kb_content_key(
                                                    &coll.encode_state(),
                                                    id,
                                                )
                                                .unwrap_or_else(|| k2.clone());
                                                if let Some(d) = mae_mcp::content_key_store::content_keys_dir()
                                                    .as_ref()
                                                {
                                                    if let Err(e) = mae_mcp::content_key_store::save(
                                                        d,
                                                        &kb_id,
                                                        winner.as_bytes(),
                                                    ) {
                                                        warn!(kb = %kb_id, error = %e, "kb/remove (E2e): failed to persist rotated content key");
                                                    }
                                                }
                                                content_keys.insert(kb_id.clone(), winner);
                                                shipped_e2e = true;
                                                info!(kb = %kb_id, member = %member, remaining = rewraps.len(), "ADR-037 §D3: removed member + rotated content key to remaining members");
                                            } else {
                                                e2e_write_failed = true;
                                                warn!(kb = %kb_id, member = %member, "kb/remove (E2e): failed to ship the revoke+rekey delta — member NOT removed, content key NOT rotated");
                                            }
                                        }
                                    }
                                }
                            }
                            if e2e_write_failed {
                                try_send_evt(
                                    &evt_tx,
                                    CollabEvent::Error {
                                        message: format!(
                                            "Failed to remove '{member}' from encrypted KB '{kb_id}' — the revoke+rekey op could not reach the daemon. The member was NOT removed and the content key was NOT rotated; try again once reconnected."
                                        ),
                                    },
                                );
                            } else if !shipped_e2e {
                                // #265: adding a member by FINGERPRINT to an E2e KB cannot wrap
                                // the content key (we don't hold their published X25519 wrap key),
                                // so they'd be admitted KEYLESS — in the roster but unable to
                                // decrypt any content. Don't let that pass silently: warn the
                                // owner and steer them to the join→approve path, which DOES seal
                                // the key to the member's published wrap key. (The add still
                                // proceeds so the member can be re-keyed on a later approve.)
                                if add && content_keys.contains_key(&kb_id) {
                                    try_send_evt(
                                        &evt_tx,
                                        CollabEvent::StatusReport {
                                            lines: vec![
                                                format!("⚠  '{member}' added to ENCRYPTED KB '{kb_id}' by fingerprint — they are KEYLESS and cannot decrypt content."),
                                                "   A fingerprint add can't seal the content key (no published wrap key to seal to).".to_string(),
                                                "   Have the member run  :kb-join  and approve them with  :kb-approve  — that seals the key.".to_string(),
                                            ],
                                        },
                                    );
                                    warn!(kb = %kb_id, member = %member, "kb/add_member on an E2e KB by fingerprint — member is KEYLESS until they join+approve (#265)");
                                }
                                if let Some(ref mut w) = writer {
                                    let req_id = next_request_id;
                                    next_request_id += 1;
                                    let method =
                                        if add { "kb/add_member" } else { "kb/remove_member" };
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
                            // ADR-037/038: on an E2e KB the OWNER authors a signed Admit
                            // carrying the content key wrapped to the approved member (whose
                            // pubkey rode the pending request), shipped via the key-blind
                            // kb/collection_op. Else: the legacy daemon-authored approve.
                            let mut shipped_e2e = false;
                            // #round-4: same "don't fall through on write failure" concern as
                            // kb/remove's E2E rekey path — falling through to the generic
                            // kb/approve_member below would admit the member WITHOUT their
                            // content key ever being wrapped/shipped (a keyless admit, exactly
                            // the #265 bug class), so a write failure here must not silently
                            // downgrade to that instead of being reported.
                            let mut e2e_write_failed = false;
                            if let (Some(key), Some(id)) =
                                (content_keys.get(&kb_id).cloned(), signing_identity.as_ref())
                            {
                                // Fail-CLOSED: author ONLY against the network task's OWN replica
                                // (`kb_collections` — the daemon's lineage, kept CRDT-merged from
                                // the ops WE authored at enable plus every daemon broadcast incl.
                                // the pending-add pubkey, by #173). NEVER fall back to the
                                // main-thread `collection_state` snapshot: independent `from_bytes`
                                // reconstructions mint different yrs client_ids, so that snapshot
                                // can be a DIVERGENT lineage whose merge tombstones the genesis/
                                // SetEncryption ops (the join-decrypt bug). If the replica is absent
                                // (e.g. post-restart, before a kbc: broadcast re-seeds it — unlike
                                // `content_keys`, it isn't reloaded from disk), use an EMPTY base so
                                // the decode below fails into the legacy member-add (the member is
                                // re-wrapped on a later sync) — never risk op-log corruption.
                                let base_state = match kb_collections.get(&kb_id) {
                                    Some(s) => s.clone(),
                                    None => {
                                        warn!(kb = %kb_id, member = %principal, "kb/approve: E2e KB but no local collection replica yet (post-restart?) — deferring the key wrap to legacy member-add (avoids authoring against a divergent base)");
                                        Vec::new()
                                    }
                                };
                                if let Ok(mut coll) =
                                    mae_sync::kb::KbCollectionDoc::from_bytes(&base_state)
                                {
                                    let pend = coll
                                        .pending()
                                        .into_iter()
                                        .find(|p| p.fingerprint == principal);
                                    // ADR-041 (#158 I1): wrap the content key to the joiner's
                                    // PUBLISHED X25519 wrap key (not the ed25519 key). Both ride
                                    // the pending record; admit needs both (ed25519 for ADR-038
                                    // rotation bookkeeping, wrap key for the actual wrap).
                                    let member_pk = pend.as_ref().and_then(|p| p.pubkey);
                                    let member_wrap_pk = pend.as_ref().and_then(|p| p.wrap_pubkey);
                                    if let (Some(member_pk), Some(member_wrap_pk)) =
                                        (member_pk, member_wrap_pk)
                                    {
                                        if let Ok(wrapped) =
                                            mae_sync::content_crypto::wrap_to_member(&key, &member_wrap_pk)
                                        {
                                            let role_enum = mae_sync::kb::Role::parse(&role)
                                                .unwrap_or(mae_sync::kb::Role::Editor);
                                            let now = std::time::SystemTime::now()
                                                .duration_since(std::time::UNIX_EPOCH)
                                                .map(|d| d.as_secs())
                                                .unwrap_or(0);
                                            let delta = coll.author_member_admit(
                                                &kb_id, &principal, &member_pk, &member_wrap_pk,
                                                role_enum, &principal, wrapped, &id.fingerprint(),
                                                &id.secret_bytes(), &id.public().to_bytes(), now,
                                            );
                                            let sent = if let Some(ref mut w) = writer {
                                                let req_id = next_request_id;
                                                next_request_id += 1;
                                                let req = mae_sync::wire::kb_collection_op_request(
                                                    req_id, &kb_id,
                                                    &mae_sync::encoding::update_to_base64(&delta),
                                                );
                                                match serde_json::to_vec(&req) {
                                                    Ok(body) => write_framed(w, &body, write_timeout)
                                                        .await
                                                        .is_ok(),
                                                    Err(_) => false,
                                                }
                                            } else {
                                                false
                                            };
                                            if sent {
                                                // Advance our replica so a later admit/rotation
                                                // chains onto admit-bob (same single-authority lineage).
                                                kb_collections
                                                    .insert(kb_id.clone(), coll.encode_state());
                                                shipped_e2e = true;
                                                info!(kb = %kb_id, member = %principal, "ADR-037: approved member with wrapped content key");
                                            } else {
                                                e2e_write_failed = true;
                                                warn!(kb = %kb_id, member = %principal, "kb/approve (E2e): failed to ship the member-admit delta — member NOT approved");
                                            }
                                        }
                                    } else {
                                        warn!(kb = %kb_id, member = %principal, "kb/approve: E2e KB but the pending request carries no pubkey — falling back to legacy (member keyless until re-wrap)");
                                    }
                                }
                            }
                            if e2e_write_failed {
                                try_send_evt(
                                    &evt_tx,
                                    CollabEvent::Error {
                                        message: format!(
                                            "Failed to approve '{principal}' for encrypted KB '{kb_id}' — the member-admit op could not reach the daemon. The member was NOT approved; try again once reconnected."
                                        ),
                                    },
                                );
                            } else if !shipped_e2e {
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
                        CollabCommand::KbBlockPrincipal { kb_id, principal, block } => {
                            // ADR-039 A2 (#162): local self-protection blocklist. Fire-and-
                            // forget like set_policy — the daemon enforces locally; nothing
                            // syncs back (the block is never in the `kbc:` collection).
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let method = if block {
                                    "kb/block_principal"
                                } else {
                                    "kb/unblock_principal"
                                };
                                let req = serde_json::json!({
                                    "jsonrpc": "2.0", "id": req_id, "method": method,
                                    "params": { "kb_id": kb_id, "fingerprint": principal }
                                });
                                if let Ok(body) = serde_json::to_vec(&req) {
                                    let _ = write_framed(w, &body, write_timeout).await;
                                }
                                // Re-pull the blocklist so the *KB Sharing* Blocked view
                                // reflects this change (the daemon is authoritative).
                                send_blocklist_fetch(w, &mut next_request_id, &mut pending_responses, write_timeout).await;
                            }
                        }
                        CollabCommand::KbSetEncryption { kb_id, mode, collection_state, node_states } => {
                            // ADR-037/038/039: enable E2E. ALL key crypto stays in this
                            // network task (it holds the secret); the daemon stays key-blind
                            // (it only stores the owner-signed delta via kb/collection_op).
                            if mode != "e2e" {
                                warn!(kb = %kb_id, %mode, "kb/set_encryption: only 'e2e' is supported (one-way)");
                            } else if let Some(id) = signing_identity.as_ref() {
                                let owner_fp = id.fingerprint();
                                let owner_pubkey = id.public().to_bytes();
                                let owner_secret = id.secret_bytes();
                                match mae_sync::kb::KbCollectionDoc::from_bytes(&collection_state) {
                                    Ok(mut coll) if coll.owner() == owner_fp => {
                                        // F4 (ADR-039): E2e requires SingleOwner governance
                                        // (a quorum-removable owner would freeze the key).
                                        let gov = mae_sync::membership::derive_governance(
                                            &coll.oplog_ops(), &owner_pubkey,
                                        );
                                        if gov != mae_sync::membership::Governance::SingleOwner {
                                            warn!(kb = %kb_id, "kb/set_encryption: E2e requires SingleOwner governance — refused");
                                        } else {
                                            // Generate-or-load + persist the content key.
                                            let dir = mae_mcp::content_key_store::content_keys_dir();
                                            let key = match dir.as_ref().and_then(|d| mae_mcp::content_key_store::load(d, &kb_id)) {
                                                Some(b) => mae_sync::content_crypto::ContentKey::from_bytes(b),
                                                None => {
                                                    let k = mae_sync::content_crypto::ContentKey::generate();
                                                    if let Some(d) = dir.as_ref() {
                                                        if let Err(e) = mae_mcp::content_key_store::save(d, &kb_id, k.as_bytes()) {
                                                            warn!(kb = %kb_id, error = %e, "kb/set_encryption: failed to persist content key");
                                                        }
                                                    }
                                                    k
                                                }
                                            };
                                            // ADR-041 (#158 I1): self-wrap to the owner's OWN
                                            // published X25519 wrap key (derived from its seed).
                                            match mae_sync::content_crypto::wrap_to_member(&key, &mae_sync::content_crypto::wrap_public_for(&owner_secret)) {
                                                Ok(self_wrap) => {
                                                    let now = std::time::SystemTime::now()
                                                        .duration_since(std::time::UNIX_EPOCH)
                                                        .map(|d| d.as_secs())
                                                        .unwrap_or(0);
                                                    let delta = coll.author_e2e_genesis(
                                                        &kb_id, &owner_fp, &owner_secret, &owner_pubkey, self_wrap, now,
                                                    );
                                                    if let Some(ref mut w) = writer {
                                                        let req_id = next_request_id;
                                                        next_request_id += 1;
                                                        let req = mae_sync::wire::kb_collection_op_request(
                                                            req_id, &kb_id, &mae_sync::encoding::update_to_base64(&delta),
                                                        );
                                                        if let Ok(body) = serde_json::to_vec(&req) {
                                                            let _ = write_framed(w, &body, write_timeout).await;
                                                        }
                                                    }
                                                    // #156 F5 (enable-time scrub): a KB enabled E2e while it
                                                    // already had nodes still has their cleartext titles in the
                                                    // manifest (the forward-case blanking only covers nodes
                                                    // added AFTER enable). Blank them now — the real titles ride
                                                    // encrypted in the resealed node op-sets below; the
                                                    // key-blind daemon must not keep the plaintext copies. Ship
                                                    // as an owner-authored collection delta (no-op if nothing
                                                    // to blank).
                                                    let blank = coll.blank_node_titles_delta();
                                                    if !blank.is_empty() {
                                                        if let Some(ref mut w) = writer {
                                                            let req_id = next_request_id;
                                                            next_request_id += 1;
                                                            let mut req = mae_sync::wire::kb_collection_op_request(
                                                                req_id, &kb_id, &mae_sync::encoding::update_to_base64(&blank),
                                                            );
                                                            // #156 F5: ask the daemon to COMPACT the kbc: doc
                                                            // after applying, so the pre-enable cleartext title
                                                            // doesn't linger in the kbc: WAL at rest (the
                                                            // snapshot is already title-blanked; this purges the
                                                            // superseded WAL update + checkpoints the sidecar).
                                                            if let Some(p) = req.get_mut("params").and_then(|p| p.as_object_mut()) {
                                                                p.insert("scrub".to_string(), serde_json::Value::Bool(true));
                                                            }
                                                            if let Ok(body) = serde_json::to_vec(&req) {
                                                                let _ = write_framed(w, &body, write_timeout).await;
                                                            }
                                                        }
                                                    }
                                                    // #171: RE-SEAL every already-shared node under the new
                                                    // key. A node shared PLAINTEXT before enable has a
                                                    // plaintext daemon lineage under the owner's content
                                                    // client_id; the ADR-023 fence pins the seal client_id to
                                                    // that SAME id, so a naive first seal (fresh op-set at
                                                    // clock 0) OVERLAPS the plaintext clocks and yrs drops it
                                                    // — joiners then never see sealed content. #171 PURGE:
                                                    // instead of merging onto that plaintext lineage we now
                                                    // build a FRESH op-set (empty seed → op 0 at clock 0,
                                                    // self-contained) and ship it `reseal:true` so the daemon
                                                    // `share_doc`-REPLACES `kb:{node}` — deleting the
                                                    // pre-enable plaintext snapshot+WAL — rather than stacking
                                                    // the op-set on top of it. Enable is the none→e2e
                                                    // transition, so no node has a prior op-set to continue;
                                                    // op 0 carries the node's full current state, later sealed
                                                    // edits chain onto it, and a joiner opens only ciphertext.
                                                    let owner_epoch = coll.epoch_of(&owner_fp);
                                                    for (node_id, plaintext_state) in &node_states {
                                                        let seed: Vec<u8> = Vec::new();
                                                        if let Some((mut req, new_op_set_state, sealed_id)) =
                                                            build_kb_node_update_request(
                                                                next_request_id,
                                                                &kb_id,
                                                                node_id,
                                                                plaintext_state,
                                                                owner_epoch,
                                                                Some(id),
                                                                Some(&key),
                                                                true,
                                                                &seed,
                                                            )
                                                        {
                                                            // #171 purge: flag this op so the daemon
                                                            // `share_doc`-REPLACES `kb:{node}` (deleting the
                                                            // pre-enable plaintext snapshot+WAL) instead of
                                                            // merging the op-set on top of it. Additive param
                                                            // (an old daemon ignores it → harmless merge).
                                                            if let Some(p) = req
                                                                .get_mut("params")
                                                                .and_then(|p| p.as_object_mut())
                                                            {
                                                                p.insert(
                                                                    "reseal".to_string(),
                                                                    serde_json::Value::Bool(true),
                                                                );
                                                            }
                                                            next_request_id += 1;
                                                            if let Some(ref mut w) = writer {
                                                                if let Ok(body) = serde_json::to_vec(&req) {
                                                                    let _ = write_framed(w, &body, write_timeout).await;
                                                                }
                                                            }
                                                            op_sets.insert(node_id.clone(), new_op_set_state);
                                                            if let Some(sid) = sealed_id {
                                                                seen_ops
                                                                    .entry(node_id.clone())
                                                                    .or_default()
                                                                    .insert(sid);
                                                            }
                                                            node_to_kb
                                                                .insert(node_id.clone(), kb_id.clone());
                                                        }
                                                    }
                                                    // Register the key so the owner's edits now seal (Phase 2b).
                                                    content_keys.insert(kb_id.clone(), key);
                                                    // #173: seed/refresh the collection replica with the
                                                    // post-genesis state so later membership deltas re-derive.
                                                    kb_collections
                                                        .insert(kb_id.clone(), coll.encode_state());
                                                    info!(kb = %kb_id, resealed_nodes = node_states.len(), "ADR-037: E2E enabled — content key generated + self-wrapped, signed genesis authored, nodes re-sealed");
                                                }
                                                Err(e) => warn!(kb = %kb_id, error = ?e, "kb/set_encryption: self-wrap failed"),
                                            }
                                        }
                                    }
                                    Ok(_) => warn!(kb = %kb_id, "kb/set_encryption: not the KB owner — skipped"),
                                    Err(e) => warn!(kb = %kb_id, error = %e, "kb/set_encryption: collection decode failed"),
                                }
                            } else {
                                warn!(kb = %kb_id, "kb/set_encryption: E2E requires key mode (no signing identity)");
                            }
                        }
                        CollabCommand::KbCollectionOp { kb_id, update } => {
                            if let Some(ref mut w) = writer {
                                let req_id = next_request_id;
                                next_request_id += 1;
                                let req = mae_sync::wire::kb_collection_op_request(
                                    req_id, &kb_id, &mae_sync::encoding::update_to_base64(&update),
                                );
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
                                &mut KbCryptoCtx {
                                    content_keys: &mut content_keys,
                                    op_sets: &mut op_sets,
                                    node_to_kb: &mut node_to_kb,
                                    seen_ops: &mut seen_ops,
                                    kb_collections: &mut kb_collections,
                                    signing_identity: signing_identity.as_ref(),
                                    pending_collection_ops: &mut pending_collection_ops,
                                },
                            );
                            // ADR-040 PR2c: ship any owner-side reactive re-wraps the receive
                            // path queued (a member rotated on an E2e KB we own → deliver the
                            // content key to their successor). The owner-gated `kb/collection_op`
                            // accepts these (we sign with the owner key); advance the replica so
                            // a later op chains on the re-wrapped state.
                            if let Some(w) = writer.as_mut() {
                                for (kb_id, delta) in pending_collection_ops.drain(..) {
                                    let req_id = next_request_id;
                                    next_request_id += 1;
                                    let req = mae_sync::wire::kb_collection_op_request(
                                        req_id,
                                        &kb_id,
                                        &mae_sync::encoding::update_to_base64(&delta),
                                    );
                                    if let Ok(body) = serde_json::to_vec(&req) {
                                        let _ = write_framed(w, &body, write_timeout).await;
                                    }
                                    if let Some(base) = kb_collections.get(&kb_id) {
                                        if let Ok(mut coll) =
                                            mae_sync::kb::KbCollectionDoc::from_bytes(base)
                                        {
                                            if coll.apply_update(&delta).is_ok() {
                                                kb_collections
                                                    .insert(kb_id.clone(), coll.encode_state());
                                            }
                                        }
                                    }
                                    info!(kb = %kb_id, "rotate-identity: owner re-wrapped content key to a rotated member's successor");
                                }
                            } else {
                                pending_collection_ops.clear();
                            }
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
                                daemon_start_grace_ms, &transport,
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
                                            // ADR-039 A2 (#162): pull the local blocklist for the Blocked view.
                                            send_blocklist_fetch(w, &mut next_request_id, &mut pending_responses, write_timeout).await;
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
                    daemon_start_grace_ms,
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
/// ADR-037 §2b (#146): the network task's content-encryption state, threaded into the
/// message handlers (which live outside `run_collab_task`). Bundling these keeps the
/// handler signatures sane and the crypto state in one place. `signing_identity` is the
/// secret half — used only to derive a content key on join/share. See `run_collab_task`.
pub(crate) struct KbCryptoCtx<'a> {
    pub content_keys:
        &'a mut std::collections::HashMap<String, mae_sync::content_crypto::ContentKey>,
    pub op_sets: &'a mut std::collections::HashMap<String, Vec<u8>>,
    pub node_to_kb: &'a mut std::collections::HashMap<String, String>,
    pub seen_ops: &'a mut std::collections::HashMap<String, std::collections::BTreeSet<String>>,
    /// #173: per-KB full collection replicas (seeded at join/share/enable), advanced by
    /// inbound `kbc:` deltas so the content key can be **re-derived** on any membership
    /// change (rotation, post-join wrap) — without this, a member's key is frozen at first
    /// derive and 3c rotation never reaches it.
    pub kb_collections: &'a mut std::collections::HashMap<String, Vec<u8>>,
    pub signing_identity: Option<&'a std::sync::Arc<mae_mcp::identity::Identity>>,
    /// ADR-040 PR2c: owner-side reactive re-wrap queue. When a member's `Rebind` for
    /// *another* principal lands on an E2e KB this peer OWNS, the receive path authors an
    /// `author_rebind_rewrap` delivering the content key to the successor and pushes
    /// `(kb_id, signed_delta)` here; the network loop ships each via the owner-gated
    /// `kb/collection_op` and advances the replica. Empty on non-owners / non-E2e KBs.
    pub pending_collection_ops: &'a mut Vec<(String, Vec<u8>)>,
}

/// ADR-040 PR2b: the planned content-op deltas to ship for ONE KB during an owner
/// identity rotation, plus the advanced collection replica to store back.
pub(crate) struct RotationPlan {
    pub kb_id: String,
    /// Signed deltas to ship via the owner-gated `kb/collection_op`, in order: the `Rebind`
    /// first, then — for an E2e KB — the owner re-wrap of the content key to the new key.
    pub deltas: Vec<Vec<u8>>,
    /// The advanced collection replica to store back into `kb_collections`.
    pub new_replica: Vec<u8>,
}

/// ADR-040 PR2b: plan an **owner** identity rotation across every KB this peer owns. For each
/// KB whose `kb_collections` replica is owned by `old_id` (genesis owner fingerprint ==
/// `old_id`), author a `Rebind` — the OLD owner key signs, carrying the new Ed25519 + published
/// X25519 wrap keys — and, for an E2e KB, the owner re-wrap of the CURRENT content key to the
/// owner's NEW wrap key (signed by the new key, anchored on the old genesis;
/// `author_rebind_rewrap`). Pure: it decodes fresh `KbCollectionDoc`s and never mutates shared
/// state — the caller ships the deltas via the owner-gated `kb/collection_op` and stores
/// `new_replica` back. KBs this peer does **not** own are skipped (a non-owner member's
/// rotation needs the PR2c member-authored path, issue #213). Order across KBs is unspecified
/// (each KB is independent); within a KB the Rebind precedes the re-wrap.
pub(crate) fn plan_owner_rotation(
    kb_collections: &std::collections::HashMap<String, Vec<u8>>,
    content_keys: &std::collections::HashMap<String, mae_sync::content_crypto::ContentKey>,
    old_id: &mae_mcp::identity::Identity,
    new_id: &mae_mcp::identity::Identity,
    now: u64,
) -> Vec<RotationPlan> {
    use mae_sync::kb::{Encryption, KbCollectionDoc};
    let old_fp = old_id.fingerprint();
    let new_fp = new_id.fingerprint();
    let old_pk = old_id.public().to_bytes();
    let old_sec = old_id.secret_bytes();
    let new_pk = new_id.public().to_bytes();
    let new_sec = new_id.secret_bytes();
    let new_wrap_pk = mae_sync::content_crypto::wrap_public_for(&new_sec);
    let mut plans = Vec::new();
    for (kb_id, state) in kb_collections {
        let Ok(mut coll) = KbCollectionDoc::from_bytes(state) else {
            continue;
        };
        // Only KBs I OWN — a non-owner member's self-Rebind has no daemon path yet (PR2c).
        if coll.owner() != old_fp {
            continue;
        }
        let mut deltas = Vec::new();
        // 1. Rebind: the OLD owner key cross-signs the new key (still valid at this point).
        deltas.push(coll.author_rebind(
            kb_id,
            &old_fp,
            &new_fp,
            &new_pk,
            &new_wrap_pk,
            &old_sec,
            &old_pk,
            now,
        ));
        // 2. E2e: re-wrap the content key to the owner's NEW wrap key. Signed by the NEW key
        //    (the old is retired the instant its Rebind lands), anchored on the OLD genesis.
        if coll.encryption() == Encryption::E2e {
            if let Some(key) = content_keys.get(kb_id) {
                if let Ok(wrapped) = mae_sync::content_crypto::wrap_to_member(key, &new_wrap_pk) {
                    deltas.push(coll.author_rebind_rewrap(
                        kb_id, &new_fp, &new_pk, wrapped, &old_pk, &new_fp, &new_sec, &new_pk, now,
                    ));
                }
            }
        }
        plans.push(RotationPlan {
            kb_id: kb_id.clone(),
            deltas,
            new_replica: coll.encode_state(),
        });
    }
    plans
}

/// ADR-040 PR2c — plan a NON-owner **member** identity rotation. For each KB in
/// `kb_collections` this peer is a member of but does **not** own, author a self-`Rebind`
/// (the OLD key signs, carrying the new Ed25519 + published X25519 wrap keys). No content
/// re-wrap: the member cannot pass the daemon's owner-`Manage` gate for a re-wrap op, so the
/// owner delivers the content key to the successor reactively
/// ([`plan_reactive_member_rewraps`]). Owned KBs are skipped (handled by
/// [`plan_owner_rotation`]); so are KBs where I am not a roster member. Pure — the caller
/// ships the deltas via the (PR2c-gated) `kb/collection_op` and stores `new_replica` back.
pub(crate) fn plan_member_rotation(
    kb_collections: &std::collections::HashMap<String, Vec<u8>>,
    old_id: &mae_mcp::identity::Identity,
    new_id: &mae_mcp::identity::Identity,
    now: u64,
) -> Vec<RotationPlan> {
    use mae_sync::kb::KbCollectionDoc;
    let old_fp = old_id.fingerprint();
    let new_fp = new_id.fingerprint();
    let old_pk = old_id.public().to_bytes();
    let old_sec = old_id.secret_bytes();
    let new_pk = new_id.public().to_bytes();
    let new_wrap_pk = mae_sync::content_crypto::wrap_public_for(&new_id.secret_bytes());
    let mut plans = Vec::new();
    for (kb_id, state) in kb_collections {
        let Ok(mut coll) = KbCollectionDoc::from_bytes(state) else {
            continue;
        };
        // Owned KBs go through plan_owner_rotation; only rotate where I am a non-owner member.
        if coll.owner() == old_fp || coll.role_of(&old_fp).is_none() {
            continue;
        }
        let delta = coll.author_rebind(
            kb_id,
            &old_fp,
            &new_fp,
            &new_pk,
            &new_wrap_pk,
            &old_sec,
            &old_pk,
            now,
        );
        plans.push(RotationPlan {
            kb_id: kb_id.clone(),
            deltas: vec![delta],
            new_replica: coll.encode_state(),
        });
    }
    plans
}

/// ADR-040 §Recovery-key — plan registering this peer's offline **recovery key** across every
/// KB it is a member of (owner or member). For each, author a `RegisterRecoveryKey` op signed
/// by the CURRENT primary (`principal`), carrying the recovery key's public Ed25519 key. Pure;
/// the caller ships the deltas (owned KBs ride the owner-`Manage` gate, member KBs ride the
/// PR3 self-service gate) and stores `new_replica` back. Registration grants no new access — it
/// just publishes the recovery key so a future [`plan_recovery_rotation`] is honored. KBs where
/// I hold no role are skipped.
pub(crate) fn plan_register_recovery_key(
    kb_collections: &std::collections::HashMap<String, Vec<u8>>,
    principal: &mae_mcp::identity::Identity,
    recovery_pubkey: &[u8; 32],
    now: u64,
) -> Vec<RotationPlan> {
    use mae_sync::kb::KbCollectionDoc;
    let fp = principal.fingerprint();
    let sec = principal.secret_bytes();
    let pk = principal.public().to_bytes();
    let mut plans = Vec::new();
    for (kb_id, state) in kb_collections {
        let Ok(mut coll) = KbCollectionDoc::from_bytes(state) else {
            continue;
        };
        if coll.role_of(&fp).is_none() {
            continue; // only KBs where I am a member
        }
        let delta = coll.author_register_recovery_key(kb_id, &fp, recovery_pubkey, &sec, &pk, now);
        plans.push(RotationPlan {
            kb_id: kb_id.clone(),
            deltas: vec![delta],
            new_replica: coll.encode_state(),
        });
    }
    plans
}

/// ADR-040 §Recovery-key — plan recovering a lost/compromised primary `old_fp` to the
/// successor `new_id` using the pre-registered offline `recovery` key. For each KB `old_fp` is
/// a member of, author a recovery-signed `Rebind` (`author_recovery_rebind`: author = `old_fp`,
/// subject = `new_id`, signed by the RECOVERY secret). `new_id` contributes only its public
/// keys — it is the connected/authorized successor (recovery is run AS the new key, so the
/// daemon's PR3 gate sees `subject == principal`). Owner re-wrap of E2e content keys is the
/// owner's reactive job ([`plan_reactive_member_rewraps`]) exactly as for a member rotation;
/// a recovering OWNER re-derives its own key from the post-rebind collection. Pure. KBs where
/// `old_fp` holds no role are skipped.
pub(crate) fn plan_recovery_rotation(
    kb_collections: &std::collections::HashMap<String, Vec<u8>>,
    old_fp: &str,
    new_id: &mae_mcp::identity::Identity,
    recovery: &mae_mcp::identity::Identity,
    now: u64,
) -> Vec<RotationPlan> {
    use mae_sync::kb::KbCollectionDoc;
    let new_fp = new_id.fingerprint();
    let new_pk = new_id.public().to_bytes();
    let new_wrap_pk = mae_sync::content_crypto::wrap_public_for(&new_id.secret_bytes());
    let rec_sec = recovery.secret_bytes();
    let rec_pk = recovery.public().to_bytes();
    let mut plans = Vec::new();
    for (kb_id, state) in kb_collections {
        let Ok(mut coll) = KbCollectionDoc::from_bytes(state) else {
            continue;
        };
        if coll.role_of(old_fp).is_none() {
            continue; // only KBs where the lost key is a member
        }
        let delta = coll.author_recovery_rebind(
            kb_id,
            old_fp,
            &new_fp,
            &new_pk,
            &new_wrap_pk,
            &rec_sec,
            &rec_pk,
            now,
        );
        plans.push(RotationPlan {
            kb_id: kb_id.clone(),
            deltas: vec![delta],
            new_replica: coll.encode_state(),
        });
    }
    plans
}

/// #173: a `kbc:` collection delta arrived — advance this peer's collection replica and
/// **re-derive** its content key, so a membership change (3c rotation, post-join
/// wrap-on-admit) reaches the seal/open path live instead of leaving the member frozen on a
/// stale key. Persists the refreshed key. No-op for a non-`kbc:` doc, an unseeded KB (we
/// never joined it), an undecodable delta, or when no key derives (unencrypted / not a
/// member / not yet wrapped to us — so a plain KB is never disturbed).
/// ADR-040 PR2c — owner-side reactive re-wrap planner (pure). Given this peer's CURRENT
/// collection replica `base` and an inbound `delta`, find every NEW member `Rebind` for a
/// principal OTHER than this owner and — for an E2e KB this peer owns (genesis owner) —
/// author an `author_rebind_rewrap` delivering `content_key` to each successor's published
/// X25519 wrap key. Returns the signed `kbc:` deltas to ship via the owner-gated
/// `kb/collection_op`. The successor inherits the content key with NO new authority (a
/// re-wrap, not an admit). Empty unless this peer currently speaks for the owner (the genesis
/// owner OR any cross-signed rotation successor), the KB is E2e, and the delta introduces a
/// fresh, fingerprint-bound member Rebind by someone else.
///
/// A rotated owner IS handled (#239/#237): authority is resolved through the owner principal
/// chain (`is_owner_principal`, ADR-040), NOT the collection's meta `owner()` field — which
/// still points at the genesis fingerprint after the owner rotates. So the compound sequence
/// "owner rotates, then a member rotates" still delivers the content key to the member's
/// successor. Pinned by `plan_reactive_member_rewraps_works_after_the_owner_has_itself_rotated`.
pub(crate) fn plan_reactive_member_rewraps(
    kb_id: &str,
    base: &[u8],
    delta: &[u8],
    content_key: &mae_sync::content_crypto::ContentKey,
    owner: &mae_mcp::identity::Identity,
    now: u64,
) -> Vec<Vec<u8>> {
    use mae_sync::kb::{Encryption, KbCollectionDoc};
    use mae_sync::membership::{fingerprint_of, MembershipAction};

    let owner_fp = owner.fingerprint();
    let Ok(base_coll) = KbCollectionDoc::from_bytes(base) else {
        return Vec::new();
    };
    let Ok(mut merged) = KbCollectionDoc::from_bytes(base) else {
        return Vec::new();
    };
    if merged.apply_update(delta).is_err() {
        return Vec::new();
    }
    // Only an E2e KB.
    if merged.encryption() != Encryption::E2e {
        return Vec::new();
    }
    // The genesis owner pubkey anchors the derive (drives find_wrapped_content_key); fall
    // back to my own key if the op-log has no genesis (legacy/owned-without-oplog).
    let anchor = merged
        .oplog_ops()
        .iter()
        .find(|o| {
            o.op.prev_hash.is_empty()
                && o.op.action == MembershipAction::Admit
                && o.op.subject == o.op.author
        })
        .map(|o| o.author_pubkey)
        .unwrap_or_else(|| owner.public().to_bytes());
    // I must currently speak for the owner to re-wrap: the genesis owner, ANY cross-signed
    // rotation successor (ADR-040 — after the owner itself rotates, the collection's meta
    // `owner()` still points at the GENESIS fingerprint, so a bare `owner() == owner_fp` check
    // wrongly skips a rotated owner reacting to a member's rebind), or the meta owner field
    // itself (a legacy/owned KB with no anchored genesis). The daemon re-validates each op.
    if !mae_sync::membership::is_owner_principal(&merged.oplog_ops(), &anchor, &owner_fp)
        && merged.owner() != owner_fp
    {
        return Vec::new();
    }

    // Collect the NEW, fresh, fingerprint-bound member Rebinds (subject ≠ me) before
    // authoring, so the immutable scan doesn't clash with the mutable re-wrap authoring.
    let base_hashes: std::collections::HashSet<String> = base_coll
        .oplog_ops()
        .iter()
        .map(|o| o.chain_hash())
        .collect();
    let mut targets: Vec<(String, [u8; 32], [u8; 32])> = Vec::new();
    for o in merged.oplog_ops() {
        if base_hashes.contains(&o.chain_hash())
            || o.op.action != MembershipAction::Rebind
            || o.op.subject == owner_fp
            || o.op.author == owner_fp
        {
            continue;
        }
        let (Some(new_pk), Some(new_wrap_pk)) = (o.op.new_pubkey, o.op.new_wrap_pubkey) else {
            continue;
        };
        if fingerprint_of(&new_pk) != o.op.subject {
            continue; // not bound — a malformed rebind never reaches a valid derive
        }
        targets.push((o.op.subject.clone(), new_pk, new_wrap_pk));
    }

    let mut out = Vec::with_capacity(targets.len());
    let owner_pk = owner.public().to_bytes();
    let owner_sec = owner.secret_bytes();
    for (new_fp, new_pk, new_wrap_pk) in targets {
        let Ok(wrapped) = mae_sync::content_crypto::wrap_to_member(content_key, &new_wrap_pk)
        else {
            continue;
        };
        out.push(merged.author_rebind_rewrap(
            kb_id, &new_fp, &new_pk, wrapped, &anchor, &owner_fp, &owner_sec, &owner_pk, now,
        ));
    }
    out
}

/// ADR-040 §Recovery (B2): mirror `kb_id`'s latest collection op-log to the durable on-disk
/// store so it survives a restart / restore. Best-effort — a write failure is logged, never
/// fatal (the in-memory replica remains authoritative for the session). The op-log is key-blind
/// (roster + signed ops + ciphertext wraps, no plaintext secrets); see [`mae_mcp::collection_store`].
pub(crate) fn persist_collection(kb_id: &str, state: &[u8]) {
    if let Some(d) = mae_mcp::collection_store::collections_dir() {
        if let Err(e) = mae_mcp::collection_store::save(&d, kb_id, state) {
            warn!(kb = %kb_id, error = %e, "B2: failed to persist collection op-log to disk");
        }
    }
}

pub(crate) fn refresh_kb_content_key_on_collection_delta(
    kb: &mut KbCryptoCtx,
    doc_id: &str,
    delta: &[u8],
) {
    let Some(kb_id) = doc_id.strip_prefix("kbc:") else {
        return;
    };
    let Some(id) = kb.signing_identity else {
        return;
    };
    // Snapshot this peer's PRE-delta replica (so the inbound rebind reads as a NEW op for
    // the reactive scan) without holding a borrow across the later `insert`.
    let Some(base_bytes) = kb.kb_collections.get(kb_id).cloned() else {
        return; // a KB we never seeded (never joined/shared/enabled here)
    };
    // Advance the local collection replica by the delta.
    let merged = match mae_sync::kb::KbCollectionDoc::from_bytes(&base_bytes) {
        Ok(mut coll) => match coll.apply_update(delta) {
            Ok(()) => coll.encode_state(),
            Err(e) => {
                warn!(kb = %kb_id, error = %e, "#173: kbc delta apply failed — content key not refreshed");
                return;
            }
        },
        Err(e) => {
            warn!(kb = %kb_id, error = %e, "#173: collection replica decode failed");
            return;
        }
    };
    kb.kb_collections.insert(kb_id.to_string(), merged.clone());
    // B2: persist every inbound membership delta so the on-disk op-log stays current.
    persist_collection(kb_id, &merged);
    // ADR-040 PR2c: owner-side reactive re-wrap. If a member rotated on an E2e KB we own,
    // deliver the content key to their successor (the member can't — only the owner passes
    // the daemon's owner-Manage gate for a re-wrap op). Queue the signed deltas; the network
    // loop ships them.
    let rewraps = match kb.content_keys.get(kb_id) {
        Some(key) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            plan_reactive_member_rewraps(kb_id, &base_bytes, delta, key, id, now)
        }
        None => Vec::new(),
    };
    for delta in rewraps {
        kb.pending_collection_ops.push((kb_id.to_string(), delta));
    }
    // Re-derive the (possibly rotated) key from the advanced collection.
    if let Some(new_key) = derive_kb_content_key(&merged, id) {
        let changed = kb
            .content_keys
            .get(kb_id)
            .map(|k| k.as_bytes() != new_key.as_bytes())
            .unwrap_or(true);
        if changed {
            if let Some(d) = mae_mcp::content_key_store::content_keys_dir() {
                if let Err(e) = mae_mcp::content_key_store::save(&d, kb_id, new_key.as_bytes()) {
                    warn!(kb = %kb_id, error = %e, "#173: failed to persist re-derived content key");
                }
            }
            info!(kb = %kb_id, "#173: re-derived content key after a collection update (rotation/admit)");
        }
        kb.content_keys.insert(kb_id.to_string(), new_key);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_incoming_message(
    text: &str,
    evt_tx: &mpsc::Sender<CollabEvent>,
    pending_responses: &mut std::collections::HashMap<u64, PendingResponseKind>,
    shared_docs: &mut Vec<String>,
    seq_tracker: &mut std::collections::HashMap<String, u64>,
    kb: &mut KbCryptoCtx,
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
                handle_response(&val, kind, evt_tx, shared_docs, seq_tracker, kb);
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
                                    route_kb_node_update(
                                        node_id,
                                        bytes,
                                        kb.content_keys,
                                        kb.node_to_kb,
                                        kb.op_sets,
                                        kb.seen_ops,
                                        evt_tx,
                                    );
                                } else {
                                    // #173: a `kbc:` collection delta — refresh this peer's
                                    // content key live (rotation / post-join wrap) BEFORE the
                                    // bytes are moved into the event (no-op for non-kbc docs).
                                    refresh_kb_content_key_on_collection_delta(
                                        kb,
                                        &buffer_name,
                                        &bytes,
                                    );
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
                                route_kb_node_update(
                                    node_id,
                                    bytes,
                                    kb.content_keys,
                                    kb.node_to_kb,
                                    kb.op_sets,
                                    kb.seen_ops,
                                    evt_tx,
                                );
                            } else {
                                // #173: refresh content key on a `kbc:` collection delta
                                // (rotation / post-join wrap) before the bytes are moved.
                                refresh_kb_content_key_on_collection_delta(kb, &doc_id, &bytes);
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
#[allow(clippy::too_many_arguments)]
fn handle_response(
    val: &serde_json::Value,
    kind: PendingResponseKind,
    evt_tx: &mpsc::Sender<CollabEvent>,
    shared_docs: &mut Vec<String>,
    seq_tracker: &mut std::collections::HashMap<String, u64>,
    kb: &mut KbCryptoCtx,
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
        PendingResponseKind::Blocklist => {
            // ADR-039 A2 (#162): `{ blocklist: { kb_id: [fingerprint, ...] } }`.
            let mut blocklist: std::collections::HashMap<String, Vec<String>> =
                std::collections::HashMap::new();
            if let Some(map) = result
                .and_then(|r| r.get("blocklist"))
                .and_then(|b| b.as_object())
            {
                for (kb_id, arr) in map {
                    let fps: Vec<String> = arr
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    blocklist.insert(kb_id.clone(), fps);
                }
            }
            try_send_evt(evt_tx, CollabEvent::BlocklistUpdated { blocklist });
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
                // ADR-037 §2b: the owner derives its own content key from the collection
                // it just shared (the genesis self-wrap targets the owner, so
                // `derive_content_key` recovers it). node→kb for the owner's nodes is
                // registered lazily on the first push (the send arm). `None` until the
                // owner has generated + self-wrapped a key (Phase 3 lifecycle).
                if let Some(id) = kb.signing_identity {
                    // #173: seed the collection replica so a later membership change
                    // (rotation / admit) re-derives the key live.
                    kb.kb_collections
                        .insert(kb_id.clone(), collection_state.clone());
                    // B2: persist the seeded op-log (durability + recovery precondition).
                    persist_collection(&kb_id, &collection_state);
                    if let Some(content_key) = derive_kb_content_key(&collection_state, id) {
                        kb.content_keys.insert(kb_id.clone(), content_key);
                        debug!(kb = %kb_id, "ADR-037: derived owner content key on share");
                    }
                }
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
                let mut nodes: Vec<JoinedNode> = result
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
                // ADR-037 §2b: on an encrypted KB, derive THIS peer's content key from
                // the collection's signed op-log (the network task holds both the bytes
                // AND the identity secret here) and register every joined node → kb, so
                // inbound op-set updates select the right key. `None` ⇒ unencrypted /
                // not a member / not yet wrapped to me ⇒ plaintext path (unchanged).
                if let Some(id) = kb.signing_identity {
                    // #173: seed the collection replica so a later rotation / admit
                    // re-derives the (rotated) key live for this member.
                    kb.kb_collections
                        .insert(kb_id.clone(), collection_state.clone());
                    // B2: persist the seeded op-log (durability + recovery precondition).
                    persist_collection(&kb_id, &collection_state);
                    if let Some(content_key) = derive_kb_content_key(&collection_state, id) {
                        for n in &mut nodes {
                            kb.node_to_kb.insert(n.id.clone(), kb_id.clone());
                            // JOIN-DECRYPT: on an E2e KB the join snapshot is the SEALED
                            // op-set (the same wire form node_updates carry) — OPEN it into
                            // the plaintext node CRDT so the joiner stores READABLE content,
                            // not ciphertext. Without this, a member who joins can't read any
                            // encrypted content (the live route_kb_node_update path opens
                            // op-sets; the join snapshot did not). Seed op_sets + seen_ops so
                            // subsequent live updates dedupe + chain on this state.
                            let prev = kb.op_sets.get(&n.id).cloned().unwrap_or_default();
                            let merged = mae_sync::op_set::merge(&prev, &n.bytes)
                                .unwrap_or_else(|_| n.bytes.clone());
                            // The join snapshot is a FULL state — reconstruct the node from the
                            // COMPLETE op-set (op 0 = base doc), NOT just the ops missing from the
                            // live `seen` set. Using `seen` here would skip an already-seen BASE op
                            // and rebuild from a mid-stream update → a gap that panics
                            // `reconcile_remote_node` (a recovered member, who absorbed pre-join
                            // broadcasts into `seen`, hit exactly this). Open against a FRESH set so
                            // every op materializes; fold them into the real `seen` afterward so
                            // subsequent LIVE updates still dedupe + chain on this state.
                            let full_seen = std::collections::BTreeSet::new();
                            let opened =
                                mae_sync::op_set::open_new_ops(&merged, &content_key, &full_seen);
                            if !opened.ops.is_empty() {
                                // Reconstruct the node CRDT: op 0 is the base doc, the rest
                                // are incremental updates (mirror of the seal round-trip).
                                if let Ok(mut reader) =
                                    mae_sync::kb::KbNodeDoc::from_bytes(&opened.ops[0].1)
                                {
                                    for (_oid, pt) in &opened.ops[1..] {
                                        let _ = reader.apply_update(pt);
                                    }
                                    debug!(target: "kb_sync", node = %n.id, "JOIN-DECRYPT: materialized plaintext from sealed snapshot on join");
                                    n.bytes = reader.encode_state();
                                }
                                let seen = kb.seen_ops.entry(n.id.clone()).or_default();
                                for (op_id, _) in &opened.ops {
                                    seen.insert(op_id.clone());
                                }
                                kb.op_sets.insert(n.id.clone(), merged);
                            }
                            // opened empty ⇒ unencrypted node / not-our-key ⇒ leave raw.
                            // #206: distinguish "wrong/rotated key" from "not yet received"
                            // on the join-snapshot path too.
                            if opened.undecryptable > 0 {
                                try_send_evt(
                                    evt_tx,
                                    CollabEvent::KbOpsUndecryptable {
                                        kb_id: kb_id.clone(),
                                        node_id: n.id.clone(),
                                        count: opened.undecryptable,
                                    },
                                );
                            }
                        }
                        kb.content_keys.insert(kb_id.clone(), content_key);
                        debug!(kb = %kb_id, nodes = nodes.len(), "ADR-037: derived content key + opened sealed snapshots on join");
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

/// Fetch the daemon's LOCAL self-protection blocklist (ADR-039 A2, #162) for the
/// `*KB Sharing*` Blocked view. The blocklist is local-only on the daemon (never in the
/// synced `kbc:` collection), so this RPC is the ONLY way the editor learns it. Sent on
/// connect (durable prior-session blocks) and after each block/unblock (the user's own
/// changes); the reply lands as `PendingResponseKind::Blocklist` → `BlocklistUpdated`.
async fn send_blocklist_fetch<W: tokio::io::AsyncWrite + Unpin>(
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
        "method": "kb/blocklist",
    });
    let body = serde_json::to_vec(&req).unwrap();
    if write_framed(writer, &body, timeout).await.is_ok() {
        pending.insert(req_id, PendingResponseKind::Blocklist);
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
    daemon_start_grace_ms: u64,
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
                            // ADR-039 A2 (#162): pull the local blocklist for the Blocked view.
                            send_blocklist_fetch(
                                w,
                                next_request_id,
                                pending_responses,
                                write_timeout,
                            )
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
                    tokio::time::sleep(std::time::Duration::from_millis(daemon_start_grace_ms))
                        .await;
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
                                    // ADR-039 A2 (#162): pull the local blocklist.
                                    send_blocklist_fetch(
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
        CollabCommand::RotateIdentity => {
            // Rotation must ship Rebinds to the daemon, so it requires a live connection.
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: "Not connected \u{2014} connect before rotating your identity"
                        .to_string(),
                },
            );
        }
        CollabCommand::RegisterRecoveryKey => {
            // Registration ships `RegisterRecoveryKey` ops to the daemon — needs a connection.
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: "Not connected \u{2014} connect before registering a recovery key"
                        .to_string(),
                },
            );
        }
        CollabCommand::RecoverIdentity { .. } => {
            // Recovery ships recovery-signed Rebinds — connect with your NEW key first (§4).
            try_send_evt(
                evt_tx,
                CollabEvent::Error {
                    message: "Not connected \u{2014} connect with your new key before recovering your identity"
                        .to_string(),
                },
            );
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
            // Re-derived from `kb_epochs` / the cached collection at the next drain, so the
            // stale epoch + e2e flag aren't carried across the requeue.
            epoch: _,
            e2e: _,
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
        | CollabCommand::KbBlockPrincipal { kb_id, .. }
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
        CollabCommand::KbSetEncryption { kb_id, .. }
        | CollabCommand::KbCollectionOp { kb_id, .. } => {
            debug!(kb = %kb_id, "not connected — KB collection op dropped (re-enable on reconnect)");
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
mod tests;
