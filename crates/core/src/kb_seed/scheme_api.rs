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
            "Create a new keymap NAME inheriting from keymap PARENT.",
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
        // Buffer introspection
        (
            "current-buffer-name",
            "(current-buffer-name)",
            "Return the name of the active buffer as a string.",
            "(current-buffer-name) ; => \"main.rs\"",
            "buffer-read",
        ),
        (
            "current-buffer-file",
            "(current-buffer-file)",
            "Return the file path of the active buffer, or #f if unsaved.",
            "(current-buffer-file) ; => \"/home/user/src/main.rs\"",
            "buffer-read",
        ),
        (
            "create-buffer",
            "(create-buffer NAME)",
            "Create a new empty buffer with the given name and return its index.",
            "(create-buffer \"*notes*\")",
            "buffer-editing",
        ),
        (
            "kill-buffer-by-name",
            "(kill-buffer-by-name NAME)",
            "Close the buffer matching NAME.",
            "(kill-buffer-by-name \"*scratch*\")",
            "buffer-editing",
        ),
        // Cursor position
        (
            "current-line-number",
            "(current-line-number)",
            "Return the current cursor line number (0-indexed).",
            "(current-line-number) ; => 42",
            "navigation",
        ),
        (
            "current-column",
            "(current-column)",
            "Return the current cursor column (0-indexed).",
            "(current-column) ; => 10",
            "navigation",
        ),
        (
            "point",
            "(point)",
            "Return the cursor position as a byte offset from the start of the buffer.",
            "(point) ; => 1024",
            "navigation",
        ),
        (
            "point-min",
            "(point-min)",
            "Return the minimum valid byte offset (always 0).",
            "(point-min) ; => 0",
            "navigation",
        ),
        (
            "point-max",
            "(point-max)",
            "Return the maximum valid byte offset (end of buffer).",
            "(point-max) ; => 5678",
            "navigation",
        ),
        (
            "line-beginning-position",
            "(line-beginning-position)",
            "Return the byte offset of the beginning of the current line.",
            "(line-beginning-position)",
            "navigation",
        ),
        (
            "line-end-position",
            "(line-end-position)",
            "Return the byte offset of the end of the current line.",
            "(line-end-position)",
            "navigation",
        ),
        // Region / selection
        (
            "region-active?",
            "(region-active?)",
            "Return #t if a visual selection (region) is currently active.",
            "(when (region-active?) (message \"Selection active\"))",
            "navigation",
        ),
        (
            "region-beginning",
            "(region-beginning)",
            "Return the byte offset of the start of the active region, or #f.",
            "(region-beginning)",
            "navigation",
        ),
        (
            "region-end",
            "(region-end)",
            "Return the byte offset of the end of the active region, or #f.",
            "(region-end)",
            "navigation",
        ),
        (
            "get-selection",
            "(get-selection)",
            "Return the text of the active visual selection, or #f if none.",
            "(get-selection) ; => \"selected text\"",
            "navigation",
        ),
        // Module system
        (
            "define-option!",
            "(define-option! NAME KIND DEFAULT DOC)",
            "Register a new editor option with type, default value, and documentation.",
            "(define-option! \"my-indent\" \"integer\" \"4\" \"Indentation width\")",
            "modules",
        ),
        (
            "register-module!",
            "(register-module! NAME VERSION)",
            "Register a loaded module (name + version) in the module registry.",
            "(register-module! \"my-module\" \"0.1.0\")",
            "modules",
        ),
        (
            "module-loaded?",
            "(module-loaded? NAME)",
            "Return #t if the named module is currently loaded.",
            "(module-loaded? \"org\") ; => #t",
            "modules",
        ),
        (
            "module-version",
            "(module-version NAME)",
            "Return the version string of a loaded module, or #f.",
            "(module-version \"org\") ; => \"0.1.0\"",
            "modules",
        ),
        (
            "module-list",
            "(module-list)",
            "Return a list of all loaded module names.",
            "(module-list) ; => (\"org\" \"markdown\" ...)",
            "modules",
        ),
        (
            "module-flags",
            "(module-flags NAME)",
            "Return the flags alist for a loaded module.",
            "(module-flags \"org\")",
            "modules",
        ),
        (
            "mae-declared-modules",
            "(mae-declared-modules)",
            "Return a list of all modules declared via mae-declare-module!.",
            "(mae-declared-modules)",
            "modules",
        ),
        (
            "mae-declare-module!",
            "(mae-declare-module! NAME FLAGS)",
            "Declare a module to be loaded by the module system. FLAGS is a list of feature-flag strings.",
            "(mae-declare-module! \"org\" '(\"+journal\"))",
            "modules",
        ),
        (
            "mae-declare-package!",
            "(mae-declare-package! NAME SOURCE PIN DISABLE)",
            "Declare a third-party package dependency. SOURCE/PIN may be empty strings; DISABLE is a boolean.",
            "(mae-declare-package! \"my-package\" \"\" \"\" #f)",
            "modules",
        ),
        // Module cleanup
        (
            "undefine-command!",
            "(undefine-command! NAME)",
            "Remove a command from the command registry.",
            "(undefine-command! \"my-old-command\")",
            "modules",
        ),
        (
            "undefine-option!",
            "(undefine-option! NAME)",
            "Remove an option from the option registry.",
            "(undefine-option! \"obsolete-option\")",
            "modules",
        ),
        (
            "unload-feature",
            "(unload-feature NAME)",
            "Mark a feature as unloaded in the package system.",
            "(unload-feature \"my-package\")",
            "modules",
        ),
        // AI tool registration
        (
            "register-ai-tool!",
            "(register-ai-tool! NAME DESCRIPTION HANDLER PERMISSION-TIER)",
            "Register a custom AI tool backed by a Scheme function. PERMISSION-TIER is the required tier (e.g. \"read\", \"write\").",
            "(register-ai-tool! \"my-tool\" \"Does something\" \"my-handler-fn\" \"write\")",
            "ai-tools",
        ),
        (
            "ai-tool-param!",
            "(ai-tool-param! TOOL-NAME PARAM-NAME TYPE DESCRIPTION)",
            "Add a parameter definition to a registered AI tool. Mark it required with ai-tool-require!.",
            "(ai-tool-param! \"my-tool\" \"query\" \"string\" \"Search query\")",
            "ai-tools",
        ),
        (
            "ai-tool-require!",
            "(ai-tool-require! TOOL-NAME PARAM-NAME)",
            "Mark a previously-added parameter of an AI tool as required.",
            "(ai-tool-require! \"my-tool\" \"query\")",
            "ai-tools",
        ),
        // KB authoring
        (
            "define-kb-node!",
            "(define-kb-node! ID TITLE BODY)",
            "Create or update a knowledge base node from Scheme.",
            "(define-kb-node! \"concept:my-topic\" \"My Topic\" \"Body text\")",
            "kb-authoring",
        ),
        (
            "kb-agenda",
            "(kb-agenda FILTER [ARGS])",
            "Query the KB graph via CozoDB Datalog. Filters: todo, priority, tag, orphan, stale, dead-end, custom.",
            "(kb-agenda \"orphan\")  ;; or (kb-agenda \"todo\" \"TODO\")",
            "kb-graph",
        ),
        (
            "kb-history",
            "(kb-history NODE-ID)",
            "Show version history for a KB node. Requires CozoDB backend.",
            "(kb-history \"concept:buffer\")",
            "kb-graph",
        ),
        (
            "kb-restore",
            "(kb-restore NODE-ID VERSION)",
            "Restore a KB node to a previous version with integrity verification.",
            "(kb-restore \"concept:buffer\" 2)",
            "kb-graph",
        ),
        (
            "kb-raw-query",
            "(kb-raw-query DATALOG-STRING)",
            "Execute a raw CozoDB Datalog query against the knowledge base.",
            "(kb-raw-query \"?[id, title] := *nodes{id, title, kind}, kind = \\\"concept\\\"\")",
            "kb-graph",
        ),
        // KB sharing / collaboration lifecycle
        (
            "kb-share",
            "(kb-share [KB-NAME])",
            "Share a knowledge base for collaborative editing (defaults to the primary KB).",
            "(kb-share) ; or (kb-share \"my-kb\")",
            "kb-sharing",
        ),
        (
            "kb-join",
            "(kb-join KB-ID)",
            "Join a shared knowledge base advertised by the daemon.",
            "(kb-join \"alice-kb\")",
            "kb-sharing",
        ),
        (
            "kb-leave",
            "(kb-leave KB-ID)",
            "Leave a shared knowledge base. The local copy is preserved.",
            "(kb-leave \"alice-kb\")",
            "kb-sharing",
        ),
        (
            "kb-add-member",
            "(kb-add-member KB-ID FINGERPRINT [ROLE])",
            "Add a peer to a shared KB by fingerprint (owner-only). ROLE defaults to \"editor\".",
            "(kb-add-member \"my-kb\" \"ab:cd:ef\" \"viewer\")",
            "kb-sharing",
        ),
        (
            "kb-remove-member",
            "(kb-remove-member KB-ID FINGERPRINT)",
            "Remove a peer from a shared KB by fingerprint (owner-only).",
            "(kb-remove-member \"my-kb\" \"ab:cd:ef\")",
            "kb-sharing",
        ),
        (
            "kb-approve",
            "(kb-approve KB-ID FINGERPRINT [ROLE])",
            "Approve a pending join request by fingerprint at a role (owner-only). ROLE defaults to \"editor\".",
            "(kb-approve \"my-kb\" \"ab:cd:ef\" \"editor\")",
            "kb-sharing",
        ),
        (
            "kb-set-policy",
            "(kb-set-policy KB-ID POLICY)",
            "Set a shared KB's join policy (owner-only): restrictive | invite | permissive.",
            "(kb-set-policy \"my-kb\" \"invite\")",
            "kb-sharing",
        ),
        (
            "kb-sharing-status",
            "(kb-sharing-status)",
            "Return a JSON snapshot of this peer's KB-sharing state (members, roles, policy, pending requests, my role/epoch). Parse it Scheme-side.",
            "(kb-sharing-status)",
            "kb-sharing",
        ),
        // Daemon capability model (ADR-035)
        (
            "daemon-available?",
            "(daemon-available?)",
            "Return #t when a daemon is present (control or read layer) right now, #f otherwise. The same capability model the AI peer (daemon_status MCP tool) and editor surfaces read.",
            "(if (daemon-available?) (kb-share-p2p \"notes\") (set-status \"start a daemon first\"))",
            "kb-sharing",
        ),
        (
            "daemon-status",
            "(daemon-status)",
            "Return a JSON snapshot of daemon state + per-feature availability (mode, present/connected/hosting, and each daemon-dependent feature's requirement + availability with reason/fix). Parse it Scheme-side.",
            "(daemon-status)",
            "kb-sharing",
        ),
        (
            "feature-available?",
            "(feature-available? FEATURE-ID)",
            "Return a JSON object describing a daemon-dependent feature's availability by id (e.g. \"p2p-sharing\", \"continuous-sync\", \"kb-hosting\"): {available, requirement, reason, fix}. Lets scripts gate on a Requires/Recommends feature with an actionable reason.",
            "(feature-available? \"p2p-sharing\")",
            "kb-sharing",
        ),
        // Advice system
        (
            "advice-add!",
            "(advice-add! COMMAND WHERE FN-NAME)",
            "Add advice to a command. WHERE is :before or :after.",
            "(advice-add! \"save\" :before \"my-before-save\")",
            "advice",
        ),
        (
            "advice-remove!",
            "(advice-remove! COMMAND FN-NAME)",
            "Remove advice (by advising function name) from a command.",
            "(advice-remove! \"save\" \"my-before-save\")",
            "advice",
        ),
        // Deprecation
        (
            "deprecate-function!",
            "(deprecate-function! OLD-NAME NEW-NAME SINCE)",
            "Mark OLD-NAME as deprecated in favor of NEW-NAME since version SINCE. Emits a warning on use.",
            "(deprecate-function! \"old-fn\" \"new-fn\" \"0.14.0\")",
            "modules",
        ),
        (
            "check-deprecated",
            "(check-deprecated NAME)",
            "Check if NAME is deprecated. Returns the replacement name or #f.",
            "(check-deprecated \"old-fn\") ; => \"new-fn\"",
            "modules",
        ),
        // String utilities
        (
            "string-split",
            "(string-split STR DELIMITER)",
            "Split STR by DELIMITER, returning a list of strings.",
            "(string-split \"a,b,c\" \",\") ; => (\"a\" \"b\" \"c\")",
            "string-utils",
        ),
        (
            "string-join",
            "(string-join LIST SEPARATOR)",
            "Join a list of strings with SEPARATOR.",
            "(string-join '(\"a\" \"b\" \"c\") \",\") ; => \"a,b,c\"",
            "string-utils",
        ),
        (
            "string-trim",
            "(string-trim STR)",
            "Remove leading and trailing whitespace from STR.",
            "(string-trim \"  hello  \") ; => \"hello\"",
            "string-utils",
        ),
        (
            "string-contains?",
            "(string-contains? HAYSTACK NEEDLE)",
            "Return #t if HAYSTACK contains NEEDLE.",
            "(string-contains? \"hello world\" \"world\") ; => #t",
            "string-utils",
        ),
        (
            "string-replace",
            "(string-replace STR OLD NEW)",
            "Replace all occurrences of OLD with NEW in STR.",
            "(string-replace \"foo bar foo\" \"foo\" \"baz\") ; => \"baz bar baz\"",
            "string-utils",
        ),
        (
            "string-upcase",
            "(string-upcase STR)",
            "Return STR converted to uppercase.",
            "(string-upcase \"hello\") ; => \"HELLO\"",
            "string-utils",
        ),
        (
            "string-downcase",
            "(string-downcase STR)",
            "Return STR converted to lowercase.",
            "(string-downcase \"HELLO\") ; => \"hello\"",
            "string-utils",
        ),
        // Shell
        (
            "shell-command",
            "(shell-command CMD)",
            "Execute CMD in a subprocess and return its stdout as a string.",
            "(shell-command \"date\") ; => \"Mon May 12 ...\"",
            "shell",
        ),
        // Agenda
        (
            "agenda-add!",
            "(agenda-add! FILE)",
            "Add an org file to the agenda sources list.",
            "(agenda-add! \"~/org/todo.org\")",
            "agenda",
        ),
        (
            "agenda-remove!",
            "(agenda-remove! FILE)",
            "Remove an org file from the agenda sources list.",
            "(agenda-remove! \"~/org/todo.org\")",
            "agenda",
        ),
        (
            "agenda-list",
            "(agenda-list)",
            "Return the list of current agenda source files.",
            "(agenda-list) ; => (\"/home/user/org/todo.org\" ...)",
            "agenda",
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
        // Testing framework (mae-test.scm)
        (
            "describe-group",
            "(describe-group NAME THUNK)",
            "BDD grouping — sets a group prefix for nested it-test blocks. THUNK is a zero-argument lambda that registers tests.",
            "(describe-group \"My feature\" (lambda () (it-test \"works\" (lambda () (should #t)))))",
            "testing",
        ),
        (
            "it-test",
            "(it-test NAME THUNK)",
            "Register a test within a describe-group. NAME is prefixed with the group name. THUNK is the test body.",
            "(it-test \"inserts text\" (lambda () (buffer-insert \"hello\") (should-equal (buffer-string) \"hello\")))",
            "testing",
        ),
        (
            "should",
            "(should VAL)",
            "Assert VAL is truthy. Signals an error on failure.",
            "(should (> 3 2))",
            "testing",
        ),
        (
            "should-not",
            "(should-not VAL)",
            "Assert VAL is falsy. Signals an error on failure.",
            "(should-not (= 1 2))",
            "testing",
        ),
        (
            "should-equal",
            "(should-equal A B)",
            "Assert A equals B (using equal?). Error message includes expected vs actual values.",
            "(should-equal (buffer-string) \"expected text\")",
            "testing",
        ),
        (
            "should-contain",
            "(should-contain HAYSTACK NEEDLE)",
            "Assert HAYSTACK string contains NEEDLE substring.",
            "(should-contain (buffer-string) \"hello\")",
            "testing",
        ),
        (
            "should-error",
            "(should-error THUNK)",
            "Assert THUNK signals an error. Passes if an error is raised, fails if THUNK returns normally.",
            "(should-error (lambda () (error \"expected\")))",
            "testing",
        ),
        (
            "should-match",
            "(should-match HAYSTACK PATTERN)",
            "Assert HAYSTACK string contains PATTERN substring. Alias for should-contain with pattern-oriented naming.",
            "(should-match (buffer-string) \"hello\")",
            "testing",
        ),
        (
            "before-each",
            "(before-each HOOK-FN)",
            "Register a setup function for the current describe scope. Called before each it-test.",
            "(before-each (lambda () (create-buffer \"*test*\")))",
            "testing",
        ),
        (
            "after-each",
            "(after-each HOOK-FN)",
            "Register a teardown function for the current describe scope. Called after each it-test.",
            "(after-each (lambda () (kill-buffer-by-name \"*test*\")))",
            "testing",
        ),
        (
            "wait-until",
            "(wait-until PRED TIMEOUT-MS)",
            "Poll PRED every 50ms, sleeping between checks (event-loop-aware). Returns #t on success, signals error on timeout.",
            "(wait-until (lambda () (file-exists? \"/tmp/result.txt\")) 5000)",
            "testing",
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
            "** Signature\n```scheme\n{sig}\n```\n\n\
             {doc}\n\n\
             ## Example\n```scheme\n{example}\n```\n\n\
             **Category:** {category}\n\n\
             See also: [[concept:scheme-api]], [[index]]"
        );
        let id = format!("scheme:{}", name);
        let title = format!("Scheme: {}", name);
        kb.insert(
            Node::new(id, title, NodeKind::SchemeApi, body).with_tags(["scheme", "api", category]),
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
            Node::new(id, title, NodeKind::SchemeApi, body)
                .with_tags(["scheme", "api", "variable"]),
        );
    }
}
