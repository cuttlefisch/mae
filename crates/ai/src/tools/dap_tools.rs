use crate::types::*;

use super::tool_def::ToolDefBuilder;

/// DAP tool definitions: start, breakpoints, stepping, variables, evaluation.
pub(super) fn dap_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefBuilder::new(
            "dap_start",
            "Start a debug session. Blocks until the session is ready and the debuggee stops (if stop_on_entry=true) or starts running. Returns JSON with session state including threads and stack frames. Requires: adapter binary on PATH.",
        )
        .prop_enum(
            "adapter",
            "string",
            "Adapter preset: 'lldb' (C/C++/Rust), 'debugpy' (Python), 'codelldb' (C/C++/Rust alt)",
            ["lldb", "debugpy", "codelldb"],
        )
        .prop("program", "string", "Path to the binary or script to debug")
        .prop("args", "array", "Program arguments (optional)")
        .prop_enum(
            "mode",
            "string",
            "'launch' (default) to start a new process, 'attach' to connect to an existing process by pid",
            ["launch", "attach"],
        )
        .prop("pid", "integer", "Process ID to attach to (required when mode='attach')")
        .prop(
            "stop_on_entry",
            "boolean",
            "Pause at program entry point before running (default: false). Use true when you need to set breakpoints before the program starts.",
        )
        .required(["adapter"])
        // Shell tier because launching programs under a
        // debug adapter is roughly equivalent to shell exec.
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "dap_set_breakpoint",
            "Set a breakpoint at source:line. Idempotent — no-op if already set. Works before or during a session; pending breakpoints are synced to the adapter on session start. Lines are 1-indexed. Supports optional condition and hit_condition for conditional breakpoints.",
        )
        .prop(
            "source",
            "string",
            "Source file path (matches the adapter's view — typically the same path the debugger sees)",
        )
        .prop("line", "integer", "1-indexed line number for the breakpoint")
        .prop(
            "condition",
            "string",
            "Optional condition expression — breakpoint only triggers when this evaluates to true (e.g. 'x > 5')",
        )
        .prop(
            "hit_condition",
            "string",
            "Optional hit count condition — breakpoint triggers on the Nth hit (e.g. '3' or '>= 10')",
        )
        .prop(
            "log_message",
            "string",
            "Optional log message (logpoint) — instead of stopping, logs a message. Expressions in {braces} are evaluated.",
        )
        .required(["source", "line"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_continue",
            "Resume execution. Blocks until the debuggee stops (breakpoint, exception) or terminates. Returns JSON with stopped state including thread, frame, and location. No need to call debug_state after this. Requires: active debug session.",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_step",
            "Step execution. Blocks until the step completes and debuggee stops. Returns JSON with new stopped state including thread, frame, and location. `direction`: 'over' (next line), 'in' (step into calls), 'out' (step out of frame). Requires: active debug session, stopped state.",
        )
        .prop_enum("direction", "string", "'over', 'in', or 'out'", ["over", "in", "out"])
        .required(["direction"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_inspect_variable",
            "Look up a single variable by name in the stopped frame's scopes. Returns JSON with name/value/type/scope/variables_reference. `variables_reference` > 0 means expandable children. Requires: active debug session, stopped state.",
        )
        .prop("name", "string", "Variable name to find")
        .prop(
            "scope",
            "string",
            "Optional scope name to restrict search (e.g. 'Locals', 'Globals'). Default: all scopes.",
        )
        .required(["name"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "dap_remove_breakpoint",
            "Remove a breakpoint at source:line. Returns remaining lines for that source. Requires: active debug session.",
        )
        .prop("source", "string", "Source file path")
        .prop("line", "integer", "1-indexed line number of breakpoint to remove")
        .required(["source", "line"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_list_variables",
            "List all variables in the current frame's scopes. Returns JSON mapping scope names to variable arrays with name/value/type/variables_reference. Requires: active debug session, stopped state.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "dap_expand_variable",
            "Request children of a nested variable by variables_reference. Call dap_list_variables after to see expanded results. Requires: active debug session, stopped state.",
        )
        .prop(
            "variables_reference",
            "integer",
            "The parent variable's variables_reference (must be > 0)",
        )
        .prop("scope", "string", "Scope name for the request (e.g. 'Locals')")
        .required(["variables_reference", "scope"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_select_frame",
            "Switch to a different stack frame by id. Queues a scopes request for the new frame. Requires: active debug session, stopped state.",
        )
        .prop("frame_id", "integer", "Stack frame id to select")
        .required(["frame_id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_select_thread",
            "Switch the active thread. Triggers a stack trace refresh for the new thread. Requires: active debug session.",
        )
        .prop("thread_id", "integer", "Thread id to switch to")
        .required(["thread_id"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_output",
            "Read recent debug output log lines. Returns JSON with output array and total line count. Requires: active debug session.",
        )
        .prop("lines", "integer", "Number of recent lines to return (default 50)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "dap_evaluate",
            "Evaluate an expression in the debuggee's context. Result arrives asynchronously — call dap_output after to see it. Requires: active debug session.",
        )
        .prop("expression", "string", "Expression to evaluate in the debuggee")
        .prop(
            "frame_id",
            "integer",
            "Optional stack frame id for evaluation context (default: topmost frame)",
        )
        .prop_enum(
            "context",
            "string",
            "Evaluation context: 'watch', 'repl', or 'hover' (default: 'repl')",
            ["watch", "repl", "hover"],
        )
        .required(["expression"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "dap_disconnect",
            "Disconnect from the debug adapter. Optionally terminate the debuggee process. Requires: active debug session.",
        )
        .prop(
            "terminate_debuggee",
            "boolean",
            "If true, also terminate the debugged process (default: false — detach only)",
        )
        .permission(PermissionTier::Write)
        .build(),
    ]
}
