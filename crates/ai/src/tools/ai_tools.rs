use crate::types::*;

use super::tool_def::ToolDefBuilder;
use super::AI_PROFILES;

/// AI-specific meta tools: mode, profile, budget, delegate, memory, plans, permissions.
pub(super) fn ai_tool_definitions() -> Vec<ToolDefinition> {
    vec![
        ToolDefBuilder::new(
            "ai_set_mode",
            "Switch the AI operating mode. 'standard' requires manual approval for edits, 'plan' focuses on drafting architectural changes without touching code, 'auto-accept' enables hands-free execution for small tasks. Workflow Hint: Switch to 'plan' mode when drafting complex architectural changes to ensure safety. Switch to 'standard' once the plan is approved.",
        )
        .prop_enum(
            "mode",
            "string",
            "New AI mode: 'standard', 'plan', 'auto-accept'",
            ["standard", "plan", "auto-accept"],
        )
        .required(["mode"])
        .permission(PermissionTier::Privileged)
        .build(),
        ToolDefBuilder::new(
            "ai_set_profile",
            "Switch the active AI prompt profile. Each profile has a different persona and specialized tool instructions.",
        )
        .prop_enum(
            "profile",
            "string",
            format!("New AI profile: {}", AI_PROFILES.join(", ")),
            AI_PROFILES.iter().copied(),
        )
        .required(["profile"])
        .permission(PermissionTier::Privileged)
        .build(),
        ToolDefBuilder::new(
            "ai_set_budget",
            "Set the session budget guardrails (USD). 'warn' emits a one-shot warning, 'cap' terminates the session turn once reached. Set to 0 to disable a guardrail.",
        )
        .prop("warn", "number", "New session warning threshold in USD")
        .prop("cap", "number", "New session hard cap in USD")
        .permission(PermissionTier::Privileged)
        .build(),
        // --- Agent Orchestration & Memory ---
        ToolDefBuilder::new(
            "delegate",
            "Spawn a specialized sub-agent for a specific sub-task (e.g. 'explorer' for code mapping, 'planner' for drafting changes). The sub-agent has a separate context but shares the session budget. Workflow Hint: Use this aggressively to offload high-volume codebase exploration or repetitive batch tasks, keeping the main context lean.",
        )
        .prop_enum(
            "profile",
            "string",
            format!("The prompt profile for the sub-agent (e.g. {}).", AI_PROFILES.join(", ")),
            AI_PROFILES.iter().copied(),
        )
        .prop("objective", "string", "The specific goal for the sub-agent.")
        .required(["profile", "objective"])
        .permission(PermissionTier::ReadOnly)
        .build(),
        ToolDefBuilder::new(
            "save_memory",
            "Persist a fact, project convention, or finding to the project's long-term memory. This information will be available to future sessions and sub-agents.",
        )
        .prop("fact", "string", "The concise fact to remember.")
        .required(["fact"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "create_plan",
            "Create a new implementation plan in the project's plans directory. Plans should be markdown files documenting complex tasks. Workflow Hint: Use this after exploring the codebase for a complex task, but BEFORE making any file edits. Present the plan to the user for approval.",
        )
        .prop("name", "string", "The name of the plan (e.g. 'feature-x')")
        .prop("content", "string", "The initial markdown content of the plan.")
        .required(["name", "content"])
        .permission(PermissionTier::Write)
        .build(),
        ToolDefBuilder::new(
            "update_plan",
            "Update an existing implementation plan. Use this to refine steps as the task progresses.",
        )
        .prop("name", "string", "The name of the plan to update.")
        .prop("content", "string", "The updated markdown content of the plan.")
        .required(["name", "content"])
        .permission(PermissionTier::Write)
        .build(),
        // --- Permission introspection ---
        ToolDefBuilder::new(
            "ai_permissions",
            "Show the current AI permission tier and what each tier allows. Returns the auto-approved tier, available tiers with descriptions, and agent trust configuration status.",
        )
        .permission(PermissionTier::ReadOnly)
        .build(),
    ]
}
