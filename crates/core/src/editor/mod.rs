//! @ai-caution: [architecture-debt] Editor module root. Went from 3,457 to
//! ~1,382 lines (2026-07): a dozen orphaned value-structs moved into the
//! sibling files that already imported them (`lsp_state.rs`, `git_ops.rs`,
//! `ai_state.rs`), and ~90 `impl Editor` methods regrouped into
//! `window_ops.rs`/`render_ops.rs`/`session_ops.rs`/`conversation_ops.rs`
//! plus extended `keymaps.rs`/`option_ops.rs`/`project_ops.rs`. Residual is
//! the `Editor` struct definition itself (see the separate `[dispatch]`
//! marker below on its field count) plus constructors/small lifecycle
//! methods, with no further obvious seam. Tracked in
//! .claude/commands/mae-audit.md's "Known exceptions" and ROADMAP.md's
//! "Architecture Debt" section.

mod agenda_ops;
pub mod ai_state;
mod babel_ops;
mod changes;
mod command;
mod conversation_ops;
pub mod daemon_capability;
mod dap_ops;
pub mod dap_state;
mod debug_panel_ops;
mod diagnostics;
pub mod dispatch;
mod edit_ops;
pub(crate) mod ex_parse;
mod file_ops;
mod git_ops;
mod graph_view_ops;
mod heading_ops;
pub(crate) mod help_ops;
mod hook_ops;
mod idle_ops;
mod jumps;
pub(crate) mod kb_ops;
mod kb_preview_ops;
mod kb_sharing_ops;
pub mod kb_state;
mod keymaps;
mod lsp_actions;
mod lsp_completion;
mod lsp_ops;
pub mod lsp_state;
mod lsp_symbols;
mod macros;
mod markdown_ops;
mod marks;
mod mouse_ops;
mod multicursor;
mod notify_ops;
mod option_ops;
mod org_ops;
pub mod perf;
mod project_ops;
mod register_ops;
mod render_ops;
mod scheme_ops;
mod search_ops;
mod session_ops;
mod surround;
mod syntax_ops;
mod table_ops;
mod text_objects;
pub mod vi_state;
mod visual;
mod window_ops;

pub use ai_state::{AiNetworkCheck, AiState, InputLock};
pub use changes::{ChangeEntry, CHANGE_LIST_CAP};
pub use daemon_capability::{
    DaemonFeature, DaemonRequirement, DaemonStateSnapshot, FeatureAvailability,
};
pub use dap_state::DapContext;
pub use diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticStore};
pub use git_ops::{BlameEntry, BlameOverlay, PendingGitDiff};
pub use help_ops::is_builtin_node;
pub use jumps::{JumpEntry, JUMP_LIST_CAP};
pub use kb_ops::{KbPromoteResult, KbResolution, KbWatcherStats, PromoteDedup};
pub use kb_state::{DaemonControl, DaemonMode, KbContext};
pub use lsp_state::{
    CodeActionItem, CodeActionMenu, CompletionItem, HoverPopup, LspContext, LspServerInfo,
    LspServerStatus, PeekReferenceLocation, PeekReferencesState, PeekState, SignatureHelpInfo,
    SignatureHelpState, SymbolOutlineEntry, SymbolOutlineState,
};
pub use session_ops::EditorStateSnapshot;
pub use vi_state::ViState;

/// Default TCP address for the collaborative state server.
pub const DEFAULT_COLLAB_ADDRESS: &str = "127.0.0.1:9473";
/// Default TCP port for the collaborative state server.
pub const DEFAULT_COLLAB_PORT: u16 = 9473;

/// Default KB instance name (primary KB).
pub const KB_DEFAULT_NAME: &str = "default";
/// Default KB sync mode for collaborative editing.
pub const KB_SYNC_MODE_DEFAULT: &str = "on_save";

/// Collaborative editing connection status.
/// Surfaced in the status bar via `format_collab_status()`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum CollabStatus {
    /// No collaborative session configured or active.
    #[default]
    Off,
    /// Establishing initial connection to the state server.
    Connecting,
    /// Connected to the state server with `peer_count` other editors.
    Connected { peer_count: usize },
    /// Lost connection, attempting to re-establish.
    Reconnecting,
    /// Disconnected from the state server (not retrying).
    Disconnected,
}

impl CollabStatus {
    /// Short string label for this status (used by AI tools, Scheme API, introspect).
    pub fn as_str(&self) -> &'static str {
        match self {
            CollabStatus::Off => "off",
            CollabStatus::Connecting => "connecting",
            CollabStatus::Connected { .. } => "connected",
            CollabStatus::Reconnecting => "reconnecting",
            CollabStatus::Disconnected => "disconnected",
        }
    }
}

/// Intent signals from the editor core to the binary event loop.
///
/// The binary drains `editor.pending_collab_intent` each tick, similar to
/// `pending_lsp_requests` and `pending_dap_intents`.
/// A KB-sharing lifecycle action requested from a high-level surface (the Scheme
/// primitives). [`Editor::queue_kb_collab_action`] lowers it to the matching
/// [`CollabIntent`] (computing editor-side data like `Join` state-vectors), so all
/// three actors — command, MCP tool, Scheme — share one intent path (#3, #7).
#[derive(Debug, Clone)]
pub enum KbCollabAction {
    Share {
        kb_name: String,
    },
    Join {
        kb_id: String,
    },
    Leave {
        kb_id: String,
    },
    AddMember {
        kb_id: String,
        member: String,
        role: String,
    },
    RemoveMember {
        kb_id: String,
        member: String,
    },
    Approve {
        kb_id: String,
        principal: String,
        role: String,
    },
    SetPolicy {
        kb_id: String,
        policy: String,
    },
    /// Enable E2E content encryption on an owned KB (owner-only, ADR-037/039).
    SetEncryption {
        kb_id: String,
        mode: String,
    },
    /// Block/unblock a principal on a KB's LOCAL self-protection blocklist (ADR-039 A2,
    /// #162). `blocked` = true blocks, false unblocks. Local-only; not owner-gated.
    SetBlock {
        kb_id: String,
        member: String,
        blocked: bool,
    },
}

#[derive(Debug, Clone)]
pub enum CollabIntent {
    /// Start a local daemon process.
    StartServer,
    /// Connect to a remote daemon.
    Connect { address: String },
    /// Disconnect from the current server.
    Disconnect,
    /// ADR-040 PR2b: rotate this peer's collab identity key — generate a new keypair, author a
    /// cross-signed `Rebind` (+ E2e content-key re-wrap) into every KB this peer owns, ship them
    /// to the daemon, and swap to the new key. Owner-only in v1 (non-owner member rotation is
    /// PR2c / #213). The transport re-anchor is out-of-band: authorize the new key on the daemon,
    /// then reconnect.
    RotateIdentity,
    /// ADR-040 PR3 / §Recovery-key: register an offline recovery key — generate a fresh Ed25519
    /// recovery keypair, author `RegisterRecoveryKey` into every KB this peer is a member of,
    /// save the recovery secret to a distinct path, and advise backing it up OFFLINE. The
    /// recovery key can later authorize a `Rebind` if the primary is lost ([`Self::RecoverIdentity`]).
    RegisterRecoveryKey,
    /// ADR-040 PR3 / §Recovery-key: recover a lost/compromised primary using the pre-registered
    /// offline recovery key at `recovery_path`. Run AS the new key (already authorized + connected
    /// out-of-band, §4): authors a recovery-signed `Rebind` (`old_fp` → the connected identity)
    /// into every KB `old_fp` is a member of, so the new key inherits the lost key's seats.
    RecoverIdentity {
        recovery_path: String,
        old_fp: String,
    },
    /// Show the *Collab Status* diagnostic buffer.
    ShowStatus,
    /// Share the named buffer for collaborative editing.
    ShareBuffer { buffer_name: String },
    /// Force sync the named buffer.
    ForceSync { buffer_name: String },
    /// Run connectivity diagnostics.
    Doctor,
    /// List shared documents on the server (opens *Collab Docs* buffer).
    ListDocs,
    /// List docs, then open a palette picker for joining.
    ListDocsForJoin,
    /// Join a shared document by name (create buffer from server state).
    JoinDoc { doc_id: String },
    /// Save a synced buffer via the collab save protocol (docs/save_intent).
    SaveCollab {
        doc_id: String,
        content_hash: String,
    },
    /// Share a KB instance for collaborative editing.
    ShareKb {
        kb_name: String,
        node_ids: Vec<String>,
    },
    /// Join a shared KB from the server. `node_svs` carries this editor's
    /// per-node state vectors (ADR-022) so the daemon can reply with an
    /// incremental diff per node and the member reconciles instead of adopting a
    /// full snapshot (crash-safe re-join). Empty on a first-ever join with no
    /// local nodes — the daemon then sends full state.
    JoinKb {
        kb_id: String,
        node_svs: Vec<(String, Vec<u8>)>,
    },
    /// Leave (unsubscribe from) a shared KB.
    LeaveKb { kb_id: String },
    /// Add a peer (by principal/fingerprint) to a KB with a role (owner-only, ADR-018).
    KbAddMember {
        kb_id: String,
        member: String,
        role: String,
    },
    /// Remove a peer (by principal) from a KB's members (owner-only, ADR-018).
    KbRemoveMember { kb_id: String, member: String },
    /// Approve a pending join request as `role` (owner-only, ADR-018).
    KbApprove {
        kb_id: String,
        principal: String,
        role: String,
    },
    /// List pending join requests for a KB (owner-only, ADR-018).
    KbListPending { kb_id: String },
    /// Set a KB's join policy (restrictive|invite|permissive; owner-only, ADR-018).
    KbSetPolicy { kb_id: String, policy: String },
    /// Add/remove a principal on a KB's LOCAL self-protection blocklist (ADR-039 A2,
    /// #162). `blocked` = true blocks, false unblocks. Local-only to this daemon — never
    /// propagated; not owner-gated (you may block even the owner).
    KbSetBlock {
        kb_id: String,
        member: String,
        blocked: bool,
    },
    /// Enable E2E content encryption on an owned KB (owner-only, ADR-037/039).
    KbSetEncryption { kb_id: String, mode: String },
    /// Send a CRDT update for a KB node to the server.
    KbNodeUpdate {
        kb_id: String,
        node_id: String,
        update: Vec<u8>,
    },
    /// ADR-024 R1: fetch a node's authoritative state from the daemon and ADOPT it
    /// locally (drop the stale-epoch divergence that the daemon fenced), so a
    /// legitimately-granted member can resume editing. If a `pending_reauthor`
    /// entry exists for `(kb_id, node_id)`, the adopted node is then re-authored
    /// under the current epoch (the graceful keep-mine path).
    KbAdoptNode { kb_id: String, node_id: String },
    /// Discover peers on the local network via mDNS.
    DiscoverPeers,
}

