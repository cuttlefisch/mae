use std::collections::HashMap;

use mae_core::CommandRegistry;

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
pub fn ai_specific_tools() -> Vec<ToolDefinition> {
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
    ]
}

/// Classify a command's permission tier based on its name.
pub fn classify_command_permission(name: &str) -> PermissionTier {
    match name {
        // Movement and read-only state changes
        n if n.starts_with("move-") => PermissionTier::ReadOnly,
        "enter-normal-mode" | "enter-insert-mode" | "enter-command-mode"
        | "enter-insert-mode-after" | "enter-insert-mode-eol" => PermissionTier::ReadOnly,

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
                tool.name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
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
        let tools = ai_specific_tools();
        assert_eq!(tools.len(), 10);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"buffer_read"));
        assert!(names.contains(&"buffer_write"));
        assert!(names.contains(&"cursor_info"));
        assert!(names.contains(&"shell_exec"));
        assert!(names.contains(&"file_read"));
        assert!(names.contains(&"list_buffers"));
        assert!(names.contains(&"editor_state"));
        assert!(names.contains(&"window_layout"));
        assert!(names.contains(&"command_list"));
        assert!(names.contains(&"debug_state"));
    }

    #[test]
    fn classify_movement_is_readonly() {
        assert_eq!(classify_command_permission("move-up"), PermissionTier::ReadOnly);
        assert_eq!(classify_command_permission("move-down"), PermissionTier::ReadOnly);
        assert_eq!(classify_command_permission("move-to-line-start"), PermissionTier::ReadOnly);
    }

    #[test]
    fn classify_editing_is_write() {
        assert_eq!(classify_command_permission("delete-line"), PermissionTier::Write);
        assert_eq!(classify_command_permission("undo"), PermissionTier::Write);
        assert_eq!(classify_command_permission("save"), PermissionTier::Write);
    }

    #[test]
    fn classify_quit_is_privileged() {
        assert_eq!(classify_command_permission("quit"), PermissionTier::Privileged);
        assert_eq!(classify_command_permission("force-quit"), PermissionTier::Privileged);
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
