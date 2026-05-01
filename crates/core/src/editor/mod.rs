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
mod help_ops;
mod hook_ops;
mod jumps;
mod keymaps;
mod lsp_ops;
mod macros;
mod marks;
pub mod perf;
mod project_ops;
mod register_ops;
mod scheme_ops;
mod search_ops;
mod surround;
mod syntax_ops;
mod text_objects;
mod visual;

pub use changes::{ChangeEntry, CHANGE_LIST_CAP};
pub use diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticStore};
pub use jumps::{JumpEntry, JUMP_LIST_CAP};
pub use lsp_ops::{LspLocation, LspRange};
pub use marks::Mark;

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::buffer::Buffer;
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
use crate::options::{parse_option_bool, OptionKind, OptionRegistry};
use crate::search::SearchState;
use crate::syntax::Language;
use crate::theme::{default_theme, Theme};
use crate::window::{Rect, WindowId, WindowManager};
use crate::Mode;

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
    /// Text area width in columns (after gutter), updated each frame.
    /// Used by word-wrap aware cursor movement (gj/gk).
    pub text_area_width: usize,
    /// Fuzzy file picker state. Some when the picker overlay is active.
    pub file_picker: Option<FilePicker>,
    /// Ranger-style directory browser. Some when the browser overlay is active.
    pub file_browser: Option<crate::FileBrowser>,
    /// Fuzzy command palette state. Some when the palette overlay is active.
    pub command_palette: Option<CommandPalette>,
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
    /// Queue of pending DAP requests for the binary to drain each event-loop tick.
    /// Same pattern as `pending_lsp_requests`: core cannot call async DAP code
    /// directly; commands push intents here and `main.rs` forwards them to
    /// `run_dap_task`.
    pub pending_dap_intents: Vec<DapIntent>,
    /// Buffer indices of newly created shell buffers that need PTY spawning.
    /// The binary drains this and creates `ShellTerminal` instances.
    pub pending_shell_spawns: Vec<usize>,
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
    /// LSP server connection status, keyed by language_id.
    pub lsp_servers: HashMap<String, LspServerStatus>,
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
    /// Saved help view state from the last `help_close`. `help-reopen`
    /// restores this to resume exactly where the user left off.
    pub last_help_state: Option<crate::help_view::HelpView>,
    /// Which ASCII art to show on the splash screen. Default is "bat".
    pub splash_art: Option<String>,
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
    /// Linked output+input buffer pair for the split-view conversation UI.
    /// `None` until the user opens the conversation buffer.
    pub conversation_pair: Option<ConversationPair>,
    /// Window ID of the file tree sidebar, if open. Used to track and close it.
    pub file_tree_window_id: Option<crate::window::WindowId>,
    /// Pending file deletion: (path, close_buffer_after_delete).
    /// Set by `file-tree-delete` or `delete-this-file`, consumed by `y`/`n` key handler.
    pub pending_file_delete: Option<(std::path::PathBuf, bool)>,
    /// Pending file tree action (rename/create). The command-line submit
    /// path checks this after the user types a new name.
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
    pub scrollbar: bool,
    pub nyan_mode: bool,
    /// Show link labels instead of raw markup (Emacs org-link-descriptive). Default true.
    pub link_descriptive: bool,
    /// Apply inline bold/italic/code styling in conversation/help buffers. Default true.
    pub render_markup: bool,
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
            command_line: String::new(),
            commands,
            keymaps,
            which_key_prefix: Vec::new(),
            message_log: MessageLog::new(1000),
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
            text_area_width: 80,
            file_picker: None,
            file_browser: None,
            command_palette: None,
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
            pending_dap_intents: Vec::new(),
            pending_shell_spawns: Vec::new(),
            pending_agent_spawns: Vec::new(),
            pending_shell_resets: Vec::new(),
            pending_shell_closes: Vec::new(),
            pending_shell_inputs: Vec::new(),
            pending_shell_scroll: None,
            pending_shell_click: None,
            pending_shell_drag: None,
            pending_shell_release: None,
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
            pending_operator: None,
            operator_start: None,
            operator_count: None,
            last_motion_linewise: false,
            pending_surround_range: None,
            last_find_char: None,
            last_visual: None,
            pending_scheme_eval: Vec::new(),
            kb,
            ai_session_cost_usd: 0.0,
            ai_session_tokens_in: 0,
            ai_session_tokens_out: 0,
            ai_cache_read_tokens: 0,
            ai_cache_creation_tokens: 0,
            ai_context_window: 0,
            ai_context_used_tokens: 0,
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
            conversation_pair: None,
            file_tree_window_id: None,
            pending_file_delete: None,
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
            ai_api_key_command: String::new(),
            ai_base_url: String::new(),
            option_registry: OptionRegistry::new(),
            splash_selection: 0,
            debug_mode: false,
            perf_stats: perf::PerfStats::default(),
            clipboard: "unnamed".to_string(),
            restore_session: false,
            insert_ctrl_d: "dedent".to_string(),
            heading_scale: true,
            ignorecase: false,
            smartcase: false,
            scrollbar: true,
            nyan_mode: false,
            link_descriptive: true,
            render_markup: true,
            pending_block_insert: None,
            heartbeat: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            watchdog_stall_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            watchdog_stall_recovery: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            event_recorder: crate::event_record::EventRecorder::new(),
            state_stack: Vec::new(),
            self_test_active: false,
            last_autosave: std::time::Instant::now(),
            autosave_interval: 0,
            swap_file: true,
            swap_directory: String::new(),
            buffer_keys_popup: false,
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

    /// Returns the active buffer's project root, falling back to the editor-wide project root.
    pub fn active_project_root(&self) -> Option<&std::path::Path> {
        let buf = self.active_buffer();
        if let Some(root) = &buf.project_root {
            return Some(root.as_path());
        }
        self.project.as_ref().map(|p| p.root.as_path())
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

    /// Set a buffer-local option on the active buffer (:setlocal).
    pub fn set_local_option(&mut self, name: &str, value: &str) -> Result<String, String> {
        let def_name = self
            .option_registry
            .find(name)
            .map(|d| d.name)
            .ok_or_else(|| format!("Unknown option: {}", name))?;
        let idx = self.active_buffer_idx();
        let opts = &mut self.buffers[idx].local_options;
        match def_name {
            "word_wrap" => {
                opts.word_wrap = Some(crate::options::parse_option_bool(value)?);
            }
            "line_numbers" => {
                opts.line_numbers = Some(crate::options::parse_option_bool(value)?);
            }
            "relative_line_numbers" => {
                opts.relative_line_numbers = Some(crate::options::parse_option_bool(value)?);
            }
            "break_indent" => {
                opts.break_indent = Some(crate::options::parse_option_bool(value)?);
            }
            "show_break" => {
                opts.show_break = Some(value.to_string());
            }
            "heading_scale" => {
                opts.heading_scale = Some(crate::options::parse_option_bool(value)?);
            }
            "link_descriptive" => {
                opts.link_descriptive = Some(crate::options::parse_option_bool(value)?);
            }
            "render_markup" => {
                opts.render_markup = Some(crate::options::parse_option_bool(value)?);
            }
            _ => {
                return Err(format!(
                    "Option '{}' does not support buffer-local override",
                    def_name
                ))
            }
        }
        Ok(format!("{} = {} (buffer-local)", def_name, value))
    }

    /// Get the current value and definition of an option by name or alias.
    pub fn get_option(&self, name: &str) -> Option<(String, &crate::options::OptionDef)> {
        let def = self.option_registry.find(name)?;
        let value = match def.name {
            "line_numbers" => self.show_line_numbers.to_string(),
            "relative_line_numbers" => self.relative_line_numbers.to_string(),
            "word_wrap" => self.word_wrap.to_string(),
            "break_indent" => self.break_indent.to_string(),
            "show_break" => self.show_break.clone(),
            "org_hide_emphasis_markers" => self.org_hide_emphasis_markers.to_string(),
            "show_fps" => self.show_fps.to_string(),
            "font_size" => self.gui_font_size.to_string(),
            "font_family" => self.gui_font_family.clone(),
            "icon_font_family" => self.gui_icon_font_family.clone(),
            "theme" => self.theme.name.clone(),
            "splash_art" => self.splash_art.clone().unwrap_or_default(),
            "debug_mode" => self.debug_mode.to_string(),
            "clipboard" => self.clipboard.clone(),
            "ai_tier" => self.ai_permission_tier.clone(),
            "ai_editor" => self.ai_editor.clone(),
            "ai_provider" => self.ai_provider.clone(),
            "ai_model" => self.ai_model.clone(),
            "ai_api_key_command" => self.ai_api_key_command.clone(),
            "ai_base_url" => self.ai_base_url.clone(),
            "ai_mode" => self.ai_mode.clone(),
            "ai_profile" => self.ai_profile.clone(),
            "restore_session" => self.restore_session.to_string(),
            "insert_ctrl_d" => self.insert_ctrl_d.clone(),
            "heading_scale" => self.heading_scale.to_string(),
            "ignorecase" => self.ignorecase.to_string(),
            "smartcase" => self.smartcase.to_string(),
            "autosave_interval" => self.autosave_interval.to_string(),
            "swap_file" => self.swap_file.to_string(),
            "swap_directory" => self.swap_directory.clone(),
            "scrollbar" => self.scrollbar.to_string(),
            "nyan_mode" => self.nyan_mode.to_string(),
            "link_descriptive" => self.link_descriptive.to_string(),
            "render_markup" => self.render_markup.to_string(),
            _ => return None,
        };
        Some((value, def))
    }

    /// Set an option by name or alias, returning a confirmation message.
    pub fn set_option(&mut self, name: &str, value: &str) -> Result<String, String> {
        let def_name = self
            .option_registry
            .find(name)
            .map(|d| d.name)
            .ok_or_else(|| format!("Unknown option: {}", name))?;
        match def_name {
            "line_numbers" => {
                self.show_line_numbers = parse_option_bool(value)?;
            }
            "relative_line_numbers" => {
                self.relative_line_numbers = parse_option_bool(value)?;
            }
            "word_wrap" => {
                self.word_wrap = parse_option_bool(value)?;
            }
            "break_indent" => {
                self.break_indent = parse_option_bool(value)?;
            }
            "show_break" => {
                self.show_break = value.to_string();
            }
            "org_hide_emphasis_markers" => {
                self.org_hide_emphasis_markers = parse_option_bool(value)?;
            }
            "show_fps" => {
                self.show_fps = parse_option_bool(value)?;
            }
            "font_size" => {
                let size: f32 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                if !(6.0..=72.0).contains(&size) {
                    return Err("Font size must be between 6 and 72".into());
                }
                self.gui_font_size = size;
            }
            "font_family" => {
                self.gui_font_family = value.to_string();
            }
            "icon_font_family" => {
                self.gui_icon_font_family = value.to_string();
            }
            "theme" => {
                self.set_theme_by_name(value);
            }
            "splash_art" => {
                self.splash_art = Some(value.to_string());
            }
            "debug_mode" => {
                self.debug_mode = parse_option_bool(value)?;
                if self.debug_mode {
                    self.show_fps = true;
                }
            }
            "clipboard" => match value {
                "unnamedplus" | "unnamed" | "internal" => {
                    self.clipboard = value.to_string();
                }
                _ => {
                    return Err(format!(
                        "Invalid clipboard mode: '{}' (expected unnamedplus, unnamed, or internal)",
                        value
                    ))
                }
            },
            "ai_tier" => match value {
                "ReadOnly" | "Write" | "Shell" | "Privileged" => {
                    self.ai_permission_tier = value.to_string();
                }
                _ => {
                    return Err(format!(
                        "Invalid AI tier: '{}' (expected ReadOnly, Write, Shell, or Privileged)",
                        value
                    ))
                }
            },
            "ai_editor" => {
                self.ai_editor = value.to_string();
            }
            "ai_provider" => {
                self.ai_provider = value.to_string();
            }
            "ai_model" => {
                self.ai_model = value.to_string();
            }
            "ai_api_key_command" => {
                self.ai_api_key_command = value.to_string();
            }
            "ai_base_url" => {
                self.ai_base_url = value.to_string();
            }
            "ai_mode" => {
                let valid = ["standard", "plan", "auto-accept"];
                if !valid.contains(&value) {
                    return Err(format!(
                        "Invalid AI mode: '{}' (expected: standard, plan, auto-accept)",
                        value
                    ));
                }
                self.ai_mode = value.to_string();
            }
            "ai_profile" => {
                self.ai_profile = value.to_string();
            }
            "restore_session" => {
                self.restore_session = parse_option_bool(value)?;
            }
            "insert_ctrl_d" => {
                let valid = ["dedent", "delete-forward"];
                if !valid.contains(&value) {
                    return Err(format!(
                        "Invalid insert_ctrl_d: '{}' (expected: dedent, delete-forward)",
                        value
                    ));
                }
                self.insert_ctrl_d = value.to_string();
            }
            "heading_scale" => {
                self.heading_scale = parse_option_bool(value)?;
            }
            "ignorecase" => {
                self.ignorecase = parse_option_bool(value)?;
            }
            "smartcase" => {
                self.smartcase = parse_option_bool(value)?;
            }
            "autosave_interval" => {
                let secs: u64 = value
                    .parse()
                    .map_err(|_| format!("Invalid integer: '{}'", value))?;
                self.autosave_interval = secs;
            }
            "swap_file" => {
                self.swap_file = parse_option_bool(value)?;
            }
            "swap_directory" => {
                self.swap_directory = value.to_string();
            }
            "scrollbar" => {
                self.scrollbar = parse_option_bool(value)?;
            }
            "nyan_mode" => {
                self.nyan_mode = parse_option_bool(value)?;
            }
            "link_descriptive" => {
                self.link_descriptive = parse_option_bool(value)?;
            }
            "render_markup" => {
                self.render_markup = parse_option_bool(value)?;
            }
            _ => return Err(format!("Unknown option: {}", name)),
        }
        let (current, _) = self
            .get_option(def_name)
            .ok_or_else(|| format!("internal: option '{}' not found after set", def_name))?;
        Ok(format!("{} = {}", def_name, current))
    }

    /// Persist an option's current value to `~/.config/mae/config.toml`.
    pub fn save_option_to_config(&self, name: &str) -> Result<String, String> {
        let (value, def) = self
            .get_option(name)
            .ok_or_else(|| format!("Unknown option: {}", name))?;
        let config_key = def
            .config_key
            .ok_or_else(|| format!("Option '{}' cannot be saved to config", def.name))?;

        let config_dir = if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            std::path::PathBuf::from(xdg).join("mae")
        } else if let Ok(home) = std::env::var("HOME") {
            std::path::PathBuf::from(home).join(".config").join("mae")
        } else {
            return Err("Cannot determine config directory".into());
        };
        let config_path = config_dir.join("config.toml");

        // Read existing config as a TOML table, or start fresh
        let mut table: toml::Table = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| format!("Failed to read config: {}", e))?;
            content
                .parse::<toml::Table>()
                .map_err(|e| format!("Failed to parse config: {}", e))?
        } else {
            std::fs::create_dir_all(&config_dir)
                .map_err(|e| format!("Failed to create config dir: {}", e))?;
            toml::Table::new()
        };

        // Parse config_key like "editor.line_numbers" into section + key
        let parts: Vec<&str> = config_key.splitn(2, '.').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid config key: {}", config_key));
        }
        let (section_name, key_name) = (parts[0], parts[1]);

        // Ensure the section table exists
        if !table.contains_key(section_name) {
            table.insert(
                section_name.to_string(),
                toml::Value::Table(toml::Table::new()),
            );
        }
        let section = table
            .get_mut(section_name)
            .and_then(|v| v.as_table_mut())
            .ok_or_else(|| format!("Config key '{}' is not a table", section_name))?;

        // Set the value with the appropriate TOML type, validating the parse.
        let toml_val = match def.kind {
            OptionKind::Bool => {
                let b: bool = value
                    .parse()
                    .map_err(|_| format!("Invalid bool: '{}'", value))?;
                toml::Value::Boolean(b)
            }
            OptionKind::Float => {
                let f: f64 = value
                    .parse()
                    .map_err(|_| format!("Invalid float: '{}'", value))?;
                toml::Value::Float(f)
            }
            OptionKind::String | OptionKind::Theme => toml::Value::String(value.clone()),
        };
        section.insert(key_name.to_string(), toml_val);

        let output = toml::to_string_pretty(&table)
            .map_err(|e| format!("Failed to serialize config: {}", e))?;
        std::fs::write(&config_path, output)
            .map_err(|e| format!("Failed to write config: {}", e))?;

        Ok(format!(
            "Saved {} = {} to {}",
            def.name,
            value,
            config_path.display()
        ))
    }

    /// Open a scratch `*Options*` buffer listing all options with current values.
    pub fn show_all_options(&mut self) {
        let mut lines = Vec::new();
        lines.push("Editor Options".to_string());
        lines.push("==============".to_string());
        lines.push(String::new());
        lines.push(format!(
            "{:<25} {:<10} {:<15} {}",
            "Option", "Type", "Current", "Default"
        ));
        lines.push(format!(
            "{:<25} {:<10} {:<15} {}",
            "------", "----", "-------", "-------"
        ));
        for def in self.option_registry.list() {
            let current = match self.get_option(def.name) {
                Some((v, _)) => v,
                None => "?".to_string(),
            };
            lines.push(format!(
                "{:<25} {:<10} {:<15} {}",
                def.name, def.kind, current, def.default_value
            ));
        }
        lines.push(String::new());
        lines.push(
            "Use :set <option> <value> to change, :set <option> to toggle booleans.".to_string(),
        );
        lines.push("Use :set-save <option> [value] to persist to config.toml.".to_string());
        lines.push("Use :describe-option <name> or SPC h o for documentation.".to_string());

        let content = lines.join("\n");
        let mut buf = crate::buffer::Buffer::new();
        buf.name = "*Options*".to_string();
        buf.replace_contents(&content);
        buf.modified = false;
        buf.read_only = true;

        let buf_idx = self.buffers.len();
        self.buffers.push(buf);
        self.window_mgr.focused_window_mut().buffer_idx = buf_idx;
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
        let line_count = self.buffers[idx].line_count();
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
        if self.mode != mode {
            self.mode = mode;
            self.fire_hook("mode-change");
        }
    }

    /// Sync the rope of the first buffer containing a conversation.
    pub fn sync_conversation_buffer_rope(&mut self) {
        if let Some(buf) = self
            .buffers
            .iter_mut()
            .find(|b| b.kind == crate::buffer::BufferKind::Conversation)
        {
            buf.sync_conversation_rope();
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
        // Check for external file changes before showing the buffer.
        self.check_and_reload_buffer(idx);
        let win = self.window_mgr.focused_window_mut();
        win.buffer_idx = idx;
        win.cursor_row = 0;
        win.cursor_col = 0;
        // Recompute search matches for the new buffer so highlights and
        // `n`/`N` navigation are correct.
        self.recompute_search_matches();
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

    /// visible during AI tool calls that open/switch files.
    pub fn switch_to_buffer_non_conversation(&mut self, idx: usize) -> bool {
        if idx >= self.buffers.len() {
            return false;
        }

        self.ai_target_buffer_idx = Some(idx);

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
                // All windows are conversation — split from the output window (larger pane).
                self.window_mgr.set_focused(pair.output_window_id);
            }
        }
        let area = self.default_area();
        match self
            .window_mgr
            .split(crate::window::SplitDirection::Vertical, idx, area)
        {
            Ok(_new_id) => true,
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
        let rope = buf.rope();
        if line >= rope.len_lines() {
            return 1;
        }
        let chars: Vec<char> = rope
            .line(line)
            .chars()
            .filter(|c| *c != '\n' && *c != '\r')
            .collect();

        let heading_rows = crate::heading::line_heading_visual_rows(&chars, self.heading_scale);

        if self.word_wrap_for(buf_idx) && self.text_area_width > 0 {
            let text: String = chars.iter().collect();
            let sb_w = self.show_break.chars().count();
            crate::wrap::wrap_line_display_rows(
                &text,
                self.text_area_width,
                self.break_indent,
                sb_w,
            )
            .max(heading_rows)
        } else {
            heading_rows
        }
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

    pub fn handle_mouse_click(
        &mut self,
        row: usize,
        col: usize,
        button: crate::input::MouseButton,
    ) {
        use crate::input::MouseButton;

        // Shell buffers: route to pending_shell_click for the binary to drain.
        // Subtract window border offset (1 row top, 1 col left).
        let active = self.active_buffer_idx();
        if self.buffers[active].kind == crate::BufferKind::Shell {
            let shell_row = row.saturating_sub(1);
            let shell_col = col.saturating_sub(1);
            self.pending_shell_click = Some((shell_row, shell_col, button));
            return;
        }

        match button {
            MouseButton::Left => {
                // Place cursor at clicked position, adjusting for gutter and scroll.
                let win = self.window_mgr.focused_window();
                let buf = &self.buffers[win.buffer_idx];
                let line_count = buf.rope().len_lines();
                let digits = if line_count == 0 {
                    1
                } else {
                    (line_count as f64).log10().floor() as usize + 1
                };
                let gutter_width = if self.show_line_numbers {
                    digits.max(2) + 1
                } else {
                    0
                };
                if col < gutter_width {
                    return; // Clicked in gutter, ignore
                }
                let text_col = col.saturating_sub(gutter_width);
                // row 0 is the window border in GUI mode; buffer content starts at row 1.
                let buf_row = win.scroll_offset + row.saturating_sub(1);
                let max_row = line_count.saturating_sub(1);
                let target_row = buf_row.min(max_row);

                // Check for link click: first try pre-populated link_spans,
                // then fall back to on-the-fly detection on the clicked line.
                if !buf.link_spans.is_empty() {
                    let line_start_byte =
                        buf.rope().char_to_byte(buf.rope().line_to_char(target_row));
                    let click_byte = line_start_byte + text_col;
                    if let Some(link) = buf
                        .link_spans
                        .iter()
                        .find(|s| click_byte >= s.byte_start && click_byte < s.byte_end)
                    {
                        let target = link.target.clone();
                        self.handle_link_click(&target);
                        return;
                    }
                }
                // On-the-fly link detection for buffers without pre-populated spans
                // (conversation, shell, text).
                if target_row < buf.rope().len_lines() {
                    let line_text: String = buf.rope().line(target_row).chars().collect();
                    let links = crate::link_detect::detect_links(&line_text);
                    for link in &links {
                        let link_char_start = line_text[..link.byte_start].chars().count();
                        let link_char_end = line_text[..link.byte_end].chars().count();
                        if text_col >= link_char_start && text_col < link_char_end {
                            let target = link.target.clone();
                            self.handle_link_click(&target);
                            return;
                        }
                    }
                }

                let line_len = buf.line_len(target_row);
                let target_col = text_col.min(if line_len > 0 { line_len - 1 } else { 0 });
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row = target_row;
                win.cursor_col = target_col;
            }
            MouseButton::Right => {
                // Right-click could open context menu in the future.
            }
            MouseButton::Middle => {
                // Middle-click paste from default register.
                self.dispatch_builtin("paste-after");
            }
        }
    }

    /// Handle a link click: open file paths in the editor, URLs externally.
    fn handle_link_click(&mut self, target: &str) {
        if target.starts_with("http://") || target.starts_with("https://") {
            // Open URL externally
            let _ = std::process::Command::new("xdg-open").arg(target).spawn();
            self.set_status(format!("Opening {}", target));
        } else {
            // Parse :line:col suffix from file paths
            let (path, line, col) = file_ops::parse_file_link(target);
            self.open_file(path);
            // Navigate to line:col if specified
            if let Some(ln) = line {
                let buf = &self.buffers[self.active_buffer_idx()];
                let target_row = ln.saturating_sub(1).min(buf.line_count().saturating_sub(1));
                let target_col = col.unwrap_or(1).saturating_sub(1);
                let win = self.window_mgr.focused_window_mut();
                win.cursor_row = target_row;
                win.cursor_col = target_col;
            }
        }
    }

    /// Handle mouse drag — update cursor position and enter/update Visual mode.
    ///
    /// On first drag event, the click position becomes the visual anchor.
    /// Subsequent drag events update the cursor, extending the selection.
    pub fn handle_mouse_drag(&mut self, row: usize, col: usize) {
        // Shell buffers: route to pending_shell_drag for selection update.
        let active = self.active_buffer_idx();
        if self.buffers[active].kind == crate::BufferKind::Shell {
            let shell_row = row.saturating_sub(1);
            let shell_col = col.saturating_sub(1);
            self.pending_shell_drag = Some((shell_row, shell_col));
            return;
        }

        let win = self.window_mgr.focused_window();
        let buf = &self.buffers[win.buffer_idx];
        let line_count = buf.rope().len_lines();
        let digits = if line_count == 0 {
            1
        } else {
            (line_count as f64).log10().floor() as usize + 1
        };
        let gutter_width = if self.show_line_numbers {
            digits.max(2) + 1
        } else {
            0
        };
        let text_col = col.saturating_sub(gutter_width);
        let buf_row = win.scroll_offset + row.saturating_sub(1);
        let max_row = buf.display_line_count().saturating_sub(1);
        let target_row = buf_row.min(max_row);
        let line_len = buf.line_len(target_row);
        let target_col = text_col.min(if line_len > 0 { line_len - 1 } else { 0 });

        // Enter Visual mode on first drag if not already in it.
        if !matches!(self.mode, crate::Mode::Visual(_)) {
            // Anchor at current cursor position (the click position).
            let win = self.window_mgr.focused_window();
            self.visual_anchor_row = win.cursor_row;
            self.visual_anchor_col = win.cursor_col;
            self.set_mode(crate::Mode::Visual(crate::VisualType::Char));
        }

        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = target_row;
        win.cursor_col = target_col;
    }

    /// Handle mouse button release at the given cell coordinates.
    ///
    /// For shell buffers, finalizes text selection and copies to registers.
    /// For text buffers, this is a no-op (Visual mode persists until Esc).
    pub fn handle_mouse_release(&mut self, row: usize, col: usize) {
        let active = self.active_buffer_idx();
        if self.buffers[active].kind == crate::BufferKind::Shell {
            let shell_row = row.saturating_sub(1);
            let shell_col = col.saturating_sub(1);
            self.pending_shell_release = Some((shell_row, shell_col));
        }
    }

    /// Handle horizontal mouse scroll (positive = right, negative = left).
    ///
    /// Adjusts col_offset directly. Only applies to normal file buffers.
    /// Clamped so the rightmost character is still visible.
    pub fn handle_mouse_scroll_horizontal(&mut self, delta: i16) {
        let cols = delta.unsigned_abs() as usize;
        if cols == 0 {
            return;
        }
        let scroll_speed = 3;
        let buf_idx = self.active_buffer_idx();
        let kind = self.buffers[buf_idx].kind;
        if kind != crate::BufferKind::Text {
            return;
        }

        // Find the longest visible line to clamp horizontal scroll.
        // Only scan the viewport region to avoid O(n) on large files.
        let buf = &self.buffers[buf_idx];
        let max_line_width = {
            let rope = buf.rope();
            let total = rope.len_lines();
            let win = self.window_mgr.focused_window();
            let start = win.scroll_offset;
            let end = (start + self.viewport_height + 1).min(total);
            let mut max_w = 0usize;
            for i in start..end {
                let line = rope.line(i);
                let w = line.chars().filter(|c| *c != '\n' && *c != '\r').count();
                if w > max_w {
                    max_w = w;
                }
            }
            max_w
        };

        let win = self.window_mgr.focused_window_mut();
        if delta > 0 {
            win.col_offset = win.col_offset.saturating_add(cols * scroll_speed);
        } else {
            win.col_offset = win.col_offset.saturating_sub(cols * scroll_speed);
        }
        // Clamp: don't scroll past the rightmost character.
        let max_offset = max_line_width.saturating_sub(1);
        if win.col_offset > max_offset {
            win.col_offset = max_offset;
        }
    }

    /// Handle mouse scroll (positive = up, negative = down).
    ///
    /// Vim-style: scroll moves the viewport and clamps the cursor into the
    /// visible area, so `ensure_scroll` on the next frame is a no-op.
    pub fn handle_mouse_scroll(&mut self, delta: i16) {
        let lines = delta.unsigned_abs() as usize;
        if lines == 0 {
            return;
        }
        let scroll_speed = 3;
        let buf_idx = self.active_buffer_idx();
        let kind = self.buffers[buf_idx].kind;

        match kind {
            crate::BufferKind::Conversation => {
                // Conversation buffers use win.scroll_offset (rope line index)
                // via the standard FrameLayout pipeline.
                let total = self.buffers[buf_idx].display_line_count();
                let vh = self.viewport_height;
                let amount = lines * scroll_speed;
                let win = self.window_mgr.focused_window_mut();
                if delta > 0 {
                    win.scroll_offset = win.scroll_offset.saturating_sub(amount);
                } else {
                    let max = total.saturating_sub(vh);
                    win.scroll_offset = (win.scroll_offset + amount).min(max);
                }
            }
            crate::BufferKind::Shell => {
                let amount = if delta > 0 {
                    lines as i32 * scroll_speed as i32
                } else {
                    -(lines as i32 * scroll_speed as i32)
                };
                self.pending_shell_scroll = Some(amount);
            }
            crate::BufferKind::Messages => {
                let total = self.message_log.len();
                let vh = self.viewport_height;
                let win = self.window_mgr.focused_window_mut();
                if delta > 0 {
                    win.scroll_offset = win.scroll_offset.saturating_sub(lines * scroll_speed);
                } else {
                    let max = total.saturating_sub(vh);
                    win.scroll_offset = (win.scroll_offset + lines * scroll_speed).min(max);
                }
            }
            _ => {
                let buf_line_count = self.buffers[buf_idx].display_line_count();
                let viewport_height = self.viewport_height;

                // Phase 1: Fold-aware scroll stepping (needs &mut win + &buf).
                {
                    let buf = &self.buffers[buf_idx];
                    let win = self.window_mgr.focused_window_mut();
                    let steps = lines * scroll_speed;
                    if delta > 0 {
                        for _ in 0..steps {
                            let prev = buf.prev_visible_line(win.scroll_offset);
                            if prev >= win.scroll_offset {
                                break;
                            }
                            win.scroll_offset = prev;
                        }
                    } else {
                        let max_scroll = buf_line_count.saturating_sub(viewport_height);
                        for _ in 0..steps {
                            if win.scroll_offset >= max_scroll {
                                break;
                            }
                            let next = buf.next_visible_line(win.scroll_offset);
                            if next <= win.scroll_offset {
                                break;
                            }
                            win.scroll_offset = next.min(max_scroll);
                        }
                    }
                }

                // Phase 2: Compute bottom visible row using canonical line_visual_rows.
                // (needs &self for line_visual_rows — no mutable borrow active)
                let scroll_off = self.window_mgr.focused_window().scroll_offset;
                let bottom = {
                    let buf = &self.buffers[buf_idx];
                    let max_row = buf_line_count.saturating_sub(1);
                    let mut visual = 0;
                    let mut last_fit = scroll_off;
                    let mut line = scroll_off;
                    while line <= max_row {
                        let rows = self.line_visual_rows(buf_idx, line);
                        if rows > 0 {
                            if visual + rows > viewport_height {
                                break;
                            }
                            visual += rows;
                            last_fit = line;
                        }
                        line = buf.next_visible_line(line);
                        if line <= last_fit {
                            break;
                        }
                    }
                    last_fit
                };

                // Phase 3: Clamp cursor (needs &mut win again).
                let buf = &self.buffers[buf_idx];
                let win = self.window_mgr.focused_window_mut();
                if win.cursor_row < scroll_off {
                    win.cursor_row = scroll_off;
                }
                if win.cursor_row > bottom {
                    win.cursor_row = bottom;
                }
                win.clamp_cursor(buf);
            }
        }
    }
}
