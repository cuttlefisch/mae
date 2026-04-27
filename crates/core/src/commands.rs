use std::collections::HashMap;

/// Metadata for an editor command.
///
/// Commands are the shared API between human keybindings, Scheme extensions,
/// and the AI agent. Every command registered here is automatically
/// discoverable by all three.
///
/// Emacs lesson: don't hardcode 26 interactive codes in C (callint.c).
/// Use a registry so packages can define custom commands.
#[derive(Clone, Debug)]
pub struct Command {
    pub name: String,
    pub doc: String,
    pub source: CommandSource,
}

/// Where a command's implementation lives.
#[derive(Clone, Debug, PartialEq)]
pub enum CommandSource {
    /// Implemented in Rust — the binary dispatches these directly.
    Builtin,
    /// Implemented in Scheme — the runtime calls the named Scheme function.
    Scheme(String),
}

/// Registry of all known commands.
///
/// This is metadata only — it tracks what commands exist, their docs, and
/// whether they're built-in or Scheme-defined. Actual dispatch happens in
/// the binary, which has access to both the Editor and SchemeRuntime.
pub struct CommandRegistry {
    commands: HashMap<String, Command>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        CommandRegistry {
            commands: HashMap::new(),
        }
    }

    /// Register a built-in (Rust) command.
    pub fn register_builtin(&mut self, name: impl Into<String>, doc: impl Into<String>) {
        let name = name.into();
        self.commands.insert(
            name.clone(),
            Command {
                name,
                doc: doc.into(),
                source: CommandSource::Builtin,
            },
        );
    }

    /// Register a Scheme-defined command.
    pub fn register_scheme(
        &mut self,
        name: impl Into<String>,
        doc: impl Into<String>,
        scheme_fn: impl Into<String>,
    ) {
        let name = name.into();
        self.commands.insert(
            name.clone(),
            Command {
                name,
                doc: doc.into(),
                source: CommandSource::Scheme(scheme_fn.into()),
            },
        );
    }

    /// Look up a command by name.
    pub fn get(&self, name: &str) -> Option<&Command> {
        self.commands.get(name)
    }

    /// Check if a command exists.
    pub fn contains(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    /// List all command names (sorted, for display/completion).
    pub fn list_names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.commands.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Return all commands (sorted by name), for tool schema generation.
    pub fn list_commands(&self) -> Vec<&Command> {
        let mut cmds: Vec<&Command> = self.commands.values().collect();
        cmds.sort_by_key(|c| &c.name);
        cmds
    }

    /// Number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Create a registry pre-populated with all built-in commands.
    pub fn with_builtins() -> Self {
        let mut reg = Self::new();

        // Movement
        reg.register_builtin("move-up", "Move cursor up one line");
        reg.register_builtin("move-down", "Move cursor down one line");
        reg.register_builtin("move-left", "Move cursor left one character");
        reg.register_builtin("move-right", "Move cursor right one character");
        reg.register_builtin("move-to-line-start", "Move cursor to start of line");
        reg.register_builtin("move-to-line-end", "Move cursor to end of line");
        reg.register_builtin(
            "move-display-down",
            "Move cursor down one display line (gj)",
        );
        reg.register_builtin("move-display-up", "Move cursor up one display line (gk)");
        reg.register_builtin(
            "move-display-line-start",
            "Move to start of display line (g0)",
        );
        reg.register_builtin("move-display-line-end", "Move to end of display line (g$)");
        reg.register_builtin("move-to-first-line", "Move cursor to first line");
        reg.register_builtin("move-to-last-line", "Move cursor to last line");
        reg.register_builtin("move-word-forward", "Move to start of next word (w)");
        reg.register_builtin("move-word-backward", "Move to start of previous word (b)");
        reg.register_builtin("move-word-end", "Move to end of word (e)");
        reg.register_builtin("move-big-word-forward", "Move to start of next WORD (W)");
        reg.register_builtin(
            "move-big-word-backward",
            "Move to start of previous WORD (B)",
        );
        reg.register_builtin("move-big-word-end", "Move to end of WORD (E)");
        reg.register_builtin(
            "move-word-end-backward",
            "Move to end of previous word (ge)",
        );
        reg.register_builtin(
            "move-big-word-end-backward",
            "Move to end of previous WORD (gE)",
        );
        reg.register_builtin(
            "move-to-first-non-blank",
            "Move to first non-blank char of line (^)",
        );
        reg.register_builtin(
            "move-line-next-non-blank",
            "Move to first non-blank of next line (+)",
        );
        reg.register_builtin(
            "move-line-prev-non-blank",
            "Move to first non-blank of previous line (-)",
        );
        reg.register_builtin("move-matching-bracket", "Jump to matching bracket (%)");
        reg.register_builtin("move-paragraph-forward", "Move to next paragraph (})");
        reg.register_builtin("move-paragraph-backward", "Move to previous paragraph ({)");
        reg.register_builtin("find-char-forward-await", "Find char forward on line (f)");
        reg.register_builtin("find-char-backward-await", "Find char backward on line (F)");
        reg.register_builtin("till-char-forward-await", "Till char forward on line (t)");
        reg.register_builtin("till-char-backward-await", "Till char backward on line (T)");

        // Scroll commands
        reg.register_builtin("scroll-half-up", "Scroll half page up (C-u)");
        reg.register_builtin("scroll-half-down", "Scroll half page down (C-d)");
        reg.register_builtin("scroll-page-up", "Scroll full page up (C-b)");
        reg.register_builtin("scroll-page-down", "Scroll full page down (C-f)");
        reg.register_builtin("scroll-center", "Center cursor line on screen (zz)");
        reg.register_builtin("scroll-top", "Scroll cursor line to top (zt)");
        reg.register_builtin("scroll-bottom", "Scroll cursor line to bottom (zb)");
        reg.register_builtin("scroll-down-line", "Scroll one line down (C-e)");
        reg.register_builtin("scroll-up-line", "Scroll one line up (C-y)");

        // Screen-relative cursor
        reg.register_builtin("move-screen-top", "Move cursor to top visible line (H)");
        reg.register_builtin(
            "move-screen-middle",
            "Move cursor to middle visible line (M)",
        );
        reg.register_builtin(
            "move-screen-bottom",
            "Move cursor to bottom visible line (L)",
        );

        // Editing
        reg.register_builtin("delete-char-forward", "Delete character under cursor");
        reg.register_builtin("delete-char-backward", "Delete character before cursor");
        reg.register_builtin("delete-line", "Delete current line");
        reg.register_builtin("delete-word-forward", "Delete to next word start (dw)");
        reg.register_builtin("delete-to-line-end", "Delete to end of line (d$)");
        reg.register_builtin("delete-to-line-start", "Delete to start of line (d0)");
        reg.register_builtin(
            "open-line-below",
            "Open new line below and enter insert mode",
        );
        reg.register_builtin(
            "open-line-above",
            "Open new line above and enter insert mode",
        );
        // Change operators
        reg.register_builtin("change-line", "Change entire line (cc)");
        reg.register_builtin("change-word-forward", "Change to next word (cw)");
        reg.register_builtin("change-to-line-end", "Change to end of line (C/c$)");
        reg.register_builtin("change-to-line-start", "Change to start of line (c0)");
        // Replace
        reg.register_builtin("replace-char-await", "Replace char under cursor (r)");
        // Substitute
        reg.register_builtin("substitute-char", "Substitute char under cursor (s)");
        reg.register_builtin("substitute-line", "Substitute entire line (S)");
        // Re-enter insert at last position
        reg.register_builtin("reinsert-at-last-position", "Insert at last edit position");
        // Jump list (Practical Vim ch. 9)
        reg.register_builtin(
            "jump-backward",
            "Navigate backward through the jump list (Ctrl-o)",
        );
        reg.register_builtin(
            "jump-forward",
            "Navigate forward through the jump list (Ctrl-i)",
        );
        // Change list (Practical Vim ch. 9)
        reg.register_builtin(
            "change-backward",
            "Navigate backward through the change list (g;)",
        );
        reg.register_builtin(
            "change-forward",
            "Navigate forward through the change list (g,)",
        );
        reg.register_builtin("show-changes-buffer", "Show change list");
        reg.register_builtin(
            "goto-file-under-cursor",
            "Open the filename under the cursor (gf)",
        );
        // Marks
        reg.register_builtin("set-mark-await", "Set mark at next-typed letter (m)");
        reg.register_builtin("jump-mark-await", "Jump to mark at next-typed letter (')");
        // Macros
        reg.register_builtin(
            "start-recording-await",
            "Start recording macro to next-typed register (q)",
        );
        reg.register_builtin(
            "replay-macro-await",
            "Replay macro from next-typed register (@)",
        );
        reg.register_builtin("replay-last-macro", "Replay the last-used macro (@@)");
        // Join, indent, dedent
        reg.register_builtin("join-lines", "Join current line with next line (J)");
        reg.register_builtin("indent-line", "Indent current line by 4 spaces (>>)");
        reg.register_builtin("dedent-line", "Dedent current line by up to 4 spaces (<<)");
        // Case change
        reg.register_builtin("toggle-case", "Toggle case of char under cursor (~)");
        reg.register_builtin("uppercase-line", "Uppercase current line (gUU)");
        reg.register_builtin("lowercase-line", "Lowercase current line (guu)");
        // Alternate file
        reg.register_builtin(
            "alternate-file",
            "Switch to alternate (previous) buffer (C-^)",
        );
        // Shell escape
        reg.register_builtin("shell-command", "Run a shell command (:!cmd)");
        // Dot repeat
        reg.register_builtin("dot-repeat", "Repeat last edit (.)");
        // Yank/Paste
        reg.register_builtin("yank-line", "Yank current line (yy)");
        reg.register_builtin("yank-word-forward", "Yank to next word start (yw)");
        reg.register_builtin("yank-to-line-end", "Yank to end of line (y$)");
        reg.register_builtin("yank-to-line-start", "Yank to start of line (y0)");
        reg.register_builtin("paste-after", "Paste after cursor (p)");
        reg.register_builtin("paste-before", "Paste before cursor (P)");

        // Operator-pending mode
        reg.register_builtin(
            "operator-delete",
            "Enter operator-pending mode for delete (d + motion)",
        );
        reg.register_builtin(
            "operator-change",
            "Enter operator-pending mode for change (c + motion)",
        );
        reg.register_builtin(
            "operator-yank",
            "Enter operator-pending mode for yank (y + motion)",
        );
        reg.register_builtin(
            "operator-surround",
            "Surround motion range with delimiter (ys + motion + char)",
        );

        // Undo/Redo
        reg.register_builtin("undo", "Undo last edit");
        reg.register_builtin("redo", "Redo last undone edit");

        // Mode changes
        reg.register_builtin("enter-insert-mode", "Enter insert mode");
        reg.register_builtin("enter-insert-mode-after", "Enter insert mode after cursor");
        reg.register_builtin("enter-insert-mode-eol", "Enter insert mode at end of line");
        reg.register_builtin("enter-normal-mode", "Return to normal mode");
        reg.register_builtin("enter-command-mode", "Enter command-line mode");

        // File operations
        reg.register_builtin("save", "Save current buffer");
        reg.register_builtin("quit", "Quit editor");
        reg.register_builtin("force-quit", "Quit without saving");
        reg.register_builtin("save-and-quit", "Save and quit");

        // Window management
        reg.register_builtin("split-vertical", "Split window vertically (left/right)");
        reg.register_builtin("split-horizontal", "Split window horizontally (top/bottom)");
        reg.register_builtin("close-window", "Close current window");
        reg.register_builtin("focus-left", "Focus window to the left");
        reg.register_builtin("focus-right", "Focus window to the right");
        reg.register_builtin("focus-up", "Focus window above");
        reg.register_builtin("focus-down", "Focus window below");

        // Diagnostics
        reg.register_builtin("view-messages", "Show *Messages* log buffer");

        // Dashboard / scratch
        reg.register_builtin("dashboard", "Show the startup dashboard");
        reg.register_builtin("toggle-scratch-buffer", "Toggle the scratch buffer (SPC x)");

        // Leader key placeholder commands
        reg.register_builtin("command-palette", "Search and run any command");
        reg.register_builtin("kill-buffer", "Close current buffer");
        reg.register_builtin("next-buffer", "Cycle to next buffer");
        reg.register_builtin("prev-buffer", "Cycle to previous buffer");
        reg.register_builtin("find-file", "Open a file");
        reg.register_builtin("file-browser", "Open directory browser");
        reg.register_builtin("recent-files", "Open recent file");
        reg.register_builtin("switch-buffer", "Switch to another buffer");
        reg.register_builtin("new-buffer", "Create a new empty scratch buffer");
        reg.register_builtin("force-kill-buffer", "Close current buffer without saving");
        reg.register_builtin("ai-prompt", "Open AI conversation and prompt");
        reg.register_builtin("ai-cancel", "Cancel current AI operation");
        reg.register_builtin("ai-accept", "Accept proposed AI changes");
        reg.register_builtin("ai-reject", "Reject proposed AI changes");
        reg.register_builtin(
            "ai-set-mode",
            "Switch the AI operating mode (standard, plan, auto-accept)",
        );
        reg.register_builtin(
            "ai-set-profile",
            "Switch the active AI prompt profile (pair-programmer, explorer, planner, reviewer)",
        );
        reg.register_builtin("describe-key", "Show what a key does");
        reg.register_builtin("describe-command", "Show command documentation");
        reg.register_builtin("show-registers", "Show named registers");
        reg.register_builtin("prompt-register", "Select a register");

        // Surrounds (vim-surround ports)
        reg.register_builtin(
            "delete-surround-await",
            "Delete surrounding delimiter (ds<char>)",
        );
        reg.register_builtin(
            "change-surround-await",
            "Change surrounding delimiter (cs<from><to>)",
        );
        reg.register_builtin(
            "surround-line-await",
            "Surround current line with char (yss<char>)",
        );
        reg.register_builtin(
            "surround-visual-await",
            "Surround visual selection with char (S<char>)",
        );
        reg.register_builtin("set-theme", "Set editor color theme");
        reg.register_builtin("cycle-theme", "Cycle to next color theme");
        reg.register_builtin("set-splash-art", "Choose splash screen art style");

        // Scheme REPL (lisp machine primitives)
        reg.register_builtin("eval-line", "Evaluate current line as Scheme (SPC e l)");
        reg.register_builtin(
            "eval-region",
            "Evaluate visual selection as Scheme (SPC e r)",
        );
        reg.register_builtin("eval-buffer", "Evaluate entire buffer as Scheme (SPC e b)");
        reg.register_builtin("open-scheme-repl", "Open *Scheme* REPL buffer (SPC e o)");

        // Repeat find (;/,)
        reg.register_builtin("repeat-find", "Repeat last f/F/t/T in same direction (;)");
        reg.register_builtin(
            "repeat-find-reverse",
            "Repeat last f/F/t/T in opposite direction (,)",
        );

        // Visual mode
        reg.register_builtin("enter-visual-char", "Enter charwise visual mode (v)");
        reg.register_builtin("enter-visual-line", "Enter linewise visual mode (V)");
        reg.register_builtin("visual-delete", "Delete visual selection (d)");
        reg.register_builtin("visual-yank", "Yank visual selection (y)");
        reg.register_builtin("visual-change", "Change visual selection (c)");
        reg.register_builtin("visual-indent", "Indent visual selection by 4 spaces (>)");
        reg.register_builtin(
            "visual-dedent",
            "Dedent visual selection by up to 4 spaces (<)",
        );
        reg.register_builtin("visual-join", "Join lines in visual selection (J)");
        reg.register_builtin(
            "visual-paste",
            "Replace visual selection with register contents (p/P)",
        );
        reg.register_builtin(
            "visual-swap-ends",
            "Swap cursor and anchor in visual mode (o)",
        );
        reg.register_builtin("visual-uppercase", "Uppercase visual selection (U)");
        reg.register_builtin("visual-lowercase", "Lowercase visual selection (u)");
        reg.register_builtin("reselect-visual", "Reselect last visual selection (gv)");

        // Search
        reg.register_builtin("search-forward-start", "Search forward (/)");
        reg.register_builtin("search-backward-start", "Search backward (?)");
        reg.register_builtin("search-next", "Jump to next match (n)");
        reg.register_builtin("search-prev", "Jump to previous match (N)");
        reg.register_builtin("search-word-under-cursor", "Search word under cursor (*)");
        reg.register_builtin(
            "search-word-under-cursor-backward",
            "Search word under cursor backward (#)",
        );
        reg.register_builtin("clear-search-highlight", "Clear search highlights (:noh)");
        reg.register_builtin(
            "visual-select-next-match",
            "Visually select next search match (gn)",
        );
        reg.register_builtin(
            "visual-select-prev-match",
            "Visually select previous search match (gN)",
        );
        reg.register_builtin("delete-next-match", "Delete next search match (dgn)");
        reg.register_builtin("delete-prev-match", "Delete previous search match (dgN)");
        reg.register_builtin("change-next-match", "Change next search match (cgn)");
        reg.register_builtin("change-prev-match", "Change previous search match (cgN)");
        reg.register_builtin("yank-next-match", "Yank next search match (ygn)");
        reg.register_builtin("yank-prev-match", "Yank previous search match (ygN)");

        // Text objects
        reg.register_builtin(
            "delete-inner-object",
            "Delete inner text object (di + char)",
        );
        reg.register_builtin(
            "delete-around-object",
            "Delete around text object (da + char)",
        );
        reg.register_builtin(
            "change-inner-object",
            "Change inner text object (ci + char)",
        );
        reg.register_builtin(
            "change-around-object",
            "Change around text object (ca + char)",
        );
        reg.register_builtin("yank-inner-object", "Yank inner text object (yi + char)");
        reg.register_builtin("yank-around-object", "Yank around text object (ya + char)");
        reg.register_builtin(
            "visual-inner-object",
            "Select inner text object in visual mode (i + char)",
        );
        reg.register_builtin(
            "visual-around-object",
            "Select around text object in visual mode (a + char)",
        );

        // Debugging
        reg.register_builtin("debug-self", "Open self-debug view (Rust + Scheme state)");
        reg.register_builtin("debug-start", "Start DAP debug session");
        reg.register_builtin("debug-stop", "Stop current debug session");
        reg.register_builtin("debug-continue", "Continue execution");
        reg.register_builtin("debug-step-over", "Step over (next line)");
        reg.register_builtin("debug-step-into", "Step into function");
        reg.register_builtin("debug-step-out", "Step out of function");
        reg.register_builtin(
            "debug-toggle-breakpoint",
            "Toggle breakpoint on current line",
        );
        reg.register_builtin("debug-inspect", "Inspect variable or evaluate expression");
        reg.register_builtin(
            "debug-panel",
            "Toggle debug panel showing threads, stack, and variables (SPC d p)",
        );
        reg.register_builtin(
            "debug-attach",
            "Attach debugger to a running process (:debug-attach <adapter> <pid>)",
        );
        reg.register_builtin(
            "debug-eval",
            "Evaluate expression in debug context (:debug-eval <expression>)",
        );

        // LSP (Phase 4a)
        reg.register_builtin(
            "lsp-goto-definition",
            "Jump to definition of symbol under cursor (gd)",
        );
        reg.register_builtin(
            "lsp-find-references",
            "Find references to symbol under cursor (gr)",
        );
        reg.register_builtin(
            "lsp-hover",
            "Show hover information for symbol under cursor (K)",
        );
        reg.register_builtin(
            "lsp-next-diagnostic",
            "Jump to next diagnostic in buffer (]d)",
        );
        reg.register_builtin(
            "lsp-prev-diagnostic",
            "Jump to previous diagnostic in buffer ([d)",
        );
        reg.register_builtin(
            "lsp-show-diagnostics",
            "Show all diagnostics in a list buffer",
        );
        reg.register_builtin(
            "lsp-complete",
            "Trigger LSP completion at cursor (insert mode)",
        );
        reg.register_builtin(
            "lsp-accept-completion",
            "Accept the selected completion item (Tab)",
        );
        reg.register_builtin("lsp-dismiss-completion", "Dismiss completion popup");
        reg.register_builtin("lsp-complete-next", "Select next completion item (Ctrl-n)");
        reg.register_builtin(
            "lsp-complete-prev",
            "Select previous completion item (Ctrl-p)",
        );

        // LSP code actions (Phase 4a stubs)
        reg.register_builtin("lsp-code-action", "Run LSP code action at cursor (SPC c a)");
        reg.register_builtin("lsp-rename", "Rename symbol under cursor via LSP (SPC c R)");
        reg.register_builtin("lsp-format", "Format buffer via LSP (SPC c f)");

        // Tree-sitter structural editing (Phase 4b M3)
        reg.register_builtin(
            "syntax-select-node",
            "Select the tree-sitter node at the cursor (SPC s s)",
        );
        reg.register_builtin(
            "syntax-expand-selection",
            "Expand Visual selection to the parent syntax node (SPC s e)",
        );
        reg.register_builtin(
            "syntax-contract-selection",
            "Contract Visual selection to the previous syntax node (SPC s c)",
        );

        // Project commands (SPC p)
        reg.register_builtin("project-find-file", "Find file in project (SPC p f)");
        reg.register_builtin("project-search", "Search in project (SPC p s)");
        reg.register_builtin("project-browse", "Browse project directory (SPC p d)");
        reg.register_builtin("project-recent-files", "Recent files in project (SPC p r)");
        reg.register_builtin("project-switch", "Switch to a recent project (SPC p p)");

        // Search aliases
        reg.register_builtin("search-buffer", "Search in current buffer (SPC s s)");

        // File operations (expanded)
        reg.register_builtin(
            "yank-file-path",
            "Copy buffer file path to clipboard (SPC f y)",
        );
        reg.register_builtin("rename-file", "Rename current file (SPC f R)");
        reg.register_builtin("save-as", "Save buffer to new path (SPC f S)");
        reg.register_builtin("edit-config", "Open init.scm config for editing (SPC f c)");
        reg.register_builtin("edit-settings", "Open config.toml settings (SPC f C)");
        reg.register_builtin(
            "setup-wizard",
            "Show how to re-run the first-run setup wizard",
        );
        reg.register_builtin("toggle-fps", "Toggle FPS overlay in status bar (SPC t F)");
        reg.register_builtin(
            "debug-mode",
            "Toggle debug mode: RSS/CPU/frame time in status bar (SPC t D)",
        );
        reg.register_builtin("reload-config", "Reload config.toml and init.scm");
        reg.register_builtin(
            "describe-option",
            "Show documentation for an editor option (SPC h o)",
        );
        reg.register_builtin(
            "set-save",
            "Set an option and persist to config.toml (:set-save <key> [value])",
        );

        // Buffer operations (expanded)
        reg.register_builtin(
            "kill-other-buffers",
            "Close all buffers except current (SPC b o)",
        );
        reg.register_builtin("save-all-buffers", "Save all modified buffers (SPC b S)");
        reg.register_builtin("revert-buffer", "Reload buffer from disk (SPC b r)");

        // Toggle commands
        reg.register_builtin(
            "toggle-line-numbers",
            "Toggle line number display (SPC t l)",
        );
        reg.register_builtin(
            "toggle-relative-line-numbers",
            "Toggle relative line numbers (SPC t r)",
        );
        reg.register_builtin("toggle-word-wrap", "Toggle word wrap (SPC t w)");

        // Git commands (shell-out stubs)
        reg.register_builtin("git-status", "Show git status in scratch buffer (SPC g s)");
        reg.register_builtin("git-blame", "Show git blame for current file (SPC g b)");
        reg.register_builtin("git-diff", "Show git diff in scratch buffer (SPC g d)");
        reg.register_builtin("git-log", "Show git log in scratch buffer (SPC g l)");

        // Notes/KB commands
        reg.register_builtin("kb-find", "Search KB nodes (SPC n f)");

        // Help / KB navigation
        reg.register_builtin("help", "Open the *Help* buffer at the knowledge-base index");
        reg.register_builtin(
            "help-follow-link",
            "Follow the focused link in the *Help* buffer",
        );
        reg.register_builtin("help-back", "Navigate back in help history (C-o)");
        reg.register_builtin("help-forward", "Navigate forward in help history (C-i)");
        reg.register_builtin(
            "help-next-link",
            "Focus the next link in the current help page",
        );
        reg.register_builtin(
            "help-prev-link",
            "Focus the previous link in the current help page",
        );
        reg.register_builtin("help-close", "Close help buffer");
        reg.register_builtin("help-search", "Search help topics");
        reg.register_builtin("help-reopen", "Reopen the last-closed help buffer");

        // Shell / terminal emulator
        reg.register_builtin("terminal", "Open a terminal emulator buffer (:terminal)");
        reg.register_builtin(
            "terminal-reset",
            "Reset/clear the current terminal emulator",
        );
        reg.register_builtin(
            "terminal-close",
            "Close the current terminal and its shell process",
        );
        reg.register_builtin(
            "shell-normal-mode",
            "Exit ShellInsert mode and return to Normal mode",
        );
        reg.register_builtin(
            "send-to-shell",
            "Send current line to a terminal buffer (SPC e s)",
        );
        reg.register_builtin(
            "send-region-to-shell",
            "Send visual selection to a terminal buffer (SPC e S)",
        );

        // Shell scrollback navigation
        reg.register_builtin("shell-scroll-page-up", "Scroll shell terminal up one page");
        reg.register_builtin(
            "shell-scroll-page-down",
            "Scroll shell terminal down one page",
        );
        reg.register_builtin(
            "shell-scroll-to-bottom",
            "Scroll shell terminal to latest output",
        );

        // Ex-command parity: commands that were only inline in execute_command()
        // are now registered so the AI can invoke them via command_* tools.
        reg.register_builtin(
            "nohlsearch",
            "Clear search highlights (alias for clear-search-highlight)",
        );
        reg.register_builtin(
            "kb-save",
            "Save knowledge base to SQLite file (:kb-save <path>)",
        );
        reg.register_builtin(
            "kb-load",
            "Load knowledge base from SQLite file (:kb-load <path>)",
        );
        reg.register_builtin(
            "kb-ingest",
            "Ingest org files from directory into knowledge base (:kb-ingest <dir>)",
        );
        reg.register_builtin(
            "ai-save",
            "Save AI conversation to JSON file (:ai-save <path>)",
        );
        reg.register_builtin(
            "ai-load",
            "Load AI conversation from JSON file (:ai-load <path>)",
        );

        // Agent bootstrap
        reg.register_builtin(
            "agent-list",
            "List all AI agents MAE can bootstrap for MCP tool discovery",
        );
        reg.register_builtin(
            "agent-setup",
            "Bootstrap an AI agent: write .mcp.json and approval settings (:agent-setup <name>)",
        );

        // Self-test
        reg.register_builtin(
            "self-test",
            "Run AI-driven self-test to validate editor tools and integrations (:self-test [categories])",
        );

        // Font zoom (GUI)
        reg.register_builtin("increase-font-size", "Increase GUI font size by 1pt");
        reg.register_builtin("decrease-font-size", "Decrease GUI font size by 1pt");
        reg.register_builtin(
            "reset-font-size",
            "Reset GUI font size to configured default",
        );
        reg.register_builtin("debug-path", "Show current PATH environment variable");

        // AI agent launcher
        reg.register_builtin("open-ai-agent", "Open AI agent in a shell terminal");

        // Tutorial
        reg.register_builtin("tutor", "Open interactive MAE tutorial");

        // Session persistence
        reg.register_builtin(
            "session-save",
            "Save current session (open buffers + cursors) to .mae/session.json",
        );
        reg.register_builtin("session-load", "Restore session from .mae/session.json");

        // Project management
        reg.register_builtin("add-project", "Add a project directory and switch to it");
        reg.register_builtin("remove-project", "Remove a project from the recent list");

        // Event recording
        reg.register_builtin("record-start", "Start event recording for debugging");
        reg.register_builtin("record-stop", "Stop event recording");
        reg.register_builtin(
            "record-save",
            "Save recorded events to JSON file (:record-save <path>)",
        );

        reg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_registry_is_empty() {
        let reg = CommandRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn register_and_get_builtin() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("move-down", "Move cursor down");
        let cmd = reg.get("move-down").unwrap();
        assert_eq!(cmd.name, "move-down");
        assert_eq!(cmd.doc, "Move cursor down");
        assert_eq!(cmd.source, CommandSource::Builtin);
    }

    #[test]
    fn register_and_get_scheme() {
        let mut reg = CommandRegistry::new();
        reg.register_scheme("greet", "Say hello", "greet-fn");
        let cmd = reg.get("greet").unwrap();
        assert_eq!(cmd.source, CommandSource::Scheme("greet-fn".into()));
    }

    #[test]
    fn get_missing_returns_none() {
        let reg = CommandRegistry::new();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn contains_works() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("move-down", "Move down");
        assert!(reg.contains("move-down"));
        assert!(!reg.contains("move-up"));
    }

    #[test]
    fn list_names_sorted() {
        let mut reg = CommandRegistry::new();
        reg.register_builtin("zzz", "Last");
        reg.register_builtin("aaa", "First");
        reg.register_builtin("mmm", "Middle");
        let names = reg.list_names();
        assert_eq!(names, vec!["aaa", "mmm", "zzz"]);
    }

    #[test]
    fn with_builtins_has_core_commands() {
        let reg = CommandRegistry::with_builtins();
        assert!(reg.contains("move-up"));
        assert!(reg.contains("move-down"));
        assert!(reg.contains("delete-line"));
        assert!(reg.contains("undo"));
        assert!(reg.contains("redo"));
        assert!(reg.contains("save"));
        assert!(reg.contains("quit"));
        assert!(reg.contains("enter-insert-mode"));
        assert!(reg.contains("enter-normal-mode"));
        assert!(reg.len() >= 30); // at least 30 built-in commands
    }

    #[test]
    fn with_builtins_has_lsp_commands() {
        let reg = CommandRegistry::with_builtins();
        assert!(reg.contains("lsp-goto-definition"));
        assert!(reg.contains("lsp-find-references"));
        assert!(reg.contains("lsp-hover"));
    }

    #[test]
    fn with_builtins_has_agent_commands() {
        let reg = CommandRegistry::with_builtins();
        assert!(reg.contains("agent-list"));
        assert!(reg.contains("agent-setup"));
    }

    #[test]
    fn with_builtins_has_self_test() {
        let reg = CommandRegistry::with_builtins();
        assert!(reg.contains("self-test"));
    }

    #[test]
    fn with_builtins_has_debug_and_recording() {
        let reg = CommandRegistry::with_builtins();
        assert!(reg.contains("debug-attach"));
        assert!(reg.contains("debug-eval"));
        assert!(reg.contains("record-start"));
        assert!(reg.contains("record-stop"));
        assert!(reg.contains("record-save"));
    }

    #[test]
    fn with_builtins_has_ex_command_parity() {
        let reg = CommandRegistry::with_builtins();
        assert!(reg.contains("nohlsearch"));
        assert!(reg.contains("kb-save"));
        assert!(reg.contains("kb-load"));
        assert!(reg.contains("kb-ingest"));
        assert!(reg.contains("ai-save"));
        assert!(reg.contains("ai-load"));
    }
}
