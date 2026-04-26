use std::collections::HashMap;

use crate::types::*;

/// DAP tool definitions: start, breakpoints, stepping, variables, evaluation.
pub(super) fn dap_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "dap_start".into(),
            description: "Start a debug session. Blocks until the session is ready and the debuggee stops (if stop_on_entry=true) or starts running. Returns JSON with session state including threads and stack frames. Requires: adapter binary on PATH.".into(),
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
                    (
                        "stop_on_entry".into(),
                        ToolProperty {
                            prop_type: "boolean".into(),
                            description: "Pause at program entry point before running (default: false). Use true when you need to set breakpoints before the program starts.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["adapter".into()],
            },
            // Shell tier because launching programs under a
            // debug adapter is roughly equivalent to shell exec.
            permission: Some(PermissionTier::Shell),
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
                            description: "1-indexed line number for the breakpoint".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "condition".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional condition expression — breakpoint only triggers when this evaluates to true (e.g. 'x > 5')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "hit_condition".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional hit count condition — breakpoint triggers on the Nth hit (e.g. '3' or '>= 10')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "log_message".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional log message (logpoint) — instead of stopping, logs a message. Expressions in {braces} are evaluated.".into(),
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
            description: "Resume execution. Blocks until the debuggee stops (breakpoint, exception) or terminates. Returns JSON with stopped state including thread, frame, and location. No need to call debug_state after this. Requires: active debug session.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "dap_step".into(),
            description: "Step execution. Blocks until the step completes and debuggee stops. Returns JSON with new stopped state including thread, frame, and location. `direction`: 'over' (next line), 'in' (step into calls), 'out' (step out of frame). Requires: active debug session, stopped state.".into(),
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
            description: "Look up a single variable by name in the stopped frame's scopes. Returns JSON with name/value/type/scope/variables_reference. `variables_reference` > 0 means expandable children. Requires: active debug session, stopped state.".into(),
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
            description: "Remove a breakpoint at source:line. Returns remaining lines for that source. Requires: active debug session.".into(),
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
            description: "List all variables in the current frame's scopes. Returns JSON mapping scope names to variable arrays with name/value/type/variables_reference. Requires: active debug session, stopped state.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "dap_expand_variable".into(),
            description: "Request children of a nested variable by variables_reference. Call dap_list_variables after to see expanded results. Requires: active debug session, stopped state.".into(),
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
            description: "Switch to a different stack frame by id. Queues a scopes request for the new frame. Requires: active debug session, stopped state.".into(),
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
            description: "Switch the active thread. Triggers a stack trace refresh for the new thread. Requires: active debug session.".into(),
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
            description: "Read recent debug output log lines. Returns JSON with output array and total line count. Requires: active debug session.".into(),
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
            description: "Evaluate an expression in the debuggee's context. Result arrives asynchronously — call dap_output after to see it. Requires: active debug session.".into(),
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
            description: "Disconnect from the debug adapter. Optionally terminate the debuggee process. Requires: active debug session.".into(),
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
    ]
}
