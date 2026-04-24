use std::collections::HashMap;

use crate::types::*;

use super::AI_PROFILES;

/// AI-specific meta tools: mode, profile, budget, delegate, memory, plans, permissions.
pub(super) fn ai_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: "ai_set_mode".into(),
            description: "Switch the AI operating mode. 'standard' requires manual approval for edits, 'plan' focuses on drafting architectural changes without touching code, 'auto-accept' enables hands-free execution for small tasks. Workflow Hint: Switch to 'plan' mode when drafting complex architectural changes to ensure safety. Switch to 'standard' once the plan is approved.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "mode".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "New AI mode: 'standard', 'plan', 'auto-accept'".into(),
                        enum_values: Some(vec!["standard".into(), "plan".into(), "auto-accept".into()]),
                    },
                )]),
                required: vec!["mode".into()],
            },
            permission: Some(PermissionTier::Privileged),
        },
        ToolDefinition {
            name: "ai_set_profile".into(),
            description: "Switch the active AI prompt profile. Each profile has a different persona and specialized tool instructions.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "profile".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: format!("New AI profile: {}", AI_PROFILES.join(", ")),
                        enum_values: Some(AI_PROFILES.iter().map(|s| s.to_string()).collect()),
                    },
                )]),
                required: vec!["profile".into()],
            },
            permission: Some(PermissionTier::Privileged),
        },
        ToolDefinition {
            name: "ai_set_budget".into(),
            description: "Set the session budget guardrails (USD). 'warn' emits a one-shot warning, 'cap' terminates the session turn once reached. Set to 0 to disable a guardrail.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "warn".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "New session warning threshold in USD".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "cap".into(),
                        ToolProperty {
                            prop_type: "number".into(),
                            description: "New session hard cap in USD".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec![],
            },
            permission: Some(PermissionTier::Privileged),
        },
        // --- Agent Orchestration & Memory ---
        ToolDefinition {
            name: "delegate".into(),
            description: "Spawn a specialized sub-agent for a specific sub-task (e.g. 'explorer' for code mapping, 'planner' for drafting changes). The sub-agent has a separate context but shares the session budget. Workflow Hint: Use this aggressively to offload high-volume codebase exploration or repetitive batch tasks, keeping the main context lean.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "profile".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: format!("The prompt profile for the sub-agent (e.g. {}).", AI_PROFILES.join(", ")),
                            enum_values: Some(AI_PROFILES.iter().map(|s| s.to_string()).collect()),
                        },
                    ),
                    (
                        "objective".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The specific goal for the sub-agent.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["profile".into(), "objective".into()],
            },
            permission: Some(PermissionTier::ReadOnly),
        },
        ToolDefinition {
            name: "save_memory".into(),
            description: "Persist a fact, project convention, or finding to the project's long-term memory. This information will be available to future sessions and sub-agents.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([(
                    "fact".into(),
                    ToolProperty {
                        prop_type: "string".into(),
                        description: "The concise fact to remember.".into(),
                        enum_values: None,
                    },
                )]),
                required: vec!["fact".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "create_plan".into(),
            description: "Create a new implementation plan in the project's plans directory. Plans should be markdown files documenting complex tasks. Workflow Hint: Use this after exploring the codebase for a complex task, but BEFORE making any file edits. Present the plan to the user for approval.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The name of the plan (e.g. 'feature-x')".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "content".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The initial markdown content of the plan.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["name".into(), "content".into()],
            },
            permission: Some(PermissionTier::Write),
        },
        ToolDefinition {
            name: "update_plan".into(),
            description: "Update an existing implementation plan. Use this to refine steps as the task progresses.".into(),
            parameters: ToolParameters {
                schema_type: "object".into(),
                properties: HashMap::from([
                    (
                        "name".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The name of the plan to update.".into(),
                            enum_values: None,
                        },
                    ),
                    (
                        "content".into(),
                        ToolProperty {
                            prop_type: "string".into(),
                            description: "The updated markdown content of the plan.".into(),
                            enum_values: None,
                        },
                    ),
                ]),
                required: vec!["name".into(), "content".into()],
            },
            permission: Some(PermissionTier::Write),
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
    ]
}
