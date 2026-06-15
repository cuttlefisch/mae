mod agenda_ops;
pub mod ai_state;
mod babel_ops;
mod changes;
mod command;
mod dap_ops;
pub mod dap_state;
mod debug_panel_ops;
mod diagnostics;
pub mod dispatch;
mod edit_ops;
pub(crate) mod ex_parse;
mod file_ops;
mod git_ops;
mod heading_ops;
pub(crate) mod help_ops;
mod hook_ops;
mod jumps;
pub(crate) mod kb_ops;
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
mod option_ops;
mod org_ops;
pub mod perf;
mod project_ops;
mod register_ops;
mod scheme_ops;
mod search_ops;
mod surround;
mod syntax_ops;
mod table_ops;
mod text_objects;
pub mod vi_state;
mod visual;

pub use ai_state::AiState;
pub use changes::{ChangeEntry, CHANGE_LIST_CAP};
pub use dap_state::DapContext;
pub use diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticStore};
pub use help_ops::is_builtin_node;
pub use jumps::{JumpEntry, JUMP_LIST_CAP};
pub use kb_ops::KbWatcherStats;
pub use kb_state::KbContext;
pub use lsp_state::LspContext;
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
#[derive(Debug, Clone)]
pub enum CollabIntent {
    /// Start a local daemon process.
    StartServer,
    /// Connect to a remote daemon.
    Connect { address: String },
    /// Disconnect from the current server.
    Disconnect,
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
    /// Join a shared KB from the server.
    JoinKb { kb_id: String },
    /// Leave (unsubscribe from) a shared KB.
    LeaveKb { kb_id: String },
    /// Add a trusted peer to a KB's member list (owner-only, ADR-017).
    KbAddMember { kb_id: String, member: String },
    /// Remove a peer from a KB's member list (owner-only, ADR-017).
    KbRemoveMember { kb_id: String, member: String },
    /// Send a CRDT update for a KB node to the server.
    KbNodeUpdate {
        kb_id: String,
        node_id: String,
        update: Vec<u8>,
    },
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
    /// KB sync mode: "manual" (explicit :kb-sync), "on_save" (auto on node edit).
    pub kb_sync_mode: String,
    /// Pending KB node updates to send (accumulated between ticks).
    pub pending_kb_updates: Vec<(String, String, Vec<u8>)>, // (kb_id, node_id, update_bytes)
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
            auto_resolve_paths: false,
            default_save_dir: String::new(),
            save_on_remote_update: false,
            heartbeat_interval: 30,
            pending_save_committed: None,
            remote_users: mae_sync::awareness::AwarenessMap::new(),
            pending_awareness: None,
            last_awareness_sent: std::time::Instant::now(),
            shared_kbs: HashMap::new(),
            kb_sync_mode: KB_SYNC_MODE_DEFAULT.to_string(),
            pending_kb_updates: Vec::new(),
            psk: String::new(),
            psk_command: String::new(),
            auth_mode: "psk".to_string(),
            host_key_policy: "prompt".to_string(),
            tls: true,
        }
    }
}

impl Default for CollabState {
    fn default() -> Self {
        Self::new()
    }
}

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
use crate::keymap::{KeyPress, Keymap, WhichKeyEntry};
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

/// LSP server connection status, tracked per language_id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspServerStatus {
    Starting,
    Connected,
    Failed,
    Exited,
}

/// Rich LSP server info — status plus discovery metadata.
#[derive(Debug, Clone)]
pub struct LspServerInfo {
    /// Current connection status.
    pub status: LspServerStatus,
    /// The command used to start this server (e.g. "rust-analyzer").
    pub command: String,
    /// Whether the binary was found on PATH at startup.
    pub binary_found: bool,
}

/// A single item in the LSP completion popup.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Display label shown in the popup.
    pub label: String,
    /// Text to insert when accepted (falls back to `label`).
    pub insert_text: String,
    /// Brief detail (e.g. type signature).
    pub detail: Option<String>,
    /// Single-char sigil for the kind (f=function, v=variable, t=type, …).
    pub kind_sigil: char,
}

/// Floating popup showing LSP hover info near the cursor.
#[derive(Debug, Clone)]
pub struct HoverPopup {
    /// Raw markdown from LSP.
    pub contents: String,
    /// Buffer index where hover was requested.
    pub buffer_idx: usize,
    /// Buffer row where K was pressed.
    pub anchor_row: usize,
    /// Buffer col where K was pressed.
    pub anchor_col: usize,
    /// Scroll offset for long content.
    pub scroll_offset: usize,
}

/// Floating popup showing LSP signature help near the cursor.
#[derive(Debug, Clone)]
pub struct SignatureHelpState {
    /// Signatures from LSP.
    pub signatures: Vec<SignatureHelpInfo>,
    /// Which signature is active.
    pub active_signature: usize,
    /// Which parameter is active (highlighted).
    pub active_parameter: usize,
    /// Anchor position where the call started.
    pub anchor_line: usize,
    pub anchor_col: usize,
}

/// A single signature for display.
#[derive(Debug, Clone)]
pub struct SignatureHelpInfo {
    /// Full signature label (e.g. "fn foo(x: i32, y: &str) -> bool").
    pub label: String,
    /// Parameter byte offset ranges in `label`.
    pub parameters: Vec<(usize, usize)>,
    /// Documentation for this signature.
    pub documentation: Option<String>,
}

/// Inline preview of a definition without navigating away.
#[derive(Debug, Clone)]
pub struct PeekState {
    /// File path of the definition.
    pub file_path: String,
    /// Line number of the definition (0-indexed).
    pub line: usize,
    /// Column of the definition.
    pub col: usize,
    /// Context lines around the definition.
    pub context_lines: Vec<String>,
    /// Which line in context_lines is the definition itself.
    pub highlight_line: usize,
    /// Scroll offset within the peek window.
    pub scroll_offset: usize,
}

/// Per-line blame annotation from `git blame`.
#[derive(Debug, Clone)]
pub struct BlameEntry {
    /// Short commit hash (8 chars).
    pub commit_hash: String,
    /// Author name.
    pub author: String,
    /// Unix timestamp.
    pub timestamp: i64,
    /// First line of commit message.
    pub summary: String,
    /// 0-indexed line in buffer.
    pub final_line: usize,
}

/// Blame overlay for the active buffer.
#[derive(Debug, Clone)]
pub struct BlameOverlay {
    /// Which buffer this blame is for.
    pub buffer_idx: usize,
    /// Blame entries, one per line.
    pub entries: Vec<BlameEntry>,
}

/// A single item in the LSP code action popup menu.
#[derive(Debug, Clone)]
pub struct CodeActionItem {
    /// Display title of the code action.
    pub title: String,
    /// The kind of the code action (e.g. "quickfix", "refactor").
    pub kind: Option<String>,
    /// JSON-serialized WorkspaceEdit to apply when selected.
    pub edit_json: Option<String>,
}

/// Code action popup menu shown after `SPC c a`.
#[derive(Debug, Clone)]
pub struct CodeActionMenu {
    pub items: Vec<CodeActionItem>,
    pub selected: usize,
}

/// A single entry in the symbol outline popup.
#[derive(Debug, Clone)]
pub struct SymbolOutlineEntry {
    pub name: String,
    /// Human-readable kind (e.g. "function", "struct").
    pub kind: String,
    /// Single-char icon for the kind.
    pub kind_icon: char,
    /// Line number (0-based) of the symbol.
    pub line: usize,
    /// Nesting depth (0 = top-level).
    pub depth: usize,
    /// Optional detail (e.g. type signature).
    pub detail: Option<String>,
}

/// Symbol outline popup state (SPC c o).
#[derive(Debug, Clone)]
pub struct SymbolOutlineState {
    pub entries: Vec<SymbolOutlineEntry>,
    pub selected: usize,
    pub filter: String,
    pub filtered_indices: Vec<usize>,
}

/// Peek references state — cycling through reference locations inline.
#[derive(Debug, Clone)]
pub struct PeekReferencesState {
    /// All reference locations.
    pub locations: Vec<PeekReferenceLocation>,
    /// Currently shown index.
    pub current: usize,
}

