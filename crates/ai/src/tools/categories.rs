use std::collections::HashMap;

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
}

/// Classify a tool into Core or Extended tier.
pub fn classify_tool_tier(name: &str) -> ToolTier {
    match name {
        // Core tools — always sent
        "buffer_read" | "buffer_write" | "cursor_info" | "open_file" | "switch_buffer"
        | "create_file" | "close_buffer" | "list_buffers" | "editor_state" | "project_search"
        | "project_files" | "project_info" | "shell_exec" | "get_option" | "set_option"
        | "help_open" | "file_read" | "self_test_suite" | "introspect" | "perf_stats"
        | "perf_benchmark" | "window_layout" | "ai_permissions" | "input_lock" | "git_status"
        | "git_diff" | "git_log" | "org_cycle" | "org_todo_cycle" | "org_open_link" => {
            ToolTier::Core
        }
        // Everything else is extended
        _ => ToolTier::Extended,
    }
}

/// Classify a tool into its category for request_tools.
pub fn classify_tool_category(name: &str) -> Option<ToolCategory> {
    if name.starts_with("lsp_") || name == "syntax_tree" {
        Some(ToolCategory::Lsp)
    } else if name.starts_with("dap_") || name == "debug_state" {
        Some(ToolCategory::Dap)
    } else if name.starts_with("kb_") || name == "help_open" || name.starts_with("org_") {
        Some(ToolCategory::Knowledge)
    } else if name.starts_with("shell_") && name != "shell_exec" {
        Some(ToolCategory::ShellMgmt)
    } else if name.starts_with("command_") {
        Some(ToolCategory::Commands)
    } else if name.starts_with("git_") {
        Some(ToolCategory::Git)
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
            _ => None,
        })
        .collect()
}

/// Build the `request_tools` meta-tool definition.
pub fn request_tools_definition() -> ToolDefinition {
    ToolDefinition {
        name: "request_tools".into(),
        description: "Request additional tool categories: lsp, dap, knowledge, shell, commands. Returns tool names added.".into(),
        parameters: ToolParameters {
            schema_type: "object".into(),
            properties: HashMap::from([(
                "categories".into(),
                ToolProperty {
                    prop_type: "string".into(),
                    description: "Comma-separated categories: lsp, dap, knowledge, shell, commands".into(),
                    enum_values: None,
                },
            )]),
            required: vec!["categories".into()],
        },
        permission: Some(PermissionTier::ReadOnly),
    }
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
