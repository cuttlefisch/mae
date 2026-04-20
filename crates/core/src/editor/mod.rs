mod changes;
mod command;
mod dap_ops;
mod debug_panel_ops;
mod diagnostics;
mod dispatch;
mod edit_ops;
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
use crate::keymap::{KeyPress, Keymap};
use crate::lsp_intent::LspIntent;
use crate::messages::MessageLog;
use crate::options::{parse_option_bool, OptionKind, OptionRegistry};
use crate::search::SearchState;
use crate::theme::{default_theme, Theme};
use crate::window::{Rect, WindowManager};
use crate::Mode;

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

/// Top-level editor state.
///
/// Designed as a clean, composable state machine that both human keybindings
/// and the AI agent will drive through the same method API. No I/O — all
/// side effects (file read/write) happen through Buffer's std::fs calls.
pub struct Editor {
    pub buffers: Vec<Buffer>,
    pub window_mgr: WindowManager,
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
    /// Per-buffer tree-sitter state (parsed trees + cached highlight spans).
    /// Buffers without a detected language simply have no entry.
    pub syntax: crate::syntax::SyntaxMap,
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
    /// Current round in the AI tool loop.
    pub ai_current_round: usize,
    /// Current transaction start index in history.
    pub ai_transaction_start_idx: Option<usize>,
    /// AI's target buffer context. When set, buffer/LSP tools operate here
    /// instead of the human-focused active buffer. This allows the AI to
    /// edit files while the human watches the *AI* conversation.
    pub ai_target_buffer_idx: Option<usize>,
    /// Toggle: show frame timing in the status bar. Default false.
    /// Toggled via `:set show_fps true` or `(set-option! "show_fps" "true")`.
    pub show_fps: bool,
    /// Name of the active rendering backend ("terminal" or "gui").
    /// Set by the binary after renderer initialization.
    pub renderer_name: String,
    /// GUI font size in points. Default 14.0. Set via config.toml `[editor] font_size`.
    pub gui_font_size: f32,
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
    /// Shared heartbeat counter — incremented each event loop tick by the
    /// binary. The watchdog thread monitors this to detect main-thread stalls.
    pub heartbeat: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Consecutive stall count from the watchdog (0 = healthy). Read-only
    /// for introspection / debug overlay.
    pub watchdog_stall_count: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// Input event recorder for reproducible debugging.
    pub event_recorder: crate::event_record::EventRecorder,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        let commands = CommandRegistry::with_builtins();
        let kb = seed_kb(&commands);
        Editor {
            buffers: vec![Buffer::new()],
            window_mgr: WindowManager::new(0),
            mode: Mode::Normal,
            running: true,
            status_msg: String::new(),
            command_line: String::new(),
            commands,
            keymaps: Self::default_keymaps(),
            which_key_prefix: Vec::new(),
            message_log: MessageLog::new(1000),
            theme: default_theme(),
            debug_state: None,
            registers: HashMap::new(),
            pending_char_command: None,
            awaiting_key_description: false,
            active_register: None,
            pending_register_prompt: false,
            pending_insert_register: false,
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
            hooks: HookRegistry::new(),
            pending_hook_evals: Vec::new(),
            diagnostics: DiagnosticStore::default(),
            syntax: crate::syntax::SyntaxMap::new(),
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
            pending_agent_setup: None,
            input_lock: InputLock::None,
            ai_streaming: false,
            ai_current_round: 0,
            ai_transaction_start_idx: None,
            ai_target_buffer_idx: None,
            show_fps: false,
            renderer_name: "terminal".to_string(),
            gui_font_size: 14.0,
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
            heartbeat: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            watchdog_stall_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            event_recorder: crate::event_record::EventRecorder::new(),
        }
    }

    pub fn with_buffer(buf: Buffer) -> Self {
        let buf_file_path_snapshot = buf.file_path().map(|p| p.to_path_buf());
        let commands = CommandRegistry::with_builtins();
        let kb = seed_kb(&commands);
        Editor {
            buffers: vec![buf],
            window_mgr: WindowManager::new(0),
            mode: Mode::Normal,
            running: true,
            status_msg: String::new(),
            command_line: String::new(),
            commands,
            keymaps: Self::default_keymaps(),
            which_key_prefix: Vec::new(),
            message_log: MessageLog::new(1000),
            theme: default_theme(),
            debug_state: None,
            registers: HashMap::new(),
            pending_char_command: None,
            awaiting_key_description: false,
            active_register: None,
            pending_register_prompt: false,
            pending_insert_register: false,
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
            hooks: HookRegistry::new(),
            pending_hook_evals: Vec::new(),
            diagnostics: DiagnosticStore::default(),
            syntax: {
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
            },
            syntax_selection_stack: Vec::new(),
            marks: HashMap::new(),
            completion_items: Vec::new(),
            completion_selected: 0,
            command_cursor: 0,
            macro_recording: false,
            macro_register: None,
            macro_log: Vec::new(),
            last_macro_register: None,
            macro_replay_depth: 0,
            last_help_state: None,
            splash_art: None,
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
            pending_agent_setup: None,
            input_lock: InputLock::None,
            ai_streaming: false,
            ai_current_round: 0,
            ai_transaction_start_idx: None,
            ai_target_buffer_idx: None,
            show_fps: false,
            renderer_name: "terminal".to_string(),
            gui_font_size: 14.0,
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
            heartbeat: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            watchdog_stall_count: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            event_recorder: crate::event_record::EventRecorder::new(),
        }
    }

    /// Get the keymap for the current mode.
    pub fn current_keymap(&self) -> Option<&Keymap> {
        let name = match self.mode {
            Mode::Normal => "normal",
            Mode::Insert => "insert",
            Mode::Visual(_) => "visual",
            Mode::Command
            | Mode::ConversationInput
            | Mode::Search
            | Mode::FilePicker
            | Mode::FileBrowser
            | Mode::CommandPalette => "command",
            Mode::ShellInsert => return None, // Keys handled by binary shell layer
        };
        self.keymaps.get(name)
    }

    /// Returns the active buffer's project root, falling back to the editor-wide project root.
    pub fn active_project_root(&self) -> Option<&std::path::Path> {
        let buf = self.active_buffer();
        if let Some(ref root) = buf.project_root {
            return Some(root);
        }
        self.project.as_ref().map(|p| p.root.as_path())
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
            "show_fps" => self.show_fps.to_string(),
            "font_size" => self.gui_font_size.to_string(),
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
            "restore_session" => self.restore_session.to_string(),
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
            "restore_session" => {
                self.restore_session = parse_option_bool(value)?;
            }
            _ => return Err(format!("Unknown option: {}", name)),
        }
        let (current, _) = self.get_option(def_name).unwrap();
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

    /// Find a buffer index by name. Returns None if not found.
    pub fn find_buffer_by_name(&self, name: &str) -> Option<usize> {
        self.buffers.iter().position(|b| b.name == name)
    }

    /// First conversation attached to any buffer, if any.
    pub fn conversation(&self) -> Option<&crate::conversation::Conversation> {
        self.buffers.iter().find_map(|b| b.conversation.as_ref())
    }

    /// Mutable view of the first conversation attached to any buffer.
    pub fn conversation_mut(&mut self) -> Option<&mut crate::conversation::Conversation> {
        self.buffers
            .iter_mut()
            .find_map(|b| b.conversation.as_mut())
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
            if let Some(view) = self.buffers[idx].help_view.as_mut() {
                view.navigate_to(node_id.to_string());
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
            .and_then(|b| b.help_view.as_mut())
    }

    /// Immutable view onto the help buffer's HelpView, if any help buffer exists.
    pub fn help_view(&self) -> Option<&crate::help_view::HelpView> {
        self.buffers
            .iter()
            .find(|b| b.kind == crate::buffer::BufferKind::Help)
            .and_then(|b| b.help_view.as_ref())
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
        idx < self.buffers.len() && self.buffers[idx].kind == crate::BufferKind::Conversation
    }

    /// Switch to buffer `idx` but avoid stealing focus from a conversation window.
    ///
    /// If the focused window shows a conversation buffer, the new buffer is
    /// routed to another window (or a new split is created). This keeps `*AI*`
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

        // 3. Fallback: split the focused window if it's conversation,
        // or just some window if we can't find a better spot.
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
                self.mode = saved;
                return;
            }
        }

        // No saved mode or invalid — use default.
        match kind {
            crate::BufferKind::Shell => {
                self.mode = Mode::ShellInsert;
            }
            _ => {
                if matches!(self.mode, Mode::ShellInsert | Mode::ConversationInput) {
                    self.mode = Mode::Normal;
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
            self.mode = crate::Mode::Visual(crate::VisualType::Char);
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
                if let Some(ref mut conv) = self.buffers[buf_idx].conversation {
                    if delta > 0 {
                        conv.scroll_up(lines * scroll_speed);
                    } else {
                        conv.scroll_down(lines * scroll_speed);
                    }
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

                let win = self.window_mgr.focused_window_mut();
                if delta > 0 {
                    win.scroll_offset = win.scroll_offset.saturating_sub(lines * scroll_speed);
                } else {
                    let max_scroll = buf_line_count.saturating_sub(viewport_height);
                    win.scroll_offset = (win.scroll_offset + lines * scroll_speed).min(max_scroll);
                }

                // Clamp cursor into visible viewport.
                if win.cursor_row < win.scroll_offset {
                    win.cursor_row = win.scroll_offset;
                }
                let bottom = win.scroll_offset + viewport_height.saturating_sub(1);
                let max_row = buf_line_count.saturating_sub(1);
                if win.cursor_row > bottom.min(max_row) {
                    win.cursor_row = bottom.min(max_row);
                }
                win.clamp_cursor(&self.buffers[buf_idx]);
            }
        }
    }
}
