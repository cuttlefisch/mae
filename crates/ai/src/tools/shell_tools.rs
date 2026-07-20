use crate::types::*;

use super::tool_def::ToolDefBuilder;

/// Shell, terminal, git, and GitHub tool definitions.
pub(super) fn shell_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefBuilder::new(
            "shell_exec",
            "Execute a shell command and return stdout/stderr. Anti-Looping: If a command fails, do not blindly retry the exact same command. Analyze the error, use diagnostics, or try a different approach. Workflow Hint: Use this for `git status`, running tests, or building the project. Always use this to verify bug fixes before reporting success. For PR status, follow up with `github_pr_status`.",
        )
        .prop("command", "string", "Shell command to execute")
        .prop("timeout_ms", "integer", "Timeout in milliseconds (default: 30000)")
        .required(["command"])
        .permission(PermissionTier::Shell)
        .build(),
        // --- Shell terminal tools ---
        ToolDefBuilder::new(
            "shell_list",
            "List all active shell terminal buffers with their names, buffer indices, and status (running/exited).",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "shell_read_output",
            "Read recent output from a shell terminal buffer's viewport. Returns the last N lines of visible terminal content.",
        )
        .prop("buffer_index", "integer", "Buffer index of the shell terminal")
        .prop("lines", "integer", "Number of lines to read (default: 24)")
        .required(["buffer_index"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "shell_send_input",
            "Send text input to a shell terminal buffer's PTY. Escape sequences: \\n or \\r for Enter, \\t for Tab, \\e for ESC.",
        )
        .prop("buffer_index", "integer", "Buffer index of the shell terminal")
        .prop(
            "input",
            "string",
            "Text to send to the terminal. Escapes: \\n/\\r=Enter, \\t=Tab, \\e=ESC",
        )
        .required(["buffer_index", "input"])
        .permission(PermissionTier::Shell)
        .build(),
        // --- Agent terminal tools ---
        ToolDefBuilder::new(
            "terminal_spawn",
            "Spawn a new interactive shell terminal buffer. Returns the buffer index. The terminal is visible to the user and supports long-running processes (compilers, servers).",
        )
        .prop("name", "string", "Optional name for the terminal buffer (e.g. '*build*')")
        .prop("command", "string", "Initial command to run in the terminal (optional)")
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "terminal_send",
            "Send input to a terminal spawned via terminal_spawn. Use for interactive prompts.",
        )
        .prop("buffer_index", "integer", "Buffer index of the terminal")
        .prop("input", "string", "Text to send (e.g. 'y\\n', 'Ctrl-C' via \\x03)")
        .required(["buffer_index", "input"])
        .permission(PermissionTier::Shell)
        .build(),
        ToolDefBuilder::new(
            "terminal_read",
            "Read the current screen content of a terminal. Returns the visible text (typically 24-80 lines).",
        )
        .prop("buffer_index", "integer", "Buffer index of the terminal")
        .required(["buffer_index"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "shell_scrollback",
            "Read text lines from a shell terminal's scrollback/viewport. Returns the cached viewport text for the given buffer.",
        )
        .prop(
            "buffer_index",
            "integer",
            "Buffer index of the shell terminal (default: active buffer)",
        )
        .prop("offset", "integer", "Lines from the bottom to start reading (default: 0)")
        .prop("lines", "integer", "Number of lines to return (default: 50)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        // --- Git tools ---
        ToolDefBuilder::new(
            "git_status",
            "Get structured git status: branch name, staged, unstaged, and untracked files. Does NOT provide PR (Pull Request) or CI information.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "git_diff",
            "Get git diff for the project or a specific path. Use staged=true to see staged changes.",
        )
        .prop("path", "string", "Optional file path to diff")
        .prop("staged", "boolean", "Show staged changes (default: false)")
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new("git_log", "Get git commit log. Returns oneline format.")
            .prop("path", "string", "Optional file path to show log for")
            .prop("limit", "integer", "Maximum number of commits to show (default: 10)")
            .permission(PermissionTier::ReadOnly)
            .build(),
        ToolDefBuilder::new("git_stage", "Stage files for commit (git add).")
            .prop("paths", "array", "List of file paths to stage")
            .required(["paths"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("git_unstage", "Unstage files (git reset).")
            .prop("paths", "array", "List of file paths to unstage")
            .required(["paths"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("git_commit", "Commit staged changes with a message.")
            .prop("message", "string", "Commit message")
            .required(["message"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("git_push", "Push commits to a remote repository.")
            .prop("remote", "string", "Remote name (default: 'origin')")
            .prop("branch", "string", "Branch name (default: current branch)")
            .permission(PermissionTier::Shell)
            .build(),
        ToolDefBuilder::new("git_pull", "Pull changes from a remote repository.")
            .prop("remote", "string", "Remote name (default: 'origin')")
            .prop("branch", "string", "Branch name (default: current branch)")
            .permission(PermissionTier::Shell)
            .build(),
        ToolDefBuilder::new("git_checkout", "Switch branches or create new ones.")
            .prop("branch", "string", "Branch name to checkout")
            .prop("create", "boolean", "Create the branch if it doesn't exist (default: false)")
            .required(["branch"])
            .permission(PermissionTier::Write)
            .build(),
        // --- Git stash & branch tools ---
        ToolDefBuilder::new(
            "git_stash_push",
            "Save working directory changes to the stash. Optionally provide a message.",
        )
        .prop("message", "string", "Optional stash message")
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "git_stash_pop",
            "Apply the most recent stash and remove it from the stash list.",
        )
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new("git_stash_list", "List all stashed changes.")
            .permission(PermissionTier::ReadOnly)
            .build(),
        ToolDefBuilder::new("git_branch_list", "List all local and remote branches.")
            .permission(PermissionTier::ReadOnly)
            .build(),
        ToolDefBuilder::new("git_branch_delete", "Delete a git branch.")
            .prop("branch", "string", "Branch name to delete")
            .prop(
                "force",
                "boolean",
                "Force delete even if not fully merged (default: false)",
            )
            .required(["branch"])
            .permission(PermissionTier::Write)
            .build(),
        ToolDefBuilder::new("git_merge", "Merge a branch into the current branch.")
            .prop("branch", "string", "Branch name to merge")
            .required(["branch"])
            .permission(PermissionTier::Write)
            .build(),
        // --- GitHub PR tools ---
        ToolDefBuilder::new(
            "github_pr_status",
            "Check the status of the current PR and its CI checks using the 'gh' CLI. Workflow Hint: Use this *after* confirming the local branch via `git status`. It fetches the remote PR link, review status, and CI checks.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "github_pr_create",
            "Create a new GitHub pull request using the 'gh' CLI.",
        )
        .prop("title", "string", "The PR title")
        .prop("body", "string", "The PR body content")
        .required(["title", "body"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "terminal_at_file",
            "Open a terminal at the directory containing a file. If no path given, uses the current buffer's file. Equivalent to 'cd <dir> && $SHELL'.",
        )
        .prop(
            "path",
            "string",
            "File path (terminal opens in its parent directory). Omit to use current buffer.",
        )
        .permission(PermissionTier::Shell)
        .build(),
    ]
}
