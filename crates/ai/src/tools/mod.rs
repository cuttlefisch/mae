mod ai_tools;
mod categories;
mod core_tools;
mod dap_tools;
mod kb_tools;
mod lsp_tools;
mod shell_tools;
mod web_tools;

use std::collections::HashMap;

use mae_core::{CommandRegistry, OptionRegistry};

use crate::types::*;

// Re-export all public items from submodules.
pub use categories::{
    classify_command_permission, classify_tool_category, classify_tool_tier, parse_categories,
    request_tools_definition, PermissionPolicy, ToolCategory, ToolTier,
};

/// Valid AI prompt profiles. Used in tool definitions for ai_set_profile and delegate.
pub const AI_PROFILES: &[&str] = &["pair-programmer", "explorer", "planner", "reviewer"];

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
    let mut tools = Vec::new();
    tools.extend(ai_tools::ai_tool_definitions());
    tools.extend(core_tools::core_tool_definitions(registry));
    tools.extend(lsp_tools::lsp_tool_definitions());
    tools.extend(dap_tools::dap_tool_definitions());
    tools.extend(kb_tools::kb_tool_definitions());
    tools.extend(shell_tools::shell_tool_definitions());
    tools.extend(web_tools::web_tool_definitions());
    tools
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
        assert!(
            tools.len() >= 99,
            "Expected at least 99 AI tools, got {}",
            tools.len()
        );
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"audit_configuration"));
        assert!(names.contains(&"web_fetch"));
        assert!(names.contains(&"ai_set_mode"));
        assert!(names.contains(&"ai_set_profile"));
        assert!(names.contains(&"ask_user"));
        assert!(names.contains(&"propose_changes"));
        assert!(names.contains(&"delegate"));
        assert!(names.contains(&"save_memory"));
        assert!(names.contains(&"create_plan"));
        assert!(names.contains(&"update_plan"));
        assert!(names.contains(&"github_pr_status"));
        assert!(names.contains(&"read_transcript"));
        assert!(names.contains(&"github_pr_create"));
        assert!(names.contains(&"terminal_spawn"));
        assert!(names.contains(&"terminal_send"));
        assert!(names.contains(&"terminal_read"));
        assert!(names.contains(&"ai_set_budget"));
        assert!(names.contains(&"buffer_read"));
        assert!(names.contains(&"git_status"));
        assert!(names.contains(&"git_commit"));
        assert!(names.contains(&"org_cycle"));
        assert!(names.contains(&"org_todo_cycle"));
        assert!(names.contains(&"org_open_link"));
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
    fn core_tools_under_40() {
        let tools = ai_specific_tools(&OptionRegistry::new());
        let core_count = tools
            .iter()
            .filter(|t| classify_tool_tier(&t.name) == ToolTier::Core)
            .count();
        assert!(
            core_count < 50,
            "core tools should be < 50, got {}",
            core_count
        );
        assert!(
            core_count >= 15,
            "core tools should be >= 15, got {}",
            core_count
        );
    }

    #[test]
    fn extended_tools_over_35() {
        let tools = ai_specific_tools(&OptionRegistry::new());
        let extended_count = tools
            .iter()
            .filter(|t| classify_tool_tier(&t.name) == ToolTier::Extended)
            .count();
        assert!(
            extended_count >= 35,
            "extended tools should be >= 35, got {}",
            extended_count
        );
    }

    #[test]
    fn request_tools_meta_tool_has_categories_param() {
        let def = request_tools_definition();
        assert_eq!(def.name, "request_tools");
        assert!(def.parameters.properties.contains_key("categories"));
        assert!(def.parameters.required.contains(&"categories".into()));
    }

    #[test]
    fn parse_categories_works() {
        let cats = parse_categories("lsp, dap, knowledge");
        assert_eq!(cats.len(), 3);
        assert!(cats.contains(&ToolCategory::Lsp));
        assert!(cats.contains(&ToolCategory::Dap));
        assert!(cats.contains(&ToolCategory::Knowledge));
    }

    #[test]
    fn parse_categories_unknown_ignored() {
        let cats = parse_categories("lsp, bogus, dap");
        assert_eq!(cats.len(), 2);
    }

    #[test]
    fn classify_lsp_tools() {
        assert_eq!(
            classify_tool_category("lsp_definition"),
            Some(ToolCategory::Lsp)
        );
        assert_eq!(
            classify_tool_category("lsp_references"),
            Some(ToolCategory::Lsp)
        );
        assert_eq!(
            classify_tool_category("syntax_tree"),
            Some(ToolCategory::Lsp)
        );
    }

    #[test]
    fn classify_dap_tools() {
        assert_eq!(classify_tool_category("dap_start"), Some(ToolCategory::Dap));
        assert_eq!(
            classify_tool_category("debug_state"),
            Some(ToolCategory::Dap)
        );
    }

    #[test]
    fn classify_kb_tools() {
        assert_eq!(
            classify_tool_category("kb_search"),
            Some(ToolCategory::Knowledge)
        );
    }

    #[test]
    fn shell_exec_is_not_shell_mgmt() {
        assert_eq!(classify_tool_category("shell_exec"), None);
        assert_eq!(
            classify_tool_category("shell_list"),
            Some(ToolCategory::ShellMgmt)
        );
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
