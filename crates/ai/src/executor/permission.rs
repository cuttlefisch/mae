//! Permission tier formatting and capability checks.

use crate::tools::PermissionPolicy;
use crate::types::PermissionTier;

pub(crate) fn format_permissions_info(policy: &PermissionPolicy) -> String {
    let tier_name = match policy.auto_approve_up_to {
        PermissionTier::ReadOnly => "readonly",
        PermissionTier::Write => "standard",
        PermissionTier::Shell => "trusted",
        PermissionTier::Privileged => "full",
    };

    format!(
        "Current auto-approve tier: {tier_name}\n\n\
         Permission tiers (lowest to highest):\n\
         - readonly: Read buffer contents, cursor state, file listings, project search\n\
         - standard: Modify buffers, edit files, save, undo/redo\n\
         - trusted: Execute shell commands (default)\n\
         - full: Quit editor, modify config, privileged operations\n\n\
         Tools at or below the '{tier_name}' tier run without prompting.\n\
         Configure via MAE_AI_PERMISSIONS env var or [ai] auto_approve_tier in config.toml.\n\
         Agent tool approval (MCP) is separate — see [agents] auto_approve_tools in config.toml."
    )
}
