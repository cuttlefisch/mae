mod command;
mod diagnostics;
mod dispatch;
mod edit_ops;
mod file_ops;
mod keymaps;
mod lsp_ops;
mod search_ops;
mod text_objects;
mod visual;

pub use diagnostics::{Diagnostic, DiagnosticSeverity, DiagnosticStore};
pub use lsp_ops::{LspLocation, LspRange};

#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::buffer::Buffer;
use crate::commands::CommandRegistry;
use crate::debug::DebugState;
use crate::file_picker::FilePicker;
use crate::keymap::{KeyPress, Keymap};
use crate::lsp_intent::LspIntent;
use crate::messages::MessageLog;
use crate::search::SearchState;
use crate::theme::{default_theme, Theme};
use crate::window::{Rect, WindowManager};
use crate::Mode;

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
    /// Tab completion matches for command mode (:e path).
    pub tab_completions: Vec<String>,
    pub tab_completion_idx: usize,
    /// Last repeatable edit for dot-repeat (`.`).
    pub last_edit: Option<EditRecord>,
    /// Char offset at the point insert mode was entered (for capturing inserted text).
    pub insert_start_offset: Option<usize>,
    /// The command that initiated the current insert mode session (for dot-repeat).
    pub insert_initiated_by: Option<String>,
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
    /// Queue of pending LSP requests for the binary to drain each event-loop tick.
    /// The core cannot call async LSP code directly; instead, commands push
    /// intents here and `main.rs` forwards them to `run_lsp_task`.
    pub pending_lsp_requests: Vec<LspIntent>,
    /// LSP diagnostics keyed by file URI. Replaced wholesale on each
    /// `publishDiagnostics` notification (the LSP contract).
    pub diagnostics: DiagnosticStore,
    /// Per-buffer tree-sitter state (parsed trees + cached highlight spans).
    /// Buffers without a detected language simply have no entry.
    pub syntax: crate::syntax::SyntaxMap,
}

impl Default for Editor {
    fn default() -> Self {
        Self::new()
    }
}

impl Editor {
    pub fn new() -> Self {
        Editor {
            buffers: vec![Buffer::new()],
            window_mgr: WindowManager::new(0),
            mode: Mode::Normal,
            running: true,
            status_msg: String::new(),
            command_line: String::new(),
            commands: CommandRegistry::with_builtins(),
            keymaps: Self::default_keymaps(),
            which_key_prefix: Vec::new(),
            message_log: MessageLog::new(1000),
            theme: default_theme(),
            debug_state: None,
            registers: HashMap::new(),
            pending_char_command: None,
            search_state: SearchState::default(),
            search_input: String::new(),
            visual_anchor_row: 0,
            visual_anchor_col: 0,
            viewport_height: 24,
            file_picker: None,
            tab_completions: Vec::new(),
            tab_completion_idx: 0,
            last_edit: None,
            insert_start_offset: None,
            insert_initiated_by: None,
            count_prefix: None,
            pending_char_count: 1,
            alternate_buffer_idx: None,
            command_history: Vec::new(),
            command_history_idx: None,
            pending_lsp_requests: Vec::new(),
            diagnostics: DiagnosticStore::default(),
            syntax: crate::syntax::SyntaxMap::new(),
        }
    }

    pub fn with_buffer(buf: Buffer) -> Self {
        let buf_file_path_snapshot = buf.file_path().map(|p| p.to_path_buf());
        Editor {
            buffers: vec![buf],
            window_mgr: WindowManager::new(0),
            mode: Mode::Normal,
            running: true,
            status_msg: String::new(),
            command_line: String::new(),
            commands: CommandRegistry::with_builtins(),
            keymaps: Self::default_keymaps(),
            which_key_prefix: Vec::new(),
            message_log: MessageLog::new(1000),
            theme: default_theme(),
            debug_state: None,
            registers: HashMap::new(),
            pending_char_command: None,
            search_state: SearchState::default(),
            search_input: String::new(),
            visual_anchor_row: 0,
            visual_anchor_col: 0,
            viewport_height: 24,
            file_picker: None,
            tab_completions: Vec::new(),
            tab_completion_idx: 0,
            last_edit: None,
            insert_start_offset: None,
            insert_initiated_by: None,
            count_prefix: None,
            pending_char_count: 1,
            alternate_buffer_idx: None,
            command_history: Vec::new(),
            command_history_idx: None,
            pending_lsp_requests: Vec::new(),
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
        }
    }

    /// Get the keymap for the current mode.
    pub fn current_keymap(&self) -> Option<&Keymap> {
        let name = match self.mode {
            Mode::Normal => "normal",
            Mode::Insert => "insert",
            Mode::Visual(_) => "visual",
            Mode::Command | Mode::ConversationInput | Mode::Search | Mode::FilePicker => "command",
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

    /// Consume the count prefix, returning the count (default 1).
    pub fn take_count(&mut self) -> usize {
        self.count_prefix.take().unwrap_or(1)
    }

    /// Default area for window operations when we don't have the real terminal size.
    /// The renderer will provide real dimensions at render time.
    pub(crate) fn default_area(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        }
    }
}