/// A single reference location for peek.
#[derive(Debug, Clone)]
pub struct PeekReferenceLocation {
    /// File path.
    pub path: String,
    /// Line number (0-indexed).
    pub line: usize,
    /// Column (0-indexed).
    pub col: usize,
    /// Context lines around the reference.
    pub context: Vec<String>,
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

/// Input lock scope — controls what keyboard input is allowed during AI/MCP operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputLock {
    /// No lock — all input accepted normally.
    None,
    /// AI session active — block editor commands but allow shell input and navigation.
    AiBusy,
    /// MCP tool executing — block editor commands but allow shell input and navigation.
    McpBusy,
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

/// Snapshot of editor state for save/restore (push/pop state stack).
/// Captures the buffer list, window layout, focus, and mode so tools
/// Pending async git diff: spawned on a background thread, polled on idle ticks.
pub struct PendingGitDiff {
    pub file_path: std::path::PathBuf,
    pub receiver: std::sync::mpsc::Receiver<
        std::collections::HashMap<usize, crate::render_common::gutter::GitLineStatus>,
    >,
}

/// can restore the editor to a known state after temporary operations.
#[derive(Clone)]
pub struct EditorStateSnapshot {
    /// Buffer names that were open (ordered).
    pub buffer_names: Vec<String>,
    /// The focused window's buffer name.
    pub focused_buffer: String,
    /// Cloned window manager state: all windows + layout tree + focus.
    pub windows: std::collections::HashMap<crate::window::WindowId, crate::window::Window>,
    pub layout: crate::window::LayoutNode,
    pub focused_id: crate::window::WindowId,
    pub next_window_id: crate::window::WindowId,
    /// Editor mode at snapshot time.
    pub mode: Mode,
    /// Conversation pair (AI split layout) at snapshot time.
    pub conversation_pair: Option<ConversationPair>,
}

/// Network connectivity check result (lightweight copy for display/reporting).
#[derive(Debug, Clone)]
pub struct AiNetworkCheck {
    pub endpoint: String,
    pub reachable: bool,
    pub http_status: Option<u16>,
    pub latency_ms: u64,
    pub error: Option<String>,
}

// @ai-caution: [dispatch] ~40 fields after ViState (41) + AiState (34) + CollabState (18) + ShellIntents (12) extraction.
// Before adding fields, check if the state belongs in a sub-struct
// (LspContext, DapContext, KbContext). See ROADMAP.md architecture debt.
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
    /// In-editor message log (*Messages* buffer equivalent).
    /// Shared with the tracing layer via MessageLogHandle.
    pub message_log: MessageLog,
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
    /// Show hover info in a floating popup (true) or status bar (false). Default true.
    pub lsp_hover_popup: bool,
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
            running: true,
            status_msg: String::new(),
            current_command: String::new(),
            commands,
            keymaps,
            keymap_registry: crate::keymap_registry::KeymapRegistry::kernel_defaults(),
            which_key_prefix: Vec::new(),
            which_key_scroll: 0,
            message_log: MessageLog::new(1000), // Max message log entries (internal bound)
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
            babel_sessions: crate::babel::session::SessionManager::new(),
            snippet_session: None,
            snippet_store: mae_snippets::SnippetStore::new(),
            format_config: mae_format::FormatConfig::new(),
            build_errors: Vec::new(),
            build_error_idx: 0,
            spell_results: HashMap::new(),
            format_on_save: false,
            spell_enabled: false,
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
            lsp_hover_popup: true,
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

    /// Returns the primary keymap name and optional fallback for the current mode.
    /// Buffer-kind overlays (git-status, file-tree, help, debug) and language
    /// overlays (org, markdown) sit on top of "normal" — if the overlay has no
    /// match, the caller should retry with the fallback.
    pub fn current_keymap_names(&self) -> Option<(&str, Option<&str>)> {
        // Transient keypad/leader layer overrides mode-based keymap selection:
        // while active, keys resolve against the shared `leader` keymap (the mae
        // which-key tree), regardless of the underlying mode (Normal for the doom
        // flavor, Insert for the non-modal flavor). See `Editor::leader_active`.
        if self.leader_active {
            return Some(("leader", None));
        }

        let idx = self.active_buffer_idx();
        let kind = self.buffers[idx].kind;
        let lang = self.syntax.language_of(idx);

        match self.mode {
            Mode::Normal => {
                // Context keymap from the data-driven registry: buffer kind first
                // (git-status, file-tree, navigation, …), then language overlay
                // (org/markdown). Both fall back to "normal". No hardcoded match —
                // a module can route a new kind/language without a kernel patch.
                if let Some(km_name) = self.keymap_registry.context_for_kind(kind) {
                    Some((km_name, Some("normal")))
                } else if let Some(km_name) =
                    lang.and_then(|l| self.keymap_registry.context_for_language(l))
                {
                    Some((km_name, Some("normal")))
                } else {
                    Some(("normal", None))
                }
            }
            Mode::Insert => Some(("insert", None)),
            Mode::Visual(_) => Some(("visual", None)),
            Mode::Command
            | Mode::ConversationInput
            | Mode::Search
            | Mode::FilePicker
            | Mode::FileBrowser
            | Mode::CommandPalette => Some(("command", None)),
            Mode::ShellInsert => None,
        }
    }

    /// Get the keymap for the current mode.
    pub fn current_keymap(&self) -> Option<&Keymap> {
        let (name, _) = self.current_keymap_names()?;
        self.keymaps.get(name)
    }

    /// The ordered keymap resolution chain for the current focus, most-specific
    /// layer first.
    ///
    /// This is the SINGLE source of truth consumed by keystroke dispatch
    /// (`handle_keymap_mode`), the which-key popup (`merged_which_key_entries`),
    /// and `describe-bindings`. Routing all three through one chain makes the
    /// keymap a key *resolves against* and the keymap the UI *shows* incapable of
    /// diverging — previously dispatch used a flat `(primary, fallback)` pair
    /// while `describe-bindings` walked the `parent` chain N levels, so a 3-deep
    /// chain (e.g. `git-log → git-status → normal`) would dispatch and display
    /// differently.
    ///
    /// The chain is `current_keymap_names()`'s primary keymap plus its `parent`
    /// ancestry, followed by the fallback plus its ancestry (deduped, cycle-safe).
    /// For the current 2-deep keymaps this reproduces the old behavior exactly.
    /// Empty when there is no keymap (ShellInsert — keys go straight to the PTY).
    ///
    /// Phase 0 derives the chain from the existing `current_keymap_names()` match;
    /// a later phase replaces the source with the data-driven keymap registry
    /// without changing any consumer.
    pub fn keymap_chain(&self) -> Vec<String> {
        let Some((primary, fallback)) = self.current_keymap_names() else {
            return Vec::new();
        };
        let mut chain: Vec<String> = Vec::new();
        self.extend_keymap_chain(primary, &mut chain);
        if let Some(fb) = fallback {
            self.extend_keymap_chain(fb, &mut chain);
        }
        chain
    }

    /// Append `start` and its `parent` ancestry to `chain`, skipping any name
    /// already present (dedupe + cycle guard).
    fn extend_keymap_chain(&self, start: &str, chain: &mut Vec<String>) {
        let mut cur = Some(start.to_string());
        while let Some(name) = cur.take() {
            if chain.iter().any(|n| n == &name) {
                break;
            }
            cur = self.keymaps.get(&name).and_then(|km| km.parent.clone());
            chain.push(name);
        }
    }

    /// Reset all keymaps to the fresh kernel defaults (vi-modal primitives only,
    /// no leader tree). Used by runtime keymap-flavor switching: reset to a clean
    /// slate, then re-run module loading to apply the new flavor — avoids stale
    /// bindings from the previous flavor (the `leader`/`insert` entries differ).
    pub fn reset_keymaps_to_kernel(&mut self) {
        self.keymaps = Self::default_keymaps();
        // Re-seed the context routing to the kernel baseline too; module
        // registrations (e.g. a "navigation" context, canvas artifact) re-apply
        // on the subsequent module reload, exactly like the keymaps themselves.
        self.keymap_registry = crate::keymap_registry::KeymapRegistry::kernel_defaults();
        self.leader_active = false;
        self.clear_which_key_prefix();
    }

    /// Look up a key binding by key string (e.g. "SPC n d t").
    /// Returns (command_name, keymap_name) if found.
    pub fn lookup_key_binding(&self, key_str: &str) -> Option<(String, String)> {
        let seq = crate::keymap::parse_key_seq_spaced(key_str);
        if seq.is_empty() {
            return None;
        }
        for (name, km) in &self.keymaps {
            for (bound_seq, cmd) in km.bindings() {
                if *bound_seq == seq {
                    return Some((cmd.clone(), name.clone()));
                }
            }
        }
        None
    }

    /// Query keybindings across all keymaps with optional filters.
    /// Returns vec of (key_display, command, keymap_name).
    pub fn query_keybindings(
        &self,
        keymap_filter: Option<&str>,
        command_filter: Option<&str>,
        prefix_filter: Option<&str>,
    ) -> Vec<(String, String, String)> {
        let prefix_seq = prefix_filter.map(crate::keymap::parse_key_seq_spaced);
        let mut results = Vec::new();
        for (name, km) in &self.keymaps {
            if let Some(filter) = keymap_filter {
                if name != filter {
                    continue;
                }
            }
            for (seq, cmd) in km.bindings() {
                if let Some(ref cmd_filter) = command_filter {
                    if !cmd.contains(cmd_filter) {
                        continue;
                    }
                }
                if let Some(ref prefix) = prefix_seq {
                    if seq.len() < prefix.len() || &seq[..prefix.len()] != prefix.as_slice() {
                        continue;
                    }
                }
                let key_display = crate::keymap::format_key_seq(seq);
                results.push((key_display, cmd.clone(), name.clone()));
            }
        }
        results.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
        results
    }

    /// Merge which-key entries across the full resolution chain (most-specific
    /// layer first; a more-specific layer's binding for a key shadows a deeper
    /// one). Uses the same `keymap_chain()` as dispatch so the popup can't show a
    /// key the dispatcher wouldn't run.
    fn merged_which_key_entries(&self, prefix: &[KeyPress]) -> Vec<WhichKeyEntry> {
        let mut entries: Vec<WhichKeyEntry> = Vec::new();
        let mut existing: std::collections::HashSet<String> = std::collections::HashSet::new();
        for km_name in self.keymap_chain() {
            let Some(km) = self.keymaps.get(&km_name) else {
                continue;
            };
            for entry in km.which_key_entries(prefix, &self.commands) {
                if existing.insert(format!("{:?}", entry.key)) {
                    entries.push(entry);
                }
            }
        }
        entries
    }

