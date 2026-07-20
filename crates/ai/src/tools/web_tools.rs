use crate::types::*;

use super::tool_def::ToolDefBuilder;

pub(super) fn web_tool_definitions() -> Vec<ToolDefinition> {
    vec![ToolDefBuilder::new(
        "web_fetch",
        "Fetch a URL and return its text content. HTML is converted to readable text. Response truncated to 32KB.",
    )
    .prop("url", "string", "The URL to fetch (http or https)")
    .required(["url"])
    .permission(PermissionTier::Shell)
    .build()]
}
