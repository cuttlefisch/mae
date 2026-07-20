use crate::types::*;

/// Tool tiers for payload optimization — only core tools are sent by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTier {
    /// Always sent (~15 tools). Essential for basic editing workflows.
    Core,
    /// Sent on request via `request_tools` meta-tool.
    Extended,
}

/// Tool categories for the `request_tools` meta-tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    Lsp,
    Dap,
    Knowledge,
    ShellMgmt,
    Commands,
    Git,
    Web,
    Ai,
    Visual,
    Debug,
    Mcp,
}

/// Classify a tool into Core or Extended tier.
pub fn classify_tool_tier(name: &str) -> ToolTier {
    match name {
        // Core tools — always sent
        "buffer_read"
        | "buffer_write"
        | "cursor_info"
        | "open_file"
        | "switch_buffer"
        | "create_file"
        | "close_buffer"
        | "list_buffers"
        | "editor_state"
        | "project_search"
        | "project_files"
        | "project_info"
        | "shell_exec"
        | "get_option"
        | "set_option"
        | "help_open"
        | "file_read"
        | "self_test_suite"
        | "introspect"
        | "perf_stats"
        | "perf_benchmark"
        | "window_layout"
        | "ai_permissions"
        | "input_lock"
        | "git_status"
        | "git_diff"
        | "git_log"
        | "org_cycle"
        | "org_todo_cycle"
        | "org_open_link"
        | "babel_execute"
        | "babel_tangle"
        | "org_export"
        | "kb_instances"
        | "kb_register"
        | "kb_unregister"
        | "kb_reimport"
        | "kb_search_context"
        | "kb_shortest_path"
        | "kb_neighborhood"
        | "kb_add_link"
        | "kb_raw_query"
        | "command_list"
        | "editor_save_state"
        | "editor_restore_state"
        | "github_pr_status"
        | "ask_user"
        | "rename_file"
        | "ai_save"
        | "ai_load"
        | "create_plan"
        | "update_plan"
        | "save_memory"
        | "debug_state"
        | "read_messages"
        | "syntax_tree"
        | "switch_project"
        | "toggle_file_tree"
        | "audit_configuration"
        | "list_modules"
        | "format_buffer"
        | "run_build"
        | "run_test"
        | "spell_check"
        | "lookup_online"
        | "next_error"
        | "search_tools"
        | "keymap_query" => ToolTier::Core,
        // Everything else is extended
        _ => ToolTier::Extended,
    }
}

/// Classify a tool into its category for request_tools.
pub fn classify_tool_category(name: &str) -> Option<ToolCategory> {
    if name.starts_with("mcp_") || name.starts_with("collab_") {
        return Some(ToolCategory::Mcp);
    }
    if name.starts_with("lsp_") || name == "syntax_tree" {
        Some(ToolCategory::Lsp)
    } else if name.starts_with("dap_") || name == "debug_state" {
        Some(ToolCategory::Dap)
    } else if name.starts_with("kb_")
        || name == "help_open"
        || name.starts_with("org_")
        || name.starts_with("babel_")
    {
        Some(ToolCategory::Knowledge)
    } else if name.starts_with("shell_") && name != "shell_exec" {
        Some(ToolCategory::ShellMgmt)
    } else if name.starts_with("command_") {
        Some(ToolCategory::Commands)
    } else if name.starts_with("git_") || name.starts_with("github_") {
        Some(ToolCategory::Git)
    } else if name.starts_with("web_") {
        Some(ToolCategory::Web)
    } else if name.starts_with("ai_") && name != "ai_permissions" {
        // ai_set_mode, ai_set_profile, ai_set_budget, ai_save, ai_load
        Some(ToolCategory::Ai)
    } else if name.starts_with("visual_buffer_") {
        Some(ToolCategory::Visual)
    } else if matches!(
        name,
        "delegate"
            | "save_memory"
            | "create_plan"
            | "update_plan"
            | "ask_user"
            | "log_activity"
            | "read_transcript"
            | "propose_changes"
    ) {
        Some(ToolCategory::Ai)
    } else if matches!(
        name,
        "theme_inspect" | "mouse_event" | "render_inspect" | "event_recording" | "trigger_hook"
    ) {
        Some(ToolCategory::Debug)
    } else {
        None
    }
}

/// Parse category names from a comma-separated string.
pub fn parse_categories(input: &str) -> Vec<ToolCategory> {
    input
        .split(',')
        .filter_map(|s| match s.trim().to_ascii_lowercase().as_str() {
            "lsp" => Some(ToolCategory::Lsp),
            "dap" => Some(ToolCategory::Dap),
            "knowledge" | "kb" => Some(ToolCategory::Knowledge),
            "shell" | "shell_mgmt" => Some(ToolCategory::ShellMgmt),
            "commands" | "command" => Some(ToolCategory::Commands),
            "git" | "github" => Some(ToolCategory::Git),
            "web" => Some(ToolCategory::Web),
            "ai" | "agent" => Some(ToolCategory::Ai),
            "visual" | "canvas" => Some(ToolCategory::Visual),
            "debug" | "profiling" => Some(ToolCategory::Debug),
            "mcp" | "external" => Some(ToolCategory::Mcp),
            _ => None,
        })
        .collect()
}

/// Build the `request_tools` meta-tool definition.
pub fn request_tools_definition() -> ToolDefinition {
    super::tool_def::ToolDefBuilder::new(
        "request_tools",
        "Request additional tools by category or specific name. Use search_tools first to discover tool names, then request them here. Categories: lsp, dap, knowledge, shell, commands, git, web, ai, visual, debug, mcp.",
    )
    .prop(
        "categories",
        "string",
        "Comma-separated categories: lsp, dap, knowledge, shell, commands, git, web, ai, visual, debug, mcp",
    )
    .prop("tools", "string", "Comma-separated tool names to add (e.g. from search_tools results)")
    .required(["categories"])
    .permission(PermissionTier::ReadOnly)
    .build()
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