    /// Get which-key entries for the current keymap, merging overlay + parent.
    /// Applies the `which-key-sort-order` option: groups first, then sorted.
    pub fn which_key_entries_for_current_keymap(&self) -> Vec<WhichKeyEntry> {
        let mut entries = self.merged_which_key_entries(&self.which_key_prefix);
        self.sort_which_key_entries(&mut entries);
        entries
    }

    /// Get all top-level bindings for the current buffer's keymap + parent.
    /// Used by `show-buffer-keys` (`?`) to show a full keybind reference.
    pub fn buffer_keys_entries(&self) -> Vec<WhichKeyEntry> {
        let mut entries = self.merged_which_key_entries(&[]);
        self.sort_which_key_entries(&mut entries);
        entries
    }

    /// Sort which-key entries: groups first (sorted by key), then leaves
    /// sorted by the chosen field (`key`, `desc`, or `none`).
    fn sort_which_key_entries(&self, entries: &mut [WhichKeyEntry]) {
        let order = self
            .get_option("which-key-sort-order")
            .map(|(v, _)| v)
            .unwrap_or_else(|| "key".to_string());
        match order.as_str() {
            "desc" => {
                entries.sort_by(|a, b| {
                    b.is_group
                        .cmp(&a.is_group)
                        .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
                });
            }
            "none" => {} // insertion order
            _ => {
                // "key" (default): groups first, then alphabetical by key
                entries.sort_by(|a, b| {
                    b.is_group.cmp(&a.is_group).then_with(|| {
                        let ak = crate::text_utils::format_keypress(&a.key);
                        let bk = crate::text_utils::format_keypress(&b.key);
                        ak.cmp(&bk)
                    })
                });
            }
        }
    }

    /// Set the which-key prefix and reset scroll to top.
    /// Use this instead of assigning `which_key_prefix` directly.
    pub fn set_which_key_prefix(&mut self, prefix: Vec<KeyPress>) {
        self.which_key_prefix = prefix;
        self.which_key_scroll = 0;
    }

    /// Clear the which-key prefix and reset scroll.
    pub fn clear_which_key_prefix(&mut self) {
        self.which_key_prefix.clear();
        self.which_key_scroll = 0;
    }

    // -- Redraw level methods (Emacs tiered redisplay pattern) ----------------

    /// Mark that only the cursor moved — reuse cached syntax spans.
    pub fn mark_cursor_moved(&mut self) {
        self.redraw_level = self
            .redraw_level
            .max(crate::redraw::RedrawLevel::CursorOnly);
    }

    /// Mark that the viewport scrolled.
    pub fn mark_scrolled(&mut self) {
        self.redraw_level = self.redraw_level.max(crate::redraw::RedrawLevel::Scroll);
    }

    /// Mark specific lines as dirty, merging with any existing dirty range.
    pub fn mark_lines_dirty(&mut self, start: usize, end: usize) {
        self.redraw_level = self
            .redraw_level
            .max(crate::redraw::RedrawLevel::PartialLines);
        self.dirty_line_range = Some(match self.dirty_line_range {
            Some((old_start, old_end)) => (old_start.min(start), old_end.max(end)),
            None => (start, end),
        });
    }

    /// Mark that a full redraw is needed (theme, resize, mode change, etc.).
    pub fn mark_full_redraw(&mut self) {
        self.redraw_level = crate::redraw::RedrawLevel::Full;
    }

    /// Reset redraw level after rendering. Called by the event loop after `render()`.
    pub fn clear_redraw(&mut self) {
        self.redraw_level = crate::redraw::RedrawLevel::None;
        self.dirty_line_range = None;
    }

    /// Returns the active buffer's project root, falling back to the editor-wide project root.
    pub fn active_project_root(&self) -> Option<&std::path::Path> {
        let buf = self.active_buffer();
        if let Some(root) = &buf.project_root {
            return Some(root.as_path());
        }
        self.project.as_ref().map(|p| p.root.as_path())
    }

    /// Returns the git repository root, falling back to the project root.
    /// Walks up from the current project root looking for `.git`.
    /// This gives the VCS-level root rather than a subcrate Cargo.toml directory.
    pub fn git_or_project_root(&self) -> Option<std::path::PathBuf> {
        let start = self
            .project
            .as_ref()
            .map(|p| p.root.as_path())
            .or_else(|| self.active_buffer().project_root.as_deref())?;
        let mut dir = start.to_path_buf();
        loop {
            if dir.join(".git").exists() {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
        Some(start.to_path_buf())
    }

    // -- Per-buffer option accessors (Emacs buffer-local / Vim setlocal) ------
    // Check the active buffer's local override first, then fall back to the
    // global Editor default.  Use these instead of reading `self.word_wrap`
    // etc. directly when the result should be buffer-sensitive.

    /// Effective word-wrap for a specific buffer index.
    pub fn word_wrap_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .word_wrap
            .unwrap_or(self.word_wrap)
    }

    /// Effective word-wrap for the currently focused buffer.
    pub fn effective_word_wrap(&self) -> bool {
        self.word_wrap_for(self.active_buffer_idx())
    }

    /// Effective show_line_numbers for a specific buffer index.
    pub fn line_numbers_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .line_numbers
            .unwrap_or(self.show_line_numbers)
    }

