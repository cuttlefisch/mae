use std::collections::HashMap;

use mae_core::{CommandRegistry, OptionRegistry};

use crate::types::*;

/// Generate tool definitions from the CommandRegistry.
/// Every command (builtin or Scheme) becomes a callable AI tool.
///
/// Tool names are prefixed with `command_` and hyphens replaced with underscores
/// to satisfy all LLM provider naming constraints (alphanumeric + underscore only).
pub fn tools_from_registry(registry: &CommandRegistry) -> Vec<ToolDefinition> {
    registry
        .list_commands()
        .iter()
        .map(|cmd| {
            let tool_name = format!("command_{}", cmd.name.replace('-', "_"));
            ToolDefinition {
                name: tool_name,
                description: cmd.doc.clone(),
                parameters: ToolParameters {
                    schema_type: "object".into(),
                    properties: HashMap::new(),
                    required: vec![],
                },
                permission: Some(classify_command_permission(&cmd.name)),
            }
        })
        .collect()
}

/// AI-specific tools that provide richer access than simple command dispatch.
/// These give the AI structured read/write access to buffers, files, and shell.
pub fn ai_specific_tools(registry: &OptionRegistry) -> Vec<ToolDefinition> {
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
        ToolDefinition {
            name: "shell_exec".into(),
            description: "Execute a shell command and return stdout/stderr.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "command".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Shell command to execute".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "timeout_ms".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Timeout in milliseconds (default: 30000)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["command".into()],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "file_read".into(),
            description: "Read a file from disk. Returns contents with line numbers.".into(),
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
            name: "command_list".into(),
            description: "List all available editor commands with their documentation and sources (builtin/scheme).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
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
            name: "lsp_definition".into(),
            description: "Go to definition of the symbol at the given position. Returns JSON with locations (uri, line, character). Requires an LSP server for the file's language. Positions are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number (default: cursor line)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "character".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed column (default: cursor column)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer to query (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_references".into(),
            description: "Find all references to the symbol at the given position. Returns JSON array of locations (uri, line, character). Requires an LSP server. Positions are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number (default: cursor line)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "character".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed column (default: cursor column)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer to query (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_hover".into(),
            description: "Get hover information (type signature, documentation) for the symbol at the given position. Returns the hover text as a string. Requires an LSP server. Positions are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number (default: cursor line)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "character".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed column (default: cursor column)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer to query (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_diagnostics".into(),
            description: "Read diagnostics (errors, warnings, hints) reported by language servers. Returns JSON with per-file diagnostics plus global severity counts. Use scope='all' to include every file, scope='buffer' (default) for just the active buffer. Positions are 1-indexed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "scope".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "'buffer' (active buffer only, default) or 'all' (every file).".into(),
                            enum_values: Some(vec!["buffer".into(), "all".into()]),
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Override the active buffer when scope='buffer'.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_workspace_symbol".into(),
            description: "Search for symbols across the workspace by name. Returns JSON array of {name, kind, path, line, character, container_name}. Requires an LSP server. Use this to find functions, structs, types, etc. by name without knowing which file they're in.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "query".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Symbol name or prefix to search for".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "language_id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Language server to query (e.g. 'rust', 'python', 'typescript'). Required because workspace/symbol is server-specific.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["query".into(), "language_id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "lsp_document_symbols".into(),
            description: "List all symbols (functions, structs, methods, etc.) in a document. Returns a hierarchical JSON tree of {name, kind, line, end_line, detail, children}. Requires an LSP server. Use this to understand file structure without reading all the code.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Buffer to query (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "syntax_tree".into(),
            description: "Return the tree-sitter syntax tree for a buffer. Useful for understanding code structure before editing. `scope='buffer'` returns the full root S-expression; `scope='cursor'` returns only the named-node kind at the current cursor position.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "scope".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "'buffer' (default) returns the full tree; 'cursor' returns the node at the cursor.".into(),
                            enum_values: Some(vec!["buffer".into(), "cursor".into()]),
                        },
                    ),
                    (
                        "buffer_name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Override the active buffer by name.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
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
        ToolDefinition {
            name: "dap_start".into(),
            description: "Start a debug session against a program using an adapter preset. Pair with `dap_set_breakpoint` and `dap_continue`/`dap_step` to drive execution. Use `debug_state` to see threads/frames/variables.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "adapter".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Adapter preset: 'lldb' (C/C++/Rust), 'debugpy' (Python), 'codelldb' (C/C++/Rust alt)".into(),
                            enum_values: Some(vec!["lldb".into(), "debugpy".into(), "codelldb".into()]),
                        },
                    ),
                    (
                        "program".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Path to the binary or script to debug".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "args".into(),
                        ToolProperty {
                            prop_type: "array".into(),
                            description: "Program arguments (optional)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "mode".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "'launch' (default) to start a new process, 'attach' to connect to an existing process by pid".into(),
                            enum_values: Some(vec!["launch".into(), "attach".into()]),
                        },
                    ),
                    (
                        "pid".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Process ID to attach to (required when mode='attach')".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["adapter".into()],
            },
            // Privileged because launching arbitrary programs under a
            // debug adapter is roughly equivalent to shell exec.
            permission: Some(PermissionTier::Privileged),
        },
        ToolDefinition {
            name: "dap_set_breakpoint".into(),
            description: "Set a breakpoint at source:line. Idempotent — no-op if already set. Works before or during a session; pending breakpoints are synced to the adapter on session start. Lines are 1-indexed. Supports optional condition and hit_condition for conditional breakpoints.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "source".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Source file path (matches the adapter's view — typically the same path the debugger sees)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "condition".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional condition expression — adapter breaks only when this evaluates to true".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "hit_condition".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional hit condition (e.g. '>= 5') — adapter breaks only after this many hits".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["source".into(), "line".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_continue".into(),
            description: "Resume execution on the active thread. Errors if no debug session is active.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_step".into(),
            description: "Step execution on the active thread. `direction`: 'over' (next line, skip calls), 'in' (step into calls), 'out' (step out of current frame). Errors if no session is active.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "direction".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "'over', 'in', or 'out'".into(),
                        enum_values: Some(vec!["over".into(), "in".into(), "out".into()]),
                    },
                )]),
                required: vec!["direction".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_inspect_variable".into(),
            description: "Look up a single variable by name in the stopped frame's scopes. Returns JSON with name/value/type/scope/variables_reference. Use `debug_state` for the full variable tree. `variables_reference` > 0 means expandable children.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Variable name to find".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "scope".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional scope name to restrict search (e.g. 'Locals', 'Globals'). Default: all scopes.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["name".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "dap_remove_breakpoint".into(),
            description: "Remove a breakpoint at source:line. Returns remaining lines for that source.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "source".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Source file path".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "line".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "1-indexed line number of breakpoint to remove".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["source".into(), "line".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_list_variables".into(),
            description: "List all variables in the current frame's scopes. Returns JSON mapping scope names to variable arrays with name/value/type/variables_reference.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "dap_expand_variable".into(),
            description: "Request children of a nested variable. Queues a DAP request; call debug_state or dap_list_variables after to see results.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "variables_reference".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "The parent variable's variables_reference (must be > 0)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "scope".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Scope name for the request (e.g. 'Locals')".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["variables_reference".into(), "scope".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_select_frame".into(),
            description: "Switch to a different stack frame by id. Queues a scopes request for the new frame.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "frame_id".into(),
                    ToolProperty {
                        prop_type: "integer".into(),
                        description: "Stack frame id to select".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["frame_id".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_select_thread".into(),
            description: "Switch the active thread. Triggers a stack trace refresh for the new thread.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "thread_id".into(),
                    ToolProperty {
                        prop_type: "integer".into(),
                        description: "Thread id to switch to".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["thread_id".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_output".into(),
            description: "Read recent debug output log lines. Returns JSON with output array and total line count.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "lines".into(),
                    ToolProperty {
                        prop_type: "integer".into(),
                        description: "Number of recent lines to return (default 50)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "dap_evaluate".into(),
            description: "Evaluate an expression in the debuggee's context. Result arrives asynchronously — call `dap_output` or `debug_state` after to see it. The result is also shown in the status bar and appended to the debug output log.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "expression".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Expression to evaluate in the debuggee".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "frame_id".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Optional stack frame id for evaluation context (default: topmost frame)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "context".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Evaluation context: 'watch', 'repl', or 'hover' (default: 'repl')".into(),
                            enum_values: Some(vec!["watch".into(), "repl".into(), "hover".into()]),
                        },
                    ),
                ]),
                required: vec!["expression".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_disconnect".into(),
            description: "Disconnect from the debug adapter. Optionally terminate the debuggee process.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "terminate_debuggee".into(),
                    ToolProperty {
                        prop_type: "boolean".into(),
                        description: "If true, also terminate the debugged process (default: false — detach only)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        // ---- Knowledge base (shared with :help) ----
        //
        // The KB is the source of truth for command/concept/key
        // documentation. The same nodes the human reads via `:help`
        // are queryable here — the agent is a peer reader.
        ToolDefinition {
            name: "kb_get".into(),
            description: "Fetch a knowledge-base node by id. Returns JSON with title, kind, body (may contain [[link]] markers), tags, outgoing links, and incoming backlinks. IDs are namespaced like 'cmd:<name>', 'concept:<slug>', 'key:<context>', or 'index'.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Node id, e.g. 'index', 'concept:buffer', 'cmd:save'".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_search".into(),
            description: "Case-insensitive substring search over KB node titles, ids, bodies, and tags. Returns ids in relevance order (title/id matches before body matches). Empty query returns all ids.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "query".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Substring to search for (case-insensitive)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_list".into(),
            description: "List all KB node ids, sorted. Optional `prefix` filters to a namespace (e.g. prefix='cmd:' returns all command docs).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "prefix".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Optional namespace prefix, e.g. 'cmd:', 'concept:'".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_links_from".into(),
            description: "Outgoing links from a node — the targets of its body's [[link]] markers, in document order (deduplicated). Errors if the node doesn't exist.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Source node id".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_links_to".into(),
            description: "Incoming links — ids of all KB nodes whose body references this target. Works even if the target node doesn't exist yet (dangling backlinks).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Target node id (may be dangling)".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "kb_graph".into(),
            description: "BFS neighborhood around a seed node up to `depth` hops (default 1, max 3). Returns {root, depth, nodes: [{id, title, kind, hop, missing?}], edges: [{src, dst}]}. Use this to orient yourself in the KB before navigating — the local graph tells you which related topics the user might want to explore next.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "id".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Seed node id".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "depth".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Hop radius (default 1, clamped to 3)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "help_open".into(),
            description: "Open the *Help* buffer focused on a KB node. The human sees the same content via `:help <node>`. Use this when the user asks about a topic that has a help page — they'll see it in a buffer and can navigate with Tab / Enter / Alt-Left / Alt-Right. Falls back to the `index` node if the id isn't found.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "id".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Node id to open, e.g. 'index', 'concept:buffer', 'cmd:save'".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["id".into()],
            },
            permission: Some(PermissionTier::Write),
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
        // --- Shell terminal tools ---
        ToolDefinition {
            name: "shell_list".into(),
            description: "List all active shell terminal buffers with their names, buffer indices, and status (running/exited).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "shell_read_output".into(),
            description: "Read recent output from a shell terminal buffer's viewport. Returns the last N lines of visible terminal content.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "buffer_index".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Buffer index of the shell terminal".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "lines".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Number of lines to read (default: 24)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["buffer_index".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "shell_send_input".into(),
            description: "Send text input to a shell terminal buffer's PTY. Escape sequences: \\n or \\r for Enter, \\t for Tab, \\e for ESC.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "buffer_index".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Buffer index of the shell terminal".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "input".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Text to send to the terminal. Escapes: \\n/\\r=Enter, \\t=Tab, \\e=ESC".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["buffer_index".into(), "input".into()],
            },
            permission: Some(PermissionTier::Shell),
        },
        // --- Permission introspection ---
        ToolDefinition {
            name: "ai_permissions".into(),
            description: "Show the current AI permission tier and what each tier allows. Returns the auto-approved tier, available tiers with descriptions, and agent trust configuration status.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Self-test suite ---
        ToolDefinition {
            name: "self_test_suite".into(),
            description: "Get the structured self-test plan for MAE's AI tool surface. Returns a JSON object with test categories, each containing an array of tests specifying: tool to call, arguments, assertion to check, and PASS/FAIL/SKIP criteria. Use this to validate that all editor tools work end-to-end. No arguments needed.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "categories".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Comma-separated list of categories to include (default: all). Options: introspection, editing, help, project, lsp, dap, performance".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        // --- Input lock ---
        ToolDefinition {
            name: "input_lock".into(),
            description: "Lock or unlock editor keyboard input. When locked, all user keystrokes are discarded except Esc/Ctrl-C (which cancel and unlock). Use this before running multi-step operations (like self-tests) to prevent user input from interfering with editor state, and unlock when done.".into(),
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
            description: "Run a micro-benchmark and return timing results. Types: 'buffer_insert' (insert N lines), 'buffer_delete' (delete N lines), 'syntax_parse' (parse N-line Rust source).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "benchmark".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Benchmark type".into(),
                            enum_values: Some(vec!["buffer_insert".into(), "buffer_delete".into(), "syntax_parse".into()]),
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
        // --- Introspection: theme, shell scrollback, mouse, render ---
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
            name: "shell_scrollback".into(),
            description: "Read text lines from a shell terminal's scrollback/viewport. Returns the cached viewport text for the given buffer.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "buffer_index".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Buffer index of the shell terminal (default: active buffer)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "offset".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Lines from the bottom to start reading (default: 0)".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "lines".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Number of lines to return (default: 50)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
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
            description: "Comprehensive diagnostic introspection of MAE's internal state. Returns structured JSON covering threads, performance, locks, buffers, shell, and AI state.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: {
                    let mut p = HashMap::new();
                    p.insert(
                        "section".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Section to inspect: 'all', 'threads', 'locks', 'perf', 'buffers', 'shell', 'ai'".into(),
                            enum_values: Some(vec![
                                "all".into(),
                                "threads".into(),
                                "locks".into(),
                                "perf".into(),
                                "buffers".into(),
                                "shell".into(),
                                "ai".into(),
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
    ]
}

/// Classify a command's permission tier based on its name.
pub fn classify_command_permission(name: &str) -> PermissionTier {
    match name {
        // Movement and read-only state changes
        n if n.starts_with("move-") => PermissionTier::ReadOnly,
        "enter-normal-mode"
        | "enter-insert-mode"
        | "enter-command-mode"
        | "enter-insert-mode-after"
        | "enter-insert-mode-eol" => PermissionTier::ReadOnly,

        // Editing commands
        n if n.starts_with("delete-") || n.starts_with("open-line-") => PermissionTier::Write,
        "undo" | "redo" => PermissionTier::Write,
        "save" | "save-and-quit" => PermissionTier::Write,

        // Dangerous operations
        "quit" | "force-quit" => PermissionTier::Privileged,

        // Default to Write for unknown commands
        _ => PermissionTier::Write,
    }
}

/// Policy for auto-approving or prompting for tool calls.
#[derive(Debug, Clone)]
pub struct PermissionPolicy {
    /// Maximum tier that is auto-approved without user confirmation.
    pub auto_approve_up_to: PermissionTier,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        // Container-first: auto-approve up to Shell tier.
        PermissionPolicy {
            auto_approve_up_to: PermissionTier::Shell,
        }
    }
}

impl PermissionPolicy {
    /// Check if a permission tier is auto-approved.
    pub fn is_allowed(&self, tier: PermissionTier) -> bool {
        tier <= self.auto_approve_up_to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_from_registry_empty() {
        let reg = CommandRegistry::new();
        let tools = tools_from_registry(&reg);
        assert!(tools.is_empty());
    }

    #[test]
    fn tools_from_registry_generates_correct_count() {
        let reg = CommandRegistry::with_builtins();
        let tools = tools_from_registry(&reg);
        assert_eq!(tools.len(), reg.len());
    }

    #[test]
    fn tools_from_registry_name_format() {
        let reg = CommandRegistry::with_builtins();
        let tools = tools_from_registry(&reg);
        let move_down = tools.iter().find(|t| t.name == "command_move_down");
        assert!(move_down.is_some(), "should have command_move_down");
        // All names should match [a-z_]+
        for tool in &tools {
            assert!(
                tool.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_'),
                "bad tool name: {}",
                tool.name
            );
        }
    }

    #[test]
    fn tools_from_registry_preserves_docs() {
        let reg = CommandRegistry::with_builtins();
        let tools = tools_from_registry(&reg);
        let undo = tools.iter().find(|t| t.name == "command_undo").unwrap();
        assert!(!undo.description.is_empty());
    }

    #[test]
    fn ai_specific_tools_count() {
        let tools = ai_specific_tools(&OptionRegistry::new());
        assert_eq!(tools.len(), 64);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"buffer_read"));
        assert!(names.contains(&"buffer_write"));
        assert!(names.contains(&"get_option"));
        assert!(names.contains(&"set_option"));
        assert!(names.contains(&"cursor_info"));
        assert!(names.contains(&"shell_exec"));
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"list_buffers"));
        assert!(names.contains(&"editor_state"));
        assert!(names.contains(&"window_layout"));
        assert!(names.contains(&"command_list"));
        assert!(names.contains(&"debug_state"));
        assert!(names.contains(&"open_file"));
        assert!(names.contains(&"switch_buffer"));
        assert!(names.contains(&"close_buffer"));
        assert!(names.contains(&"create_file"));
        assert!(names.contains(&"project_files"));
        assert!(names.contains(&"project_info"));
        assert!(names.contains(&"project_search"));
        assert!(names.contains(&"lsp_definition"));
        assert!(names.contains(&"lsp_references"));
        assert!(names.contains(&"lsp_hover"));
        assert!(names.contains(&"lsp_diagnostics"));
        assert!(names.contains(&"syntax_tree"));
        assert!(names.contains(&"dap_start"));
        assert!(names.contains(&"dap_set_breakpoint"));
        assert!(names.contains(&"dap_continue"));
        assert!(names.contains(&"dap_step"));
        assert!(names.contains(&"dap_inspect_variable"));
        assert!(names.contains(&"dap_remove_breakpoint"));
        assert!(names.contains(&"dap_list_variables"));
        assert!(names.contains(&"dap_expand_variable"));
        assert!(names.contains(&"dap_select_frame"));
        assert!(names.contains(&"dap_select_thread"));
        assert!(names.contains(&"dap_output"));
        assert!(names.contains(&"dap_evaluate"));
        assert!(names.contains(&"dap_disconnect"));
        assert!(names.contains(&"kb_get"));
        assert!(names.contains(&"kb_search"));
        assert!(names.contains(&"kb_list"));
        assert!(names.contains(&"kb_links_from"));
        assert!(names.contains(&"kb_links_to"));
        assert!(names.contains(&"kb_graph"));
        assert!(names.contains(&"help_open"));
        assert!(names.contains(&"switch_project"));
    }

    #[test]
    fn classify_movement_is_readonly() {
        assert_eq!(
            classify_command_permission("move-up"),
            PermissionTier::ReadOnly
        );
        assert_eq!(
            classify_command_permission("move-down"),
            PermissionTier::ReadOnly
        );
        assert_eq!(
            classify_command_permission("move-to-line-start"),
            PermissionTier::ReadOnly
        );
    }

    #[test]
    fn classify_editing_is_write() {
        assert_eq!(
            classify_command_permission("delete-line"),
            PermissionTier::Write
        );
        assert_eq!(classify_command_permission("undo"), PermissionTier::Write);
        assert_eq!(classify_command_permission("save"), PermissionTier::Write);
    }

    #[test]
    fn classify_quit_is_privileged() {
        assert_eq!(
            classify_command_permission("quit"),
            PermissionTier::Privileged
        );
        assert_eq!(
            classify_command_permission("force-quit"),
            PermissionTier::Privileged
        );
    }

    #[test]
    fn set_option_enum_covers_all_options() {
        let registry = OptionRegistry::new();
        let tools = ai_specific_tools(&registry);
        let set_opt = tools.iter().find(|t| t.name == "set_option").unwrap();
        let enum_values = set_opt.parameters.properties["option"]
            .enum_values
            .as_ref()
            .expect("set_option should have enum_values");
        assert_eq!(
            enum_values.len(),
            registry.list().len(),
            "set_option enum_values must match OptionRegistry count"
        );
        for opt in registry.list() {
            assert!(
                enum_values.contains(&opt.name.to_string()),
                "Missing option '{}' in set_option enum_values",
                opt.name
            );
        }
    }

    #[test]
    fn default_policy_allows_up_to_shell() {
        let policy = PermissionPolicy::default();
        assert!(policy.is_allowed(PermissionTier::ReadOnly));
        assert!(policy.is_allowed(PermissionTier::Write));
        assert!(policy.is_allowed(PermissionTier::Shell));
        assert!(!policy.is_allowed(PermissionTier::Privileged));
    }
}
