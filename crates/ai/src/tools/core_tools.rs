use std::collections::HashMap;

use mae_core::OptionRegistry;

use crate::types::*;

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
        ToolDefinition {
            name: "buffer_read".into(),
            description: "Read buffer contents. Returns text with line numbers.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "start_line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "First line to read (1-indexed, default: 1)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "end_line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Last line to read (inclusive, default: last line)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer name to read (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "buffer_write".into(),
            description: "Replace a range of lines with new content. Use for insert, delete, and replace operations.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "start_line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "First line to replace (1-indexed)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "end_line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Last line to replace (inclusive). Omit to insert before start_line.".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "content".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "New content (empty string to delete lines)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer name to write to (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["start_line".into(), "content".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Cursor & state ---
        ToolDefinition {
            name: "cursor_info".into(),
            description: "Get current cursor position, mode, buffer name, and line count.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- File tools ---
        ToolDefinition {
            name: "file_read".into(),
            description: "Read a file from disk. Returns contents with line numbers. Path supports ~ (home dir). If not found, call audit_configuration for correct paths.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "path".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "File path to read".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["path".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "list_buffers".into(),
            description: "List all open buffers with names, modified status, and file paths.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "editor_state".into(),
            description: "Full JSON snapshot of editor state: mode, theme, buffer count, window count, active buffer, message log size, debug session status.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "read_messages".into(),
            description: "Read the editor's *Messages* log buffer. Shows errors, warnings, and info from all subsystems (DAP, LSP, AI, etc.). Essential for diagnosing command failures — check this when a tool call fails or times out.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "last_n".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Number of recent messages to return (default: 30)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "level".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Minimum severity: error, warn, info, debug, trace (default: info)".into(),
                            enum_values: Some(vec![
                                "error".into(),
                                "warn".into(),
                                "info".into(),
                                "debug".into(),
                                "trace".into(),
                            ]),
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "notifications_list".into(),
            description: "List outstanding + recently-resolved notifications from the attention bus (ADR-024) as JSON: id, severity, source, title, body, actions, resolved. The agent/headless path for seeing what demands attention (e.g. a collab edit fenced and needing resolution).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "notify_resolve".into(),
            description: "Resolve an outstanding notification by id: run one of its actions (by index) or dismiss it. For a collab fenced-edit, action 0 = Accept-remote (discard local), 1 = Keep-mine (re-author), 2 = Stash externally.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Notification id (from notifications_list).".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "action".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Action index to run. Omit to dismiss the notification.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "window_layout".into(),
            description: "JSON of all windows with their buffer assignments and dimensions.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "execute_command".into(),
            description: "Execute a registered editor command by name. Equivalent to typing the command in command mode. Use command_list to discover available commands.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "command".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "The command name to execute (e.g. 'move-to-last-line', 'scroll-down-line')".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["command".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "command_list".into(),
            description: "List all available editor commands. Use format='names' for a compact list of just command names (recommended). Default returns full JSON with docs and sources.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "format".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Output format: 'names' for compact name-only list, 'full' for JSON with docs (default: 'full')".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "debug_state".into(),
            description: "If a debug session is active, returns full JSON of threads, scopes, variables, breakpoints. Otherwise returns 'No active debug session'.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "open_file".into(),
            description: "Open a file from disk into a new buffer and switch to it. If already open, switches to the existing buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "path".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "File path to open".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["path".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "switch_buffer".into(),
            description: "Switch the active buffer by name. Use list_buffers to see available buffers.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Buffer name to switch to".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["name".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "close_buffer".into(),
            description: "Close a buffer by name. Fails if the buffer has unsaved changes unless force=true.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer name to close (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "force".into(),
                        ToolProperty {
                            prop_type: "boolean".into(),
                            description: "If true, close even if the buffer has unsaved changes (default: false)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "create_file".into(),
            description: "Create a new file with content and open it as a buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "path".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "File path to create".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "content".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Initial file content (default: empty)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["path".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Project tools ---
        ToolDefinition {
            name: "project_info".into(),
            description: "Get project state: name, root, config, recent files, and display settings (line numbers, relative line numbers, word wrap).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "project_files".into(),
            description: "List files in the project. Uses git ls-files if in a git repo, otherwise lists files recursively. Returns one path per line.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "pattern".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Optional glob pattern to filter files (e.g. '*.rs', 'src/**/*.toml')".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "project_search".into(),
            description: "Search across project files using a regex pattern. Returns matching lines with file paths and line numbers.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "pattern".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Regex pattern to search for".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "glob".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional file glob to limit search (e.g. '*.rs')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "max_results".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Maximum number of results (default: 100)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["pattern".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Project management ---
        ToolDefinition {
            name: "switch_project".into(),
            description: "Switch the active project to a new root directory. Detects project markers (.git, Cargo.toml, etc.) and updates the editor's project context. Adds the root to recent projects.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "path".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Absolute path to the project root directory".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["path".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Editor settings ---
        ToolDefinition {
            name: "get_option".into(),
            description: "Get current value of an editor option, or list all options. Returns name, current value, type, default, and documentation. Call with no name (or name='all') to list everything. Use before set_option to read current values.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Option name to query, or 'all' to list everything. Omit for all options.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "set_option".into(),
            description: option_desc,
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "option".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Option name".into(),
                            enum_values: Some(option_names),
                        },
                    ),
                    (
                        "value".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "New value for the option".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["option".into(), "value".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Agent UX tools ---
        ToolDefinition {
            name: "ask_user".into(),
            description: "Ask the user a clarifying question when the current instructions are ambiguous or more context is needed. The AI session will pause until the user replies.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "question".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "The question to ask the user.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["question".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "log_activity".into(),
            description: "Log a visible status update or reasoning step to the user's AI buffer. Use this during long operations to keep the user informed of your progress and current focus.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "activity".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Description of the current activity or reasoning step.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["activity".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "propose_changes".into(),
            description: "Propose a set of file changes for user approval. Displays a diff and waits for user to accept or reject. Use this for potentially destructive or large changes to ensure safety.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "changes".into(),
                    ToolProperty {
                        prop_type: "array".into(),
                        description: "List of changes, each with file_path and new_content.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["changes".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Self-test suite (v3: sandbox, grading, exam categories) ---
        ToolDefinition {
            name: "self_test_suite".into(),
            description: "Unified test suite for MAE's AI tool surface. Actions: 'plan' (default) returns a v3 JSON test plan with sandbox paths, deterministic grading specs, and both direct-tool + prompt-based tests. 'grade' accepts results array and returns deterministic ExamResult with verdict. File writes are confined to a sandbox directory during test mode.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "action".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Action: 'plan' to get test plan, 'grade' to grade results (default: plan)".into(),
                            enum_values: Some(vec!["plan".into(), "grade".into()]),
                        },
                    ),
                    (
                        "categories".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Comma-separated categories (default: all). Options: introspection, editing, help, project, lsp, dap, git, performance, scrolling, babel, modules, federation, guidance, tool_selection, parameter_accuracy, output_interpretation, multi_step, pushback".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "results".into(),
                        ToolProperty {
                            prop_type: "array".into(),
                            description: "For 'grade' action: array of {test_id, output, success, grading, tool_calls?, final_text?}".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "model".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "For 'grade' action: model name for the exam result".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Transcript Access ---
        ToolDefinition {
            name: "read_transcript".into(),
            description: "Read the full JSON transcript of the current AI session. This contains the raw provider responses, full tool outputs, and reasoning steps. Use this if you get stuck or need to review your own previous thoughts in detail. No arguments needed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "input_lock".into(),
            description: "Lock or unlock editor keyboard input. When locked, all user keystrokes are discarded except Esc/Ctrl-C (which cancel and unlock). Use this before running multi-step operations (like self-tests) to prevent user input from interfering with editor state. Check current lock status via 'introspect' to avoid redundant calls.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "locked".into(),
                    ToolProperty {
                        prop_type: "boolean".into(),
                        description: "true to lock input, false to unlock".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["locked".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "set_ai_target".into(),
            description: "Set the target buffer/window for subsequent AI tool calls. Avoids changing user focus. Use this before operating on a non-focused buffer (e.g. in split layouts where the MCP shim runs in a terminal window).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Target buffer by name".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "window_id".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Target window by ID (from window_layout)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "clear".into(),
                        ToolProperty {
                            prop_type: "boolean".into(),
                            description: "Clear targeting (revert to focused window)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "trigger_hook".into(),
            description: "Manually fire a lifecycle hook by name. This triggers all Scheme functions registered for that hook point.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "hook_name".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Name of the hook to fire (e.g. 'app-start', 'buffer-open')".into(),
                        enum_values: Some(
                            mae_core::hooks::WELL_KNOWN_HOOKS
                                .iter()
                                .map(|s: &&str| s.to_string())
                                .collect(),
                        ),
                    },
                )]),
                required: vec!["hook_name".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Visual Debugger ---
        ToolDefinition {
            name: "visual_buffer_add_rect".into(),
            description: "Add a rectangle to the active visual buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "x".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "X coordinate".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "y".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Y coordinate".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "w".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Width".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "h".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Height".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "fill".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Fill hex color".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "stroke".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Stroke hex color".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["x".into(), "y".into(), "w".into(), "h".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "visual_buffer_add_line".into(),
            description: "Add a line to the active visual buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "x1".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Start X".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "y1".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Start Y".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "x2".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "End X".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "y2".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "End Y".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "color".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Stroke hex color".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "thickness".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Line thickness (default: 1.0)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["x1".into(), "y1".into(), "x2".into(), "y2".into(), "color".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "visual_buffer_add_circle".into(),
            description: "Add a circle to the active visual buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "cx".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Center X".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "cy".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Center Y".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "r".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Radius".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "fill".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Fill hex color".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "stroke".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Stroke hex color".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["cx".into(), "cy".into(), "r".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "visual_buffer_add_text".into(),
            description: "Add text to the active visual buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "x".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "X coordinate".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "y".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Y coordinate".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "text".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Text to display".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "font_size".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "Font size".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "color".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Text hex color".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["x".into(), "y".into(), "text".into(), "font_size".into(), "color".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "visual_buffer_clear".into(),
            description: "Clear all elements from the active visual buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Conversation persistence ---
        ToolDefinition {
            name: "ai_save".into(),
            description: "Save the current AI conversation to a JSON file. Returns the number of entries saved.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "path".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "File path to save conversation to".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["path".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "ai_load".into(),
            description: "Load an AI conversation from a JSON file. Replaces the current conversation.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "path".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "File path to load conversation from".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["path".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- File management ---
        ToolDefinition {
            name: "rename_file".into(),
            description: "Rename the current buffer's file on disk and update the buffer path.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "new_path".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "New file path".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["new_path".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Performance tools ---
        ToolDefinition {
            name: "perf_stats".into(),
            description: "Get current editor performance statistics: RSS memory, CPU usage, frame timing, buffer count, total lines.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "perf_benchmark".into(),
            description: "Run a micro-benchmark and return timing results. Types: 'buffer_insert' (insert N lines), 'buffer_delete' (delete N lines), 'syntax_parse' (parse N-line Rust source), 'scroll_stress' (scroll N times, returns latency stats), 'kb_search_stress' (search N-node KB, returns p50/p95), 'kb_graph_stress' (BFS depth-2 on N-node graph, returns latency stats).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "benchmark".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Benchmark type".into(),
                            enum_values: Some(vec!["buffer_insert".into(), "buffer_delete".into(), "syntax_parse".into(), "scroll_stress".into(), "kb_search_stress".into(), "kb_graph_stress".into()]),
                        },
                    ),
                    (
                        "size".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Number of lines/items for the benchmark (default: 1000)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["benchmark".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "perf_profile".into(),
            description: "Frame-level profiling session. Actions: 'start' (begin recording frames), 'stop' (stop recording), 'report' (analyze recorded frames: timing stats, redraw level distribution, cache hit rates, slow frames, auto-diagnosis).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "action".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Action to perform".into(),
                        enum_values: Some(vec![
                            "start".into(),
                            "stop".into(),
                            "report".into(),
                        ]),
                    },
                )]),
                required: vec!["action".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Introspection: theme, mouse, render ---
        ToolDefinition {
            name: "theme_inspect".into(),
            description: "Look up a resolved theme style by semantic key (e.g. 'conversation.user.text', 'ui.statusline'). Returns JSON with fg, bg, bold, italic, dim, underline.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "key".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Theme style key (dot-namespaced, e.g. 'ui.statusline')".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["key".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "mouse_event".into(),
            description: "Simulate a mouse event (scroll or click) in the editor.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "event_type".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Type of mouse event".into(),
                            enum_values: Some(vec!["scroll".into(), "click".into()]),
                        },
                    ),
                    (
                        "row".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Screen row for click events".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "col".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Screen column for click events".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "delta".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Scroll delta (positive=up, negative=down) for scroll events".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "button".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Mouse button for click events".into(),
                            enum_values: Some(vec!["left".into(), "right".into(), "middle".into()]),
                        },
                    ),
                ]),
                required: vec!["event_type".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "render_inspect".into(),
            description: "Inspect what is rendered at a given screen position. Returns the buffer name, buffer kind, and theme colors at that cell.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "row".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Screen row to inspect".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "col".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Screen column to inspect".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["row".into(), "col".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "introspect".into(),
            description: "Comprehensive diagnostic introspection of MAE's internal state. Returns structured JSON covering threads, performance, locks, buffers, shell, AI state, and per-frame render profiling.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: {
                    let mut p = HashMap::new();
                    p.insert(
                        "section".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Section to inspect: 'all', 'threads', 'locks', 'perf', 'buffers', 'shell', 'ai', 'frame' (per-frame render profiling with phase timing and cache stats)".into(),
                            enum_values: Some(vec![
                                "all".into(),
                                "threads".into(),
                                "locks".into(),
                                "perf".into(),
                                "buffers".into(),
                                "shell".into(),
                                "ai".into(),
                                "frame".into(),
                            ]),
                        },
                    );
                    p
                },
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "event_recording".into(),
            description: "Control input event recording for debugging. Actions: start, stop, status, dump.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: {
                    let mut p = HashMap::new();
                    p.insert(
                        "action".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Action to perform: 'start', 'stop', 'status', 'dump'"
                                .into(),
                            enum_values: Some(vec![
                                "start".into(),
                                "stop".into(),
                                "status".into(),
                                "dump".into(),
                            ]),
                        },
                    );
                    p.insert(
                        "last_n".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description:
                                "Number of recent events to return (for 'dump' action, default 50)"
                                    .into(),
                            enum_values: None,
                        },
                    );
                    p
                },
                required: vec!["action".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- State stack tools ---
        ToolDefinition {
            name: "editor_save_state".into(),
            description: "Save current editor state (buffer list, window layout, focus, mode) onto a stack. Call before temporary operations like self-test to enable clean restore later.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "editor_restore_state".into(),
            description: "Restore editor state from the stack: closes buffers opened since the save, restores window layout, focus, and mode. Inverse of editor_save_state.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Scheme evaluation ---
        ToolDefinition {
            name: "eval_scheme".into(),
            description: "Evaluate a Scheme expression in the editor's embedded runtime. Returns the result or error. To dispatch editor commands from Scheme use (run-command \"name\") — NOT (command ...). NOTE: Scheme (load) does NOT expand ~ — use absolute paths from audit_configuration. For running editor commands, prefer calling command_<name> tools directly instead of eval_scheme. Examples: '(+ 3 4)', '(buffer-name)', '(run-command \"reload-config\")'.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "code".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Scheme expression to evaluate".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["code".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Configuration audit ---
        ToolDefinition {
            name: "audit_configuration".into(),
            description: "Audit the editor configuration and return a structured JSON report. Includes AI agent/chat status, LSP servers, DAP adapters, init files (with absolute paths), modified options, prompt tier, and actionable issues. Call FIRST when diagnosing config problems or when you need absolute paths to config files.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- UI ---
        ToolDefinition {
            name: "toggle_file_tree".into(),
            description: "Toggle the file tree sidebar. Opens a project directory browser on the left side of the editor, or closes it if already open. Use this to browse the project structure.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Image tools ---
        ToolDefinition {
            name: "image_info".into(),
            description: "Read image metadata: dimensions, format, file size, EXIF data (camera, date, GPS, exposure). Path supports ~.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "path".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Path to the image file".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["path".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "image_list".into(),
            description: "List all image links in the current buffer with resolved paths, dimensions, and display attributes (#+attr width).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Module tools ---
        ToolDefinition {
            name: "list_modules".into(),
            description: "List all active modules with full details (name, version, status, category, description, commands, options, flags, path). MAE has a Doom-style module system — use this to discover available modules.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "pkg_sync".into(),
            description: "Synchronize packages — clone missing modules and update lockfile. Equivalent to `mae pkg sync`. Requires restart to apply.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "pkg_upgrade".into(),
            description: "Upgrade all packages to latest versions. Equivalent to `mae pkg upgrade`. Requires restart to apply.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "pkg_doctor".into(),
            description: "Run package health checks — verify lockfile integrity, detect missing modules, check for version conflicts. Equivalent to `mae pkg doctor`.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Format/Build/Spell/Lookup tools ---
        ToolDefinition {
            name: "format_buffer".into(),
            description: "Format the current buffer using the configured external formatter or LSP.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "run_build".into(),
            description: "Detect the project build system (Cargo, Make, npm, etc.) and run the build command. Parse compiler errors for navigation.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "run_test".into(),
            description: "Detect the project build system and run the test command.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "spell_check".into(),
            description: "Run spell check on the current buffer using aspell or hunspell.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lookup_online".into(),
            description: "Look up documentation URL for the word at cursor (docs.rs, MDN, devdocs.io).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "next_error".into(),
            description: "Navigate to the next build error after running a build command.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "convert_buffer".into(),
            description: "Convert the current buffer between Org and Markdown formats in-place."
                .into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "target_format".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Target format: 'org' (markdown→org) or 'markdown' (org→markdown)".into(),
                        enum_values: Some(vec!["org".into(), "markdown".into()]),
                    },
                )]),
                required: vec!["target_format".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Tool Discovery ---
        ToolDefinition {
            name: "search_tools".into(),
            description: "Fuzzy search over all available tools by name or description. Use this to discover tools when you don't know the exact name. Example: search for 'breakpoint' to find dap_set_breakpoint.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "query".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Natural language search query (e.g. 'set breakpoint', 'find references')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "limit".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Max results to return (default: 10)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["query".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Model Validation ---
        ToolDefinition {
            name: "model_exam".into(),
            description: "Deprecated: use self_test_suite instead. Model validation exam \u{2014} deterministic known-answer tests that grade tool selection, parameter accuracy, output interpretation, and safety pushback. Actions: 'plan' returns the exam JSON (via self_test_suite), 'grade' accepts results and returns ExamResult with verdict.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "action".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Action: 'plan' to get exam tests, 'grade' to grade results".into(),
                            enum_values: Some(vec!["plan".into(), "grade".into()]),
                        },
                    ),
                    (
                        "results".into(),
                        ToolProperty {
                            prop_type: "array".into(),
                            description: "Array of {test_id, tool_calls: [{name, arguments}], final_text} for grading".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "model".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Model name for the grade report".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["action".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Keymap query ---
        ToolDefinition {
            name: "keymap_query".into(),
            description: "Query keybindings across all keymaps. Filter by keymap name, command substring, or key prefix.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "keymap".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Filter to a specific keymap (e.g. 'normal', 'visual', 'insert')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "command".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Substring filter on command names (e.g. 'daily', 'kb-')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "prefix".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Key prefix filter (e.g. 'SPC n d' returns all bindings under that prefix)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
    ]
}