    /// Effective relative_line_numbers for a specific buffer index.
    pub fn relative_line_numbers_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .relative_line_numbers
            .unwrap_or(self.relative_line_numbers)
    }

    /// Effective break_indent for a specific buffer index.
    pub fn break_indent_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .break_indent
            .unwrap_or(self.break_indent)
    }

    /// Effective show_break for a specific buffer index.
    pub fn show_break_for(&self, buf_idx: usize) -> &str {
        self.buffers[buf_idx]
            .local_options
            .show_break
            .as_deref()
            .unwrap_or(&self.show_break)
    }

    /// Effective heading_scale for a specific buffer index.
    pub fn heading_scale_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .heading_scale
            .unwrap_or(self.heading_scale)
    }

    /// Effective link_descriptive for a specific buffer index.
    pub fn link_descriptive_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .link_descriptive
            .unwrap_or(self.link_descriptive)
    }

    /// Effective render_markup for a specific buffer index.
    pub fn render_markup_for(&self, buf_idx: usize) -> bool {
        self.buffers[buf_idx]
            .local_options
            .render_markup
            .unwrap_or(self.render_markup)
    }

    /// Resolve the effective markup flavor for a buffer, respecting the
    /// priority chain: BufferMode → Language → None, gated by render_markup.
    pub fn effective_markup_flavor(&self, buf_idx: usize) -> crate::syntax::MarkupFlavor {
        use crate::buffer_mode::BufferMode;
        if !self.render_markup_for(buf_idx) {
            return crate::syntax::MarkupFlavor::None;
        }
        let buf = &self.buffers[buf_idx];
        if let Some(flavor) = buf.kind.markup_flavor() {
            return flavor;
        }
        if let Some(lang) = self.syntax.language_of(buf_idx) {
            return lang.markup_flavor();
        }
        crate::syntax::MarkupFlavor::None
    }

    /// Detect whether a buffer is too large for full feature rendering.
    /// Returns true for files exceeding `degrade_threshold_chars` or any line
    /// exceeding `degrade_threshold_line_length` (both user-configurable).
    /// Callers should skip markup spans, display regions, code block
    /// detection, and heading scale for such buffers (Emacs `so-long` pattern).
    ///
    /// Result is cached per buffer (`buffer.degraded`). The cache is set on
    /// first access and on file open — degradation status is monotonic during
    /// normal editing so re-scanning every frame is unnecessary.
    pub fn should_degrade_features(&self, buf_idx: usize) -> bool {
        if buf_idx >= self.buffers.len() {
            return false;
        }
        if let Some(cached) = self.buffers[buf_idx].degraded {
            return cached;
        }
        let buf = &self.buffers[buf_idx];
        let rope = buf.rope();
        if rope.len_chars() > self.degrade_threshold_chars {
            return true;
        }
        // Sample first 200 lines + last 50 for long-line detection (avoid O(n) full scan).
        let lc = rope.len_lines();
        let check_lines = (0..200.min(lc)).chain(lc.saturating_sub(50)..lc);
        for li in check_lines {
            let line = rope.line(li);
            if line.len_chars() > self.degrade_threshold_line_length {
                return true;
            }
        }
        false
    }

    /// Compute and cache the degradation status for a buffer.
    pub fn cache_degraded(&mut self, buf_idx: usize) {
        let degraded = self.should_degrade_features(buf_idx);
        self.buffers[buf_idx].degraded = Some(degraded);
    }

    /// Get or compute cached markup spans for a buffer. Returns empty if
    /// flavor is None. The cache is keyed by buffer generation so editing
    /// invalidates it but pure scrolling reuses cached spans.
    pub fn get_or_compute_markup_spans(
        &mut self,
        buf_idx: usize,
        flavor: crate::syntax::MarkupFlavor,
    ) -> Vec<crate::syntax::HighlightSpan> {
        if flavor == crate::syntax::MarkupFlavor::None {
            return Vec::new();
        }
        let gen = self.buffers[buf_idx].generation;
        if let Some(cached) = self.markup_cache.get(&buf_idx) {
            if cached.generation == gen && cached.flavor == flavor {
                return cached.spans.clone();
            }
        }
        let rope = self.buffers[buf_idx].rope();
        let line_count = rope.len_lines();
        let source: String = rope.chars().collect();
        let spans = crate::syntax::compute_markup_spans(&source, flavor);
        self.markup_cache.insert(
            buf_idx,
            crate::syntax::MarkupCache {
                generation: gen,
                flavor,
                line_start: 0,
                line_end: line_count,
                byte_offset: 0,
                spans: spans.clone(),
            },
        );
        spans
    }

    /// Clamp all window cursors to their buffer bounds. Safety net against
    /// stale cursor positions after buffer mutations (MCP tools, AI edits).
    /// Also clamps visual anchors and last_visual so rendering never panics.
    pub fn clamp_all_cursors(&mut self) {
        for win in self.window_mgr.iter_windows_mut() {
            let buf_idx = win.buffer_idx;
            if buf_idx < self.buffers.len() {
                win.clamp_cursor(&self.buffers[buf_idx]);
            }
        }

        // Clamp visual anchor to focused buffer bounds.
        let idx = self.active_buffer_idx();
        let line_count = self.buffers[idx].display_line_count();
        if line_count == 0 {
            self.vi.visual_anchor_row = 0;
            self.vi.visual_anchor_col = 0;
        } else {
            let max_row = line_count.saturating_sub(1);
            if self.vi.visual_anchor_row > max_row {
                self.vi.visual_anchor_row = max_row;
            }
            let max_col = self.buffers[idx].line_len(self.vi.visual_anchor_row);
            if self.vi.visual_anchor_col > max_col {
                self.vi.visual_anchor_col = max_col;
            }
        }

        // Clamp last_visual so `gv` reselect never panics.
        if let Some((ref mut ar, ref mut ac, ref mut cr, ref mut cc, _)) = self.vi.last_visual {
            if line_count == 0 {
                *ar = 0;
                *ac = 0;
                *cr = 0;
                *cc = 0;
            } else {
                let max_row = line_count.saturating_sub(1);
                if *ar > max_row {
                    *ar = max_row;
                }
                *ac = (*ac).min(self.buffers[idx].line_len(*ar));
                if *cr > max_row {
                    *cr = max_row;
                }
                *cc = (*cc).min(self.buffers[idx].line_len(*cr));
            }
        }
    }

    /// Insert a dashboard buffer at position 0 and focus it.
    /// Call this at application startup (before opening files) to get a
    /// Doom-style splash screen. The existing scratch buffer shifts to index 1.
    pub fn install_dashboard(&mut self) {
        self.buffers.insert(0, Buffer::new_dashboard());
        // Fix up window buffer indices — they all shift right by 1.
        for win in self.window_mgr.iter_windows_mut() {
            win.buffer_idx += 1;
        }
        if let Some(alt) = self.vi.alternate_buffer_idx.as_mut() {
            *alt += 1;
        }
        // Focus the dashboard.
        self.window_mgr.focused_window_mut().buffer_idx = 0;
    }

    /// Convenience: index of the active (focused window's) buffer.
    pub fn active_buffer_idx(&self) -> usize {
        self.window_mgr.focused_window().buffer_idx
    }

    /// AI-aware buffer index: returns `ai_target_buffer_idx` if set,
    /// otherwise falls back to `active_buffer_idx()`.
    pub fn ai_active_buffer_idx(&self) -> usize {
        self.ai
            .target_buffer_idx
            .unwrap_or_else(|| self.active_buffer_idx())
    }

    /// AI-aware cursor row: reads cursor from the AI target window if set,
    /// otherwise from the focused window.
    pub fn ai_cursor_row(&self) -> usize {
        if let Some(win_id) = self.ai.target_window_id {
            if let Some(win) = self.window_mgr.iter_windows().find(|w| w.id == win_id) {
                return win.cursor_row;
            }
        }
        self.window_mgr.focused_window().cursor_row
    }

    pub fn active_buffer(&self) -> &Buffer {
        let idx = self.active_buffer_idx();
        assert!(
            idx < self.buffers.len(),
            "buffer_idx {} out of range ({})",
            idx,
            self.buffers.len()
        );
        &self.buffers[idx]
    }

    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        let idx = self.active_buffer_idx();
        assert!(
            idx < self.buffers.len(),
            "buffer_idx {} out of range ({})",
            idx,
            self.buffers.len()
        );
        &mut self.buffers[idx]
    }

    /// Per-window viewport height from cached layout. Falls back to global.
    pub fn window_viewport_height(&self, win_id: WindowId) -> usize {
        if self.last_layout_area.width > 0 && self.last_layout_area.height > 0 {
            let rects = self.window_mgr.layout_rects(self.last_layout_area);
            for (id, rect) in &rects {
                if *id == win_id && rect.height >= 3 {
                    return (rect.height as usize).saturating_sub(2); // status + border
                }
            }
        }
        self.viewport_height // fallback (startup, tests, zero-area)
    }

    /// Focused window's viewport height (convenience).
    pub fn focused_viewport_height(&self) -> usize {
        self.window_viewport_height(self.window_mgr.focused_id())
    }

    /// Save current editor state (buffer list, window layout, focus, mode)
    /// onto the state stack. Returns the stack depth after push.
    pub fn save_state(&mut self) -> usize {
        let buffer_names: Vec<String> = self.buffers.iter().map(|b| b.name.clone()).collect();
        let focused_buffer = self.active_buffer().name.clone();
        let (windows, layout, focused_id, next_id) = self.window_mgr.snapshot();
        self.state_stack.push(EditorStateSnapshot {
            buffer_names,
            focused_buffer,
            windows,
            layout,
            focused_id,
            next_window_id: next_id,
            mode: self.mode,
            conversation_pair: self.ai.conversation_pair.clone(),
        });
        self.state_stack.len()
    }

    /// Restore editor state from the state stack. Closes buffers that weren't
    /// in the snapshot, restores window layout and focus. Returns a summary
    /// of what was restored, or an error if the stack is empty.
    pub fn restore_state(&mut self) -> Result<String, String> {
        let snapshot = self
            .state_stack
            .pop()
            .ok_or_else(|| "State stack is empty — nothing to restore".to_string())?;

        // 1. Close buffers that weren't in the snapshot (reverse order to keep indices stable)
        let mut closed = Vec::new();
        let mut i = self.buffers.len();
        while i > 0 {
            i -= 1;
            if !snapshot.buffer_names.contains(&self.buffers[i].name) {
                closed.push(self.buffers[i].name.clone());
                self.buffers.remove(i);
                self.notify_buffer_removed(i);
            }
        }

        // 2. Remap window buffer_idx values: snapshot had indices into the old buffer list,
        //    but buffers may have shifted. Remap by name.
        let mut restored_windows = snapshot.windows;
        for win in restored_windows.values_mut() {
            // Find the buffer name this window was pointing to
            let old_name = snapshot
                .buffer_names
                .get(win.buffer_idx)
                .cloned()
                .unwrap_or_default();
            // Find new index for that buffer
            if let Some(new_idx) = self.buffers.iter().position(|b| b.name == old_name) {
                win.buffer_idx = new_idx;
            } else {
                // Buffer no longer exists — point to buffer 0
                win.buffer_idx = 0;
            }
        }

        // 3. Restore window manager
        self.window_mgr.restore(
            restored_windows,
            snapshot.layout,
            snapshot.focused_id,
            snapshot.next_window_id,
        );

        // 4. Restore mode
        self.mode = snapshot.mode;

        // 5. Restore conversation pair with remapped buffer indices.
        if let Some(mut pair) = snapshot.conversation_pair {
            let out_name = snapshot
                .buffer_names
                .get(pair.output_buffer_idx)
                .cloned()
                .unwrap_or_default();
            let in_name = snapshot
                .buffer_names
                .get(pair.input_buffer_idx)
                .cloned()
                .unwrap_or_default();
            let out_ok = self.buffers.iter().position(|b| b.name == out_name);
            let in_ok = self.buffers.iter().position(|b| b.name == in_name);
            if let (Some(out_idx), Some(in_idx)) = (out_ok, in_ok) {
                pair.output_buffer_idx = out_idx;
                pair.input_buffer_idx = in_idx;
                self.ai.conversation_pair = Some(pair);
            } else {
                self.ai.conversation_pair = None;
            }
        } else {
            self.ai.conversation_pair = None;
        }

        // 6. Focus the originally focused buffer
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.name == snapshot.focused_buffer)
        {
            self.window_mgr.focused_window_mut().buffer_idx = idx;
        }

        let summary = if closed.is_empty() {
            "State restored (no buffers closed)".to_string()
        } else {
            format!(
                "State restored, closed {} buffer(s): {}",
                closed.len(),
                closed.join(", ")
            )
        };
        Ok(summary)
    }

    /// Find a buffer index by name. Returns None if not found.
    pub fn find_buffer_by_name(&self, name: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.name == name)
    }

    /// Find a buffer by its collaborative document ID.
    /// Falls back to `find_buffer_by_name` if no buffer has a matching `collab_doc_id`.
    pub fn find_buffer_by_collab_doc_id(&self, doc_id: &str) -> Option<usize> {
        self.buffers
            .iter()
            .position(|b| b.collab_doc_id.as_deref() == Some(doc_id))
            .or_else(|| self.find_buffer_by_name(doc_id))
    }

    /// Find a buffer by name, or create it with the provided closure.
    /// Returns the buffer index.
    pub fn find_or_create_buffer(&mut self, name: &str, create: impl FnOnce() -> Buffer) -> usize {
        if let Some(idx) = self.find_buffer_by_name(name) {
            idx
        } else {
            self.buffers.push(create());
            self.buffers.len() - 1
        }
    }

    /// Open a command palette popup and switch to CommandPalette mode.
    pub fn open_palette(&mut self, palette: CommandPalette) {
        self.command_palette = Some(palette);
        self.set_mode(Mode::CommandPalette);
    }

    /// First conversation attached to any buffer, if any.
    pub fn conversation(&self) -> Option<&crate::conversation::Conversation> {
        self.buffers.iter().find_map(|b| b.conversation())
    }

    /// Mutable view of the first conversation attached to any buffer.
    pub fn conversation_mut(&mut self) -> Option<&mut crate::conversation::Conversation> {
        self.buffers.iter_mut().find_map(|b| b.conversation_mut())
    }

    /// Set the editor mode and fire the `mode-change` hook.
    /// Returns `true` if the mode was changed, `false` if blocked or already in that mode.
    pub fn set_mode(&mut self, mode: Mode) -> bool {
        // Block non-Normal modes for buffers that only allow Normal mode
        // (e.g. Dashboard, Modules).
        if mode != Mode::Normal
            && mode != Mode::Command
            && mode != Mode::Search
            && mode != Mode::CommandPalette
            && mode != Mode::FilePicker
            && mode != Mode::FileBrowser
        {
            use crate::BufferMode;
            if self.active_buffer().kind.normal_mode_only() {
                tracing::debug!(
                    requested = ?mode,
                    buffer = %self.active_buffer().name,
                    kind = ?self.active_buffer().kind,
                    "set_mode blocked: buffer is normal_mode_only"
                );
                return false;
            }
        }
        if self.mode != mode {
            self.mode = mode;
            self.fire_hook("mode-change");
            true
        } else {
            false
        }
    }

    /// Sync the rope of the first conversation buffer.
    /// Only escalates to `Full` redraw when the rope content actually changed,
    /// avoiding unnecessary syntax recomputation on no-op AI events.
    pub fn sync_conversation_buffer_rope(&mut self) {
        if let Some(buf) = self
            .buffers
            .iter_mut()
            .find(|b| b.kind == crate::buffer::BufferKind::Conversation)
        {
            if buf.sync_conversation_rope() {
                self.mark_full_redraw();
            }
        }
    }

    /// Index of the conversation buffer, creating `*AI*` if none exists.
    /// Used by both interactive open and programmatic load to keep the
    /// "find or create by kind" logic in one place.
    pub(crate) fn ensure_conversation_buffer_idx(&mut self) -> usize {
        if let Some(i) = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Conversation)
        {
            return i;
        }
        self.buffers.push(Buffer::new_conversation("*AI*"));
        self.buffers.len() - 1
    }

    /// Find or create the appropriate KB buffer (`*Help*` for builtins,
    /// `*KB*` for user/federated nodes) and navigate it to `node_id`.
    /// Returns the buffer index. Does NOT switch focus — callers decide.
    pub fn ensure_kb_buffer_idx(&mut self, node_id: &str) -> usize {
        use crate::buffer::buffer_names;
        use crate::editor::help_ops::is_builtin_node;

        let target_name = if is_builtin_node(node_id) {
            buffer_names::HELP
        } else {
            buffer_names::KB
        };

        // Look for an existing buffer with the right name
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Kb && b.name == target_name)
        {
            if let Some(view) = self.buffers[idx].kb_view_mut() {
                let v: &mut crate::kb_view::KbView = view;
                v.navigate_to(node_id.to_string());
            }
            return idx;
        }
        let mut buf = Buffer::new_kb(node_id);
        buf.name = target_name.to_string();
        self.buffers.push(buf);
        self.buffers.len() - 1
    }

    /// Mutable view onto the KB buffer's KbView, if any KB buffer exists.
    pub fn kb_view_mut(&mut self) -> Option<&mut crate::kb_view::KbView> {
        self.buffers
            .iter_mut()
            .find(|b| b.kind == crate::buffer::BufferKind::Kb)
            .and_then(|b| b.kb_view_mut())
    }

    /// Immutable view onto the KB buffer's KbView, if any KB buffer exists.
    pub fn kb_view(&self) -> Option<&crate::kb_view::KbView> {
        self.buffers
            .iter()
            .find(|b| b.kind == crate::buffer::BufferKind::Kb)
            .and_then(|b| b.kb_view())
    }

    /// Switch the focused window to the buffer at the given index.
    /// Returns false if index is out of bounds.
    pub fn switch_to_buffer(&mut self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }
        let prev_idx = self.active_buffer_idx();
        if prev_idx != idx {
            self.vi.alternate_buffer_idx = Some(prev_idx);
        }
        self.save_mode_to_buffer();
        // Check for external file changes before showing the buffer.
        self.check_and_reload_buffer(idx);
        let win = self.window_mgr.focused_window_mut();
        win.save_view_state();
        win.restore_view_state(idx);
        // Clamp cursor to buffer bounds (file may have changed on disk).
        let line_count = self.buffers[idx].line_count();
        let win = self.window_mgr.focused_window_mut();
        if win.cursor_row >= line_count {
            win.cursor_row = line_count.saturating_sub(1);
        }
        let line_len = self.buffers[idx].line_len(win.cursor_row);
        if win.cursor_col > line_len {
            win.cursor_col = line_len;
        }
        // Recompute search matches for the new buffer so highlights and
        // `n`/`N` navigation are correct.
        self.recompute_search_matches();
        self.sync_mode_to_buffer();
        true
    }

    /// Returns true if the buffer at `idx` is a Conversation buffer.
    pub fn is_conversation_buffer(&self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }
        if self.buffers[idx].kind == crate::BufferKind::Conversation {
            return true;
        }
        // The *ai-input* buffer is also part of the conversation pair.
        if let Some(ref pair) = self.ai.conversation_pair {
            if idx == pair.input_buffer_idx {
                return true;
            }
        }
        false
    }

    /// Returns true if windows showing this buffer kind can be replaced by new content.
    pub fn is_kind_replaceable(&self, kind: crate::BufferKind) -> bool {
        self.replaceable_kinds.contains(&kind)
    }

    /// Find a window showing a replaceable buffer kind. Returns the window ID.
    /// Prefers the focused window if it's replaceable. Excludes conversation pair windows.
    fn find_replaceable_window(&self) -> Option<crate::window::WindowId> {
        let conv_ids = self
            .ai
            .conversation_pair
            .as_ref()
            .map(|p| [p.output_window_id, p.input_window_id]);
        let focused_id = self.window_mgr.focused_id();
        // Prefer the focused window (natural UX: what you see gets replaced).
        if let Some(fw) = self.window_mgr.window(focused_id) {
            if fw.buffer_idx < self.buffers.len()
                && self.is_kind_replaceable(self.buffers[fw.buffer_idx].kind)
                && !conv_ids.is_some_and(|ids| ids.contains(&focused_id))
            {
                return Some(focused_id);
            }
        }
        // Then check all other windows.
        self.window_mgr
            .iter_windows()
            .find(|w| {
                w.buffer_idx < self.buffers.len()
                    && self.is_kind_replaceable(self.buffers[w.buffer_idx].kind)
                    && !conv_ids.is_some_and(|ids| ids.contains(&w.id))
            })
            .map(|w| w.id)
    }

    /// Returns true if `win_id` belongs to a dedicated purpose (file tree,
    /// conversation pair) and should never be repurposed for general buffer routing.
    pub fn is_dedicated_window(&self, win_id: crate::window::WindowId) -> bool {
        if self.file_tree_window_id == Some(win_id) {
            return true;
        }
        if let Some(ref pair) = self.ai.conversation_pair {
            if win_id == pair.output_window_id || win_id == pair.input_window_id {
                return true;
            }
        }
        // Fallback: check buffer kind for other sidebar types (debug, messages, etc.)
        // but exclude replaceable kinds — those windows CAN be repurposed.
        if let Some(w) = self.window_mgr.window(win_id) {
            if w.buffer_idx < self.buffers.len()
                && self.buffers[w.buffer_idx].kind.is_sidebar()
                && !self.is_kind_replaceable(self.buffers[w.buffer_idx].kind)
            {
                return true;
            }
        }
        false
    }

    /// Clean up self-test state after cancellation or completion.
    /// Returns true if cleanup was performed.
    pub fn cleanup_self_test(&mut self) -> bool {
        if !self.self_test_active {
            return false;
        }
        self.self_test_active = false;
        if let Some(ref dir) = self.test_sandbox_dir.take() {
            if dir.exists() && dir.starts_with(std::env::temp_dir()) {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        let _ = self.restore_state();
        true
    }

    /// Switch to buffer `idx` but avoid stealing focus from a conversation window.
    ///
    /// If the focused window shows a conversation buffer, the new buffer is
    /// routed to another window (or a new split is created). This keeps `*AI*`
    /// Adjust `ai_target_buffer_idx` after a buffer at `removed_idx` was removed.
    /// Must be called after every `buffers.remove()` to prevent stale indices.
    pub fn adjust_ai_target_after_remove(&mut self, removed_idx: usize) {
        if let Some(ref mut target) = self.ai.target_buffer_idx {
            if *target == removed_idx {
                // The target buffer was removed — clear it
                self.ai.target_buffer_idx = None;
            } else if *target > removed_idx {
                *target -= 1;
            }
        }
    }

    /// Central bookkeeping after `buffers.remove(removed_idx)`.
    ///
    /// Rekeys all Editor-owned HashMaps keyed by buffer index, adjusts
    /// pending queues, alternate_buffer_idx, AI target, syntax map, and
    /// per-window saved_view_states. Also pushes `removed_idx` to
    /// `pending_buffer_removals` so the binary can rekey its own maps
    /// (shell_terminals, shell_last_dims, shell_generations).
    ///
    /// Callers are still responsible for adjusting `window.buffer_idx`
    /// (different sites have different retarget logic).
    pub fn notify_buffer_removed(&mut self, removed_idx: usize) {
        // 1. Syntax + AI target
        self.syntax.shift_after_remove(removed_idx);
        self.adjust_ai_target_after_remove(removed_idx);

        // 2. Editor-owned shell maps
        rekey_after_remove(&mut self.shell.viewports, removed_idx);
        rekey_after_remove(&mut self.shell.viewport_cwds, removed_idx);
        rekey_after_remove(&mut self.shell.cwds, removed_idx);

        // 3. Pending shell queues (Vec<usize> and Vec<(usize, _)>)
        self.shell.spawns.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.agent_spawns.retain_mut(|(idx, _)| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.resets.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.closes.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.shell.inputs.retain_mut(|(idx, _)| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });

        // 4. Alternate buffer index
        if let Some(ref mut alt) = self.vi.alternate_buffer_idx {
            if *alt == removed_idx {
                self.vi.alternate_buffer_idx = None;
            } else if *alt > removed_idx {
                *alt -= 1;
            }
        }

        // 5. Per-window saved_view_states
        for win in self.window_mgr.iter_windows_mut() {
            rekey_after_remove(&mut win.saved_view_states, removed_idx);
        }

        // 6. Conversation pair buffer indices
        if let Some(ref mut pair) = self.ai.conversation_pair {
            if pair.output_buffer_idx == removed_idx || pair.input_buffer_idx == removed_idx {
                self.ai.conversation_pair = None; // invalidate
            } else {
                if pair.output_buffer_idx > removed_idx {
                    pair.output_buffer_idx -= 1;
                }
                if pair.input_buffer_idx > removed_idx {
                    pair.input_buffer_idx -= 1;
                }
            }
        }

        // 7. Signal the binary to rekey its own maps
        self.pending_buffer_removals.push(removed_idx);
    }

    /// visible during AI tool calls that open/switch files.
    pub fn switch_to_buffer_non_conversation(&mut self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }

        self.ai.target_buffer_idx = Some(idx);

        // 0. Reuse the dedicated AI work window if it exists and is still valid.
        if let Some(work_id) = self.ai.work_window_id {
            if self.window_mgr.window(work_id).is_some() {
                if let Some(win) = self.window_mgr.window_mut(work_id) {
                    win.buffer_idx = idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
                self.ai.target_window_id = Some(work_id);
                self.mark_full_redraw();
                return true;
            } else {
                self.ai.work_window_id = None; // stale reference
            }
        }

        // 1. Is this buffer already visible?
        if let Some(w) = self.window_mgr.iter_windows().find(|w| w.buffer_idx == idx) {
            self.ai.target_window_id = Some(w.id);
            return true;
        }

        // 2. Can we put it in a non-focused, non-dedicated window?
        let focused_id = self.window_mgr.focused_id();
        let win_ids: Vec<_> = self.window_mgr.iter_windows().map(|w| w.id).collect();
        let other = win_ids
            .into_iter()
            .find(|&wid| wid != focused_id && !self.is_dedicated_window(wid));

        if let Some(other_id) = other {
            if let Some(win) = self.window_mgr.window_mut(other_id) {
                win.buffer_idx = idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
            self.ai.work_window_id = Some(other_id);
            self.ai.target_window_id = Some(other_id);
            self.mark_full_redraw();
            return true;
        }

        // 2.5: If there's a replaceable window (e.g. dashboard), take it over.
        if let Some(repl_id) = self.find_replaceable_window() {
            if let Some(win) = self.window_mgr.window_mut(repl_id) {
                win.buffer_idx = idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
            self.ai.work_window_id = Some(repl_id);
            self.ai.target_window_id = Some(repl_id);
            self.mark_full_redraw();
            return true;
        }

        // 3. Fallback: split a window. Prefer a non-conversation window to avoid
        // splitting the tiny *ai-input* pane or the *AI* output pane.
        let focused_is_conv = self.is_conversation_buffer(self.active_buffer_idx());
        if focused_is_conv {
            // Find any non-conversation window to focus before splitting.
            let non_conv_win = self
                .window_mgr
                .iter_windows()
                .find(|w| !self.is_conversation_buffer(w.buffer_idx))
                .map(|w| w.id);
            if let Some(id) = non_conv_win {
                self.window_mgr.set_focused(id);
            } else if let Some(ref pair) = self.ai.conversation_pair {
                // All windows are conversation. Agent shells are persistent
                // interactive sessions — stealing the output window would
                // permanently replace the conversation display. Skip the steal
                // and fall through to the split attempt below.
                let is_agent_shell = idx < self.buffers.len() && self.buffers[idx].agent_shell;
                if !is_agent_shell {
                    // Non-agent buffer: steal output temporarily (restored on session end).
                    let out_id = pair.output_window_id;
                    if let Some(win) = self.window_mgr.window_mut(out_id) {
                        win.buffer_idx = idx;
                        win.cursor_row = 0;
                        win.cursor_col = 0;
                    }
                    self.ai.work_window_id = Some(out_id);
                    self.ai.target_window_id = Some(out_id);
                    self.mark_full_redraw();
                    return true;
                }
                // Agent shell: fall through to split attempt.
            }
        }
        let area = self.default_area();
        let is_agent = idx < self.buffers.len() && self.buffers[idx].agent_shell;

        // For agent shells, use split_root to guarantee the shell gets a
        // top-level window beside the entire conversation group, regardless
        // of which conversation pane is focused.
        let split_result = if is_agent {
            self.window_mgr
                .split_root(crate::window::SplitDirection::Vertical, idx, area, 0.5)
        } else {
            self.window_mgr
                .split(crate::window::SplitDirection::Vertical, idx, area)
        };

        match split_result {
            Ok(new_id) => {
                self.ai.work_window_id = Some(new_id);
                self.ai.target_window_id = Some(new_id);
                self.mark_full_redraw();
                true
            }
            Err(_) => {
                // Too small to split — if we are in conversation, we HAVE to steal focus
                // but we try to avoid it.
                if self.is_conversation_buffer(self.active_buffer_idx()) {
                    self.switch_to_buffer(idx)
                } else {
                    // Not in conversation, so just keep focus where it is.
                    true
                }
            }
        }
    }

    /// Policy-aware buffer display: routes the buffer to the right window
    /// based on its `BufferKind` and the active `DisplayPolicy`.
    ///
    /// This is the primary entry point for making a buffer visible. It replaces
    /// direct `focused_window_mut().buffer_idx = idx` assignments throughout
    /// the codebase, adding conversation protection and side-window reuse.
    pub fn display_buffer(&mut self, buf_idx: usize) {
        if buf_idx >= self.buffers.len() {
            return;
        }
        let kind = self.buffers[buf_idx].kind;
        let action = self.display_policy.action_for(kind);
        match action {
            crate::display_policy::DisplayAction::ReplaceFocused => {
                if self.is_conversation_buffer(self.active_buffer_idx()) {
                    self.switch_to_buffer_non_conversation(buf_idx);
                } else {
                    let win = self.window_mgr.focused_window_mut();
                    win.buffer_idx = buf_idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
            }
            crate::display_policy::DisplayAction::AvoidConversation => {
                if self.is_conversation_buffer(self.active_buffer_idx()) {
                    self.switch_to_buffer_non_conversation(buf_idx);
                } else {
                    let win = self.window_mgr.focused_window_mut();
                    win.buffer_idx = buf_idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
            }
            crate::display_policy::DisplayAction::ReuseOrSplit { direction, ratio } => {
                // Side-window pattern: reuse existing window of same kind.
                let reuse_win_id = self.find_window_with_kind(kind);
                if let Some(win_id) = reuse_win_id {
                    if let Some(win) = self.window_mgr.window_mut(win_id) {
                        win.buffer_idx = buf_idx;
                        win.cursor_row = 0;
                        win.cursor_col = 0;
                    }
                } else if let Some(repl_id) = self.find_replaceable_window() {
                    // Replace a replaceable window instead of splitting alongside it.
                    if kind
                        != self.buffers[self.window_mgr.window(repl_id).unwrap().buffer_idx].kind
                    {
                        if let Some(win) = self.window_mgr.window_mut(repl_id) {
                            win.buffer_idx = buf_idx;
                            win.cursor_row = 0;
                            win.cursor_col = 0;
                        }
                    } else {
                        self.display_buffer_split(buf_idx, direction, ratio);
                    }
                } else {
                    self.display_buffer_split(buf_idx, direction, ratio);
                }
            }
            crate::display_policy::DisplayAction::Hidden => {}
        }
        self.mark_full_redraw();
    }

    /// Like `display_buffer` but also sets focus to the window showing the buffer.
    /// Use this when opening a buffer that the user wants to interact with immediately
    /// (e.g. terminal, agenda). Also sets `alternate_buffer_idx`.
    pub fn display_buffer_and_focus(&mut self, buf_idx: usize) {
        if buf_idx >= self.buffers.len() {
            return;
        }
        let prev_idx = self.active_buffer_idx();
        self.save_mode_to_buffer();
        // Save the current window's view state before switching.
        self.window_mgr.focused_window_mut().save_view_state();
        self.display_buffer(buf_idx);
        // Find the window now showing buf_idx and focus it.
        let win_id = self
            .window_mgr
            .iter_windows()
            .find(|w| w.buffer_idx == buf_idx)
            .map(|w| w.id);
        if let Some(id) = win_id {
            self.window_mgr.set_focused(id);
        }
        // Restore view state for the new buffer (scroll position, cursor).
        self.window_mgr
            .focused_window_mut()
            .restore_view_state(buf_idx);
        // No forced fallback: if display_buffer() routed the buffer via
        // switch_to_buffer_non_conversation (e.g. split_root for agent
        // shells), the buffer is already placed in a new window that may
        // not match the iter_windows search above. Forcing it into the
        // focused window would steal conversation windows.
        if prev_idx != buf_idx {
            self.vi.alternate_buffer_idx = Some(prev_idx);
        }
        self.sync_mode_to_buffer();
    }

    /// Find a window showing a buffer of the given kind (non-conversation).
    /// Excludes windows that are part of the conversation pair (output/input).
    fn find_window_with_kind(&self, kind: crate::BufferKind) -> Option<crate::window::WindowId> {
        let conv_ids = self
            .ai
            .conversation_pair
            .as_ref()
            .map(|p| [p.output_window_id, p.input_window_id]);
        for w in self.window_mgr.iter_windows() {
            if w.buffer_idx < self.buffers.len()
                && self.buffers[w.buffer_idx].kind == kind
                && !self.is_conversation_buffer(w.buffer_idx)
                && !conv_ids.is_some_and(|ids| ids.contains(&w.id))
            {
                return Some(w.id);
            }
        }
        None
    }

    /// Split helper for display_buffer: creates a new split.
    /// Group-aware: if focused inside a conversation group, the split wraps the
    /// entire group rather than splitting within it.
    fn display_buffer_split(
        &mut self,
        buf_idx: usize,
        direction: crate::window::SplitDirection,
        ratio: f32,
    ) {
        // Redirect focus away from conversation windows before splitting,
        // so the split happens outside the conversation group.
        if self.is_conversation_buffer(self.active_buffer_idx()) {
            let non_conv_win = self
                .window_mgr
                .iter_windows()
                .find(|w| !self.is_conversation_buffer(w.buffer_idx))
                .map(|w| w.id);
            if let Some(nc_id) = non_conv_win {
                self.window_mgr.set_focused(nc_id);
            } else {
                // All windows are conversation — split_root to place beside the group.
                let area = self.default_area();
                match self.window_mgr.split_root(direction, buf_idx, area, ratio) {
                    Ok(new_win_id) => self.window_mgr.set_focused(new_win_id),
                    Err(_) => {
                        self.switch_to_buffer_non_conversation(buf_idx);
                    }
                }
                return;
            }
        }
        let area = self.default_area();
        match self
            .window_mgr
            .split_with_ratio(direction, buf_idx, area, ratio)
        {
            Ok(new_win_id) => {
                self.window_mgr.set_focused(new_win_id);
            }
            Err(_) => {
                self.switch_to_buffer_non_conversation(buf_idx);
            }
        }
    }

    /// Open a file without stealing focus from a conversation window.
    ///
    /// The file is opened "hidden" (not assigned to focused window), then
    /// routed via `switch_to_buffer_non_conversation`.
    pub fn open_file_non_conversation(&mut self, path: impl AsRef<std::path::Path>) {
        if let Some(new_idx) = self.open_file_hidden(path) {
            self.switch_to_buffer_non_conversation(new_idx);
        }
    }

    /// Save current mode to the active buffer before switching away.
    pub fn save_mode_to_buffer(&mut self) {
        let idx = self.active_buffer_idx();
        self.buffers[idx].saved_mode = Some(self.mode);
    }

    /// Sync `self.mode` to the active buffer's kind after a focus/buffer change.
    /// Restores per-buffer `saved_mode` when available; otherwise falls back to
    /// a sensible default based on buffer kind.
    pub fn sync_mode_to_buffer(&mut self) {
        let idx = self.active_buffer_idx();
        self.ensure_buffer_git_branch(idx);
        let kind = self.buffers[idx].kind;

        if let Some(saved) = self.buffers[idx].saved_mode {
            // Validate saved mode is appropriate for the buffer kind.
            let valid = match kind {
                crate::BufferKind::Shell => {
                    matches!(saved, Mode::ShellInsert | Mode::Normal)
                }
                crate::BufferKind::Conversation => {
                    matches!(
                        saved,
                        Mode::ConversationInput | Mode::Normal | Mode::Visual(_)
                    )
                }
                _ => !matches!(saved, Mode::ShellInsert),
            };
            if valid {
                self.set_mode(saved);
                return;
            }
        }

        // No saved mode or invalid — use default.
        match kind {
            crate::BufferKind::Shell => {
                self.set_mode(Mode::ShellInsert);
            }
            _ => {
                if matches!(self.mode, Mode::ShellInsert | Mode::ConversationInput) {
                    self.set_mode(Mode::Normal);
                }
            }
        }
    }

    /// Reset the AI session: request cancellation, clear state, and end streaming.
    pub fn reset_ai_session(&mut self) {
        self.ai.cancel_requested = true;
        self.ai.streaming = false;
        self.ai.current_round = 0;
        self.ai.transaction_start_idx = None;
        if let Some(conv) = self.conversation_mut() {
            conv.end_streaming();
            conv.push_system("[AI Session Reset]");
        }
        self.ai.input_lock = crate::InputLock::None;
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
        if !s.is_empty() {
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

    /// Single source of truth for how many visual cell-rows a buffer line occupies.
    ///
    /// Accounts for folds (0 rows), word wrap (>= 1 rows), and heading scale
    /// (ceil of scale factor). All scroll paths — `ensure_scroll_wrapped`,
    /// `scroll_up_line_wrapped`, mouse scroll bottom computation — must use this
    /// instead of computing visual rows independently, to prevent the scroll guard
    /// from fighting with scroll commands.
    pub fn line_visual_rows(&self, buf_idx: usize, line: usize) -> usize {
        let buf = &self.buffers[buf_idx];
        // Folded lines are invisible.
        if buf.is_line_folded(line) {
            return 0;
        }
        if line >= buf.rope().len_lines() {
            return 1;
        }

        // Check visual rows cache for text rows.
        let text_rows = if let Some(ref cache) = buf.visual_rows_cache {
            if cache.generation == buf.generation
                && cache.display_regions_gen == buf.display_regions_gen
                && cache.text_width == self.text_area_width
                && cache.break_indent == self.break_indent
                && cache.show_break_width == self.show_break.chars().count()
                && cache.heading_scale == self.heading_scale
                && line >= cache.line_start
                && line < cache.line_start + cache.rows.len()
            {
                let v = cache.rows[line - cache.line_start] as usize;
                if v > 0 {
                    Some(v)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let text_rows = text_rows.unwrap_or_else(|| self.line_text_visual_rows(buf_idx, line));

        // Account for inline image display height.
        let image_rows = if buf.local_options.inline_images.unwrap_or(false) {
            self.image_extra_rows(buf, line)
        } else {
            0
        };

        text_rows + image_rows
    }

    /// Compute the text-only visual rows for a line, applying display regions
    /// (link concealment) before wrapping — matching what `compute_layout()` does.
    fn line_text_visual_rows(&self, buf_idx: usize, line: usize) -> usize {
        let buf = &self.buffers[buf_idx];
        let rope = buf.rope();
        if line >= rope.len_lines() {
            return 1;
        }

        let line_slice = rope.line(line);
        let line_char_count = line_slice.len_chars();
        let line_byte_count = line_slice.len_bytes();
        // Content length excluding trailing newline.
        let content_len = if line_char_count > 0 && line_slice.char(line_char_count - 1) == '\n' {
            line_char_count - 1
        } else {
            line_char_count
        };

        // Fast path: no wrap, no heading scale, no display regions — skip char collection.
        // byte_len == char_count implies all single-byte (ASCII) chars, so
        // content_len == display_width.
        let is_ascii = line_byte_count == line_char_count;
        let has_display_regions = !buf.display_regions.is_empty();

        if !self.word_wrap_for(buf_idx) || self.text_area_width == 0 {
            // Still need heading rows for non-wrapped headings.
            if !self.heading_scale {
                return 1;
            }
            // Only collect chars for heading detection.
            let rope_chars: Vec<char> = line_slice
                .chars()
                .filter(|c| *c != '\n' && *c != '\r')
                .collect();
            let heading_level = crate::heading::heading_level_from_chars(&rope_chars);
            if heading_level == 0 {
                return 1;
            }
            return crate::heading::heading_scale_for_level_with(
                heading_level,
                self.heading_scale_h1,
                self.heading_scale_h2,
                self.heading_scale_h3,
            )
            .ceil() as usize;
        }

        // Fast path: short ASCII line, no heading scale, no display regions →
        // guaranteed to fit in one row, skip all allocation.
        if is_ascii
            && content_len <= self.text_area_width
            && !self.heading_scale
            && !has_display_regions
        {
            return 1;
        }

        let rope_chars: Vec<char> = line_slice
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();

        let heading_level = crate::heading::heading_level_from_chars(&rope_chars);
        let heading_scale_factor = if self.heading_scale && heading_level > 0 {
            crate::heading::heading_scale_for_level_with(
                heading_level,
                self.heading_scale_h1,
                self.heading_scale_h2,
                self.heading_scale_h3,
            )
        } else {
            1.0
        };
        let heading_rows = heading_scale_factor.ceil() as usize;

        // Apply display regions (link concealment) to match compute_layout() behavior.
        let effective_regions = crate::display_region::regions_with_cursor_reveal(
            &buf.display_regions,
            buf.display_reveal_cursor,
        );

        let line_byte_start = rope.line_to_byte(line);
        let next_line_byte = if line + 1 < rope.len_lines() {
            rope.line_to_byte(line + 1)
        } else {
            rope.len_bytes()
        };

        // Check if any display regions overlap this line (binary search).
        let start_idx = effective_regions.partition_point(|r| r.byte_end <= line_byte_start);
        let has_regions = effective_regions
            .get(start_idx)
            .is_some_and(|r| r.byte_start < next_line_byte);

        let chars_for_wrap = if has_regions {
            let (display_chars, _) = crate::display_region::apply_display_regions_to_line(
                &rope_chars,
                line_byte_start,
                next_line_byte,
                &effective_regions,
            );
            display_chars
        } else {
            rope_chars
        };

        // For headings with scale > 1, reduce wrap width to match GUI layout.
        // compute_layout() does: (text_area_width / scale).floor()
        let wrap_width = if heading_scale_factor > 1.0 {
            (self.text_area_width as f32 / heading_scale_factor).floor() as usize
        } else {
            self.text_area_width
        };

        let text: String = chars_for_wrap.iter().collect();
        let sb_w = self.show_break.chars().count();
        let wrap_rows =
            crate::wrap::wrap_line_display_rows(&text, wrap_width, self.break_indent, sb_w);

        // Heading wrap correctness: first wrap segment gets heading scale,
        // continuation rows are normal height. Total cell rows =
        // heading_rows (ceil of scale) + (wrap_count - 1) continuation rows.
        (wrap_rows - 1) + heading_rows
    }

    /// Pre-compute visual row counts for a contiguous line range and store in the
    /// buffer's cache. Subsequent `line_visual_rows()` calls hit the cache.
    /// Pre-compute visual row counts for a contiguous needed range and store
    /// in the buffer's cache.
    ///
    /// **Fix A**: The cache is checked against the `needed_start..needed_end`
    /// range, but on miss it computes a wider padded range to absorb future
    /// scroll shifts without re-computation.
    pub fn populate_visual_rows_cache(
        &mut self,
        buf_idx: usize,
        needed_start: usize,
        needed_end: usize,
    ) {
        let buf = &self.buffers[buf_idx];
        let gen = buf.generation;
        let dr_gen = buf.display_regions_gen;
        let text_width = self.text_area_width;
        let break_indent = self.break_indent;
        let sb_w = self.show_break.chars().count();
        let hs = self.heading_scale;

        // Check if existing cache covers the NEEDED range (not padded).
        if let Some(ref cache) = buf.visual_rows_cache {
            if cache.generation == gen
                && cache.display_regions_gen == dr_gen
                && cache.text_width == text_width
                && cache.break_indent == break_indent
                && cache.show_break_width == sb_w
                && cache.heading_scale == hs
                && cache.line_start <= needed_start
                && cache.line_start + cache.rows.len() >= needed_end
            {
                self.perf_stats.visual_rows_cache_hits += 1;
                return;
            }
        }
        self.perf_stats.visual_rows_cache_misses += 1;

        // Miss — compute with padding to absorb future scroll shifts.
        let total = buf.display_line_count();
        let pad = self.focused_viewport_height();
        let compute_start = needed_start.saturating_sub(pad);
        let compute_end = (needed_end + pad).min(total);

        let mut rows = Vec::with_capacity(compute_end.saturating_sub(compute_start));
        for line in compute_start..compute_end {
            let r = self.line_text_visual_rows(buf_idx, line);
            rows.push(r.min(255) as u8);
        }

        self.buffers[buf_idx].visual_rows_cache = Some(crate::buffer::VisualRowsCache {
            generation: gen,
            display_regions_gen: dr_gen,
            text_width,
            break_indent,
            show_break_width: sb_w,
            heading_scale: hs,
            line_start: compute_start,
            rows,
        });
    }

    /// Estimate extra visual rows consumed by an inline image on this line.
    /// Uses the same sizing logic as GUI layout (MAX_H=400, aspect-ratio fit).
    fn image_extra_rows(&self, buf: &crate::buffer::Buffer, line: usize) -> usize {
        let rope = buf.rope();
        if line >= rope.len_lines() {
            return 0;
        }
        let line_byte_start = rope.line_to_byte(line);
        let line_byte_end = if line + 1 < rope.len_lines() {
            rope.line_to_byte(line + 1)
        } else {
            rope.len_bytes()
        };
        for region in &buf.display_regions {
            if region.byte_start >= line_byte_end {
                break;
            }
            if region.byte_end <= line_byte_start {
                continue;
            }
            if let Some(ref img) = region.image {
                // Mirror GUI layout sizing: MAX_H=400, text_area_width for max_w.
                // Use actual cell dimensions pushed by the GUI (or 8.0/16.0 defaults).
                let text_area_px = (self.text_area_width as f32) * self.gui_cell_width;
                let max_w = if let Some(w) = img.width {
                    (w as f32).min(text_area_px)
                } else {
                    text_area_px
                };
                const MAX_H: f32 = 400.0;
                // Use cached dimensions from ImageAttrs (populated at region creation time).
                let (img_w, img_h) = if img.natural_width > 0 && img.natural_height > 0 {
                    (img.natural_width as f32, img.natural_height as f32)
                } else {
                    (max_w, max_w)
                };
                let display_h = if img_w > 0.0 && img_h > 0.0 {
                    let h = max_w / (img_w / img_h);
                    h.min(MAX_H)
                } else {
                    max_w.min(MAX_H)
                };
                let cell_h = self.gui_cell_height;
                return (display_h / cell_h).ceil() as usize;
            }
        }
        0
    }

    /// Calculate the actual inner height (text rows) for the focused window.
    /// This accounts for the window manager layout AND window borders.
    pub fn focused_window_viewport_height(&self, total_area: Rect) -> usize {
        let rects = self.window_mgr.layout_rects(total_area);
        let focused_id = self.window_mgr.focused_id();
        if let Some((_, rect)) = rects.iter().find(|(id, _)| *id == focused_id) {
            // Every window currently has a top and bottom border (2 rows total).
            (rect.height as usize).saturating_sub(2)
        } else {
            (total_area.height as usize).saturating_sub(2)
        }
    }

    /// Default area for window operations when we don't have the real terminal size.
    /// The renderer will provide real dimensions at render time.
    pub fn default_area(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        }
    }

    /// Handle a mouse click at the given cell coordinates.
    ///
    /// Left-click places the cursor, adjusting for gutter width and scroll offset.
    /// Middle-click pastes from the default register. Right-click is reserved for
    /// future context menu support.
    /// Set cursor position directly from buffer (row, col) coordinates.
    /// Used by the GUI mouse handler when FrameLayout-based pixel positioning
    /// is available (bypasses scroll/gutter arithmetic).
    pub fn set_cursor_position(&mut self, buf_row: usize, char_col: usize) {
        let win = self.window_mgr.focused_window();
        let buf = &self.buffers[win.buffer_idx];
        let max_row = buf.display_line_count().saturating_sub(1);
        let target_row = buf_row.min(max_row);
        let line_len = buf.line_len(target_row);
        let target_col = char_col.min(if line_len > 0 { line_len - 1 } else { 0 });
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = target_row;
        win.cursor_col = target_col;
    }
}
