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

        // Leader key placeholder commands
        reg.register_builtin("command-palette", "Search and run any command");
        reg.register_builtin("kill-buffer", "Close current buffer");
        reg.register_builtin("next-buffer", "Cycle to next buffer");
        reg.register_builtin("prev-buffer", "Cycle to previous buffer");
        reg.register_builtin("find-file", "Open a file");
        reg.register_builtin("recent-files", "Open recent file");
        reg.register_builtin("switch-buffer", "Switch to another buffer");
        reg.register_builtin("ai-prompt", "Open AI conversation and prompt");
        reg.register_builtin("ai-cancel", "Cancel current AI operation");
        reg.register_builtin("describe-key", "Show what a key does");
        reg.register_builtin("describe-command", "Show command documentation");
        reg.register_builtin("set-theme", "Set editor color theme");
        reg.register_builtin("cycle-theme", "Cycle to next color theme");

        // Visual mode
        reg.register_builtin("enter-visual-char", "Enter charwise visual mode (v)");
        reg.register_builtin("enter-visual-line", "Enter linewise visual mode (V)");
        reg.register_builtin("visual-delete", "Delete visual selection (d)");
        reg.register_builtin("visual-yank", "Yank visual selection (y)");
        reg.register_builtin("visual-change", "Change visual selection (c)");

        // Search
        reg.register_builtin("search-forward-start", "Search forward (/)");
        reg.register_builtin("search-backward-start", "Search backward (?)");
        reg.register_builtin("search-next", "Jump to next match (n)");
        reg.register_builtin("search-prev", "Jump to previous match (N)");
        reg.register_builtin("search-word-under-cursor", "Search word under cursor (*)");
        reg.register_builtin("clear-search-highlight", "Clear search highlights (:noh)");

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
}
