mod agenda_ops;
mod babel_ops;
mod changes;
mod command;
mod dap_ops;
mod debug_panel_ops;
mod diagnostics;
mod dispatch;
mod edit_ops;
pub(crate) mod ex_parse;
mod file_ops;
mod git_ops;
mod heading_ops;
mod help_ops;
mod hook_ops;
mod jumps;
pub(crate) mod kb_ops;
mod keymaps;
mod lsp_actions;
mod lsp_completion;
mod lsp_ops;
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
mod visual;

pub use changes::{ChangeEntry, CHANGE_LIST_CAP};
pub use diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticStore};
pub use jumps::{JumpEntry, JUMP_LIST_CAP};
pub use kb_ops::KbWatcherStats;
pub use lsp_ops::{DocumentHighlightRange, HighlightKind, LspLocation, LspRange};
pub use marks::Mark;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

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
use crate::dap_intent::DapIntent;
use crate::debug::DebugState;
use crate::file_picker::FilePicker;
use crate::hooks::HookRegistry;
use crate::kb_seed::seed_kb;
use crate::keymap::{KeyPress, Keymap, WhichKeyEntry};
use crate::lsp_intent::LspIntent;
use crate::messages::MessageLog;
use crate::options::OptionRegistry;
use crate::search::SearchState;
use crate::syntax::Language;
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
    pub running: bool,
    pub status_msg: String,
    /// Name of the command currently being dispatched (Emacs `this-command`).
    pub current_command: String,
    pub command_line: String,
    pub commands: CommandRegistry,
    pub keymaps: HashMap<String, Keymap>,
    /// Current which-key prefix being accumulated. Empty = no popup.
    pub which_key_prefix: Vec<KeyPress>,
    /// In-editor message log (*Messages* buffer equivalent).
    /// Shared with the tracing layer via MessageLogHandle.
    pub message_log: MessageLog,
    /// Active color theme. All rendering reads from this.
    pub theme: Theme,
    /// Active debug session state, if any. Both self-debug and DAP populate this.
    pub debug_state: Option<DebugState>,
    /// Named registers for yank/paste (vi `"` register is the default).
    pub registers: HashMap<char, String>,
    /// Pending char-argument command (e.g. after pressing `f`, waiting for target char).
    pub pending_char_command: Option<String>,
    /// True while the user is resolving `SPC h k` (describe-key).
    /// The next key sequence they type is looked up in the normal
    /// keymap, and the resulting command's help page is opened instead
    /// of dispatched. Cleared on resolution or Escape.
    pub awaiting_key_description: bool,
    /// Transient flag for double-Esc detection in the *AI* output buffer.
    pub conv_esc_pending: bool,
    /// Active named register selected by `"x` prefix. Consumed by the
    /// next yank/delete/paste operation. Uppercase = append mode,
    /// `_` = black-hole (discard), `+`/`*` = system clipboard.
    pub active_register: Option<char>,
    /// True after the user pressed `"` in normal/visual mode; the next
    /// char will populate [`Self::active_register`].
    pub pending_register_prompt: bool,
    /// True after the user pressed `Ctrl-R` in insert mode; the next
    /// char selects a register whose contents will be inserted at the
    /// cursor. Cleared on resolution or Escape.
    pub pending_insert_register: bool,
    /// C-o in insert mode: execute one normal command then return to insert.
    pub insert_mode_oneshot_normal: bool,
    /// First delimiter captured during a `cs<from><to>` sequence. Set
    /// after `cs` + the first char, consumed when the second char
    /// arrives.
    pub pending_surround_from: Option<char>,
    /// Search state (pattern, cached matches, direction).
    pub search_state: SearchState,
    /// Current search input being typed in Search mode.
    pub search_input: String,
    /// Visual mode anchor (row, col) — start of selection.
    pub visual_anchor_row: usize,
    pub visual_anchor_col: usize,
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
    /// Tab completion matches for command mode (:e path).
    pub tab_completions: Vec<String>,
    pub tab_completion_idx: usize,
    /// Last repeatable edit for dot-repeat (`.`).
    pub last_edit: Option<EditRecord>,
    /// Char offset at the point insert mode was entered (for capturing inserted text).
    pub insert_start_offset: Option<usize>,
    /// The command that initiated the current insert mode session (for dot-repeat).
    pub insert_initiated_by: Option<String>,
    /// Cursor position (buffer_idx, row, col) at the point insert mode was
    /// last exited. Used by `gi` to re-enter insert at that spot.
    pub last_insert_pos: Option<(usize, usize, usize)>,
    /// Jump list (vim `Ctrl-o` / `Ctrl-i`, Practical Vim ch. 9).
    /// Oldest → newest. Capped at [`JUMP_LIST_CAP`].
    pub jumps: Vec<JumpEntry>,
    /// Cursor into `jumps`. `jump_idx == jumps.len()` means "past newest"
    /// (fresh state); a successful Ctrl-o decrements it.
    pub jump_idx: usize,
    /// Change list (vim `g;` / `g,`, Practical Vim ch. 9). Oldest →
    /// newest. Capped at [`CHANGE_LIST_CAP`].
    pub changes: Vec<ChangeEntry>,
    /// Cursor into `changes`. `change_idx == changes.len()` means
    /// "past newest"; a successful `g;` decrements it.
    pub change_idx: usize,
    /// Vi-style count prefix (e.g. `5j` = move down 5). None = no count typed.
    pub count_prefix: Option<usize>,
    /// Count saved for pending char-argument commands (f/F/t/T/r + char).
    pub pending_char_count: usize,
    /// Index of the previously active buffer (for Ctrl-^ alternate file).
    pub alternate_buffer_idx: Option<usize>,
    /// Command-line history (for up/down recall in `:` mode).
    pub command_history: Vec<String>,
    /// Current index into command_history when recalling (None = not recalling).
    pub command_history_idx: Option<usize>,
    /// Cursor position (byte index) within `command_line` for readline-style editing.
    pub command_cursor: usize,
    /// Queue of pending LSP requests for the binary to drain each event-loop tick.
    /// The core cannot call async LSP code directly; instead, commands push
    /// intents here and `main.rs` forwards them to `run_lsp_task`.
    pub pending_lsp_requests: Vec<LspIntent>,
    /// LSP trigger characters per language (populated from server capabilities).
    pub lsp_trigger_characters: std::collections::HashMap<String, Vec<String>>,
    /// Signal for the binary to send `workspace/didChangeWorkspaceFolders`
    /// when a project root is first detected after LSP has already started
    /// (e.g. launched from app launcher with `cwd = $HOME`).
    pub pending_lsp_root_change: Option<String>,
    /// Queue of pending DAP requests for the binary to drain each event-loop tick.
    /// Same pattern as `pending_lsp_requests`: core cannot call async DAP code
    /// directly; commands push intents here and `main.rs` forwards them to
    /// `run_dap_task`.
    pub pending_dap_intents: Vec<DapIntent>,
    /// Buffer indices of newly created shell buffers that need PTY spawning.
    /// The binary drains this and creates `ShellTerminal` instances.
    pub pending_shell_spawns: Vec<usize>,
    /// Working directory overrides for shell spawns: buffer_idx → dir.
    /// Drained together with `pending_shell_spawns` by the binary.
    pub pending_shell_cwds: HashMap<usize, std::path::PathBuf>,
    /// Agent shell spawns: (buf_idx, command). The binary spawns these with
    /// `spawn_command` so the PTY exits when the agent command exits.
    pub pending_agent_spawns: Vec<(usize, String)>,
    /// Buffer indices of shell terminals that should be reset (clear screen).
    /// Drained by the binary which owns the `ShellTerminal` instances.
    pub pending_shell_resets: Vec<usize>,
    /// Buffer indices of shell terminals that should be closed.
    /// Drained by the binary which shuts down the PTY and removes the terminal.
    pub pending_shell_closes: Vec<usize>,
    /// Queued text to send to shell terminals: (buffer_index, text).
    /// Drained by the binary which owns the `ShellTerminal` instances.
    pub pending_shell_inputs: Vec<(usize, String)>,
    /// Pending shell scroll amount. Positive = scroll up, negative = scroll down,
    /// zero = scroll to bottom. Consumed by the binary which owns `ShellTerminal`.
    pub pending_shell_scroll: Option<i32>,
    /// Pending shell mouse click: (row, col, button). Set by `handle_mouse_click`
    /// for shell buffers, drained by the binary which owns `ShellTerminal`.
    pub pending_shell_click: Option<(usize, usize, crate::input::MouseButton)>,
    /// Pending shell mouse drag position: (row, col). Set during drag in shell
    /// buffers, drained by the binary.
    pub pending_shell_drag: Option<(usize, usize)>,
    /// Pending shell mouse release position: (row, col). Set on button release
    /// in shell buffers, drained by the binary to finalize selection.
    pub pending_shell_release: Option<(usize, usize)>,
    /// Buffer indices removed this tick, for the binary to rekey its own
    /// shell-related HashMaps (shell_terminals, shell_last_dims, etc.).
    pub pending_buffer_removals: Vec<usize>,
    /// Cached viewport snapshots for shell terminals, updated by the binary
    /// each render tick. Keyed by buffer index. Used by AI tools to read
    /// terminal output without direct access to `ShellTerminal`.
    pub shell_viewports: HashMap<usize, Vec<String>>,
    /// Cached current working directories for shell terminals, keyed by
    /// buffer index. Updated by the binary via /proc/{pid}/cwd.
    pub shell_cwds: HashMap<usize, String>,
    /// Hook registry: named extension points with ordered Scheme function lists.
    /// Populated by `(add-hook! ...)` from Scheme, fired by core operations.
    pub hooks: HookRegistry,
    /// Queued hook evaluations for the binary to drain. Each entry is
    /// `(hook_name, scheme_fn_name)`. Core pushes here; the binary drains
    /// and calls the Scheme runtime (same pattern as `pending_scheme_eval`).
    pub pending_hook_evals: Vec<(String, String)>,
    /// LSP diagnostics keyed by file URI. Replaced wholesale on each
    /// `publishDiagnostics` notification (the LSP contract).
    pub diagnostics: DiagnosticStore,
    /// LSP server info (status + discovery metadata), keyed by language_id.
    pub lsp_servers: HashMap<String, LspServerInfo>,
    /// Per-buffer tree-sitter state (parsed trees + cached highlight spans).
    /// Buffers without a detected language simply have no entry.
    pub syntax: crate::syntax::SyntaxMap,
    /// Buffer indices that need a deferred syntax reparse. Populated by the
    /// renderer when it uses stale spans; drained by the event loop after
    /// a debounce period (~50ms after last edit).
    pub syntax_reparse_pending: std::collections::HashSet<usize>,
    /// Timestamp of the last buffer edit. Used for debouncing syntax reparses.
    pub last_edit_time: std::time::Instant,
    /// Stack of prior char-offset visual selections created by
    /// `syntax_expand_selection` — lets `syntax_contract_selection` walk
    /// back down the node tree. Cleared on `syntax_select_node`.
    pub syntax_selection_stack: Vec<(usize, usize)>,
    /// Named cursor marks, keyed by mark letter (`m`+letter to set,
    /// `'`+letter to jump). Paths make marks survive buffer switches.
    pub marks: HashMap<char, Mark>,
    /// LSP completion popup state. Empty = no popup visible.
    pub completion_items: Vec<CompletionItem>,
    /// Index of the currently selected completion item.
    pub completion_selected: usize,
    /// True while a macro is being recorded into `macro_register`.
    pub macro_recording: bool,
    /// Register letter being recorded into (a-z).
    pub macro_register: Option<char>,
    /// Raw keystroke log for the active recording session.
    pub macro_log: Vec<crate::keymap::KeyPress>,
    /// Register letter of the last-replayed macro (for `@@`).
    pub last_macro_register: Option<char>,
    /// Recursion depth guard during macro replay (max 10).
    pub macro_replay_depth: usize,
    /// Knowledge base: backing store for the help system and the
    /// AI-facing `kb_*` tools. Seeded from `CommandRegistry` +
    /// hand-authored concept nodes on startup.
    pub kb: mae_kb::KnowledgeBase,
    /// KB federation: registry of external KB instances (org-roam dirs etc.).
    pub kb_registry: mae_kb::federation::KbRegistry,
    /// KB federation: loaded KB instances keyed by registry UUID.
    pub kb_instances: HashMap<String, mae_kb::KnowledgeBase>,
    /// KB federation: live file watchers for registered org directories.
    pub kb_watchers: HashMap<String, mae_kb::watch::OrgDirWatcher>,
    /// KB watcher: last drain timestamp per instance UUID (for debounce).
    pub kb_last_drain: HashMap<String, std::time::Instant>,
    /// KB watcher: cumulative statistics.
    pub kb_watcher_stats: KbWatcherStats,
    /// KB option: enable/disable file watchers.
    pub kb_watcher_enabled: bool,
    /// KB option: debounce interval in ms between watcher drains.
    pub kb_watcher_debounce_ms: u64,
    /// KB option: max events processed per idle tick.
    pub kb_max_drain_events: usize,
    /// KB option: max bytes for RAG excerpt truncation.
    pub kb_search_excerpt_length: usize,
    /// KB option: hard cap for kb_search_context results.
    pub kb_search_max_results: usize,
    /// KB option: auto-register org directories in project root.
    pub kb_auto_register: bool,
    /// KB node IDs visited via AI tools (kb_get/links_from/links_to) this session.
    /// Append guidance on revisit to steer away from manual graph traversal loops.
    /// Cleared when a new AI conversation starts.
    pub kb_ai_visited_ids: std::collections::HashSet<String>,

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
    pub last_help_state: Option<crate::help_view::HelpView>,
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
    /// Pending operator for operator-pending mode (`d`, `c`, `y`).
    /// When set, the next motion completes the operator.
    pub pending_operator: Option<String>,
    /// Cursor position (row, col) when operator-pending started.
    pub operator_start: Option<(usize, usize)>,
    /// Count prefix saved from the operator key (e.g. `2d` saves 2).
    /// Multiplied with the motion's own count when the motion fires.
    pub operator_count: Option<usize>,
    /// True if the last dispatched motion was linewise (gg, G, {, }, etc.).
    pub last_motion_linewise: bool,
    /// Char offset range saved by `ys{motion}` for the subsequent char-await
    /// that wraps the range with a delimiter pair.
    pub pending_surround_range: Option<(usize, usize)>,
    /// Last f/F/t/T search: (char, command-name). `;` repeats same direction,
    /// `,` repeats opposite.
    pub last_find_char: Option<(char, String)>,
    /// Saved visual selection from last exit: (anchor_row, anchor_col, cursor_row, cursor_col, visual_type).
    pub last_visual: Option<(usize, usize, usize, usize, crate::VisualType)>,
    /// Scheme code queued for evaluation by the binary. Commands like
    /// `eval-line` / `eval-buffer` push the captured text here; the
    /// event loop drains it after dispatch (same pattern as LSP intents).
    pub pending_scheme_eval: Vec<String>,
    /// Running AI session spend in USD (zero for unpriced/local models).
    /// Surfaced in the status line so users see the meter tick before
    /// they blow past a budget.
    pub ai_session_cost_usd: f64,
    /// Cumulative prompt tokens this session (all providers).
    pub ai_session_tokens_in: u64,
    /// Cumulative completion tokens this session (all providers).
    pub ai_session_tokens_out: u64,
    /// Cumulative cache read tokens (prompt cache hits).
    pub ai_cache_read_tokens: u64,
    /// Cumulative cache creation tokens.
    pub ai_cache_creation_tokens: u64,
    /// Model's context window size in tokens.
    pub ai_context_window: u64,
    /// Estimated tokens currently used in context.
    pub ai_context_used_tokens: u64,
    /// Timestamp of the last successful AI API call.
    pub ai_last_api_success: Option<std::time::Instant>,
    /// Last AI API error message (if any).
    pub ai_last_api_error: Option<String>,
    /// Latency of the last AI API call in milliseconds.
    pub ai_last_api_latency_ms: Option<u64>,
    /// Total number of AI API calls this session.
    pub ai_api_call_count: u64,
    /// Last network connectivity check result (from :ai-ping).
    /// Fields: (endpoint, reachable, http_status, latency_ms, error).
    pub ai_last_network_check: Option<AiNetworkCheck>,
    /// Throttle for AI output scroll during streaming. Only `StreamChunk`
    /// events are throttled (50ms); discrete events always scroll immediately.
    pub ai_last_output_scroll: Option<std::time::Instant>,
    /// Dedicated window for AI file operations. Reused across all open_file/switch_buffer
    /// calls during a session. Prevents the AI from creating multiple splits.
    /// Cleared on session end.
    pub ai_work_window_id: Option<crate::window::WindowId>,
    /// Visual bell: when set, the renderer inverts the status bar background
    /// until this instant passes. Emacs `visible-bell` equivalent.
    pub bell_until: Option<std::time::Instant>,
    /// Detected project for the current working context.
    pub project: Option<crate::project::Project>,
    /// Cached git branch name for the active project. Updated on project detect and file save.
    pub git_branch: Option<String>,
    /// Current AI permission tier label for status display.
    pub ai_permission_tier: String,
    /// Recently opened files (bounded, deduplicated).
    pub recent_files: crate::project::RecentFiles,
    /// Recently used project roots (bounded, deduplicated).
    pub recent_projects: crate::project::RecentProjects,
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
    /// Toggle: hide *bold* and /italic/ markers in Org-mode.
    pub org_hide_emphasis_markers: bool,
    /// Pending agent setup request from `:agent-setup <name>` or `:agent-list`.
    /// The binary drains this and calls `agents::setup_agent()`.
    /// `Some("__list__")` is the sentinel for `:agent-list`.
    pub pending_agent_setup: Option<String>,
    /// Controls what keyboard input is allowed during AI/MCP operations.
    /// When not `None`, editor commands are blocked but shell input and
    /// navigation may still be allowed. Esc / Ctrl-C always cancel and
    /// release the lock.
    pub input_lock: InputLock,
    /// True while the AI session is actively streaming (text chunks or tool
    /// calls). Used to distinguish "AI thinking" from "idle but locked".
    pub ai_streaming: bool,
    /// Set to true when the user requests AI cancellation (e.g. via `ai-cancel` command).
    /// The event loop will read and reset this flag, sending the actual cancel command to the AI thread.
    pub ai_cancel_requested: bool,
    /// Last time the Escape key was pressed (for double-esc detection).
    pub last_esc_time: Option<std::time::Instant>,
    /// AI operating mode (manual, auto-accept, plan).
    pub ai_mode: String,
    /// Active prompt profile name.
    pub ai_profile: String,
    /// Current round in the AI tool loop.
    pub ai_current_round: usize,
    /// Current transaction start index in history.
    pub ai_transaction_start_idx: Option<usize>,
    /// AI's target buffer context. When set, buffer/LSP tools operate here
    /// instead of the human-focused active buffer. This allows the AI to
    /// edit files while the human watches the *AI* conversation.
    pub ai_target_buffer_idx: Option<usize>,
    /// AI's target window context. When set, cursor/scroll tools operate on
    /// this window instead of the focused window. Set via `set_ai_target` tool.
    pub ai_target_window_id: Option<crate::window::WindowId>,
    /// Linked output+input buffer pair for the split-view conversation UI.
    /// `None` until the user opens the conversation buffer.
    pub conversation_pair: Option<ConversationPair>,
    /// Window ID of the file tree sidebar, if open. Used to track and close it.
    pub file_tree_window_id: Option<crate::window::WindowId>,
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
    /// AI editor/agent command to launch in a shell (e.g. "claude", "aider").
    /// Used by `open-ai-agent` to spawn an agent shell.
    pub ai_editor: String,
    /// AI provider name: "claude", "openai", "gemini", "ollama", "deepseek".
    /// Set via `(set-option! "ai-provider" "deepseek")` or config.toml.
    pub ai_provider: String,
    /// AI model identifier. Empty = use provider default.
    pub ai_model: String,
    /// Scheme-registered AI tools (via `register-ai-tool!`).
    pub scheme_ai_tools: Vec<crate::SchemeToolDef>,
    /// Shell command whose stdout is the API key (e.g. "pass show deepseek/api-key").
    pub ai_api_key_command: String,
    /// Base URL override for the AI API.
    pub ai_base_url: String,
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
    /// Apply inline bold/italic/code styling in conversation/help buffers. Default true.
    pub render_markup: bool,
    /// Show hover info in a floating popup (true) or status bar (false). Default true.
    pub lsp_hover_popup: bool,
    /// Active hover popup (shown via K when lsp_hover_popup=true).
    pub hover_popup: Option<HoverPopup>,
    /// Active signature help popup (triggered on `(` and `,` in insert mode).
    pub signature_help: Option<SignatureHelpState>,
    /// Peek definition preview (shown via SPC l p).
    pub peek_state: Option<PeekState>,
    /// When true, the next GotoDefinition result goes to peek_state instead of jumping.
    pub peek_definition_pending: bool,
    /// Peek references state (SPC l r) — cycle through reference locations in a preview.
    pub peek_references: Option<PeekReferencesState>,
    /// When true, the next FindReferences result populates peek_references.
    pub peek_references_pending: bool,
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
    /// Symbol outline popup state (SPC c o).
    pub symbol_outline: Option<SymbolOutlineState>,
    /// Whether a document symbol request is pending for the outline popup.
    pub symbol_outline_pending: bool,
    /// Show breadcrumb bar (file > symbol ancestry). Default false.
    pub show_breadcrumbs: bool,
    /// Current breadcrumb path (file > module > fn).
    pub breadcrumbs: Option<Vec<String>>,
    /// Cached document symbols for breadcrumb computation (from last symbol request).
    pub cached_doc_symbols: Vec<SymbolOutlineEntry>,
    /// Buffer index the cached symbols belong to.
    pub cached_doc_symbols_buf: Option<usize>,
    /// Whether a document symbol request is pending for breadcrumbs (not outline popup).
    pub breadcrumb_symbols_pending: bool,
    /// Active code action menu (shown via SPC c a).
    pub code_action_menu: Option<CodeActionMenu>,
    /// Symbol occurrence highlights from `textDocument/documentHighlight`.
    /// Cleared on every cursor move; repopulated after idle timeout.
    pub highlight_ranges: Vec<DocumentHighlightRange>,
    /// Generation counter — incremented on cursor move to invalidate stale highlights.
    pub highlight_generation: u64,
    /// Last cursor position when a documentHighlight request was sent.
    /// Used to avoid duplicate requests when the cursor hasn't moved.
    pub highlight_last_pos: Option<(usize, usize)>,
    /// Pending block-visual insert: (min_row, max_row, min_col) saved when `I`
    /// is pressed in block visual mode. On insert-mode exit, the typed text is
    /// replicated to all rows in the range.
    pub pending_block_insert: Option<(usize, usize, usize)>,
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
    /// When true, dashboard windows are closed when any non-dashboard buffer
    /// is displayed via a split path. Default false (Doom parity: dashboard stays).
    pub dashboard_dismiss_on_split: bool,
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
    /// Whether an AI provider was successfully configured at startup.
    /// Set by `setup_ai()` in bootstrap.rs. Used by the UI layer to
    /// show guidance when the user tries to open an AI conversation
    /// without credentials.
    pub ai_configured: bool,
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
        Editor {
            buffers: vec![Buffer::new()],
            window_mgr: WindowManager::new(0),
            saved_maximize_layout: None,
            mode: Mode::Normal,
            running: true,
            status_msg: String::new(),
            current_command: String::new(),
            command_line: String::new(),
            commands,
            keymaps,
            which_key_prefix: Vec::new(),
            message_log: MessageLog::new(1000), // Max message log entries (internal bound)
            theme: default_theme(),
            debug_state: None,
            registers: HashMap::new(),
            pending_char_command: None,
            awaiting_key_description: false,
            conv_esc_pending: false,
            active_register: None,
            pending_register_prompt: false,
            pending_insert_register: false,
            insert_mode_oneshot_normal: false,
            pending_surround_from: None,
            search_state: SearchState::default(),
            search_input: String::new(),
            visual_anchor_row: 0,
            visual_anchor_col: 0,
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
            tab_completions: Vec::new(),
            tab_completion_idx: 0,
            last_edit: None,
            insert_start_offset: None,
            insert_initiated_by: None,
            last_insert_pos: None,
            jumps: Vec::new(),
            jump_idx: 0,
            changes: Vec::new(),
            change_idx: 0,
            count_prefix: None,
            pending_char_count: 1,
            alternate_buffer_idx: None,
            command_history: Vec::new(),
            command_history_idx: None,
            command_cursor: 0,
            pending_lsp_requests: Vec::new(),
            lsp_trigger_characters: std::collections::HashMap::new(),
            pending_lsp_root_change: None,
            pending_dap_intents: Vec::new(),
            pending_shell_spawns: Vec::new(),
            pending_shell_cwds: HashMap::new(),
            pending_agent_spawns: Vec::new(),
            pending_shell_resets: Vec::new(),
            pending_shell_closes: Vec::new(),
            pending_shell_inputs: Vec::new(),
            pending_shell_scroll: None,
            pending_shell_click: None,
            pending_shell_drag: None,
            pending_shell_release: None,
            pending_buffer_removals: Vec::new(),
            shell_viewports: HashMap::new(),
            shell_cwds: HashMap::new(),
            hooks,
            pending_hook_evals: Vec::new(),
            diagnostics: DiagnosticStore::default(),
            lsp_servers: HashMap::new(),
            syntax: crate::syntax::SyntaxMap::new(),
            syntax_reparse_pending: std::collections::HashSet::new(),
            last_edit_time: std::time::Instant::now(),
            syntax_selection_stack: Vec::new(),
            marks: HashMap::new(),
            completion_items: Vec::new(),
            completion_selected: 0,
            macro_recording: false,
            macro_register: None,
            macro_log: Vec::new(),
            last_macro_register: None,
            macro_replay_depth: 0,
            last_help_state: None,
            splash_art: Some("bat".to_string()),
            custom_splash_arts: Vec::new(),
            splash_image_width: 25,
            splash_image_height: 20,
            splash_show_logo: true,
            pending_operator: None,
            operator_start: None,
            operator_count: None,
            last_motion_linewise: false,
            pending_surround_range: None,
            last_find_char: None,
            last_visual: None,
            pending_scheme_eval: Vec::new(),
            kb,
            kb_registry: mae_kb::federation::KbRegistry::default(),
            kb_instances: HashMap::new(),
            kb_watchers: HashMap::new(),
            kb_last_drain: HashMap::new(),
            kb_watcher_stats: KbWatcherStats::default(),
            kb_watcher_enabled: true,
            kb_watcher_debounce_ms: 500,
            kb_max_drain_events: 100,
            kb_search_excerpt_length: 500,
            kb_search_max_results: 20,
            kb_auto_register: false,
            kb_ai_visited_ids: std::collections::HashSet::new(),
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
            ai_session_cost_usd: 0.0,
            ai_session_tokens_in: 0,
            ai_session_tokens_out: 0,
            ai_cache_read_tokens: 0,
            ai_cache_creation_tokens: 0,
            ai_context_window: 0,
            ai_context_used_tokens: 0,
            ai_last_api_success: None,
            ai_last_api_error: None,
            ai_last_api_latency_ms: None,
            ai_api_call_count: 0,
            ai_last_output_scroll: None,
            ai_work_window_id: None,
            ai_last_network_check: None,
            bell_until: None,
            project: None,
            git_branch: None,
            ai_permission_tier: "ReadOnly".to_string(),
            recent_files: crate::project::RecentFiles::default(),
            recent_projects: crate::project::RecentProjects::default(),
            show_line_numbers: true,
            relative_line_numbers: false,
            word_wrap: false,
            break_indent: true,
            show_break: "↪ ".to_string(),
            org_hide_emphasis_markers: false,
            pending_agent_setup: None,
            input_lock: InputLock::None,
            ai_streaming: false,
            ai_cancel_requested: false,
            last_esc_time: None,
            ai_mode: "standard".to_string(),
            ai_profile: "pair-programmer".to_string(),
            ai_current_round: 0,
            ai_transaction_start_idx: None,
            ai_target_buffer_idx: None,
            ai_target_window_id: None,
            conversation_pair: None,
            file_tree_window_id: None,
            file_tree_action: None,
            show_fps: false,
            renderer_name: "terminal".to_string(),
            gui_font_size: 14.0,
            gui_font_size_default: 14.0,
            gui_font_family: String::new(),
            gui_icon_font_family: String::new(),
            ai_editor: "claude".to_string(),
            ai_provider: String::new(),
            ai_model: String::new(),
            scheme_ai_tools: Vec::new(),
            ai_api_key_command: String::new(),
            ai_base_url: String::new(),
            option_registry: OptionRegistry::new(),
            splash_selection: 0,
            debug_mode: false,
            debug_init: false,
            clean_mode: false,
            perf_stats: perf::PerfStats::default(),
            clipboard: "unnamed".to_string(),
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
            hover_popup: None,
            signature_help: None,
            peek_state: None,
            peek_definition_pending: false,
            peek_references: None,
            peek_references_pending: false,
            blame_overlay: None,
            lsp_diagnostics_inline: true,
            lsp_diagnostics_virtual_text: true,
            lsp_completion: true,
            auto_complete: true,
            symbol_outline: None,
            symbol_outline_pending: false,
            show_breadcrumbs: false,
            breadcrumbs: None,
            cached_doc_symbols: Vec::new(),
            cached_doc_symbols_buf: None,
            breadcrumb_symbols_pending: false,
            code_action_menu: None,
            highlight_ranges: Vec::new(),
            highlight_generation: 0,
            highlight_last_pos: None,
            pending_block_insert: None,
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
            dashboard_dismiss_on_split: false,
            large_file_lines: 5_000,
            degrade_threshold_chars: 500_000,
            degrade_threshold_line_length: 10_000,
            display_region_debounce_ms: 150,
            syntax_reparse_debounce_ms: 50,
            markup_cache: HashMap::new(),
            code_block_cache: HashMap::new(),
            org_agenda_files: Vec::new(),
            ai_configured: false,
            active_modules: Vec::new(),
            module_binding_warnings: Vec::new(),
            pending_module_reloads: Vec::new(),
            pending_pkg_commands: Vec::new(),
            pending_git_diff: None,
        }
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
    pub fn current_keymap_names(&self) -> Option<(&'static str, Option<&'static str>)> {
        let idx = self.active_buffer_idx();
        let kind = self.buffers[idx].kind;
        let lang = self.syntax.language_of(idx);

        match self.mode {
            Mode::Normal => {
                // Buffer-kind overlay via BufferMode trait
                use crate::buffer_mode::BufferMode;
                if let Some(km_name) = kind.keymap_name() {
                    Some((km_name, Some("normal")))
                } else if lang == Some(Language::Org) {
                    // Language overlays stay hardcoded until Language::keymap_name() exists
                    Some(("org", Some("normal")))
                } else if lang == Some(Language::Markdown) {
                    Some(("markdown", Some("normal")))
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

    /// Merge which-key entries from the overlay keymap and its parent.
    fn merged_which_key_entries(&self, prefix: &[KeyPress]) -> Vec<WhichKeyEntry> {
        let Some((primary, fallback)) = self.current_keymap_names() else {
            return vec![];
        };
        let mut entries = self
            .keymaps
            .get(primary)
            .map(|km| km.which_key_entries(prefix, &self.commands))
            .unwrap_or_default();
        if let Some(fb_name) = fallback {
            if let Some(fb_km) = self.keymaps.get(fb_name) {
                let fb_entries = fb_km.which_key_entries(prefix, &self.commands);
                let existing: std::collections::HashSet<String> =
                    entries.iter().map(|e| format!("{:?}", e.key)).collect();
                for entry in fb_entries {
                    if !existing.contains(&format!("{:?}", entry.key)) {
                        entries.push(entry);
                    }
                }
            }
        }
        entries
    }

    /// Get which-key entries for the current keymap, merging overlay + parent.
    pub fn which_key_entries_for_current_keymap(&self) -> Vec<WhichKeyEntry> {
        self.merged_which_key_entries(&self.which_key_prefix)
    }

    /// Get all top-level bindings for the current buffer's keymap + parent.
    /// Used by `show-buffer-keys` (`?`) to show a full keybind reference.
    pub fn buffer_keys_entries(&self) -> Vec<WhichKeyEntry> {
        self.merged_which_key_entries(&[])
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
            self.visual_anchor_row = 0;
            self.visual_anchor_col = 0;
        } else {
            let max_row = line_count.saturating_sub(1);
            if self.visual_anchor_row > max_row {
                self.visual_anchor_row = max_row;
            }
            let max_col = self.buffers[idx].line_len(self.visual_anchor_row);
            if self.visual_anchor_col > max_col {
                self.visual_anchor_col = max_col;
            }
        }

        // Clamp last_visual so `gv` reselect never panics.
        if let Some((ref mut ar, ref mut ac, ref mut cr, ref mut cc, _)) = self.last_visual {
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
        if let Some(alt) = self.alternate_buffer_idx.as_mut() {
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
        self.ai_target_buffer_idx
            .unwrap_or_else(|| self.active_buffer_idx())
    }

    /// AI-aware cursor row: reads cursor from the AI target window if set,
    /// otherwise from the focused window.
    pub fn ai_cursor_row(&self) -> usize {
        if let Some(win_id) = self.ai_target_window_id {
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
            conversation_pair: self.conversation_pair.clone(),
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
                self.conversation_pair = Some(pair);
            } else {
                self.conversation_pair = None;
            }
        } else {
            self.conversation_pair = None;
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

    /// First conversation attached to any buffer, if any.
    pub fn conversation(&self) -> Option<&crate::conversation::Conversation> {
        self.buffers.iter().find_map(|b| b.conversation())
    }

    /// Mutable view of the first conversation attached to any buffer.
    pub fn conversation_mut(&mut self) -> Option<&mut crate::conversation::Conversation> {
        self.buffers.iter_mut().find_map(|b| b.conversation_mut())
    }

    /// Set the editor mode and fire the `mode-change` hook.
    pub fn set_mode(&mut self, mode: Mode) {
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
                return;
            }
        }
        if self.mode != mode {
            self.mode = mode;
            self.fire_hook("mode-change");
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

    /// Find or create the `*Help*` buffer and navigate it to `node_id`.
    /// Returns the buffer index. Does NOT switch focus — callers decide.
    pub fn ensure_help_buffer_idx(&mut self, node_id: &str) -> usize {
        if let Some(idx) = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Help)
        {
            if let Some(view) = self.buffers[idx].help_view_mut() {
                let v: &mut crate::help_view::HelpView = view;
                v.navigate_to(node_id.to_string());
            }
            return idx;
        }
        self.buffers.push(Buffer::new_help(node_id));
        self.buffers.len() - 1
    }

    /// Mutable view onto the help buffer's HelpView, if any help buffer exists.
    pub fn help_view_mut(&mut self) -> Option<&mut crate::help_view::HelpView> {
        self.buffers
            .iter_mut()
            .find(|b| b.kind == crate::buffer::BufferKind::Help)
            .and_then(|b| b.help_view_mut())
    }

    /// Immutable view onto the help buffer's HelpView, if any help buffer exists.
    pub fn help_view(&self) -> Option<&crate::help_view::HelpView> {
        self.buffers
            .iter()
            .find(|b| b.kind == crate::buffer::BufferKind::Help)
            .and_then(|b| b.help_view())
    }

    /// Switch the focused window to the buffer at the given index.
    /// Returns false if index is out of bounds.
    pub fn switch_to_buffer(&mut self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }
        let prev_idx = self.active_buffer_idx();
        if prev_idx != idx {
            self.alternate_buffer_idx = Some(prev_idx);
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
        if let Some(ref pair) = self.conversation_pair {
            if idx == pair.input_buffer_idx {
                return true;
            }
        }
        false
    }

    /// Switch to buffer `idx` but avoid stealing focus from a conversation window.
    ///
    /// If the focused window shows a conversation buffer, the new buffer is
    /// routed to another window (or a new split is created). This keeps `*AI*`
    /// Adjust `ai_target_buffer_idx` after a buffer at `removed_idx` was removed.
    /// Must be called after every `buffers.remove()` to prevent stale indices.
    pub fn adjust_ai_target_after_remove(&mut self, removed_idx: usize) {
        if let Some(ref mut target) = self.ai_target_buffer_idx {
            if *target == removed_idx {
                // The target buffer was removed — clear it
                self.ai_target_buffer_idx = None;
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
        rekey_after_remove(&mut self.shell_viewports, removed_idx);
        rekey_after_remove(&mut self.shell_cwds, removed_idx);
        rekey_after_remove(&mut self.pending_shell_cwds, removed_idx);

        // 3. Pending shell queues (Vec<usize> and Vec<(usize, _)>)
        self.pending_shell_spawns.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.pending_agent_spawns.retain_mut(|(idx, _)| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.pending_shell_resets.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.pending_shell_closes.retain_mut(|idx| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });
        self.pending_shell_inputs.retain_mut(|(idx, _)| {
            if *idx == removed_idx {
                return false;
            }
            if *idx > removed_idx {
                *idx -= 1;
            }
            true
        });

        // 4. Alternate buffer index
        if let Some(ref mut alt) = self.alternate_buffer_idx {
            if *alt == removed_idx {
                self.alternate_buffer_idx = None;
            } else if *alt > removed_idx {
                *alt -= 1;
            }
        }

        // 5. Per-window saved_view_states
        for win in self.window_mgr.iter_windows_mut() {
            rekey_after_remove(&mut win.saved_view_states, removed_idx);
        }

        // 6. Signal the binary to rekey its own maps
        self.pending_buffer_removals.push(removed_idx);
    }

    /// visible during AI tool calls that open/switch files.
    pub fn switch_to_buffer_non_conversation(&mut self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }

        self.ai_target_buffer_idx = Some(idx);

        // 0. Reuse the dedicated AI work window if it exists and is still valid.
        if let Some(work_id) = self.ai_work_window_id {
            if self.window_mgr.window(work_id).is_some() {
                if let Some(win) = self.window_mgr.window_mut(work_id) {
                    win.buffer_idx = idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
                self.ai_target_window_id = Some(work_id);
                self.mark_full_redraw();
                return true;
            } else {
                self.ai_work_window_id = None; // stale reference
            }
        }

        // 1. Is this buffer already visible?
        if self.window_mgr.iter_windows().any(|w| w.buffer_idx == idx) {
            return true;
        }

        // 2. Can we put it in a non-focused window that isn't a conversation?
        let focused_id = self.window_mgr.focused_id();
        let other = self
            .window_mgr
            .iter_windows()
            .find(|w| w.id != focused_id && !self.is_conversation_buffer(w.buffer_idx))
            .map(|w| w.id);

        if let Some(other_id) = other {
            if let Some(win) = self.window_mgr.window_mut(other_id) {
                win.buffer_idx = idx;
                win.cursor_row = 0;
                win.cursor_col = 0;
            }
            self.ai_work_window_id = Some(other_id);
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
            } else if let Some(ref pair) = self.conversation_pair {
                // All windows are conversation — steal the output window temporarily
                // instead of splitting (which creates narrow panes beside the
                // conversation group). The output window is restored on session end.
                if let Some(win) = self.window_mgr.window_mut(pair.output_window_id) {
                    win.buffer_idx = idx;
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                }
                self.ai_work_window_id = Some(pair.output_window_id);
                self.mark_full_redraw();
                return true;
            }
        }
        let area = self.default_area();
        match self
            .window_mgr
            .split(crate::window::SplitDirection::Vertical, idx, area)
        {
            Ok(new_id) => {
                self.ai_work_window_id = Some(new_id);
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
                } else if self.dashboard_dismiss_on_split && kind != crate::BufferKind::Dashboard {
                    // Replace dashboard windows instead of splitting alongside them.
                    let dashboard_win = self
                        .window_mgr
                        .iter_windows()
                        .find(|w| {
                            w.buffer_idx < self.buffers.len()
                                && self.buffers[w.buffer_idx].kind == crate::BufferKind::Dashboard
                        })
                        .map(|w| w.id);
                    if let Some(dw_id) = dashboard_win {
                        // Replace the dashboard window's buffer directly.
                        if let Some(win) = self.window_mgr.window_mut(dw_id) {
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
        if prev_idx != buf_idx {
            self.alternate_buffer_idx = Some(prev_idx);
        }
        self.sync_mode_to_buffer();
    }

    /// Find a window showing a buffer of the given kind (non-conversation).
    /// Excludes windows that are part of the conversation pair (output/input).
    fn find_window_with_kind(&self, kind: crate::BufferKind) -> Option<crate::window::WindowId> {
        let conv_ids = self
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
        self.ai_cancel_requested = true;
        self.ai_streaming = false;
        self.ai_current_round = 0;
        self.ai_transaction_start_idx = None;
        if let Some(conv) = self.conversation_mut() {
            conv.end_streaming();
            conv.push_system("[AI Session Reset]");
        }
        self.input_lock = crate::InputLock::None;
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
        self.count_prefix.take().unwrap_or(1)
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