/// Shell/terminal intent queue and cached state, extracted from Editor.
/// All fields were previously `pending_shell_*` / `shell_*` on Editor;
/// now accessed via `editor.shell.*`.
#[derive(Debug, Default)]
pub struct ShellIntents {
    /// Buffer indices of newly created shell buffers that need PTY spawning.
    pub spawns: Vec<usize>,
    /// Working directory overrides for shell spawns: buffer_idx → dir.
    pub cwds: HashMap<usize, std::path::PathBuf>,
    /// Agent shell spawns: (buf_idx, command).
    pub agent_spawns: Vec<(usize, String)>,
    /// Buffer indices of shell terminals that should be reset (clear screen).
    pub resets: Vec<usize>,
    /// Buffer indices of shell terminals that should be closed.
    pub closes: Vec<usize>,
    /// Queued text to send to shell terminals: (buffer_index, text).
    pub inputs: Vec<(usize, String)>,
    /// Pending scroll amount. Positive = up, negative = down, zero = bottom.
    pub scroll: Option<i32>,
    /// Pending mouse click: (row, col, button).
    pub click: Option<(usize, usize, crate::input::MouseButton)>,
    /// Pending mouse drag position: (row, col).
    pub drag: Option<(usize, usize)>,
    /// Pending mouse release position: (row, col).
    pub release: Option<(usize, usize)>,
    /// Cached viewport snapshots, keyed by buffer index.
    pub viewports: HashMap<usize, Vec<String>>,
    /// Cached current working directories, keyed by buffer index.
    pub viewport_cwds: HashMap<usize, String>,
}

/// Node field values captured for an ADR-024 keep-mine resolution (re-applied
/// after adopting authoritative state, under the current epoch).
#[derive(Debug, Clone)]
pub struct ReauthorFields {
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
}

/// Collaborative editing state extracted from Editor.
/// All fields were previously `collab_*` on Editor; now accessed via `editor.collab.*`.
#[derive(Debug)]
pub struct CollabState {
    /// Current connection status (Off/Connecting/Connected/Reconnecting/Disconnected).
    pub status: CollabStatus,
    /// Number of documents currently synced via the collaborative state server.
    pub synced_docs: usize,
    /// Set of buffer names currently synced via the collaborative state server.
    pub synced_buffers: HashSet<String>,
    /// Pending collaborative editing intent for the binary event loop to drain.
    pub pending_intent: Option<CollabIntent>,
    /// Queue of reconstruction intents (ADR-019) drained one-per-tick through
    /// `pending_intent` — e.g. re-join/re-share every durably-shared KB on
    /// reconnect so the editor resumes RECEIVING remote edits without a manual
    /// re-share. Fans out beyond the single-slot `pending_intent`.
    pub reconnect_intents: std::collections::VecDeque<CollabIntent>,
    /// KBs already re-subscribed this connection (idempotency guard so a
    /// reconnect storm doesn't double-join).
    pub subscribed_kbs: HashSet<String>,
    /// TCP address of the collaborative state server.
    pub server_address: String,
    /// Automatically connect to the state server on startup.
    pub auto_connect: bool,
    /// Automatically share new buffers when connected.
    pub auto_share: bool,
    /// Seconds between automatic reconnection attempts.
    pub reconnect_interval: u64,
    /// Display name for collaborative edits.
    pub user_name: String,
    /// Write timeout for peer connections, in milliseconds.
    pub write_timeout_ms: u64,
    /// Maximum pending updates before warning (0 = unlimited).
    pub max_pending_updates: u64,
    /// Exponential backoff multiplier for reconnection attempts.
    pub reconnect_backoff_factor: u64,
    /// Maximum reconnection attempts before giving up (0 = infinite).
    pub max_reconnect_attempts: u64,
    /// Milliseconds to batch local updates before sending (0 = immediate).
    pub batch_update_ms: u64,
    /// Bounded capacity of the editor→network command channel.
    pub command_queue_size: u64,
    /// Minimum seconds between force-sync gathers for the same doc (debounce).
    pub force_sync_debounce_secs: u64,
    /// Milliseconds to wait after spawning a local daemon before connecting.
    pub daemon_start_grace_ms: u64,
    /// Seconds to wait for a response to an unknown-daemon host-key trust prompt.
    pub host_key_prompt_timeout_secs: u64,
    /// When joining a doc, prompt to map to local project path.
    pub auto_resolve_paths: bool,
    /// Default directory for :saveas on joined buffers (empty = CWD).
    pub default_save_dir: String,
    /// Auto-save local file when CRDT update arrives.
    pub save_on_remote_update: bool,
    /// Seconds between heartbeat pings to the state server (0 = disabled).
    pub heartbeat_interval: u64,
    /// Pending save_committed to send on next drain tick.
    /// Format: (doc_id, save_epoch, content_hash, saved_by).
    pub pending_save_committed: Option<(String, u64, String, String)>,
    /// Doc IDs confirmed by the server (via BufferShared/BufferJoined events).
    /// Unlike `synced_buffers` which is optimistically updated on intent drain,
    /// this set is only populated after the server acknowledges the share/join.
    pub confirmed_shares: HashSet<String>,
    /// Remote user awareness state (cursors, selections, presence).
    pub remote_users: mae_sync::awareness::AwarenessMap,
    /// Pending awareness update to send (throttled at 50ms).
    pub pending_awareness: Option<(String, String)>, // (doc_id, state_json)
    /// Timestamp of last awareness send (for throttling).
    pub last_awareness_sent: std::time::Instant,
    /// Shared KB tracking: kb_id → set of node_ids being synced.
    /// Populated on KbShared (host) and KbJoined (guest) events.
    pub shared_kbs: HashMap<String, HashSet<String>>,
    /// Phase D (ADR-029): kb_ids for which a *daemon-host* share is in flight
    /// (auto-hosting the primary on connect). Consumed on the matching `KbShared`
    /// to route it down the runtime-only host path (NO durable `primary_shared`
    /// marker — hosting must not imply peer-share or survive a daemon-less launch).
    /// Also a once-per-connection guard against re-enqueuing the host share.
    pub daemon_host_pending: HashSet<String>,
    /// KB sync mode: "manual" (explicit :kb-sync), "on_save" (auto on node edit).
    pub kb_sync_mode: String,
    /// Epoch-fence resolution: "prompt" (raise the ADR-024 Accept/Keep/Stash
    /// notification — default, keeps the user in the loop) or "auto" (adopt the
    /// authoritative version + re-author the local edit in the background).
    pub fence_resolution: String,
    /// Pending KB node updates to send (accumulated between ticks). Transient
    /// fallback used only when there is no durable store; store-backed updates
    /// live in the SQLite pending queue (ADR-020 single-source emit).
    pub pending_kb_updates: Vec<(String, String, Vec<u8>)>, // (kb_id, node_id, update_bytes)
    /// Phase D1.1 (ADR-029): pending collection-manifest ops to send — `(kb_id,
    /// node_id, title, add)`. A *created* node joins the daemon's `kbc:` manifest
    /// (so the projector materializes it); a *deleted* one leaves it. Best-effort
    /// (drained when connected); creates also self-heal via the reconnect re-share.
    pub pending_kb_manifest: Vec<(String, String, String, bool)>,
    /// Durable-queue rowids of `kb/node_update`s currently on the wire awaiting the
    /// daemon's apply-confirmation (ADR-020 queue→send→confirm→ack). Prevents the
    /// drain from re-sending an in-flight row every tick; cleared on ack, requeue,
    /// or disconnect (so unconfirmed updates retry on reconnect).
    pub inflight_kb_updates: std::collections::HashSet<i64>,
    /// Stable, per-peer yrs `client_id` for local KB CRDT edits (ADR-020 B-16),
    /// derived once at startup from the durable collab identity fingerprint. Two
    /// peers MUST have distinct client_ids or their concurrent edits to the same
    /// node collide in yrs' clock space and diverge. `0` = unset (no collab identity
    /// loaded) → `kb_local_client_id()` falls back to a legacy default.
    pub local_kb_client_id: u64,
    /// ADR-024 R1: node field values captured for the **keep-mine** resolution,
    /// keyed by `(kb_id, node_id)`. Captured BEFORE a `KbAdoptNode` (since adopt
    /// overwrites the local doc); the `KbNodeAdopted` handler re-applies them under
    /// the current epoch after adopt, then removes the entry. Absent = accept-remote
    /// (discard local).
    pub pending_reauthor: HashMap<(String, String), ReauthorFields>,
    /// This peer's own collab principal (key fingerprint) — the identity the daemon
    /// authorizes against. Stored so KB node ops can be re-derived under a rotated
    /// authorization epoch (ADR-023). Empty when no collab identity is loaded.
    pub local_fingerprint: String,
    /// ADR-023 per-KB authorization epoch for THIS peer, learned from each shared
    /// KB's `kbc:` collection doc (on join + every membership broadcast). A node
    /// edit is authored under `derive_kb_client_id(local_fingerprint, epoch)`; a
    /// role change bumps the epoch (daemon-authored, unforgeable), rotating the
    /// client_id so the daemon fences the peer's pre-change lineage. Absent ⇒ 0
    /// (fresh grant / unshared), which equals the legacy base client_id.
    pub kb_epochs: HashMap<String, u64>,
    /// ADR-023 / C1: a local CRDT replica (encoded `KbCollectionDoc` state bytes)
    /// of each joined KB's `kbc:` collection doc, keyed by `kb_id`. Seeded from the
    /// full `collection_state` on join, then advanced by every live `kbc:`
    /// membership broadcast — so this peer relearns its authorization epoch the
    /// moment the owner promotes/demotes it, WITHOUT a manual reconnect. The daemon
    /// remains the sole authority (it re-derives the epoch from its own collection
    /// when fencing), so a tampered local replica can only mislead this client about
    /// its own epoch — it can never self-elevate at the daemon.
    pub kb_collection_state: HashMap<String, Vec<u8>>,
    /// ADR-039 A2 (#162): this peer's view of the daemon's LOCAL self-protection
    /// blocklist, `kb_id → blocked principals`. Unlike membership (derived from
    /// `kb_collection_state`), the blocklist is NEVER in the synced collection, so it is
    /// fetched from the daemon via `kb/blocklist` (on connect + after each block/unblock)
    /// and cached here purely to render the `*KB Sharing*` Blocked view. The daemon
    /// stays authoritative; this is display-only.
    pub kb_blocklists: HashMap<String, Vec<String>>,
    /// Pre-shared key for mutual authentication (plaintext fallback).
    pub psk: String,
    /// Shell command to retrieve the PSK (preferred over psk for security).
    pub psk_command: String,
    /// Auth mode for connecting to the daemon: "none" | "psk" | "key".
    /// "key" uses the Ed25519 trusted-peer identity (mTLS).
    pub auth_mode: String,
    /// Host-key (daemon identity) trust policy in key mode:
    /// "prompt" (interactive TOFU) | "accept-new" | "strict".
    pub host_key_policy: String,
    /// Cross-thread live mirror of `host_key_policy` for the background collab
    /// task's host-key verifier (B-21). The verifier is built once at collab-task
    /// setup but holds a clone of this `Arc` and reads it at verify-time, so a
    /// runtime `:set collab-host-key-policy` / `(set-option! …)` takes effect on
    /// the NEXT connect with no relaunch. Kept in sync with `host_key_policy`
    /// (the canonical option value used by get/set_option).
    pub host_key_policy_live: std::sync::Arc<std::sync::Mutex<String>>,
    /// Use native mTLS in key mode (recommended). When false, the plaintext
    /// JSON KeyAuth handshake is used.
    pub tls: bool,
}

