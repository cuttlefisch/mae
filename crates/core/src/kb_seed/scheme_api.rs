use mae_kb::{KnowledgeBase, Node, NodeKind};

/// Install `scheme:<name>` nodes for all Scheme API functions and variables.
pub(super) fn install_scheme_nodes(kb: &mut KnowledgeBase) {
    // Each entry: (name, signature, doc, example, category)
    let functions: &[(&str, &str, &str, &str, &str)] = &[
        // Buffer editing
        (
            "buffer-insert",
            "(buffer-insert TEXT)",
            "Insert TEXT at the current cursor position.",
            "(buffer-insert \"hello world\")",
            "buffer-editing",
        ),
        (
            "buffer-delete-range",
            "(buffer-delete-range START END)",
            "Delete characters from byte offset START to END.",
            "(buffer-delete-range 0 10)",
            "buffer-editing",
        ),
        (
            "buffer-replace-range",
            "(buffer-replace-range START END TEXT)",
            "Replace characters from START to END with TEXT.",
            "(buffer-replace-range 0 5 \"new\")",
            "buffer-editing",
        ),
        (
            "buffer-undo",
            "(buffer-undo)",
            "Undo the last edit in the active buffer.",
            "(buffer-undo)",
            "buffer-editing",
        ),
        (
            "buffer-redo",
            "(buffer-redo)",
            "Redo the last undone edit in the active buffer.",
            "(buffer-redo)",
            "buffer-editing",
        ),
        // Cursor / navigation
        (
            "cursor-goto",
            "(cursor-goto ROW COL)",
            "Move the cursor to absolute position (0-indexed).",
            "(cursor-goto 0 0) ; go to top-left",
            "navigation",
        ),
        (
            "open-file",
            "(open-file PATH)",
            "Open a file in a new buffer.",
            "(open-file \"/tmp/test.txt\")",
            "navigation",
        ),
        (
            "switch-to-buffer",
            "(switch-to-buffer IDX)",
            "Switch to the buffer at index IDX.",
            "(switch-to-buffer 0)",
            "navigation",
        ),
        // Buffer read
        (
            "buffer-line",
            "(buffer-line N)",
            "Return the text of line N (0-indexed) in the active buffer.",
            "(buffer-line 0) ; first line",
            "buffer-read",
        ),
        (
            "buffer-text-range",
            "(buffer-text-range START END)",
            "Return a substring of the active buffer from char START to END.",
            "(buffer-text-range 0 100)",
            "buffer-read",
        ),
        (
            "get-buffer-by-name",
            "(get-buffer-by-name NAME)",
            "Return the buffer index for NAME, or #f if not found.",
            "(get-buffer-by-name \"*scratch*\")",
            "buffer-read",
        ),
        // Commands
        (
            "define-command",
            "(define-command NAME DOC FN-NAME)",
            "Register a new command backed by a Scheme function.",
            "(define-command \"greet\" \"Say hello\" \"my-greet-fn\")",
            "commands",
        ),
        (
            "run-command",
            "(run-command NAME)",
            "Dispatch a registered command by name.",
            "(run-command \"save\")",
            "commands",
        ),
        (
            "command-exists?",
            "(command-exists? NAME)",
            "Return #t if a command with NAME is registered.",
            "(command-exists? \"save\") ; => #t",
            "commands",
        ),
        // Keymaps
        (
            "define-key",
            "(define-key MAP KEY COMMAND)",
            "Bind KEY in keymap MAP to COMMAND.",
            "(define-key \"normal\" \"g g\" \"goto-first-line\")",
            "keymaps",
        ),
        (
            "define-keymap",
            "(define-keymap NAME PARENT)",
            "Create a new keymap with an optional parent for inheritance.",
            "(define-keymap \"my-mode\" \"normal\")",
            "keymaps",
        ),
        (
            "undefine-key!",
            "(undefine-key! MAP KEY)",
            "Remove a key binding from a keymap.",
            "(undefine-key! \"normal\" \"q\")",
            "keymaps",
        ),
        (
            "keymap-bindings",
            "(keymap-bindings MAP-NAME)",
            "Return a list of (key command) pairs for a keymap.",
            "(keymap-bindings \"normal\")",
            "keymaps",
        ),
        // Options
        (
            "set-option!",
            "(set-option! KEY VALUE)",
            "Set a global editor option.",
            "(set-option! \"theme\" \"dracula\")",
            "options",
        ),
        (
            "set-local-option!",
            "(set-local-option! KEY VALUE)",
            "Set a buffer-local option on the active buffer.",
            "(set-local-option! \"word_wrap\" \"true\")",
            "options",
        ),
        (
            "get-option",
            "(get-option NAME)",
            "Return the current value of an option as a string, or #f.",
            "(get-option \"theme\") ; => \"dracula\"",
            "options",
        ),
        // Hooks
        (
            "add-hook!",
            "(add-hook! HOOK-NAME FN-NAME)",
            "Register FN-NAME to run when HOOK-NAME fires.",
            "(add-hook! \"buffer-open\" \"my-on-open\")",
            "hooks",
        ),
        (
            "remove-hook!",
            "(remove-hook! HOOK-NAME FN-NAME)",
            "Remove a function from a hook.",
            "(remove-hook! \"buffer-open\" \"my-on-open\")",
            "hooks",
        ),
        // Display
        (
            "set-status",
            "(set-status MSG)",
            "Set the status bar message.",
            "(set-status \"Done!\")",
            "display",
        ),
        (
            "set-theme",
            "(set-theme NAME)",
            "Switch the editor theme.",
            "(set-theme \"gruvbox\")",
            "display",
        ),
        (
            "message",
            "(message TEXT)",
            "Append TEXT to the *Messages* log buffer.",
            "(message \"Init complete\")",
            "display",
        ),
        // Visual buffer (canvas)
        (
            "visual-buffer-add-rect!",
            "(visual-buffer-add-rect! X Y W H FILL STROKE)",
            "Draw a rectangle on the visual canvas.",
            "(visual-buffer-add-rect! 10.0 10.0 100.0 50.0 \"#ff0000\" #f)",
            "visual",
        ),
        (
            "visual-buffer-clear!",
            "(visual-buffer-clear!)",
            "Clear all shapes from the visual canvas.",
            "(visual-buffer-clear!)",
            "visual",
        ),
        (
            "visual-buffer-add-line!",
            "(visual-buffer-add-line! X1 Y1 X2 Y2 COLOR THICKNESS)",
            "Draw a line on the visual canvas.",
            "(visual-buffer-add-line! 0.0 0.0 100.0 100.0 \"white\" 2.0)",
            "visual",
        ),
        (
            "visual-buffer-add-circle!",
            "(visual-buffer-add-circle! CX CY R FILL STROKE)",
            "Draw a circle on the visual canvas.",
            "(visual-buffer-add-circle! 50.0 50.0 25.0 \"blue\" #f)",
            "visual",
        ),
        (
            "visual-buffer-add-text!",
            "(visual-buffer-add-text! X Y TEXT FONT-SIZE COLOR)",
            "Draw text on the visual canvas.",
            "(visual-buffer-add-text! 10.0 20.0 \"Hello\" 16.0 \"white\")",
            "visual",
        ),
        // Shell
        (
            "shell-send-input",
            "(shell-send-input BUF-IDX TEXT)",
            "Send TEXT to the PTY of a terminal buffer.",
            "(shell-send-input 1 \"ls\\n\")",
            "shell",
        ),
        (
            "shell-cwd",
            "(shell-cwd BUF-IDX)",
            "Return the current working directory of a shell buffer.",
            "(shell-cwd 1)",
            "shell",
        ),
        (
            "shell-read-output",
            "(shell-read-output BUF-IDX MAX-LINES)",
            "Read the last MAX-LINES from a shell buffer's viewport.",
            "(shell-read-output 1 20)",
            "shell",
        ),
        // File I/O
        (
            "read-file",
            "(read-file PATH)",
            "Read a file's contents as a string (max 1MB).",
            "(read-file \"/etc/hostname\")",
            "file-io",
        ),
        (
            "file-exists?",
            "(file-exists? PATH)",
            "Return #t if PATH exists on disk.",
            "(file-exists? \"/tmp/test.txt\")",
            "file-io",
        ),
        (
            "list-directory",
            "(list-directory PATH)",
            "Return a list of (name is-dir?) pairs for entries in PATH.",
            "(list-directory \"/tmp\")",
            "file-io",
        ),
        // Packages
        (
            "provide-feature",
            "(provide-feature FEATURE)",
            "Mark FEATURE as loaded in the package system.",
            "(provide-feature \"my-package\")",
            "packages",
        ),
        (
            "featurep",
            "(featurep FEATURE)",
            "Return #t if FEATURE has been loaded.",
            "(featurep \"my-package\")",
            "packages",
        ),
        (
            "require-feature",
            "(require-feature FEATURE)",
            "Request loading of FEATURE from the load-path.",
            "(require-feature \"my-package\")",
            "packages",
        ),
        (
            "load-path",
            "(load-path)",
            "Return the current load-path as a list of directory strings.",
            "(load-path)",
            "packages",
        ),
        (
            "add-to-load-path!",
            "(add-to-load-path! DIR)",
            "Prepend DIR to the load-path.",
            "(add-to-load-path! \"~/.config/mae/lisp\")",
            "packages",
        ),
        (
            "autoload",
            "(autoload COMMAND FEATURE DOC)",
            "Register a command that auto-loads FEATURE on first use.",
            "(autoload \"my-cmd\" \"my-pkg\" \"Does something\")",
            "packages",
        ),
        // Recent files
        (
            "recent-files-add!",
            "(recent-files-add! PATH)",
            "Add PATH to the recent files list.",
            "(recent-files-add! \"/tmp/test.txt\")",
            "navigation",
        ),
        (
            "recent-projects-add!",
            "(recent-projects-add! PATH)",
            "Add PATH to the recent projects list.",
            "(recent-projects-add! \"~/src/my-project\")",
            "navigation",
        ),
        // Display policy
        (
            "display-buffer-policy",
            "(display-buffer-policy KIND)",
            "Query the active display rule for a BufferKind. Returns a string like \"reuse-or-split:vertical:0.5\" or \"avoid-conversation\".",
            "(display-buffer-policy \"help\")",
            "configuration",
        ),
        (
            "set-display-rule!",
            "(set-display-rule! KIND ACTION)",
            "Override the display policy for a BufferKind. ACTION formats: \"replace-focused\", \"avoid-conversation\", \"hidden\", \"reuse-or-split:vertical:0.5\".",
            "(set-display-rule! \"help\" \"replace-focused\")",
            "configuration",
        ),
    ];

    // Variables (injected from editor state before each eval)
    let variables: &[(&str, &str, &str)] = &[
        ("*buffer-name*", "string", "Name of the active buffer."),
        (
            "*buffer-modified?*",
            "boolean",
            "Whether the active buffer has unsaved changes.",
        ),
        (
            "*buffer-line-count*",
            "integer",
            "Number of lines in the active buffer.",
        ),
        (
            "*buffer-char-count*",
            "integer",
            "Total characters in the active buffer.",
        ),
        (
            "*buffer-text*",
            "string",
            "Full text content of the active buffer.",
        ),
        ("*buffer-count*", "integer", "Number of open buffers."),
        (
            "*buffer-list*",
            "list of (index name kind modified?)",
            "Information about all open buffers.",
        ),
        (
            "*buffer-language*",
            "string",
            "Detected language of the active buffer (e.g. \"rust\", \"text\").",
        ),
        (
            "*buffer-file-path*",
            "string",
            "File path of the active buffer, or empty if unsaved.",
        ),
        ("*cursor-row*", "integer", "Current cursor row (0-indexed)."),
        (
            "*cursor-col*",
            "integer",
            "Current cursor column (0-indexed).",
        ),
        (
            "*mode*",
            "string",
            "Current editor mode (\"normal\", \"insert\", \"visual\", etc).",
        ),
        ("*window-count*", "integer", "Number of open windows."),
        (
            "*window-list*",
            "list of (id buffer-idx cursor-row cursor-col)",
            "Information about all windows.",
        ),
        (
            "*shell-buffers*",
            "list of integers",
            "Buffer indices that are shell terminals.",
        ),
        (
            "*option-list*",
            "list of (name kind default doc)",
            "All registered editor options.",
        ),
        (
            "*command-list*",
            "list of (name doc source)",
            "All registered commands.",
        ),
        ("*keymap-list*", "list of strings", "Names of all keymaps."),
    ];

    for &(name, sig, doc, example, category) in functions {
        let body = format!(
            "## Signature\n```scheme\n{sig}\n```\n\n\
             {doc}\n\n\
             ## Example\n```scheme\n{example}\n```\n\n\
             **Category:** {category}\n\n\
             See also: [[concept:scheme-api]], [[index]]"
        );
        let id = format!("scheme:{}", name);
        let title = format!("Scheme: {}", name);
        kb.insert(
            Node::new(id, title, NodeKind::Concept, body).with_tags(["scheme", "api", category]),
        );
    }

    for &(name, typ, doc) in variables {
        let body = format!(
            "**Type:** {typ}\n\n\
             {doc}\n\n\
             This is a read-only variable injected from editor state before each Scheme evaluation. \
             Access it directly by name in your Scheme code.\n\n\
             ## Example\n```scheme\n(message (string-append \"Buffer: \" {name}))\n```\n\n\
             See also: [[concept:scheme-api]], [[index]]"
        );
        let id = format!("scheme:{}", name);
        let title = format!("Scheme: {}", name);
        kb.insert(
            Node::new(id, title, NodeKind::Concept, body).with_tags(["scheme", "api", "variable"]),
        );
    }
}
