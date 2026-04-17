mod changes;
mod command;
mod dap_ops;
mod diagnostics;
mod dispatch;
mod edit_ops;
mod file_ops;
mod help_ops;
mod jumps;
mod keymaps;
mod lsp_ops;
mod macros;
mod marks;
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
use crate::kb_seed::seed_kb;
use crate::keymap::{KeyPress, Keymap};
use crate::lsp_intent::LspIntent;
use crate::messages::MessageLog;
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
    /// Which ASCII art to show on the splash screen. Options: "cherry-blossom",
    /// "hairbow", "bat". Default is "cherry-blossom".
    pub splash_art: Option<String>,
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
            splash_art: Some("cherry-blossom".to_string()),
            last_find_char: None,
            last_visual: None,
            pending_scheme_eval: Vec::new(),
            kb,
            ai_session_cost_usd: 0.0,
            ai_session_tokens_in: 0,
            ai_session_tokens_out: 0,
            bell_until: None,
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
            last_find_char: None,
            last_visual: None,
            pending_scheme_eval: Vec::new(),
            kb,
            ai_session_cost_usd: 0.0,
            ai_session_tokens_in: 0,
            ai_session_tokens_out: 0,
            bell_until: None,
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
        };
        self.keymaps.get(name)
    }

    /// Convenience: index of the active (focused window's) buffer.
    pub fn active_buffer_idx(&self) -> usize {
        self.window_mgr.focused_window().buffer_idx
    }

    pub fn active_buffer(&self) -> &Buffer {
        &self.buffers[self.active_buffer_idx()]
    }

    pub fn active_buffer_mut(&mut self) -> &mut Buffer {
        let idx = self.active_buffer_idx();
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
        let win = self.window_mgr.focused_window_mut();
        win.buffer_idx = idx;
        win.cursor_row = 0;
        win.cursor_col = 0;
        true
    }

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = msg.into();
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
}