impl CollabState {
    pub fn new() -> Self {
        Self {
            status: CollabStatus::Off,
            synced_docs: 0,
            synced_buffers: HashSet::new(),
            confirmed_shares: HashSet::new(),
            pending_intent: None,
            reconnect_intents: std::collections::VecDeque::new(),
            subscribed_kbs: HashSet::new(),
            server_address: DEFAULT_COLLAB_ADDRESS.to_string(),
            auto_connect: false,
            auto_share: false,
            reconnect_interval: 5,
            user_name: String::new(),
            write_timeout_ms: 5000,
            max_pending_updates: 1000,
            reconnect_backoff_factor: 2,
            max_reconnect_attempts: 0,
            batch_update_ms: 0,
            command_queue_size: 256,
            force_sync_debounce_secs: 2,
            daemon_start_grace_ms: 500,
            host_key_prompt_timeout_secs: 120,
            auto_resolve_paths: false,
            default_save_dir: String::new(),
            save_on_remote_update: false,
            heartbeat_interval: 30,
            pending_save_committed: None,
            remote_users: mae_sync::awareness::AwarenessMap::new(),
            pending_awareness: None,
            last_awareness_sent: std::time::Instant::now(),
            shared_kbs: HashMap::new(),
            daemon_host_pending: HashSet::new(),
            pending_kb_manifest: Vec::new(),
            kb_sync_mode: KB_SYNC_MODE_DEFAULT.to_string(),
            fence_resolution: "prompt".to_string(),
            pending_kb_updates: Vec::new(),
            inflight_kb_updates: std::collections::HashSet::new(),
            local_kb_client_id: 0,
            local_fingerprint: String::new(),
            kb_epochs: HashMap::new(),
            kb_collection_state: HashMap::new(),
            kb_blocklists: HashMap::new(),
            pending_reauthor: HashMap::new(),
            psk: String::new(),
            psk_command: String::new(),
            auth_mode: "psk".to_string(),
            host_key_policy: "prompt".to_string(),
            host_key_policy_live: std::sync::Arc::new(std::sync::Mutex::new("prompt".to_string())),
            tls: true,
        }
    }
}

impl Default for CollabState {
    fn default() -> Self {
        Self::new()
    }
}

/// A node delivered by the daemon on `kb/join` (ADR-022 reconcile).
#[derive(Debug, Clone)]
pub struct JoinedNode {
    /// Bare KB node id.
    pub id: String,
    /// Bytes to merge into the local node: an incremental **diff** (reconcile
    /// mode — the daemon sent only the ops we lacked) or the full **state**
    /// (first time we've seen the node, or a pre-ADR-022 daemon).
    pub bytes: Vec<u8>,
    /// The daemon's state vector for this node. `Some` → reconcile (compute our
    /// local-ahead diff against it and push back); `None` → a pre-ADR-022 daemon
    /// that sent no SV, so fall back to a legacy full-state adopt.
    pub daemon_sv: Option<Vec<u8>>,
}

/// Derive a stable, unique yrs `client_id` for KB CRDT edits from this peer's
/// durable collab identity fingerprint (ADR-020 B-16). FNV-1a, folded into the
/// **53-bit** range yrs permits for a `ClientID` (B-17); never returns `0` (yrs
/// sentinel) or `1` (the legacy single-peer default), so distinct identities map
/// to distinct ids and "unset" stays distinguishable. Set once at startup into
/// `CollabState::local_kb_client_id`.
///
/// yrs only uses the low 53 bits of a client id (the top 11 are an internal
/// tag): a full-u64 id panics in debug and *silently truncates* in release,
/// which would let two fingerprints differing only above bit 53 collide on one
/// yrs lineage — the very B-16 collision this derivation prevents.
///
/// ADR-023: `epoch` is the member's per-KB authorization epoch (0 = primary /
/// unscoped); a role change bumps it, rotating the client_id so the daemon can
/// fence a member's pre-grant ops. The implementation lives in `mae-sync` so the
/// daemon derives identically; re-exported here for the editor's call sites.
pub use mae_sync::kb::derive_kb_client_id;

/// State for an active note capture session (org-roam parity).
/// Set when `kb_create_note_from_title` creates a note; cleared by
/// `capture-finalize` (C-c C-c) or `capture-abort` (C-c C-k).
#[derive(Debug, Clone)]
pub struct CaptureState {
    pub node_id: String,
    pub file_path: Option<std::path::PathBuf>,
    pub return_buffer_idx: usize,
}
pub use lsp_ops::{DocumentHighlightRange, HighlightKind, LspLocation, LspRange};
pub use marks::Mark;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::buffer::Buffer;

/// Rekey a `HashMap<usize, V>` after a buffer at `removed_idx` was removed.
/// Drops the entry for `removed_idx` and decrements every key above it.
pub fn rekey_after_remove<V>(map: &mut HashMap<usize, V>, removed_idx: usize) {
    // Collect affected entries, then rebuild. Sorting ensures no key collisions
    // when re-inserting (e.g. removing key 0 with keys 0,1,2 present).
    let mut affected: Vec<(usize, V)> = Vec::new();
    let stale: Vec<usize> = map.keys().filter(|&&k| k >= removed_idx).copied().collect();
    for k in stale {
        if let Some(v) = map.remove(&k) {
            affected.push((k, v));
        }
    }
    for (k, v) in affected {
        if k > removed_idx {
            map.insert(k - 1, v);
        }
        // k == removed_idx: dropped
    }
}
use crate::command_palette::CommandPalette;
use crate::commands::CommandRegistry;
use crate::file_picker::FilePicker;
use crate::hooks::HookRegistry;
use crate::kb_seed::seed_kb;
use crate::keymap::{KeyPress, Keymap};
use crate::messages::MessageLog;
use crate::options::OptionRegistry;
use crate::search::SearchState;
use crate::theme::{default_theme, Theme};
use crate::window::{Rect, WindowId, WindowManager};
use crate::Mode;

/// Module information exposed to the editor and AI tools.
/// This is a projection of the richer `ModuleManifest` that lives in the binary crate.
#[derive(Debug, Clone, Default)]
pub struct ModuleInfo {
    pub name: String,
    pub version: String,
    pub status: String,
    pub category: String,
    pub description: String,
    pub commands: Vec<String>,
    pub options: Vec<String>,
    pub flags: Vec<(String, String)>,
    pub path: String,
    /// Dependencies declared in module.toml `[dependencies]`.
    pub depends: Vec<String>,
    /// Flags enabled by the user via `(use-modules! '((mod +flag)))`.
    pub enabled_flags: Vec<String>,
}

/// Links the output `*AI*` buffer and input `*ai-input*` buffer in a
/// split-view pair. The output pane is read-only (conversation history);
/// the input pane is a normal Text buffer with full vi editing.
#[derive(Debug, Clone)]
pub struct ConversationPair {
    pub output_buffer_idx: usize,
    pub input_buffer_idx: usize,
    pub output_window_id: WindowId,
    pub input_window_id: WindowId,
}

