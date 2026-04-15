use std::collections::HashMap;
use std::path::Path;

use regex::Regex;

use crate::buffer::Buffer;
use crate::commands::CommandRegistry;
use crate::debug::{DebugState, DebugTarget};
use crate::file_picker::FilePicker;
use crate::keymap::{parse_key_seq, parse_key_seq_spaced, Key, KeyPress, Keymap};
use crate::messages::MessageLog;
use crate::search::{self, SearchDirection, SearchState};
use crate::theme::{bundled_theme_names, default_theme, BundledResolver, Theme};
use crate::window::{Direction, Rect, SplitDirection, WindowManager};
use crate::{Mode, VisualType};

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
        }
    }

    pub fn with_buffer(buf: Buffer) -> Self {
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

    /// Create the default vi-like keymaps.
    fn default_keymaps() -> HashMap<String, Keymap> {
        let mut maps = HashMap::new();

        let mut normal = Keymap::new("normal");
        // Movement
        normal.bind(parse_key_seq("h"), "move-left");
        normal.bind(parse_key_seq("j"), "move-down");
        normal.bind(parse_key_seq("k"), "move-up");
        normal.bind(parse_key_seq("l"), "move-right");
        normal.bind(vec![KeyPress::special(Key::Left)], "move-left");
        normal.bind(vec![KeyPress::special(Key::Down)], "move-down");
        normal.bind(vec![KeyPress::special(Key::Up)], "move-up");
        normal.bind(vec![KeyPress::special(Key::Right)], "move-right");
        normal.bind(parse_key_seq("0"), "move-to-line-start");
        normal.bind(parse_key_seq("$"), "move-to-line-end");
        normal.bind(parse_key_seq("G"), "move-to-last-line");
        normal.bind(parse_key_seq("gg"), "move-to-first-line");
        // Word motions
        normal.bind(parse_key_seq("w"), "move-word-forward");
        normal.bind(parse_key_seq("b"), "move-word-backward");
        normal.bind(parse_key_seq("e"), "move-word-end");
        normal.bind(parse_key_seq("W"), "move-big-word-forward");
        normal.bind(parse_key_seq("B"), "move-big-word-backward");
        normal.bind(parse_key_seq("E"), "move-big-word-end");
        normal.bind(parse_key_seq("%"), "move-matching-bracket");
        normal.bind(parse_key_seq("{"), "move-paragraph-backward");
        normal.bind(parse_key_seq("}"), "move-paragraph-forward");
        normal.bind(parse_key_seq("f"), "find-char-forward-await");
        normal.bind(parse_key_seq("F"), "find-char-backward-await");
        normal.bind(parse_key_seq("t"), "till-char-forward-await");
        normal.bind(parse_key_seq("T"), "till-char-backward-await");
        // Scroll
        normal.bind(parse_key_seq("C-u"), "scroll-half-up");
        normal.bind(parse_key_seq("C-d"), "scroll-half-down");
        normal.bind(parse_key_seq("C-f"), "scroll-page-down");
        normal.bind(parse_key_seq("C-b"), "scroll-page-up");
        normal.bind(parse_key_seq("zz"), "scroll-center");
        normal.bind(parse_key_seq("zt"), "scroll-top");
        normal.bind(parse_key_seq("zb"), "scroll-bottom");
        // Screen-relative cursor
        normal.bind(parse_key_seq("H"), "move-screen-top");
        normal.bind(parse_key_seq("M"), "move-screen-middle");
        normal.bind(parse_key_seq("L"), "move-screen-bottom");
        // Search
        normal.bind(parse_key_seq("/"), "search-forward-start");
        normal.bind(parse_key_seq("?"), "search-backward-start");
        normal.bind(parse_key_seq("n"), "search-next");
        normal.bind(parse_key_seq("N"), "search-prev");
        normal.bind(parse_key_seq("*"), "search-word-under-cursor");
        // Editing
        normal.bind(parse_key_seq("x"), "delete-char-forward");
        normal.bind(parse_key_seq("dd"), "delete-line");
        normal.bind(parse_key_seq("dw"), "delete-word-forward");
        normal.bind(parse_key_seq("d$"), "delete-to-line-end");
        normal.bind(parse_key_seq("d0"), "delete-to-line-start");
        // Change operators
        normal.bind(parse_key_seq("cc"), "change-line");
        normal.bind(parse_key_seq("cw"), "change-word-forward");
        normal.bind(parse_key_seq("c$"), "change-to-line-end");
        normal.bind(parse_key_seq("C"), "change-to-line-end");
        normal.bind(parse_key_seq("c0"), "change-to-line-start");
        // Replace
        normal.bind(parse_key_seq("r"), "replace-char-await");
        // Dot repeat
        normal.bind(parse_key_seq("."), "dot-repeat");
        // Yank/Paste
        normal.bind(parse_key_seq("yy"), "yank-line");
        normal.bind(parse_key_seq("yw"), "yank-word-forward");
        normal.bind(parse_key_seq("y$"), "yank-to-line-end");
        normal.bind(parse_key_seq("y0"), "yank-to-line-start");
        normal.bind(parse_key_seq("p"), "paste-after");
        normal.bind(parse_key_seq("P"), "paste-before");
        // Undo/Redo
        normal.bind(parse_key_seq("u"), "undo");
        normal.bind(parse_key_seq("C-r"), "redo");
        // Mode changes
        normal.bind(parse_key_seq("i"), "enter-insert-mode");
        normal.bind(parse_key_seq("a"), "enter-insert-mode-after");
        normal.bind(parse_key_seq("A"), "enter-insert-mode-eol");
        normal.bind(parse_key_seq("o"), "open-line-below");
        normal.bind(parse_key_seq("O"), "open-line-above");
        normal.bind(parse_key_seq(":"), "enter-command-mode");
        // Window management (Ctrl-W prefix, normal mode only)
        normal.bind(parse_key_seq_spaced("C-w v"), "split-vertical");
        normal.bind(parse_key_seq_spaced("C-w s"), "split-horizontal");
        normal.bind(parse_key_seq_spaced("C-w q"), "close-window");
        normal.bind(parse_key_seq_spaced("C-w h"), "focus-left");
        normal.bind(parse_key_seq_spaced("C-w j"), "focus-down");
        normal.bind(parse_key_seq_spaced("C-w k"), "focus-up");
        normal.bind(parse_key_seq_spaced("C-w l"), "focus-right");

        // Leader key (SPC) bindings — Doom Emacs style
        normal.bind(parse_key_seq_spaced("SPC SPC"), "command-palette");
        // +buffer
        normal.bind(parse_key_seq_spaced("SPC b s"), "save");
        normal.bind(parse_key_seq_spaced("SPC b b"), "switch-buffer");
        normal.bind(parse_key_seq_spaced("SPC b d"), "kill-buffer");
        normal.bind(parse_key_seq_spaced("SPC b n"), "next-buffer");
        normal.bind(parse_key_seq_spaced("SPC b p"), "prev-buffer");
        // +file
        normal.bind(parse_key_seq_spaced("SPC f f"), "find-file");
        normal.bind(parse_key_seq_spaced("SPC f s"), "save");
        // +window
        normal.bind(parse_key_seq_spaced("SPC w v"), "split-vertical");
        normal.bind(parse_key_seq_spaced("SPC w s"), "split-horizontal");
        normal.bind(parse_key_seq_spaced("SPC w q"), "close-window");
        normal.bind(parse_key_seq_spaced("SPC w h"), "focus-left");
        normal.bind(parse_key_seq_spaced("SPC w j"), "focus-down");
        normal.bind(parse_key_seq_spaced("SPC w k"), "focus-up");
        normal.bind(parse_key_seq_spaced("SPC w l"), "focus-right");
        // +ai
        normal.bind(parse_key_seq_spaced("SPC a a"), "ai-prompt");
        normal.bind(parse_key_seq_spaced("SPC a c"), "ai-cancel");
        // +help
        normal.bind(parse_key_seq_spaced("SPC h k"), "describe-key");
        normal.bind(parse_key_seq_spaced("SPC h c"), "describe-command");
        // +theme
        normal.bind(parse_key_seq_spaced("SPC t t"), "cycle-theme");
        normal.bind(parse_key_seq_spaced("SPC t s"), "set-theme");
        // +debug
        normal.bind(parse_key_seq_spaced("SPC d d"), "debug-start");
        normal.bind(parse_key_seq_spaced("SPC d s"), "debug-self");
        normal.bind(parse_key_seq_spaced("SPC d q"), "debug-stop");
        normal.bind(parse_key_seq_spaced("SPC d c"), "debug-continue");
        normal.bind(parse_key_seq_spaced("SPC d n"), "debug-step-over");
        normal.bind(parse_key_seq_spaced("SPC d i"), "debug-step-into");
        normal.bind(parse_key_seq_spaced("SPC d o"), "debug-step-out");
        normal.bind(parse_key_seq_spaced("SPC d b"), "debug-toggle-breakpoint");
        normal.bind(parse_key_seq_spaced("SPC d v"), "debug-inspect");
        // +quit
        normal.bind(parse_key_seq_spaced("SPC q q"), "quit");
        normal.bind(parse_key_seq_spaced("SPC q Q"), "force-quit");

        // Group labels for which-key popup
        normal.set_group_name(parse_key_seq_spaced("SPC b"), "+buffer");
        normal.set_group_name(parse_key_seq_spaced("SPC f"), "+file");
        normal.set_group_name(parse_key_seq_spaced("SPC w"), "+window");
        normal.set_group_name(parse_key_seq_spaced("SPC a"), "+ai");
        normal.set_group_name(parse_key_seq_spaced("SPC t"), "+theme");
        normal.set_group_name(parse_key_seq_spaced("SPC d"), "+debug");
        normal.set_group_name(parse_key_seq_spaced("SPC h"), "+help");
        normal.set_group_name(parse_key_seq_spaced("SPC q"), "+quit");

        let mut insert = Keymap::new("insert");
        insert.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");
        insert.bind(vec![KeyPress::special(Key::Left)], "move-left");
        insert.bind(vec![KeyPress::special(Key::Down)], "move-down");
        insert.bind(vec![KeyPress::special(Key::Up)], "move-up");
        insert.bind(vec![KeyPress::special(Key::Right)], "move-right");
        // Note: Enter, Backspace, and printable chars are handled specially
        // by the binary, not through the keymap, since they need arguments.

        // Visual mode: v/V enter from normal
        normal.bind(parse_key_seq("v"), "enter-visual-char");
        normal.bind(parse_key_seq("V"), "enter-visual-line");

        // Visual keymap: all normal movements plus operators
        let mut visual = Keymap::new("visual");
        // Movement (same as normal)
        visual.bind(parse_key_seq("h"), "move-left");
        visual.bind(parse_key_seq("j"), "move-down");
        visual.bind(parse_key_seq("k"), "move-up");
        visual.bind(parse_key_seq("l"), "move-right");
        visual.bind(vec![KeyPress::special(Key::Left)], "move-left");
        visual.bind(vec![KeyPress::special(Key::Down)], "move-down");
        visual.bind(vec![KeyPress::special(Key::Up)], "move-up");
        visual.bind(vec![KeyPress::special(Key::Right)], "move-right");
        visual.bind(parse_key_seq("0"), "move-to-line-start");
        visual.bind(parse_key_seq("$"), "move-to-line-end");
        visual.bind(parse_key_seq("G"), "move-to-last-line");
        visual.bind(parse_key_seq("gg"), "move-to-first-line");
        visual.bind(parse_key_seq("w"), "move-word-forward");
        visual.bind(parse_key_seq("b"), "move-word-backward");
        visual.bind(parse_key_seq("e"), "move-word-end");
        visual.bind(parse_key_seq("W"), "move-big-word-forward");
        visual.bind(parse_key_seq("B"), "move-big-word-backward");
        visual.bind(parse_key_seq("E"), "move-big-word-end");
        visual.bind(parse_key_seq("%"), "move-matching-bracket");
        visual.bind(parse_key_seq("{"), "move-paragraph-backward");
        visual.bind(parse_key_seq("}"), "move-paragraph-forward");
        visual.bind(parse_key_seq("f"), "find-char-forward-await");
        visual.bind(parse_key_seq("F"), "find-char-backward-await");
        visual.bind(parse_key_seq("t"), "till-char-forward-await");
        visual.bind(parse_key_seq("T"), "till-char-backward-await");
        // Scroll
        visual.bind(parse_key_seq("C-u"), "scroll-half-up");
        visual.bind(parse_key_seq("C-d"), "scroll-half-down");
        visual.bind(parse_key_seq("C-f"), "scroll-page-down");
        visual.bind(parse_key_seq("C-b"), "scroll-page-up");
        visual.bind(parse_key_seq("zz"), "scroll-center");
        visual.bind(parse_key_seq("zt"), "scroll-top");
        visual.bind(parse_key_seq("zb"), "scroll-bottom");
        // Screen-relative cursor
        visual.bind(parse_key_seq("H"), "move-screen-top");
        visual.bind(parse_key_seq("M"), "move-screen-middle");
        visual.bind(parse_key_seq("L"), "move-screen-bottom");
        // Operators
        visual.bind(parse_key_seq("d"), "visual-delete");
        visual.bind(parse_key_seq("x"), "visual-delete");
        visual.bind(parse_key_seq("y"), "visual-yank");
        visual.bind(parse_key_seq("c"), "visual-change");
        // Mode switches
        visual.bind(parse_key_seq("v"), "enter-visual-char");
        visual.bind(parse_key_seq("V"), "enter-visual-line");
        visual.bind(vec![KeyPress::special(Key::Escape)], "enter-normal-mode");

        maps.insert("normal".to_string(), normal);
        maps.insert("insert".to_string(), insert);
        maps.insert("visual".to_string(), visual);
        maps.insert("command".to_string(), Keymap::new("command"));

        maps
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

    pub fn set_status(&mut self, msg: impl Into<String>) {
        self.status_msg = msg.into();
    }

    /// Consume the count prefix, returning the count (default 1).
    pub fn take_count(&mut self) -> usize {
        self.count_prefix.take().unwrap_or(1)
    }

    /// Dispatch a built-in command by name. Returns true if recognized.
    ///
    /// This is the shared dispatch point for human keybindings and the AI agent.
    /// Scheme-defined commands are handled by the binary (which has the SchemeRuntime).
    pub fn dispatch_builtin(&mut self, name: &str) -> bool {
        // Consume the count prefix at the top of every dispatch.
        // `count` is Some(n) if user typed a digit prefix, None if not.
        // `n` is the effective repeat count (default 1).
        let count = self.count_prefix.take();
        let n = count.unwrap_or(1);

        match name {
            // Movement — operates on focused window + its buffer
            "move-up" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_up(buf);
                }
            }
            "move-down" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_down(buf);
                }
            }
            "move-left" => {
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_left();
                }
            }
            "move-right" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_right(buf);
                }
            }
            "move-to-line-start" => {
                self.window_mgr.focused_window_mut().move_to_line_start();
            }
            "move-to-line-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr.focused_window_mut().move_to_line_end(buf);
            }
            "move-to-first-line" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                if let Some(target) = count {
                    // ngg = go to line n (1-indexed)
                    let row = (target.saturating_sub(1)).min(buf.line_count().saturating_sub(1));
                    self.window_mgr.focused_window_mut().cursor_row = row;
                    self.window_mgr.focused_window_mut().clamp_cursor(buf);
                } else {
                    self.window_mgr.focused_window_mut().move_to_first_line(buf);
                }
            }
            "move-to-last-line" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                if let Some(target) = count {
                    // nG = go to line n (1-indexed)
                    let row = (target.saturating_sub(1)).min(buf.line_count().saturating_sub(1));
                    self.window_mgr.focused_window_mut().cursor_row = row;
                    self.window_mgr.focused_window_mut().clamp_cursor(buf);
                } else {
                    self.window_mgr.focused_window_mut().move_to_last_line(buf);
                }
            }
            "move-word-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_forward(buf);
                }
            }
            "move-word-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_backward(buf);
                }
            }
            "move-word-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_word_end(buf);
                }
            }
            "move-big-word-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_forward(buf);
                }
            }
            "move-big-word-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_big_word_backward(buf);
                }
            }
            "move-big-word-end" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().move_big_word_end(buf);
                }
            }
            "move-matching-bracket" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr
                    .focused_window_mut()
                    .move_matching_bracket(buf);
            }
            "move-paragraph-forward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_paragraph_forward(buf);
                }
            }
            "move-paragraph-backward" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .move_paragraph_backward(buf);
                }
            }
            "find-char-forward-await" => {
                self.pending_char_command = Some("find-char-forward".to_string());
                self.pending_char_count = n;
            }
            "find-char-backward-await" => {
                self.pending_char_command = Some("find-char-backward".to_string());
                self.pending_char_count = n;
            }
            "till-char-forward-await" => {
                self.pending_char_command = Some("till-char-forward".to_string());
                self.pending_char_count = n;
            }
            "till-char-backward-await" => {
                self.pending_char_command = Some("till-char-backward".to_string());
                self.pending_char_count = n;
            }

            // Scroll commands
            "scroll-half-up" => {
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().scroll_half_up(vh);
                }
            }
            "scroll-half-down" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .scroll_half_down(buf, vh);
                }
            }
            "scroll-page-up" => {
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr.focused_window_mut().scroll_page_up(vh);
                }
            }
            "scroll-page-down" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                for _ in 0..n {
                    self.window_mgr
                        .focused_window_mut()
                        .scroll_page_down(buf, vh);
                }
            }
            "scroll-center" => {
                let vh = self.viewport_height;
                self.window_mgr.focused_window_mut().scroll_center(vh);
            }
            "scroll-top" => {
                self.window_mgr.focused_window_mut().scroll_cursor_top();
            }
            "scroll-bottom" => {
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .scroll_cursor_bottom(vh);
            }
            // Screen-relative cursor
            "move-screen-top" => {
                self.window_mgr.focused_window_mut().move_to_screen_top();
            }
            "move-screen-middle" => {
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .move_to_screen_middle(vh);
            }
            "move-screen-bottom" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let vh = self.viewport_height;
                self.window_mgr
                    .focused_window_mut()
                    .move_to_screen_bottom(buf, vh);
            }

            // Editing
            "delete-char-forward" => {
                for _ in 0..n {
                    let idx = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window_mut();
                    self.buffers[idx].delete_char_forward(win);
                }
                self.record_edit_with_count("delete-char-forward", count);
            }
            "delete-char-backward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].delete_char_backward(win);
            }
            "delete-line" => {
                let idx = self.active_buffer_idx();
                let mut all_deleted = String::new();
                for _ in 0..n {
                    let win = self.window_mgr.focused_window_mut();
                    let deleted = self.buffers[idx].delete_line(win);
                    all_deleted.push_str(&deleted);
                }
                if !all_deleted.is_empty() {
                    self.registers.insert('"', all_deleted);
                }
                self.record_edit_with_count("delete-line", count);
            }
            "delete-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                // Find the Nth word boundary
                let mut end = start;
                for _ in 0..n {
                    end = crate::word::word_start_forward(self.buffers[idx].rope(), end);
                }
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.record_edit_with_count("delete-word-forward", count);
            }
            "delete-to-line-end" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                let line_len = self.buffers[idx].line_len(win.cursor_row);
                let end = line_start + line_len;
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.record_edit("delete-to-line-end");
            }
            "delete-to-line-start" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                let end = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_col = 0;
                }
                self.record_edit("delete-to-line-start");
            }
            "yank-line" => {
                let idx = self.active_buffer_idx();
                let start_row = self.window_mgr.focused_window().cursor_row;
                let line_count = self.buffers[idx].line_count();
                let end_row = (start_row + n).min(line_count);
                let mut yanked = String::new();
                for row in start_row..end_row {
                    yanked.push_str(&self.buffers[idx].line_text(row));
                }
                if !yanked.is_empty() {
                    self.registers.insert('"', yanked);
                    let yanked_count = end_row - start_row;
                    self.set_status(format!(
                        "{} line{} yanked",
                        yanked_count,
                        if yanked_count == 1 { "" } else { "s" }
                    ));
                }
            }
            "yank-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let end = crate::word::word_start_forward(self.buffers[idx].rope(), start);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.registers.insert('"', text);
                }
            }
            "yank-to-line-end" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                let line_len = self.buffers[idx].line_len(win.cursor_row);
                let end = line_start + line_len;
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.registers.insert('"', text);
                }
            }
            "yank-to-line-start" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let end = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.registers.insert('"', text);
                }
            }
            "paste-after" => {
                if let Some(text) = self.registers.get(&'"').cloned() {
                    let idx = self.active_buffer_idx();
                    let is_linewise = text.ends_with('\n');
                    for _ in 0..n {
                        if is_linewise {
                            // Insert on line below
                            let win = self.window_mgr.focused_window();
                            let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                            let line_len =
                                self.buffers[idx].rope().line(win.cursor_row).len_chars();
                            let insert_pos = line_start + line_len;
                            self.buffers[idx].insert_text_at(insert_pos, &text);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_row += 1;
                            win.cursor_col = 0;
                        } else {
                            let win = self.window_mgr.focused_window();
                            let offset =
                                self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                            let insert_pos = (offset + 1).min(self.buffers[idx].rope().len_chars());
                            self.buffers[idx].insert_text_at(insert_pos, &text);
                            // Move cursor to end of pasted text
                            let end_pos = insert_pos + text.chars().count() - 1;
                            let rope = self.buffers[idx].rope();
                            let new_row = rope.char_to_line(end_pos);
                            let line_start = rope.line_to_char(new_row);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_row = new_row;
                            win.cursor_col = end_pos - line_start;
                        }
                    }
                }
                self.record_edit_with_count("paste-after", count);
            }
            "paste-before" => {
                if let Some(text) = self.registers.get(&'"').cloned() {
                    let idx = self.active_buffer_idx();
                    let is_linewise = text.ends_with('\n');
                    for _ in 0..n {
                        if is_linewise {
                            // Insert on line above
                            let win = self.window_mgr.focused_window();
                            let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                            self.buffers[idx].insert_text_at(line_start, &text);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_col = 0;
                        } else {
                            let win = self.window_mgr.focused_window();
                            let offset =
                                self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                            self.buffers[idx].insert_text_at(offset, &text);
                            let end_pos = offset + text.chars().count() - 1;
                            let rope = self.buffers[idx].rope();
                            let new_row = rope.char_to_line(end_pos);
                            let line_start = rope.line_to_char(new_row);
                            let win = self.window_mgr.focused_window_mut();
                            win.cursor_row = new_row;
                            win.cursor_col = end_pos - line_start;
                        }
                    }
                }
                self.record_edit_with_count("paste-before", count);
            }
            "open-line-below" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].open_line_below(win);
                self.enter_insert_for_change("open-line-below");
            }
            "open-line-above" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].open_line_above(win);
                self.enter_insert_for_change("open-line-above");
            }

            // Undo/Redo
            "undo" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].undo(win);
            }
            "redo" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window_mut();
                self.buffers[idx].redo(win);
            }

            // Mode changes
            "enter-insert-mode" => self.mode = Mode::Insert,
            "enter-insert-mode-after" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr.focused_window_mut().move_right(buf);
                self.mode = Mode::Insert;
            }
            "enter-insert-mode-eol" => {
                let buf = &self.buffers[self.active_buffer_idx()];
                self.window_mgr.focused_window_mut().move_to_line_end(buf);
                self.mode = Mode::Insert;
            }
            "enter-normal-mode" => {
                if self.mode == Mode::Insert {
                    // Finalize dot-repeat record before adjusting cursor
                    self.finalize_insert_for_repeat();
                    let win = self.window_mgr.focused_window_mut();
                    if win.cursor_col > 0 {
                        win.cursor_col -= 1;
                    }
                }
                self.mode = Mode::Normal;
            }
            "enter-command-mode" => {
                self.mode = Mode::Command;
                self.command_line.clear();
            }

            // Window management
            "split-vertical" => {
                let buf_idx = self.active_buffer_idx();
                let area = self.default_area();
                match self
                    .window_mgr
                    .split(SplitDirection::Vertical, buf_idx, area)
                {
                    Ok(_) => {}
                    Err(e) => self.set_status(e),
                }
            }
            "split-horizontal" => {
                let buf_idx = self.active_buffer_idx();
                let area = self.default_area();
                match self
                    .window_mgr
                    .split(SplitDirection::Horizontal, buf_idx, area)
                {
                    Ok(_) => {}
                    Err(e) => self.set_status(e),
                }
            }
            "close-window" => {
                if self
                    .window_mgr
                    .close(self.window_mgr.focused_id())
                    .is_none()
                {
                    self.set_status("Cannot close last window");
                }
            }
            "focus-left" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Left, area);
            }
            "focus-right" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Right, area);
            }
            "focus-up" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Up, area);
            }
            "focus-down" => {
                let area = self.default_area();
                self.window_mgr.focus_direction(Direction::Down, area);
            }

            // Diagnostics
            "view-messages" => {
                self.open_messages_buffer();
            }

            // Placeholder commands (stubs for leader key tree)
            "command-palette" => self.set_status("Not yet implemented: command-palette"),
            "next-buffer" => {
                if self.buffers.len() <= 1 {
                    return true;
                }
                let win = self.window_mgr.focused_window_mut();
                win.buffer_idx = (win.buffer_idx + 1) % self.buffers.len();
                win.cursor_row = 0;
                win.cursor_col = 0;
                let name = self.buffers[win.buffer_idx].name.clone();
                self.set_status(format!("Buffer: {}", name));
            }
            "prev-buffer" => {
                if self.buffers.len() <= 1 {
                    return true;
                }
                let count = self.buffers.len();
                let win = self.window_mgr.focused_window_mut();
                win.buffer_idx = (win.buffer_idx + count - 1) % count;
                win.cursor_row = 0;
                win.cursor_col = 0;
                let name = self.buffers[win.buffer_idx].name.clone();
                self.set_status(format!("Buffer: {}", name));
            }
            "kill-buffer" => {
                let idx = self.active_buffer_idx();
                if self.buffers[idx].modified {
                    self.set_status("Buffer has unsaved changes (save first or use :q!)");
                } else if self.buffers.len() <= 1 {
                    // Replace with empty scratch — never have 0 buffers
                    self.buffers[0] = Buffer::new();
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = 0;
                    win.cursor_col = 0;
                    self.set_status("Buffer killed — [scratch]");
                } else {
                    self.buffers.remove(idx);
                    // Fix all window buffer_idx references
                    for win in self.window_mgr.iter_windows_mut() {
                        if win.buffer_idx == idx {
                            win.buffer_idx = idx.saturating_sub(1).min(self.buffers.len() - 1);
                            win.cursor_row = 0;
                            win.cursor_col = 0;
                        } else if win.buffer_idx > idx {
                            win.buffer_idx -= 1;
                        }
                    }
                    let new_idx = self.active_buffer_idx();
                    let name = self.buffers[new_idx].name.clone();
                    self.set_status(format!("Buffer killed — now: {}", name));
                }
            }
            "switch-buffer" => {
                let list: Vec<String> = self
                    .buffers
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        let modified = if b.modified { " [+]" } else { "" };
                        let current = if i == self.active_buffer_idx() {
                            ">"
                        } else {
                            " "
                        };
                        format!("{}{} {}{}", current, i, b.name, modified)
                    })
                    .collect();
                self.set_status(list.join("  |  "));
            }
            "find-file" => {
                let root = std::env::current_dir().unwrap_or_default();
                self.file_picker = Some(FilePicker::scan(&root));
                self.mode = Mode::FilePicker;
            }
            "recent-files" => self.set_status("Not yet implemented: recent-files"),
            "ai-prompt" => {
                self.open_conversation_buffer();
            }
            "ai-cancel" => {
                // Mark streaming as stopped in conversation buffer.
                // Actual channel cancel is handled by the binary (AiCommand::Cancel).
                if let Some(conv) = self
                    .buffers
                    .iter_mut()
                    .find_map(|b| b.conversation.as_mut())
                {
                    if conv.streaming {
                        conv.streaming = false;
                        conv.streaming_start = None;
                        conv.push_system("[cancelled]");
                        self.set_status("[AI] Cancelled");
                    } else {
                        self.set_status("No active AI request to cancel");
                    }
                } else {
                    self.set_status("No AI conversation active");
                }
            }
            "describe-key" => self.set_status("Not yet implemented: describe-key"),
            "describe-command" => self.set_status("Not yet implemented: describe-command"),
            "set-theme" => {
                // Stub: in full implementation, opens command-line for theme name.
                // For now, set status with available themes.
                let names = bundled_theme_names().join(", ");
                self.set_status(format!("Available themes: {} — use :theme <name>", names));
            }
            "cycle-theme" => {
                self.cycle_theme();
            }

            // Debug commands
            "debug-self" => {
                self.start_self_debug();
            }
            "debug-start" => self.set_status("Not yet implemented: debug-start (DAP)"),
            "debug-stop" => {
                if self.debug_state.is_some() {
                    self.debug_state = None;
                    self.set_status("Debug session ended");
                } else {
                    self.set_status("No active debug session");
                }
            }
            "debug-continue" | "debug-step-over" | "debug-step-into" | "debug-step-out" => {
                if self.debug_state.is_none() {
                    self.set_status("No active debug session");
                } else {
                    self.set_status(format!("Not yet implemented: {}", name));
                }
            }
            "debug-toggle-breakpoint" => {
                if self.debug_state.is_some() {
                    let buf_idx = self.active_buffer_idx();
                    let line = self.window_mgr.focused_window().cursor_row as i64 + 1;
                    let source = self.buffers[buf_idx].name.clone();
                    let state = self.debug_state.as_mut().unwrap();
                    // Toggle: remove if exists at this line, else add
                    let existing = state
                        .breakpoints
                        .get(&source)
                        .and_then(|bps| bps.iter().find(|b| b.line == line).map(|b| b.id));
                    if let Some(id) = existing {
                        state.remove_breakpoint(id);
                        self.set_status(format!("Breakpoint removed: {}:{}", source, line));
                    } else {
                        state.add_breakpoint(&source, line);
                        self.set_status(format!("Breakpoint set: {}:{}", source, line));
                    }
                } else {
                    self.set_status("No active debug session");
                }
            }
            "debug-inspect" => {
                if self.debug_state.is_some() {
                    self.set_status("Debug inspect: use :debug-eval <expr> or AI agent");
                } else {
                    self.set_status("No active debug session");
                }
            }

            // Visual mode
            "enter-visual-char" => match self.mode {
                Mode::Visual(VisualType::Char) => self.mode = Mode::Normal,
                Mode::Visual(VisualType::Line) => self.mode = Mode::Visual(VisualType::Char),
                _ => self.enter_visual_mode(VisualType::Char),
            },
            "enter-visual-line" => match self.mode {
                Mode::Visual(VisualType::Line) => self.mode = Mode::Normal,
                Mode::Visual(VisualType::Char) => self.mode = Mode::Visual(VisualType::Line),
                _ => self.enter_visual_mode(VisualType::Line),
            },
            "visual-delete" => self.visual_delete(),
            "visual-yank" => self.visual_yank(),
            "visual-change" => self.visual_change(),

            // Search
            "search-forward-start" => {
                self.search_state.direction = SearchDirection::Forward;
                self.search_input.clear();
                self.mode = Mode::Search;
            }
            "search-backward-start" => {
                self.search_state.direction = SearchDirection::Backward;
                self.search_input.clear();
                self.mode = Mode::Search;
            }
            "search-next" => {
                for _ in 0..n {
                    self.jump_to_next_match(true);
                }
            }
            "search-prev" => {
                for _ in 0..n {
                    self.jump_to_next_match(false);
                }
            }
            "search-word-under-cursor" => {
                self.search_word_at_cursor();
            }
            "clear-search-highlight" => {
                self.search_state.highlight_active = false;
            }

            // Change operators — delete range, then enter insert mode
            "change-line" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let row = win.cursor_row;
                let line_start = self.buffers[idx].rope().line_to_char(row);
                let line_len = self.buffers[idx].line_len(row);
                if line_len > 0 {
                    let text = self.buffers[idx].text_range(line_start, line_start + line_len);
                    self.buffers[idx].delete_range(line_start, line_start + line_len);
                    self.registers.insert('"', text);
                }
                let win = self.window_mgr.focused_window_mut();
                win.cursor_col = 0;
                self.enter_insert_for_change("change-line");
            }
            "change-word-forward" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let end = crate::word::word_start_forward(self.buffers[idx].rope(), start);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.enter_insert_for_change("change-word-forward");
            }
            "change-to-line-end" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let start = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let line_start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                let line_len = self.buffers[idx].line_len(win.cursor_row);
                let end = line_start + line_len;
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.clamp_cursor(&self.buffers[idx]);
                }
                self.enter_insert_for_change("change-to-line-end");
            }
            "change-to-line-start" => {
                let idx = self.active_buffer_idx();
                let win = self.window_mgr.focused_window();
                let end = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                let start = self.buffers[idx].rope().line_to_char(win.cursor_row);
                if end > start {
                    let text = self.buffers[idx].text_range(start, end);
                    self.buffers[idx].delete_range(start, end);
                    self.registers.insert('"', text);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_col = 0;
                }
                self.enter_insert_for_change("change-to-line-start");
            }

            // Replace char (pending — next key replaces char under cursor)
            "replace-char-await" => {
                self.pending_char_command = Some("replace-char".to_string());
            }

            // Dot repeat
            "dot-repeat" => {
                self.replay_last_edit();
            }

            // File operations
            "save" => self.save_current_buffer(),
            "quit" => {
                self.execute_command("q");
            }
            "force-quit" => {
                self.execute_command("q!");
            }
            "save-and-quit" => {
                self.execute_command("wq");
            }

            _ => return false,
        }
        true
    }

    // --- Visual mode ---

    /// Enter visual mode, recording the anchor at the current cursor position.
    pub fn enter_visual_mode(&mut self, vtype: VisualType) {
        let win = self.window_mgr.focused_window();
        self.visual_anchor_row = win.cursor_row;
        self.visual_anchor_col = win.cursor_col;
        self.mode = Mode::Visual(vtype);
    }

    /// Compute the ordered char-offset range for the current visual selection.
    /// Returns `(start, end)` where `start..end` is the selected range.
    pub fn visual_selection_range(&self) -> (usize, usize) {
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();

        match self.mode {
            Mode::Visual(VisualType::Line) => {
                let min_row = self.visual_anchor_row.min(win.cursor_row);
                let max_row = self.visual_anchor_row.max(win.cursor_row);
                let start = buf.rope().line_to_char(min_row);
                let end = if max_row + 1 < buf.line_count() {
                    buf.rope().line_to_char(max_row + 1)
                } else {
                    buf.rope().len_chars()
                };
                (start, end)
            }
            _ => {
                // Charwise
                let anchor = buf.char_offset_at(self.visual_anchor_row, self.visual_anchor_col);
                let cursor = buf.char_offset_at(win.cursor_row, win.cursor_col);
                let start = anchor.min(cursor);
                let end = (anchor.max(cursor) + 1).min(buf.rope().len_chars());
                (start, end)
            }
        }
    }

    /// Delete the visual selection, storing it in the default register.
    pub fn visual_delete(&mut self) {
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            return;
        }
        let idx = self.active_buffer_idx();
        let text = self.buffers[idx].text_range(start, end);
        self.buffers[idx].delete_range(start, end);
        self.registers.insert('"', text);
        // Move cursor to start of deleted range
        let rope = self.buffers[idx].rope();
        let new_row = rope.char_to_line(start.min(rope.len_chars().saturating_sub(1).max(0)));
        let line_start = rope.line_to_char(new_row);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = start.saturating_sub(line_start);
        win.clamp_cursor(&self.buffers[idx]);
        self.mode = Mode::Normal;
    }

    /// Yank the visual selection into the default register without deleting.
    pub fn visual_yank(&mut self) {
        let (start, end) = self.visual_selection_range();
        if start >= end {
            self.mode = Mode::Normal;
            return;
        }
        let idx = self.active_buffer_idx();
        let text = self.buffers[idx].text_range(start, end);
        self.registers.insert('"', text);
        // Move cursor to start of selection
        let rope = self.buffers[idx].rope();
        let new_row = rope.char_to_line(start);
        let line_start = rope.line_to_char(new_row);
        let win = self.window_mgr.focused_window_mut();
        win.cursor_row = new_row;
        win.cursor_col = start - line_start;
        self.mode = Mode::Normal;
    }

    /// Change the visual selection: delete it and enter insert mode.
    pub fn visual_change(&mut self) {
        self.visual_delete();
        self.mode = Mode::Insert;
    }

    /// Dispatch a char-argument motion (f/F/t/T/r + char). Returns true if handled.
    pub fn dispatch_char_motion(&mut self, command: &str, ch: char) -> bool {
        if command == "replace-char" {
            let idx = self.active_buffer_idx();
            let win = self.window_mgr.focused_window();
            let row = win.cursor_row;
            let col = win.cursor_col;
            let line_len = self.buffers[idx].line_len(row);
            if col < line_len {
                let offset = self.buffers[idx].char_offset_at(row, col);
                self.buffers[idx].delete_range(offset, offset + 1);
                self.buffers[idx].insert_text_at(offset, &ch.to_string());
                // Record for dot-repeat
                self.last_edit = Some(EditRecord {
                    command: "replace-char".to_string(),
                    inserted_text: None,
                    char_arg: Some(ch),
                    count: None,
                });
            }
            return true;
        }

        let repeat = self.pending_char_count;
        self.pending_char_count = 1;
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window_mut();
        match command {
            "find-char-forward" => {
                for _ in 0..repeat {
                    win.move_find_char(buf, ch);
                }
            }
            "find-char-backward" => {
                for _ in 0..repeat {
                    win.move_find_char_back(buf, ch);
                }
            }
            "till-char-forward" => {
                for _ in 0..repeat {
                    win.move_till_char(buf, ch);
                }
            }
            "till-char-backward" => {
                for _ in 0..repeat {
                    win.move_till_char_back(buf, ch);
                }
            }
            _ => return false,
        }
        true
    }

    /// Enter insert mode from a change command, recording state for dot-repeat.
    fn enter_insert_for_change(&mut self, command: &str) {
        let idx = self.active_buffer_idx();
        let win = self.window_mgr.focused_window();
        let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
        self.insert_start_offset = Some(offset);
        self.insert_initiated_by = Some(command.to_string());
        self.mode = Mode::Insert;
    }

    /// Called when exiting insert mode to finalize the dot-repeat record.
    /// Captures any text that was typed during the insert session.
    pub fn finalize_insert_for_repeat(&mut self) {
        if let (Some(cmd), Some(start_offset)) = (
            self.insert_initiated_by.take(),
            self.insert_start_offset.take(),
        ) {
            let idx = self.active_buffer_idx();
            let win = self.window_mgr.focused_window();
            let current_offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
            // The cursor is at the end of what was inserted (or beyond).
            // In insert mode the cursor is *after* the last inserted char,
            // so the inserted text is start_offset..current_offset.
            let inserted = if current_offset > start_offset {
                Some(self.buffers[idx].text_range(start_offset, current_offset))
            } else {
                None
            };
            self.last_edit = Some(EditRecord {
                command: cmd,
                inserted_text: inserted,
                char_arg: None,
                count: None,
            });
        }
    }

    /// Record a non-insert edit for dot-repeat (delete, paste, etc.).
    pub fn record_edit(&mut self, command: &str) {
        self.last_edit = Some(EditRecord {
            command: command.to_string(),
            inserted_text: None,
            char_arg: None,
            count: None,
        });
    }

    /// Record a non-insert edit with count for dot-repeat.
    fn record_edit_with_count(&mut self, command: &str, count: Option<usize>) {
        self.last_edit = Some(EditRecord {
            command: command.to_string(),
            inserted_text: None,
            char_arg: None,
            count,
        });
    }

    /// Replay the last recorded edit (dot-repeat).
    fn replay_last_edit(&mut self) {
        let record = match self.last_edit.clone() {
            Some(r) => r,
            None => return,
        };

        // Restore count prefix from the recorded edit so the repeated
        // dispatch uses the same count as the original.
        self.count_prefix = record.count;

        match record.command.as_str() {
            "replace-char" => {
                if let Some(ch) = record.char_arg {
                    self.dispatch_char_motion("replace-char", ch);
                }
            }
            "change-line"
            | "change-word-forward"
            | "change-to-line-end"
            | "change-to-line-start" => {
                // Re-dispatch the change command (which enters insert mode)
                self.dispatch_builtin(&record.command);
                // Now we need to insert the recorded text and return to normal mode
                if let Some(ref text) = record.inserted_text {
                    let idx = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window();
                    let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                    self.buffers[idx].insert_text_at(offset, text);
                    // Move cursor past inserted text
                    let new_offset = offset + text.chars().count();
                    let rope = self.buffers[idx].rope();
                    let new_row =
                        rope.char_to_line(new_offset.min(rope.len_chars().saturating_sub(1)));
                    let line_start = rope.line_to_char(new_row);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = new_row;
                    win.cursor_col = new_offset.saturating_sub(line_start);
                }
                // Exit insert mode without recording (would overwrite the repeat record)
                self.mode = Mode::Normal;
                self.insert_initiated_by = None;
                self.insert_start_offset = None;
                // Restore the last_edit since dispatch_builtin would have set up
                // insert_initiated_by, and we need to preserve the original record
                self.last_edit = Some(record);
            }
            "open-line-below" | "open-line-above" => {
                self.dispatch_builtin(&record.command);
                if let Some(ref text) = record.inserted_text {
                    let idx = self.active_buffer_idx();
                    let win = self.window_mgr.focused_window();
                    let offset = self.buffers[idx].char_offset_at(win.cursor_row, win.cursor_col);
                    self.buffers[idx].insert_text_at(offset, text);
                    let new_offset = offset + text.chars().count();
                    let rope = self.buffers[idx].rope();
                    let new_row =
                        rope.char_to_line(new_offset.min(rope.len_chars().saturating_sub(1)));
                    let line_start = rope.line_to_char(new_row);
                    let win = self.window_mgr.focused_window_mut();
                    win.cursor_row = new_row;
                    win.cursor_col = new_offset.saturating_sub(line_start);
                }
                self.mode = Mode::Normal;
                self.insert_initiated_by = None;
                self.insert_start_offset = None;
                self.last_edit = Some(record);
            }
            _ => {
                // Simple commands: delete-line, delete-char-forward, paste-after, etc.
                self.dispatch_builtin(&record.command);
            }
        }
    }

    /// Default area for window operations when we don't have the real terminal size.
    /// The renderer will provide real dimensions at render time.
    fn default_area(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: 120,
            height: 40,
        }
    }

    /// Parse and execute a command-line string (the text after ':').
    pub fn execute_command(&mut self, cmd: &str) -> bool {
        let cmd = cmd.trim();
        let (command, args) = match cmd.split_once(' ') {
            Some((c, a)) => (c, Some(a.trim())),
            None => (cmd, None),
        };

        match command {
            "w" => {
                if let Some(path) = args {
                    let idx = self.active_buffer_idx();
                    self.buffers[idx].set_file_path(std::path::PathBuf::from(path));
                }
                self.save_current_buffer();
                true
            }
            "q" => {
                if self.active_buffer().modified {
                    self.set_status("No write since last change (add ! to override)");
                } else {
                    self.running = false;
                }
                true
            }
            "q!" => {
                self.running = false;
                true
            }
            "wq" | "x" => {
                self.save_current_buffer();
                if self.running && !self.active_buffer().modified {
                    self.running = false;
                }
                true
            }
            "e" => {
                if let Some(path) = args {
                    self.open_file(path);
                } else {
                    self.set_status("Usage: :e <filename>");
                }
                true
            }
            "vsplit" => {
                self.dispatch_builtin("split-vertical");
                true
            }
            "split" => {
                self.dispatch_builtin("split-horizontal");
                true
            }
            "close" => {
                self.dispatch_builtin("close-window");
                true
            }
            "messages" => {
                self.dispatch_builtin("view-messages");
                true
            }
            "theme" => {
                if let Some(name) = args {
                    self.set_theme_by_name(name);
                } else {
                    let names = bundled_theme_names().join(", ");
                    self.set_status(format!("Usage: :theme <name>  Available: {}", names));
                }
                true
            }
            "noh" | "nohlsearch" => {
                self.search_state.highlight_active = false;
                true
            }
            _ => {
                // Check for substitute commands: s/.../.../  or %s/.../.../
                if cmd.starts_with("s/") || cmd.starts_with("%s/") {
                    self.execute_substitute_command(cmd);
                    return true;
                }
                self.set_status(format!("Unknown command: {}", command));
                false
            }
        }
    }

    /// Compile the search pattern, cache all matches, and jump to the first result.
    pub fn execute_search(&mut self) {
        let pattern = self.search_input.clone();
        if pattern.is_empty() {
            return;
        }

        match Regex::new(&pattern) {
            Ok(re) => {
                let buf = &self.buffers[self.active_buffer_idx()];
                let matches = search::find_all(buf.rope(), &re);
                let match_count = matches.len();

                self.search_state.pattern = pattern;
                self.search_state.regex = Some(re);
                self.search_state.matches = matches;
                self.search_state.highlight_active = true;

                if match_count > 0 {
                    self.jump_to_next_match(true);
                } else {
                    self.set_status("Pattern not found");
                }
            }
            Err(e) => {
                self.set_status(format!("Invalid regex: {}", e));
            }
        }
    }

    /// Recompute search matches after buffer edit (call when buffer changes).
    pub fn recompute_search_matches(&mut self) {
        if let Some(ref re) = self.search_state.regex {
            let buf = &self.buffers[self.active_buffer_idx()];
            self.search_state.matches = search::find_all(buf.rope(), re);
        }
    }

    /// Navigate to the next/prev match. `same_direction` = true means n, false means N.
    fn jump_to_next_match(&mut self, same_direction: bool) {
        let re = match self.search_state.regex {
            Some(ref re) => re.clone(),
            None => {
                self.set_status("No previous search");
                return;
            }
        };

        let direction = if same_direction {
            self.search_state.direction
        } else {
            match self.search_state.direction {
                SearchDirection::Forward => SearchDirection::Backward,
                SearchDirection::Backward => SearchDirection::Forward,
            }
        };

        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);

        if let Some(m) = search::find_next(buf.rope(), &re, char_offset, direction, true) {
            let rope = buf.rope();
            let new_row = rope.char_to_line(m.start);
            let line_start = rope.line_to_char(new_row);
            let new_col = m.start - line_start;

            let win = self.window_mgr.focused_window_mut();
            win.cursor_row = new_row;
            win.cursor_col = new_col;

            // Show "N of M" status
            let matches = &self.search_state.matches;
            let idx = matches
                .iter()
                .position(|sm| sm.start == m.start)
                .map(|i| i + 1)
                .unwrap_or(0);
            self.set_status(format!("[{}/{}]", idx, matches.len()));
        } else {
            self.set_status("Pattern not found");
        }
    }

    /// Search for word under cursor (* command).
    fn search_word_at_cursor(&mut self) {
        let buf = &self.buffers[self.active_buffer_idx()];
        let win = self.window_mgr.focused_window();
        let char_offset = buf.char_offset_at(win.cursor_row, win.cursor_col);

        if let Some(pattern) = search::word_at_offset(buf.rope(), char_offset) {
            self.search_input = pattern.clone();
            self.search_state.direction = SearchDirection::Forward;
            match Regex::new(&pattern) {
                Ok(re) => {
                    let matches = search::find_all(buf.rope(), &re);
                    self.search_state.pattern = pattern;
                    self.search_state.regex = Some(re);
                    self.search_state.matches = matches;
                    self.search_state.highlight_active = true;
                    self.jump_to_next_match(true);
                }
                Err(e) => {
                    self.set_status(format!("Invalid regex: {}", e));
                }
            }
        } else {
            self.set_status("No word under cursor");
        }
    }

    /// Execute a substitute command (`:s/old/new/g` or `:%s/old/new/g`).
    fn execute_substitute_command(&mut self, cmd: &str) {
        let sub = match search::parse_substitute(cmd) {
            Ok(s) => s,
            Err(e) => {
                self.set_status(format!("Substitute error: {}", e));
                return;
            }
        };

        let re = match Regex::new(&sub.pattern) {
            Ok(r) => r,
            Err(e) => {
                self.set_status(format!("Invalid pattern: {}", e));
                return;
            }
        };

        let idx = self.active_buffer_idx();
        let win_row = self.window_mgr.focused_window().cursor_row;

        let (start_line, end_line) = if sub.whole_buffer {
            (0, self.buffers[idx].line_count())
        } else {
            (win_row, win_row + 1)
        };

        let mut total_subs = 0;
        let mut lines_changed = 0;

        // Process lines in reverse so char offsets remain stable
        for line_idx in (start_line..end_line).rev() {
            let line_start = self.buffers[idx].rope().line_to_char(line_idx);
            let line_text = self.buffers[idx].line_text(line_idx);
            let line_content = line_text.trim_end_matches('\n');

            let (new_text, count) =
                search::substitute_line(line_content, &re, &sub.replacement, sub.global);
            if count > 0 {
                total_subs += count;
                lines_changed += 1;
                let end_offset = line_start + line_content.chars().count();
                self.buffers[idx].delete_range(line_start, end_offset);
                self.buffers[idx].insert_text_at(line_start, &new_text);
            }
        }

        if total_subs > 0 {
            self.set_status(format!(
                "{} substitution{} on {} line{}",
                total_subs,
                if total_subs == 1 { "" } else { "s" },
                lines_changed,
                if lines_changed == 1 { "" } else { "s" }
            ));
            self.recompute_search_matches();
        } else {
            self.set_status("Pattern not found");
        }
    }

    fn save_current_buffer(&mut self) {
        let idx = self.active_buffer_idx();
        match self.buffers[idx].save() {
            Ok(()) => {
                let name = self.buffers[idx].name.clone();
                self.set_status(format!("\"{}\" written", name));
            }
            Err(e) => {
                self.set_status(format!("Error saving: {}", e));
            }
        }
    }

    /// Set the editor theme by name. Looks up bundled themes.
    pub fn set_theme_by_name(&mut self, name: &str) {
        match Theme::load(name, &BundledResolver) {
            Ok(theme) => {
                self.set_status(format!("Theme: {}", theme.name));
                self.theme = theme;
            }
            Err(e) => {
                self.set_status(format!("Failed to load theme '{}': {}", name, e));
            }
        }
    }

    /// Cycle to the next bundled theme.
    pub fn cycle_theme(&mut self) {
        let names = bundled_theme_names();
        if names.is_empty() {
            return;
        }
        let current_idx = names
            .iter()
            .position(|n| n == &self.theme.name)
            .unwrap_or(0);
        let next_idx = (current_idx + 1) % names.len();
        self.set_theme_by_name(&names[next_idx]);
    }

    /// Open the *Messages* buffer showing the in-editor log.
    /// Uses `BufferKind::Messages` — the renderer reads live from `editor.message_log`.
    /// No rope copy needed; the buffer is just a view marker.
    pub fn open_messages_buffer(&mut self) {
        let existing_idx = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Messages);

        if let Some(idx) = existing_idx {
            self.window_mgr.focused_window_mut().buffer_idx = idx;
        } else {
            self.buffers.push(Buffer::new_messages());
            let new_idx = self.buffers.len() - 1;
            self.window_mgr.focused_window_mut().buffer_idx = new_idx;
        }
        let count = self.message_log.len();
        self.set_status(format!("{} log entries", count));
    }

    /// Open (or focus) the *AI* conversation buffer and enter ConversationInput mode.
    pub fn open_conversation_buffer(&mut self) {
        let conv_idx = self
            .buffers
            .iter()
            .position(|b| b.kind == crate::buffer::BufferKind::Conversation);
        let idx = if let Some(i) = conv_idx {
            i
        } else {
            self.buffers.push(Buffer::new_conversation("*AI*"));
            self.buffers.len() - 1
        };
        self.window_mgr.focused_window_mut().buffer_idx = idx;
        self.mode = Mode::ConversationInput;
    }

    /// Start a self-debug session, populating DebugState with the editor's
    /// current Rust state. Scheme state is populated separately when the
    /// binary calls `populate_scheme_debug_state()` (since core doesn't own SchemeRuntime).
    pub fn start_self_debug(&mut self) {
        use crate::debug::{Scope, StackFrame, Variable};

        let mut state = DebugState::new_self_debug();

        // Synthetic stack frame for the Rust core event loop
        state.stack_frames.push(StackFrame {
            id: 1,
            name: format!("event_loop [mode={:?}]", self.mode),
            source: Some("crates/mae/src/main.rs".into()),
            line: 0,
            column: 0,
        });

        // Scopes for Rust Core thread
        state.scopes.push(Scope {
            name: "Editor State".into(),
            variables_reference: 1,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "Active Buffer".into(),
            variables_reference: 2,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "Active Window".into(),
            variables_reference: 3,
            expensive: false,
        });
        state.scopes.push(Scope {
            name: "All Buffers".into(),
            variables_reference: 4,
            expensive: false,
        });

        // Editor State variables
        let buf = self.active_buffer();
        let win = self.window_mgr.focused_window();
        state.variables.insert(
            "Editor State".into(),
            vec![
                Variable {
                    name: "mode".into(),
                    value: format!("{:?}", self.mode),
                    var_type: Some("Mode".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "running".into(),
                    value: format!("{}", self.running),
                    var_type: Some("bool".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "status_msg".into(),
                    value: self.status_msg.clone(),
                    var_type: Some("String".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "command_line".into(),
                    value: self.command_line.clone(),
                    var_type: Some("String".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "buffer_count".into(),
                    value: format!("{}", self.buffers.len()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "window_count".into(),
                    value: format!("{}", self.window_mgr.window_count()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "theme".into(),
                    value: self.theme.name.clone(),
                    var_type: Some("Theme".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "command_count".into(),
                    value: format!("{}", self.commands.len()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "message_log_entries".into(),
                    value: format!("{}", self.message_log.len()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
            ],
        );

        // Active Buffer variables
        state.variables.insert(
            "Active Buffer".into(),
            vec![
                Variable {
                    name: "name".into(),
                    value: buf.name.clone(),
                    var_type: Some("String".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "kind".into(),
                    value: format!("{:?}", buf.kind),
                    var_type: Some("BufferKind".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "modified".into(),
                    value: format!("{}", buf.modified),
                    var_type: Some("bool".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "line_count".into(),
                    value: format!("{}", buf.line_count()),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "file_path".into(),
                    value: buf
                        .file_path()
                        .map_or("None".to_string(), |p| p.display().to_string()),
                    var_type: Some("Option<PathBuf>".into()),
                    variables_reference: 0,
                },
            ],
        );

        // Active Window variables
        state.variables.insert(
            "Active Window".into(),
            vec![
                Variable {
                    name: "cursor_row".into(),
                    value: format!("{}", win.cursor_row),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "cursor_col".into(),
                    value: format!("{}", win.cursor_col),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "scroll_offset".into(),
                    value: format!("{}", win.scroll_offset),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
                Variable {
                    name: "buffer_idx".into(),
                    value: format!("{}", win.buffer_idx),
                    var_type: Some("usize".into()),
                    variables_reference: 0,
                },
            ],
        );

        // All Buffers (expandable summary)
        let all_bufs: Vec<Variable> = self
            .buffers
            .iter()
            .enumerate()
            .map(|(i, b)| Variable {
                name: format!("[{}]", i),
                value: format!(
                    "{} ({:?}{})",
                    b.name,
                    b.kind,
                    if b.modified { ", modified" } else { "" }
                ),
                var_type: Some("Buffer".into()),
                variables_reference: 0,
            })
            .collect();
        state.variables.insert("All Buffers".into(), all_bufs);

        // Mark as stopped (self-debug is always "stopped" — it's a snapshot)
        state.stopped_location = Some(("crates/mae/src/main.rs".into(), 0));

        self.debug_state = Some(state);
        self.set_status("Self-debug: Rust state captured. Use SPC d v to inspect.");
    }

    /// Refresh the Rust portion of the self-debug state (call on each debug render).
    pub fn refresh_self_debug(&mut self) {
        if let Some(ref state) = self.debug_state {
            if state.target == DebugTarget::SelfDebug {
                // Re-capture by starting fresh
                self.start_self_debug();
            }
        }
    }

    pub fn open_file(&mut self, path: &str) {
        match Buffer::from_file(Path::new(path)) {
            Ok(buf) => {
                let name = buf.name.clone();
                self.buffers.push(buf);
                let new_idx = self.buffers.len() - 1;
                self.window_mgr.focused_window_mut().buffer_idx = new_idx;
                self.set_status(format!("\"{}\" opened", name));
            }
            Err(e) => {
                self.set_status(format!("Error opening: {}", e));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LookupResult, VisualType};
    use std::fs;

    #[test]
    fn new_editor_has_scratch_buffer() {
        let editor = Editor::new();
        assert_eq!(editor.buffers.len(), 1);
        assert_eq!(editor.active_buffer().name, "[scratch]");
        assert!(editor.running);
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn quit_clean_buffer() {
        let mut editor = Editor::new();
        editor.execute_command("q");
        assert!(!editor.running);
    }

    #[test]
    fn quit_modified_buffer_refuses() {
        let mut editor = Editor::new();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'x');
        editor.execute_command("q");
        assert!(editor.running);
        assert!(editor.status_msg.contains("No write"));
    }

    #[test]
    fn force_quit_modified_buffer() {
        let mut editor = Editor::new();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'x');
        editor.execute_command("q!");
        assert!(!editor.running);
    }

    #[test]
    fn save_command() {
        let dir = std::env::temp_dir().join("mae_test_save_cmd");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        fs::write(&path, "original").unwrap();

        let buf = Buffer::from_file(&path).unwrap();
        let mut editor = Editor::with_buffer(buf);
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, '!');
        editor.execute_command("w");
        assert!(!editor.active_buffer().modified);
        assert!(editor.status_msg.contains("written"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_and_quit() {
        let dir = std::env::temp_dir().join("mae_test_wq");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.txt");
        fs::write(&path, "hi").unwrap();

        let buf = Buffer::from_file(&path).unwrap();
        let mut editor = Editor::with_buffer(buf);
        editor.execute_command("wq");
        assert!(!editor.running);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_file_command() {
        let dir = std::env::temp_dir().join("mae_test_open");
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("new.txt");
        fs::write(&path, "new content").unwrap();

        let mut editor = Editor::new();
        editor.open_file(path.to_str().unwrap());
        assert_eq!(editor.buffers.len(), 2);
        // Focused window should now point to the new buffer
        assert_eq!(editor.active_buffer_idx(), 1);
        assert_eq!(editor.active_buffer().text(), "new content");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_command_sets_status() {
        let mut editor = Editor::new();
        let result = editor.execute_command("bogus");
        assert!(!result);
        assert!(editor.status_msg.contains("Unknown command"));
    }

    #[test]
    fn dispatch_builtin_movement() {
        let mut editor = Editor::new();
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'a');
        }
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, '\n');
        }
        {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, 'b');
        }
        assert!(editor.dispatch_builtin("move-up"));
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
        assert!(editor.dispatch_builtin("move-to-line-end"));
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 1);
        assert!(editor.dispatch_builtin("move-to-line-start"));
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
    }

    #[test]
    fn dispatch_builtin_mode_changes() {
        let mut editor = Editor::new();
        assert_eq!(editor.mode, Mode::Normal);
        editor.dispatch_builtin("enter-insert-mode");
        assert_eq!(editor.mode, Mode::Insert);
        editor.dispatch_builtin("enter-normal-mode");
        assert_eq!(editor.mode, Mode::Normal);
        editor.dispatch_builtin("enter-command-mode");
        assert_eq!(editor.mode, Mode::Command);
    }

    #[test]
    fn dispatch_builtin_unknown_returns_false() {
        let mut editor = Editor::new();
        assert!(!editor.dispatch_builtin("nonexistent-command"));
    }

    #[test]
    fn mode_transitions() {
        let mut editor = Editor::new();
        assert_eq!(editor.mode, Mode::Normal);
        editor.mode = Mode::Insert;
        assert_eq!(editor.mode, Mode::Insert);
        editor.mode = Mode::Command;
        assert_eq!(editor.mode, Mode::Command);
    }

    #[test]
    fn split_and_focus() {
        let mut editor = Editor::new();
        assert_eq!(editor.window_mgr.window_count(), 1);
        editor.dispatch_builtin("split-vertical");
        assert_eq!(editor.window_mgr.window_count(), 2);
        // Focus should still be on the original window
        assert_eq!(editor.active_buffer_idx(), 0);
        editor.dispatch_builtin("focus-right");
        // After focusing right, should be on the second window
        // (which also views buffer 0 since we split the same buffer)
        assert_eq!(editor.active_buffer_idx(), 0);
    }

    #[test]
    fn leader_bindings_exist() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        use crate::keymap::{parse_key_seq_spaced, LookupResult};
        // SPC should be a prefix
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC")),
            LookupResult::Prefix
        );
        // SPC b should be a prefix
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC b")),
            LookupResult::Prefix
        );
        // SPC b s should be save
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC b s")),
            LookupResult::Exact("save")
        );
        // SPC w v should be split-vertical
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC w v")),
            LookupResult::Exact("split-vertical")
        );
        // SPC a a should be ai-prompt
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC a a")),
            LookupResult::Exact("ai-prompt")
        );
    }

    #[test]
    fn which_key_prefix_initialized_empty() {
        let editor = Editor::new();
        assert!(editor.which_key_prefix.is_empty());
    }

    #[test]
    fn placeholder_commands_dispatch() {
        let mut editor = Editor::new();
        // ai-prompt is no longer a stub — it creates a conversation buffer
        assert!(editor.dispatch_builtin("ai-prompt"));
        assert_eq!(editor.mode, Mode::ConversationInput);
        assert!(editor.dispatch_builtin("kill-buffer"));
        assert!(editor.dispatch_builtin("command-palette"));
        assert!(editor.dispatch_builtin("describe-key"));
    }

    #[test]
    fn all_leader_targets_registered() {
        let editor = Editor::new();
        let leader_targets = [
            "command-palette",
            "save",
            "kill-buffer",
            "next-buffer",
            "prev-buffer",
            "find-file",
            "split-vertical",
            "split-horizontal",
            "close-window",
            "focus-left",
            "focus-down",
            "focus-up",
            "focus-right",
            "ai-prompt",
            "ai-cancel",
            "describe-key",
            "describe-command",
            "quit",
            "force-quit",
            "debug-self",
            "debug-start",
            "debug-stop",
            "debug-continue",
            "debug-step-over",
            "debug-step-into",
            "debug-step-out",
            "debug-toggle-breakpoint",
            "debug-inspect",
        ];
        for target in &leader_targets {
            assert!(
                editor.commands.contains(target),
                "Command '{}' not registered",
                target
            );
        }
    }

    #[test]
    fn ctrl_w_bindings_are_two_keys() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        use crate::keymap::{parse_key_seq_spaced, LookupResult};
        // C-w v should be 2 keys (Ctrl-w then v), not 3
        let seq = parse_key_seq_spaced("C-w v");
        assert_eq!(seq.len(), 2);
        assert_eq!(normal.lookup(&seq), LookupResult::Exact("split-vertical"));
    }

    #[test]
    fn ai_prompt_creates_conversation_buffer() {
        let mut editor = Editor::new();
        assert_eq!(editor.buffers.len(), 1);
        assert_eq!(editor.mode, Mode::Normal);

        editor.dispatch_builtin("ai-prompt");

        assert_eq!(editor.mode, Mode::ConversationInput);
        assert_eq!(editor.buffers.len(), 2);
        assert_eq!(
            editor.buffers[1].kind,
            crate::buffer::BufferKind::Conversation
        );
        assert_eq!(editor.buffers[1].name, "*AI*");
        assert_eq!(editor.active_buffer_idx(), 1);
    }

    #[test]
    fn ai_prompt_reuses_existing_conversation() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("ai-prompt");
        assert_eq!(editor.buffers.len(), 2);

        // Go back to normal mode and switch to scratch buffer
        editor.mode = Mode::Normal;
        editor.window_mgr.focused_window_mut().buffer_idx = 0;

        // Second ai-prompt should reuse, not create another
        editor.dispatch_builtin("ai-prompt");
        assert_eq!(editor.buffers.len(), 2);
        assert_eq!(editor.active_buffer_idx(), 1);
        assert_eq!(editor.mode, Mode::ConversationInput);
    }

    #[test]
    fn ai_cancel_when_streaming() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("ai-prompt");
        // Simulate streaming state
        if let Some(conv) = editor.buffers[1].conversation.as_mut() {
            conv.streaming = true;
            conv.streaming_start = Some(std::time::Instant::now());
        }
        editor.dispatch_builtin("ai-cancel");
        let conv = editor.buffers[1].conversation.as_ref().unwrap();
        assert!(!conv.streaming);
        assert!(conv.streaming_start.is_none());
        assert!(editor.status_msg.contains("Cancelled"));
    }

    #[test]
    fn ai_cancel_when_not_streaming() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("ai-prompt");
        editor.dispatch_builtin("ai-cancel");
        assert!(editor.status_msg.contains("No active AI request"));
    }

    #[test]
    fn close_window_returns_to_single() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("split-vertical");
        assert_eq!(editor.window_mgr.window_count(), 2);
        editor.dispatch_builtin("close-window");
        assert_eq!(editor.window_mgr.window_count(), 1);
    }

    #[test]
    fn debug_state_starts_none() {
        let editor = Editor::new();
        assert!(editor.debug_state.is_none());
    }

    #[test]
    fn debug_self_populates_state() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("debug-self");
        assert!(editor.debug_state.is_some());

        let state = editor.debug_state.as_ref().unwrap();
        assert_eq!(state.target, crate::debug::DebugTarget::SelfDebug);
        assert_eq!(state.threads.len(), 2);
        assert_eq!(state.threads[0].name, "Rust Core");
        assert_eq!(state.threads[1].name, "Scheme Runtime");

        // Should have Rust state scopes
        assert!(state.variables.contains_key("Editor State"));
        assert!(state.variables.contains_key("Active Buffer"));
        assert!(state.variables.contains_key("Active Window"));
        assert!(state.variables.contains_key("All Buffers"));
    }

    #[test]
    fn debug_self_captures_correct_values() {
        let mut editor = Editor::new();
        editor.mode = Mode::Insert;
        editor.dispatch_builtin("debug-self");

        let state = editor.debug_state.as_ref().unwrap();
        let editor_vars = &state.variables["Editor State"];
        let mode_var = editor_vars.iter().find(|v| v.name == "mode").unwrap();
        assert_eq!(mode_var.value, "Insert");
    }

    #[test]
    fn debug_stop_clears_state() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("debug-self");
        assert!(editor.debug_state.is_some());
        editor.dispatch_builtin("debug-stop");
        assert!(editor.debug_state.is_none());
        assert!(editor.status_msg.contains("ended"));
    }

    #[test]
    fn debug_stop_when_no_session() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("debug-stop");
        assert!(editor.status_msg.contains("No active debug session"));
    }

    #[test]
    fn debug_toggle_breakpoint() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("debug-self");
        editor.dispatch_builtin("debug-toggle-breakpoint");
        let state = editor.debug_state.as_ref().unwrap();
        assert_eq!(state.breakpoint_count(), 1);
        assert!(editor.status_msg.contains("Breakpoint set"));

        // Toggle again removes it
        editor.dispatch_builtin("debug-toggle-breakpoint");
        let state = editor.debug_state.as_ref().unwrap();
        assert_eq!(state.breakpoint_count(), 0);
        assert!(editor.status_msg.contains("Breakpoint removed"));
    }

    #[test]
    fn debug_leader_bindings_exist() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        use crate::keymap::{parse_key_seq_spaced, LookupResult};
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC d")),
            LookupResult::Prefix
        );
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC d s")),
            LookupResult::Exact("debug-self")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq_spaced("SPC d b")),
            LookupResult::Exact("debug-toggle-breakpoint")
        );
    }

    // --- Word motions (dispatch integration) ---

    fn editor_with_text(text: &str) -> Editor {
        let mut editor = Editor::new();
        for ch in text.chars() {
            let win = editor.window_mgr.focused_window_mut();
            editor.buffers[0].insert_char(win, ch);
        }
        // Move cursor to start
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.window_mgr.focused_window_mut().cursor_col = 0;
        editor
    }

    #[test]
    fn word_forward_dispatch() {
        let mut editor = editor_with_text("hello world");
        editor.dispatch_builtin("move-word-forward");
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 6);
    }

    #[test]
    fn word_backward_dispatch() {
        let mut editor = editor_with_text("hello world");
        editor.window_mgr.focused_window_mut().cursor_col = 6;
        editor.dispatch_builtin("move-word-backward");
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
    }

    #[test]
    fn word_end_dispatch() {
        let mut editor = editor_with_text("hello world");
        editor.dispatch_builtin("move-word-end");
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 4);
    }

    #[test]
    fn matching_bracket_dispatch() {
        let mut editor = editor_with_text("(hello)");
        editor.dispatch_builtin("move-matching-bracket");
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 6);
    }

    #[test]
    fn find_char_dispatch() {
        let mut editor = editor_with_text("hello world");
        editor.dispatch_char_motion("find-char-forward", 'o');
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 4);
    }

    // --- Yank/Paste ---

    #[test]
    fn yank_line_and_paste_after() {
        let mut editor = editor_with_text("aaa\nbbb\n");
        editor.dispatch_builtin("yank-line");
        assert_eq!(editor.registers.get(&'"'), Some(&"aaa\n".to_string()));
        editor.dispatch_builtin("paste-after");
        assert_eq!(editor.buffers[0].text(), "aaa\naaa\nbbb\n");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
    }

    #[test]
    fn yank_line_and_paste_before() {
        let mut editor = editor_with_text("aaa\nbbb\n");
        editor.window_mgr.focused_window_mut().cursor_row = 1;
        editor.window_mgr.focused_window_mut().cursor_col = 0;
        editor.dispatch_builtin("yank-line");
        assert_eq!(editor.registers.get(&'"'), Some(&"bbb\n".to_string()));
        editor.dispatch_builtin("paste-before");
        assert_eq!(editor.buffers[0].text(), "aaa\nbbb\nbbb\n");
    }

    #[test]
    fn delete_line_copies_to_register_then_paste_restores() {
        let mut editor = editor_with_text("aaa\nbbb\nccc\n");
        editor.window_mgr.focused_window_mut().cursor_row = 1;
        editor.window_mgr.focused_window_mut().cursor_col = 0;
        editor.dispatch_builtin("delete-line");
        assert_eq!(editor.buffers[0].text(), "aaa\nccc\n");
        assert_eq!(editor.registers.get(&'"'), Some(&"bbb\n".to_string()));
        // Paste it back
        editor.window_mgr.focused_window_mut().cursor_row = 0;
        editor.dispatch_builtin("paste-after");
        assert_eq!(editor.buffers[0].text(), "aaa\nbbb\nccc\n");
    }

    #[test]
    fn delete_word_forward() {
        let mut editor = editor_with_text("hello world");
        editor.dispatch_builtin("delete-word-forward");
        assert_eq!(editor.buffers[0].text(), "world");
        assert_eq!(editor.registers.get(&'"'), Some(&"hello ".to_string()));
    }

    #[test]
    fn delete_to_line_end() {
        let mut editor = editor_with_text("hello world");
        editor.window_mgr.focused_window_mut().cursor_col = 5;
        editor.dispatch_builtin("delete-to-line-end");
        assert_eq!(editor.buffers[0].text(), "hello");
        assert_eq!(editor.registers.get(&'"'), Some(&" world".to_string()));
    }

    #[test]
    fn delete_to_line_start() {
        let mut editor = editor_with_text("hello world");
        editor.window_mgr.focused_window_mut().cursor_col = 5;
        editor.dispatch_builtin("delete-to-line-start");
        assert_eq!(editor.buffers[0].text(), " world");
        assert_eq!(editor.registers.get(&'"'), Some(&"hello".to_string()));
    }

    #[test]
    fn yank_word_does_not_modify_buffer() {
        let mut editor = editor_with_text("hello world");
        editor.dispatch_builtin("yank-word-forward");
        assert_eq!(editor.buffers[0].text(), "hello world");
        assert_eq!(editor.registers.get(&'"'), Some(&"hello ".to_string()));
    }

    #[test]
    fn yank_to_line_end() {
        let mut editor = editor_with_text("hello world");
        editor.window_mgr.focused_window_mut().cursor_col = 6;
        editor.dispatch_builtin("yank-to-line-end");
        assert_eq!(editor.registers.get(&'"'), Some(&"world".to_string()));
    }

    #[test]
    fn multiple_yanks_overwrite_register() {
        let mut editor = editor_with_text("aaa\nbbb\n");
        editor.dispatch_builtin("yank-line");
        assert_eq!(editor.registers.get(&'"'), Some(&"aaa\n".to_string()));
        editor.window_mgr.focused_window_mut().cursor_row = 1;
        editor.dispatch_builtin("yank-line");
        assert_eq!(editor.registers.get(&'"'), Some(&"bbb\n".to_string()));
    }

    #[test]
    fn paste_in_empty_buffer() {
        let mut editor = Editor::new();
        editor.registers.insert('"', "hello".to_string());
        editor.dispatch_builtin("paste-after");
        assert_eq!(editor.buffers[0].text(), "hello");
    }

    // --- Buffer management ---

    #[test]
    fn next_buffer_cycles() {
        let mut editor = Editor::new();
        let mut b = Buffer::new();
        b.name = "a".into();
        editor.buffers.push(b);
        let mut b = Buffer::new();
        b.name = "b".into();
        editor.buffers.push(b);
        assert_eq!(editor.buffers.len(), 3);
        editor.window_mgr.focused_window_mut().buffer_idx = 0;
        editor.dispatch_builtin("next-buffer");
        assert_eq!(editor.active_buffer_idx(), 1);
        editor.dispatch_builtin("next-buffer");
        assert_eq!(editor.active_buffer_idx(), 2);
        editor.dispatch_builtin("next-buffer");
        assert_eq!(editor.active_buffer_idx(), 0); // wraps
    }

    #[test]
    fn prev_buffer_cycles() {
        let mut editor = Editor::new();
        let mut b = Buffer::new();
        b.name = "a".into();
        editor.buffers.push(b);
        let mut b = Buffer::new();
        b.name = "b".into();
        editor.buffers.push(b);
        editor.window_mgr.focused_window_mut().buffer_idx = 0;
        editor.dispatch_builtin("prev-buffer");
        assert_eq!(editor.active_buffer_idx(), 2); // wraps backward
    }

    #[test]
    fn next_buffer_single_is_noop() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("next-buffer");
        assert_eq!(editor.active_buffer_idx(), 0);
    }

    #[test]
    fn kill_buffer_single_becomes_scratch() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("kill-buffer");
        assert_eq!(editor.buffers.len(), 1);
        assert_eq!(editor.buffers[0].name, "[scratch]");
    }

    #[test]
    fn kill_buffer_multi_removes_and_fixes_indices() {
        let mut editor = Editor::new();
        // Add a second buffer
        editor.buffers.push(Buffer::new());
        editor.buffers[1].name = "second".to_string();
        editor.buffers.push(Buffer::new());
        editor.buffers[2].name = "third".to_string();
        // Focus on buffer 1
        editor.window_mgr.focused_window_mut().buffer_idx = 1;
        editor.dispatch_builtin("kill-buffer");
        assert_eq!(editor.buffers.len(), 2);
        // Should now be on buffer 0 (saturating_sub(1))
        assert_eq!(editor.active_buffer_idx(), 0);
    }

    #[test]
    fn kill_buffer_modified_refuses() {
        let mut editor = Editor::new();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[0].insert_char(win, 'x');
        editor.dispatch_builtin("kill-buffer");
        assert!(editor.status_msg.contains("unsaved"));
        assert_eq!(editor.buffers.len(), 1);
    }

    #[test]
    fn switch_buffer_shows_list() {
        let mut editor = Editor::new();
        editor.dispatch_builtin("switch-buffer");
        assert!(editor.status_msg.contains("[scratch]"));
    }

    // --- New command registrations ---

    #[test]
    fn new_commands_registered() {
        let editor = Editor::new();
        let new_commands = [
            "move-word-forward",
            "move-word-backward",
            "move-word-end",
            "move-big-word-forward",
            "move-big-word-backward",
            "move-big-word-end",
            "move-matching-bracket",
            "move-paragraph-forward",
            "move-paragraph-backward",
            "find-char-forward-await",
            "find-char-backward-await",
            "till-char-forward-await",
            "till-char-backward-await",
            "delete-word-forward",
            "delete-to-line-end",
            "delete-to-line-start",
            "yank-line",
            "yank-word-forward",
            "yank-to-line-end",
            "yank-to-line-start",
            "paste-after",
            "paste-before",
            "switch-buffer",
        ];
        for cmd in &new_commands {
            assert!(
                editor.commands.contains(cmd),
                "Command '{}' not registered",
                cmd
            );
        }
    }

    // --- New keybindings ---

    #[test]
    fn word_motion_keybindings() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        use crate::keymap::{parse_key_seq, LookupResult};
        assert_eq!(
            normal.lookup(&parse_key_seq("w")),
            LookupResult::Exact("move-word-forward")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("b")),
            LookupResult::Exact("move-word-backward")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("e")),
            LookupResult::Exact("move-word-end")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("W")),
            LookupResult::Exact("move-big-word-forward")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("B")),
            LookupResult::Exact("move-big-word-backward")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("E")),
            LookupResult::Exact("move-big-word-end")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("%")),
            LookupResult::Exact("move-matching-bracket")
        );
    }

    #[test]
    fn yank_paste_keybindings() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        use crate::keymap::{parse_key_seq, LookupResult};
        assert_eq!(normal.lookup(&parse_key_seq("y")), LookupResult::Prefix);
        assert_eq!(
            normal.lookup(&parse_key_seq("yy")),
            LookupResult::Exact("yank-line")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("yw")),
            LookupResult::Exact("yank-word-forward")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("p")),
            LookupResult::Exact("paste-after")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("P")),
            LookupResult::Exact("paste-before")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("dw")),
            LookupResult::Exact("delete-word-forward")
        );
    }

    // --- Search ---

    #[test]
    fn search_forward_finds_match() {
        let mut editor = editor_with_text("hello world hello");
        editor.search_input = "hello".to_string();
        editor.search_state.direction = crate::search::SearchDirection::Forward;
        editor.execute_search();
        // Should jump to second "hello" (first match start > cursor pos 0)
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 12);
        assert!(editor.search_state.highlight_active);
        assert_eq!(editor.search_state.matches.len(), 2);
    }

    #[test]
    fn search_next_advances() {
        let mut editor = editor_with_text("aa bb aa bb aa");
        editor.search_input = "aa".to_string();
        editor.search_state.direction = crate::search::SearchDirection::Forward;
        editor.execute_search();
        let first_col = editor.window_mgr.focused_window().cursor_col;
        editor.dispatch_builtin("search-next");
        let second_col = editor.window_mgr.focused_window().cursor_col;
        assert!(second_col > first_col || second_col == 0); // advanced or wrapped
    }

    #[test]
    fn search_prev_goes_backward() {
        let mut editor = editor_with_text("aa bb aa bb aa");
        editor.search_input = "aa".to_string();
        editor.search_state.direction = crate::search::SearchDirection::Forward;
        editor.execute_search();
        // Now at some match. N goes backward.
        editor.dispatch_builtin("search-prev");
        // Should land on a match before current
        assert!(editor.search_state.highlight_active);
    }

    #[test]
    fn search_wraps_around() {
        let mut editor = editor_with_text("aa bb");
        editor.search_input = "aa".to_string();
        editor.search_state.direction = crate::search::SearchDirection::Forward;
        editor.execute_search();
        // Only one match — n should wrap back to it
        editor.dispatch_builtin("search-next");
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
    }

    #[test]
    fn search_invalid_regex_shows_error() {
        let mut editor = editor_with_text("hello");
        editor.search_input = "[invalid".to_string();
        editor.execute_search();
        assert!(editor.status_msg.contains("Invalid regex"));
        assert!(!editor.search_state.highlight_active);
    }

    #[test]
    fn substitute_single_line() {
        let mut editor = editor_with_text("foo bar foo");
        editor.execute_command("s/foo/baz/");
        assert_eq!(editor.buffers[0].text(), "baz bar foo");
    }

    #[test]
    fn substitute_whole_buffer() {
        let mut editor = editor_with_text("foo bar\nfoo baz\n");
        editor.execute_command("%s/foo/qux/g");
        assert_eq!(editor.buffers[0].text(), "qux bar\nqux baz\n");
    }

    #[test]
    fn substitute_is_undoable() {
        let mut editor = editor_with_text("foo bar");
        let original = editor.buffers[0].text();
        editor.execute_command("s/foo/baz/");
        assert_eq!(editor.buffers[0].text(), "baz bar");
        // Each substitute does delete_range + insert_text_at = 2 undo steps per line
        editor.dispatch_builtin("undo");
        editor.dispatch_builtin("undo");
        assert_eq!(editor.buffers[0].text(), original);
    }

    #[test]
    fn star_searches_word_under_cursor() {
        let mut editor = editor_with_text("hello world hello");
        // Cursor at col 0 = on "hello"
        editor.dispatch_builtin("search-word-under-cursor");
        assert!(editor.search_state.highlight_active);
        assert_eq!(editor.search_state.matches.len(), 2);
        // Should jump to second occurrence
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 12);
    }

    #[test]
    fn search_keybindings() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").unwrap();
        use crate::keymap::LookupResult;
        assert_eq!(
            normal.lookup(&parse_key_seq("/")),
            LookupResult::Exact("search-forward-start")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("?")),
            LookupResult::Exact("search-backward-start")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("n")),
            LookupResult::Exact("search-next")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("N")),
            LookupResult::Exact("search-prev")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("*")),
            LookupResult::Exact("search-word-under-cursor")
        );
    }

    #[test]
    fn search_commands_registered() {
        let editor = Editor::new();
        assert!(editor.commands.contains("search-forward-start"));
        assert!(editor.commands.contains("search-backward-start"));
        assert!(editor.commands.contains("search-next"));
        assert!(editor.commands.contains("search-prev"));
        assert!(editor.commands.contains("search-word-under-cursor"));
        assert!(editor.commands.contains("clear-search-highlight"));
    }

    #[test]
    fn noh_clears_highlights() {
        let mut editor = editor_with_text("hello world hello");
        editor.search_input = "hello".to_string();
        editor.execute_search();
        assert!(editor.search_state.highlight_active);
        editor.execute_command("noh");
        assert!(!editor.search_state.highlight_active);
    }

    // -----------------------------------------------------------------------
    // Visual mode tests
    // -----------------------------------------------------------------------

    #[test]
    fn visual_char_mode_sets_anchor() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 3;
        editor.dispatch_builtin("enter-visual-char");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
        assert_eq!(editor.visual_anchor_row, 0);
        assert_eq!(editor.visual_anchor_col, 3);
    }

    #[test]
    fn visual_line_mode_sets_anchor() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 1;
        editor.dispatch_builtin("enter-visual-line");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
        assert_eq!(editor.visual_anchor_row, 1);
    }

    #[test]
    fn visual_escape_returns_to_normal() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("enter-visual-char");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
        editor.dispatch_builtin("enter-normal-mode");
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn visual_v_toggles_off() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("enter-visual-char");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
        editor.dispatch_builtin("enter-visual-char");
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn visual_big_v_toggles_off() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("enter-visual-line");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
        editor.dispatch_builtin("enter-visual-line");
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn visual_v_switches_from_line() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("enter-visual-line");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
        editor.dispatch_builtin("enter-visual-char");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
    }

    #[test]
    fn visual_big_v_switches_from_char() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("enter-visual-char");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Char));
        editor.dispatch_builtin("enter-visual-line");
        assert_eq!(editor.mode, Mode::Visual(VisualType::Line));
    }

    #[test]
    fn visual_char_range_forward() {
        let mut editor = editor_with_text("hello world");
        editor.dispatch_builtin("enter-visual-char");
        // anchor at 0, cursor moves to col 5
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 5;
        let (start, end) = editor.visual_selection_range();
        assert_eq!(start, 0);
        assert_eq!(end, 6); // includes char at cursor
    }

    #[test]
    fn visual_char_range_backward() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 5;
        editor.dispatch_builtin("enter-visual-char");
        // anchor at col 5, move cursor backward
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 2;
        let (start, end) = editor.visual_selection_range();
        assert_eq!(start, 2);
        assert_eq!(end, 6); // includes char at anchor
    }

    #[test]
    fn visual_line_range_single() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        editor.dispatch_builtin("enter-visual-line");
        let (start, end) = editor.visual_selection_range();
        // Line 0: "line1\n" = chars 0..6
        assert_eq!(start, 0);
        assert_eq!(end, 6);
    }

    #[test]
    fn visual_line_range_multi() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        editor.dispatch_builtin("enter-visual-line");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 2;
        let (start, end) = editor.visual_selection_range();
        // Lines 0-2: all text = "line1\nline2\nline3" = 17 chars
        assert_eq!(start, 0);
        assert_eq!(end, 17);
    }

    #[test]
    fn visual_line_range_backward() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 2;
        editor.dispatch_builtin("enter-visual-line");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 0;
        let (start, end) = editor.visual_selection_range();
        assert_eq!(start, 0);
        assert_eq!(end, 17);
    }

    #[test]
    fn visual_movement_extends_selection() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        editor.dispatch_builtin("enter-visual-char");
        // Move down
        let buf = &editor.buffers[editor.active_buffer_idx()];
        editor.window_mgr.focused_window_mut().move_down(buf);
        let (start, end) = editor.visual_selection_range();
        // Anchor at (0,0), cursor at (1,0) → chars 0..7 (includes char at cursor)
        assert_eq!(start, 0);
        assert!(end > 1); // selection extends past first char
    }

    #[test]
    fn visual_word_motion_extends() {
        let mut editor = editor_with_text("hello world test");
        editor.dispatch_builtin("enter-visual-char");
        let buf = &editor.buffers[editor.active_buffer_idx()];
        editor
            .window_mgr
            .focused_window_mut()
            .move_word_forward(buf);
        let (start, end) = editor.visual_selection_range();
        assert_eq!(start, 0);
        assert!(end >= 6); // at least "hello " selected
    }

    #[test]
    fn visual_delete_charwise() {
        let mut editor = editor_with_text("hello world");
        // Select "llo" (cols 2-4)
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 2;
        editor.dispatch_builtin("enter-visual-char");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 4;
        editor.visual_delete();
        assert_eq!(editor.active_buffer().rope().to_string(), "he world");
        assert_eq!(editor.registers.get(&'"').unwrap(), "llo");
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn visual_delete_linewise() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        editor.dispatch_builtin("enter-visual-line");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_row = 1;
        editor.visual_delete();
        assert_eq!(editor.active_buffer().rope().to_string(), "line3");
        let reg = editor.registers.get(&'"').unwrap();
        assert!(reg.contains("line1"));
        assert!(reg.contains("line2"));
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn visual_yank_charwise() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 0;
        editor.dispatch_builtin("enter-visual-char");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 4;
        editor.visual_yank();
        assert_eq!(editor.registers.get(&'"').unwrap(), "hello");
        // Text unchanged
        assert_eq!(editor.active_buffer().rope().to_string(), "hello world");
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn visual_yank_linewise() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        editor.dispatch_builtin("enter-visual-line");
        editor.visual_yank();
        assert_eq!(editor.registers.get(&'"').unwrap(), "line1\n");
        // Text unchanged
        assert_eq!(
            editor.active_buffer().rope().to_string(),
            "line1\nline2\nline3"
        );
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn visual_change_charwise() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 0;
        editor.dispatch_builtin("enter-visual-char");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 4;
        editor.visual_change();
        assert_eq!(editor.active_buffer().rope().to_string(), " world");
        assert_eq!(editor.mode, Mode::Insert);
    }

    #[test]
    fn visual_delete_cursor_position() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 2;
        editor.dispatch_builtin("enter-visual-char");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 6;
        editor.visual_delete();
        // Cursor should be at start of deleted range (col 2)
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn visual_yank_cursor_position() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 6;
        editor.dispatch_builtin("enter-visual-char");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 2;
        editor.visual_yank();
        // Cursor should move to start of selection
        let win = editor.window_mgr.focused_window();
        assert_eq!(win.cursor_col, 2);
    }

    #[test]
    fn visual_select_entire_buffer() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        // gg (already at top), then V, then G
        editor.dispatch_builtin("enter-visual-line");
        let buf = &editor.buffers[editor.active_buffer_idx()];
        editor
            .window_mgr
            .focused_window_mut()
            .move_to_last_line(buf);
        let (start, end) = editor.visual_selection_range();
        assert_eq!(start, 0);
        assert_eq!(end, 17); // entire buffer
    }

    #[test]
    fn visual_empty_selection_single_char() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("enter-visual-char");
        // Immediately yank (no movement) → should yank char under cursor
        editor.visual_yank();
        assert_eq!(editor.registers.get(&'"').unwrap(), "h");
    }

    #[test]
    fn visual_keymap_has_movements() {
        let editor = Editor::new();
        let visual = editor.keymaps.get("visual").expect("visual keymap exists");
        // Check a few movement keys
        assert_eq!(
            visual.lookup(&parse_key_seq("h")),
            LookupResult::Exact("move-left")
        );
        assert_eq!(
            visual.lookup(&parse_key_seq("j")),
            LookupResult::Exact("move-down")
        );
        assert_eq!(
            visual.lookup(&parse_key_seq("w")),
            LookupResult::Exact("move-word-forward")
        );
        assert_eq!(
            visual.lookup(&parse_key_seq("b")),
            LookupResult::Exact("move-word-backward")
        );
    }

    #[test]
    fn visual_keymap_has_operators() {
        let editor = Editor::new();
        let visual = editor.keymaps.get("visual").expect("visual keymap exists");
        assert_eq!(
            visual.lookup(&parse_key_seq("d")),
            LookupResult::Exact("visual-delete")
        );
        assert_eq!(
            visual.lookup(&parse_key_seq("y")),
            LookupResult::Exact("visual-yank")
        );
        assert_eq!(
            visual.lookup(&parse_key_seq("c")),
            LookupResult::Exact("visual-change")
        );
        assert_eq!(
            visual.lookup(&parse_key_seq("x")),
            LookupResult::Exact("visual-delete")
        );
    }

    #[test]
    fn normal_keymap_has_v_and_big_v() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").expect("normal keymap exists");
        assert_eq!(
            normal.lookup(&parse_key_seq("v")),
            LookupResult::Exact("enter-visual-char")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("V")),
            LookupResult::Exact("enter-visual-line")
        );
    }

    // ===== Change operator tests =====

    #[test]
    fn change_line_clears_and_enters_insert() {
        let mut editor = editor_with_text("hello world\nsecond line");
        editor.dispatch_builtin("change-line");
        // Line content should be cleared
        assert_eq!(editor.active_buffer().line_text(0), "\n");
        // Should be in insert mode
        assert_eq!(editor.mode, Mode::Insert);
        // Cursor should be at col 0
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
    }

    #[test]
    fn change_line_sets_register() {
        let mut editor = editor_with_text("hello world\nsecond line");
        editor.dispatch_builtin("change-line");
        assert_eq!(editor.registers.get(&'"').unwrap(), "hello world");
    }

    #[test]
    fn change_word_forward_deletes_word_enters_insert() {
        let mut editor = editor_with_text("hello world test");
        editor.dispatch_builtin("change-word-forward");
        // "hello " should be deleted, leaving "world test"
        let text = editor.active_buffer().rope().to_string();
        assert!(text.starts_with("world test"));
        assert_eq!(editor.mode, Mode::Insert);
    }

    #[test]
    fn change_to_line_end_deletes_to_eol_enters_insert() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 5; // at the space
        editor.dispatch_builtin("change-to-line-end");
        let text = editor.active_buffer().rope().to_string();
        assert_eq!(text, "hello");
        assert_eq!(editor.mode, Mode::Insert);
    }

    #[test]
    fn change_to_line_start_deletes_to_sol_enters_insert() {
        let mut editor = editor_with_text("hello world");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 5; // at the space
        editor.dispatch_builtin("change-to-line-start");
        let text = editor.active_buffer().rope().to_string();
        assert_eq!(text, " world");
        assert_eq!(editor.mode, Mode::Insert);
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 0);
    }

    // ===== Replace char tests =====

    #[test]
    fn replace_char_replaces_under_cursor() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_char_motion("replace-char", 'X');
        let text = editor.active_buffer().rope().to_string();
        assert_eq!(text, "Xello");
    }

    #[test]
    fn replace_char_does_not_change_mode() {
        let mut editor = editor_with_text("hello");
        assert_eq!(editor.mode, Mode::Normal);
        editor.dispatch_char_motion("replace-char", 'X');
        assert_eq!(editor.mode, Mode::Normal);
    }

    #[test]
    fn replace_char_at_end_of_line() {
        let mut editor = editor_with_text("hello");
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 4; // at 'o'
        editor.dispatch_char_motion("replace-char", 'Z');
        let text = editor.active_buffer().rope().to_string();
        assert_eq!(text, "hellZ");
    }

    // ===== Dot repeat tests =====

    #[test]
    fn dot_repeats_delete_line() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        editor.dispatch_builtin("delete-line");
        assert_eq!(editor.active_buffer().rope().to_string(), "line2\nline3");
        editor.dispatch_builtin("dot-repeat");
        assert_eq!(editor.active_buffer().rope().to_string(), "line3");
    }

    #[test]
    fn dot_repeats_delete_char() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("delete-char-forward");
        assert_eq!(editor.active_buffer().rope().to_string(), "ello");
        editor.dispatch_builtin("dot-repeat");
        assert_eq!(editor.active_buffer().rope().to_string(), "llo");
    }

    #[test]
    fn dot_repeats_replace_char() {
        let mut editor = editor_with_text("abcde");
        editor.dispatch_char_motion("replace-char", 'X');
        assert_eq!(editor.active_buffer().rope().to_string(), "Xbcde");
        // Move right then repeat
        let buf = &editor.buffers[editor.active_buffer_idx()];
        editor.window_mgr.focused_window_mut().move_right(buf);
        editor.dispatch_builtin("dot-repeat");
        assert_eq!(editor.active_buffer().rope().to_string(), "XXcde");
    }

    #[test]
    fn dot_repeats_change_word() {
        let mut editor = editor_with_text("hello world test");
        // Change word forward (deletes "hello ") and enters insert mode
        editor.dispatch_builtin("change-word-forward");
        assert_eq!(editor.mode, Mode::Insert);
        // Simulate typing "XX" in insert mode
        let idx = editor.active_buffer_idx();
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].insert_char(win, 'X');
        let win = editor.window_mgr.focused_window_mut();
        editor.buffers[idx].insert_char(win, 'X');
        // Exit insert mode
        editor.dispatch_builtin("enter-normal-mode");
        assert_eq!(editor.mode, Mode::Normal);
        let text = editor.active_buffer().rope().to_string();
        assert_eq!(text, "XXworld test");

        // Move cursor to 'w' (col 2) for the next word
        let win = editor.window_mgr.focused_window_mut();
        win.cursor_col = 2;
        // Now dot-repeat should change-word "world " and insert "XX"
        editor.dispatch_builtin("dot-repeat");
        let text = editor.active_buffer().rope().to_string();
        assert_eq!(text, "XXXXtest");
    }

    #[test]
    fn dot_repeat_no_previous_does_nothing() {
        let mut editor = editor_with_text("hello");
        // No previous edit recorded
        editor.dispatch_builtin("dot-repeat");
        // Buffer should be unchanged
        assert_eq!(editor.active_buffer().rope().to_string(), "hello");
    }

    // ===== Keybinding tests =====

    #[test]
    fn normal_keymap_has_change_bindings() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").expect("normal keymap exists");
        assert_eq!(
            normal.lookup(&parse_key_seq("cc")),
            LookupResult::Exact("change-line")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("cw")),
            LookupResult::Exact("change-word-forward")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("c$")),
            LookupResult::Exact("change-to-line-end")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("C")),
            LookupResult::Exact("change-to-line-end")
        );
        assert_eq!(
            normal.lookup(&parse_key_seq("c0")),
            LookupResult::Exact("change-to-line-start")
        );
    }

    #[test]
    fn normal_keymap_has_replace_binding() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").expect("normal keymap exists");
        assert_eq!(
            normal.lookup(&parse_key_seq("r")),
            LookupResult::Exact("replace-char-await")
        );
    }

    #[test]
    fn normal_keymap_has_dot_repeat_binding() {
        let editor = Editor::new();
        let normal = editor.keymaps.get("normal").expect("normal keymap exists");
        assert_eq!(
            normal.lookup(&parse_key_seq(".")),
            LookupResult::Exact("dot-repeat")
        );
    }

    #[test]
    fn replace_char_await_sets_pending() {
        let mut editor = editor_with_text("hello");
        editor.dispatch_builtin("replace-char-await");
        assert_eq!(
            editor.pending_char_command,
            Some("replace-char".to_string())
        );
    }

    // ===== Count prefix tests (Phase 3e M4) =====

    #[test]
    fn count_prefix_default_none() {
        let editor = Editor::new();
        assert_eq!(editor.count_prefix, None);
    }

    #[test]
    fn take_count_default_is_1() {
        let mut editor = Editor::new();
        assert_eq!(editor.take_count(), 1);
    }

    #[test]
    fn take_count_returns_and_clears() {
        let mut editor = Editor::new();
        editor.count_prefix = Some(5);
        assert_eq!(editor.take_count(), 5);
        assert_eq!(editor.count_prefix, None);
    }

    #[test]
    fn move_down_with_count() {
        let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5\n");
        editor.count_prefix = Some(3);
        editor.dispatch_builtin("move-down");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 3);
    }

    #[test]
    fn move_up_with_count_clamps() {
        let mut editor = editor_with_text("line1\nline2\nline3\n");
        editor.window_mgr.focused_window_mut().cursor_row = 2;
        editor.count_prefix = Some(10); // more than available
        editor.dispatch_builtin("move-up");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 0);
    }

    #[test]
    fn move_right_with_count() {
        let mut editor = editor_with_text("hello world");
        editor.count_prefix = Some(5);
        editor.dispatch_builtin("move-right");
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 5);
    }

    #[test]
    fn move_left_with_count() {
        let mut editor = editor_with_text("hello world");
        editor.window_mgr.focused_window_mut().cursor_col = 8;
        editor.count_prefix = Some(3);
        editor.dispatch_builtin("move-left");
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 5);
    }

    #[test]
    fn delete_char_with_count() {
        let mut editor = editor_with_text("hello world");
        editor.count_prefix = Some(3);
        editor.dispatch_builtin("delete-char-forward");
        assert_eq!(editor.active_buffer().rope().to_string(), "lo world");
    }

    #[test]
    fn delete_line_with_count() {
        let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
        editor.count_prefix = Some(2);
        editor.dispatch_builtin("delete-line");
        assert_eq!(editor.active_buffer().rope().to_string(), "line3\nline4\n");
        // Register should contain both deleted lines
        let reg = editor.registers.get(&'"').unwrap();
        assert!(reg.contains("line1"));
        assert!(reg.contains("line2"));
    }

    #[test]
    fn g_without_count_goes_to_last() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        // No count prefix set
        editor.dispatch_builtin("move-to-last-line");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
    }

    #[test]
    fn g_with_count_goes_to_line() {
        let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5");
        editor.count_prefix = Some(3); // 3G = go to line 3 (1-indexed = row 2)
        editor.dispatch_builtin("move-to-last-line");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 2);
    }

    #[test]
    fn g_with_count_clamps() {
        let mut editor = editor_with_text("line1\nline2\nline3");
        editor.count_prefix = Some(100); // beyond buffer
        editor.dispatch_builtin("move-to-last-line");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 2); // last line
    }

    #[test]
    fn gg_with_count() {
        let mut editor = editor_with_text("line1\nline2\nline3\nline4\nline5");
        editor.window_mgr.focused_window_mut().cursor_row = 4;
        editor.count_prefix = Some(2); // 2gg = go to line 2 (1-indexed = row 1)
        editor.dispatch_builtin("move-to-first-line");
        assert_eq!(editor.window_mgr.focused_window().cursor_row, 1);
    }

    #[test]
    fn word_motion_with_count() {
        let mut editor = editor_with_text("one two three four five");
        editor.count_prefix = Some(3);
        editor.dispatch_builtin("move-word-forward");
        // Should skip past "one ", "two ", "three " → at "four"
        assert_eq!(editor.window_mgr.focused_window().cursor_col, 14);
    }

    #[test]
    fn count_consumed_after_dispatch() {
        let mut editor = editor_with_text("line1\nline2\nline3\n");
        editor.count_prefix = Some(2);
        editor.dispatch_builtin("move-down");
        assert_eq!(editor.count_prefix, None);
    }

    #[test]
    fn yank_line_with_count() {
        let mut editor = editor_with_text("line1\nline2\nline3\nline4\n");
        editor.count_prefix = Some(2);
        editor.dispatch_builtin("yank-line");
        let reg = editor.registers.get(&'"').unwrap();
        assert_eq!(reg, "line1\nline2\n");
        // Buffer unchanged
        assert_eq!(
            editor.active_buffer().rope().to_string(),
            "line1\nline2\nline3\nline4\n"
        );
    }

    #[test]
    fn paste_after_with_count() {
        let mut editor = editor_with_text("hello");
        editor.registers.insert('"', "x".to_string());
        editor.count_prefix = Some(3);
        editor.dispatch_builtin("paste-after");
        // "x" pasted 3 times after cursor
        assert_eq!(editor.active_buffer().rope().to_string(), "hxxxello");
    }

    #[test]
    fn scroll_half_down_with_count() {
        let mut editor =
            editor_with_text(&(0..50).map(|i| format!("line{}\n", i)).collect::<String>());
        editor.viewport_height = 20;
        editor.count_prefix = Some(2);
        editor.dispatch_builtin("scroll-half-down");
        // Should scroll down twice (half page = 10, so 20 lines)
        assert!(editor.window_mgr.focused_window().cursor_row >= 20);
    }

    #[test]
    fn search_next_with_count() {
        let mut editor = editor_with_text("aa bb aa bb aa bb aa");
        editor.search_input = "aa".to_string();
        editor.search_state.direction = crate::search::SearchDirection::Forward;
        editor.execute_search();
        let first_pos = editor.window_mgr.focused_window().cursor_col;
        // Search next with count 2 (skip one match)
        editor.count_prefix = Some(2);
        editor.dispatch_builtin("search-next");
        let final_pos = editor.window_mgr.focused_window().cursor_col;
        // Should have advanced past two matches
        assert!(final_pos != first_pos);
    }

    #[test]
    fn delete_word_forward_with_count() {
        let mut editor = editor_with_text("one two three four");
        editor.count_prefix = Some(2);
        editor.dispatch_builtin("delete-word-forward");
        assert_eq!(editor.active_buffer().rope().to_string(), "three four");
    }

    #[test]
    fn paragraph_motion_with_count() {
        let mut editor = editor_with_text("a\n\nb\n\nc\n\nd");
        editor.count_prefix = Some(2);
        editor.dispatch_builtin("move-paragraph-forward");
        // Two paragraph motions from line 0: first lands on blank line 1,
        // second lands on blank line 3.
        let row = editor.window_mgr.focused_window().cursor_row;
        assert_eq!(row, 3);
    }
}
