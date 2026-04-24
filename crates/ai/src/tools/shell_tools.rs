use std::collections::HashMap;

use crate::types::*;

/// Shell, terminal, git, and GitHub tool definitions.
pub(super) fn shell_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "shell_exec".into(),
            description: "Execute a shell command and return stdout/stderr. Anti-Looping: If a command fails, do not blindly retry the exact same command. Analyze the error, use diagnostics, or try a different approach. Workflow Hint: Use this for `git status`, running tests, or building the project. Always use this to verify bug fixes before reporting success. For PR status, follow up with `github_pr_status`.".into(),
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
        // --- Agent terminal tools ---
        ToolDefinition {
            name: "terminal_spawn".into(),
            description: "Spawn a new interactive shell terminal buffer. Returns the buffer index. The terminal is visible to the user and supports long-running processes (compilers, servers).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional name for the terminal buffer (e.g. '*build*')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "command".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Initial command to run in the terminal (optional)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "terminal_send".into(),
            description: "Send input to a terminal spawned via terminal_spawn. Use for interactive prompts.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "buffer_index".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Buffer index of the terminal".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "input".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Text to send (e.g. 'y\\n', 'Ctrl-C' via \\x03)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["buffer_index".into(), "input".into()],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "terminal_read".into(),
            description: "Read the current screen content of a terminal. Returns the visible text (typically 24-80 lines).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "buffer_index".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Buffer index of the terminal".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["buffer_index".into()],
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
        // --- Git tools ---
        ToolDefinition {
            name: "git_status".into(),
            description: "Get structured git status: branch name, staged, unstaged, and untracked files. Does NOT provide PR (Pull Request) or CI information.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "git_diff".into(),
            description: "Get git diff for the project or a specific path. Use staged=true to see staged changes.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "path".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional file path to diff".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "staged".into(),
                        ToolProperty {
                            prop_type: "boolean".into(),
                            description: "Show staged changes (default: false)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "git_log".into(),
            description: "Get git commit log. Returns oneline format.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "path".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Optional file path to show log for".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "limit".into(),
                        ToolProperty {
                            prop_type: "integer".into(),
                            description: "Maximum number of commits to show (default: 10)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "git_stage".into(),
            description: "Stage files for commit (git add).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "paths".into(),
                    ToolProperty {
                        prop_type: "array".into(),
                        description: "List of file paths to stage".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["paths".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "git_unstage".into(),
            description: "Unstage files (git reset).".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "paths".into(),
                    ToolProperty {
                        prop_type: "array".into(),
                        description: "List of file paths to unstage".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["paths".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "git_commit".into(),
            description: "Commit staged changes with a message.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "message".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Commit message".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["message".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "git_push".into(),
            description: "Push commits to a remote repository.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "remote".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Remote name (default: 'origin')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "branch".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Branch name (default: current branch)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "git_pull".into(),
            description: "Pull changes from a remote repository.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "remote".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Remote name (default: 'origin')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "branch".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Branch name (default: current branch)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::Shell),
        },
        ToolDefinition {
            name: "git_checkout".into(),
            description: "Switch branches or create new ones.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "branch".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Branch name to checkout".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "create".into(),
                        ToolProperty {
                            prop_type: "boolean".into(),
                            description: "Create the branch if it doesn't exist (default: false)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["branch".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- Git stash & branch tools ---
        ToolDefinition {
            name: "git_stash_push".into(),
            description: "Save working directory changes to the stash. Optionally provide a message.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "message".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Optional stash message".into(),
                        enum_values: None,
                    },
                )]),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "git_stash_pop".into(),
            description: "Apply the most recent stash and remove it from the stash list.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "git_stash_list".into(),
            description: "List all stashed changes.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "git_branch_list".into(),
            description: "List all local and remote branches.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "git_branch_delete".into(),
            description: "Delete a git branch.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "branch".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "Branch name to delete".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "force".into(),
                        ToolProperty {
                            prop_type: "boolean".into(),
                            description: "Force delete even if not fully merged (default: false)".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["branch".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "git_merge".into(),
            description: "Merge a branch into the current branch.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "branch".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "Branch name to merge".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["branch".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        // --- GitHub PR tools ---
        ToolDefinition {
            name: "github_pr_status".into(),
            description: "Check the status of the current PR and its CI checks using the 'gh' CLI. Workflow Hint: Use this *after* confirming the local branch via `git status`. It fetches the remote PR link, review status, and CI checks.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "github_pr_create".into(),
            description: "Create a new GitHub pull request using the 'gh' CLI.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "title".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The PR title".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "body".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The PR body content".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["title".into(), "body".into()],
            },
            permission: Some(PermissionTier::Write),
        },
    ]
}
