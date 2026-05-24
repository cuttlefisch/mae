//! Vi-modal editing state extracted from Editor.
//! All fields were previously directly on Editor; now accessed via `editor.vi.*`.

use std::collections::HashMap;

use crate::keymap::KeyPress;
use crate::VisualType;

use super::changes::ChangeEntry;
use super::jumps::JumpEntry;
use super::marks::Mark;
use super::EditRecord;

/// Vi-modal editing state: operators, counts, registers, marks, macros,
/// visual selection, command-line, jump/change lists, and dot-repeat.
#[derive(Debug)]
pub struct ViState {
    /// Pending char-argument command (e.g. after pressing `f`, waiting for target char).
    pub pending_char_command: Option<String>,
    /// Count saved for pending char-argument commands (f/F/t/T/r + char).
    pub pending_char_count: usize,
    /// True after the user pressed `"` in normal/visual mode; the next
    /// char will populate [`Self::active_register`].
    pub pending_register_prompt: bool,
    /// Active named register selected by `"x` prefix.
    pub active_register: Option<char>,
    /// True after the user pressed `Ctrl-R` in insert mode; the next
    /// char selects a register whose contents will be inserted.
    pub pending_insert_register: bool,
    /// C-o in insert mode: execute one normal command then return to insert.
    pub insert_mode_oneshot_normal: bool,
    /// Char offset at the point insert mode was entered (for capturing inserted text).
    pub insert_start_offset: Option<usize>,
    /// The command that initiated the current insert mode session (for dot-repeat).
    pub insert_initiated_by: Option<String>,
    /// Cursor position (buffer_idx, row, col) at the point insert mode was last exited.
    pub last_insert_pos: Option<(usize, usize, usize)>,
    /// First delimiter captured during a `cs<from><to>` sequence.
    pub pending_surround_from: Option<char>,
    /// Char offset range saved by `ys{motion}` for the subsequent char-await.
    pub pending_surround_range: Option<(usize, usize)>,
    /// Visual mode anchor (row, col) — start of selection.
    pub visual_anchor_row: usize,
    pub visual_anchor_col: usize,
    /// Saved visual selection from last exit.
    pub last_visual: Option<(usize, usize, usize, usize, VisualType)>,
    /// Pending operator for operator-pending mode (`d`, `c`, `y`).
    pub pending_operator: Option<String>,
    /// Cursor position (row, col) when operator-pending started.
    pub operator_start: Option<(usize, usize)>,
    /// Count prefix saved from the operator key.
    pub operator_count: Option<usize>,
    /// True if the last dispatched motion was linewise.
    pub last_motion_linewise: bool,
    /// Vi-style count prefix (e.g. `5j` = move down 5). None = no count typed.
    pub count_prefix: Option<usize>,
    /// Last repeatable edit for dot-repeat (`.`).
    pub last_edit: Option<EditRecord>,
    /// Last f/F/t/T search: (char, command-name).
    pub last_find_char: Option<(char, String)>,
    /// True while a macro is being recorded into `macro_register`.
    pub macro_recording: bool,
    /// Register letter being recorded into (a-z).
    pub macro_register: Option<char>,
    /// Raw keystroke log for the active recording session.
    pub macro_log: Vec<KeyPress>,
    /// Register letter of the last-replayed macro (for `@@`).
    pub last_macro_register: Option<char>,
    /// Recursion depth guard during macro replay (max 10).
    pub macro_replay_depth: usize,
    /// Named cursor marks, keyed by mark letter.
    pub marks: HashMap<char, Mark>,
    /// Named registers for yank/paste.
    pub registers: HashMap<char, String>,
    /// Jump list (vim `Ctrl-o` / `Ctrl-i`).
    pub jumps: Vec<JumpEntry>,
    /// Cursor into `jumps`.
    pub jump_idx: usize,
    /// Change list (vim `g;` / `g,`).
    pub changes: Vec<ChangeEntry>,
    /// Cursor into `changes`.
    pub change_idx: usize,
    /// Command-line text (`:` mode content).
    pub command_line: String,
    /// Command-line history (for up/down recall in `:` mode).
    pub command_history: Vec<String>,
    /// Current index into command_history when recalling.
    pub command_history_idx: Option<usize>,
    /// Cursor position (byte index) within `command_line`.
    pub command_cursor: usize,
    /// Tab completion matches for command mode.
    pub tab_completions: Vec<String>,
    pub tab_completion_idx: usize,
    /// Stack of prior char-offset visual selections for syntax expand/contract.
    pub syntax_selection_stack: Vec<(usize, usize)>,
    /// Index of the previously active buffer (for Ctrl-^ alternate file).
    pub alternate_buffer_idx: Option<usize>,
    /// Pending block-visual insert: (min_row, max_row, min_col).
    pub pending_block_insert: Option<(usize, usize, usize)>,
}

impl ViState {
    pub fn new() -> Self {
        Self {
            pending_char_command: None,
            pending_char_count: 1,
            pending_register_prompt: false,
            active_register: None,
            pending_insert_register: false,
            insert_mode_oneshot_normal: false,
            insert_start_offset: None,
            insert_initiated_by: None,
            last_insert_pos: None,
            pending_surround_from: None,
            pending_surround_range: None,
            visual_anchor_row: 0,
            visual_anchor_col: 0,
            last_visual: None,
            pending_operator: None,
            operator_start: None,
            operator_count: None,
            last_motion_linewise: false,
            count_prefix: None,
            last_edit: None,
            last_find_char: None,
            macro_recording: false,
            macro_register: None,
            macro_log: Vec::new(),
            last_macro_register: None,
            macro_replay_depth: 0,
            marks: HashMap::new(),
            registers: HashMap::new(),
            jumps: Vec::new(),
            jump_idx: 0,
            changes: Vec::new(),
            change_idx: 0,
            command_line: String::new(),
            command_history: Vec::new(),
            command_history_idx: None,
            command_cursor: 0,
            tab_completions: Vec::new(),
            tab_completion_idx: 0,
            syntax_selection_stack: Vec::new(),
            alternate_buffer_idx: None,
            pending_block_insert: None,
        }
    }
}

impl Default for ViState {
    fn default() -> Self {
        Self::new()
    }
}