/// Record of a repeatable edit for dot-repeat (`.`).
#[derive(Clone, Debug)]
pub struct EditRecord {
    /// The command name that initiated the edit.
    pub command: String,
    /// Text inserted during insert mode (captured on exit).
    pub inserted_text: Option<String>,
    /// Character argument (for replace-char).
    pub char_arg: Option<char>,
    /// Count prefix used with this edit (for dot-repeat).
    pub count: Option<usize>,
}

/// Cached Scheme runtime statistics for MCP introspection.
/// Updated by the binary crate after each scheme eval cycle.
#[derive(Clone, Debug, Default)]
pub struct SchemeStats {
    /// Number of eval calls processed by the VM.
    pub eval_count: u64,
    /// Number of gc-collect! calls.
    pub collections_count: u64,
    /// Number of registered global bindings.
    pub globals_count: usize,
    /// Total registered functions (foreign + closure + macro).
    pub function_count: usize,
    /// Stack high-water mark.
    pub stack_hwm: usize,
    /// Number of recent errors in error history.
    pub error_count: usize,
}

// @ai-caution: [architecture-debt] [dispatch] Field count has grown well past
// the "~40 fields after ViState/AiState/CollabState/ShellIntents extraction"
// note this comment used to make (that snapshot is stale — re-measure with
// `grep -c 'pub .*:' ` scoped to the struct body rather than trusting this
// comment's number, which drifts). Before adding a field, check if the state
// belongs in a sub-struct (LspContext, DapContext, KbContext) instead — this
// struct is also independently tracked as a source-line exception (see the
// file-header marker above); the field count is the SAME debt, a second
// dimension of it, not a separate issue. Tracked in
// .claude/commands/mae-audit.md's "Known exceptions" and ROADMAP.md's
// "Architecture Debt" section.
/// Top-level editor state.
///
/// Designed as a clean, composable state machine that both human keybindings
/// and the AI agent will drive through the same method API. No I/O — all
/// side effects (file read/write) happen through Buffer's std::fs calls.
pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub window_mgr: WindowManager,
    /// Saved layout state for window maximize/restore toggle.
    pub saved_maximize_layout: Option<(
        std::collections::HashMap<crate::window::WindowId, crate::window::Window>,
        crate::window::LayoutNode,
        crate::window::WindowId,
        crate::window::WindowId,
    )>,
    pub mode: Mode,
    /// Transient keypad/leader layer (God-Mode / Meow-Keypad model). When true,
    /// key input resolves against the shared `leader` keymap (the mae which-key
    /// tree) regardless of `mode`, and clears after one command (or on cancel).
    /// Entered via the `leader-dispatch` command — `SPC` in the doom flavor,
    /// `C-;` in the non-modal flavor — so both flavors share one leader tree.
    pub leader_active: bool,
    /// Wall-clock time `leader_active` was last flipped true — `None` while
    /// inactive. Drives the `which_key_idle_delay` gate (ROADMAP #83): the
    /// which-key popup only paints once this long ago, evaluated fresh on
    /// every render by `leader_popup_ready()`. Always set/cleared together
    /// with `leader_active` via `Editor::set_leader_active` so the two can
    /// never drift out of sync.
    pub leader_activated_at: Option<std::time::Instant>,
    /// Set once `on_idle_tick` has already forced the redraw that reveals the
    /// which-key popup for the CURRENT leader activation, so repeated idle
    /// ticks don't keep marking a full redraw while the popup just sits idle
    /// on screen. Reset by `Editor::set_leader_active`.
    pub which_key_popup_redraw_done: bool,
    pub running: bool,
    pub status_msg: String,
    /// Name of the command currently being dispatched (Emacs `this-command`).
    pub current_command: String,
    pub commands: CommandRegistry,
    pub keymaps: HashMap<String, Keymap>,
    /// Data-driven routing from buffer context (kind / language) to the context
    /// keymap that overlays the modality keymap in the resolution chain. Replaces
    /// the old hardcoded match; kernel-seeded, re-seeded on
    /// `reset_keymaps_to_kernel`, and extended by modules via Scheme. See
    /// [`crate::keymap_registry`].
    pub keymap_registry: crate::keymap_registry::KeymapRegistry,
    /// Current which-key prefix being accumulated. Empty = no popup.
    pub which_key_prefix: Vec<KeyPress>,
    /// Scroll offset (in rows) for the which-key popup. Reset when prefix changes.
    pub which_key_scroll: usize,
    /// Milliseconds of idle time (no input) required, after the leader
    /// transient keypad activates, before the which-key popup paints (0 =
    /// immediate, matching the old un-timed behavior). Mirrors the
    /// `which_key_idle_delay` option (ROADMAP #83); see `on_idle_tick`.
    pub which_key_idle_delay: u64,
    /// Separator string between a key and its label in the which-key popup.
    /// Mirrors the `which_key_separator` option.
    pub which_key_separator: String,
    /// Max characters of a which-key entry's doc string before truncation.
    /// Mirrors the `which_key_max_desc_length` option.
    pub which_key_max_desc_length: usize,
    /// Max height of the which-key popup as a percentage of the window
    /// height (10-90). Mirrors the `which_key_max_height_pct` option.
    pub which_key_max_height_pct: usize,
    /// Sort order for which-key entries: "key", "desc", or "none". Mirrors
    /// the `which_key_sort_order` option; applied by `sort_which_key_entries`.
    pub which_key_sort_order: String,
    /// Milliseconds of idle time required before a KB-link hover preview
    /// popup would appear. Mirrors the `kb_preview_idle_delay` option.
    /// TODO(Part D, KB-link hover preview): the popup itself isn't built
    /// yet — this field and its idle-dispatch hook (`maybe_show_kb_preview_popup`)
    /// are the forward-compatible hook point only.
    pub kb_preview_idle_delay: u64,
    /// Default hop radius (`SubgraphSpec::max_depth`) for `(kb-graph-view-open)`
    /// when no explicit depth is given. Mirrors the `kb_graph_default_depth`
    /// option.
    pub kb_graph_default_depth: usize,
    /// Whether `extract_subgraph` includes backlinks (not just outgoing
    /// links) in the graph view's BFS walk. Mirrors `kb_graph_include_backlinks`.
    pub kb_graph_include_backlinks: bool,
    /// Safety-net cap on `extract_subgraph`'s node count (`SubgraphSpec::
    /// node_cap`), independent of `kb_graph_default_depth`/
    /// `kb_graph_include_backlinks` — a densely cross-referenced KB can make
    /// even a shallow walk explode. Mirrors `kb_graph_node_count_cap`.
    pub kb_graph_node_count_cap: usize,
    /// Node circle radius in logical pixels for the graph view's GUI
    /// rendering. Mirrors `kb_graph_node_radius`.
    pub kb_graph_node_radius: u32,
    /// Whether graph-view nodes are sized by connection count (degree).
    /// Mirrors `kb_graph_node_size_by_degree`.
    pub kb_graph_node_size_by_degree: bool,
    /// Logical px added per sqrt(degree) when `kb_graph_node_size_by_degree`
    /// is on. Mirrors `kb_graph_node_degree_scale`.
    pub kb_graph_node_degree_scale: f32,
    /// Whether graph-view node circles scale (sub-linearly, by
    /// sqrt(zoom)) with viewport zoom. Mirrors
    /// `kb_graph_node_size_scales_with_zoom`.
    pub kb_graph_node_size_scales_with_zoom: bool,
    /// Exponent applied to viewport zoom when
    /// `kb_graph_node_size_scales_with_zoom` is on. Mirrors
    /// `kb_graph_node_zoom_scale_exponent`; see `graph_view::node_render_radius`.
    pub kb_graph_node_zoom_scale_exponent: f32,
    /// Minimum node circle radius (logical px), applied after degree/zoom
    /// scaling. Mirrors `kb_graph_node_min_radius`.
    pub kb_graph_node_min_radius: u32,
    /// Maximum node circle radius (logical px), applied after degree/zoom
    /// scaling. Mirrors `kb_graph_node_max_radius`.
    pub kb_graph_node_max_radius: u32,
    /// Below this viewport zoom level, graph-view node labels are hidden.
    /// Mirrors `kb_graph_label_zoom_threshold`.
    pub kb_graph_label_zoom_threshold: f32,
    /// Whether the graph view suppresses lower-priority overlapping node
    /// labels via greedy priority-based occlusion culling — see
    /// `graph_view::compute_label_winners`. Mirrors
    /// `kb_graph_label_declutter_enabled`.
    pub kb_graph_label_declutter_enabled: bool,
    /// Curvature of internal graph-view edges, as a fraction of edge
    /// length. Mirrors `kb_graph_edge_curvature`.
    pub kb_graph_edge_curvature: f32,
    /// Whether the graph view animates hover/selection color transitions.
    /// Mirrors `kb_graph_color_tween_enabled`.
    pub kb_graph_color_tween_enabled: bool,
    /// Duration (ms) of the graph view's hover/selection color tween.
    /// Mirrors `kb_graph_color_tween_duration_ms`.
    pub kb_graph_color_tween_duration_ms: u32,
    /// Whether graph-view node circles get a stroke outline. Mirrors
    /// `kb_graph_node_border_enabled`.
    pub kb_graph_node_border_enabled: bool,
    /// Caps the HSL saturation of every graph-view node fill color
    /// resolved from the theme, preserving hue/lightness — see
    /// `graph_view::cap_saturation`. Mirrors `kb_graph_node_saturation_cap`.
    pub kb_graph_node_saturation_cap: f32,
    /// Node label font size in points for the graph view's GUI rendering.
    /// Mirrors `kb_graph_font_size` — defaults to the same numeric default
    /// as the base `font_size` option (14), but is a fully independent
    /// setting (no live-inheritance wiring — MAE has no general
    /// option-inherits-from-option mechanism today), so changing `font_size`
    /// does not retroactively change this.
    pub kb_graph_font_size: u32,
    /// Force-directed layout iteration count run by the background
    /// `graph_layout_bridge` on each open/refresh/set-depth. Mirrors
    /// `kb_graph_layout_iterations`.
    pub kb_graph_layout_iterations: usize,
    /// Strength (0.0-1.0) of node-kind-based visual clustering in the
    /// force-directed layout. Mirrors `kb_graph_layout_kind_clustering`;
    /// see `graph_view::kind_affinity_from_strength` for how this single
    /// knob maps onto the layout's repulsion/attraction multipliers.
    pub kb_graph_layout_kind_clustering: f32,
    /// Multiplies the force-layout's per-node area budget — see
    /// `mae_canvas::layout::LayoutConfig::spacing_scale`'s doc comment for
    /// the `k = sqrt(area/n) ~ sqrt(spacing_scale)` relationship. Mirrors
    /// `kb_graph_layout_spacing_scale`.
    pub kb_graph_layout_spacing_scale: f32,
    /// TODO(Part C Phase 2, not wired yet): whether the graph view
    /// re-centers on the human/AI's current KB node automatically. Mirrors
    /// `kb_graph_follow_current_node` — registered now so the OptionRegistry
    /// surface is complete ahead of Phase 2's `command-post` wiring.
    pub kb_graph_follow_current_node: bool,
    /// TODO(Part C Phase 3, not wired yet): whether the graph view's
    /// force-layout keeps ticking (physics animation) after the initial
    /// layout settles. Mirrors `kb_graph_animate` — registered now, unused
    /// until Phase 3 extends `graph_layout_bridge` to tick continuously.
    pub kb_graph_animate: bool,
    /// Whether hovering the mouse over a graph-view node highlights it in
    /// real time. Mirrors `kb_graph_hover_enabled`; read by `gui_app.rs`'s
    /// `CursorMoved` handler to gate the hover hit-test branch.
    pub kb_graph_hover_enabled: bool,
    /// Whether the graph view is currently showing as a full-frame modal
    /// overlay (dimmed background, drawn via `render_common::overlay`)
    /// instead of its normal tiled split-window pane. Deliberately NOT an
    /// `OptionRegistry` entry — this is momentary interaction state (like
    /// `mini_dialog`/`leader_active`), not a persisted preference; the
    /// preference part (how much to dim) is `kb_graph_view_overlay_dim_opacity`.
    /// Toggled by `Editor::kb_graph_view_toggle_overlay`.
    pub kb_graph_view_overlay_active: bool,
    /// Opacity (0.0-1.0) of the dimming scrim drawn behind the graph view
    /// when `kb_graph_view_overlay_active` is true.
    pub kb_graph_view_overlay_dim_opacity: f32,
    /// Queued background layout request for the open/refreshed graph-view
    /// buffer (`mae::graph_layout_bridge`, Part C Phase 1) — drained once
    /// per GUI event-loop tick, see `crate::graph_view::GraphLayoutIntent`'s
    /// doc comment for why the TUI safely ignores this.
    pub pending_graph_layout: Option<crate::graph_view::GraphLayoutIntent>,
    /// In-editor message log (*Messages* buffer equivalent).
    /// Shared with the tracing layer via MessageLogHandle.
    pub message_log: MessageLog,
    /// `MessageLog` entry `seq` last synced into the `*Messages*` buffer's
    /// rope (see `Editor::sync_open_messages_buffer`). The renderer always
    /// reads `message_log` live, but yank/visual-select/search operate on
    /// the buffer's rope — this tracks staleness so the rope gets
    /// refreshed whenever new entries have arrived since the last sync,
    /// not just once at buffer-open time.
    pub messages_synced_seq: Option<u64>,
    /// Active color theme. All rendering reads from this.
    pub theme: Theme,
    /// DAP debug session state and pending intent queue.
    pub dap: DapContext,
    /// Vi-modal editing state (operators, registers, marks, macros, command-line, etc.).
    pub vi: ViState,
    /// True while the user is resolving `SPC h k` (describe-key).
    /// The next key sequence they type is looked up in the normal
    /// keymap, and the resulting command's help page is opened instead
    /// of dispatched. Cleared on resolution or Escape.
    pub awaiting_key_description: bool,
    /// Transient flag for double-Esc detection in the *AI* output buffer.
    pub conv_esc_pending: bool,
    /// Search state (pattern, cached matches, direction).
    pub search_state: SearchState,
    /// Current search input being typed in Search mode.
    pub search_input: String,
    /// Viewport height in lines, updated each frame from the renderer.
    /// Used by scroll commands (Ctrl-U/D/F/B, H/M/L, zz/zt/zb).
    pub viewport_height: usize,
    /// Last known layout area (cell units), updated on resize events.
    /// Used by `scroll_output_to_bottom()` to compute per-window viewport heights
    /// without adding per-frame overhead.
    pub last_layout_area: Rect,
    /// Text area width in columns (after gutter), updated each frame.
    /// Used by word-wrap aware cursor movement (gj/gk).
    pub text_area_width: usize,
    /// Fuzzy file picker state. Some when the picker overlay is active.
    pub file_picker: Option<FilePicker>,
    /// Ranger-style directory browser. Some when the browser overlay is active.
    pub file_browser: Option<crate::FileBrowser>,
    /// Fuzzy command palette state. Some when the palette overlay is active.
    pub command_palette: Option<CommandPalette>,
    /// Mini-dialog state for interactive commands (edit-link, rename, etc.).
    pub mini_dialog: Option<crate::command_palette::MiniDialogState>,
    /// ADR-024 attention bus — background subsystems raise notifications here;
    /// routed by severity to status / badge / modal / `*Notifications*` buffer.
    pub notifications: crate::notifications::NotificationCenter,
    /// Reply channel for a pending `BlockingReply` notification routed to a modal
    /// (generalizes `pending_host_key_reply`). `(notif_id, reply)`; consumed on answer.
    pub pending_notif_reply: Option<(u64, crate::notifications::NotifReply)>,
    /// LSP state: intent queues, completion, hover, peek, symbols, diagnostics.
    pub lsp: LspContext,
    /// Shell/terminal intent queue and cached state.
    pub shell: ShellIntents,
    /// Buffer indices removed this tick, for the binary to rekey its own
    /// shell-related HashMaps (shell_terminals, shell_last_dims, etc.).
    pub pending_buffer_removals: Vec<usize>,
    /// Hook registry: named extension points with ordered Scheme function lists.
    /// Populated by `(add-hook! ...)` from Scheme, fired by core operations.
    pub hooks: HookRegistry,
    /// Queued hook evaluations for the binary to drain. Each entry is
    /// `(hook_name, scheme_fn_name)`. Core pushes here; the binary drains
    /// and calls the Scheme runtime (same pattern as `pending_scheme_eval`).
    pub pending_hook_evals: Vec<(String, String)>,
    /// Per-buffer tree-sitter state (parsed trees + cached highlight spans).
    /// Buffers without a detected language simply have no entry.
    pub syntax: crate::syntax::SyntaxMap,
    /// Buffer indices that need a deferred syntax reparse. Populated by the
    /// renderer when it uses stale spans; drained by the event loop after
    /// a debounce period (~50ms after last edit).
    pub syntax_reparse_pending: std::collections::HashSet<usize>,
    /// Timestamp of the last buffer edit. Used for debouncing syntax reparses.
    pub last_edit_time: std::time::Instant,
    /// Knowledge base state: backing store, federation, watchers, and config.
    pub kb: KbContext,

    /// Override for config dir (test isolation — prevents clobbering ~/.config/mae).
    pub config_dir_override: Option<std::path::PathBuf>,
    /// Override for data dir (test isolation — prevents clobbering ~/.local/share/mae).
    pub data_dir_override: Option<std::path::PathBuf>,
    /// Babel: prompt before executing blocks (default true).
    pub babel_confirm: bool,
    /// Babel: trusted file patterns that skip confirmation.
    pub babel_trust_paths: Vec<String>,
    /// Babel: execution timeout in seconds (default 30).
    pub babel_timeout: u64,
    /// Babel: merge the user's resolved shell environment into
    /// babel-spawned processes/sessions (default true).
    pub babel_inherit_shell_env: bool,
    /// Babel: C++ compiler for c++/cpp blocks (default "c++").
    pub babel_cxx_compiler: String,
    /// Babel: C compiler for c blocks (default "cc").
    pub babel_c_compiler: String,
    /// Babel: C++ standard passed as -std=<value> (default "c++17"; empty omits).
    pub babel_cxx_std: String,
    /// Babel: persistent REPL session manager.
    pub babel_sessions: crate::babel::session::SessionManager,
    // --- Snippet session ---
    /// Active snippet expansion session (Tab/S-Tab cycle fields).
    pub snippet_session: Option<mae_snippets::SnippetSession>,
    /// Snippet template store (loaded from ~/.config/mae/snippets/).
    pub snippet_store: mae_snippets::SnippetStore,
    // --- Format ---
    /// External formatter configuration (language → command).
    pub format_config: mae_format::FormatConfig,
    // --- Build ---
    /// Parsed build errors from last compilation.
    pub build_errors: Vec<mae_make::BuildError>,
    /// Current index into build_errors for next-error/prev-error navigation.
    pub build_error_idx: usize,
    // --- Spell ---
    /// Cached misspellings per buffer (keyed by buffer index).
    pub spell_results: std::collections::HashMap<usize, Vec<mae_spell::Misspelling>>,
    // --- Format/Spell options ---
    /// Run formatter before saving buffers.
    pub format_on_save: bool,
    /// Enable spell checking.
    pub spell_enabled: bool,
    /// Enable the legacy embedded AI chat window (deprecated in favor of
    /// the mae-agent TUI harness). Default false; see ADR-049.
    pub ai_chat_enabled: bool,
    /// Name of a registered KB instance (or "primary") whose content is
    /// actively surfaced to AI agents at session start as standing
    /// practices/guidance. Empty (default) disables this. See
    /// `mae_ai::guidance`.
    pub ai_guidance_kb: String,
    /// Saved help view state from the last `help_close`. `help-reopen`
    /// restores this to resume exactly where the user left off.
    pub last_kb_state: Option<crate::kb_view::KbView>,
    /// Which ASCII art to show on the splash screen. Default is "bat".
    pub splash_art: Option<String>,
    /// Custom splash arts registered via `(register-splash-art! ...)`.
    pub custom_splash_arts: Vec<crate::render_common::splash::CustomSplashArt>,
    /// Max width percentage for splash image rendering area (10–80). Default 25.
    pub splash_image_width: u32,
    /// Max height percentage of viewport for splash image (5–50). Default 20.
    pub splash_image_height: u32,
    /// Show ASCII MAE logo text below splash art/image. Default true.
    pub splash_show_logo: bool,
    /// Scheme code queued for evaluation by the binary. Commands like
    /// `eval-line` / `eval-buffer` push the captured text here; the
    /// event loop drains it after dispatch (same pattern as LSP intents).
    pub pending_scheme_eval: Vec<String>,
    /// Cached Scheme runtime statistics for introspection.
    pub scheme_stats: SchemeStats,
    /// AI session state (provider config, tokens, streaming, conversation pair, etc.).
    pub ai: AiState,
    /// Visual bell: when set, the renderer inverts the status bar background
    /// until this instant passes. Emacs `visible-bell` equivalent.
    pub bell_until: Option<std::time::Instant>,
    /// Detected project for the current working context.
    pub project: Option<crate::project::Project>,
    /// Cached git branch name for the active project. Updated on project detect and file save.
    pub git_branch: Option<String>,
    /// Recently opened files (bounded, deduplicated).
    pub recent_files: crate::project::RecentFiles,
    /// Recently used project roots (bounded, deduplicated).
    pub recent_projects: crate::project::RecentProjects,
    /// Persistent project list (saved to `projects.toml`).
    pub project_list: crate::project::ProjectList,
    /// Toggle: show line numbers in the gutter. Default true.
    pub show_line_numbers: bool,
    /// Toggle: use relative line numbers. Default false.
    pub relative_line_numbers: bool,
    /// Toggle: wrap long lines. Default false.
    pub word_wrap: bool,
    /// Toggle: continuation lines preserve indentation. Default true.
    pub break_indent: bool,
    /// String prefix for continuation lines (neovim showbreak). Default "↪ ".
    pub show_break: String,
    /// Column at which fill-paragraph wraps text (Emacs fill-column).
    pub fill_column: usize,
    /// Toggle: hide *bold* and /italic/ markers in Org-mode.
    pub org_hide_emphasis_markers: bool,
    /// Window ID of the file tree sidebar, if open. Used to track and close it.
    pub file_tree_window_id: Option<crate::window::WindowId>,
    /// Whether to auto-focus the file tree window when it opens.
    pub file_tree_focus_on_open: bool,
    /// Pending file tree action (rename/create). The command-line submit
    /// path checks this after the user types a new name.
    /// NOTE: Mostly replaced by MiniDialog — retained only for backward compat
    /// with any remaining callers during migration.
    pub file_tree_action: Option<crate::file_tree::FileTreeAction>,
    /// Toggle: show frame timing in the status bar. Default false.
    /// Toggled via `:set show_fps true` or `(set-option! "show_fps" "true")`.
    pub show_fps: bool,
    /// Name of the active rendering backend ("terminal" or "gui").
    /// Set by the binary after renderer initialization.
    pub renderer_name: String,
    /// GUI font size in points. Default 14.0. Set via config.toml `[editor] font_size`.
    pub gui_font_size: f32,
    /// User-configured font size (from config.toml). Used by reset-font-size.
    pub gui_font_size_default: f32,
    /// GUI primary font family. Default "". Set via config.toml `[editor] font_family`.
    pub gui_font_family: String,
    /// GUI icon font family (fallback). Default "". Set via config.toml `[editor] icon_font_family`.
    pub gui_icon_font_family: String,
    /// Registry of all configurable editor options — single source of truth
    /// for metadata, aliases, types, defaults, and config.toml paths.
    pub option_registry: OptionRegistry,
    /// Currently highlighted splash screen menu item index.
    pub splash_selection: usize,
    /// Debug mode: show RSS/CPU/frame time in status bar. Toggled via
    /// `--debug` CLI flag, `:debug-mode`, or `SPC t D`.
    pub debug_mode: bool,
    /// Debug init mode: verbose init file loading. Set via `--debug-init`.
    pub debug_init: bool,
    /// Clean mode: skip user config, init.scm, history on startup; skip history save on exit.
    pub clean_mode: bool,
    /// Rolling performance statistics (frame time, RSS, CPU).
    pub perf_stats: perf::PerfStats,
    /// Clipboard integration mode: "unnamedplus" (system clipboard for paste),
    /// "unnamed" (yank syncs out, paste reads internal), "internal" (no sync).
    pub clipboard: String,
    /// Keymap flavor (default "doom"): module loading auto-enables the
    /// `keymap-<flavor>` module unless the user declared a different keymap-*
    /// module. Read before autoloads run, so it belongs in init.scm/the mae!
    /// block (config.scm is too late); change at runtime via :reload-modules.
    pub keymap_flavor: String,
    /// How following a KB-graph link (gx/Enter) resolves outside the `*KB*`
    /// view: "kb-view" (default — open the rendered, federation-aware `*KB*`
    /// view, same resolver `:help <id>` uses) or "source-file" (jump
    /// straight to the node's raw `.org` source file instead). #293.
    pub kb_link_follow_mode: String,
    /// Startup editor mode ("normal" | "insert"), set by the keymap flavor
    /// (non-modal flavors use "insert"). Applied by bootstrap after modules +
    /// config load. See [`leader_active`](Self::leader_active) for the keypad.
    pub default_mode: String,
    /// Whether to restore sessions on startup. Default false.
    pub restore_session: bool,
    /// Insert-mode C-d behavior: "dedent" (vim) or "delete-forward" (Emacs).
    pub insert_ctrl_d: String,
    /// Toggle: scale heading font size in org/markdown buffers. Default true.
    pub heading_scale: bool,
    /// Case-insensitive search (vim ignorecase).
    pub ignorecase: bool,
    /// When ignorecase is on and pattern contains uppercase, search case-sensitively.
    pub smartcase: bool,
    /// Minimum lines of context above/below cursor (vim scrolloff). Default 5.
    pub scrolloff: usize,
    pub scrollbar: bool,
    pub nyan_mode: bool,
    /// Emacs `mouse-autoselect-window`: focus follows mouse hover. Default false.
    pub mouse_autoselect_window: bool,
    /// Emacs `mouse-wheel-follow-mouse`: scroll targets window under pointer. Default true.
    pub mouse_wheel_follow_mouse: bool,
    /// Mouse scroll speed multiplier. Default 3.
    pub scroll_speed: usize,
    /// Max items in LSP completion popup. Default 10.
    pub completion_max_items: usize,
    /// Max items in LSP code-action popup. Default 12.
    pub code_action_max_items: usize,
    /// Max items shown at once in the symbol-outline popup (TUI only — GUI's
    /// outline popup is a fixed-size box with scrolling). Default 20.
    pub symbol_outline_max_items: usize,
    /// Max lines in LSP hover popup. Default 15.
    pub hover_max_lines: usize,
    /// Popup width as percentage of screen. Default 70.
    pub popup_width_pct: usize,
    /// Popup height as percentage of screen. Default 60.
    pub popup_height_pct: usize,
    /// GUI scrollbar width in pixels. Default 6.0.
    pub scrollbar_width: f32,
    /// File picker max recursion depth. Default 12.
    pub file_picker_max_depth: usize,
    /// File picker max candidates. Default 50000.
    pub file_picker_max_candidates: usize,
    /// GUI window title. Default "MAE — Modern AI Editor".
    pub window_title: String,
    /// Heading scale for h1 (0.5–3.0). Default 1.5.
    pub heading_scale_h1: f32,
    /// Heading scale for h2 (0.5–3.0). Default 1.3.
    pub heading_scale_h2: f32,
    /// Heading scale for h3 (0.5–3.0). Default 1.15.
    pub heading_scale_h3: f32,
    /// Show link labels instead of raw markup (Emacs org-link-descriptive). Default true.
    pub link_descriptive: bool,
    /// Apply inline bold/italic/code styling in conversation and KB buffers. Default true.
    pub render_markup: bool,
    /// Display images inline in org/markdown buffers (GUI renders image, TUI
    /// shows placeholder). Default true. Effective value for a specific
    /// buffer is `Editor::inline_images_for` (buffer-local override via
    /// `BufferLocalOptions::inline_images` takes precedence).
    pub inline_images: bool,
    /// Show hover info in a floating popup (true) or status bar (false). Default true.
    pub lsp_hover_popup: bool,
    /// Whether the KB-link hover preview popup (Part D) auto-triggers when
    /// the cursor idles over a link in a KB-view-mode buffer, gated by
    /// `kb_preview_idle_delay`. The manual `kb-preview` command/keybinding
    /// works regardless of this option. Default true.
    pub kb_preview_on_hover: bool,
    /// Max lines shown in the KB-link hover preview popup before scrolling.
    /// Mirrors `hover_max_lines`. Default 15.
    pub kb_preview_max_lines: usize,
    /// Git blame overlay for current buffer.
    pub blame_overlay: Option<BlameOverlay>,
    /// Show inline diagnostic underlines on error/warning ranges. Default true.
    pub lsp_diagnostics_inline: bool,
    /// Show diagnostic messages as virtual text at end of line. Default true.
    pub lsp_diagnostics_virtual_text: bool,
    /// Enable LSP auto-completion popup in insert mode. Default true.
    pub lsp_completion: bool,
    /// Auto-trigger completion on trigger characters (e.g. `.`, `::`). Default true.
    pub auto_complete: bool,
    /// Show breadcrumb bar (file > symbol ancestry). Default false.
    pub show_breadcrumbs: bool,
    /// Last cursor position when a documentHighlight request was sent.
    /// Used to avoid duplicate requests when the cursor hasn't moved.
    pub highlight_last_pos: Option<(usize, usize)>,
    /// Shared heartbeat counter — incremented each event loop tick by the
    /// binary. The watchdog thread monitors this to detect main-thread stalls.
    pub heartbeat: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Consecutive stall count from the watchdog (0 = healthy). Read-only
    /// for introspection / debug overlay.
    pub watchdog_stall_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Set by watchdog after prolonged stall (>10s). Main loop checks this
    /// to cancel pending AI work and force a redraw.
    pub watchdog_stall_recovery: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Input event recorder for reproducible debugging.
    pub event_recorder: crate::event_record::EventRecorder,
    /// State stack for save/restore (push/pop) during temporary operations
    /// like self-test. AI tools call `editor_save_state` / `editor_restore_state`.
    pub state_stack: Vec<EditorStateSnapshot>,
    /// True while a self-test session is running. Set when `self_test_suite`
    /// is called (auto `save_state`), cleared on `SessionComplete` (auto `restore_state`).
    pub self_test_active: bool,
    /// Sandbox directory for test execution. When `Some`, write-path tools
    /// (create_file, rename_file, shell_exec) are confined to this directory.
    pub test_sandbox_dir: Option<std::path::PathBuf>,
    /// Last time autosave fired. Compared against `autosave_interval` option.
    pub last_autosave: std::time::Instant,
    /// Autosave interval in seconds (0 = disabled). Parsed from option registry.
    pub autosave_interval: u64,
    /// Enable swap file writing for crash recovery (default true).
    pub swap_file: bool,
    /// Custom swap directory (empty = XDG default).
    pub swap_directory: String,
    /// When `true`, the renderer shows a which-key popup with all bindings
    /// from the current buffer's overlay keymap. Set by `show-buffer-keys`,
    /// cleared on the next keypress.
    pub buffer_keys_popup: bool,
    /// Display policy: maps BufferKind → DisplayAction for buffer placement.
    /// Governs how buffers become visible (replace, avoid conversation, reuse/split, hidden).
    pub display_policy: crate::display_policy::DisplayPolicy,
    /// Editor-side share of `display_buffer_for_agent`'s fallback split, when
    /// it must place a buffer beside the conversation window group. AI
    /// conversation buffers are a PAIR (output+input) and sit outside
    /// `DisplayPolicy` (`BufferKind::Conversation` is `Hidden` there), so
    /// this — and `ai_conversation_split_ratio` below — are the pair's own
    /// equivalent lever. Mirrors `agent_display_split_ratio`.
    pub agent_display_split_ratio: f32,
    /// Output-pane share of the AI conversation buffer's output/input split.
    /// Mirrors `ai_conversation_split_ratio`.
    pub ai_conversation_split_ratio: f32,
    /// Tiered redraw level — how much work the renderer needs to do this frame.
    /// Set by event handlers, cleared after render.
    pub redraw_level: crate::redraw::RedrawLevel,
    /// Dirty line range (start_line, end_line inclusive) for PartialLines redraws.
    pub dirty_line_range: Option<(usize, usize)>,
    /// Click detection: (timestamp, row, col, click_count) of last left-click.
    pub last_click: Option<(std::time::Instant, usize, usize, u8)>,
    /// Pending rename workspace edit JSON — stored while the *Rename Preview*
    /// buffer is shown. Apply with `apply_pending_rename()`, discard with
    /// `abort_pending_rename()`.
    pub pending_rename_edit: Option<String>,
    /// GUI cell width in pixels (set by GUI after font init). Default 8.0.
    /// TUI should set to 1.0 (1 char = 1 cell).
    pub gui_cell_width: f32,
    /// GUI cell height in pixels (set by GUI after font init). Default 16.0.
    /// TUI should set to 1.0.
    pub gui_cell_height: f32,
    /// Buffer kinds whose windows can be replaced by new content instead of splitting.
    /// Configured via `set-buffer-kind-replaceable!` in Scheme or `dashboard_dismiss_on_split` in config.
    pub replaceable_kinds: Vec<crate::BufferKind>,
    /// Line count threshold for viewport-local syntax highlighting (default 5000).
    pub large_file_lines: usize,
    /// Character count above which all features degrade (default 500_000).
    pub degrade_threshold_chars: usize,
    /// Maximum line length before degradation triggers (default 10_000).
    pub degrade_threshold_line_length: usize,
    /// Milliseconds to debounce display region recomputation (default 150).
    pub display_region_debounce_ms: u64,
    /// Milliseconds to debounce syntax reparse after edits (default 50).
    pub syntax_reparse_debounce_ms: u64,
    /// Per-buffer markup span cache, keyed by buffer index. Avoids recomputing
    /// regex-based markup spans every frame for org/markdown buffers.
    pub markup_cache: HashMap<usize, crate::syntax::MarkupCache>,
    /// Per-buffer code-block-lines cache, keyed by buffer index.
    /// Viewport-local for large files, full-buffer for small files.
    pub code_block_cache: HashMap<usize, crate::syntax::ViewportCodeBlockCache>,
    /// Persistent list of org directories/files to scan for agenda items.
    /// Stored in config.toml as `[org] agenda_files = [...]`.
    pub org_agenda_files: Vec<String>,
    /// Active modules. Populated by the module loader in bootstrap.rs.
    /// Used by `:describe-module`, `list_modules` MCP tool, and `audit_configuration`.
    pub active_modules: Vec<ModuleInfo>,
    /// Keybinding conflict warnings from module loading.
    /// Populated by bootstrap when a module's autoloads.scm overrides an
    /// existing binding.
    pub module_binding_warnings: Vec<String>,
    /// Pending module reload requests. Drained by the binary which owns
    /// the SchemeRuntime and ModuleRegistry.
    pub pending_module_reloads: Vec<String>,
    /// Pending async git diff result. `request_git_diff()` spawns a background
    /// thread; `poll_pending_git_diff()` drains the result on idle ticks.
    pub pending_git_diff: Option<PendingGitDiff>,
    /// Pending package management commands (sync, upgrade, doctor).
    /// Drained by the binary crate in the event loop.
    pub pending_pkg_commands: Vec<String>,
    /// Paths for which this editor instance holds advisory file locks.
    /// Locks are acquired on file open and released on buffer close or exit.
    pub locked_files: HashSet<PathBuf>,
    /// When true, `:setup-all` is chaining through unconfigured sections.
    /// Each section's completion handler checks for the next unconfigured section.
    /// Cleared on Escape or when all sections are done.
    pub setup_all_pending: bool,
    /// Collaborative editing state (connection, sync, options).
    pub collab: CollabState,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        let commands = CommandRegistry::with_builtins();
        let keymaps = Self::default_keymaps();
        let hooks = HookRegistry::new();
        let kb = seed_kb(&commands, &keymaps, &hooks);
        Self::new_inner(commands, keymaps, hooks, kb)
    }

    fn new_inner(
        commands: CommandRegistry,
        keymaps: HashMap<String, crate::keymap::Keymap>,
        hooks: HookRegistry,
        kb: mae_kb::KnowledgeBase,
    ) -> Self {
        Editor {
            buffers: vec![Buffer::new()],
            window_mgr: WindowManager::new(0),
            saved_maximize_layout: None,
            mode: Mode::Normal,
            leader_active: false,
            leader_activated_at: None,
            which_key_popup_redraw_done: false,
            running: true,
            status_msg: String::new(),
            current_command: String::new(),
            commands,
            keymaps,
            keymap_registry: crate::keymap_registry::KeymapRegistry::kernel_defaults(),
            which_key_prefix: Vec::new(),
            which_key_scroll: 0,
            which_key_idle_delay: 0,
            which_key_separator: " ".to_string(),
            which_key_max_desc_length: 40,
            which_key_max_height_pct: crate::text_utils::WK_MAX_HEIGHT_PCT_DEFAULT,
            which_key_sort_order: "key".to_string(),
            kb_preview_idle_delay: 300,
            kb_graph_default_depth: 1,
            kb_graph_include_backlinks: true,
            kb_graph_node_count_cap: 300,
            kb_graph_node_radius: 18,
            kb_graph_node_size_by_degree: true,
            kb_graph_node_degree_scale: 4.0,
            kb_graph_node_size_scales_with_zoom: true,
            kb_graph_node_zoom_scale_exponent: 0.5,
            kb_graph_node_min_radius: 4,
            kb_graph_node_max_radius: 36,
            kb_graph_label_zoom_threshold: 0.5,
            kb_graph_label_declutter_enabled: true,
            kb_graph_edge_curvature: 0.12,
            kb_graph_color_tween_enabled: true,
            kb_graph_color_tween_duration_ms: 150,
            kb_graph_node_border_enabled: false,
            kb_graph_node_saturation_cap: 0.55,
            kb_graph_font_size: 14,
            kb_graph_layout_iterations: 50,
            kb_graph_layout_kind_clustering: 0.5,
            kb_graph_layout_spacing_scale: 2.25,
            kb_graph_follow_current_node: true,
            kb_graph_animate: false,
            kb_graph_hover_enabled: true,
            kb_graph_view_overlay_active: false,
            kb_graph_view_overlay_dim_opacity: 0.6,
            pending_graph_layout: None,
            message_log: MessageLog::new(1000), // Max message log entries (internal bound)
            messages_synced_seq: None,
            theme: default_theme(),
            dap: DapContext::new(),
            vi: ViState::new(),
            awaiting_key_description: false,
            conv_esc_pending: false,
            search_state: SearchState::default(),
            search_input: String::new(),
            viewport_height: 24,
            last_layout_area: Rect {
                x: 0,
                y: 0,
                width: 80,
                height: 24,
            },
            text_area_width: 80,
            file_picker: None,
            file_browser: None,
            command_palette: None,
            mini_dialog: None,
            notifications: crate::notifications::NotificationCenter::new(),
            pending_notif_reply: None,
            lsp: LspContext::new(),
            shell: ShellIntents::default(),
            pending_buffer_removals: Vec::new(),
            hooks,
            pending_hook_evals: Vec::new(),
            syntax: crate::syntax::SyntaxMap::new(),
            syntax_reparse_pending: std::collections::HashSet::new(),
            last_edit_time: std::time::Instant::now(),
            last_kb_state: None,
            splash_art: Some("bat".to_string()),
            custom_splash_arts: Vec::new(),
            splash_image_width: 25,
            splash_image_height: 20,
            splash_show_logo: true,
            pending_scheme_eval: Vec::new(),
            scheme_stats: SchemeStats::default(),
            kb: KbContext::new(kb),
            config_dir_override: None,
            data_dir_override: None,
            babel_confirm: true,
            babel_trust_paths: Vec::new(),
            babel_timeout: 30,
            babel_inherit_shell_env: true,
            babel_cxx_compiler: "c++".to_string(),
            babel_c_compiler: "cc".to_string(),
            babel_cxx_std: "c++17".to_string(),
            babel_sessions: crate::babel::session::SessionManager::new(),
            snippet_session: None,
            snippet_store: mae_snippets::SnippetStore::new(),
            format_config: mae_format::FormatConfig::new(),
            build_errors: Vec::new(),
            build_error_idx: 0,
            spell_results: HashMap::new(),
            format_on_save: false,
            spell_enabled: false,
            ai_chat_enabled: false,
            ai_guidance_kb: String::new(),
            ai: AiState::new(),
            bell_until: None,
            project: None,
            git_branch: None,
            recent_files: crate::project::RecentFiles::default(),
            recent_projects: crate::project::RecentProjects::default(),
            project_list: crate::project::ProjectList::default(),
            show_line_numbers: true,
            relative_line_numbers: false,
            word_wrap: false,
            break_indent: true,
            show_break: "↪ ".to_string(),
            fill_column: 80,
            org_hide_emphasis_markers: false,
            file_tree_window_id: None,
            file_tree_focus_on_open: true,
            file_tree_action: None,
            show_fps: false,
            renderer_name: "terminal".to_string(),
            gui_font_size: 14.0,
            gui_font_size_default: 14.0,
            gui_font_family: String::new(),
            gui_icon_font_family: String::new(),
            option_registry: OptionRegistry::new(),
            splash_selection: 0,
            debug_mode: false,
            debug_init: false,
            clean_mode: false,
            perf_stats: perf::PerfStats::default(),
            clipboard: "unnamed".to_string(),
            keymap_flavor: "doom".to_string(),
            kb_link_follow_mode: "kb-view".to_string(),
            default_mode: "normal".to_string(),
            restore_session: false,
            insert_ctrl_d: "dedent".to_string(),
            heading_scale: true,
            ignorecase: false,
            smartcase: false,
            scrolloff: 5,
            scrollbar: true,
            nyan_mode: false,
            mouse_autoselect_window: false,
            mouse_wheel_follow_mouse: true,
            scroll_speed: 3,
            completion_max_items: 10,
            code_action_max_items: 12,
            symbol_outline_max_items: 20,
            hover_max_lines: 15,
            popup_width_pct: 70,
            popup_height_pct: 60,
            scrollbar_width: 6.0,
            file_picker_max_depth: 12,
            file_picker_max_candidates: 50000,
            window_title: "MAE — Modern AI Editor".to_string(),
            heading_scale_h1: 1.5,
            heading_scale_h2: 1.3,
            heading_scale_h3: 1.15,
            link_descriptive: true,
            render_markup: true,
            inline_images: true,
            lsp_hover_popup: true,
            kb_preview_on_hover: true,
            kb_preview_max_lines: 15,
            blame_overlay: None,
            lsp_diagnostics_inline: true,
            lsp_diagnostics_virtual_text: true,
            lsp_completion: true,
            auto_complete: true,
            show_breadcrumbs: false,
            highlight_last_pos: None,
            heartbeat: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            watchdog_stall_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            watchdog_stall_recovery: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            event_recorder: crate::event_record::EventRecorder::new(),
            state_stack: Vec::new(),
            self_test_active: false,
            test_sandbox_dir: None,
            last_autosave: std::time::Instant::now(),
            autosave_interval: 0,
            swap_file: true,
            swap_directory: String::new(),
            buffer_keys_popup: false,
            display_policy: crate::display_policy::DisplayPolicy::default(),
            agent_display_split_ratio: 0.5,
            ai_conversation_split_ratio: 0.85,
            redraw_level: crate::redraw::RedrawLevel::Full,
            dirty_line_range: None,
            last_click: None,
            pending_rename_edit: None,
            gui_cell_width: 8.0,
            gui_cell_height: 16.0,
            replaceable_kinds: Vec::new(),
            large_file_lines: 5_000,
            degrade_threshold_chars: 500_000,
            degrade_threshold_line_length: 10_000,
            display_region_debounce_ms: 150,
            syntax_reparse_debounce_ms: 50,
            markup_cache: HashMap::new(),
            code_block_cache: HashMap::new(),
            org_agenda_files: Vec::new(),
            active_modules: Vec::new(),
            module_binding_warnings: Vec::new(),
            pending_module_reloads: Vec::new(),
            pending_pkg_commands: Vec::new(),
            pending_git_diff: None,
            locked_files: HashSet::new(),
            setup_all_pending: false,
            collab: CollabState::new(),
        }
    }

    /// Create an editor with a pre-built knowledge base (skipping `seed_kb()`).
    ///
    /// Used when the manual KB is loaded from a pre-built CozoDB file rather
    /// than generated at startup. The KB is seeded later with command/keymap
    /// nodes via `seed_dynamic_nodes()`.
    pub fn with_kb(kb: mae_kb::KnowledgeBase) -> Self {
        let commands = CommandRegistry::with_builtins();
        let keymaps = Self::default_keymaps();
        let hooks = HookRegistry::new();
        // Skip seed_kb() — KB already populated from persistent store.
        // Command/keymap nodes will be added by seed_dynamic_nodes() after
        // the editor is constructed and modules are loaded.
        Self::new_inner(commands, keymaps, hooks, kb)
    }

    pub fn with_buffer(buf: Buffer) -> Self {
        let buf_file_path_snapshot = buf.file_path().map(|p| p.to_path_buf());
        let syntax = {
            let mut m = crate::syntax::SyntaxMap::new();
            // If the buffer was opened with a file path, attach the
            // matching language immediately so the first render shows
            // syntax highlighting.
            if let Some(path) = buf_file_path_snapshot {
                if let Some(lang) = crate::syntax::language_for_path(&path) {
                    m.set_language(0, lang);
                }
            }
            m
        };
        Editor {
            buffers: vec![buf],
            splash_art: None,
            custom_splash_arts: Vec::new(),
            syntax,
            ..Self::new()
        }
    }

    /// Shutdown hook — called before `running = false`. Persists message log.
    pub fn on_quit(&mut self) {
        if !self.message_log.is_empty() {
            match self.save_message_log() {
                Ok(path) => {
                    // Log to message_log itself (won't be visible since we're quitting,
                    // but will appear in the saved file if written before the flush).
                    tracing::info!("Messages saved to {}", path.display());
                }
                Err(e) => {
                    tracing::warn!("Failed to save message log: {}", e);
                }
            }
        }
    }

    /// Replay a cursor operation at all secondary cursors (multi-cursor editing).
    pub fn mc_replay_op(&mut self, op: &crate::cursor::CursorOp) {
        multicursor::replay_at_secondaries(self, op);
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        let s = msg.into();
        // #305: only log a genuine change. Without this, a status re-set to
        // the SAME value on every render tick it remains "current" floods
        // `*Messages*` with dozens of identical entries for one real
        // transition. Consecutive-only: re-raising an earlier value after
        // something else was shown in between still logs (this is not a
        // global "have we ever seen this string" dedup).
        if !s.is_empty() && s != self.status_msg {
            self.message_log
                .push(crate::messages::MessageLevel::Info, "status", &s);
        }
        self.status_msg = s;
    }

    /// Trigger a visual bell — the renderer will briefly flash the status
    /// bar. Emacs `visible-bell` equivalent. Duration: 150ms.
    pub fn ring_bell(&mut self) {
        self.bell_until = Some(std::time::Instant::now() + std::time::Duration::from_millis(150));
    }

    /// Returns true if the visual bell is currently active.
    pub fn bell_active(&self) -> bool {
        self.bell_until
            .map(|t| std::time::Instant::now() < t)
            .unwrap_or(false)
    }

    /// Consume the count prefix, returning the count (default 1).
    pub fn take_count(&mut self) -> usize {
        self.vi.count_prefix.take().unwrap_or(1)
    }
}
