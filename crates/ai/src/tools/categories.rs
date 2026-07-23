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

/// Mechanically derive MCP tool-annotation hints from a tool's
/// `PermissionTier` (ADR-050 D2). Returns `(read_only_hint, destructive_hint,
/// idempotent_hint)`. This is the single source of truth for the mapping --
/// never hand-author a tool's annotations elsewhere, since doing so per tool
/// across 700+ registered tools would be an unauditable drift risk (a false
/// `read_only_hint: true` on a mutating tool would make external clients
/// like VS Code's Copilot skip their own confirmation dialog on a real
/// write). `ReadOnly` tools are read-only and idempotent by construction;
/// `Write` tools mutate but are ordinary, reversible editing operations;
/// `Shell`/`Privileged` tools can perform effects MAE cannot reason about or
/// undo (arbitrary shell commands, host filesystem/network access), so both
/// are flagged destructive.
pub fn annotations_for_tier(tier: PermissionTier) -> (bool, bool, bool) {
    match tier {
        PermissionTier::ReadOnly => (true, false, true),
        PermissionTier::Write => (false, false, false),
        PermissionTier::Shell => (false, true, false),
        PermissionTier::Privileged => (false, true, false),
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
mod annotation_tests {
    use super::*;

    #[test]
    fn read_only_tier_is_read_only_and_idempotent_never_destructive() {
        let (read_only, destructive, idempotent) = annotations_for_tier(PermissionTier::ReadOnly);
        assert!(read_only);
        assert!(!destructive);
        assert!(idempotent);
    }

    #[test]
    fn write_tier_is_neither_read_only_nor_flagged_destructive() {
        let (read_only, destructive, idempotent) = annotations_for_tier(PermissionTier::Write);
        assert!(!read_only);
        assert!(!destructive);
        assert!(!idempotent);
    }

    #[test]
    fn shell_and_privileged_tiers_are_flagged_destructive_never_read_only() {
        for tier in [PermissionTier::Shell, PermissionTier::Privileged] {
            let (read_only, destructive, _) = annotations_for_tier(tier);
            assert!(!read_only, "{tier:?} must never be read_only_hint: true");
            assert!(
                destructive,
                "{tier:?} must be flagged destructive_hint: true"
            );
        }
    }

    /// Exhaustive consistency check across every `PermissionTier` variant:
    /// `read_only_hint` must be true if and only if the tier is `ReadOnly`.
    /// This is what makes the mapping in `annotations_for_tier` a genuine
    /// single source of truth rather than something that could silently
    /// drift from `PermissionTier` if a variant is ever added -- add the new
    /// variant to this array and the compiler/test forces the mapping to be
    /// considered.
    #[test]
    fn read_only_hint_is_exactly_read_only_tier() {
        for tier in [
            PermissionTier::ReadOnly,
            PermissionTier::Write,
            PermissionTier::Shell,
            PermissionTier::Privileged,
        ] {
            let (read_only, _, _) = annotations_for_tier(tier);
            assert_eq!(
                read_only,
                tier == PermissionTier::ReadOnly,
                "read_only_hint mismatch for {tier:?}"
            );
        }
    }
}
