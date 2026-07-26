use mae_core::OptionRegistry;

use crate::types::*;

use super::tool_def::ToolDefBuilder;

/// Core tool definitions: buffer, cursor, file, editor state, project, visual,
/// introspection, conversation persistence, and miscellaneous editor tools.
///
/// Takes an `OptionRegistry` because `set_option` needs to enumerate valid options.
pub(super) fn core_tool_definitions(registry: &OptionRegistry) -> Vec<ToolDefinition> {
    let option_names: Vec<String> = registry.list().iter().map(|o| o.name.to_string()).collect();
    let option_desc = {
        let items: Vec<String> = registry
            .list()
            .iter()
            .map(|o| format!("'{}' ({})", o.name, o.kind))
            .collect();
        format!(
            "Set an editor option by name. Options: {}.",
            items.join(", ")
        )
    };
    vec![
        // --- Buffer tools ---
        ToolDefBuilder::new("buffer_read", "Read buffer contents. Returns text with line numbers.")
            .prop("start_line", "integer", "First line to read (1-indexed, default: 1)")
            .prop("end_line", "integer", "Last line to read (inclusive, default: last line)")
            .prop("buffer_name", "string", "Buffer name to read (default: active buffer)")
            .permission(PermissionTier::ReadOnly)
            .build(),
        ToolDefBuilder::new(
            "buffer_write",
            "Replace a range of lines with new content. Use for insert, delete, and replace operations.",
        )
        .prop("start_line", "integer", "First line to replace (1-indexed)")
        .prop(
            "end_line",
            "integer",
            "Last line to replace (inclusive). Omit to insert before start_line.",
        )
        .prop("content", "string", "New content (empty string to delete lines)")
        .prop("buffer_name", "string", "Buffer name to write to (default: active buffer)")
        .required(["start_line", "content"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Cursor & state ---
        ToolDefBuilder::new(
            "cursor_info",
            "Get current cursor position, mode, buffer name, and line count.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- File tools ---
        ToolDefBuilder::new(
            "file_read",
            "Read a file from disk. Returns contents with line numbers. Path supports ~ (home dir). If not found, call audit_configuration for correct paths.",
        )
        .prop("path", "string", "File path to read")
        .required(["path"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "list_buffers",
            "List all open buffers with names, modified status, and file paths.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "editor_state",
            "Full JSON snapshot of editor state: mode, theme, buffer count, window count, active buffer, message log size, debug session status.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "read_messages",
            "Read the editor's *Messages* log buffer. Shows errors, warnings, and info from all subsystems (DAP, LSP, AI, etc.). Essential for diagnosing command failures — check this when a tool call fails or times out.",
        )
        .prop("last_n", "integer", "Number of recent messages to return (default: 30)")
        .prop_enum(
            "level",
            "string",
            "Minimum severity: error, warn, info, debug, trace (default: info)",
            ["error", "warn", "info", "debug", "trace"],
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "notifications_list",
            "List outstanding + recently-resolved notifications from the attention bus (ADR-024) as JSON: id, severity, source, title, body, actions, resolved. The agent/headless path for seeing what demands attention (e.g. a collab edit fenced and needing resolution).",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "notify_resolve",
            "Resolve an outstanding notification by id: run one of its actions (by index) or dismiss it. For a collab fenced-edit, action 0 = Accept-remote (discard local), 1 = Keep-mine (re-author), 2 = Stash externally.",
        )
        .prop("id", "integer", "Notification id (from notifications_list).")
        .prop("action", "integer", "Action index to run. Omit to dismiss the notification.")
        .required(["id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "window_layout",
            "JSON of all windows (window_id, buffer_idx/kind/name, kb_node_id if applicable, cursor/scroll) plus shared_buffer_groups: any buffer_idx shown by more than one window, flagged explicitly instead of requiring a manual cross-check against list_buffers.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "execute_command",
            "Execute a registered editor command by name. Equivalent to typing the command in command mode. Use command_list to discover available commands.",
        )
        .prop(
            "command",
            "string",
            "The command name to execute (e.g. 'move-to-last-line', 'scroll-down-line')",
        )
        .required(["command"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "command_list",
            "List all available editor commands. Use format='names' for a compact list of just command names (recommended). Default returns full JSON with docs and sources.",
        )
        .prop(
            "format",
            "string",
            "Output format: 'names' for compact name-only list, 'full' for JSON with docs (default: 'full')",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "debug_state",
            "If a debug session is active, returns full JSON of threads, scopes, variables, breakpoints. Otherwise returns 'No active debug session'.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "open_file",
            "Open a file from disk into a new buffer and switch to it. If already open, switches to the existing buffer.",
        )
        .prop("path", "string", "File path to open")
        .required(["path"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "switch_buffer",
            "Switch the active buffer by name. Use list_buffers to see available buffers.",
        )
        .prop("name", "string", "Buffer name to switch to")
        .required(["name"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "close_buffer",
            "Close a buffer by name. Fails if the buffer has unsaved changes unless force=true.",
        )
        .prop("name", "string", "Buffer name to close (default: active buffer)")
        .prop(
            "force",
            "boolean",
            "If true, close even if the buffer has unsaved changes (default: false)",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new("create_file", "Create a new file with content and open it as a buffer.")
            .prop("path", "string", "File path to create")
            .prop("content", "string", "Initial file content (default: empty)")
            .required(["path"])
            .permission(PermissionTier::Write)
            .build(),
        // --- Project tools ---
        ToolDefBuilder::new(
            "project_info",
            "Get project state: name, root, config, recent files, and display settings (line numbers, relative line numbers, word wrap).",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "project_files",
            "List files in the project. Uses git ls-files if in a git repo, otherwise lists files recursively. Returns one path per line.",
        )
        .prop(
            "pattern",
            "string",
            "Optional glob pattern to filter files (e.g. '*.rs', 'src/**/*.toml')",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "project_search",
            "Search across project files using a regex pattern. Returns matching lines with file paths and line numbers.",
        )
        .prop("pattern", "string", "Regex pattern to search for")
        .prop("glob", "string", "Optional file glob to limit search (e.g. '*.rs')")
        .prop("max_results", "integer", "Maximum number of results (default: 100)")
        .required(["pattern"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Project management ---
        ToolDefBuilder::new(
            "switch_project",
            "Switch the active project to a new root directory. Detects project markers (.git, Cargo.toml, etc.) and updates the editor's project context. Adds the root to recent projects.",
        )
        .prop("path", "string", "Absolute path to the project root directory")
        .required(["path"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Editor settings ---
        ToolDefBuilder::new(
            "get_option",
            "Get current value of an editor option, or list all options. Returns name, current value, type, default, and documentation. Call with no name (or name='all') to list everything. Use before set_option to read current values.",
        )
        .prop(
            "name",
            "string",
            "Option name to query, or 'all' to list everything. Omit for all options.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new("set_option", option_desc)
            .prop_enum("option", "string", "Option name", option_names)
            .prop("value", "string", "New value for the option")
            .required(["option", "value"])
            .permission(PermissionTier::Write)
            .build(),
        // --- Agent UX tools ---
        ToolDefBuilder::new(
            "ask_user",
            "Ask the user a clarifying question when the current instructions are ambiguous or more context is needed. The AI session will pause until the user replies.",
        )
        .prop("question", "string", "The question to ask the user.")
        .required(["question"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "log_activity",
            "Log a visible status update or reasoning step to the user's AI buffer. Use this during long operations to keep the user informed of your progress and current focus.",
        )
        .prop("activity", "string", "Description of the current activity or reasoning step.")
        .required(["activity"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "propose_changes",
            "Propose a set of file changes for user approval. Displays a diff and waits for user to accept or reject. Use this for potentially destructive or large changes to ensure safety.",
        )
        .prop_array_of_objects(
            "changes",
            "List of changes, each with file_path and new_content.",
            [
                ("file_path", "string", "Path to the file to change"),
                ("new_content", "string", "The file's full new content"),
            ],
            ["file_path", "new_content"],
        )
        .required(["changes"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Self-test suite (v3: sandbox, grading, exam categories) ---
        ToolDefBuilder::new(
            "self_test_suite",
            "Unified test suite for MAE's AI tool surface. Actions: 'plan' (default) returns a v3 JSON test plan with sandbox paths, deterministic grading specs, and both direct-tool + prompt-based tests. 'grade' accepts results array and returns deterministic ExamResult with verdict. File writes are confined to a sandbox directory during test mode.",
        )
        .prop_enum(
            "action",
            "string",
            "Action: 'plan' to get test plan, 'grade' to grade results (default: plan)",
            ["plan", "grade"],
        )
        .prop(
            "categories",
            "string",
            "Comma-separated categories (default: all). Options: introspection, editing, help, project, lsp, dap, git, performance, scrolling, babel, modules, federation, guidance, tool_selection, parameter_accuracy, output_interpretation, multi_step, pushback",
        )
        .prop(
            "results",
            "array",
            "For 'grade' action: array of {test_id, output, success, grading, tool_calls?, final_text?}",
        )
        .prop("model", "string", "For 'grade' action: model name for the exam result")
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Transcript Access ---
        ToolDefBuilder::new(
            "read_transcript",
            "Read the full JSON transcript of the current AI session. This contains the raw provider responses, full tool outputs, and reasoning steps. Use this if you get stuck or need to review your own previous thoughts in detail. No arguments needed.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "input_lock",
            "Lock or unlock editor keyboard input. When locked, all user keystrokes are discarded except Esc/Ctrl-C (which cancel and unlock). Use this before running multi-step operations (like self-tests) to prevent user input from interfering with editor state. Check current lock status via 'introspect' to avoid redundant calls.",
        )
        .prop("locked", "boolean", "true to lock input, false to unlock")
        .required(["locked"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "set_ai_target",
            "Set the target buffer/window for subsequent AI tool calls. Avoids changing user focus. Use this before operating on a non-focused buffer (e.g. in split layouts where the MCP shim runs in a terminal window).",
        )
        .prop("buffer_name", "string", "Target buffer by name")
        .prop("window_id", "integer", "Target window by ID (from window_layout)")
        .prop("clear", "boolean", "Clear targeting (revert to focused window)")
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "trigger_hook",
            "Manually fire a lifecycle hook by name. This triggers all Scheme functions registered for that hook point.",
        )
        .prop_enum(
            "hook_name",
            "string",
            "Name of the hook to fire (e.g. 'app-start', 'buffer-open')",
            mae_core::hooks::WELL_KNOWN_HOOKS.iter().copied(),
        )
        .required(["hook_name"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Visual Debugger ---
        ToolDefBuilder::new("visual_buffer_add_rect", "Add a rectangle to the active visual buffer.")
            .prop("x", "number", "X coordinate")
            .prop("y", "number", "Y coordinate")
            .prop("w", "number", "Width")
            .prop("h", "number", "Height")
            .prop("fill", "string", "Fill hex color")
            .prop("stroke", "string", "Stroke hex color")
            .required(["x", "y", "w", "h"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("visual_buffer_add_line", "Add a line to the active visual buffer.")
            .prop("x1", "number", "Start X")
            .prop("y1", "number", "Start Y")
            .prop("x2", "number", "End X")
            .prop("y2", "number", "End Y")
            .prop("color", "string", "Stroke hex color")
            .prop("thickness", "number", "Line thickness (default: 1.0)")
            .required(["x1", "y1", "x2", "y2", "color"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("visual_buffer_add_circle", "Add a circle to the active visual buffer.")
            .prop("cx", "number", "Center X")
            .prop("cy", "number", "Center Y")
            .prop("r", "number", "Radius")
            .prop("fill", "string", "Fill hex color")
            .prop("stroke", "string", "Stroke hex color")
            .required(["cx", "cy", "r"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("visual_buffer_add_text", "Add text to the active visual buffer.")
            .prop("x", "number", "X coordinate")
            .prop("y", "number", "Y coordinate")
            .prop("text", "string", "Text to display")
            .prop("font_size", "number", "Font size")
            .prop("color", "string", "Text hex color")
            .required(["x", "y", "text", "font_size", "color"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("visual_buffer_clear", "Clear all elements from the active visual buffer.")
            .permission(PermissionTier::Write)
            .build(),
        // --- Conversation persistence ---
        ToolDefBuilder::new(
            "ai_save",
            "Save the current AI conversation to a JSON file. Returns the number of entries saved.",
        )
        .prop("path", "string", "File path to save conversation to")
        .required(["path"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "ai_load",
            "Load an AI conversation from a JSON file. Replaces the current conversation.",
        )
        .prop("path", "string", "File path to load conversation from")
        .required(["path"])
        .permission(PermissionTier::Write)
        .build(),
        // --- File management ---
        ToolDefBuilder::new(
            "rename_file",
            "Rename the current buffer's file on disk and update the buffer path.",
        )
        .prop("new_path", "string", "New file path")
        .required(["new_path"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Performance tools ---
        ToolDefBuilder::new(
            "perf_stats",
            "Get current editor performance statistics: RSS memory, CPU usage, frame timing, buffer count, total lines.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "perf_benchmark",
            "Run a micro-benchmark and return timing results. Types: 'buffer_insert' (insert N lines), 'buffer_delete' (delete N lines), 'syntax_parse' (parse N-line Rust source), 'scroll_stress' (scroll N times, returns latency stats), 'kb_search_stress' (search N-node KB, returns p50/p95), 'kb_graph_stress' (BFS depth-2 on N-node graph, returns latency stats).",
        )
        .prop_enum(
            "benchmark",
            "string",
            "Benchmark type",
            [
                "buffer_insert",
                "buffer_delete",
                "syntax_parse",
                "scroll_stress",
                "kb_search_stress",
                "kb_graph_stress",
            ],
        )
        .prop("size", "integer", "Number of lines/items for the benchmark (default: 1000)")
        .required(["benchmark"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "perf_profile",
            "Frame-level profiling session. Actions: 'start' (begin recording frames), 'stop' (stop recording), 'report' (analyze recorded frames: timing stats, redraw level distribution, cache hit rates, slow frames, auto-diagnosis).",
        )
        .prop_enum("action", "string", "Action to perform", ["start", "stop", "report"])
        .required(["action"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Introspection: theme, mouse, render ---
        ToolDefBuilder::new(
            "theme_inspect",
            "Look up a resolved theme style by semantic key (e.g. 'conversation.user.text', 'ui.statusline'). Returns JSON with fg, bg, bold, italic, dim, underline.",
        )
        .prop("key", "string", "Theme style key (dot-namespaced, e.g. 'ui.statusline')")
        .required(["key"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new("mouse_event", "Simulate a mouse event (scroll or click) in the editor.")
            .prop_enum("event_type", "string", "Type of mouse event", ["scroll", "click"])
            .prop("row", "integer", "Screen row for click events")
            .prop("col", "integer", "Screen column for click events")
            .prop(
                "delta",
                "integer",
                "Scroll delta (positive=up, negative=down) for scroll events",
            )
            .prop_enum("button", "string", "Mouse button for click events", ["left", "right", "middle"])
            .required(["event_type"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new(
            "render_inspect",
            "Inspect what is rendered at a given screen position. Returns the buffer name, buffer kind, and theme colors at that cell.",
        )
        .prop("row", "integer", "Screen row to inspect")
        .prop("col", "integer", "Screen column to inspect")
        .required(["row", "col"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "introspect",
            "Comprehensive diagnostic introspection of MAE's internal state. Returns structured JSON covering threads, performance, locks, buffers, shell, AI state, and per-frame render profiling.",
        )
        .prop_enum(
            "section",
            "string",
            "Section to inspect: 'all', 'threads', 'locks', 'perf', 'buffers', 'shell', 'ai', 'frame' (per-frame render profiling with phase timing and cache stats)",
            ["all", "threads", "locks", "perf", "buffers", "shell", "ai", "frame"],
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "event_recording",
            "Control input event recording for debugging. Actions: start, stop, status, dump.",
        )
        .prop_enum(
            "action",
            "string",
            "Action to perform: 'start', 'stop', 'status', 'dump'",
            ["start", "stop", "status", "dump"],
        )
        .prop("last_n", "integer", "Number of recent events to return (for 'dump' action, default 50)")
        .required(["action"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- State stack tools ---
        ToolDefBuilder::new(
            "editor_save_state",
            "Save current editor state (buffer list, window layout, focus, mode) onto a stack. Call before temporary operations like self-test to enable clean restore later.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "editor_restore_state",
            "Restore editor state from the stack: closes buffers opened since the save, restores window layout, focus, and mode. Inverse of editor_save_state.",
        )
        .permission(PermissionTier::Write)
        .build(),
        // --- Scheme evaluation ---
        ToolDefBuilder::new(
            "eval_scheme",
            "Evaluate a Scheme expression in the editor's embedded runtime. Returns the result or error. To dispatch editor commands from Scheme use (run-command \"name\") — NOT (command ...). NOTE: Scheme (load) does NOT expand ~ — use absolute paths from audit_configuration. For running editor commands, prefer calling command_<name> tools directly instead of eval_scheme. Examples: '(+ 3 4)', '(buffer-name)', '(run-command \"reload-config\")'.",
        )
        .prop("code", "string", "Scheme expression to evaluate")
        .required(["code"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Configuration audit ---
        ToolDefBuilder::new(
            "audit_configuration",
            "Audit the editor configuration and return a structured JSON report. Includes AI agent/chat status, an agent-shell startup-file environment diff (what .bashrc/.zshrc would add/change for the open-ai-agent shell vs. this process's ambient env — diagnoses \"my agent shell is missing an env var/token my normal terminal has\"), LSP servers, DAP adapters, init files (with absolute paths), modified options, prompt tier, and actionable issues. Call FIRST when diagnosing config problems or when you need absolute paths to config files.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- UI ---
        ToolDefBuilder::new(
            "toggle_file_tree",
            "Toggle the file tree sidebar. Opens a project directory browser on the left side of the editor, or closes it if already open. Use this to browse the project structure.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Image tools ---
        ToolDefBuilder::new(
            "image_info",
            "Read image metadata: dimensions, format, file size, EXIF data (camera, date, GPS, exposure). Path supports ~.",
        )
        .prop("path", "string", "Path to the image file")
        .required(["path"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "image_list",
            "List all image links in the current buffer with resolved paths, dimensions, and display attributes (#+attr width).",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Module tools ---
        ToolDefBuilder::new(
            "list_modules",
            "List all active modules with full details (name, version, status, category, description, commands, options, flags, path). MAE has a Doom-style module system — use this to discover available modules.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "pkg_sync",
            "Synchronize packages — clone missing modules and update lockfile. Equivalent to `mae pkg sync`. Requires restart to apply.",
        )
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "pkg_upgrade",
            "Upgrade all packages to latest versions. Equivalent to `mae pkg upgrade`. Requires restart to apply.",
        )
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "pkg_doctor",
            "Run package health checks — verify lockfile integrity, detect missing modules, check for version conflicts. Equivalent to `mae pkg doctor`.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Format/Build/Spell/Lookup tools ---
        ToolDefBuilder::new(
            "format_buffer",
            "Format the current buffer using the configured external formatter or LSP.",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "run_build",
            "Detect the project build system (Cargo, Make, npm, etc.) and run the build command. Parse compiler errors for navigation.",
        )
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new("run_test", "Detect the project build system and run the test command.")
            .permission(PermissionTier::Shell)
            .build(),
        ToolDefBuilder::new("spell_check", "Run spell check on the current buffer using aspell or hunspell.")
            .permission(PermissionTier::ReadOnly)
            .build(),
        ToolDefBuilder::new(
            "lookup_online",
            "Look up documentation URL for the word at cursor (docs.rs, MDN, devdocs.io).",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new("next_error", "Navigate to the next build error after running a build command.")
            .permission(PermissionTier::ReadOnly)
            .build(),
        ToolDefBuilder::new(
            "convert_buffer",
            "Convert the current buffer between Org and Markdown formats in-place.",
        )
        .prop_enum(
            "target_format",
            "string",
            "Target format: 'org' (markdown→org) or 'markdown' (org→markdown)",
            ["org", "markdown"],
        )
        .required(["target_format"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Tool Discovery ---
        ToolDefBuilder::new(
            "search_tools",
            "Fuzzy search over all available tools by name or description. Use this to discover tools when you don't know the exact name. Example: search for 'breakpoint' to find dap_set_breakpoint.",
        )
        .prop(
            "query",
            "string",
            "Natural language search query (e.g. 'set breakpoint', 'find references')",
        )
        .prop("limit", "integer", "Max results to return (default: 10)")
        .required(["query"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Model Validation ---
        ToolDefBuilder::new(
            "model_exam",
            "Deprecated: use self_test_suite instead. Model validation exam \u{2014} deterministic known-answer tests that grade tool selection, parameter accuracy, output interpretation, and safety pushback. Actions: 'plan' returns the exam JSON (via self_test_suite), 'grade' accepts results and returns ExamResult with verdict.",
        )
        .prop_enum(
            "action",
            "string",
            "Action: 'plan' to get exam tests, 'grade' to grade results",
            ["plan", "grade"],
        )
        .prop(
            "results",
            "array",
            "Array of {test_id, tool_calls: [{name, arguments}], final_text} for grading",
        )
        .prop("model", "string", "Model name for the grade report")
        .required(["action"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Keymap query ---
        ToolDefBuilder::new(
            "keymap_query",
            "Query keybindings across all keymaps. Filter by keymap name, command substring, or key prefix.",
        )
        .prop("keymap", "string", "Filter to a specific keymap (e.g. 'normal', 'visual', 'insert')")
        .prop("command", "string", "Substring filter on command names (e.g. 'daily', 'kb-')")
        .prop(
            "prefix",
            "string",
            "Key prefix filter (e.g. 'SPC n d' returns all bindings under that prefix)",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
    ]
}
